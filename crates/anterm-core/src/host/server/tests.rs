mod aggregate_tests {
    use super::super::dispatch::control_endpoint_display;
    use super::super::*;
    use crate::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};
    use crate::daemon::control::{ControlClient, ControlRequest, ReplyOk, server_started};
    use crate::host::local::LocalHost;
    use std::net::{TcpListener, TcpStream};

    struct ThreePanesOneAgent {
        panes: Vec<PaneInfo>,
    }

    impl PaneDirectory for ThreePanesOneAgent {
        fn pane_count(&self) -> u64 {
            3
        }

        fn panes(&self) -> Vec<PaneInfo> {
            self.panes.clone()
        }

        fn pane_procs(&self, pane_id: u64) -> crate::daemon::protocol::PaneProcs {
            crate::daemon::protocol::PaneProcs {
                procs: vec![crate::daemon::protocol::ProcEntry {
                    pid: 900 + pane_id as u32,
                    name: "node".into(),
                    depth: 0,
                    foreground: true,
                    ..Default::default()
                }],
                ports: vec![crate::daemon::protocol::PortEntry {
                    port: 3000,
                    pid: 900 + pane_id as u32,
                    name: "node".into(),
                    addr: "*".into(),
                }],
                probe: Default::default(),
                context: None,
            }
        }

        fn hibernate_pane(&self, _pane_id: u64) {}

        fn close_pane(&self, _pane_id: u64) {}

        fn agent_states(&self) -> Vec<PaneAgentState> {
            vec![PaneAgentState {
                pane_id: 7,
                agent: Some(CLIAgent::Claude),
                state: AgentSessionState {
                    status: AgentStatus::Working,
                    session_id: Some("sess-7".into()),
                    ..Default::default()
                },
            }]
        }
    }

    fn client_with(services: Services) -> ControlClient {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let _ = serve_with(stream, LocalHost::new(), services);
        });
        let sock = TcpStream::connect(addr).unwrap();
        ControlClient::over_tcp(
            sock,
            &ControlHello::host_rpc("tok", "test-host"),
            Box::new(|_| {}),
        )
        .unwrap()
    }

    #[test]
    fn status_answers_with_this_servers_facts() {
        let services = Services {
            panes: Some(Arc::new(ThreePanesOneAgent { panes: Vec::new() })),
            ..Services::none()
        };
        let client = client_with(services);

        let ReplyOk::Status(status) = client.call(ControlRequest::Status).unwrap() else {
            panic!("Status must answer with ReplyOk::Status");
        };
        assert_eq!(status.pid, std::process::id());
        assert_eq!(status.panes, 3);
        assert_eq!(status.control_version, CONTROL_VERSION);
        assert_eq!(
            status.protocol_version,
            crate::daemon::protocol::PROTOCOL_VERSION
        );
        assert_eq!(status.build, env!("CARGO_PKG_VERSION"));
        assert_eq!(
            status.socket,
            control_endpoint_display(),
            "the CLI dials whatever path Status names"
        );
        assert!(status.uptime_secs <= server_started().elapsed().as_secs());
    }

    #[test]
    fn agent_states_are_the_pane_directorys_snapshot() {
        let services = Services {
            panes: Some(Arc::new(ThreePanesOneAgent { panes: Vec::new() })),
            ..Services::none()
        };
        let client = client_with(services);

        let ReplyOk::AgentStates(states) = client.call(ControlRequest::AgentStates).unwrap() else {
            panic!("AgentStates must answer with ReplyOk::AgentStates");
        };
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].pane_id, 7);
        assert_eq!(states[0].agent, Some(CLIAgent::Claude));
        assert_eq!(states[0].state.status, AgentStatus::Working);
        assert_eq!(states[0].state.session_id.as_deref(), Some("sess-7"));
    }

    /// A remote workspace's pane runs on the peer, so the peer is the only
    /// one that can walk its process tree — the client's own daemon has never
    /// heard of the pane. Without this request its ports were simply invisible.
    #[test]
    fn a_peer_says_what_is_listening_inside_one_of_its_panes() {
        let services = Services {
            panes: Some(Arc::new(ThreePanesOneAgent { panes: Vec::new() })),
            ..Services::none()
        };
        let client = client_with(services);

        assert!(
            client.hello().has_feature(feature::PANE_PROCS),
            "a peer that serves panes has to say it can describe them"
        );
        let ReplyOk::PaneProcs(procs) = client
            .call(ControlRequest::PaneProcs { pane_id: 4 })
            .unwrap()
        else {
            panic!("PaneProcs must answer with ReplyOk::PaneProcs");
        };
        assert_eq!(procs.ports.len(), 1);
        assert_eq!(procs.ports[0].port, 3000);
        assert_eq!(
            procs.ports[0].pid, 904,
            "the answer is about the pane that was asked for"
        );
    }

    /// The feature is the client's only way to tell "nothing is listening"
    /// from "nobody here can tell you", and a process serving no panes is the
    /// second. Announcing it anyway would draw an empty Ports list over a
    /// question that was never answered.
    #[test]
    fn a_process_with_no_panes_does_not_claim_it_can_list_ports() {
        let client = client_with(Services::none());
        assert!(!client.hello().has_feature(feature::PANE_PROCS));
    }

    #[test]
    fn machine_get_overlays_live_pane_titles() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = MachineStore::open(dir.path().join(machine::MACHINE_FILE));
        store
            .tab_create(
                None,
                machine::PaneSeed {
                    pane: 7,
                    cwd: Some("/repo/anterm".into()),
                    ssh_spec: None,
                    agent: None,
                    shell: None,
                },
                None,
                None,
            )
            .unwrap();

        let services = Services {
            machine: Some(store),
            attachments: Arc::new(AttachRegistry::default()),
            panes: Some(Arc::new(ThreePanesOneAgent {
                panes: vec![PaneInfo {
                    pane_id: 7,
                    cwd: Some(PathBuf::from("/repo/anterm")),
                    title: "nvim".into(),
                    osc_title: Some("nvim ~/repo/anterm".into()),
                    alive: true,
                    owner: None,
                }],
            })),
        };
        let client = client_with(services);
        let ReplyOk::MachineTree(machine) = client.call(ControlRequest::MachineGet).unwrap() else {
            panic!("MachineGet must answer with a machine tree");
        };
        let pane = machine.panes.iter().find(|p| p.id == 7).unwrap();
        assert_eq!(pane.title, "nvim");
        assert_eq!(pane.osc_title.as_deref(), Some("nvim ~/repo/anterm"));
        assert!(pane.live);
    }

    #[test]
    fn aggregates_still_answer_when_this_process_serves_no_panes() {
        let client = client_with(Services::none());

        let ReplyOk::AgentStates(states) = client.call(ControlRequest::AgentStates).unwrap() else {
            panic!("AgentStates must answer with ReplyOk::AgentStates");
        };
        assert!(states.is_empty());

        let ReplyOk::Status(status) = client.call(ControlRequest::Status).unwrap() else {
            panic!("Status must answer with ReplyOk::Status");
        };
        assert_eq!(status.panes, 0);

        let ReplyOk::Routes(routes) = client.call(ControlRequest::Routes).unwrap() else {
            panic!("Routes must answer with ReplyOk::Routes");
        };
        assert!(
            routes.iter().all(|r| !r.key.is_empty()),
            "whatever links exist are named; none are blank"
        );
    }

    /// Records which panes were stopped for a sleeping tab, and in what order
    /// relative to the tree: each entry carries whether the tree already
    /// called the tab asleep when the pane was stopped.
    struct SleepRecorder {
        store: Arc<MachineStore>,
        stopped: std::sync::Mutex<Vec<(u64, bool)>>,
    }

    impl PaneDirectory for SleepRecorder {
        fn pane_count(&self) -> u64 {
            0
        }

        fn panes(&self) -> Vec<PaneInfo> {
            Vec::new()
        }

        fn pane_procs(&self, _pane_id: u64) -> crate::daemon::protocol::PaneProcs {
            Default::default()
        }

        fn agent_states(&self) -> Vec<PaneAgentState> {
            Vec::new()
        }

        fn hibernate_pane(&self, pane_id: u64) {
            let asleep = self
                .store
                .machine()
                .tabs
                .iter()
                .any(|t| t.hibernated && t.root.contains(pane_id));
            self.stopped.lock().unwrap().push((pane_id, asleep));
        }

        fn close_pane(&self, _pane_id: u64) {}
    }

    fn seed(pane: u64) -> machine::PaneSeed {
        machine::PaneSeed {
            pane,
            cwd: Some("/repo".into()),
            ssh_spec: None,
            agent: None,
            shell: None,
        }
    }

    /// Going to sleep stops every pane of the tab — after the tree says the
    /// tab is asleep, so nobody reading the tree in between takes the stopped
    /// panes for dead ones — and keeps them in the tree for the wake. Waking
    /// only clears the mark: the client brings the panes back.
    #[test]
    fn a_tab_put_to_sleep_stops_its_panes_and_keeps_them_in_the_tree() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = MachineStore::open(dir.path().join(machine::MACHINE_FILE));
        let tab = store.tab_create(None, seed(7), None, None).unwrap();
        store
            .pane_split(7, machine::Axis::Horizontal, 0.5, seed(8), false, None)
            .unwrap();
        let other = store.tab_create(None, seed(9), None, None).unwrap();
        let recorder = Arc::new(SleepRecorder {
            store: Arc::clone(&store),
            stopped: std::sync::Mutex::new(Vec::new()),
        });
        let client = client_with(Services {
            machine: Some(Arc::clone(&store)),
            attachments: Arc::new(AttachRegistry::default()),
            panes: Some(recorder.clone()),
        });
        assert!(client.hello().has_feature(feature::TAB_HIBERNATE));

        let ReplyOk::Panes(panes) = client
            .call(ControlRequest::TabSetHibernated {
                tab: tab.id,
                hibernated: true,
            })
            .unwrap()
        else {
            panic!("TabSetHibernated must answer with the tab's panes");
        };
        assert_eq!(panes, vec![7, 8]);
        assert_eq!(
            *recorder.stopped.lock().unwrap(),
            vec![(7, true), (8, true)],
            "both panes stopped, each after the tree marked the tab"
        );
        let machine = store.machine();
        let asleep = &machine.tabs;
        assert!(asleep.iter().find(|t| t.id == tab.id).unwrap().hibernated);
        assert!(!asleep.iter().find(|t| t.id == other.id).unwrap().hibernated);
        for pane in [7, 8] {
            assert!(
                machine.panes.iter().any(|p| p.id == pane),
                "pane {pane}'s record is what the wake restores from"
            );
        }

        recorder.stopped.lock().unwrap().clear();
        let ReplyOk::Panes(_) = client
            .call(ControlRequest::TabSetHibernated {
                tab: tab.id,
                hibernated: false,
            })
            .unwrap()
        else {
            panic!("TabSetHibernated must answer with the tab's panes");
        };
        assert!(recorder.stopped.lock().unwrap().is_empty());
        assert!(!store.machine().tabs[0].hibernated);
    }

    /// A peer with a tree and no panes could mark a tab asleep and leave its
    /// shells running. It does not offer to, and refuses if asked anyway —
    /// before the mark, so the tree is not left claiming a sleep that never
    /// happened.
    #[test]
    fn a_peer_serving_no_panes_cannot_put_a_tab_to_sleep() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = MachineStore::open(dir.path().join(machine::MACHINE_FILE));
        let tab = store.tab_create(None, seed(7), None, None).unwrap();
        let client = client_with(Services::with_machine(Arc::clone(&store)));
        assert!(client.hello().has_feature(feature::MACHINE_TREE));
        assert!(!client.hello().has_feature(feature::TAB_HIBERNATE));
        assert!(
            client
                .call(ControlRequest::TabSetHibernated {
                    tab: tab.id,
                    hibernated: true,
                })
                .is_err()
        );
        assert!(!store.machine().tabs[0].hibernated);
    }

    /// Records what a remembered close does to the panes: each stopped pane
    /// with whether the tree already listed it as a closed tab's at the time,
    /// and each pane ended for good.
    struct CloseRecorder {
        store: Arc<MachineStore>,
        stopped: std::sync::Mutex<Vec<(u64, bool)>>,
        ended: std::sync::Mutex<Vec<u64>>,
    }

    impl PaneDirectory for CloseRecorder {
        fn pane_count(&self) -> u64 {
            0
        }

        fn panes(&self) -> Vec<PaneInfo> {
            Vec::new()
        }

        fn pane_procs(&self, _pane_id: u64) -> crate::daemon::protocol::PaneProcs {
            Default::default()
        }

        fn agent_states(&self) -> Vec<PaneAgentState> {
            Vec::new()
        }

        fn hibernate_pane(&self, pane_id: u64) {
            let remembered = self
                .store
                .machine()
                .closed
                .iter()
                .any(|c| c.tab.root.contains(pane_id));
            self.stopped.lock().unwrap().push((pane_id, remembered));
        }

        fn close_pane(&self, pane_id: u64) {
            self.ended.lock().unwrap().push(pane_id);
        }
    }

    /// Closing to remember stops the tab's panes keeping their screens — after
    /// the entry is in the tree, so the sweeps never see them as nobody's —
    /// ends the pane the tree never had, and reopening answers the tab with the
    /// records it is rebuilt from, once.
    #[test]
    fn a_remembered_close_stops_the_panes_and_a_reopen_hands_the_tab_back() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = MachineStore::open(dir.path().join(machine::MACHINE_FILE));
        let tab = store.tab_create(None, seed(7), None, None).unwrap();
        store
            .pane_split(7, machine::Axis::Vertical, 0.5, seed(8), false, None)
            .unwrap();
        let recorder = Arc::new(CloseRecorder {
            store: Arc::clone(&store),
            stopped: std::sync::Mutex::new(Vec::new()),
            ended: std::sync::Mutex::new(Vec::new()),
        });
        let client = client_with(Services {
            machine: Some(Arc::clone(&store)),
            attachments: Arc::new(AttachRegistry::default()),
            panes: Some(recorder.clone()),
        });
        assert!(client.hello().has_feature(feature::CLOSED_TABS));

        let ReplyOk::Panes(stopped) = client
            .call(ControlRequest::TabCloseRemembered {
                tab: tab.id,
                panes: vec![7, 8, 11],
            })
            .unwrap()
        else {
            panic!("TabCloseRemembered must answer with the panes it stopped");
        };
        assert_eq!(stopped, vec![7, 8]);
        assert_eq!(
            *recorder.stopped.lock().unwrap(),
            vec![(7, true), (8, true)],
            "both stopped, each after the tree listed the tab as closed"
        );
        assert_eq!(*recorder.ended.lock().unwrap(), vec![11]);

        let ReplyOk::ReopenedTab(Some(reopened)) = client
            .call(ControlRequest::TabReopen { tab: None })
            .unwrap()
        else {
            panic!("TabReopen must answer with the closed tab");
        };
        assert_eq!(reopened.tab.id, tab.id);
        assert_eq!(reopened.tab.root.pane_ids(), vec![7, 8]);
        let cwds: Vec<_> = reopened.panes.iter().map(|p| p.cwd.as_deref()).collect();
        assert_eq!(cwds, vec![Some("/repo"), Some("/repo")]);

        let again = client
            .call(ControlRequest::TabReopen { tab: None })
            .unwrap();
        assert_eq!(again, ReplyOk::ReopenedTab(None));
    }

    /// Without panes there is nothing to keep a screen with, so the peer does
    /// not offer the list — a client then closes and reopens the old way — and
    /// refuses the close if asked anyway, leaving the tab where it was.
    #[test]
    fn a_peer_serving_no_panes_does_not_offer_closed_tabs() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = MachineStore::open(dir.path().join(machine::MACHINE_FILE));
        let tab = store.tab_create(None, seed(7), None, None).unwrap();
        let client = client_with(Services::with_machine(Arc::clone(&store)));
        assert!(!client.hello().has_feature(feature::CLOSED_TABS));
        assert!(
            client
                .call(ControlRequest::TabCloseRemembered {
                    tab: tab.id,
                    panes: vec![7],
                })
                .is_err()
        );
        let machine = store.machine();
        assert_eq!(machine.tabs.len(), 1);
        assert!(machine.closed.is_empty());
    }
}

mod pool_tests {
    use super::super::pool::{Job, PoolState};
    use super::super::*;
    use std::sync::Condvar;
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    #[test]
    fn the_pool_reuses_a_warm_worker() {
        let pool = Pool::new();

        let settled = || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                let st = pool.inner.state.lock().unwrap();
                if st.idle == st.workers {
                    return;
                }
                drop(st);
                std::thread::sleep(Duration::from_millis(1));
            }
            panic!("the pool never went idle");
        };

        for _ in 0..50 {
            let (tx, rx) = std::sync::mpsc::channel();
            assert!(pool.submit(move || {
                let _ = tx.send(());
            }));
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
            settled();
        }
        let st = pool.inner.state.lock().unwrap();
        assert_eq!(
            st.workers, 1,
            "50 sequential jobs should not want 50 threads"
        );
    }

    #[test]
    fn a_worker_is_spawned_when_the_backlog_outruns_the_parked_workers() {
        let state = |jobs: usize, workers: usize, idle: usize| PoolState {
            jobs: (0..jobs).map(|_| Box::new(|| ()) as Job).collect(),
            workers,
            idle,
            closed: false,
        };

        assert!(!state(1, 1, 1).wants_another_worker());

        assert!(state(2, 1, 1).wants_another_worker());

        assert!(state(1, 1, 0).wants_another_worker());

        assert!(!state(64, MAX_WORKERS, 0).wants_another_worker());
    }

    #[test]
    fn the_pool_grows_for_concurrent_work() {
        let pool = Pool::new();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        for _ in 0..8 {
            let gate = Arc::clone(&gate);
            let done_tx = done_tx.clone();
            assert!(pool.submit(move || {
                let (lock, cv) = &*gate;
                let mut open = lock.lock().unwrap();
                let _ = done_tx.send(());
                while !*open {
                    open = cv.wait(open).unwrap();
                }
            }));
        }
        for _ in 0..8 {
            done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        assert_eq!(pool.inner.state.lock().unwrap().workers, 8);

        let (lock, cv) = &*gate;
        *lock.lock().unwrap() = true;
        cv.notify_all();
        pool.close();
    }

    #[test]
    fn closing_the_pool_drops_queued_work() {
        let pool = Pool::new();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let ran = Arc::new(AtomicBool::new(false));

        let blocker = Arc::clone(&gate);
        assert!(pool.submit(move || {
            let (lock, cv) = &*blocker;
            let mut open = lock.lock().unwrap();
            while !*open {
                open = cv.wait(open).unwrap();
            }
        }));
        std::thread::sleep(Duration::from_millis(100));
        let ran2 = Arc::clone(&ran);
        pool.submit(move || ran2.store(true, Ordering::SeqCst));

        pool.close();
        let (lock, cv) = &*gate;
        *lock.lock().unwrap() = true;
        cv.notify_all();
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            !ran.load(Ordering::SeqCst),
            "a job queued at close still ran"
        );
        assert!(!pool.submit(|| {}), "a closed pool must refuse work");
    }
}

mod gui_registry_tests {
    use super::super::*;
    use std::io::Cursor;

    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn gui_open_is_delivered_only_while_a_gui_is_registered() {
        let registry = AttachRegistry::default();
        assert!(!registry.open_gui(Some("/work".into())));

        let bytes = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::new(Sink::new(SharedWriter(Arc::clone(&bytes))));
        registry.register_gui(7, sink);
        assert!(registry.open_gui(Some("/work".into())));

        let frame = bytes.lock().unwrap().clone();
        assert_eq!(
            ControlServerMsg::read(&mut Cursor::new(frame)).unwrap(),
            ControlServerMsg::Event(ControlEvent::GuiOpen {
                path: Some("/work".into()),
            })
        );

        registry.unregister_gui(7);
        assert!(!registry.open_gui(None));
    }
}

#[cfg(unix)]
mod tests {
    use super::super::dispatch::drop_unsendable_hits;
    use super::super::transport::sock;
    use super::super::*;
    use crate::daemon::control::{
        ControlHello, ControlRequest, MTime, ReplyOk, WireErrorKind, feature,
    };
    use crate::host::local::LocalHost;
    use crate::host::remote::RemoteHost;
    use crate::host::{Entry, HostId, Meta, Output, SearchHit};
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    struct Pair {
        host: Arc<RemoteHost>,
    }

    fn pair_with(server_host: SharedHost) -> Pair {
        let (server, client) = UnixStream::pair().unwrap();
        std::thread::spawn(move || {
            let _ = serve(server, server_host);
        });
        let hello = ControlHello::host_rpc("test-token", "test-host");
        let host = RemoteHost::over_unix(client, "test:pair", &hello).unwrap();
        Pair { host }
    }

    fn pair() -> Pair {
        pair_with(LocalHost::new())
    }

    fn raw() -> (UnixStream, ControlHelloOk) {
        raw_with(Services::none())
    }

    fn raw_with(services: Services) -> (UnixStream, ControlHelloOk) {
        let (server, mut client) = UnixStream::pair().unwrap();
        std::thread::spawn(move || {
            let _ = serve_with(server, LocalHost::new(), services);
        });
        ControlClientMsg::Hello(ControlHello::host_rpc("t", "h"))
            .encode(&mut client)
            .unwrap();
        client.flush().unwrap();
        let ok = match ControlServerMsg::read(&mut client).unwrap() {
            ControlServerMsg::HelloOk(ok) => ok,
            other => panic!("{other:?}"),
        };
        (client, ok)
    }

    fn machine_services() -> (Services, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let store = MachineStore::open(dir.path().join(machine::MACHINE_FILE));
        (Services::with_machine(store), dir)
    }

    struct SlowGit {
        inner: SharedHost,
        delay: Duration,
        running: Arc<AtomicBool>,
    }

    impl Host for SlowGit {
        fn id(&self) -> HostId {
            self.inner.id()
        }
        fn separator(&self) -> char {
            self.inner.separator()
        }
        fn is_absolute(&self, p: &Path) -> bool {
            self.inner.is_absolute(p)
        }
        fn read_dir(&self, dir: &Path, root: Option<&Path>) -> io::Result<Vec<Entry>> {
            self.inner.read_dir(dir, root)
        }
        fn stat(&self, p: &Path) -> io::Result<Meta> {
            self.inner.stat(p)
        }
        fn read_file(&self, p: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
            self.inner.read_file(p, max_bytes)
        }
        fn canonicalize(&self, p: &Path) -> io::Result<PathBuf> {
            self.inner.canonicalize(p)
        }
        fn search(
            &self,
            roots: &[PathBuf],
            query: &str,
            limit: usize,
            max_dirs: usize,
            show_hidden: bool,
        ) -> io::Result<Vec<SearchHit>> {
            self.inner
                .search(roots, query, limit, max_dirs, show_hidden)
        }
        fn search_content(
            &self,
            roots: &[PathBuf],
            query: &crate::host::ContentQuery,
            limits: &crate::host::ContentLimits,
        ) -> io::Result<crate::host::ContentResults> {
            self.inner.search_content(roots, query, limits)
        }
        fn write_file(&self, p: &Path, bytes: &[u8]) -> io::Result<Meta> {
            self.inner.write_file(p, bytes)
        }
        fn create_file_new(&self, p: &Path) -> io::Result<()> {
            self.inner.create_file_new(p)
        }
        fn create_dir(&self, p: &Path, recursive: bool) -> io::Result<()> {
            self.inner.create_dir(p, recursive)
        }
        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            self.inner.rename(from, to)
        }
        fn remove(&self, p: &Path, recursive: bool) -> io::Result<()> {
            self.inner.remove(p, recursive)
        }
        fn repo_root(&self, p: &Path) -> io::Result<Option<PathBuf>> {
            self.inner.repo_root(p)
        }
        fn git(&self, _cwd: &Path, _args: &[&str]) -> io::Result<Output> {
            self.running.store(true, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            Ok(Output {
                status: Some(0),
                stdout: b"slow".to_vec(),
                stderr: Vec::new(),
            })
        }
        fn shells(&self) -> io::Result<crate::host::ShellInventory> {
            self.inner.shells()
        }
        fn watch(&self, dirs: &[PathBuf]) -> io::Result<WatchSub> {
            self.inner.watch(dirs)
        }
    }

    #[test]
    fn a_slow_request_does_not_block_a_fast_one() {
        let running = Arc::new(AtomicBool::new(false));
        let p = pair_with(Arc::new(SlowGit {
            inner: LocalHost::new(),
            delay: Duration::from_millis(1500),
            running: Arc::clone(&running),
        }));

        let tmp = tempfile::TempDir::new().unwrap();
        let slow_host = Arc::clone(&p.host);
        let dir = tmp.path().to_path_buf();
        let slow = std::thread::spawn(move || slow_host.git(&dir, &["status"]));

        let deadline = Instant::now() + Duration::from_secs(5);
        while !running.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(running.load(Ordering::SeqCst), "the slow request never ran");

        let started = Instant::now();
        p.host.stat(tmp.path()).unwrap();
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(600),
            "a stat behind an in-flight git took {elapsed:?} — the server is serializing"
        );

        let out = slow.join().unwrap().unwrap();
        assert_eq!(out.stdout, b"slow");
    }

    #[test]
    fn many_slow_requests_still_leave_the_server_answering() {
        let running = Arc::new(AtomicBool::new(false));
        let p = pair_with(Arc::new(SlowGit {
            inner: LocalHost::new(),
            delay: Duration::from_millis(1200),
            running: Arc::clone(&running),
        }));
        let tmp = tempfile::TempDir::new().unwrap();

        let mut slow = Vec::new();
        for _ in 0..24 {
            let host = Arc::clone(&p.host);
            let dir = tmp.path().to_path_buf();
            slow.push(std::thread::spawn(move || {
                let _ = host.git(&dir, &["status"]);
            }));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while !running.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }

        let started = Instant::now();
        assert!(p.host.exists(tmp.path()));
        assert!(
            started.elapsed() < Duration::from_millis(800),
            "24 in-flight gits stalled a trivial request"
        );
        for t in slow {
            let _ = t.join();
        }
    }

    #[test]
    fn a_short_deadline_gives_up_on_a_slow_git() {
        let p = pair_with(Arc::new(SlowGit {
            inner: LocalHost::new(),
            delay: Duration::from_millis(300),
            running: Arc::new(AtomicBool::new(false)),
        }));
        let tmp = tempfile::TempDir::new().unwrap();

        let err = p
            .host
            .git_with_deadline(tmp.path(), &["status"], Duration::from_millis(100))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "{err}");
    }

    #[test]
    fn a_long_deadline_outlasts_the_one_a_git_request_would_get() {
        let p = pair_with(Arc::new(SlowGit {
            inner: LocalHost::new(),
            delay: Duration::from_millis(300),
            running: Arc::new(AtomicBool::new(false)),
        }));
        let tmp = tempfile::TempDir::new().unwrap();

        // The first call abandons its request mid-flight. The server has no
        // timer of its own — it runs the job to completion and answers into a
        // slot nobody is waiting on — so the point of doing it twice is that
        // the link is still usable afterwards. A `push` behind a cancelled
        // probe depends on exactly that.
        let _ = p
            .host
            .git_with_deadline(tmp.path(), &["status"], Duration::from_millis(100));
        let out = p
            .host
            .git_with_deadline(tmp.path(), &["status"], Duration::from_secs(5))
            .unwrap();
        assert_eq!(out.stdout, b"slow");
    }

    #[test]
    fn replies_come_back_out_of_order() {
        let running = Arc::new(AtomicBool::new(false));
        let p = pair_with(Arc::new(SlowGit {
            inner: LocalHost::new(),
            delay: Duration::from_millis(800),
            running: Arc::clone(&running),
        }));
        let tmp = tempfile::TempDir::new().unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        let host = Arc::clone(&p.host);
        let dir = tmp.path().to_path_buf();
        let slow_tx = tx.clone();
        let slow = std::thread::spawn(move || {
            let _ = host.git(&dir, &["status"]);
            let _ = slow_tx.send("git");
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !running.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        p.host.stat(tmp.path()).unwrap();
        tx.send("stat").unwrap();

        assert_eq!(
            rx.recv().unwrap(),
            "stat",
            "the later request answered first"
        );
        assert_eq!(rx.recv().unwrap(), "git");
        slow.join().unwrap();
    }

    #[test]
    fn the_handshake_describes_this_server() {
        let p = pair();
        let peer = p.host.peer();
        assert_eq!(peer.control_version, CONTROL_VERSION);
        assert_eq!(
            peer.protocol_version,
            crate::daemon::protocol::PROTOCOL_VERSION
        );
        assert_eq!(peer.separator, std::path::MAIN_SEPARATOR);
        assert!(peer.has_feature(feature::CONTROL));
        assert!(peer.has_feature(feature::HOST_RPC));
        assert!(peer.has_feature(feature::CONTENT_SEARCH));
        assert!(
            !peer.home.is_empty(),
            "the server's $HOME backs `new workspace in ~`"
        );
    }

    #[test]
    fn the_handshake_advertises_the_pane_daemons_features() {
        let p = pair();
        let peer = p.host.peer();
        for name in crate::daemon::protocol::DaemonVersion::current().features {
            assert!(
                peer.has_feature(&name),
                "the hello must carry the pane feature {name:?}: a pane \
                 connection answers one message, so this handshake is the \
                 only affordable place for a client to learn it"
            );
        }
        assert!(
            peer.has_feature(crate::daemon::protocol::FEATURE_RESIZE_ECHO),
            "the resize-echo deferral on remote routes hangs off this name"
        );
    }

    #[test]
    fn a_version_mismatch_is_answered_then_closed() {
        let (server, mut client) = UnixStream::pair().unwrap();
        std::thread::spawn(move || {
            let _ = serve(server, LocalHost::new());
        });

        let mut hello = ControlHello::host_rpc("t", "h");
        hello.control_version = CONTROL_VERSION + 7;
        ControlClientMsg::Hello(hello).encode(&mut client).unwrap();
        client.flush().unwrap();

        match ControlServerMsg::read(&mut client).unwrap() {
            ControlServerMsg::HelloOk(ok) => assert_eq!(ok.control_version, CONTROL_VERSION),
            other => panic!("{other:?}"),
        }
        let mut buf = [0u8; 1];
        assert_eq!(
            client.read(&mut buf).unwrap(),
            0,
            "the server should hang up"
        );
    }

    #[test]
    fn the_client_refuses_a_mismatched_peer() {
        let (server, client) = UnixStream::pair().unwrap();
        std::thread::spawn(move || {
            let mut s = server;
            let _ = ControlClientMsg::read(&mut s);
            let _ = ControlServerMsg::HelloOk(ControlHelloOk {
                control_version: CONTROL_VERSION + 1,
                protocol_version: 3,
                build: "other".into(),
                separator: '/',
                home: "/root".into(),
                features: vec![],
                instance: "other-instance".into(),
            })
            .encode(&mut s);
        });
        let hello = ControlHello::host_rpc("t", "h");
        let err = RemoteHost::over_unix(client, "test:skew", &hello).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported, "{err}");
    }

    #[test]
    fn an_oversized_read_ships_no_bytes() {
        let (mut client, _) = raw();
        let tmp = tempfile::TempDir::new().unwrap();
        let fat = tmp.path().join("fat.bin");
        std::fs::write(&fat, vec![b'x'; 4096]).unwrap();

        ControlClientMsg::Request {
            req_id: 1,
            req: ControlRequest::ReadFile {
                path: fat.to_string_lossy().into_owned(),
                max_bytes: 16,
            },
        }
        .encode(&mut client)
        .unwrap();
        client.flush().unwrap();

        match ControlServerMsg::read(&mut client).unwrap() {
            ControlServerMsg::Response { req_id, reply } => {
                assert_eq!(req_id, 1);
                match reply {
                    ControlReply::Err(e) => assert_eq!(e.kind, WireErrorKind::FileTooLarge),
                    other => panic!("{other:?}"),
                }
            }
            ControlServerMsg::ResponseBlob { blob, .. } => {
                panic!("the refusal carried {} bytes of the file", blob.len())
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_empty_read_still_uses_the_blob_kind() {
        let (mut client, _) = raw();
        let tmp = tempfile::TempDir::new().unwrap();
        let empty = tmp.path().join("empty.txt");
        std::fs::write(&empty, b"").unwrap();

        ControlClientMsg::Request {
            req_id: 1,
            req: ControlRequest::ReadFile {
                path: empty.to_string_lossy().into_owned(),
                max_bytes: 1024,
            },
        }
        .encode(&mut client)
        .unwrap();
        client.flush().unwrap();

        match ControlServerMsg::read(&mut client).unwrap() {
            ControlServerMsg::ResponseBlob { reply, blob, .. } => {
                assert!(blob.is_empty());
                assert!(matches!(reply, ControlReply::Ok(ReplyOk::FileMeta { .. })));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn error_kinds_survive_the_round_trip() {
        let p = pair();
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        let cases: Vec<(io::ErrorKind, io::Error)> = vec![
            (
                io::ErrorKind::NotFound,
                p.host.stat(&root.join("nope")).unwrap_err(),
            ),
            (io::ErrorKind::AlreadyExists, {
                let f = root.join("taken.txt");
                p.host.write_file(&f, b"x").unwrap();
                p.host.create_file_new(&f).unwrap_err()
            }),
            (io::ErrorKind::FileTooLarge, {
                let f = root.join("fat.txt");
                p.host.write_file(&f, &vec![b'x'; 512]).unwrap();
                p.host.read_file(&f, 16).unwrap_err()
            }),
            (io::ErrorKind::IsADirectory, {
                let d = root.join("adir");
                p.host.create_dir(&d, false).unwrap();
                p.host.read_file(&d, 1024).unwrap_err()
            }),
            (io::ErrorKind::DirectoryNotEmpty, {
                let d = root.join("full");
                p.host.create_dir(&d, false).unwrap();
                p.host.write_file(&d.join("f"), b"x").unwrap();
                p.host.remove(&d, false).unwrap_err()
            }),
        ];
        for (want, got) in cases {
            assert_eq!(got.kind(), want, "{got}");
            assert!(!got.to_string().is_empty());
        }

        for kind in [
            WireErrorKind::NotFound,
            WireErrorKind::PermissionDenied,
            WireErrorKind::AlreadyExists,
            WireErrorKind::InvalidInput,
            WireErrorKind::NotADirectory,
            WireErrorKind::IsADirectory,
            WireErrorKind::DirectoryNotEmpty,
            WireErrorKind::FileTooLarge,
            WireErrorKind::TimedOut,
            WireErrorKind::ConnectionReset,
            WireErrorKind::Other,
        ] {
            let io_err = io::Error::new(kind.to_io_kind(), "x");
            assert_eq!(
                WireError::from_io(&io_err).kind,
                kind,
                "{kind:?} is not its own round trip"
            );
        }
    }

    #[test]
    fn a_failing_git_is_ok_across_the_wire() {
        let p = pair();
        let tmp = tempfile::TempDir::new().unwrap();
        let Ok(out) = p.host.git(tmp.path(), &["rev-parse", "--show-toplevel"]) else {
            return;
        };
        if out.success() {
            return;
        }
        assert!(out.status.is_some());
        assert!(
            !out.stderr.is_empty(),
            "stderr crosses the wire, not dropped"
        );
    }

    #[test]
    fn an_unknown_frame_kind_ends_the_connection() {
        let (mut client, _) = raw();
        crate::daemon::protocol::write_frame(&mut client, 99, b"junk").unwrap();
        client.flush().unwrap();
        let mut buf = [0u8; 1];
        assert_eq!(
            client.read(&mut buf).unwrap(),
            0,
            "the server should hang up"
        );
    }

    #[test]
    fn a_cancelled_request_is_answered_with_silence() {
        let (mut client, _) = raw();
        let tmp = tempfile::TempDir::new().unwrap();

        ControlClientMsg::Cancel { req_id: 1 }
            .encode(&mut client)
            .unwrap();
        ControlClientMsg::Request {
            req_id: 2,
            req: ControlRequest::Stat {
                path: tmp.path().to_string_lossy().into_owned(),
            },
        }
        .encode(&mut client)
        .unwrap();
        client.flush().unwrap();

        match ControlServerMsg::read(&mut client).unwrap() {
            ControlServerMsg::Response { req_id, reply } => {
                assert_eq!(req_id, 2);
                assert!(matches!(reply, ControlReply::Ok(ReplyOk::Meta(_))));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_machine_tree_is_declined_not_faked() {
        let p = pair();
        let err = p
            .host
            .client()
            .call(ControlRequest::MachineGet)
            .unwrap_err();
        assert!(err.to_string().contains("machine tree"), "{err}");
    }

    #[test]
    fn watch_batches_reach_the_client() {
        let p = pair();
        let tmp = tempfile::TempDir::new().unwrap();
        let sub = p.host.watch(&[tmp.path().to_path_buf()]).unwrap();
        while sub.events().try_recv().is_ok() {}

        let deadline = Instant::now() + Duration::from_secs(8);
        let mut seen = false;
        while Instant::now() < deadline && !seen {
            std::fs::write(tmp.path().join("hello.txt"), b"x").unwrap();
            std::thread::sleep(Duration::from_millis(200));
            while let Ok(batch) = sub.events().try_recv() {
                if batch
                    .iter()
                    .any(|q| q.file_name().is_some_and(|n| n == "hello.txt"))
                {
                    seen = true;
                }
            }
        }
        assert!(seen, "no watch event crossed the connection");
    }

    #[test]
    fn a_watch_id_reaches_the_client_before_any_batch_for_it() {
        let (mut client, _ok) = raw();
        let tmp = tempfile::TempDir::new().unwrap();

        ControlClientMsg::Request {
            req_id: 1,
            req: ControlRequest::WatchOpen {
                dirs: vec![tmp.path().to_string_lossy().into_owned()],
            },
        }
        .encode(&mut client)
        .unwrap();
        client.flush().unwrap();

        let churn = tmp.path().to_path_buf();
        let stop = Arc::new(AtomicBool::new(false));
        let churning = Arc::clone(&stop);
        let churner = std::thread::spawn(move || {
            let mut n = 0u32;
            while !churning.load(Ordering::SeqCst) {
                let _ = std::fs::write(churn.join(format!("f{n}")), b"x");
                n = n.wrapping_add(1);
                std::thread::sleep(Duration::from_millis(1));
            }
        });

        let first = ControlServerMsg::read(&mut client).unwrap();
        stop.store(true, Ordering::SeqCst);
        churner.join().unwrap();

        match first {
            ControlServerMsg::Response {
                req_id: 1,
                reply: ControlReply::Ok(ReplyOk::WatchId(_)),
            } => {}
            other => panic!("a batch overtook the WatchId reply: {other:?}"),
        }
    }

    #[test]
    fn closing_a_watch_releases_it_on_the_server() {
        let (server, client) = UnixStream::pair().unwrap();
        let host = LocalHost::new();
        std::thread::spawn(move || {
            let _ = serve(server, host);
        });
        let hello = ControlHello::host_rpc("t", "h");
        let remote = RemoteHost::over_unix(client, "test:watch", &hello).unwrap();

        let tmp = tempfile::TempDir::new().unwrap();
        let sub = remote.watch(&[tmp.path().to_path_buf()]).unwrap();
        let rx = sub.events().clone();
        drop(sub);

        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(tmp.path().join("after.txt"), b"x").unwrap();
        std::thread::sleep(Duration::from_millis(800));
        while let Ok(batch) = rx.try_recv() {
            assert!(
                !batch
                    .iter()
                    .any(|q| q.file_name().is_some_and(|n| n == "after.txt")),
                "the server kept watching after the subscription closed"
            );
        }
    }

    #[test]
    fn a_gitignore_edit_reaches_a_remote_listing() {
        let p = pair();
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        p.host
            .write_file(&root.join(".gitignore"), b"*.log\n")
            .unwrap();
        p.host.write_file(&root.join("a.log"), b"").unwrap();

        let listed = p.host.read_dir(&root, Some(&root)).unwrap();
        assert!(
            listed.iter().any(|e| e.name == "a.log" && e.ignored),
            "the fixture should start out ignored"
        );

        let sub = p.host.watch(std::slice::from_ref(&root)).unwrap();

        let deadline = Instant::now() + Duration::from_secs(15);
        let mut cleared = false;
        while Instant::now() < deadline {
            p.host
                .write_file(&root.join(".gitignore"), b"# nothing\n")
                .unwrap();
            std::thread::sleep(Duration::from_millis(250));
            while sub.events().try_recv().is_ok() {}
            if p.host
                .read_dir(&root, Some(&root))
                .unwrap()
                .iter()
                .any(|e| e.name == "a.log" && !e.ignored)
            {
                cleared = true;
                break;
            }
        }
        assert!(
            cleared,
            "a `.gitignore` change on the server must reach the remote client's listing"
        );
    }

    #[test]
    fn a_write_answers_with_the_new_metadata() {
        let (mut client, _) = raw();
        let tmp = tempfile::TempDir::new().unwrap();
        let f = tmp.path().join("w.txt");

        ControlClientMsg::RequestBlob {
            req_id: 1,
            req: ControlRequest::WriteFile {
                path: f.to_string_lossy().into_owned(),
            },
            blob: b"hello".to_vec(),
        }
        .encode(&mut client)
        .unwrap();
        client.flush().unwrap();

        match ControlServerMsg::read(&mut client).unwrap() {
            ControlServerMsg::Response {
                reply: ControlReply::Ok(ReplyOk::Meta(m)),
                ..
            } => {
                assert_eq!(m.len, 5);
                assert!(
                    m.mtime.is_some(),
                    "the editor keys its echo detection on this"
                );
                let on_disk = std::fs::metadata(&f).unwrap();
                assert_eq!(
                    MTime::from_system_time(on_disk.modified().unwrap()),
                    m.mtime.unwrap()
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_socket_path_honours_its_override() {
        let dir = tempfile::TempDir::new().unwrap();
        let sock = dir.path().join("explicit.sock");
        let listener = bind_control_socket(&sock).unwrap();
        assert!(sock.exists());
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(&sock).unwrap().permissions().mode() & 0o777,
            0o600,
            "a control socket is the access boundary; it must not be group- or world-reachable"
        );
        drop(listener);
    }

    #[test]
    fn binding_does_not_re_permission_a_directory_it_did_not_create() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::TempDir::new().unwrap();
        let shared = dir.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o1777)).unwrap();

        let listener = bind_control_socket(&shared.join("s.sock")).unwrap();
        assert_eq!(
            std::fs::metadata(&shared).unwrap().permissions().mode() & 0o7777,
            0o1777,
            "a directory anterm did not create must keep its own permissions"
        );
        drop(listener);
    }

    #[test]
    fn a_bind_is_owner_only_under_a_permissive_umask() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("permissive.sock");

        let previous = unsafe { libc::umask(0) };
        let bound = sock::bind_private(&path);
        unsafe { libc::umask(previous) };

        let listener = bound.unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
            0,
            "bind left a window in which another user could connect"
        );
        drop(listener);
    }

    #[test]
    fn a_filename_that_is_not_utf8_costs_only_its_own_hit() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;

        let hit = |name: &str, path: &Path| SearchHit {
            name: name.to_string(),
            path: path.to_path_buf(),
            is_dir: false,
            ignored: false,
        };
        let mut hits = vec![
            hit("plain-needle.rs", Path::new("/home/me/plain-needle.rs")),
            hit(
                "caf\u{fffd}-needle.rs",
                Path::new(OsStr::from_bytes(b"/home/me/caf\xe9-needle.rs")),
            ),
            hit("other-needle.rs", Path::new("/home/me/other-needle.rs")),
        ];

        drop_unsendable_hits(&mut hits);
        let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(
            names,
            ["plain-needle.rs", "other-needle.rs"],
            "the representable hits must survive their neighbour"
        );
    }

    #[test]
    fn a_socket_connection_serves_requests() {
        let dir = tempfile::TempDir::new().unwrap();
        let sock = dir.path().join("s.sock");
        let listener = bind_control_socket(&sock).unwrap();
        std::thread::spawn(move || serve_listener(listener, LocalHost::new()));

        let stream = UnixStream::connect(&sock).unwrap();
        let hello = ControlHello::host_rpc("t", "h");
        let host = RemoteHost::over_unix(stream, "test:sock", &hello).unwrap();

        let tmp = tempfile::TempDir::new().unwrap();
        host.write_file(&tmp.path().join("x.txt"), b"content")
            .unwrap();
        assert_eq!(
            host.read_file(&tmp.path().join("x.txt"), 1024).unwrap(),
            b"content"
        );
        let entries = host.read_dir(tmp.path(), None).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "x.txt");
    }

    #[test]
    fn binding_refuses_a_live_server() {
        let dir = tempfile::TempDir::new().unwrap();
        let sock = dir.path().join("s.sock");
        let _first = bind_control_socket(&sock).unwrap();
        assert_eq!(
            bind_control_socket(&sock).unwrap_err().kind(),
            io::ErrorKind::AddrInUse
        );
    }

    #[test]
    fn a_too_long_socket_path_falls_back_to_one_that_fits() {
        let short = PathBuf::from("/tmp/rt");
        let long = PathBuf::from(format!("/tmp/{}", "d".repeat(120)));

        assert_eq!(
            sock::socket_path_in(&short, &[]).unwrap(),
            PathBuf::from("/tmp/rt/control.sock")
        );

        let picked = sock::socket_path_in(&long, &[long.clone(), short.clone()]).unwrap();
        assert!(picked.starts_with(&short), "{}", picked.display());
        assert!(picked.to_string_lossy().ends_with(".sock"));

        let other = PathBuf::from(format!("/tmp/{}", "e".repeat(120)));
        assert_ne!(
            picked,
            sock::socket_path_in(&other, std::slice::from_ref(&short)).unwrap()
        );

        assert_eq!(
            picked,
            sock::socket_path_in(&long, &[long.clone(), short.clone()]).unwrap()
        );

        let err = sock::socket_path_in(&long, std::slice::from_ref(&long)).unwrap_err();
        assert!(err.to_string().contains(CONTROL_SOCK_ENV), "{err}");
    }

    /// `run_daemon` keeps serving panes past a control listener it could not
    /// open only when something else is answering there, and the errno cannot
    /// make that call. `bind_control_socket` clears the leftovers it can, but a
    /// path it cannot clear still comes back `AddrInUse` — indistinguishable
    /// from a live server, with nothing listening behind it. A daemon that read
    /// the error as "someone else is serving" stayed alive holding the
    /// single-server lock, answering nothing, forever.
    #[cfg(unix)]
    #[test]
    fn only_a_connect_tells_a_live_server_from_a_blocked_path() {
        let dir = tempfile::TempDir::new().unwrap();

        let sock = dir.path().join("s.sock");
        let listener = bind_control_socket(&sock).unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                drop(stream);
            }
        });
        assert_eq!(
            bind_control_socket(&sock).unwrap_err().kind(),
            io::ErrorKind::AddrInUse,
            "a live server is in use"
        );
        assert!(
            sock::control_socket_answers(&sock),
            "and it answers, which is what makes it live"
        );

        // A directory where the socket goes. Nothing is listening, the connect
        // fails so there is nothing to unlink, and `bind` refuses it in the
        // same words it used for the live server above.
        let blocked = dir.path().join("blocked.sock");
        std::fs::create_dir(&blocked).unwrap();
        assert_eq!(
            bind_control_socket(&blocked).unwrap_err().kind(),
            io::ErrorKind::AddrInUse,
            "same error, and no server anywhere near it"
        );
        assert!(
            !sock::control_socket_answers(&blocked),
            "which is the difference the daemon has to act on"
        );
    }

    #[test]
    fn binding_clears_a_socket_a_crash_left_behind() {
        let dir = tempfile::TempDir::new().unwrap();
        let sock = dir.path().join("s.sock");
        let first = bind_control_socket(&sock).unwrap();
        drop(first);
        assert!(sock.exists());

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match bind_control_socket(&sock) {
                Ok(listener) => {
                    drop(listener);
                    return;
                }
                Err(e) if Instant::now() >= deadline => {
                    panic!("a stale control socket was never cleared: {e}")
                }
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }

    #[test]
    fn a_stream_slot_comes_back_however_it_leaves() {
        let count = Arc::new(AtomicUsize::new(0));

        {
            let slot = StreamSlot(Arc::clone(&count));
            slot.0.fetch_add(1, Ordering::AcqRel);
            assert_eq!(count.load(Ordering::Acquire), 1);
        }
        assert_eq!(count.load(Ordering::Acquire), 0, "released on drop");

        let refused = StreamSlot(Arc::clone(&count));
        refused.0.fetch_add(1, Ordering::AcqRel);
        drop(refused);
        assert_eq!(count.load(Ordering::Acquire), 0);

        let panicking = Arc::clone(&count);
        let _ = std::thread::spawn(move || {
            let slot = StreamSlot(panicking);
            slot.0.fetch_add(1, Ordering::AcqRel);
            panic!("the read blew up");
        })
        .join();
        assert_eq!(
            count.load(Ordering::Acquire),
            0,
            "a panicking stream must not take its slot with it"
        );
    }

    #[test]
    fn git_stream_delivers_the_same_lines_as_the_buffered_read() {
        let (mut client, _hello) = raw();
        let here = env!("CARGO_MANIFEST_DIR");
        let args: Vec<String> = ["log", "--oneline", "-n", "30"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let buffered = match ask(
            &mut client,
            1,
            ControlRequest::Git {
                cwd: here.to_string(),
                args: args.clone(),
            },
        ) {
            ControlReply::Ok(ReplyOk::Output(o)) => o,
            other => panic!("expected output, got {other:?}"),
        };
        if buffered.status != Some(0) {
            return;
        }
        let expected: Vec<String> = String::from_utf8_lossy(&buffered.stdout)
            .lines()
            .map(str::to_string)
            .collect();

        ControlClientMsg::Request {
            req_id: 2,
            req: ControlRequest::GitStream {
                id: 77,
                cwd: here.to_string(),
                args,
            },
        }
        .encode(&mut client)
        .unwrap();
        client.flush().unwrap();

        let mut split = crate::git::LineSplitter::default();
        let mut got: Vec<String> = Vec::new();
        let mut accepted = false;
        let code = loop {
            match ControlServerMsg::read(&mut client).unwrap() {
                ControlServerMsg::Response { req_id: 2, reply } => {
                    assert!(
                        matches!(reply, ControlReply::Ok(ReplyOk::Unit)),
                        "{reply:?}"
                    );
                    accepted = true;
                }
                ControlServerMsg::Event(ControlEvent::GitChunk { id, bytes }) => {
                    assert_eq!(id, 77, "chunks carry the id the client chose");
                    split.push(&bytes, |l| got.push(l.to_string()));
                }
                ControlServerMsg::Event(ControlEvent::GitEnd { id, code, failed }) => {
                    assert_eq!(id, 77);
                    assert!(!failed, "git ran");
                    break code;
                }
                other => panic!("unexpected {other:?}"),
            }
        };
        split.finish(|l| got.push(l.to_string()));
        assert!(accepted, "the request was answered");
        assert_eq!(code, Some(0));
        assert_eq!(got, expected, "streamed lines match the buffered read");
        assert!(!got.is_empty(), "this repo has commits");
    }

    fn ask(client: &mut UnixStream, req_id: u64, req: ControlRequest) -> ControlReply {
        ControlClientMsg::Request { req_id, req }
            .encode(&mut *client)
            .unwrap();
        client.flush().unwrap();
        loop {
            match ControlServerMsg::read(&mut *client).unwrap() {
                ControlServerMsg::Response { req_id: got, reply } if got == req_id => {
                    return reply;
                }
                ControlServerMsg::Event(_) => continue,
                other => panic!("unexpected frame: {other:?}"),
            }
        }
    }

    #[test]
    fn a_closed_connection_stops_being_a_subscriber() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = MachineStore::open(dir.path().join(machine::MACHINE_FILE));
        let served = {
            let store = Arc::clone(&store);
            let (server, client) = UnixStream::pair().unwrap();
            let handle = std::thread::spawn(move || {
                let _ = serve_with(server, LocalHost::new(), Services::with_machine(store));
            });
            let mut client = client;
            ControlClientMsg::Hello(ControlHello::host_rpc("t", "h"))
                .encode(&mut client)
                .unwrap();
            client.flush().unwrap();
            let _ = ControlServerMsg::read(&mut client).unwrap();
            drop(client);
            handle
        };
        served.join().unwrap();

        let tab = store
            .tab_create(None, machine::PaneSeed::bare(1), None, None)
            .unwrap();
        assert_eq!(store.machine().tabs.len(), 1);
        assert_eq!(store.machine().tabs[0].id, tab.id);
    }

    #[test]
    fn concurrent_connections_can_all_write_the_tree() {
        let (services, _dir) = machine_services();
        let store = Arc::clone(services.machine.as_ref().unwrap());

        let writers: Vec<_> = (0..6)
            .map(|worker| {
                let services = services.clone();
                std::thread::spawn(move || {
                    let (mut client, _) = raw_with(services);
                    for i in 0..10 {
                        let pane = (worker * 100 + i + 1) as u64;
                        let reply = ask(
                            &mut client,
                            i as u64 + 1,
                            ControlRequest::TabCreate {
                                at: None,
                                pane: machine::PaneSeed::bare(pane),
                                tab: None,
                            },
                        );
                        assert!(
                            matches!(reply, ControlReply::Ok(ReplyOk::TabTree(_))),
                            "{reply:?}"
                        );
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        assert_eq!(
            store.machine().tabs.len(),
            60,
            "every operation landed exactly once"
        );
    }

    #[test]
    fn a_lagged_connection_hears_a_resync_instead_of_the_superseded_backlog() {
        let (server_end, mut client_end) = UnixStream::pair().unwrap();
        let sink = Arc::new(Sink::new(server_end));
        let (tx, rx) = smol::channel::bounded::<machine::LayoutDelta>(8);
        let lagged = Arc::new(AtomicBool::new(false));
        let stale_id = machine::TabId::new();
        let fresh_id = machine::TabId::new();

        for _ in 0..4 {
            tx.send_blocking(machine::LayoutDelta::TabClosed { tab: stale_id })
                .unwrap();
        }
        lagged.store(true, Ordering::Release);
        spawn_layout_forwarder(rx, sink, Arc::clone(&lagged));

        assert_eq!(
            ControlServerMsg::read(&mut client_end).unwrap(),
            ControlServerMsg::Event(ControlEvent::LayoutResync)
        );
        assert!(
            !lagged.load(Ordering::Acquire),
            "the flag is consumed: one gap, one resync"
        );

        tx.send_blocking(machine::LayoutDelta::TabClosed { tab: fresh_id })
            .unwrap();
        match ControlServerMsg::read(&mut client_end).unwrap() {
            ControlServerMsg::Event(ControlEvent::Layout { delta }) => {
                assert_eq!(
                    delta,
                    machine::LayoutDelta::TabClosed { tab: fresh_id },
                    "the stale backlog was delivered anyway"
                );
            }
            other => panic!("expected the post-resync delta, got {other:?}"),
        }
    }

    #[test]
    fn set_tags_and_tab_set_tags_dispatch_through_the_control_server() {
        let (services, _dir) = machine_services();
        let store = Arc::clone(services.machine.as_ref().unwrap());
        let (mut client, _) = raw_with(services);

        let tab = match ask(
            &mut client,
            1,
            ControlRequest::TabCreate {
                at: None,
                pane: machine::PaneSeed::bare(1),
                tab: None,
            },
        ) {
            ControlReply::Ok(ReplyOk::TabTree(tab)) => tab,
            other => panic!("expected TabTree, got {other:?}"),
        };

        let mut catalog = crate::tag::SidebarTags::default();
        let prod = catalog
            .create("Prod", crate::tag::TagColor::Red)
            .expect("valid tag");
        let unknown = crate::tag::TagId::new();

        assert_eq!(
            ask(
                &mut client,
                2,
                ControlRequest::SetTags {
                    tags: catalog.clone()
                },
            ),
            ControlReply::Ok(ReplyOk::Unit)
        );
        assert_eq!(
            ask(
                &mut client,
                3,
                ControlRequest::TabSetTags {
                    tab: tab.id,
                    tags: vec![prod, unknown, prod],
                },
            ),
            ControlReply::Ok(ReplyOk::Unit)
        );

        let machine = store.machine();
        assert_eq!(machine.tags, catalog);
        assert_eq!(machine.tabs[0].tags, vec![prod]);

        assert_eq!(
            ask(
                &mut client,
                4,
                ControlRequest::SetTags {
                    tags: crate::tag::SidebarTags::default(),
                },
            ),
            ControlReply::Ok(ReplyOk::Unit)
        );
        let machine = store.machine();
        assert!(machine.tags.is_empty());
        assert!(machine.tabs[0].tags.is_empty());
    }
}
