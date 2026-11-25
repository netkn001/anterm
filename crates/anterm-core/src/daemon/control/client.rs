use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::daemon::protocol::{read_frame, write_frame};

use super::codec::invalid;
use super::{
    ControlClientMsg, ControlEvent, ControlHello, ControlHelloOk, ControlReply, ControlRequest,
    ControlServerMsg, ReplyOk, WireError, WireErrorKind, dialect_refusal,
};

pub const KEEPALIVE_PING_INTERVAL: Duration = Duration::from_secs(15);
pub const KEEPALIVE_IDLE_BEFORE_PING: Duration = Duration::from_secs(30);
pub const KEEPALIVE_DEAD_AFTER: Duration = Duration::from_secs(45);

pub const CLOSE_GRACE: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, PartialEq)]
pub struct ControlResponse {
    pub reply: ReplyOk,
    pub blob: Vec<u8>,
}

pub type EventSink = Box<dyn Fn(ControlEvent) + Send + Sync + 'static>;

pub trait LinkShutdown: Send + Sync + 'static {
    fn shutdown_link(&self) -> io::Result<()>;
}

impl LinkShutdown for std::net::TcpStream {
    fn shutdown_link(&self) -> io::Result<()> {
        self.shutdown(std::net::Shutdown::Both)
    }
}

#[cfg(unix)]
impl LinkShutdown for std::os::unix::net::UnixStream {
    fn shutdown_link(&self) -> io::Result<()> {
        self.shutdown(std::net::Shutdown::Both)
    }
}

pub struct ControlClient {
    inner: Arc<ClientInner>,
    reader: Mutex<Option<std::thread::JoinHandle<()>>>,
}

pub(super) struct ClientInner {
    pub(super) writer: Mutex<Box<dyn Write + Send>>,
    pub(super) next_req_id: AtomicU64,
    pub(super) pending: Mutex<HashMap<u64, SyncSender<ControlReply>>>,
    pub(super) blobs: Mutex<HashMap<u64, Vec<u8>>>,
    pub(super) connected: AtomicBool,
    pub(super) last_inbound: Mutex<Instant>,
    /// How long the last `Ping` took to come back, in microseconds; zero until
    /// one has. Only `Ping` is timed: every other request does work on the far
    /// side, so its round trip measures that work and not the link, and a
    /// `ReadFile` of a large file would report the network as seconds slow.
    pub(super) last_rtt_us: AtomicU64,
    pub(super) hello: ControlHelloOk,
    pub(super) shutdown: Option<Arc<dyn LinkShutdown>>,
    pub(super) reader_done: Mutex<bool>,
    pub(super) reader_exit: Condvar,
    pub(super) link_down: Mutex<Vec<Box<dyn Fn() + Send + Sync>>>,
}

impl ControlClient {
    pub fn connect<R, W>(
        r: R,
        w: W,
        hello: &ControlHello,
        events: EventSink,
    ) -> io::Result<ControlClient>
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
    {
        Self::connect_with(r, w, None, hello, events)
    }

    pub fn over_tcp(
        sock: std::net::TcpStream,
        hello: &ControlHello,
        events: EventSink,
    ) -> io::Result<ControlClient> {
        let r = sock.try_clone()?;
        let closer: Arc<dyn LinkShutdown> = Arc::new(sock.try_clone()?);
        Self::connect_with(r, sock, Some(closer), hello, events)
    }

    #[cfg(unix)]
    pub fn over_unix(
        sock: std::os::unix::net::UnixStream,
        hello: &ControlHello,
        events: EventSink,
    ) -> io::Result<ControlClient> {
        let r = sock.try_clone()?;
        let closer: Arc<dyn LinkShutdown> = Arc::new(sock.try_clone()?);
        Self::connect_with(r, sock, Some(closer), hello, events)
    }

    pub fn connect_with<R, W>(
        mut r: R,
        mut w: W,
        shutdown: Option<Arc<dyn LinkShutdown>>,
        hello: &ControlHello,
        events: EventSink,
    ) -> io::Result<ControlClient>
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
    {
        ControlClientMsg::Hello(hello.clone()).encode(&mut w)?;
        w.flush()?;

        let ok = match ControlServerMsg::read(&mut r)? {
            ControlServerMsg::HelloOk(ok) => ok,
            other => {
                return Err(invalid(format!(
                    "control peer answered the handshake with {other:?} instead of HELLO_OK"
                )));
            }
        };
        if ok.control_version != hello.control_version {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                dialect_refusal(&ok.build, ok.control_version, hello.control_version),
            ));
        }

        let inner = Arc::new(ClientInner {
            writer: Mutex::new(Box::new(w)),
            next_req_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            blobs: Mutex::new(HashMap::new()),
            connected: AtomicBool::new(true),
            last_inbound: Mutex::new(Instant::now()),
            last_rtt_us: AtomicU64::new(0),
            hello: ok,
            shutdown,
            reader_done: Mutex::new(false),
            reader_exit: Condvar::new(),
            link_down: Mutex::new(Vec::new()),
        });

        let reader_inner = Arc::clone(&inner);
        let reader = std::thread::Builder::new()
            .name("anterm-control-reader".into())
            .spawn(move || reader_loop(reader_inner, r, events))?;

        Ok(ControlClient {
            inner,
            reader: Mutex::new(Some(reader)),
        })
    }

    pub fn hello(&self) -> &ControlHelloOk {
        &self.inner.hello
    }

    pub fn is_connected(&self) -> bool {
        self.inner.connected.load(Ordering::Acquire)
    }

    pub fn on_link_down<F: Fn() + Send + Sync + 'static>(&self, hook: F) {
        if let Ok(mut hooks) = self.inner.link_down.lock() {
            hooks.push(Box::new(hook));
        }
        if !self.is_connected() {
            self.inner.run_link_down();
        }
    }

    pub fn idle_for(&self) -> Duration {
        self.inner
            .last_inbound
            .lock()
            .map(|t| t.elapsed())
            .unwrap_or_default()
    }

    /// The last measured round trip to the peer, or `None` on a link nothing
    /// has pinged yet.
    ///
    /// Fed by every [`ControlRequest::Ping`] that comes back — the keepalive's
    /// as well as any a caller sends itself — so a link that is being kept
    /// alive already carries a number without anyone asking for one.
    pub fn last_rtt(&self) -> Option<Duration> {
        match self.inner.last_rtt_us.load(Ordering::Relaxed) {
            0 => None,
            us => Some(Duration::from_micros(us)),
        }
    }

    pub fn call(&self, req: ControlRequest) -> io::Result<ReplyOk> {
        self.call_full(req, &[]).map(|r| r.reply)
    }

    pub fn call_with_blob(&self, req: ControlRequest, blob: &[u8]) -> io::Result<ReplyOk> {
        self.call_full(req, blob).map(|r| r.reply)
    }

    /// Sends `req` without waiting for its answer. The reply, when it comes,
    /// finds nobody waiting for its `req_id` and the reader drops it.
    ///
    /// For requests whose answer nobody acts on, made from places that cannot
    /// afford a round trip: a `Drop` that may run on the UI thread, where a
    /// `call` parks every window for as long as the peer takes to answer — up
    /// to the request's whole deadline on a link that has gone quiet.
    pub fn post(&self, req: ControlRequest) -> io::Result<()> {
        if !self.is_connected() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "control connection is down",
            ));
        }
        let req_id = self.inner.next_req_id.fetch_add(1, Ordering::Relaxed);
        log::debug!(target: "anterm::control", "#{req_id} {req:?} (not awaited)");
        self.inner.send(&ControlClientMsg::Request { req_id, req })
    }

    pub fn call_full(&self, req: ControlRequest, blob: &[u8]) -> io::Result<ControlResponse> {
        let deadline = req.deadline();
        self.call_with_deadline(req, blob, deadline)
    }

    pub fn call_with_deadline(
        &self,
        req: ControlRequest,
        blob: &[u8],
        deadline: Duration,
    ) -> io::Result<ControlResponse> {
        if !self.is_connected() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "control connection is down",
            ));
        }

        let req_id = self.inner.next_req_id.fetch_add(1, Ordering::Relaxed);
        // Read off before `req` is moved into the message below.
        let timed = matches!(req, ControlRequest::Ping);
        let sent_at = Instant::now();
        log::debug!(target: "anterm::control", "#{req_id} {req:?}");
        let (tx, rx) = sync_channel(1);
        self.inner.pending()?.insert(req_id, tx);

        let msg = if blob.is_empty() && !req.takes_blob() {
            ControlClientMsg::Request { req_id, req }
        } else {
            ControlClientMsg::RequestBlob {
                req_id,
                req,
                blob: blob.to_vec(),
            }
        };

        if let Err(e) = self.inner.send(&msg) {
            self.inner.forget(req_id);
            return Err(e);
        }

        match rx.recv_timeout(deadline) {
            Ok(reply) => {
                let blob = self.inner.take_blob(req_id);
                let out = reply
                    .into_result()
                    .map(|reply| ControlResponse { reply, blob });
                // Only a ping that actually came back. A timeout leaves the
                // last good number in place rather than recording the deadline
                // as the link's latency, and a refusal measures the peer's
                // opinion of the request rather than the distance to it.
                if timed && out.is_ok() {
                    let us = sent_at.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
                    // Zero means "never measured", so a sub-microsecond round
                    // trip on a loopback link rounds up rather than reading as
                    // no measurement at all.
                    self.inner.last_rtt_us.store(us.max(1), Ordering::Relaxed);
                }
                out
            }
            Err(RecvTimeoutError::Timeout) => {
                self.inner.forget(req_id);
                let _ = self.inner.send(&ControlClientMsg::Cancel { req_id });
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("control request {req_id} timed out after {deadline:?}"),
                ))
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.inner.forget(req_id);
                Err(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "control connection closed while the request was in flight",
                ))
            }
        }
    }

    pub fn ping(&self) -> io::Result<()> {
        self.call(ControlRequest::Ping).map(|_| ())
    }

    pub fn close(&self) {
        self.inner
            .fail_all("control connection closed by this client");
        if let Some(closer) = &self.inner.shutdown
            && let Err(e) = closer.shutdown_link()
        {
            log::trace!("control link shutdown reported {e}");
        }

        let Ok(mut slot) = self.reader.lock() else {
            return;
        };
        let Some(handle) = slot.take() else { return };

        let reaped = match self.inner.reader_done.lock() {
            Ok(done) => self
                .inner
                .reader_exit
                .wait_timeout_while(done, CLOSE_GRACE, |done| !*done)
                .map(|(done, _)| *done)
                .unwrap_or(false),
            Err(_) => false,
        };
        if reaped {
            let _ = handle.join();
        } else {
            log::debug!(
                "control reader did not exit within {CLOSE_GRACE:?}; detaching it \
                 rather than blocking the caller"
            );
            drop(handle);
        }
    }
}

impl Drop for ControlClient {
    fn drop(&mut self) {
        self.close();
    }
}

impl std::fmt::Debug for ControlClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let in_flight = self.inner.pending.lock().map(|p| p.len()).unwrap_or(0);
        f.debug_struct("ControlClient")
            .field("build", &self.inner.hello.build)
            .field("control_version", &self.inner.hello.control_version)
            .field("connected", &self.is_connected())
            .field("in_flight", &in_flight)
            .finish()
    }
}

impl ClientInner {
    fn pending(
        &self,
    ) -> io::Result<std::sync::MutexGuard<'_, HashMap<u64, SyncSender<ControlReply>>>> {
        self.pending
            .lock()
            .map_err(|_| io::Error::other("control request table was poisoned"))
    }

    pub(super) fn send(&self, msg: &ControlClientMsg) -> io::Result<()> {
        let (k, payload) = msg.to_frame()?;

        let mut w = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("control writer was poisoned"))?;
        let r = write_frame(&mut *w, k, &payload).and_then(|()| w.flush());
        if r.is_err() {
            self.connected.store(false, Ordering::Release);
        }
        r
    }

    pub(super) fn forget(&self, req_id: u64) {
        if let Ok(mut p) = self.pending.lock() {
            p.remove(&req_id);
        }
        if let Ok(mut b) = self.blobs.lock() {
            b.remove(&req_id);
        }
    }

    fn take_blob(&self, req_id: u64) -> Vec<u8> {
        self.blobs
            .lock()
            .ok()
            .and_then(|mut b| b.remove(&req_id))
            .unwrap_or_default()
    }

    pub(super) fn deliver(&self, req_id: u64, reply: ControlReply, blob: Vec<u8>) {
        let Ok(mut pending) = self.pending.lock() else {
            return;
        };
        let Some(tx) = pending.remove(&req_id) else {
            log::trace!("control reply for unknown req_id {req_id}; dropping");
            return;
        };
        if !blob.is_empty()
            && let Ok(mut blobs) = self.blobs.lock()
        {
            blobs.insert(req_id, blob);
        }
        drop(pending);
        let _ = tx.try_send(reply);
    }

    fn fail_all(&self, why: &str) {
        self.connected.store(false, Ordering::Release);
        let drained: Vec<_> = match self.pending.lock() {
            Ok(mut p) => p.drain().collect(),
            Err(_) => Vec::new(),
        };
        for (req_id, tx) in drained {
            let _ = tx.try_send(ControlReply::Err(WireError::new(
                WireErrorKind::ConnectionReset,
                format!("{why} (request {req_id})"),
            )));
        }
        self.run_link_down();
    }

    fn run_link_down(&self) {
        if let Ok(hooks) = self.link_down.lock() {
            for hook in hooks.iter() {
                hook();
            }
        }
    }
}

fn reader_loop<R: Read>(inner: Arc<ClientInner>, r: R, events: EventSink) {
    read_until_closed(&inner, r, events);
    if let Ok(mut done) = inner.reader_done.lock() {
        *done = true;
    }
    inner.reader_exit.notify_all();
}

fn read_until_closed<R: Read>(inner: &Arc<ClientInner>, mut r: R, events: EventSink) {
    loop {
        let frame = match read_frame(&mut r) {
            Ok(f) => f,
            Err(e) => {
                log::debug!("control reader stopping: {e}");
                inner.fail_all("control connection lost");
                return;
            }
        };
        if let Ok(mut t) = inner.last_inbound.lock() {
            *t = Instant::now();
        }
        match ControlServerMsg::from_frame(frame.0, frame.1) {
            Ok(ControlServerMsg::Response { req_id, reply }) => {
                inner.deliver(req_id, reply, Vec::new());
            }
            Ok(ControlServerMsg::ResponseBlob {
                req_id,
                reply,
                blob,
            }) => {
                inner.deliver(req_id, reply, blob);
            }
            Ok(ControlServerMsg::Event(event)) => events(event),
            Ok(ControlServerMsg::HelloOk(_)) => {
                log::warn!("control peer sent a second HELLO_OK; dropping the connection");
                inner.fail_all("control peer re-sent its handshake");
                return;
            }
            Err(e) => {
                log::warn!("control decode error: {e}");
                inner.fail_all("control stream desynchronized");
                return;
            }
        }
    }
}
