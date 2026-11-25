use super::{Event, GraphicsParser, Image, ImageDelete, MAX_TRANSMISSION_BASE64, MediumTransfer};

/// Cap on a single APC payload (one chunk) we'll buffer before abandoning it.
/// `terminal-browser` chunks direct transmissions into 4 KiB base64 pieces, but
/// the spec only *recommends* chunking — a sender is free to put a whole frame
/// in one command, and an uncompressed 4K RGBA frame is ~44 MB of base64. Size
/// this to admit a one-shot frame that still fits [`MAX_TRANSMISSION_BASE64`];
/// [`ApcTokenizer::push_graphics`] logs whatever it has to drop.
pub(super) const MAX_APC_PAYLOAD: usize = MAX_TRANSMISSION_BASE64;

/// A streaming tokenizer that splits raw output into a *passthrough* byte stream
/// and the kitty graphics commands lifted out of it.
///
/// Feed it raw output bytes. It invokes `on_passthrough` with every byte that is
/// **not** part of a `_G` graphics sequence — that is the exact input with each
/// `ESC _ G … ST` removed, and it is what the daemon appends to the replay ring
/// and forwards to the client. It invokes `on_command` with the complete payload
/// of each `_G` command (the bytes after `ESC _`, terminator excluded — e.g.
/// `Ga=T,f=32,…;<base64>`). Non-graphics APC sequences (any payload not starting
/// with `G`) are passed through verbatim, since only kitty graphics is ours to
/// intercept; everything else must reach the client's VT parser unchanged. State
/// persists across `feed` calls, so a sequence split over several reads is still
/// handled.
pub struct ApcTokenizer {
    /// Bytes of a `_G` command accumulated after `ESC _ G` while it can still
    /// terminate. Cleared whenever a command finishes or is abandoned.
    buf: Vec<u8>,
    state: State,
}

#[derive(Default, Clone, Copy)]
enum State {
    /// Not inside an escape sequence; bytes are passthrough.
    #[default]
    Ground,
    /// Held one `ESC` in ground state; a following `_` opens an APC.
    Esc,
    /// Held `ESC _`; the next byte decides graphics (`G`) vs. passthrough APC.
    ApcStart,
    /// Buffering a `_G` graphics command (stripped from passthrough).
    ApcGraphics,
    /// Saw `ESC` inside a graphics command — a following `\` is the terminator.
    ApcGraphicsEsc,
    /// Forwarding a non-graphics APC as passthrough (bytes kept verbatim).
    PassApc,
    /// Saw `ESC` inside a passthrough APC — a following `\` is the terminator.
    PassApcEsc,
    /// Dropping an abandoned/oversized graphics APC (emitted nowhere).
    ApcDrop,
    /// Saw `ESC` inside a dropped APC — a following `\` is the terminator.
    ApcDropEsc,
}

impl Default for ApcTokenizer {
    fn default() -> Self {
        Self::new()
    }
}

impl ApcTokenizer {
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            state: State::Ground,
        }
    }

    /// Feed one chunk of output. `on_passthrough` receives runs of non-graphics
    /// bytes (the input minus every `ESC _ G … ST`); `on_command` receives the
    /// `G…` payload of each graphics command that terminates within the chunk.
    ///
    /// Like the OSC tokenizer, the states that dominate a real stream — `Ground`
    /// between sequences, and the scan-to-terminator states inside an APC — skip
    /// ahead with SIMD `memchr` rather than stepping per byte; only the few
    /// escape-decision states step one byte at a time. APC has no `BEL`
    /// terminator (kitty always closes with `ST`), so only `ESC` can end a
    /// payload. A held `ESC`/`ESC _` that later proves to be passthrough is
    /// re-emitted as a constant, so nothing needs to survive across `feed` calls
    /// but the small state tag and the graphics buffer.
    pub fn feed(
        &mut self,
        bytes: &[u8],
        mut on_passthrough: impl FnMut(&[u8]),
        mut on_command: impl FnMut(&[u8]),
    ) {
        let mut i = 0;
        while i < bytes.len() {
            match self.state {
                State::Ground => match memchr::memchr(0x1b, &bytes[i..]) {
                    Some(off) => {
                        if off > 0 {
                            on_passthrough(&bytes[i..i + off]);
                        }
                        self.state = State::Esc;
                        i += off + 1;
                    }
                    None => {
                        on_passthrough(&bytes[i..]);
                        return;
                    }
                },
                State::PassApc => match memchr::memchr(0x1b, &bytes[i..]) {
                    Some(off) => {
                        if off > 0 {
                            on_passthrough(&bytes[i..i + off]);
                        }
                        self.state = State::PassApcEsc;
                        i += off + 1;
                    }
                    None => {
                        on_passthrough(&bytes[i..]);
                        return;
                    }
                },
                State::ApcDrop => match memchr::memchr(0x1b, &bytes[i..]) {
                    Some(off) => {
                        self.state = State::ApcDropEsc;
                        i += off + 1;
                    }
                    None => return,
                },
                State::ApcGraphics => match memchr::memchr(0x1b, &bytes[i..]) {
                    Some(off) => {
                        if self.push_graphics(&bytes[i..i + off]) {
                            self.state = State::ApcGraphicsEsc;
                            i += off + 1;
                        } else {
                            // Oversized: abandon and let the drop state consume
                            // the terminator we just found (leave `i` on the ESC).
                            self.state = State::ApcDrop;
                            i += off;
                        }
                    }
                    None => {
                        if !self.push_graphics(&bytes[i..]) {
                            self.state = State::ApcDrop;
                        }
                        return;
                    }
                },
                State::Esc => match bytes[i] {
                    b'_' => {
                        self.buf.clear();
                        self.state = State::ApcStart;
                        i += 1;
                    }
                    // A run of ESCs: the earlier one was a lone ESC (passthrough);
                    // keep the newest one held.
                    0x1b => {
                        on_passthrough(b"\x1b");
                        i += 1;
                    }
                    // The ESC began some other escape: emit it and re-examine this
                    // byte from ground (it is not an ESC, so it joins the run).
                    _ => {
                        on_passthrough(b"\x1b");
                        self.state = State::Ground;
                    }
                },
                State::ApcStart => {
                    if bytes[i] == b'G' {
                        self.buf.clear();
                        self.buf.push(b'G');
                        self.state = State::ApcGraphics;
                        i += 1;
                    } else {
                        // Not ours: emit the held `ESC _` and forward the rest of
                        // the APC verbatim (re-examine this byte in `PassApc`).
                        on_passthrough(b"\x1b_");
                        self.state = State::PassApc;
                    }
                }
                State::ApcGraphicsEsc => match bytes[i] {
                    b'\\' => {
                        on_command(&self.buf);
                        self.buf.clear();
                        self.state = State::Ground;
                        i += 1;
                    }
                    0x1b => i += 1, // a run of ESCs; stay poised for `\`
                    // A bare `ESC _` re-opens: the next byte decides graphics again.
                    b'_' => {
                        self.buf.clear();
                        self.state = State::ApcStart;
                        i += 1;
                    }
                    // ESC began some other escape: abandon this graphics command.
                    // Its bytes were meant as graphics, so they stay stripped —
                    // but the escape itself belongs to the terminal, so forward
                    // it and re-examine this byte from Ground rather than eating
                    // both. Swallowing them would turn a `\x1b[31m` that follows
                    // an unterminated graphics command into literal `31m`.
                    _ => {
                        self.buf.clear();
                        on_passthrough(b"\x1b");
                        self.state = State::Ground;
                    }
                },
                State::PassApcEsc => match bytes[i] {
                    b'\\' => {
                        on_passthrough(b"\x1b\\"); // ST is part of the forwarded APC
                        self.state = State::Ground;
                        i += 1;
                    }
                    0x1b => {
                        on_passthrough(b"\x1b"); // a lone ESC in APC data
                        i += 1;
                    }
                    // A bare `ESC _` re-opens, same as the two graphics states
                    // do. Without this, a foreign APC that never sends its ST
                    // swallows every `ESC _G …` after it — the graphics get
                    // forwarded as APC text that the client's vte then discards,
                    // so the image is simply lost.
                    b'_' => {
                        self.buf.clear();
                        self.state = State::ApcStart;
                        i += 1;
                    }
                    _ => {
                        on_passthrough(b"\x1b");
                        self.state = State::PassApc; // re-examine this byte
                    }
                },
                State::ApcDropEsc => match bytes[i] {
                    b'\\' => {
                        self.state = State::Ground;
                        i += 1;
                    }
                    0x1b => i += 1,
                    b'_' => {
                        self.buf.clear();
                        self.state = State::ApcStart;
                        i += 1;
                    }
                    // As in `ApcGraphicsEsc`: the dropped command's bytes stay
                    // stripped, but the escape that interrupted it is the
                    // terminal's and has to reach the client intact.
                    _ => {
                        on_passthrough(b"\x1b");
                        self.state = State::Ground;
                    }
                },
            }
        }
    }

    /// Append a run to the graphics buffer; returns `false` (and clears the
    /// buffer) if it would exceed [`MAX_APC_PAYLOAD`], signalling the caller to
    /// abandon the command.
    fn push_graphics(&mut self, run: &[u8]) -> bool {
        if self.buf.len() + run.len() > MAX_APC_PAYLOAD {
            log::debug!(
                "kitty graphics: dropping a command whose payload passed {MAX_APC_PAYLOAD} bytes"
            );
            self.buf.clear();
            return false;
        }
        self.buf.extend_from_slice(run);
        true
    }
}

/// Daemon-side splitter: the [`ApcTokenizer`] wired to a [`GraphicsParser`].
///
/// This is the one type the PTY reader loop touches. Feed it raw output and a
/// passthrough sink; it strips every kitty graphics sequence, funnels the
/// passthrough bytes to the sink (for the replay ring and the client), and
/// returns the decoded graphics [`Event`]s — query replies to write back to the
/// PTY, and images to forward out-of-band.
#[derive(Default)]
pub struct GraphicsSniffer {
    tokenizer: ApcTokenizer,
    parser: GraphicsParser,
}

impl GraphicsSniffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// A sniffer whose parser may honor file/shm transfer (local pane only).
    /// See [`GraphicsParser::new_local`].
    pub fn new_local(local: bool) -> Self {
        Self {
            tokenizer: ApcTokenizer::new(),
            parser: GraphicsParser::new_local(local),
        }
    }

    /// Update whether the sender currently shares this host's filesystem. A pane
    /// is local when anterm spawned a local shell and no foreground `ssh` owns the
    /// PTY; that can flip mid-session, and it gates whether the next `a=q` probe
    /// is answered `OK` for file/shm transfer. Cheap enough to call per chunk.
    pub fn set_local(&mut self, local: bool) {
        self.parser.local = local;
    }

    /// Feed one chunk of raw PTY output. `on_passthrough` receives the byte
    /// stream with all graphics sequences removed; the returned events are the
    /// queries/images/deletes that completed within this chunk, in order.
    ///
    /// This drops the *relative order* of passthrough vs. events; for the daemon
    /// loop, prefer [`sniff`](Self::sniff), which preserves it. Retained as the
    /// low-level primitive the unit tests drive.
    pub fn feed(&mut self, bytes: &[u8], on_passthrough: impl FnMut(&[u8])) -> Vec<Event> {
        let Self { tokenizer, parser } = self;
        let mut events = Vec::new();
        tokenizer.feed(bytes, on_passthrough, |cmd| {
            if let Some(ev) = parser.feed(cmd) {
                events.push(ev);
            }
        });
        events
    }

    /// Feed a chunk and get back its content **in stream order**, taking a
    /// zero-copy fast path on the overwhelmingly common chunk that carries no
    /// graphics at all.
    ///
    /// Order matters: a kitty image is placed at the cursor cell as it stood
    /// *when its command appeared in the stream*, so the client must apply the
    /// text before an image before resolving that image's anchor. Returning
    /// ordered [`Segment`]s lets the daemon forward `Output`/`Image` frames
    /// interleaved exactly as they arrived, so the client's mirror cursor is
    /// correct at each placement.
    ///
    /// The fast path fires when the tokenizer is between sequences *and* the
    /// chunk holds no APC opener (`ESC _`) — then the input **is** one big
    /// output segment, returned borrowed with no allocation. A build's worth of
    /// colored stdout (full of CSI escapes but no APC) stays on this path. The
    /// trailing-`ESC` guard keeps an `ESC _` straddling a chunk boundary on the
    /// slow path, where the held-`ESC` state carries it across correctly.
    pub fn sniff<'a>(&mut self, bytes: &'a [u8]) -> Sniffed<'a> {
        if matches!(self.tokenizer.state, State::Ground)
            && bytes.last() != Some(&0x1b)
            && memchr::memmem::find(bytes, b"\x1b_").is_none()
        {
            return Sniffed::Plain(bytes);
        }
        let Self { tokenizer, parser } = self;
        // Both tokenizer callbacks push onto the same segment list; `RefCell`
        // lets the passthrough and command sinks share it without either taking
        // a lasting mutable borrow the borrow checker would reject.
        let segments: std::cell::RefCell<Vec<Segment>> = std::cell::RefCell::new(Vec::new());
        tokenizer.feed(
            bytes,
            |run| {
                // Coalesce adjacent passthrough runs into one output segment so
                // the client applies them in a single `advance`.
                let mut segs = segments.borrow_mut();
                if let Some(Segment::Output(buf)) = segs.last_mut() {
                    buf.extend_from_slice(run);
                } else {
                    segs.push(Segment::Output(run.to_vec()));
                }
            },
            |cmd| {
                if let Some(ev) = parser.feed(cmd) {
                    segments.borrow_mut().push(Segment::from(ev));
                }
            },
        );
        Sniffed::Segments(segments.into_inner())
    }
}

/// One ordered piece of a sniffed chunk — see [`GraphicsSniffer::sniff`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Non-graphics bytes to append to the ring / forward as `Output`.
    Output(Vec<u8>),
    /// An `a=q` reply to write back to the PTY.
    Query(Vec<u8>),
    /// An image to forward out-of-band.
    Image(Image),
    /// A file/shm transmission the daemon must resolve into pixels before
    /// forwarding (local panes only). See [`MediumTransfer`].
    ImageFromMedium(MediumTransfer),
    /// A delete to forward out-of-band.
    Delete(ImageDelete),
}

impl From<Event> for Segment {
    fn from(ev: Event) -> Self {
        match ev {
            Event::Query { reply, .. } => Segment::Query(reply),
            Event::Image(img) => Segment::Image(img),
            Event::ImageFromMedium(t) => Segment::ImageFromMedium(t),
            Event::Delete(c) => Segment::Delete(ImageDelete::from_control(&c)),
        }
    }
}

/// The result of [`GraphicsSniffer::sniff`]: either the whole chunk borrowed as
/// output (the graphics-free fast path), or ordered [`Segment`]s.
pub enum Sniffed<'a> {
    /// No graphics in this chunk: the input is output, verbatim and borrowed.
    Plain(&'a [u8]),
    /// Graphics present: apply these in order.
    Segments(Vec<Segment>),
}
