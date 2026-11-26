mod attach;
mod dispatch;
mod pool;
mod transport;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::machine::{self, MachineStore};
use crate::daemon::control::{
    CONTROL_VERSION, ControlClientMsg, ControlEvent, ControlHello, ControlHelloOk, ControlReply,
    ControlServerMsg, GIT_STREAM_CHUNK, GIT_STREAM_CHUNK_MAX, LinkShutdown,
    MAX_CONCURRENT_GIT_STREAMS, PaneAgentState, WATCH_BURST_CAP, WireError, feature,
};
use crate::daemon::duplex::{Duplex, Halves};
use crate::daemon::protocol::PaneInfo;
use crate::host::{Host, SharedHost, WatchSub};

use dispatch::submit;
use pool::Pool;

pub use attach::AttachRegistry;
pub use pool::{MAX_QUEUED, MAX_WORKERS, WORKER_LINGER};
pub use transport::CONTROL_SOCK_ENV;
#[cfg(all(unix, test))]
pub(crate) use transport::socket_path_in;
#[cfg(unix)]
pub use transport::{
    bind_control_socket, control_endpoint_answers, control_socket_path, serve_listener,
    serve_listener_with, spawn_control_listener, spawn_control_listener_with,
};
#[cfg(windows)]
pub use transport::{
    CONTROL_PORT_FILE, connect_control, control_endpoint_answers, control_endpoint_path,
    remove_control_endpoint, spawn_control_listener, spawn_control_listener_with,
};

pub const LAYOUT_EVENT_QUEUE: usize = 1024;

pub trait PaneDirectory: Send + Sync {
    fn pane_count(&self) -> u64;
    fn panes(&self) -> Vec<PaneInfo>;
    fn agent_states(&self) -> Vec<PaneAgentState>;
    /// What is running inside one pane, and what it is listening on.
    ///
    /// Answered by whoever owns the pane's PTY, which for a remote workspace
    /// is this process and not the client's own daemon: the client asks over
    /// the control link precisely because the processes are here.
    fn pane_procs(&self, pane_id: u64) -> crate::daemon::protocol::PaneProcs;
    /// Stop a pane for a tab going to sleep: the processes go, the pane's last
    /// screen stays on disk for the wake to open with. Unlike closing a pane,
    /// which drops that screen because nobody will ask for it again.
    fn hibernate_pane(&self, pane_id: u64);
    /// End a pane for good: the processes go and so does its stored screen,
    /// because nothing will ask to restore it again — a pane the tree never
    /// recorded, or one whose closed tab has aged off the recently-closed list.
    fn close_pane(&self, pane_id: u64);
}

#[derive(Clone, Default)]
pub struct Services {
    pub machine: Option<Arc<MachineStore>>,
    pub attachments: Arc<AttachRegistry>,
    pub panes: Option<Arc<dyn PaneDirectory>>,
}

impl Services {
    pub fn none() -> Services {
        Services::default()
    }

    pub fn with_machine(store: Arc<MachineStore>) -> Services {
        Services {
            machine: Some(store),
            attachments: Arc::new(AttachRegistry::default()),
            panes: None,
        }
    }
}

pub fn serve<D: Duplex>(link: D, host: SharedHost) -> io::Result<()> {
    serve_with(link, host, Services::none())
}

pub fn serve_with<D: Duplex>(link: D, host: SharedHost, services: Services) -> io::Result<()> {
    let label = link.kind_label();
    let Halves {
        read,
        write,
        shutdown,
    } = link.split()?;
    serve_halves_with(read, write, shutdown, host, services, label)
}

pub fn serve_halves<R, W>(
    r: R,
    w: W,
    shutdown: Arc<dyn LinkShutdown>,
    host: SharedHost,
    label: &'static str,
) -> io::Result<()>
where
    R: Read,
    W: Write + Send + 'static,
{
    serve_halves_with(r, w, shutdown, host, Services::none(), label)
}

pub fn serve_halves_with<R, W>(
    mut r: R,
    w: W,
    shutdown: Arc<dyn LinkShutdown>,
    host: SharedHost,
    services: Services,
    label: &'static str,
) -> io::Result<()>
where
    R: Read,
    W: Write + Send + 'static,
{
    let sink = Arc::new(Sink::new(w));

    let hello = match handshake(&mut r, &sink, &*host, &services) {
        Ok(Some(hello)) => hello,
        Ok(None) => {
            let _ = shutdown.shutdown_link();
            return Ok(());
        }
        Err(e) => {
            let _ = shutdown.shutdown_link();
            return if is_disconnect(&e) {
                log::debug!("control peer ({label}) left during the handshake: {e}");
                Ok(())
            } else {
                Err(e)
            };
        }
    };

    let machine_sub = subscribe_machine(&services, &sink);

    let conn = Arc::new(Conn {
        host,
        sink: Arc::clone(&sink),
        inflight: Mutex::new(HashMap::new()),
        watches: Mutex::new(HashMap::new()),
        deferred_watches: Mutex::new(HashMap::new()),
        next_watch: AtomicU64::new(1),
        git_streams: Arc::new(AtomicUsize::new(0)),
        pool: Pool::new(),
        machine: services.machine.clone(),
        machine_origin: machine_sub.as_ref().map(machine::Subscription::id),
        attachments: Arc::clone(&services.attachments),
        panes: services.panes.clone(),
        id: NEXT_CONN.fetch_add(1, Ordering::Relaxed),
    });

    if hello.gui {
        // GUI registration starts only after a successful version handshake,
        // so an incompatible app can never be reported as a live receiver.
        services
            .attachments
            .register_gui(conn.id, Arc::clone(&sink));
    }

    let outcome = read_loop(&mut r, &conn);

    conn.pool.close();
    conn.watches
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    // Every exit path converges here, including EOF and protocol errors.
    conn.attachments.unregister_gui(conn.id);
    drop(machine_sub);
    sink.retire();
    let _ = shutdown.shutdown_link();

    match outcome {
        Ok(()) => {
            log::debug!("control connection ({label}) closed by peer");
            Ok(())
        }
        Err(e) if is_disconnect(&e) => {
            log::debug!("control connection ({label}) ended: {e}");
            Ok(())
        }
        Err(e) => Err(e),
    }
}

fn is_disconnect(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
    )
}

fn handshake<R: Read>(
    r: &mut R,
    sink: &Sink,
    host: &dyn Host,
    services: &Services,
) -> io::Result<Option<ControlHello>> {
    let hello = match ControlClientMsg::read(r)? {
        ControlClientMsg::Hello(h) => h,
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("control peer opened with {other:?} instead of HELLO"),
            ));
        }
    };

    let mut features = vec![
        feature::CONTROL.to_string(),
        feature::HOST_RPC.to_string(),
        feature::STDIO_BRIDGE.to_string(),
        // Every host this server fronts can search its own files; the
        // request only needs a name so an older client's peer is never sent
        // it (see `RemoteHost::search_content`).
        feature::CONTENT_SEARCH.to_string(),
        crate::daemon::protocol::FEATURE_GATEWAY_UPLOAD.to_string(),
    ];
    // Only where there are panes to ask about. A control peer serving no panes
    // would answer every ask with an empty list, which reads to a client as
    // "nothing is listening" rather than as "I cannot tell you".
    if services.panes.is_some() {
        features.push(feature::PANE_PROCS.to_string());
    }
    if services.machine.is_some() {
        features.push(feature::MACHINE_TREE.to_string());
    }
    // A sleeping tab is a mark in the tree *and* stopped panes; a peer that can
    // do only half of that would leave a tab asleep with its shells running,
    // or its shells stopped with nothing saying the tab is meant to wake.
    if services.machine.is_some() && services.panes.is_some() {
        features.push(feature::TAB_HIBERNATE.to_string());
        // The same pair for the same reason: a closed tab is an entry in the
        // tree *and* screens kept by whoever stops the panes.
        features.push(feature::CLOSED_TABS.to_string());
    }
    // The pane daemon's features ride along, same as `protocol_version` above:
    // a pane connection answers exactly one message, so a client that wanted
    // them from `ClientMsg::Version` would pay a whole routed connection — for
    // ssh and WSL, a whole bridge process — per pane. The hello already travels
    // once per host and is cached on the link, so this is where a client can
    // afford to learn what the panes it routes here will do (the resize echo
    // above all). Additive names are safe: every check on either end asks for
    // a name, never for the exact list.
    features.extend(crate::daemon::protocol::DaemonVersion::current().features);

    sink.send(&ControlServerMsg::HelloOk(ControlHelloOk {
        control_version: CONTROL_VERSION,
        protocol_version: crate::daemon::protocol::PROTOCOL_VERSION,
        build: env!("CARGO_PKG_VERSION").to_string(),
        separator: host.separator(),
        home: home_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        features,
        instance: crate::daemon::control::server_instance().to_string(),
    }))?;

    if hello.control_version != CONTROL_VERSION {
        log::warn!(
            "control peer speaks v{}, this build speaks v{CONTROL_VERSION}; closing",
            hello.control_version
        );
        return Ok(None);
    }
    Ok(Some(hello))
}

static NEXT_CONN: AtomicU64 = AtomicU64::new(1);

fn home_dir() -> Option<PathBuf> {
    crate::util::home_dir_any()
}

fn read_loop<R: Read>(r: &mut R, conn: &Arc<Conn>) -> io::Result<()> {
    loop {
        let msg = ControlClientMsg::read(r)?;
        match msg {
            ControlClientMsg::Hello(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "control peer sent a second HELLO on an established connection",
                ));
            }
            ControlClientMsg::Request { req_id, req } => submit(conn, req_id, req, Vec::new()),
            ControlClientMsg::RequestBlob { req_id, req, blob } => submit(conn, req_id, req, blob),
            ControlClientMsg::Cancel { req_id } => conn.cancel(req_id),
        }
    }
}

struct Conn {
    host: SharedHost,
    sink: Arc<Sink>,
    inflight: Mutex<HashMap<u64, bool>>,
    watches: Mutex<HashMap<u64, WatchSub>>,
    deferred_watches: Mutex<HashMap<u64, (u64, smol::channel::Receiver<Vec<PathBuf>>)>>,
    next_watch: AtomicU64,
    git_streams: Arc<AtomicUsize>,
    pool: Pool,
    machine: Option<Arc<MachineStore>>,
    machine_origin: Option<machine::SubscriberId>,
    attachments: Arc<AttachRegistry>,
    panes: Option<Arc<dyn PaneDirectory>>,
    id: u64,
}

impl Conn {
    fn machine(&self) -> io::Result<&Arc<MachineStore>> {
        self.machine
            .as_ref()
            .ok_or_else(|| io::Error::other("this server does not serve the machine tree"))
    }

    fn is_cancelled(&self, req_id: u64) -> bool {
        self.inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&req_id)
            .copied()
            .unwrap_or(false)
    }

    fn forget(&self, req_id: u64) {
        self.inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&req_id);
    }

    fn cancel(&self, req_id: u64) {
        let mut f = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(flag) = f.get_mut(&req_id) {
            *flag = true;
        }
    }

    fn finish(&self, req_id: u64, reply: ControlReply, blob: Vec<u8>, wants_blob: bool) {
        let cancelled = self
            .inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&req_id)
            .unwrap_or(false);
        if cancelled {
            log::trace!("control request {req_id} was cancelled; dropping its reply");
            self.start_deferred_watch(req_id, false);
            return;
        }

        let msg = if wants_blob && matches!(reply, ControlReply::Ok(_)) {
            ControlServerMsg::ResponseBlob {
                req_id,
                reply,
                blob,
            }
        } else {
            ControlServerMsg::Response { req_id, reply }
        };
        let delivered = match msg.to_frame() {
            Ok((k, payload)) => match self.sink.send_frame(k, &payload) {
                Ok(()) => true,
                Err(e) => {
                    log::debug!("control reply {req_id} could not be written: {e}");
                    false
                }
            },
            Err(e) => {
                log::warn!("control reply {req_id} could not be encoded: {e}");
                let excuse = ControlServerMsg::Response {
                    req_id,
                    reply: ControlReply::Err(WireError::from_io(&e)),
                };
                if let Err(e) = self.sink.send(&excuse) {
                    log::debug!("control error reply {req_id} could not be written: {e}");
                }
                false
            }
        };

        self.start_deferred_watch(req_id, delivered);
    }

    fn open_watch(&self, req_id: u64, dirs: &[PathBuf]) -> io::Result<u64> {
        let sub = self.host.watch(dirs)?;
        let id = self.next_watch.fetch_add(1, Ordering::Relaxed);
        let rx = sub.events().clone();
        self.watches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, sub);
        self.deferred_watches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(req_id, (id, rx));
        Ok(id)
    }

    fn start_deferred_watch(&self, req_id: u64, deliver: bool) {
        let parked = self
            .deferred_watches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&req_id);
        let Some((id, rx)) = parked else {
            return;
        };
        if !deliver {
            self.watches
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            return;
        }
        spawn_watch_forwarder(id, rx, Arc::clone(&self.sink));
    }

    fn start_git_stream(&self, id: u64, cwd: PathBuf, args: Vec<String>) {
        let host = Arc::clone(&self.host);
        let sink = Arc::clone(&self.sink);
        let slot = StreamSlot(Arc::clone(&self.git_streams));
        if slot.0.fetch_add(1, Ordering::AcqRel) >= MAX_CONCURRENT_GIT_STREAMS {
            drop(slot);
            let _ = self
                .sink
                .send(&ControlServerMsg::Event(ControlEvent::GitEnd {
                    id,
                    code: None,
                    failed: true,
                }));
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("anterm-control-git-stream".into())
            .spawn(move || {
                let _slot = slot;
                let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
                let mut batch: Vec<u8> = Vec::with_capacity(GIT_STREAM_CHUNK);
                let mut stopped: Option<StreamStop> = None;
                let flush = |batch: &mut Vec<u8>, stopped: &mut Option<StreamStop>| {
                    if stopped.is_some() {
                        batch.clear();
                        return;
                    }
                    if batch.is_empty() {
                        return;
                    }
                    let bytes = std::mem::take(batch);
                    for piece in bytes.chunks(GIT_STREAM_CHUNK_MAX) {
                        let event = ControlServerMsg::Event(ControlEvent::GitChunk {
                            id,
                            bytes: piece.to_vec(),
                        });
                        let Ok((kind, payload)) = event.to_frame() else {
                            *stopped = Some(StreamStop::Unencodable);
                            return;
                        };
                        if sink.send_frame(kind, &payload).is_err() {
                            *stopped = Some(StreamStop::LinkGone);
                            return;
                        }
                    }
                };
                let result = host.git_lines(&cwd, &borrowed, &mut |line| {
                    if stopped.is_some() {
                        return;
                    }
                    batch.extend_from_slice(line.as_bytes());
                    batch.push(b'\n');
                    if batch.len() >= GIT_STREAM_CHUNK {
                        flush(&mut batch, &mut stopped);
                    }
                });
                flush(&mut batch, &mut stopped);
                if matches!(stopped, Some(StreamStop::LinkGone)) {
                    return;
                }
                let (code, failed) = match (stopped, result) {
                    (Some(StreamStop::Unencodable), _) => (None, true),
                    (_, Ok(code)) => (code, false),
                    (_, Err(_)) => (None, true),
                };
                let _ = sink.send(&ControlServerMsg::Event(ControlEvent::GitEnd {
                    id,
                    code,
                    failed,
                }));
            });
        if spawned.is_err() {
            let _ = self
                .sink
                .send(&ControlServerMsg::Event(ControlEvent::GitEnd {
                    id,
                    code: None,
                    failed: true,
                }));
        }
    }

    fn set_watch_dirs(&self, id: u64, dirs: &[PathBuf]) -> io::Result<()> {
        let watches = self.watches.lock().unwrap_or_else(|e| e.into_inner());
        let sub = watches.get(&id).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("no such watch subscription {id}"),
            )
        })?;
        sub.set_dirs(dirs)
    }

    fn close_watch(&self, id: u64) {
        self.watches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
    }
}

fn subscribe_machine(services: &Services, sink: &Arc<Sink>) -> Option<machine::Subscription> {
    let store = services.machine.as_ref()?;
    let (tx, rx) = smol::channel::bounded::<machine::LayoutDelta>(LAYOUT_EVENT_QUEUE);
    let lagged = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let saw_drop = Arc::clone(&lagged);
    let subscription = store.subscribe(Arc::new(move |delta: &machine::LayoutDelta| {
        if tx.try_send(delta.clone()).is_err() {
            saw_drop.store(true, Ordering::Release);
            log::warn!(
                "dropping a layout delta for a peer {LAYOUT_EVENT_QUEUE} deltas behind; \
                 it will be told to resync"
            );
        }
    }));
    spawn_layout_forwarder(rx, Arc::clone(sink), lagged);
    Some(subscription)
}

fn spawn_layout_forwarder(
    rx: smol::channel::Receiver<machine::LayoutDelta>,
    sink: Arc<Sink>,
    lagged: Arc<std::sync::atomic::AtomicBool>,
) {
    let spawned = std::thread::Builder::new()
        .name("anterm-control-layout".into())
        .spawn(move || {
            while let Ok(delta) = rx.recv_blocking() {
                if lagged.swap(false, Ordering::AcqRel) {
                    let mut superseded = 1;
                    while rx.try_recv().is_ok() {
                        superseded += 1;
                    }
                    log::info!(
                        "dropping {superseded} superseded layout delta(s) and asking the peer \
                         to resync"
                    );
                    if sink
                        .send(&ControlServerMsg::Event(ControlEvent::LayoutResync))
                        .is_err()
                    {
                        return;
                    }
                    continue;
                }
                let event = ControlEvent::Layout { delta };
                if sink.send(&ControlServerMsg::Event(event)).is_err() {
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        log::warn!("could not start the layout forwarder: {e}");
    }
}

fn spawn_watch_forwarder(id: u64, rx: smol::channel::Receiver<Vec<PathBuf>>, sink: Arc<Sink>) {
    let spawned = std::thread::Builder::new()
        .name("anterm-control-watch".into())
        .spawn(move || {
            while let Ok(batch) = rx.recv_blocking() {
                let event = if batch.len() > WATCH_BURST_CAP {
                    ControlEvent::WatchOverflow { id }
                } else {
                    ControlEvent::Watch {
                        id,
                        paths: batch
                            .iter()
                            .map(|p| p.to_string_lossy().into_owned())
                            .collect(),
                    }
                };
                if sink.send(&ControlServerMsg::Event(event)).is_err() {
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        log::warn!("could not start the watch forwarder for subscription {id}: {e}");
    }
}

struct StreamSlot(Arc<AtomicUsize>);

impl Drop for StreamSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StreamStop {
    LinkGone,
    Unencodable,
}

struct Sink {
    out: Mutex<Option<Box<dyn Write + Send>>>,
}

impl Sink {
    fn new<W: Write + Send + 'static>(w: W) -> Sink {
        Sink {
            out: Mutex::new(Some(Box::new(w))),
        }
    }

    fn send(&self, msg: &ControlServerMsg) -> io::Result<()> {
        let (k, payload) = msg.to_frame()?;
        self.send_frame(k, &payload)
    }

    fn send_frame(&self, k: u8, payload: &[u8]) -> io::Result<()> {
        let mut slot = self.out.lock().unwrap_or_else(|e| e.into_inner());
        let w = slot.as_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "control connection is closed")
        })?;
        crate::daemon::protocol::write_frame(&mut *w, k, payload).and_then(|()| w.flush())
    }

    fn retire(&self) {
        *self.out.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}
