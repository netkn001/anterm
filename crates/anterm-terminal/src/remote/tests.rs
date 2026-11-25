mod config_tests {
    use super::super::*;

    #[test]
    fn user_config_maps_into_alacritty_config() {
        let user = anterm_ui_kit::config::Config::default();
        let config = terminal_config_from_user(&user);
        assert_eq!(config.scrolling_history, user.scrollback_limit);
        assert_eq!(config.semantic_escape_chars, user.word_separators);
        assert!(config.kitty_keyboard);
    }
}

/// Issue #857 asked whether a read boundary can leave the grid different from
/// the one the same bytes parse into in one go — a Pi response streamed as
/// synchronized differential frames (cursor-up, erase-line, rewrite) with
/// OSC 133 marks and CJK text. The reader hands the emulator whatever the
/// socket delivers, so the grid has to come out the same for every split.
mod chunking_tests {
    use super::super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::grid::Dimensions as _;
    use alacritty_terminal::index::{Column, Line};

    const COLS: usize = 24;
    const ROWS: usize = 8;

    fn parse(chunks: &[&[u8]]) -> Vec<String> {
        let config = terminal_config_from_user(&anterm_ui_kit::config::Config::default());
        let mut term = Term::new(config, &TermSize::new(COLS, ROWS), VoidListener);
        let mut processor: ansi::Processor = ansi::Processor::new();
        for chunk in chunks {
            processor.advance(&mut term, chunk);
        }
        assert!(
            processor.sync_timeout().sync_timeout().is_none(),
            "every frame closes its synchronized update"
        );
        let grid = term.grid();
        let mut rows: Vec<String> = (0..grid.screen_lines() as i32)
            .map(|line| {
                (0..COLS)
                    .map(|col| {
                        let cell = &grid[Line(line)][Column(col)];
                        let mut s = format!("{}{:?}", cell.c, cell.flags);
                        for z in cell.zerowidth().unwrap_or(&[]) {
                            s.push(*z);
                        }
                        s
                    })
                    .collect()
            })
            .collect();
        rows.push(format!("{:?}", grid.cursor.point));
        rows
    }

    /// Three frames of a response streaming in, each redrawing the lines it
    /// touched the way Pi's main-screen renderer does.
    fn stream() -> Vec<u8> {
        let mark = "\x1b]133;B\x07\x1b]133;C\x07";
        let frames = [
            format!("\x1b[?2026h\r\x1b[2K例{mark}Ex\x1b[?2026l"),
            format!(
                "\x1b[?2026h\r\x1b[2K例子：Example sentence.\r\n\r\n\x1b[2K## 资源{mark}N\x1b[?2026l"
            ),
            format!(
                "\x1b[?2026h\x1b[2A\r\x1b[2K例子：Example.❤️\r\n\r\n\x1b[2K## 资源\r\n\
                 \x1b[2K{mark}下一步：Next step.\x1b[?2026l"
            ),
        ];
        frames.concat().into_bytes()
    }

    #[test]
    fn a_streamed_response_parses_the_same_at_every_split() {
        let bytes = stream();
        let whole = parse(&[&bytes]);
        assert!(
            whole.iter().all(|row| !row.contains("33;")),
            "no mark reaches the grid as text"
        );
        for cut in 0..=bytes.len() {
            assert_eq!(
                parse(&[&bytes[..cut], &bytes[cut..]]),
                whole,
                "split at byte {cut}"
            );
        }
        let bytewise: Vec<&[u8]> = bytes.chunks(1).collect();
        assert_eq!(parse(&bytewise), whole, "a byte at a time");
    }

    /// A batch several lock-holds long, colored and multi-byte so slices end
    /// mid-sequence and mid-character.
    fn long_batch() -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut n = 0;
        while bytes.len() < 3 * MAX_LOCKED_FEED + 777 {
            bytes.extend_from_slice(format!("\x1b[3{}m行{n}\x1b[0m\r\n", n % 8).as_bytes());
            n += 1;
        }
        bytes
    }

    fn fresh() -> Term<VoidListener> {
        let config = terminal_config_from_user(&anterm_ui_kit::config::Config::default());
        Term::new(config, &TermSize::new(COLS, ROWS), VoidListener)
    }

    /// Where the cursor stood at each cut, and the screen after the whole batch.
    type Seen = (Vec<(i32, usize)>, Vec<String>);

    fn screen(term: &Term<VoidListener>) -> Vec<String> {
        let grid = term.grid();
        (0..ROWS as i32)
            .map(|line| {
                (0..COLS)
                    .map(|col| grid[Line(line)][Column(col)].c)
                    .collect()
            })
            .collect()
    }

    fn cursor(term: &Term<VoidListener>) -> (i32, usize) {
        let point = term.grid().cursor.point;
        (point.line.0, point.column.0)
    }

    /// The reader lets go of the grid every `MAX_LOCKED_FEED` bytes, and not
    /// one of those slices may show: the screen, and the state each cut acts
    /// on, come out as if the batch had been parsed in one hold — including a
    /// cut that lands exactly on a slice boundary.
    #[test]
    fn a_batch_fed_in_slices_parses_as_one() {
        let bytes = long_batch();
        let offsets = [
            0,
            5,
            MAX_LOCKED_FEED,
            MAX_LOCKED_FEED + 1,
            2 * MAX_LOCKED_FEED + 9,
            bytes.len(),
        ];

        let whole: Seen = {
            let mut term = fresh();
            let mut processor: ansi::Processor = ansi::Processor::new();
            let mut at = 0;
            let mut stops = Vec::new();
            for &off in &offsets {
                processor.advance(&mut term, &bytes[at..off]);
                at = off;
                stops.push(cursor(&term));
            }
            (stops, screen(&term))
        };

        let term = FairMutex::new(fresh());
        let mut processor: ansi::Processor = ansi::Processor::new();
        let mut guard = term_adapter::KeyboardStackGuard::default();
        let cuts = offsets
            .iter()
            .map(|&off| (off, ReaderCut::Command(CommandMark::Finished)))
            .collect();
        let mut stops = Vec::new();
        let fed = feed_grid(
            &term,
            &mut processor,
            &mut guard,
            &bytes,
            cuts,
            &AtomicBool::new(false),
            |term, _, _| stops.push(cursor(term)),
        );
        assert!(fed.is_some());
        assert_eq!((stops, screen(&term.lock())), whole);
    }

    /// A reader retired while it waited for the grid writes nothing into it:
    /// the grid belongs to its successor now.
    #[test]
    fn a_retired_reader_leaves_the_grid_alone() {
        let term = FairMutex::new(fresh());
        let before = screen(&term.lock());
        let mut processor: ansi::Processor = ansi::Processor::new();
        let mut guard = term_adapter::KeyboardStackGuard::default();
        let fed = feed_grid(
            &term,
            &mut processor,
            &mut guard,
            b"stale",
            Vec::new(),
            &AtomicBool::new(true),
            |_, _, _| {},
        );
        assert!(fed.is_none());
        assert_eq!(screen(&term.lock()), before);
    }
}

/// What a re-attach's replay actually leaves in the grid.
///
/// Switching workspaces tears every pane down and attaches to the same daemon
/// pane again (#711), so the whole of a pane's screen has to survive one trip
/// through the replay — several ring segments, each preceded by the geometry it
/// was written at, and a snapshot frame far larger than one socket read. These
/// drive that over a real socket pair rather than a mock, because the loss
/// being hunted is between the wire and the grid; and they are not `cfg(unix)`
/// like their neighbours because nothing about the path is.
mod replay_tests {
    use super::super::*;
    use anterm_core::daemon::protocol::DaemonMsg;
    use anterm_core::daemon::transport::Stream;

    pub(super) fn socket_pair() -> (Stream, Stream) {
        #[cfg(unix)]
        {
            std::os::unix::net::UnixStream::pair().unwrap()
        }
        #[cfg(windows)]
        {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let client_side = std::net::TcpStream::connect(addr).unwrap();
            let (daemon_side, _) = listener.accept().unwrap();
            (client_side, daemon_side)
        }
    }

    fn ws(cols: u16, rows: u16) -> WinSize {
        WinSize {
            cols,
            rows,
            cell_w: 8,
            cell_h: 17,
        }
    }

    /// Everything the grid holds, scrollback included, one row per line.
    fn all_text(term: &RemoteTerminal) -> String {
        use alacritty_terminal::grid::Dimensions as _;
        use alacritty_terminal::index::{Column, Line};
        let t = term.term.lock();
        let grid = t.grid();
        let mut out = String::new();
        let top = -(grid.history_size() as i32);
        for line in top..grid.screen_lines() as i32 {
            let row = &grid[Line(line)];
            let mut text = String::new();
            for col in 0..grid.columns() {
                text.push(row[Column(col)].c);
            }
            let text = text.trim_end();
            if !text.is_empty() {
                out.push_str(text);
                out.push('\n');
            }
        }
        out
    }

    /// The visible screen alone.
    fn screen_text(term: &RemoteTerminal) -> String {
        use alacritty_terminal::grid::Dimensions as _;
        use alacritty_terminal::index::{Column, Line};
        let t = term.term.lock();
        let grid = t.grid();
        let mut out = String::new();
        for line in 0..grid.screen_lines() as i32 {
            let row = &grid[Line(line)];
            let mut text = String::new();
            for col in 0..grid.columns() {
                text.push(row[Column(col)].c);
            }
            let text = text.trim_end();
            if !text.is_empty() {
                out.push_str(text);
                out.push('\n');
            }
        }
        out
    }

    /// The grid's width. A grid is born at the size its `RemoteTerminal` was
    /// built with and only ever leaves it on a `DaemonMsg::Size`, so this is
    /// what says which geometry frames were applied.
    fn columns(term: &RemoteTerminal) -> usize {
        use alacritty_terminal::grid::Dimensions as _;
        term.term.lock().grid().columns()
    }

    /// Waits for the reader thread to have applied everything named, then
    /// returns the whole grid either way so a failure can print what landed.
    fn settled(term: &RemoteTerminal, needles: &[&str]) -> String {
        for _ in 0..600 {
            let text = all_text(term);
            if needles.iter().all(|n| text.contains(n)) {
                return text;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        all_text(term)
    }

    /// The shape a re-attach takes: the client's grid is born at the hardcoded
    /// attach size, and the daemon replays every ring segment at the geometry
    /// it was recorded at.
    #[test]
    fn every_replayed_segment_reaches_the_grid() {
        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, mut daemon) = socket_pair();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        DaemonMsg::Size(ws(80, 24)).encode(&mut daemon).unwrap();
        DaemonMsg::Snapshot(b"BIRTH-BANNER\r\n".to_vec())
            .encode(&mut daemon)
            .unwrap();
        DaemonMsg::Size(ws(120, 40)).encode(&mut daemon).unwrap();
        DaemonMsg::Snapshot(b"SECOND-SEGMENT\r\n".to_vec())
            .encode(&mut daemon)
            .unwrap();
        DaemonMsg::Size(ws(120, 40)).encode(&mut daemon).unwrap();
        DaemonMsg::Snapshot(b"THIRD-SEGMENT\r\n".to_vec())
            .encode(&mut daemon)
            .unwrap();

        let text = settled(&term, &["BIRTH-BANNER", "SECOND-SEGMENT", "THIRD-SEGMENT"]);
        assert!(text.contains("BIRTH-BANNER"), "grid held:\n{text}");
        assert!(text.contains("SECOND-SEGMENT"), "grid held:\n{text}");
        assert!(text.contains("THIRD-SEGMENT"), "grid held:\n{text}");
        // Each segment's geometry too, or this would pass on a replay that
        // dropped every `Size`. The unix-gated
        // `segmented_ring_replay_reproduces_live_rendering` asserts that half
        // already; nothing did on the platforms this module exists for.
        assert_eq!(
            columns(&term),
            120,
            "the geometry each segment was recorded at never reached the grid, \
             so it is still at the attach size"
        );
        drop(daemon);
    }

    /// A real pane's ring is megabytes; one `Snapshot` frame is far larger than
    /// the reader's 256 KiB read buffer, so the frame is assembled across many
    /// reads — several of which time out at `QUIT_POLL` while the sender is
    /// still pushing.
    #[test]
    fn a_snapshot_larger_than_one_read_still_lands_whole() {
        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, daemon) = socket_pair();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        let mut bulk = Vec::new();
        bulk.extend_from_slice(b"HEAD-OF-THE-RING\r\n");
        for i in 0..40_000 {
            bulk.extend_from_slice(format!("line {i} of the pane's history\r\n").as_bytes());
        }
        bulk.extend_from_slice(b"TAIL-OF-THE-RING\r\n");

        let feeder = std::thread::spawn(move || {
            let mut daemon = daemon;
            DaemonMsg::Size(ws(120, 40)).encode(&mut daemon).unwrap();
            DaemonMsg::Snapshot(bulk).encode(&mut daemon).unwrap();
            DaemonMsg::Size(ws(120, 40)).encode(&mut daemon).unwrap();
            DaemonMsg::Snapshot(b"AFTER-THE-BULK\r\n".to_vec())
                .encode(&mut daemon)
                .unwrap();
            daemon
        });

        let text = settled(&term, &["AFTER-THE-BULK"]);
        assert!(
            text.contains("AFTER-THE-BULK"),
            "the frame after a multi-megabyte snapshot never arrived; grid tail:\n{}",
            screen_text(&term)
        );
        assert!(
            text.contains("TAIL-OF-THE-RING"),
            "the snapshot itself was truncated"
        );
        drop(feeder.join().unwrap());
    }

    /// The view resizes to its real geometry the first time it paints, which
    /// happens while the replay is still arriving. Against a resize-echoing
    /// daemon the grid must not reflow when the request goes out — only when
    /// the daemon echoes the `Size` back, which is the stream position where
    /// the bytes stop being old-width — and neither step may cost the
    /// replayed screen.
    ///
    /// The two geometries are deliberately different: at identical dimensions
    /// a reflow and a deferred reflow look the same, so the invariant would be
    /// unobservable.
    #[test]
    fn a_resize_racing_the_replay_keeps_the_replayed_screen() {
        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, mut daemon) = socket_pair();
        let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        term.route = echoing_route();

        DaemonMsg::Size(ws(120, 40)).encode(&mut daemon).unwrap();
        DaemonMsg::Snapshot(b"REPLAYED-SCREEN\r\n".to_vec())
            .encode(&mut daemon)
            .unwrap();
        let text = settled(&term, &["REPLAYED-SCREEN"]);
        assert!(text.contains("REPLAYED-SCREEN"), "grid held:\n{text}");

        assert_eq!(columns(&term), 120, "the replay set the grid's geometry");

        // First paint: the pane is 100x30 on screen, not the 120x40 the ring
        // was recorded at. The request goes down the link and the grid stays
        // where the replay left it.
        term.resize(TermSize::new(100, 30), 8, 17);
        assert_eq!(
            columns(&term),
            120,
            "the grid reflowed at request time instead of waiting for the echo"
        );
        let _ = daemon.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        match ClientMsg::read(&mut daemon) {
            Ok(ClientMsg::Resize(size)) => assert_eq!((size.cols, size.rows), (100, 30)),
            other => panic!("the resize never reached the daemon: {other:?}"),
        }

        // The echo is the stream position the reflow belongs at.
        DaemonMsg::Size(ws(100, 30)).encode(&mut daemon).unwrap();
        for _ in 0..600 {
            if columns(&term) == 100 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let text = all_text(&term);
        assert_eq!(columns(&term), 100, "the echo never reflowed the grid");
        assert!(
            text.contains("REPLAYED-SCREEN"),
            "the first paint's resize cost the replay; grid held:\n{text}"
        );
        drop(daemon);
    }

    /// The attach handshake reads off the same socket the reader will, far
    /// enough to tell an `Error` frame from a replay, and hands what it read
    /// on as the reader's starting buffer. A whole replay can already be
    /// sitting in the socket when it looks, so the prefix it takes is several
    /// frames wide and the split lands mid-frame — every byte of it has to
    /// reach the grid.
    #[test]
    fn the_attach_handshakes_prefix_carries_the_replay_it_swallowed() {
        anterm_ui_kit::config::pin_test_config_dir();
        let (mut client_side, mut daemon) = socket_pair();

        // The whole replay before the client looks: this is the daemon that
        // answered instantly, which is the daemon a local attach meets.
        DaemonMsg::Size(ws(80, 24)).encode(&mut daemon).unwrap();
        DaemonMsg::Snapshot(b"BIRTH-BANNER\r\n".to_vec())
            .encode(&mut daemon)
            .unwrap();
        DaemonMsg::Size(ws(120, 40)).encode(&mut daemon).unwrap();
        let mut bulk = Vec::new();
        for i in 0..400 {
            bulk.extend_from_slice(format!("scrollback row {i}\r\n").as_bytes());
        }
        bulk.extend_from_slice(b"LAST-ROW-OF-THE-RING\r\n");
        DaemonMsg::Snapshot(bulk).encode(&mut daemon).unwrap();

        let buffered =
            attach_reply_prefix(&mut client_side, 1, std::time::Duration::from_secs(2)).unwrap();
        assert!(
            !buffered.is_empty(),
            "the handshake read nothing, so it proves nothing"
        );
        let term = RemoteTerminal::from_stream_with(
            client_side,
            TermSize::new(80, 24),
            buffered,
            PtySource::Raw,
        )
        .unwrap();

        let text = settled(&term, &["BIRTH-BANNER", "LAST-ROW-OF-THE-RING"]);
        assert!(text.contains("BIRTH-BANNER"), "grid held:\n{text}");
        assert!(
            text.contains("LAST-ROW-OF-THE-RING"),
            "the replay the handshake swallowed never reached the grid"
        );
        drop(daemon);
    }

    /// Issue #711: what a prompt report costs a replay that ended inside the
    /// alternate screen.
    ///
    /// A re-attach replays the pane's ring and then the pane's *state*, and
    /// the state ends with the shell's prompt status. `active && at_prompt`
    /// makes the reader scrub the TUI modes it finds in the grid, alternate
    /// screen first — see `prompt_report_scrubs_stale_tui_modes`, which is the
    /// case that is meant to reach it: a program that died without its
    /// `?1049l` leaves the grid stranded on a screen nothing owns, and the
    /// shell's next prompt is what proves it stale.
    ///
    /// On a replay the alternate screen the scrub finds is the one the ring
    /// just rebuilt, and `?1049l` does not undo it — it swaps it away. The
    /// grid is left holding the primary screen underneath: the banner and the
    /// command line the pane was born with, which is exactly what #711's
    /// reporter photographed. The client cannot tell the two apart from the
    /// frame, so the daemon decides — it declines to claim a prompt while a
    /// foreground command owns the pty (`daemon::pane::replayed_at_prompt`).
    /// Both halves of that contract are pinned here, from the side that pays
    /// for it.
    #[test]
    fn a_replayed_prompt_report_decides_whether_the_alternate_screen_survives() {
        anterm_ui_kit::config::pin_test_config_dir();

        // A re-attach, as the daemon writes one: the shell's banner on the
        // primary screen, the agent's alternate screen drawn over it, then the
        // pane's stored state. The trailing `Output` is a barrier — frames are
        // applied in order, so a grid that holds it has already been through
        // the prompt report, and neither half can pass on a race.
        let replayed_pane = |at_prompt: bool| {
            let (client_side, mut daemon) = socket_pair();
            let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
            DaemonMsg::Size(ws(80, 24)).encode(&mut daemon).unwrap();
            DaemonMsg::Snapshot(
                b"BIRTH-BANNER\r\nMac:Accounts joe$ claude --resume --fork-session\r\n".to_vec(),
            )
            .encode(&mut daemon)
            .unwrap();
            DaemonMsg::Snapshot(b"\x1b[?1049h\x1b[2J\x1b[HAGENT-SCREEN\r\n".to_vec())
                .encode(&mut daemon)
                .unwrap();
            DaemonMsg::Prompt {
                active: true,
                at_prompt,
                last_exit: Some(0),
            }
            .encode(&mut daemon)
            .unwrap();
            DaemonMsg::Output(b"REPORT-APPLIED\r\n".to_vec())
                .encode(&mut daemon)
                .unwrap();
            let text = settled(&term, &["REPORT-APPLIED"]);
            drop(daemon);
            text
        };

        // A program owns the pane, so the replay reports no prompt and the
        // screen the ring rebuilt is the screen the pane shows.
        let text = replayed_pane(false);
        assert!(
            text.contains("REPORT-APPLIED"),
            "the barrier never arrived, so nothing below is tested; grid held:\n{text}"
        );
        assert!(
            text.contains("AGENT-SCREEN"),
            "a replay that does not claim a prompt must leave the alternate screen it \
             rebuilt alone; grid held:\n{text}"
        );

        // The same replay under the stale claim: the scrub swaps the agent's
        // screen away, and what the pane comes back showing is the banner it
        // was born with. That is #711.
        let text = replayed_pane(true);
        assert!(
            !text.contains("AGENT-SCREEN"),
            "a prompt report still has to scrub a stranded alternate screen, or the daemon's \
             gate is load-bearing for nothing; grid held:\n{text}"
        );
        assert!(
            text.contains("BIRTH-BANNER"),
            "what the scrub leaves is the primary screen underneath; grid held:\n{text}"
        );
    }

    // ---- The switch, end to end, with a real daemon pane ----------------
    //
    // Everything above drives the client half against a scripted daemon. This
    // drives the real one: a live `DaemonPane` over a real pty, in this
    // process, with its `DaemonMsg` stream forwarded onto a socket pair the
    // way `spawn_writer` forwards it, so an attach here is the same attach a
    // re-attach makes. It is the switch (#711) minus gpui: attach, resize to
    // the geometry the window actually has, produce output, drop the client,
    // produce more, attach again — and read the grid the second client ends up
    // with.

    /// A client hung off `pane`, the way `stream_pane_with_attach` hangs one
    /// off it: subscribe, forward every queued message onto the wire, and read
    /// the client's own frames back the way `run_stream` does — epoch guard
    /// included, so a displaced client's resize is dropped here exactly as the
    /// daemon drops it.
    ///
    /// The route is a resize-echoing one, which is what the local daemon is:
    /// the grid must not reflow until the `Size` the daemon echoes back.
    fn attach_client(
        pane: &std::sync::Arc<anterm_core::daemon::pane::DaemonPane>,
    ) -> (u64, RemoteTerminal, std::thread::JoinHandle<()>) {
        let (client_side, daemon_side) = socket_pair();
        let mut daemon_write = daemon_side.try_clone().expect("clone the daemon half");
        let (tx, rx) = std::sync::mpsc::channel::<DaemonMsg>();
        let epoch = pane.attach(tx);
        let gate = pane.gate();
        let forward = std::thread::spawn(move || {
            while let Ok(msg) = rx.recv() {
                let drained = match &msg {
                    DaemonMsg::Output(b) | DaemonMsg::Image(b) => b.len(),
                    _ => 0,
                };
                let ok = msg.encode(&mut daemon_write).is_ok();
                if drained > 0 {
                    gate.sub(drained);
                }
                if !ok {
                    break;
                }
            }
        });
        {
            let pane = pane.clone();
            let mut daemon_read = daemon_side;
            std::thread::spawn(move || {
                use std::io::Read as _;
                let mut pending: Vec<u8> = Vec::new();
                let mut chunk = [0u8; 65536];
                loop {
                    while let Ok(Some((kind, payload))) =
                        anterm_core::daemon::protocol::take_frame(&mut pending)
                    {
                        match ClientMsg::from_frame(kind, payload) {
                            Ok(ClientMsg::Input(bytes)) if pane.controls(epoch) => {
                                pane.write_input(&bytes)
                            }
                            Ok(ClientMsg::Resize(size)) if pane.controls(epoch) => {
                                pane.resize(size)
                            }
                            Ok(_) => {}
                            Err(_) => return,
                        }
                    }
                    match daemon_read.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => pending.extend_from_slice(&chunk[..n]),
                    }
                }
            });
        }
        let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24))
            .expect("a client over the pair");
        term.route = echoing_route();
        (epoch, term, forward)
    }

    /// A route whose daemon echoes `Size` when it applies a resize — which is
    /// what `PaneRoute::Local` is against a current daemon, and what the
    /// harness above implements.
    ///
    /// Built the way the neighbouring resize tests build one, rather than
    /// through a `PaneWorkspace`: routing a workspace is not what any of these
    /// cover, and going that way would have a change to `NativeSshSpec`'s
    /// serde shape fail them on an `unwrap` that has nothing to do with
    /// replay.
    fn echoing_route() -> PaneRoute {
        PaneRoute::Remote {
            header: Box::new(anterm_core::daemon::router::RouteHeader::wsl("Ubuntu-22.04")),
            resize_echo: true,
            size_lease: false,
            clear_scrollback: false,
        }
    }

    fn wait_for(term: &RemoteTerminal, needle: &str) -> bool {
        for _ in 0..600 {
            if all_text(term).contains(needle) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        false
    }

    /// Types `line` at the pane until the grid echoes `needle` back, and
    /// answers whether it ever did.
    ///
    /// One write is not enough for the *first* command a fresh pane is given.
    /// `spawn` returning means the pty exists, not that the shell behind it
    /// has started, printed a prompt, or begun reading — and on Windows the
    /// ConPTY has not necessarily connected the child to its input pipe yet,
    /// so bytes typed into that window reach nobody at all. On a loaded runner
    /// the window is wide: both cases showed up on Windows CI as a grid that
    /// was still completely empty after 15s, so not even a prompt had been
    /// printed, let alone an echo.
    ///
    /// Retyping is safe for everything asserted here: a line that did land and
    /// was merely slow simply runs twice, and every assertion is a `contains`.
    /// The later commands in these tests keep their single `write_input` —
    /// by then the shell has echoed once, which is proof it is reading.
    fn type_until_echoed(
        pane: &anterm_core::daemon::pane::DaemonPane,
        line: &[u8],
        term: &RemoteTerminal,
        needle: &str,
    ) -> bool {
        for _ in 0..15 {
            pane.write_input(line);
            for _ in 0..80 {
                if all_text(term).contains(needle) {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
        false
    }

    /// Switching workspaces drops every pane and attaches to the same daemon
    /// panes again. Whatever the pane put on screen while the window was
    /// elsewhere — and whatever it had already — has to come back with it.
    #[test]
    fn a_pane_re_attached_after_a_switch_gets_its_screen_back() {
        anterm_ui_kit::config::pin_test_config_dir();
        let pane = anterm_core::daemon::pane::DaemonPane::spawn(
            7711,
            std::env::current_dir().ok(),
            ws(80, 24),
            None,
            None,
            None,
            false,
            || {},
        )
        .expect("a pty-backed pane");

        // The window this pane is shown in, attaching for the first time.
        let (epoch, mut first, forward) = attach_client(&pane);
        // What the first paint does, from the same side it does it on: the
        // pane is not 80x24 on screen, so the real geometry goes down the link
        // and the grid waits for the daemon to echo it back.
        first.resize(TermSize::new(120, 40), 8, 17);
        assert!(
            type_until_echoed(
                &pane,
                b"echo BEFORE-THE-SWITCH\r",
                &first,
                "BEFORE-THE-SWITCH"
            ),
            "the pane never echoed the first command; grid held:\n{}",
            all_text(&first)
        );

        // Switching away: the view is dropped, which closes the link, and the
        // daemon gives up the seat.
        drop(first);
        pane.detach(epoch);
        drop(forward);

        // The pane keeps working while the window is showing another workspace.
        pane.write_input(b"echo DURING-THE-SWITCH\r");
        std::thread::sleep(std::time::Duration::from_millis(600));

        // Switching back: a brand new view, attaching at the hardcoded size.
        //
        // It deliberately never resizes. Resizing is the workaround #711's
        // reporter found — it makes the daemon `TIOCSWINSZ` the pty and the
        // *child* repaint, which would put the screen back whether or not the
        // replay ever arrived, and would also feed the grid a `Size` echo that
        // `resize_state` sends unconditionally. Everything asserted below has
        // to come from the replay itself.
        let (_epoch, second, forward) = attach_client(&pane);
        let landed = wait_for(&second, "DURING-THE-SWITCH");
        let text = all_text(&second);
        let width = columns(&second);
        pane.kill();
        drop(forward);

        assert!(
            landed,
            "the re-attached pane never got the output produced while it was hidden; \
             grid held:\n{text}"
        );
        assert!(
            text.contains("BEFORE-THE-SWITCH"),
            "the re-attached pane lost the screen it had before the switch; grid held:\n{text}"
        );
        // Nothing resized this client, so 120 can only have come from the
        // `Size` frame the replay sends ahead of the segment recorded at it.
        assert_eq!(
            width, 120,
            "the replay's geometry never reached the grid, so it is still at the attach size"
        );
    }

    /// A switch can rebuild a window twice — a second hydration lands while the
    /// first one's panes are still draining their replay — so the same pane is
    /// attached to twice in quick succession and the window keeps the later
    /// view. That view must hold the screen, and it must still be the one the
    /// daemon obeys: the displaced attach's teardown must not take the seat
    /// with it.
    ///
    /// The needle is weaker here than in the switch test above, and knowingly
    /// so: the pane is attached to throughout, and a ConPTY that owns the whole
    /// viewport reprints what is on screen as ordinary live output, so
    /// `RACED-OUTPUT` can reach the second view without the replay. What only
    /// the replay can supply is the geometry — nothing resizes this client —
    /// and what only the seat can supply is `AFTER-THE-RACE`. Those two are the
    /// assertions that discriminate.
    #[test]
    fn the_later_of_two_racing_attaches_keeps_the_screen_and_the_seat() {
        anterm_ui_kit::config::pin_test_config_dir();
        let pane = anterm_core::daemon::pane::DaemonPane::spawn(
            7712,
            std::env::current_dir().ok(),
            ws(80, 24),
            None,
            None,
            None,
            false,
            || {},
        )
        .expect("a pty-backed pane");

        let (first_epoch, mut first, first_forward) = attach_client(&pane);
        first.resize(TermSize::new(120, 40), 8, 17);
        assert!(
            type_until_echoed(&pane, b"echo RACED-OUTPUT\r", &first, "RACED-OUTPUT"),
            "the pane never echoed; grid held:\n{}",
            all_text(&first)
        );

        // The second rebuild attaches before the first one's view is dropped.
        let (second_epoch, second, second_forward) = attach_client(&pane);
        drop(first);
        drop(first_forward);
        // The displaced connection detaches on its way out, epoch-guarded.
        pane.detach(first_epoch);

        // Never resized, for the reason the switch test is not: a resize would
        // repaint the pane from the child and echo a `Size` back, and both are
        // exactly what must not be allowed to stand in for the replay.
        let landed = wait_for(&second, "RACED-OUTPUT");
        let width = columns(&second);
        // Two halves of "the seat survived", and only one of them can catch a
        // detach that took it. `controls` answers from `subscriber_epoch`
        // alone, and `DaemonPane::detach` clears `subscriber` without ever
        // touching that counter — so it says the pane would obey this view's
        // input and resizes, but it would keep saying so with the epoch guard
        // stripped out of `detach`. `live` is what notices that: a detach that
        // dropped the wrong subscriber silences the surviving view.
        let controls = pane.controls(second_epoch);
        pane.write_input(b"echo AFTER-THE-RACE\r");
        let live = wait_for(&second, "AFTER-THE-RACE");
        let text = all_text(&second);
        pane.kill();
        drop(second_forward);

        assert!(
            landed,
            "the second attach's replay was lost; grid held:\n{text}"
        );
        assert_eq!(
            width, 120,
            "the second attach's replay carried no geometry, so the grid is still at the \
             attach size"
        );
        assert!(
            controls,
            "the pane no longer answers to the surviving view, so nothing it \
             types or resizes would reach the pty"
        );
        assert!(
            live,
            "the displaced attach's detach took the live seat: the surviving \
             view stopped receiving output"
        );
    }

    /// What a physical Escape is written to the pane as, by the encoder a
    /// local ConPTY pane uses (#1155 is a ConPTY-only symptom), once the
    /// reader has applied everything up to `barrier`. Also returns the reply
    /// a `CSI ? u` sent after the barrier gets.
    fn escape_after(frames: Vec<DaemonMsg>, barrier: &str) -> (Vec<u8>, Option<String>) {
        use crate::input::{KeyFlags, keystroke_to_bytes};
        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, mut daemon) = socket_pair();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        for frame in frames {
            frame.encode(&mut daemon).unwrap();
        }
        DaemonMsg::Output(format!("{barrier}\x1b[?u").into_bytes())
            .encode(&mut daemon)
            .unwrap();
        let text = settled(&term, &[barrier]);
        assert!(text.contains(barrier), "the barrier never arrived:\n{text}");
        let mut reply = None;
        for _ in 0..200 {
            while let Ok(event) = term.events.try_recv() {
                if let AlacEvent::PtyWrite(text) = event {
                    reply = Some(text);
                }
            }
            if reply.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let mode = *term.term.lock().mode();
        let flags = KeyFlags::from_mode_with_local_conpty(&mode, true);
        let escape = keystroke_to_bytes(&gpui::Keystroke::parse_with_os("escape", anterm_ui_kit::platform::current_os()).unwrap(), flags);
        drop(daemon);
        (escape.unwrap_or_default(), reply)
    }

    fn at_prompt() -> DaemonMsg {
        DaemonMsg::Prompt {
            active: true,
            at_prompt: true,
            last_exit: Some(130),
        }
    }

    /// #1155: a program pushed kitty keyboard flags on the main screen and
    /// again on the alternate one, and was killed there. Once the prompt is
    /// back, the next program to enter the alternate screen must not find the
    /// dead one's flags on its stack.
    #[test]
    fn a_prompt_forgets_kitty_flags_a_killed_program_left_on_either_screen() {
        let (escape, reply) = escape_after(
            vec![
                DaemonMsg::Output(b"\x1b[>1u\x1b[?1049h\x1b[>7u".to_vec()),
                at_prompt(),
                DaemonMsg::Output(b"\x1b[?1049h".to_vec()),
            ],
            "NEXT-PROGRAM",
        );
        assert_eq!(escape, b"\x1b", "Escape must reach the next program bare");
        assert_eq!(
            reply.as_deref(),
            Some("\x1b[?0u"),
            "a flags query must agree with the flags in effect"
        );
    }

    /// #1155: a program that left the alternate screen *before* popping
    /// popped the main screen's stack, so its push stayed on the alternate
    /// one while no flags were in effect at the prompt.
    #[test]
    fn a_prompt_forgets_a_kitty_push_stranded_on_the_inactive_screen() {
        let (escape, _) = escape_after(
            vec![
                DaemonMsg::Output(b"\x1b[?1049h\x1b[>7u\x1b[?1049l\x1b[<u".to_vec()),
                at_prompt(),
                DaemonMsg::Output(b"\x1b[?1049h".to_vec()),
            ],
            "NEXT-PROGRAM",
        );
        assert_eq!(escape, b"\x1b");
    }

    /// A stale entry under the next program's own push must not come back
    /// when it pops.
    #[test]
    fn a_pop_after_the_prompt_does_not_restore_stale_kitty_flags() {
        let (escape, _) = escape_after(
            vec![
                DaemonMsg::Output(b"\x1b[>7u".to_vec()),
                at_prompt(),
                DaemonMsg::Output(b"\x1b[>1u\x1b[<u".to_vec()),
            ],
            "NEXT-PROGRAM",
        );
        assert_eq!(escape, b"\x1b");
    }

    /// The guard on the other side: flags a program negotiates after the
    /// prompt returned are its own and take effect.
    #[test]
    fn kitty_flags_pushed_after_the_prompt_take_effect() {
        let (escape, reply) = escape_after(
            vec![
                DaemonMsg::Output(b"\x1b[?1049h\x1b[>7u".to_vec()),
                at_prompt(),
                DaemonMsg::Output(b"\x1b[?1049h\x1b[>1u".to_vec()),
            ],
            "NEXT-PROGRAM",
        );
        assert_eq!(escape, b"\x1b[27u");
        assert_eq!(reply.as_deref(), Some("\x1b[?1u"));
    }

    /// The same on a re-attach. The replay carries one prompt report for the
    /// whole ring, and here it says a program owns the pane — so the prompt
    /// the shell drew between the dead program and this one is only in the
    /// ring's bytes, and has to clear the flags where it stands.
    #[test]
    fn a_replay_forgets_kitty_flags_from_before_an_earlier_prompt() {
        let (escape, _) = escape_after(
            vec![
                DaemonMsg::Size(ws(80, 24)),
                DaemonMsg::Snapshot(b"\x1b[>1u\x1b[?1049h\x1b[>7u".to_vec()),
                DaemonMsg::Snapshot(
                    b"\x1b]133;D;130\x07\x1b]133;A\x07$ \x1b]133;B\x07codex\r\n\x1b]133;C\x07"
                        .to_vec(),
                ),
                DaemonMsg::Snapshot(b"\x1b[?1049h".to_vec()),
                DaemonMsg::Prompt {
                    active: true,
                    at_prompt: false,
                    last_exit: Some(130),
                },
            ],
            "NEXT-PROGRAM",
        );
        assert_eq!(escape, b"\x1b");

        // What the program after the prompt pushes is replayed as it was.
        let (escape, _) = escape_after(
            vec![
                DaemonMsg::Size(ws(80, 24)),
                DaemonMsg::Snapshot(b"\x1b[>7u\x1b]133;A\x07\x1b]133;C\x07\x1b[>1u".to_vec()),
                DaemonMsg::Prompt {
                    active: true,
                    at_prompt: false,
                    last_exit: None,
                },
            ],
            "NEXT-PROGRAM",
        );
        assert_eq!(escape, b"\x1b[27u");
    }
}

#[cfg(windows)]
mod windows_tests {
    use super::super::*;

    use super::replay_tests::socket_pair as tcp_pair;

    /// On Windows, `shutdown()` does not wake a thread parked in a blocking
    /// `read` on the same socket (it does on unix). `detach_link`,
    /// `adopt_relink`, and `Drop` all shut the writer down and then `join()`
    /// the reader thread, counting on that wake-up — so when the peer stays
    /// silent (a routed pane whose SSH leg went zombie: nothing arrives, no
    /// FIN ever comes), the join blocks its caller, which in production is
    /// the UI thread. This is the whole-window "not responding" hang right
    /// after a remote workspace reconnects.
    #[test]
    fn detach_link_returns_promptly_when_the_peer_stays_silent() {
        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, daemon_side) = tcp_pair();
        let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        // Let the reader thread park in read() before tearing down.
        std::thread::sleep(std::time::Duration::from_millis(200));

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            term.detach_link();
            let _ = tx.send(());
        });
        let outcome = rx.recv_timeout(std::time::Duration::from_secs(3));
        drop(daemon_side);
        assert!(
            outcome.is_ok(),
            "detach_link blocked for over 3s against a silent peer: shutdown() \
             did not unblock the reader, so join() hangs the calling thread"
        );
    }

    /// The reconnect path: `relink_panes` calls this on the UI thread right
    /// after a remote workspace re-attaches, with the old link still parked
    /// on a zombie route. It must swap links promptly, and the adopted link
    /// must actually work.
    #[test]
    fn adopt_relink_swaps_links_promptly_when_the_old_peer_stays_silent() {
        anterm_ui_kit::config::pin_test_config_dir();
        let (old_client, mut _old_daemon) = tcp_pair();
        let (new_client, mut new_daemon) = tcp_pair();
        let term = RemoteTerminal::from_stream(old_client, TermSize::new(80, 24)).unwrap();
        // Let the old reader park in read() on the silent link.
        std::thread::sleep(std::time::Duration::from_millis(200));

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut term = term;
            term.adopt_relink(
                new_client,
                Vec::new(),
                &PaneRoute::Local,
                TermSize::new(80, 24),
                8,
                16,
            )
            .unwrap();
            let _ = tx.send(term);
        });
        let term = rx
            .recv_timeout(std::time::Duration::from_secs(3))
            .expect("adopt_relink blocked for over 3s against a silent old peer");

        DaemonMsg::Output(b"hi".to_vec())
            .encode(&mut new_daemon)
            .unwrap();
        let mut first = ' ';
        for _ in 0..200 {
            first = term.term.lock().grid()[alacritty_terminal::index::Line(0)]
                [alacritty_terminal::index::Column(0)]
            .c;
            if first == 'h' {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(first, 'h', "output on the adopted link must reach the grid");
        assert!(
            !term.exited_flag.load(Ordering::SeqCst),
            "retiring the old reader must not mark the pane as exited"
        );

        // Late traffic on the abandoned link must be ignored wholesale: an
        // Exited frame processed by the retired reader would flip
        // `child_exited` and close the freshly adopted pane from under the
        // user. Give the retired reader a full quit-poll tick to (mis)handle
        // it before checking.
        let _ = DaemonMsg::Exited { code: Some(0) }.encode(&mut _old_daemon);
        std::thread::sleep(std::time::Duration::from_millis(700));
        assert!(
            !term.child_exited(),
            "an Exited frame on the abandoned link must not close the pane"
        );
        assert!(
            !term.exited_flag.load(Ordering::SeqCst),
            "the abandoned link must not tear the adopted pane down"
        );
    }

    /// What the daemon replays into a pane restored from a stored screen, in
    /// the frames and the order it sends them: the dead pane's screen, then the
    /// preamble, then the new shell's own first output.
    fn replay_restore_into(daemon: &mut std::net::TcpStream, old_screen: &[u8], shell: &[u8]) {
        let size = WinSize {
            cols: 40,
            rows: 10,
            cell_w: 8,
            cell_h: 17,
        };
        DaemonMsg::Size(size).encode(daemon).unwrap();
        DaemonMsg::Snapshot(old_screen.to_vec())
            .encode(daemon)
            .unwrap();
        DaemonMsg::Size(size).encode(daemon).unwrap();
        DaemonMsg::Snapshot(anterm_core::daemon::pane::restore_preamble(Some(
            "the shell below is new",
        )))
        .encode(daemon)
        .unwrap();
        DaemonMsg::Output(shell.to_vec()).encode(daemon).unwrap();
    }

    fn row(term: &RemoteTerminal, line: i32) -> String {
        use alacritty_terminal::grid::Dimensions as _;
        use alacritty_terminal::index::{Column, Line};
        let term = term.term.lock();
        let grid = term.grid();
        (0..grid.columns())
            .map(|c| grid[Line(line)][Column(c)].c)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    /// A restored screen must not be sitting in the viewport when the new
    /// shell's ConPTY starts drawing on it.
    ///
    /// conhost addresses its own screen buffer absolutely — PSReadLine repaints
    /// the line being typed with `ESC[6;20H` and conhost frames it the same way
    /// — and that buffer starts blank with the cursor at the top-left. Restored
    /// output is output conhost never produced: left on screen it shifts every
    /// row conhost names, so the first keystroke repaints the input line on top
    /// of the old text instead of at the prompt. The restored screen belongs in
    /// scrollback, where it survives without claiming a row.
    #[test]
    fn a_restored_screen_leaves_the_new_shell_the_viewport_conpty_thinks_it_has() {
        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = tcp_pair();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(40, 10)).unwrap();

        // Five lines of the dead pane, then the shell painting its prompt the
        // way conhost does: at row 1 of a buffer it believes is blank.
        replay_restore_into(
            &mut daemon_side,
            b"line one\r\nline two\r\nline three\r\nline four\r\nline five\r\n",
            b"\x1b[?25l\x1b[1;1HPS C:\\> \x1b[?25h",
        );

        let mut top = String::new();
        for _ in 0..400 {
            top = row(&term, 0);
            if top.starts_with("PS C:") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            top, "PS C:\\>",
            "the shell's prompt paints where conhost put it"
        );

        for line in 1..10 {
            assert_eq!(
                row(&term, line),
                "",
                "row {line} still holds restored output, so conhost and the client \
                 disagree about which row is which: the next repaint of the input \
                 line lands on the old screen instead of at the prompt"
            );
        }

        // Kept, not erased: `ESC[2J` on the primary screen scrolls the viewport
        // into history, so the screen the daemon restored is one scroll away.
        let depth = {
            use alacritty_terminal::grid::Dimensions as _;
            term.term.lock().grid().history_size() as i32
        };
        let history: Vec<String> = (-depth..0).map(|line| row(&term, line)).collect();
        for wanted in [
            "line one",
            "line two",
            "line three",
            "line four",
            "line five",
        ] {
            assert!(
                history.iter().any(|row| row == wanted),
                "{wanted:?} is not in the scrollback; the restored screen was erased \
                 rather than scrolled away. History holds {history:?}"
            );
        }
    }
}

/// Issue #430, and #774 for the half of it that was still open.
///
/// Whether the reader puts back a parked cursor is a property of the pty this
/// pane is attached to, not of the platform the client was compiled for — so
/// these drive both answers on every platform, over a real socket pair. Before
/// #774 the decision was `cfg!(windows)`, which meant a Windows client talking
/// to a Linux host repaired a cursor no conhost had parked, and the `wq` typed
/// after vim's `:` landed on the row being edited.
mod parked_cursor_tests {
    use super::replay_tests::socket_pair;
    use super::super::*;
    use anterm_core::daemon::protocol::RemoteKind;
    use std::io::Write as _;

    fn terminal_on(pty: PtySource, size: TermSize) -> (RemoteTerminal, Stream) {
        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, daemon_side) = socket_pair();
        let term = RemoteTerminal::from_stream_with(client_side, size, Vec::new(), pty)
            .expect("a terminal over a socket pair");
        assert_eq!(term.is_local_conpty(), pty == PtySource::LocalConpty);
        (term, daemon_side)
    }

    /// Feeds one conhost-shaped repaint and reports the cell the cursor ends on,
    /// waiting for the `X` the frame paints so the reader is known to be done.
    fn cursor_after_conpty_frame(pty: PtySource, frame: &[u8]) -> (i32, usize) {
        let (term, mut daemon_side) = terminal_on(pty, TermSize::new(80, 24));

        // Where the TUI put the cursor before conhost repainted over it.
        DaemonMsg::Output(b"\x1b[6;4H".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        DaemonMsg::Output(frame.to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        for _ in 0..600 {
            {
                let t = term.term.lock();
                let painted = t.grid()[alacritty_terminal::index::Line(19)]
                    [alacritty_terminal::index::Column(1)]
                .c;
                if painted == 'X' {
                    let point = t.grid().cursor.point;
                    return (point.line.0, point.column.0);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the reader never applied the frame");
    }

    #[test]
    fn a_local_route_is_a_conpty_only_where_conhost_exists() {
        assert_eq!(
            PtySource::for_route(&PaneRoute::Local),
            if cfg!(windows) {
                PtySource::LocalConpty
            } else {
                PtySource::Raw
            },
        );
    }

    #[test]
    fn a_routed_pane_reads_a_raw_pty_whatever_the_client_was_built_for() {
        let spec: NativeSshSpec = serde_json::from_str(
            r#"{"host":"linux-box","port":22,"user":"dev","auth_mode":"auto"}"#,
        )
        .unwrap();
        // A remote workspace only ever installs a `anterm-server` on Linux or
        // macOS (`install::asset::asset_for_uname`), and a WSL workspace is a
        // Linux server too, so a routed pane's pty is a raw one — the far end
        // is never a conhost, whoever is dialling it.
        for header in [
            anterm_core::daemon::router::RouteHeader::ssh(spec),
            anterm_core::daemon::router::RouteHeader::wsl("Ubuntu-22.04"),
        ] {
            let route = PaneRoute::Remote {
                header: Box::new(header),
                resize_echo: false,
                size_lease: false,
                clear_scrollback: false,
            };
            assert_eq!(PtySource::for_route(&route), PtySource::Raw);
        }
    }

    #[test]
    fn a_conpty_frame_that_shows_the_cursor_over_an_erase_keeps_the_cell_it_hid_on() {
        assert_eq!(
            cursor_after_conpty_frame(
                PtySource::LocalConpty,
                b"\x1b[?25l\x1b[20;2HX\x1b[K\x1b[m\x1b[22;42H\x1b[K\x1b[?25h",
            ),
            (5, 3),
            "conhost parked the cursor on the cell it erased last; the cursor \
             belongs where it was when the repaint hid it"
        );
    }

    #[test]
    fn the_same_frame_off_a_raw_pty_leaves_the_cursor_where_the_frame_left_it() {
        assert_eq!(
            cursor_after_conpty_frame(
                PtySource::Raw,
                b"\x1b[?25l\x1b[20;2HX\x1b[K\x1b[m\x1b[22;42H\x1b[K\x1b[?25h",
            ),
            (21, 41),
            "with no conhost in between the stream is the application's own, \
             and the cell it left the cursor on is the cell it meant"
        );
    }

    #[test]
    fn a_conpty_frame_that_moves_the_cursor_before_showing_it_is_obeyed() {
        assert_eq!(
            cursor_after_conpty_frame(
                PtySource::LocalConpty,
                b"\x1b[?25l\x1b[20;2HX\x1b[K\x1b[m\x1b[22;42H\x1b[K\x1b[9;9H\x1b[?25h"
            ),
            (8, 8),
            "the frame painted the cursor somewhere on purpose"
        );
    }

    /// Vim opens its command line with exactly the shape the parked-cursor
    /// scanner calls parked — hide, move around to paint, end on the `:` it
    /// wrote — and then echoes every following keystroke as a bare byte at
    /// wherever that left the cursor. Putting the cursor back on a raw pty
    /// therefore does not straighten out a stray caret, it drops `wq!` onto the
    /// row vim was editing. Bytes below are a capture of vim 9 on a 20x11 pty.
    #[test]
    fn a_raw_pty_repaint_keeps_the_cursor_the_frame_left_so_the_echo_lands_on_it() {
        let (term, mut daemon_side) = terminal_on(PtySource::Raw, TermSize::new(20, 11));

        let mut stream: Vec<u8> = Vec::new();
        // `vim test.md`: the alternate screen, the file, cursor home.
        stream.extend_from_slice(b"\x1b[?1049h\x1b[H\x1b[2J\x1b[1;1H123456789\x1b[1;1H");
        // Esc, then `:` — two bracketed repaints, the second ending on the `:`
        // vim wrote at the head of the command line.
        stream.extend_from_slice(b"\x1b[?25l\x1b[m\x1b[11;10H^[\x1b[1;1H\x1b[?25h");
        stream.extend_from_slice(b"\x1b[?25l\x1b[11;10H  \x1b[1;1H\x07\x1b[?25h");
        stream.extend_from_slice(
            b"\x1b[?25l\x1b[11;10H:\x1b[1;1H\x1b[11;1H\x1b[K\x1b[11;1H:\x1b[?25h",
        );
        // `w`, `q`, `!`: vim echoes them with no positioning of their own.
        stream.extend_from_slice(b"wq!");
        DaemonMsg::Output(stream).encode(&mut daemon_side).unwrap();
        daemon_side.flush().unwrap();

        let row = |t: &Term<EventProxy>, line: i32| -> String {
            (0..20)
                .map(|col| {
                    t.grid()[alacritty_terminal::index::Line(line)]
                        [alacritty_terminal::index::Column(col)]
                    .c
                })
                .collect::<String>()
                .trim_end()
                .to_string()
        };

        // The whole batch is applied under one lock, so the `:` landing on the
        // command line means every byte after it landed too.
        let mut command_line = String::new();
        let mut edited = String::new();
        for _ in 0..600 {
            {
                let t = term.term.lock();
                command_line = row(&t, 10);
                edited = row(&t, 0);
            }
            if command_line.starts_with(':') {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            command_line, ":wq!",
            "the keystrokes belong after the `:` the repaint ended on"
        );
        assert_eq!(
            edited, "123456789",
            "and nothing of them belongs on the row vim was editing"
        );
    }

    /// The route cannot tell a native-SSH pane from a local shell: both are
    /// spawned through this machine's daemon. What tells them apart is the
    /// `RemoteContext` the daemon sends — and it has to, because a window
    /// reopening onto an existing native-SSH pane attaches by id and has
    /// nothing else to go on.
    #[test]
    fn a_native_ssh_context_turns_the_repair_off_mid_stream() {
        let (term, mut daemon_side) = terminal_on(PtySource::LocalConpty, TermSize::new(80, 24));

        DaemonMsg::RemoteContext(Some(RemoteContext {
            kind: RemoteKind::NativeSsh,
            argv: Vec::new(),
            target: "dev@linux-box".into(),
        }))
        .encode(&mut daemon_side)
        .unwrap();
        daemon_side.flush().unwrap();
        for _ in 0..600 {
            if term.remote_context().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            term.remote_context().is_some(),
            "the reader never applied the context"
        );
        assert!(!term.is_local_conpty());

        DaemonMsg::Output(b"\x1b[6;4H".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        DaemonMsg::Output(b"\x1b[?25l\x1b[20;2HX\x1b[K\x1b[m\x1b[22;42H\x1b[K\x1b[?25h".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        let mut cursor = (0, 0);
        for _ in 0..600 {
            {
                let t = term.term.lock();
                let painted = t.grid()[alacritty_terminal::index::Line(19)]
                    [alacritty_terminal::index::Column(1)]
                .c;
                if painted == 'X' {
                    let point = t.grid().cursor.point;
                    cursor = (point.line.0, point.column.0);
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            cursor,
            (21, 41),
            "an ssh channel's bytes are the far host's own; nothing parked that cursor"
        );
    }
}

/// Issue #774, from the client's end. A full-screen tool sets its modes once
/// and the replay ring drops them, so the daemon re-sends them from what it
/// folded out of the stream ([`anterm_core::term_modes`]). This is the
/// other half of that: the bytes it re-sends have to land the emulator back
/// where the application left it, because those are the modes `wheel_route`
/// reads before it decides the pane has a scrollback to move at all.
mod replayed_mode_tests {
    use super::replay_tests::socket_pair;
    use super::super::*;
    use alacritty_terminal::term::TermMode;
    use std::io::Write as _;
    use anterm_core::term_modes::TerminalModes;

    #[test]
    fn a_replayed_mode_frame_puts_the_client_back_on_the_alternate_screen() {
        anterm_ui_kit::config::pin_test_config_dir();
        // `btop`'s startup prefix, folded the way the daemon folds it out of
        // the pty — and then re-sent from the fold, the ring having dropped
        // the bytes themselves hours ago.
        let mut modes = TerminalModes::new();
        modes.feed(b"\x1b[?1049h\x1b[?1002h\x1b[?1006h");

        let (client_side, mut daemon_side) = socket_pair();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        DaemonMsg::Snapshot(modes.restore_bytes().expect("a fold with modes in it"))
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        for _ in 0..600 {
            if term.term.lock().mode().contains(TermMode::ALT_SCREEN) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let mode = *term.term.lock().mode();
        assert!(
            mode.contains(TermMode::ALT_SCREEN),
            "the pane belongs on the alternate screen it never left: {mode:?}"
        );
        // Reporting first, SGR encoding with it: `wheel_route` sends the wheel
        // to the application on the first and encodes the report with the
        // second — see `wheel_routes_by_negotiated_mode_with_reporting_first`.
        assert!(
            mode.intersects(TermMode::MOUSE_MODE),
            "mouse reporting is what keeps the wheel off the scrollback: {mode:?}"
        );
        assert!(mode.contains(TermMode::SGR_MOUSE), "{mode:?}");
    }

    /// #1074, checked against the emulator itself rather than against our
    /// reading of it: wherever the ring's front falls, the prelude plus what
    /// the ring still holds has to leave the client's kitty keyboard flags
    /// where the whole stream leaves them — and its stacks too, which only
    /// show once the program pops or switches screens, so those are probed
    /// after.
    #[test]
    fn a_replay_prelude_lands_the_kitty_keyboard_flags_where_the_stream_does() {
        use alacritty_terminal::event::VoidListener;

        fn kitty_after(chunks: &[&[u8]], probes: &[&[u8]]) -> Vec<TermMode> {
            let config = terminal_config_from_user(&anterm_ui_kit::config::Config::default());
            let mut term = Term::new(config, &TermSize::new(40, 10), VoidListener);
            let mut processor: ansi::Processor = ansi::Processor::new();
            for chunk in chunks {
                processor.advance(&mut term, chunk);
            }
            let mut seen = vec![*term.mode() & TermMode::KITTY_KEYBOARD_PROTOCOL];
            for probe in probes {
                processor.advance(&mut term, probe);
                seen.push(*term.mode() & TermMode::KITTY_KEYBOARD_PROTOCOL);
            }
            seen
        }

        let streams: [&[u8]; 5] = [
            // An agent CLI: one push at startup, a pop and push around a
            // subprocess later on.
            b"\x1b[>1uhello\r\n\x1b[<u$ ls\r\n\x1b[>1uback\r\n",
            b"\x1b[>1u\x1b[?1049h\x1b[>7uframe\x1b[=3uframe\x1b[>5u",
            b"\x1b[>1uidle\x1b[=0;1u\x1b[=1;2utyping",
            b"\x1b[>1u\x1b[>3u\x1b[?1049h\x1b[>7u\x1b[<u\x1b[?1049l\x1b[<u\x1b[>2u",
            b"\x1b[>1u\x1bc\x1b[>4uafter a reset",
        ];
        let probes: [&[u8]; 7] = [
            b"\x1b[<u",
            b"\x1b[?1049h",
            b"\x1b[<u",
            b"\x1b[?1049l",
            b"\x1b[<u",
            b"\x1b[<u",
            b"\x1b[>8u",
        ];
        for stream in streams {
            let whole = {
                let mut m = TerminalModes::new();
                m.feed(stream);
                m
            };
            let expected = kitty_after(&[stream], &probes);
            for cut in (0..=stream.len()).filter(|&i| i == stream.len() || stream[i] == 0x1b) {
                let (dropped, kept) = stream.split_at(cut);
                let (mut head, mut ring) = (TerminalModes::new(), TerminalModes::new());
                head.feed(dropped);
                ring.feed(kept);
                // A ring that starts inside an alternate screen it later leaves
                // is replayed onto the primary one — the DEC restore's own
                // limit, not the keyboard's; there is no stack to push it to.
                let restores_alt = whole.is_on(1049) && !ring.is_on(1049);
                if head.is_on(1049) && !restores_alt {
                    continue;
                }
                let prelude = whole.replay_prelude(&head, &ring).unwrap_or_default();
                assert_eq!(
                    kitty_after(&[&prelude, kept], &probes),
                    expected,
                    "cut at {cut} of {:?}, prelude {:?}",
                    String::from_utf8_lossy(stream),
                    String::from_utf8_lossy(&prelude),
                );
            }
        }
    }
}

#[cfg(unix)]
mod unix_tests {
    use super::super::reader::stale_mode_resets;
    use super::super::*;
    use alacritty_terminal::term::TermMode;
    use std::io::Write;
    use std::os::unix::net::UnixStream;

    fn ssh_route() -> PaneRoute {
        let spec: NativeSshSpec = serde_json::from_str(
            r#"{"host":"build-box","port":22,"user":"me","auth_mode":"auto"}"#,
        )
        .unwrap();
        PaneRoute::Remote {
            header: Box::new(anterm_core::daemon::router::RouteHeader::ssh(spec).for_pane()),
            resize_echo: false,
            size_lease: false,
            clear_scrollback: false,
        }
    }

    #[test]
    fn a_local_pane_prefixes_nothing() {
        assert!(PaneRoute::Local.header().is_none());
        assert!(matches!(PaneRoute::default(), PaneRoute::Local));
    }

    #[test]
    fn only_a_local_pane_may_restart_the_local_daemon() {
        assert!(PaneRoute::Local.is_local());

        assert!(
            !ssh_route().is_local(),
            "a routed pane's disconnect is the remote's failure, not the local daemon's"
        );
        assert!(
            !PaneRoute::Unroutable("no ssh details".into()).is_local(),
            "nothing was ever asked of the local daemon"
        );
    }

    #[test]
    fn kitty_keyboard_negotiation_reports_the_requested_mode() {
        let config = terminal_config_from_user(&anterm_ui_kit::config::Config::default());
        assert!(config.kitty_keyboard);

        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        DaemonMsg::Output(b"\x1b[>7u\x1b[?u".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        let mut reply = None;
        for _ in 0..200 {
            while let Ok(event) = term.events.try_recv() {
                if let AlacEvent::PtyWrite(text) = event {
                    reply = Some(text);
                }
            }
            if reply.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        assert_eq!(reply.as_deref(), Some("\x1b[?7u"));
        assert!(
            term.term
                .lock()
                .mode()
                .contains(TermMode::DISAMBIGUATE_ESC_CODES)
        );
    }

    #[test]
    fn deep_keyboard_mode_pushes_leave_the_reader_alive() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        let mut payload = b"\x1b[>1u".repeat(4097);
        payload.extend_from_slice(b"\x1b[?u");
        DaemonMsg::Output(payload).encode(&mut daemon_side).unwrap();
        daemon_side.flush().unwrap();

        let mut reply = None;
        for _ in 0..200 {
            while let Ok(event) = term.events.try_recv() {
                if let AlacEvent::PtyWrite(text) = event {
                    reply = Some(text);
                }
            }
            if reply.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        assert_eq!(
            reply.as_deref(),
            Some("\x1b[?1u"),
            "the reader thread must survive a deep mode-push run and still answer queries"
        );
    }

    #[test]
    fn emoji_presentation_sequences_reserve_two_columns() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        DaemonMsg::Output("\u{2764}\u{FE0F}x".as_bytes().to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        let mut row = String::new();
        for _ in 0..200 {
            {
                let t = term.term.lock();
                let grid = t.grid();
                row.clear();
                for col in 0..3usize {
                    row.push(
                        grid[alacritty_terminal::index::Line(0)]
                            [alacritty_terminal::index::Column(col)]
                        .c,
                    );
                }
            }
            if row.contains('x') {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        assert_eq!(
            row, "\u{2764} x",
            "❤️ must hold two columns (glyph + spacer) before the next glyph"
        );
    }

    #[test]
    fn spawn_retry_only_for_daemon_disconnects() {
        let eof: anyhow::Error =
            std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "closed").into();
        assert!(daemon_disconnected_before_spawn_reply(&eof));

        let refused = anyhow::anyhow!("daemon refused Spawn: configured shell missing");
        assert!(!daemon_disconnected_before_spawn_reply(&refused));
    }

    #[test]
    fn only_a_dead_daemon_is_worth_starting_one_for() {
        let connect_failed = |kind| -> anyhow::Error {
            anyhow::Error::new(std::io::Error::new(kind, "no listener"))
                .context("connect to daemon at /tmp/anterm.sock")
        };
        assert!(daemon_not_listening(&connect_failed(
            std::io::ErrorKind::ConnectionRefused
        )));
        assert!(daemon_not_listening(&connect_failed(
            std::io::ErrorKind::NotFound
        )));

        let refused = anyhow::anyhow!("daemon refused Spawn: configured shell missing");
        assert!(!daemon_not_listening(&refused));
        let eof: anyhow::Error =
            std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "closed").into();
        assert!(!daemon_not_listening(&eof));
    }

    #[test]
    fn an_attach_to_a_missing_pane_is_an_error_not_a_disconnect() {
        let (mut client_side, mut daemon_side) = UnixStream::pair().unwrap();
        DaemonMsg::Error("no such pane 7".to_string())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        let err = attach_reply_prefix(&mut client_side, 7, attach_reply_wait(&PaneRoute::Local))
            .expect_err("a missing pane must fail");
        assert!(
            format!("{err:#}").contains("no such pane 7"),
            "the daemon's own wording is what names which pane went: {err:#}"
        );
    }

    #[test]
    fn an_attach_the_daemon_hangs_up_on_is_an_error() {
        let (mut client_side, daemon_side) = UnixStream::pair().unwrap();
        drop(daemon_side);
        assert!(
            attach_reply_prefix(&mut client_side, 7, attach_reply_wait(&PaneRoute::Local)).is_err()
        );
    }

    /// #673: a daemon that attached replays the pane's ring at once, and the
    /// ring always holds a segment, so even a pane that has printed nothing
    /// answers with a `Size`. A connection that stays silent for the whole wait
    /// is therefore one nobody is serving — read as success, it became a pane
    /// the window drew as restored while every keystroke, Ctrl-C included,
    /// went into a socket nothing drained.
    #[test]
    fn an_attach_nobody_answers_is_not_an_attach() {
        let (mut client_side, _daemon_side) = UnixStream::pair().unwrap();
        let wait = std::time::Duration::from_millis(200);
        let err = attach_reply_prefix(&mut client_side, 7, wait)
            .expect_err("silence for the whole wait must not pass for an attached pane");
        assert!(
            format!("{err:#}").contains("did not answer"),
            "the failure has to say the daemon never answered, not that the pane is gone: {err:#}"
        );
        assert!(
            attach_unanswered(&err),
            "silence is its own verdict, told apart from a refusal or a hangup"
        );
        // A refusal is not it: the pane really is gone then, and the caller
        // may say so.
        let (mut client_side, mut daemon_side) = UnixStream::pair().unwrap();
        DaemonMsg::Error("no such pane 7".into())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        let refused = attach_reply_prefix(&mut client_side, 7, wait).expect_err("refused");
        assert!(!attach_unanswered(&refused));
    }

    /// The relink retry loop keys off this split: a refusal means the pane is
    /// gone on its machine and asking again can only repeat the answer, while
    /// silence and a hangup are transport trouble worth another try.
    #[test]
    fn a_refused_attach_is_final_and_every_other_failure_is_not() {
        let wait = std::time::Duration::from_millis(200);

        let (mut client_side, mut daemon_side) = UnixStream::pair().unwrap();
        DaemonMsg::Error("no such pane 7".into())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        let refused = attach_reply_prefix(&mut client_side, 7, wait).expect_err("refused");
        assert!(attach_refused(&refused));
        assert!(
            format!("{refused:#}").contains("no such pane 7"),
            "the daemon's own words survive: {refused:#}"
        );

        let (mut client_side, _held_open) = UnixStream::pair().unwrap();
        let silent = attach_reply_prefix(&mut client_side, 7, wait).expect_err("silent");
        assert!(
            !attach_refused(&silent),
            "silence is not a verdict on the pane"
        );

        let (mut client_side, daemon_side) = UnixStream::pair().unwrap();
        drop(daemon_side);
        let hungup = attach_reply_prefix(&mut client_side, 7, wait).expect_err("hung up");
        assert!(
            !attach_refused(&hungup),
            "a hangup is not a verdict on the pane"
        );
    }

    /// The other side of the rule above: the frame a quiet pane does send is
    /// enough. Nothing else is required of the daemon for the attach to stand.
    #[test]
    fn a_size_frame_alone_is_a_live_attach() {
        let (mut client_side, mut daemon_side) = UnixStream::pair().unwrap();
        DaemonMsg::Size(WinSize {
            cols: 80,
            rows: 24,
            cell_w: 8,
            cell_h: 17,
        })
        .encode(&mut daemon_side)
        .unwrap();
        daemon_side.flush().unwrap();

        let buffered =
            attach_reply_prefix(&mut client_side, 7, std::time::Duration::from_millis(200))
                .expect("the daemon's first frame is the attach ack");
        assert!(
            !buffered.is_empty(),
            "the Size frame was read to classify the reply; it must reach the reader"
        );
    }

    #[test]
    fn a_local_attach_does_not_wait_as_long_as_a_remote_one() {
        let local = attach_reply_wait(&PaneRoute::Local);
        let remote = attach_reply_wait(&ssh_route());
        assert!(local < remote, "{local:?} must be the shorter wait");
        assert!(
            local <= std::time::Duration::from_secs(2),
            "the UI thread is holding still for this"
        );
    }

    #[test]
    fn a_live_attach_hands_its_replay_bytes_to_the_reader() {
        anterm_ui_kit::config::pin_test_config_dir();
        let (mut client_side, mut daemon_side) = UnixStream::pair().unwrap();
        DaemonMsg::Snapshot(b"hello".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        let buffered =
            attach_reply_prefix(&mut client_side, 7, attach_reply_wait(&PaneRoute::Local))
                .expect("a live pane attaches");
        assert!(
            !buffered.is_empty(),
            "the classification read the Snapshot frame; it must come back"
        );
        let term = RemoteTerminal::from_stream_with(
            client_side,
            TermSize::new(80, 24),
            buffered,
            PtySource::Raw,
        )
        .unwrap();

        let mut got = String::new();
        for _ in 0..200 {
            {
                let t = term.term.lock();
                let grid = t.grid();
                got.clear();
                for col in 0..5usize {
                    got.push(
                        grid[alacritty_terminal::index::Line(0)]
                            [alacritty_terminal::index::Column(col)]
                        .c,
                    );
                }
            }
            if got == "hello" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            got, "hello",
            "the pre-read replay must still reach the grid"
        );
    }

    #[test]
    fn reader_feeds_local_grid() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();

        let size = TermSize::new(80, 24);
        let term = RemoteTerminal::from_stream(client_side, size).unwrap();

        DaemonMsg::Output(b"hello".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        DaemonMsg::Cwd(PathBuf::from("/tmp/work"))
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        let mut got = String::new();
        for _ in 0..200 {
            {
                let t = term.term.lock();
                let grid = t.grid();
                got.clear();
                for col in 0..5usize {
                    let cell = &grid[alacritty_terminal::index::Line(0)]
                        [alacritty_terminal::index::Column(col)];
                    got.push(cell.c);
                }
            }
            if got == "hello" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(got, "hello", "reader thread should have fed the grid");

        let mut cwd = None;
        for _ in 0..200 {
            cwd = term.foreground_cwd();
            if cwd.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(cwd, Some(PathBuf::from("/tmp/work")));

        drop(daemon_side);
        for _ in 0..200 {
            if term.exited_flag.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(term.exited_flag.load(Ordering::SeqCst));
    }

    #[test]
    fn cursor_style_sequence_overrides_and_resets_to_user_default() {
        use alacritty_terminal::vte::ansi::CursorShape;

        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        let mut user_config = anterm_ui_kit::config::Config::default();
        user_config.cursor_style = ConfigCursorStyle::Underline;
        term.apply_user_config(&user_config);

        let mut shape = term.term.lock().cursor_style().shape;
        assert_eq!(shape, CursorShape::Underline);

        DaemonMsg::Output(b"\x1b[6 q".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        for _ in 0..200 {
            shape = term.term.lock().cursor_style().shape;
            if shape == CursorShape::Beam {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(shape, CursorShape::Beam);

        DaemonMsg::Output(b"\x1b[0 q".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        for _ in 0..200 {
            shape = term.term.lock().cursor_style().shape;
            if shape == CursorShape::Underline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(shape, CursorShape::Underline);
    }

    /// Feeds `output` to a pane configured with `configured`, one daemon frame
    /// per chunk, and reports the cursor shape once all of it has been parsed.
    fn cursor_shape_after(
        configured: ConfigCursorStyle,
        output: &[&[u8]],
    ) -> alacritty_terminal::vte::ansi::CursorShape {
        use alacritty_terminal::index::{Column, Line};

        // The pane reads config.json when it is built; keep that off the
        // user's real one.
        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        let mut user_config = anterm_ui_kit::config::Config::default();
        user_config.cursor_style = configured;
        term.apply_user_config(&user_config);

        for chunk in output {
            DaemonMsg::Output(chunk.to_vec())
                .encode(&mut daemon_side)
                .unwrap();
        }
        // A sentinel painted after everything else: once it is on the grid,
        // every chunk before it has been parsed, so the shape read below is
        // the final one and not a transient match.
        DaemonMsg::Output(b"\x1b[24;1H#".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        for _ in 0..400 {
            if term.term.lock().grid()[Line(23)][Column(0)].c == '#' {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let term = term.term.lock();
        assert_eq!(
            term.grid()[Line(23)][Column(0)].c,
            '#',
            "output never parsed"
        );
        term.cursor_style().shape
    }

    /// #837: both spellings of the DECSCUSR reset land on `cursor_style`.
    #[test]
    fn cursor_style_reset_returns_to_a_configured_bar_or_underline() {
        use alacritty_terminal::vte::ansi::CursorShape;

        for reset in [&b"\x1b[0 q"[..], &b"\x1b[ q"[..]] {
            for (configured, want) in [
                (ConfigCursorStyle::Bar, CursorShape::Beam),
                (ConfigCursorStyle::Underline, CursorShape::Underline),
            ] {
                assert_eq!(
                    cursor_shape_after(configured, &[b"\x1b[6 q", b"\x1b[2 q", reset]),
                    want,
                    "{configured:?} after {reset:?}"
                );
            }
        }
    }

    /// #837: what nvim actually sends on exit under `xterm-256color` is `Se`,
    /// `\e[2 q` — a literal steady block. The command's `D` hands the prompt
    /// its configured cursor back, whether the block arrives in the same
    /// frame as the marks or in one of its own.
    #[test]
    fn a_block_an_exiting_command_left_is_undone_at_its_finish_mark() {
        use alacritty_terminal::vte::ansi::CursorShape;

        for (configured, want) in [
            (ConfigCursorStyle::Bar, CursorShape::Beam),
            (ConfigCursorStyle::Underline, CursorShape::Underline),
        ] {
            assert_eq!(
                cursor_shape_after(
                    configured,
                    &[b"\x1b]133;C;nvim x\x07\x1b[2 q\x1b]133;D;0\x07$ "],
                ),
                want,
                "{configured:?}, one frame"
            );
            assert_eq!(
                cursor_shape_after(
                    configured,
                    &[
                        b"\x1b]133;C;nvim x\x07",
                        b"\x1b[6 q",
                        b"\x1b[2 q\x1b[?1049l",
                        b"\x1b]133;D;0\x07\x1b]133;A\x07$ ",
                    ],
                ),
                want,
                "{configured:?}, split frames"
            );
        }
    }

    #[test]
    fn reader_surfaces_auth_prompt_and_status() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        DaemonMsg::SshStatus {
            phase: SshPhase::Authenticating,
        }
        .encode(&mut daemon_side)
        .unwrap();
        DaemonMsg::AuthPrompt {
            request_id: 7,
            prompt: AuthPromptKind::Password {
                user: "deploy".into(),
                host: "10.0.0.5".into(),
            },
        }
        .encode(&mut daemon_side)
        .unwrap();
        daemon_side.flush().unwrap();

        let mut prompt = None;
        for _ in 0..200 {
            if let Some(p) = term.take_auth_prompt() {
                prompt = Some(p);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let (id, kind) = prompt.expect("auth prompt should have surfaced");
        assert_eq!(id, 7);
        assert!(matches!(kind, AuthPromptKind::Password { .. }));
        assert_eq!(term.ssh_phase(), Some(SshPhase::Authenticating));
        assert!(!term.has_pending_auth());
    }

    #[test]
    fn child_exit_is_distinguished_from_daemon_disconnect() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        DaemonMsg::Exited { code: Some(0) }
            .encode(&mut daemon_side)
            .unwrap();
        for _ in 0..200 {
            if term.exited_flag.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(term.exited_flag.load(Ordering::SeqCst));
        assert!(
            term.child_exited(),
            "an Exited frame is a genuine child exit"
        );

        let (client_side, daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        drop(daemon_side);
        for _ in 0..200 {
            if term.exited_flag.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(term.exited_flag.load(Ordering::SeqCst));
        assert!(
            !term.child_exited(),
            "a disconnect is not a child exit — auto-close must not fire"
        );
    }

    #[test]
    fn stale_mode_resets_target_only_the_dirty_bits() {
        let clean = TermMode::SHOW_CURSOR | TermMode::LINE_WRAP | TermMode::BRACKETED_PASTE;
        assert!(stale_mode_resets(clean).is_empty());

        let hidden = TermMode::LINE_WRAP;
        assert_eq!(stale_mode_resets(hidden), b"\x1b[?25h");

        let residue = TermMode::ALT_SCREEN | TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let seq = stale_mode_resets(residue);
        let text = String::from_utf8_lossy(&seq).into_owned();
        assert!(text.starts_with("\x1b[?1049l"));
        assert!(text.contains("\x1b[?25h"));
        assert!(text.contains("\x1b[?1002l"));
        assert!(text.contains("\x1b[?1006l"));
        assert!(
            !text.contains("u"),
            "kitty flags are the caller's: {text:?}"
        );

        let kitty = TermMode::SHOW_CURSOR | TermMode::DISAMBIGUATE_ESC_CODES;
        assert!(stale_mode_resets(kitty).is_empty());
    }

    #[test]
    fn prompt_report_scrubs_stale_tui_modes() {
        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        DaemonMsg::Output(b"\x1b[?1049h\x1b[?25l\x1b[?1002h\x1b[?1006h".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        DaemonMsg::Prompt {
            active: true,
            at_prompt: true,
            last_exit: Some(255),
        }
        .encode(&mut daemon_side)
        .unwrap();
        daemon_side.flush().unwrap();

        let mut mode = TermMode::NONE;
        for _ in 0..200 {
            mode = *term.term.lock().mode();
            let scrubbed = !mode.contains(TermMode::ALT_SCREEN)
                && mode.contains(TermMode::SHOW_CURSOR)
                && !mode.intersects(TermMode::MOUSE_MODE)
                && !mode.contains(TermMode::SGR_MOUSE);
            if scrubbed && term.at_prompt() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            !mode.contains(TermMode::ALT_SCREEN),
            "the prompt report must pull the grid off the stranded alt screen"
        );
        assert!(
            mode.contains(TermMode::SHOW_CURSOR),
            "the prompt report must re-show the DECTCEM-hidden cursor"
        );
        assert!(
            !mode.intersects(TermMode::MOUSE_MODE) && !mode.contains(TermMode::SGR_MOUSE),
            "the prompt report must disable stale mouse reporting"
        );
    }

    #[test]
    fn snapshot_replay_suppresses_query_replies_and_side_effects() {
        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        // `CSI 14t` is the one query alacritty hands out as its own event
        // (the pixel size lives with the view) rather than as a `PtyWrite`;
        // answered from a replay, its `CSI 4;h;w t` lands on whatever reads
        // the pane now — a shell prompt, once the TUI that asked has quit.
        DaemonMsg::Snapshot(
            b"\x1b[6n\x1b]11;?\x07\x1b[14t\x1b[16t\x1b[18t\x1b]52;c;aGk=\x07\x07replayed".to_vec(),
        )
        .encode(&mut daemon_side)
        .unwrap();
        daemon_side.flush().unwrap();

        let mut events = Vec::new();
        for _ in 0..200 {
            while let Ok(ev) = term.events.try_recv() {
                events.push(ev);
            }
            if events.iter().any(|e| matches!(e, AlacEvent::Wakeup)) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            events.iter().any(|e| matches!(e, AlacEvent::Wakeup)),
            "the replay's Wakeup should still arrive"
        );
        assert!(
            !events.iter().any(|e| matches!(
                e,
                AlacEvent::PtyWrite(_)
                    | AlacEvent::ColorRequest(..)
                    | AlacEvent::TextAreaSizeRequest(_)
                    | AlacEvent::ClipboardStore(..)
                    | AlacEvent::ClipboardLoad(..)
                    | AlacEvent::Bell
            )),
            "replayed history must not re-answer queries or replay side effects"
        );

        DaemonMsg::Output(b"\x1b[14t".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        let mut got_size_request = false;
        for _ in 0..200 {
            while let Ok(ev) = term.events.try_recv() {
                if matches!(ev, AlacEvent::TextAreaSizeRequest(_)) {
                    got_size_request = true;
                }
            }
            if got_size_request {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            got_size_request,
            "a live pixel-size query must still be answered"
        );

        DaemonMsg::Output(b"\x1b[6n".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        let mut got_reply = false;
        for _ in 0..200 {
            while let Ok(ev) = term.events.try_recv() {
                if matches!(ev, AlacEvent::PtyWrite(_)) {
                    got_reply = true;
                }
            }
            if got_reply {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(got_reply, "live queries must still be answered");
    }

    #[test]
    fn decrqm_probe_reports_sync_update_supported() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        DaemonMsg::Output(b"\x1b[?2026$p".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        let mut reply = None;
        for _ in 0..200 {
            while let Ok(ev) = term.events.try_recv() {
                if let AlacEvent::PtyWrite(text) = ev {
                    reply = Some(text);
                }
            }
            if reply.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            reply.as_deref(),
            Some("\x1b[?2026;2$y"),
            "DECRQM ?2026 must be answered as supported (2 = reset)"
        );
    }

    #[test]
    fn win_size_carries_grid_and_cell_dims() {
        let ws = win_size(TermSize::new(80, 24), 8, 17);
        assert_eq!(ws.cols, 80);
        assert_eq!(ws.rows, 24);
        assert_eq!(ws.cell_w, 8);
        assert_eq!(ws.cell_h, 17);
    }

    #[test]
    fn write_sends_input_frames() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        term.write(Vec::<u8>::new());
        term.write(b"echo hi\r".to_vec());

        match ClientMsg::read(&mut daemon_side).unwrap() {
            ClientMsg::Input(bytes) => assert_eq!(bytes, b"echo hi\r"),
            other => panic!("expected Input, got {other:?}"),
        }
    }

    /// A peer that holds the link open and stops reading is what the router
    /// looks like from here when the far end is congested: `copy_bidirectional`
    /// stops draining our half and the send buffer fills. The socket is
    /// blocking and has no write timeout, so `write(2)` parks — and it used to
    /// park on the UI thread, which draws every window. macOS gives a unix
    /// stream 8K, so it took about 1400 keystrokes, or one paste.
    ///
    /// Typing into such a pane must now cost the window nothing at all.
    #[test]
    fn a_link_that_stopped_reading_does_not_park_the_writing_thread() {
        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        // Far past the 8K that used to be fatal, one keystroke at a time, on
        // this very thread: a park anywhere in here is a frozen window.
        let started = std::time::Instant::now();
        for _ in 0..64 * 1024 {
            term.write(vec![b'x']);
        }
        let spent = started.elapsed();

        drop(daemon_side);
        assert!(
            spent < std::time::Duration::from_secs(1),
            "64K keystrokes into a link nobody is draining took {spent:?} — \
             the caller is a gpui event handler, so this is the UI thread"
        );
    }

    /// The backlog is bounded, and reaching the bound is not a slow link but a
    /// dead one: nothing has taken a byte for four megabytes of typing. Saying
    /// so is what stops the pane quietly swallowing input forever, and it is
    /// said in the same words — and once — as an outright refused write.
    #[test]
    fn a_backlog_nothing_drains_is_reported_as_a_lost_link() {
        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        // 8K a shot rather than a byte, so this is a few hundred frames and not
        // a few million: the bound is on bytes queued, not on frames.
        //
        // Twice the bound rather than a frame or two past it. The sender does
        // get some of this onto the wire before it parks — as much as the send
        // buffer holds, which is 8K on macOS but a couple hundred K on Linux —
        // and that much is discounted from the backlog. A margin narrower than
        // the widest of those buffers is a test that passes on one platform and
        // not the other.
        let chunk = vec![b'x'; 8 << 10];
        for _ in 0..2 * MAX_BACKLOG / chunk.len() {
            term.write(chunk.clone());
        }

        assert!(
            term.exited_flag.load(Ordering::SeqCst),
            "a link that has taken nothing for {MAX_BACKLOG} bytes is gone, and the pane must say so"
        );
        let mut exits = 0;
        while let Ok(ev) = term.events.try_recv() {
            exits += usize::from(matches!(ev, AlacEvent::Exit));
        }
        assert_eq!(exits, 1, "said once, not once per keystroke");
        drop(daemon_side);
    }

    /// One frame can be larger than the whole backlog bound: a paste is
    /// whatever the clipboard holds. That is not a link nothing is draining,
    /// and a pane must not die of being pasted into — nor may the keystrokes
    /// that follow read as a backlog merely because the paste is still going.
    #[test]
    fn a_paste_larger_than_the_backlog_bound_is_not_a_dead_link() {
        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        // A peer that keeps reading: the link is healthy, just carrying a lot.
        let draining = std::thread::spawn(move || {
            let _ = std::io::copy(&mut daemon_side, &mut std::io::sink());
        });

        term.write(vec![b'x'; MAX_BACKLOG + (1 << 20)]);
        for _ in 0..1024 {
            term.write(vec![b'y']);
        }

        assert!(
            !term.exited_flag.load(Ordering::SeqCst),
            "a five megabyte paste is a paste, not a link that has stopped taking input"
        );
        drop(term);
        draining.join().unwrap();
    }

    /// Input the link refuses used to vanish: `write` threw the error away, so a
    /// pane whose daemon had stopped reading kept taking keystrokes into
    /// nothing. The refusal now marks the pane exited by the reader's own signal
    /// — only our sending half is shut here, so the reader is still parked on
    /// an open receiving half and the writing side is the one that finds out.
    /// (Shutting the peer's receiving half instead is not portable: Linux
    /// answers the next write with EPIPE, macOS buffers it.)
    ///
    /// The refusal is met on the sender thread now, so the pane learns of it a
    /// moment after the keystroke rather than during it. That is the trade the
    /// queue buys: the window never waits on the socket to find out.
    #[test]
    fn a_write_the_link_refuses_marks_the_pane_gone_once() {
        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, _daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        term.link
            .closer
            .shutdown(std::net::Shutdown::Write)
            .unwrap();

        term.write(b"echo hi\r".to_vec());
        let noticed = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !term.exited_flag.load(Ordering::SeqCst) && std::time::Instant::now() < noticed {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            term.exited_flag.load(Ordering::SeqCst),
            "a refused Input is the link gone, and the pane has to say so"
        );
        let mut exits = 0;
        while let Ok(ev) = term.events.try_recv() {
            exits += usize::from(matches!(ev, AlacEvent::Exit));
        }
        assert_eq!(
            exits, 1,
            "reported through the same event the reader raises on EOF"
        );

        term.write(b"echo again\r".to_vec());
        while let Ok(ev) = term.events.try_recv() {
            exits += usize::from(matches!(ev, AlacEvent::Exit));
        }
        assert_eq!(exits, 1, "said once, not once per keystroke");
    }

    /// A link retired for a relink takes no more input — `stop_reader` closes
    /// the queue and shuts the socket down — and that must not read as the pane
    /// dying under the swap, for the same reason the retired reader exits
    /// silently. The frame is now turned away at the queue rather than by the
    /// socket, but the pane has to stay alive either way.
    #[test]
    fn a_write_on_a_retired_link_does_not_mark_the_pane_gone() {
        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, _daemon_side) = UnixStream::pair().unwrap();
        let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        term.stop_reader();

        term.write(b"echo hi\r".to_vec());
        assert!(
            !term.exited_flag.load(Ordering::SeqCst),
            "the pane is being relinked or released, not dying"
        );
    }

    #[test]
    fn attach_replay_runs_at_the_daemon_reported_size() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        DaemonMsg::Size(WinSize {
            cols: 120,
            rows: 30,
            cell_w: 8,
            cell_h: 17,
        })
        .encode(&mut daemon_side)
        .unwrap();
        DaemonMsg::Snapshot(vec![b'x'; 100])
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        let (mut tail, mut wrapped) = (' ', ' ');
        for _ in 0..200 {
            {
                use alacritty_terminal::grid::Dimensions as _;
                let t = term.term.lock();
                let grid = t.grid();
                if grid.columns() >= 120 {
                    tail = grid[alacritty_terminal::index::Line(0)]
                        [alacritty_terminal::index::Column(99)]
                    .c;
                    wrapped = grid[alacritty_terminal::index::Line(1)]
                        [alacritty_terminal::index::Column(0)]
                    .c;
                }
            }
            if tail == 'x' {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(tail, 'x', "replay should run at the recorded 120-col width");
        assert_eq!(
            wrapped, ' ',
            "a 100-char line must not wrap on a 120-col grid"
        );
    }

    #[test]
    fn first_resize_always_syncs_then_dedups() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        term.resize(TermSize::new(80, 24), 8, 17);
        match ClientMsg::read(&mut daemon_side).unwrap() {
            ClientMsg::Resize(ws) => assert_eq!((ws.cols, ws.rows), (80, 24)),
            other => panic!("expected the first Resize to be sent, got {other:?}"),
        }

        term.resize(TermSize::new(80, 24), 8, 17);
        term.write(b"marker".to_vec());
        match ClientMsg::read(&mut daemon_side).unwrap() {
            ClientMsg::Input(bytes) => assert_eq!(bytes, b"marker"),
            other => panic!("expected Input (dup resize sends nothing), got {other:?}"),
        }
    }

    #[test]
    fn freshly_spawned_terminal_skips_redundant_first_resize_but_sends_real_changes() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let initial = TermSize::new(148, 43);
        let mut term = RemoteTerminal::from_stream(client_side, initial).unwrap();
        // Mirrors `spawn_once` / `spawn_native_ssh_once`, which already sent
        // the exact initial `WinSize` in `ClientMsg::Spawn` / `SpawnNativeSsh`.
        term.synced_size = true;
        term.synced_cell = (16, 34);

        // Matching Frame 1 layout must NOT emit a redundant Resize / SIGWINCH.
        term.resize(initial, 16, 34);
        term.write(b"ok".to_vec());
        match ClientMsg::read(&mut daemon_side).unwrap() {
            ClientMsg::Input(bytes) => assert_eq!(bytes, b"ok"),
            other => panic!("expected Input without redundant Resize, got {other:?}"),
        }

        // Device-pixel cell change (e.g. display scale change) still sends Resize.
        term.resize(initial, 8, 17);
        match ClientMsg::read(&mut daemon_side).unwrap() {
            ClientMsg::Resize(ws) => {
                assert_eq!((ws.cols, ws.rows, ws.cell_w, ws.cell_h), (148, 43, 8, 17));
            }
            other => panic!("expected Resize on cell-metric change, got {other:?}"),
        }

        // Real grid dimension change still sends Resize.
        term.resize(TermSize::new(120, 40), 8, 17);
        match ClientMsg::read(&mut daemon_side).unwrap() {
            ClientMsg::Resize(ws) => {
                assert_eq!((ws.cols, ws.rows, ws.cell_w, ws.cell_h), (120, 40, 8, 17));
            }
            other => panic!("expected Resize on size change, got {other:?}"),
        }
    }

    #[test]
    fn sync_update_without_esu_flushes_after_the_deadline() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        DaemonMsg::Output(b"\x1b[?2026habc".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        let mut got = String::new();
        for _ in 0..600 {
            {
                let t = term.term.lock();
                let grid = t.grid();
                got.clear();
                for col in 0..3usize {
                    got.push(
                        grid[alacritty_terminal::index::Line(0)]
                            [alacritty_terminal::index::Column(col)]
                        .c,
                    );
                }
            }
            if got == "abc" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(got, "abc", "dangling BSU must flush on the sync deadline");
    }

    #[test]
    fn sync_update_suppresses_intermediate_wakeups() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        // Send an opening DEC ?2026h chunk and a second buffered chunk without ESU.
        DaemonMsg::Output(b"\x1b[?2026hhello ".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(25));
        DaemonMsg::Output(b"world".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(25));

        let mut early_wakeups = 0usize;
        while let Ok(ev) = term.events.try_recv() {
            if matches!(ev, AlacEvent::Wakeup) {
                early_wakeups += 1;
            }
        }
        assert_eq!(
            early_wakeups, 0,
            "buffered chunks inside an open ?2026h must not emit Wakeup"
        );

        // Closing ESU flushes the frame and emits a single Wakeup.
        DaemonMsg::Output(b"!\x1b[?2026l".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        let mut wakeups = 0usize;
        for _ in 0..200 {
            while let Ok(ev) = term.events.try_recv() {
                if matches!(ev, AlacEvent::Wakeup) {
                    wakeups += 1;
                }
            }
            if wakeups > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(wakeups, 1, "closing ?2026l must emit Wakeup");
        let row0: String = {
            let t = term.term.lock();
            (0..12)
                .map(|c| {
                    t.grid()[alacritty_terminal::index::Line(0)]
                        [alacritty_terminal::index::Column(c)]
                    .c
                })
                .collect()
        };
        assert_eq!(row0, "hello world!");
    }

    #[test]
    fn wakeup_events_coalesce_to_one_in_flight() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        let start_rev = term.grid_revision();
        for i in 0..25 {
            DaemonMsg::Output(format!("line {i}\r\n").into_bytes())
                .encode(&mut daemon_side)
                .unwrap();
            daemon_side.flush().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // Wait until the reader has processed the final line.
        for _ in 0..200 {
            if term.grid_revision() > start_rev && term.history_size_nonblocking() >= 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let mut wakeups = 0usize;
        while let Ok(ev) = term.events.try_recv() {
            if matches!(ev, AlacEvent::Wakeup) {
                wakeups += 1;
            }
        }
        assert_eq!(
            wakeups, 1,
            "multiple undrained output batches must coalesce to one queued Wakeup"
        );

        // Once drained, the next output re-arms a fresh Wakeup.
        DaemonMsg::Output(b"after".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        let mut after_wakeups = 0usize;
        for _ in 0..200 {
            while let Ok(ev) = term.events.try_recv() {
                if matches!(ev, AlacEvent::Wakeup) {
                    after_wakeups += 1;
                }
            }
            if after_wakeups > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(after_wakeups, 1, "draining Wakeup must re-arm future Wakeups");
    }

    #[test]
    fn snapshot_replay_flushes_a_dangling_sync_frame_suppressed() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        DaemonMsg::Snapshot(b"\x1b[?2026h\x1b[6nhi".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        let mut got = String::new();
        for _ in 0..200 {
            {
                let t = term.term.lock();
                let grid = t.grid();
                got.clear();
                for col in 0..2usize {
                    got.push(
                        grid[alacritty_terminal::index::Line(0)]
                            [alacritty_terminal::index::Column(col)]
                        .c,
                    );
                }
            }
            if got == "hi" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            got, "hi",
            "the trapped replay tail must flush with the snapshot"
        );

        let mut events = Vec::new();
        while let Ok(ev) = term.events.try_recv() {
            events.push(ev);
        }
        assert!(
            !events.iter().any(|e| matches!(e, AlacEvent::PtyWrite(_))),
            "a query inside the replayed sync tail must stay suppressed"
        );
    }

    #[test]
    fn layout_resize_reasserts_geometry_after_a_late_size_frame() {
        use alacritty_terminal::grid::Dimensions as _;
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        term.resize(TermSize::new(100, 40), 8, 17);
        assert!(matches!(
            ClientMsg::read(&mut daemon_side).unwrap(),
            ClientMsg::Resize(_)
        ));

        DaemonMsg::Size(WinSize {
            cols: 120,
            rows: 30,
            cell_w: 8,
            cell_h: 17,
        })
        .encode(&mut daemon_side)
        .unwrap();
        DaemonMsg::Snapshot(b"old screen".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        for _ in 0..200 {
            if term.term.lock().columns() == 120 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(term.term.lock().columns(), 120, "replay geometry applied");

        term.resize(TermSize::new(100, 40), 8, 17);
        assert_eq!(term.term.lock().columns(), 100);
        assert_eq!(term.term.lock().screen_lines(), 40);
        assert!(matches!(
            ClientMsg::read(&mut daemon_side).unwrap(),
            ClientMsg::Resize(ws) if ws.cols == 100 && ws.rows == 40
        ));
    }

    #[test]
    fn resize_updates_size_and_notifies_daemon() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        term.resize(TermSize::new(100, 40), 9, 18);
        assert_eq!(term.size(), TermSize::new(100, 40));
        match ClientMsg::read(&mut daemon_side).unwrap() {
            ClientMsg::Resize(ws) => {
                assert_eq!((ws.cols, ws.rows, ws.cell_w, ws.cell_h), (100, 40, 9, 18));
            }
            other => panic!("expected Resize, got {other:?}"),
        }
    }

    #[test]
    fn echoed_resize_defers_the_grid_reflow_to_the_daemons_size_frame() {
        use alacritty_terminal::grid::Dimensions as _;
        use alacritty_terminal::index::{Column, Line};

        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        // A remote route whose host advertised the echo. The local daemon's
        // answer rides the same gate through `local_daemon_supports`, so this
        // covers the deferral for both.
        term.route = PaneRoute::Remote {
            header: Box::new(anterm_core::daemon::router::RouteHeader::wsl("Ubuntu-22.04")),
            resize_echo: true,
            size_lease: false,
            clear_scrollback: false,
        };

        term.resize(TermSize::new(120, 30), 8, 17);
        assert!(matches!(
            ClientMsg::read(&mut daemon_side).unwrap(),
            ClientMsg::Resize(ws) if ws.cols == 120 && ws.rows == 30
        ));
        assert_eq!(
            term.term.lock().columns(),
            80,
            "the grid keeps the old geometry until the daemon's echo arrives"
        );

        // Old-width bytes still in flight ahead of the echo: a CUP to column
        // 100 must clamp on the 80-col grid. The same addressing after the
        // echo reaches the real column on the 120-col grid.
        DaemonMsg::Output(b"\x1b[1;100HA".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        DaemonMsg::Size(WinSize {
            cols: 120,
            rows: 30,
            cell_w: 8,
            cell_h: 17,
        })
        .encode(&mut daemon_side)
        .unwrap();
        DaemonMsg::Output(b"\x1b[2;100HB".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        for _ in 0..400 {
            {
                let t = term.term.lock();
                if t.columns() == 120 && t.grid()[Line(1)][Column(99)].c == 'B' {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let t = term.term.lock();
        let grid = t.grid();
        assert_eq!(
            grid[Line(0)][Column(79)].c,
            'A',
            "bytes queued before the echo parse at the old width"
        );
        assert_eq!(
            grid[Line(1)][Column(99)].c,
            'B',
            "bytes after the echo parse at the new width"
        );
    }

    /// A route whose daemon promised to echo every resize, over a socket pair
    /// the test plays the daemon on. The switch tests build the same route
    /// against a real pane.
    fn echoing_pair() -> (RemoteTerminal, UnixStream) {
        let (client_side, daemon_side) = UnixStream::pair().unwrap();
        let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        term.route = PaneRoute::Remote {
            header: Box::new(anterm_core::daemon::router::RouteHeader::wsl("Ubuntu-22.04")),
            resize_echo: true,
            size_lease: false,
            clear_scrollback: false,
        };
        daemon_side
            .set_read_timeout(Some(std::time::Duration::from_millis(150)))
            .unwrap();
        (term, daemon_side)
    }

    /// The geometry of the next frame the pane sent, if it sent one at all.
    fn next_resize(daemon_side: &mut UnixStream) -> Option<(u16, u16)> {
        match ClientMsg::read(daemon_side) {
            Ok(ClientMsg::Resize(ws)) => Some((ws.cols, ws.rows)),
            Ok(other) => panic!("expected a Resize, got {other:?}"),
            Err(_) => None,
        }
    }

    /// Makes the last resize old enough that an echo for it is no longer
    /// expected, without the test sitting out the grace for real.
    fn outlive_the_echo_grace(term: &mut RemoteTerminal) {
        term.resize_sent_at = std::time::Instant::now()
            .checked_sub(RESIZE_ECHO_GRACE + std::time::Duration::from_millis(1));
    }

    fn wait_for_columns(term: &RemoteTerminal, cols: usize) {
        use alacritty_terminal::grid::Dimensions as _;
        for _ in 0..400 {
            if term.term.lock().columns() == cols {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the grid never reached {cols} columns");
    }

    /// #893: a pane came back from sleep or a workspace switch painting a grid
    /// shorter than its bounds, and stayed that way until a divider drag. On an
    /// echoing route the grid only moves when the daemon's `Size` comes back,
    /// and the early-out trusted the request: once asked for, the same size
    /// was never sent again, so a request or echo that went missing stranded
    /// the grid — and the child — at the old geometry for good.
    #[test]
    fn an_echoed_resize_whose_echo_never_came_is_asked_for_again() {
        use alacritty_terminal::grid::Dimensions as _;

        let (mut term, mut daemon_side) = echoing_pair();
        term.resize(TermSize::new(120, 30), 8, 17);
        assert_eq!(next_resize(&mut daemon_side), Some((120, 30)));

        // Every frame asks again. While the echo can still be on its way that
        // is not a reason to resend.
        term.resize(TermSize::new(120, 30), 8, 17);
        assert_eq!(
            next_resize(&mut daemon_side),
            None,
            "an echo still in flight must not be chased with duplicates"
        );

        // The echo never comes. Past the grace the grid is still 80x24, and
        // the frame that notices must ask for the size again.
        outlive_the_echo_grace(&mut term);
        term.resize(TermSize::new(120, 30), 8, 17);
        assert_eq!(
            next_resize(&mut daemon_side),
            Some((120, 30)),
            "a resize whose echo never arrived was never retried, so the grid \
             stays at the old size until the layout asks for a different one"
        );
        assert_eq!(
            term.term.lock().columns(),
            80,
            "the retry still waits for the echo rather than reflowing early"
        );

        // Once the echo lands the size is settled, however old the request.
        DaemonMsg::Size(WinSize {
            cols: 120,
            rows: 30,
            cell_w: 8,
            cell_h: 17,
        })
        .encode(&mut daemon_side)
        .unwrap();
        daemon_side.flush().unwrap();
        wait_for_columns(&term, 120);
        assert_eq!(term.term.lock().screen_lines(), 30);
        outlive_the_echo_grace(&mut term);
        term.resize(TermSize::new(120, 30), 8, 17);
        assert_eq!(
            next_resize(&mut daemon_side),
            None,
            "a grid that reached the requested size must not be resized again"
        );
    }

    /// The other way the grid and the request can part on an echoing route: a
    /// `Size` frame that lands after the echo moves the grid off the size the
    /// layout asked for. The non-echoing path already re-asserts there
    /// (`layout_resize_reasserts_geometry_after_a_late_size_frame`); the
    /// echoing one skipped it, since the request itself had not changed.
    #[test]
    fn an_echoed_resize_reasserts_geometry_after_a_late_size_frame() {
        let (mut term, mut daemon_side) = echoing_pair();
        term.resize(TermSize::new(120, 30), 8, 17);
        assert_eq!(next_resize(&mut daemon_side), Some((120, 30)));
        for (cols, rows) in [(120, 30), (100, 12)] {
            DaemonMsg::Size(WinSize {
                cols,
                rows,
                cell_w: 8,
                cell_h: 17,
            })
            .encode(&mut daemon_side)
            .unwrap();
        }
        daemon_side.flush().unwrap();
        wait_for_columns(&term, 100);

        outlive_the_echo_grace(&mut term);
        term.resize(TermSize::new(120, 30), 8, 17);
        assert_eq!(
            next_resize(&mut daemon_side),
            Some((120, 30)),
            "a grid knocked off the requested size was left there"
        );
    }

    /// A relink re-sends the geometry the pane has now — including a resize
    /// made after the old link had already died, which reached no daemon.
    #[test]
    fn a_relink_delivers_the_resize_made_while_the_link_was_down() {
        anterm_ui_kit::config::pin_test_config_dir();
        let (old_client, mut old_daemon) = UnixStream::pair().unwrap();
        let mut term = RemoteTerminal::from_stream(old_client, TermSize::new(80, 24)).unwrap();
        term.resize(TermSize::new(100, 40), 8, 17);
        assert!(matches!(
            ClientMsg::read(&mut old_daemon).unwrap(),
            ClientMsg::Resize(ws) if ws.cols == 100 && ws.rows == 40
        ));

        // The link drops; the layout keeps moving.
        drop(old_daemon);
        term.resize(TermSize::new(120, 30), 8, 17);

        let (new_client, mut new_daemon) = UnixStream::pair().unwrap();
        new_daemon
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        // What `relink_plan` hands the relink: the size the view has now.
        let size = term.size();
        term.adopt_relink(new_client, Vec::new(), &PaneRoute::Local, size, 8, 17)
            .unwrap();
        assert!(
            matches!(
                ClientMsg::read(&mut new_daemon).unwrap(),
                ClientMsg::Resize(ws) if ws.cols == 120 && ws.rows == 30
            ),
            "the relinked daemon must be told the size the pane has now"
        );
    }
    #[test]
    fn a_remote_route_without_the_advertised_echo_reflows_at_request_time() {
        use alacritty_terminal::grid::Dimensions as _;

        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        // What `for_workspace` builds when the host's hello named no echo —
        // an older server, or a link that was down when the route was made.
        term.route = PaneRoute::Remote {
            header: Box::new(anterm_core::daemon::router::RouteHeader::wsl("Ubuntu-22.04")),
            resize_echo: false,
            size_lease: false,
            clear_scrollback: false,
        };

        term.resize(TermSize::new(120, 30), 8, 17);
        assert_eq!(
            term.term.lock().columns(),
            120,
            "with no promised echo the grid must reflow at request time — \
             deferring would leave it at the old width forever"
        );
        assert!(matches!(
            ClientMsg::read(&mut daemon_side).unwrap(),
            ClientMsg::Resize(ws) if ws.cols == 120 && ws.rows == 30
        ));

        // The other route with no daemon to promise anything.
        term.route = PaneRoute::Unroutable("no ssh details".into());
        assert!(!term.resize_echoed(), "no route, no echo to wait for");
    }

    #[test]
    fn clear_scrollback_only_sends_when_daemon_advertises_support() {
        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        daemon_side
            .set_read_timeout(Some(std::time::Duration::from_millis(80)))
            .unwrap();
        let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        term.route = PaneRoute::Remote {
            header: Box::new(anterm_core::daemon::router::RouteHeader::wsl("Ubuntu-22.04")),
            resize_echo: false,
            size_lease: false,
            clear_scrollback: false,
        };
        term.clear_scrollback();
        assert!(
            ClientMsg::read(&mut daemon_side).is_err(),
            "older remote daemon must not receive ClearScrollback"
        );

        term.route = PaneRoute::Remote {
            header: Box::new(anterm_core::daemon::router::RouteHeader::wsl("Ubuntu-22.04")),
            resize_echo: false,
            size_lease: false,
            clear_scrollback: true,
        };
        term.clear_scrollback();
        assert!(matches!(
            ClientMsg::read(&mut daemon_side).unwrap(),
            ClientMsg::ClearScrollback
        ));
    }

    #[test]
    fn at_prompt_stays_false_while_shell_integration_is_inactive() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        assert!(!term.shell_active(), "no report yet → integration inactive");

        DaemonMsg::Prompt {
            active: false,
            at_prompt: true,
            last_exit: None,
        }
        .encode(&mut daemon_side)
        .unwrap();
        DaemonMsg::Output(b"m".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        let mut synced = false;
        for _ in 0..200 {
            let c = term.term.lock().grid()[alacritty_terminal::index::Line(0)]
                [alacritty_terminal::index::Column(0)]
            .c;
            if c == 'm' {
                synced = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(synced, "reader should have applied both frames");
        assert!(!term.shell_active());
        assert!(!term.at_prompt(), "inactive shell must gate at_prompt off");
    }

    #[test]
    fn at_prompt_follows_daemon_prompt_reports() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        assert!(!term.at_prompt());

        DaemonMsg::Prompt {
            active: true,
            at_prompt: true,
            last_exit: Some(0),
        }
        .encode(&mut daemon_side)
        .unwrap();
        daemon_side.flush().unwrap();

        let mut at = false;
        for _ in 0..200 {
            if term.at_prompt() {
                at = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(at, "at_prompt should become true after the Prompt report");
    }

    /// #654, end to end on the reader: a shell at its prompt (the daemon's
    /// `Prompt` report) whose clock sits one column short of the edge, the
    /// pane narrowed by several columns at a time and widened back, each step
    /// followed by zsh's SIGWINCH redraw. Every live echo has to hand the
    /// prompt back before reflowing, or each narrowing strands the head of the
    /// old prompt and the widening joins them into a line of fragments.
    #[test]
    fn a_live_size_echo_at_a_prompt_leaves_one_prompt() {
        use alacritty_terminal::grid::Dimensions as _;
        use alacritty_terminal::index::{Column, Line};

        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(98, 12)).unwrap();
        // zsh 5.9's redraw for `PROMPT='[~] '` and an `RPROMPT` clock.
        let redraw = |cols: u16, clock: &str| {
            format!(
                "\r\r\x1b[0m\x1b[J[~] \x1b[K\x1b[{}C{clock}\x1b[{}D",
                cols - 13,
                cols - 5
            )
            .into_bytes()
        };
        DaemonMsg::Prompt {
            active: true,
            at_prompt: true,
            last_exit: Some(0),
        }
        .encode(&mut daemon_side)
        .unwrap();
        DaemonMsg::Output(redraw(98, "16:46:20"))
            .encode(&mut daemon_side)
            .unwrap();
        for cols in [92, 86, 92, 98] {
            DaemonMsg::Size(WinSize {
                cols,
                rows: 12,
                cell_w: 8,
                cell_h: 17,
            })
            .encode(&mut daemon_side)
            .unwrap();
            let clock = if cols == 98 { "16:46:33" } else { "16:46:2x" };
            DaemonMsg::Output(redraw(cols, clock))
                .encode(&mut daemon_side)
                .unwrap();
        }
        daemon_side.flush().unwrap();

        let text = || {
            let t = term.term.lock();
            let grid = t.grid();
            let top = -(grid.history_size() as i32);
            (top..grid.screen_lines() as i32)
                .map(|line| {
                    (0..grid.columns())
                        .map(|col| grid[Line(line)][Column(col)].c)
                        .collect::<String>()
                        .trim_end()
                        .to_string()
                })
                .filter(|row| !row.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        };
        for _ in 0..400 {
            if term.term.lock().columns() == 98 && text().contains("16:46:33") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let text = text();
        assert!(
            text.contains("16:46:33"),
            "the last redraw never landed:\n{text}"
        );
        assert_eq!(
            text.matches("[~]").count(),
            1,
            "a narrowing stranded part of an old prompt:\n{text}"
        );
    }

    #[test]
    fn foreground_agent_follows_daemon_agent_reports() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        assert_eq!(term.foreground_agent(), None, "none before any report");

        let poll = |want: Option<CLIAgent>| {
            for _ in 0..200 {
                if term.foreground_agent() == want {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            false
        };

        DaemonMsg::Agent(Some(CLIAgent::Claude))
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(poll(Some(CLIAgent::Claude)), "agent report should surface");

        DaemonMsg::Agent(None).encode(&mut daemon_side).unwrap();
        daemon_side.flush().unwrap();
        assert!(poll(None), "agent exit should clear it");
    }

    /// The daemon replays a reattached pane's stored agent status as an
    /// ordinary report. Nothing on the wire says so, so the link has to
    /// remember that the first report it hears is that replay — and that
    /// everything after it is live.
    #[test]
    fn a_reattached_link_marks_only_its_first_status_report_as_replayed() {
        use anterm_core::cli_agent::{AgentSessionState, AgentStatus};

        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term =
            RemoteTerminal::from_stream_reattached(client_side, TermSize::new(80, 24)).unwrap();
        assert_eq!(
            term.take_replayed_agent_status(),
            None,
            "nothing replayed until the frame actually arrives"
        );

        let report = |status, daemon: &mut UnixStream| {
            DaemonMsg::AgentStatus(Some(AgentSessionState {
                status,
                message: None,
                session_id: Some("sid-1".into()),
                launch_argv: None,
                rich: true,
                cwd: None,
                activity: 0,
                turns: 0,
                inferred: false,
                readout: Default::default(),
                since_ms: None,
            }))
            .encode(daemon)
            .unwrap();
            daemon.flush().unwrap();
        };
        let poll = |want: AgentStatus| {
            for _ in 0..200 {
                if term.agent_session().map(|s| s.status) == Some(want) {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            false
        };

        report(AgentStatus::Done, &mut daemon_side);
        assert!(poll(AgentStatus::Done), "the replayed status should land");
        assert_eq!(
            term.take_replayed_agent_status(),
            Some(Some(AgentStatus::Done)),
            "the first report on a reattached link is stored state"
        );
        assert_eq!(
            term.take_replayed_agent_status(),
            None,
            "only one taker gets it"
        );

        report(AgentStatus::Working, &mut daemon_side);
        assert!(poll(AgentStatus::Working), "the live status should land");
        assert_eq!(
            term.take_replayed_agent_status(),
            None,
            "everything after the replay is something the client watched happen"
        );
    }

    /// A pane with no agent session to replay sends no status frame at all, so
    /// the replay window has to close on its own — otherwise the first report
    /// from an agent launched *later* would be discounted as stored state.
    #[test]
    fn live_output_closes_the_replay_window() {
        use anterm_core::cli_agent::{AgentSessionState, AgentStatus};

        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term =
            RemoteTerminal::from_stream_reattached(client_side, TermSize::new(80, 24)).unwrap();

        DaemonMsg::Output(b"$ claude\r\n".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        DaemonMsg::AgentStatus(Some(AgentSessionState {
            status: AgentStatus::Done,
            message: None,
            session_id: Some("sid-1".into()),
            launch_argv: None,
            rich: true,
            cwd: None,
            activity: 0,
            turns: 0,
            inferred: false,
            readout: Default::default(),
            since_ms: None,
        }))
        .encode(&mut daemon_side)
        .unwrap();
        daemon_side.flush().unwrap();

        for _ in 0..200 {
            if term.agent_session().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            term.agent_session().map(|s| s.status),
            Some(AgentStatus::Done),
            "the status still lands"
        );
        assert_eq!(
            term.take_replayed_agent_status(),
            None,
            "a report that follows live output is live"
        );
    }

    #[test]
    fn agent_session_follows_daemon_status_reports() {
        use anterm_core::cli_agent::{AgentSessionState, AgentStatus};

        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        assert_eq!(term.agent_session(), None, "none before any report");

        let poll = |want: &dyn Fn(Option<AgentSessionState>) -> bool| {
            for _ in 0..200 {
                if want(term.agent_session()) {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            false
        };

        DaemonMsg::AgentStatus(Some(AgentSessionState {
            status: AgentStatus::Waiting,
            message: Some("Claude needs your permission".into()),
            session_id: Some("sid-1".into()),
            launch_argv: None,
            rich: true,
            cwd: None,
            activity: 0,
            turns: 0,
            inferred: false,
            readout: Default::default(),
            since_ms: None,
        }))
        .encode(&mut daemon_side)
        .unwrap();
        daemon_side.flush().unwrap();
        assert!(
            poll(&|s| s.is_some_and(|s| s.status == AgentStatus::Waiting
                && s.session_id.as_deref() == Some("sid-1")
                && s.rich)),
            "status report should surface with message + session id"
        );

        DaemonMsg::AgentStatus(None)
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(poll(&|s| s.is_none()), "a None report clears the session");
    }

    #[test]
    fn the_spawn_directory_answers_until_the_shell_reports_its_own() {
        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        let poll = |want: Option<&str>| {
            let want = want.map(PathBuf::from);
            for _ in 0..200 {
                if term.foreground_cwd() == want {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            false
        };

        assert_eq!(term.foreground_cwd(), None, "nothing known before the seed");
        term.seed_cwd(Some(PathBuf::from("/repo/anterm")));
        assert_eq!(
            term.foreground_cwd(),
            Some(PathBuf::from("/repo/anterm")),
            "the sidebar can group this pane without waiting for the shell"
        );

        DaemonMsg::Cwd(PathBuf::from("/repo/anterm/crates"))
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(
            poll(Some("/repo/anterm/crates")),
            "the shell's own report replaces the seed"
        );
    }

    #[test]
    fn a_shell_report_that_beat_the_seed_wins() {
        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        DaemonMsg::Cwd(PathBuf::from("/somewhere/else"))
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        for _ in 0..200 {
            if term.foreground_cwd().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        term.seed_cwd(Some(PathBuf::from("/repo/anterm")));
        assert_eq!(
            term.foreground_cwd(),
            Some(PathBuf::from("/somewhere/else")),
            "where the shell actually landed beats where we asked it to"
        );
    }

    #[test]
    fn zle_reading_follows_live_prompt_end_marks() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        let poll = |want: bool| {
            for _ in 0..200 {
                if term.zle_reading() == want {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            false
        };
        assert!(!term.zle_reading(), "conservative false before any mark");

        DaemonMsg::Snapshot(b"\x1b]133;B\x07".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        DaemonMsg::Output(b"\x1b]133;D;0\x07m".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        let mut synced = false;
        for _ in 0..200 {
            let c = term.term.lock().grid()[alacritty_terminal::index::Line(0)]
                [alacritty_terminal::index::Column(0)]
            .c;
            if c == 'm' {
                synced = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(synced, "reader should have applied both frames");
        assert!(
            !term.zle_reading(),
            "replayed B / live D must not arm the flag"
        );

        DaemonMsg::Output(b"\x1b]133;B\x07".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(poll(true), "live B should arm zle_reading");

        DaemonMsg::Output(b"\x1b]133;C\x07".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(poll(false), "C (command start) should disarm zle_reading");
    }

    #[test]
    fn the_running_command_is_the_line_the_shell_marked_and_ends_with_it() {
        anterm_ui_kit::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        let poll = |want: &str| {
            for _ in 0..200 {
                if term.running_command() == want {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            false
        };
        assert_eq!(term.running_command(), "", "nothing runs before a mark");

        // The escaping is the integration's; this side stores it verbatim and
        // the reader of the field undoes it.
        DaemonMsg::Output(b"\x1b]133;C;printf '100%25'\x07".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(
            poll("printf '100%25'"),
            "C should record the line it carried, got {:?}",
            term.running_command()
        );

        // Back at a prompt the command is over — holding the name would make
        // the next close question ask about something that already finished.
        DaemonMsg::Output(b"\x1b]133;B\x07".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(poll(""), "a new prompt clears the running command");

        // A bare `C` starts a command without naming it (the PowerShell path).
        // It must not leave the previous name standing.
        DaemonMsg::Output(b"\x1b]133;C;cargo build\x07".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(poll("cargo build"));
        DaemonMsg::Output(b"\x1b]133;C\x07".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(poll(""), "an unnamed command start does not inherit a name");
    }

    fn full_dump(term: &RemoteTerminal) -> String {
        use alacritty_terminal::grid::Dimensions as _;
        let t = term.term.lock();
        let grid = t.grid();
        let mut out = String::new();
        for l in -(grid.history_size() as i32)..grid.screen_lines() as i32 {
            for c in 0..grid.columns() {
                out.push(
                    grid[alacritty_terminal::index::Line(l)][alacritty_terminal::index::Column(c)]
                        .c,
                );
            }
            out.push('\n');
        }
        out
    }

    fn tui_frame(lines: &[String], prev_rows: usize) -> Vec<u8> {
        let mut b = Vec::new();
        if prev_rows > 1 {
            b.extend_from_slice(format!("\r\x1b[{}A\x1b[J", prev_rows - 1).as_bytes());
        }
        b.extend_from_slice(lines.join("\r\n").as_bytes());
        b
    }

    #[test]
    fn segmented_ring_replay_reproduces_live_rendering() {
        const MARK: &str = "DUPMARK";
        let frame_lines = |f: usize| -> Vec<String> {
            (0..10)
                .map(|i| format!("{MARK} f{f:02} l{i:02} {:.<74}", ""))
                .collect()
        };
        let mut history = Vec::new();
        for f in 0..8 {
            history.extend(tui_frame(&frame_lines(f), if f == 0 { 0 } else { 10 }));
        }

        let wait_for = |term: &RemoteTerminal, needle: &str| -> String {
            let mut dump = String::new();
            for _ in 0..400 {
                dump = full_dump(term);
                if dump.contains(needle) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            dump
        };
        let ws = |cols: u16| WinSize {
            cols,
            rows: 24,
            cell_w: 8,
            cell_h: 17,
        };

        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let mut live = RemoteTerminal::from_stream(client_side, TermSize::new(100, 24)).unwrap();
        DaemonMsg::Output(history.clone())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        let dump = wait_for(&live, "f07 l09");
        assert!(dump.contains("f07 l09"), "live output should have landed");
        live.resize(TermSize::new(80, 24), 8, 17);
        let live_count = full_dump(&live).matches(MARK).count();
        assert_eq!(
            live_count, 10,
            "live rendering is clean: each redraw erases the previous frame, \
             so exactly one 10-line copy survives the resize"
        );

        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let replay = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        DaemonMsg::Size(ws(100)).encode(&mut daemon_side).unwrap();
        DaemonMsg::Snapshot(history.clone())
            .encode(&mut daemon_side)
            .unwrap();
        DaemonMsg::Size(ws(80)).encode(&mut daemon_side).unwrap();
        DaemonMsg::Snapshot(Vec::new())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        let dump = wait_for(&replay, "f07 l09");
        {
            use alacritty_terminal::grid::Dimensions as _;
            let mut cols = 0;
            for _ in 0..400 {
                cols = replay.term.lock().columns();
                if cols == 80 {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert_eq!(cols, 80, "the trailing pair must end the grid at 80 cols");
        }
        let replay_count = dump.matches(MARK).count();
        assert_eq!(
            replay_count, live_count,
            "the segmented replay must reproduce the live rendering exactly"
        );

        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let flat = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        DaemonMsg::Size(ws(80)).encode(&mut daemon_side).unwrap();
        DaemonMsg::Snapshot(history)
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        let dump = wait_for(&flat, "f07 l09");
        let flat_count = dump.matches(MARK).count();
        assert!(
            flat_count > live_count,
            "flat replay at the final width should duplicate (got {flat_count}); \
             if it stopped, the segmented path may no longer be exercising anything"
        );
    }
}

mod osc_tests {
    use super::super::reader::{MAX_OSC_NOTES, OscNotifyScanner, parse_osc_notification};
    use super::super::{Note, RemoteTerminal, VecDeque};

    fn scan(chunks: &[&[u8]]) -> Vec<Note> {
        let mut s = OscNotifyScanner::default();
        let mut out = VecDeque::new();
        for c in chunks {
            s.feed(c, &mut out);
        }
        out.into()
    }

    /// A burst of notifications shows only its newest few, once.
    #[test]
    fn a_burst_of_notes_is_handed_over_as_its_newest_few() {
        use anterm_core::daemon::protocol::DaemonMsg;
        use crate::backend::size::TermSize;

        anterm_ui_kit::config::pin_test_config_dir();
        let (client, mut daemon) = crate::view::test_stream_pair();
        let term = RemoteTerminal::from_stream(client, TermSize::new(80, 24)).unwrap();
        let burst: Vec<u8> = (0..40)
            .flat_map(|n| format!("\x1b]9;note {n}\x07").into_bytes())
            .collect();
        DaemonMsg::Output(burst).encode(&mut daemon).unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let taken = loop {
            let batch = term.take_osc_notes();
            if !batch.is_empty() || std::time::Instant::now() > deadline {
                break batch;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        let bodies: Vec<_> = taken.into_iter().map(|(_, body)| body).collect();
        assert_eq!(bodies, ["note 37", "note 38", "note 39"]);
        assert!(term.take_osc_notes().is_empty(), "the rest were dropped");
    }

    #[test]
    fn an_unread_queue_keeps_only_the_newest_notes() {
        let chunks: Vec<Vec<u8>> = (0..MAX_OSC_NOTES + 2)
            .map(|n| format!("\x1b]9;note {n}\x07").into_bytes())
            .collect();
        let got = scan(&chunks.iter().map(Vec::as_slice).collect::<Vec<_>>());
        assert_eq!(got.len(), MAX_OSC_NOTES);
        assert_eq!(got[0], (None, "note 2".to_string()));
    }

    #[test]
    fn osc9_bel_and_st_terminators() {
        assert_eq!(
            scan(&[b"\x1b]9;Build done\x07"]),
            vec![(None, "Build done".to_string())]
        );
        assert_eq!(
            scan(&[b"\x1b]9;Tests passed\x1b\\"]),
            vec![(None, "Tests passed".to_string())]
        );
    }

    #[test]
    fn osc777_notify_title_and_body() {
        assert_eq!(
            scan(&[b"\x1b]777;notify;Title;Body text\x07"]),
            vec![(Some("Title".to_string()), "Body text".to_string())]
        );
        assert_eq!(
            scan(&[b"\x1b]777;notify;Just a message\x1b\\"]),
            vec![(None, "Just a message".to_string())]
        );
    }

    #[test]
    fn split_across_reads_is_reassembled() {
        assert_eq!(
            scan(&[b"\x1b]9;Hel", b"lo wor", b"ld\x07"]),
            vec![(None, "Hello world".to_string())]
        );
        assert_eq!(
            scan(&[b"\x1b]9;Ping\x1b", b"\\"]),
            vec![(None, "Ping".to_string())]
        );
    }

    #[test]
    fn uninteresting_osc_is_ignored_cheaply() {
        assert_eq!(
            scan(&[b"\x1b]52;c;bWFueSBieXRlcw==\x07\x1b]0;my title\x07"]),
            vec![]
        );
        assert_eq!(
            scan(&[b"\x1b]0;title\x07\x1b]9;After\x07"]),
            vec![(None, "After".to_string())]
        );
    }

    #[test]
    fn claude_codes_kitty_and_ghostty_channels() {
        assert_eq!(
            scan(&[
                b"\x1b]99;i=3:d=0:p=title;Claude Code\x07\x1b]99;i=3:p=body;Done\x07",
                b"\x1b]99;i=3:d=1:a=focus;\x07",
            ]),
            vec![(Some("Claude Code".to_string()), "Done".to_string())]
        );
        assert_eq!(
            scan(&[b"\x1b]777;notify;Claude Code;Done\x07"]),
            vec![(Some("Claude Code".to_string()), "Done".to_string())]
        );
    }

    #[test]
    fn conemu_osc9_subcommands_are_not_notifications() {
        assert_eq!(scan(&[b"\x1b]9;4;1;50\x07"]), vec![]);
        assert_eq!(scan(&[b"\x1b]9;9;/home/u\x07"]), vec![]);
        assert_eq!(scan(&[b"\x1b]9;12\x07"]), vec![], "a prompt mark");
        assert_eq!(scan(&[b"\x1b]9;11;a comment\x07"]), vec![]);
        assert_eq!(scan(&[b"\x1b]9;42\x07"]), vec![(None, "42".to_string())]);
    }

    #[test]
    fn parse_rejects_empty_and_unrelated() {
        let n = &mut Default::default();
        assert_eq!(parse_osc_notification(n, b"9;"), None);
        assert_eq!(parse_osc_notification(n, b"777;notify;"), None);
        assert_eq!(parse_osc_notification(n, b"8;;https://example.com"), None);
    }

    #[test]
    fn resyncs_on_new_osc_after_an_unterminated_one() {
        assert_eq!(
            scan(&[b"\x1b]9;dropped\x1b]9;kept\x07"]),
            vec![(None, "kept".to_string())]
        );
        assert_eq!(
            scan(&[b"\x1b]0;title\x1b]9;After title\x07"]),
            vec![(None, "After title".to_string())]
        );
    }
}

mod dec_mode_2027_tests {
    use super::super::term_adapter::{self, KeyboardStackGuard};
    use super::super::*;
    use alacritty_terminal::event::{Event as AlacEvent, EventListener};
    use alacritty_terminal::index::{Column, Line};
    use alacritty_terminal::term::cell::Flags;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct CaptureListener(Arc<Mutex<Vec<String>>>);

    impl EventListener for CaptureListener {
        fn send_event(&self, event: AlacEvent) {
            if let AlacEvent::PtyWrite(reply) = event {
                self.0.lock().unwrap().push(reply);
            }
        }
    }

    #[test]
    fn dec_mode_2027_decrqm_and_grapheme_clustering() {
        let listener = CaptureListener::default();
        let config = terminal_config_from_user(&anterm_ui_kit::config::Config::default());
        let mut term = Term::new(config, &TermSize::new(40, 6), listener.clone());
        let mut proc = ansi::Processor::new();
        let mut guard = KeyboardStackGuard::default();

        // Initially off -> DECRQM reports supported-but-reset (2).
        term_adapter::advance_with_listener(
            &mut proc,
            &mut term,
            &mut guard,
            Some(&listener),
            b"\x1b[?2027$p",
        );
        assert!(!guard.grapheme_clustering());
        assert_eq!(
            std::mem::take(&mut *listener.0.lock().unwrap()),
            vec!["\x1b[?2027;2$y"]
        );

        // Enable Mode 2027 -> DECRQM reports set (1).
        term_adapter::advance_with_listener(
            &mut proc,
            &mut term,
            &mut guard,
            Some(&listener),
            b"\x1b[?2027h\x1b[?2027$p",
        );
        assert!(guard.grapheme_clustering());
        assert_eq!(
            std::mem::take(&mut *listener.0.lock().unwrap()),
            vec!["\x1b[?2027;1$y"]
        );

        // Feed ZWJ emoji, chained ZWJ, skin-tone, keycap, and regional-indicator flags.
        let sample = "👨\u{200D}💻A👨\u{200D}👩\u{200D}👧\u{200D}👦B👍🏽C1\u{FE0F}\u{20E3}D🇺🇸🇯🇵E";
        term_adapter::advance_with_listener(
            &mut proc,
            &mut term,
            &mut guard,
            Some(&listener),
            sample.as_bytes(),
        );

        let row = &term.grid()[Line(0)];
        // Col 0..2: 👨‍💻, Col 2: 'A'
        assert_eq!(row[Column(0)].c, '👨');
        assert!(row[Column(0)].flags.contains(Flags::WIDE_CHAR));
        assert_eq!(row[Column(0)].zerowidth(), Some(&['\u{200D}', '💻'][..]));
        assert!(row[Column(1)].flags.contains(Flags::WIDE_CHAR_SPACER));
        assert_eq!(row[Column(2)].c, 'A');

        // Col 3..5: 👨‍👩‍👧‍👦, Col 5: 'B'
        assert_eq!(row[Column(3)].c, '👨');
        assert!(row[Column(3)].flags.contains(Flags::WIDE_CHAR));
        assert_eq!(
            row[Column(3)].zerowidth(),
            Some(&['\u{200D}', '👩', '\u{200D}', '👧', '\u{200D}', '👦'][..])
        );
        assert_eq!(row[Column(5)].c, 'B');

        // Col 6..8: 👍🏽, Col 8: 'C'
        assert_eq!(row[Column(6)].c, '👍');
        assert!(row[Column(6)].flags.contains(Flags::WIDE_CHAR));
        assert_eq!(row[Column(6)].zerowidth(), Some(&['🏽'][..]));
        assert_eq!(row[Column(8)].c, 'C');

        // Col 9..11: 1️⃣, Col 11: 'D'
        assert_eq!(row[Column(9)].c, '1');
        assert!(row[Column(9)].flags.contains(Flags::WIDE_CHAR));
        assert_eq!(row[Column(9)].zerowidth(), Some(&['\u{FE0F}', '\u{20E3}'][..]));
        assert_eq!(row[Column(11)].c, 'D');

        // Col 12..14: 🇺🇸, Col 14..16: 🇯🇵, Col 16: 'E'
        assert_eq!(row[Column(12)].c, '🇺');
        assert!(row[Column(12)].flags.contains(Flags::WIDE_CHAR));
        assert_eq!(row[Column(12)].zerowidth(), Some(&['🇸'][..]));
        assert_eq!(row[Column(14)].c, '🇯');
        assert!(row[Column(14)].flags.contains(Flags::WIDE_CHAR));
        assert_eq!(row[Column(14)].zerowidth(), Some(&['🇵'][..]));
        assert_eq!(row[Column(16)].c, 'E');

        // Disable Mode 2027 -> DECRQM reports reset (2).
        term_adapter::advance_with_listener(
            &mut proc,
            &mut term,
            &mut guard,
            Some(&listener),
            b"\x1b[?2027l\x1b[?2027$p",
        );
        assert!(!guard.grapheme_clustering());
        assert_eq!(
            std::mem::take(&mut *listener.0.lock().unwrap()),
            vec!["\x1b[?2027;2$y"]
        );

        // Enable then RIS (\x1bc) resets Mode 2027.
        term_adapter::advance_with_listener(
            &mut proc,
            &mut term,
            &mut guard,
            Some(&listener),
            b"\x1b[?2027h\x1bc",
        );
        assert!(!guard.grapheme_clustering());
    }
}
