use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::daemon::protocol::{DaemonMsg, WinSize};

use super::restore::replay_state;
use super::ring::RING_CAP;
use super::{GraphicsFrame, PaneState};

pub struct OutputGate {
    queued: AtomicI64,
    park: Mutex<()>,
    drained: Condvar,
}

impl OutputGate {
    pub(super) const HIGH_WATER: i64 = 16 * 1024 * 1024;
    pub(super) const MAX_WAIT: Duration = Duration::from_secs(2);

    pub(crate) fn new() -> Self {
        Self {
            queued: AtomicI64::new(0),
            park: Mutex::new(()),
            drained: Condvar::new(),
        }
    }

    pub(super) fn add(&self, n: usize) {
        self.queued.fetch_add(n as i64, Ordering::Relaxed);
    }

    pub fn sub(&self, n: usize) {
        let prev = self.queued.fetch_sub(n as i64, Ordering::Relaxed);
        if prev >= Self::HIGH_WATER && prev - (n as i64) < Self::HIGH_WATER {
            let _park = self.park.lock().unwrap_or_else(|e| e.into_inner());
            self.drained.notify_all();
        }
    }

    pub(super) fn reset(&self) {
        self.queued.store(0, Ordering::Relaxed);
        let _park = self.park.lock().unwrap_or_else(|e| e.into_inner());
        self.drained.notify_all();
    }

    pub(crate) fn queued_bytes(&self) -> i64 {
        self.queued.load(Ordering::Relaxed)
    }

    pub(super) fn wait_below_high_water(&self) {
        if self.queued.load(Ordering::Relaxed) < Self::HIGH_WATER {
            return;
        }
        let deadline = std::time::Instant::now() + Self::MAX_WAIT;
        let mut park = self.park.lock().unwrap_or_else(|e| e.into_inner());
        while self.queued.load(Ordering::Relaxed) >= Self::HIGH_WATER {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return;
            }
            let (guard, _) = self.drained.wait_timeout(park, left).unwrap_or_else(|e| e.into_inner());
            park = guard;
        }
    }
}

/// What an observer may have queued and not yet written: the replay it is
/// charged for on subscribing, which can be a whole ring, and as much again
/// of live output. Were it only a ring, a full one would leave no room while
/// its replay is still going out, and the first byte the pane printed in that
/// time would drop the observer.
pub(crate) const OBSERVER_BUDGET: i64 = 2 * RING_CAP as i64;

/// What an observer dropped at its budget is told, last on its stream.
pub(crate) const OBSERVER_BEHIND: &str =
    "fell behind this pane's output; open it again to catch up";

/// The bytes a message counts against its stream's [`OutputGate`]: charged
/// when it is queued, credited back by the writer once it is on the wire. The
/// two sides must use this one measure, or a gate drifts until it throttles a
/// stream or drops an observer that has nothing queued.
pub(crate) fn gated_len(msg: &DaemonMsg) -> usize {
    match msg {
        DaemonMsg::Output(b) | DaemonMsg::Snapshot(b) => b.len(),
        // Lifted from the same PTY read as the output around them.
        DaemonMsg::Image(b) | DaemonMsg::ClipboardWrite(b) => b.len(),
        _ => 0,
    }
}

pub(super) struct Observer {
    pub(super) id: u64,
    pub(super) tx: Sender<DaemonMsg>,
    pub(super) gate: Arc<OutputGate>,
}

/// An observer showing the pane at its own size — a phone, typically, while
/// its user is away from the desk.
///
/// The pty runs at the observer's size and the observers see it; the
/// controller keeps its own grid, is told who holds the pane, and is the only
/// one that can end it early ([`LeaseRequest::TakeBack`]). What the controller
/// asks for meanwhile is kept, not applied, and is what the pane goes back to.
pub(super) struct SizeLease {
    pub(super) observer: u64,
    pub(super) by: String,
    pub(super) desk: WinSize,
}

pub(super) fn notify(st: &mut PaneState, msg: DaemonMsg) {
    if let Some(sub) = &st.subscriber {
        let _ = sub.send(msg.clone());
    }
    // Status traffic is charged the same budget as output. These messages are
    // small, but an agent pane emits AgentStatus often enough that an observer
    // which stopped draining would still queue without bound. Being over the
    // line already disqualifies it; the message is not sized individually.
    st.observers.retain(|obs| send_to_observer(obs, &msg, 0));
}

/// One message to an observer, within its budget. One past it is told why and
/// let go. Its sender was the stream's last, so the stream then ends, and the
/// client opens the pane again for a fresh replay instead of watching a
/// screen that will never move again.
fn send_to_observer(obs: &Observer, msg: &DaemonMsg, len: usize) -> bool {
    if obs.gate.queued_bytes() + len as i64 > OBSERVER_BUDGET {
        let _ = obs.tx.send(DaemonMsg::Error(OBSERVER_BEHIND.to_string()));
        return false;
    }
    if obs.tx.send(msg.clone()).is_err() {
        return false;
    }
    obs.gate.add(len);
    true
}

/// Apply a resize to the pane's shared state: seal the replay ring's segment
/// and tell every connected client the new geometry.
///
/// The controller's `Size` echo has to be sent while the state lock is held —
/// output is fanned out under the same lock, so the frame lands in the stream
/// after every byte produced at the old size and before the repaint the PTY
/// emits once it is actually resized (which happens after the lock is
/// released). A client that speaks `FEATURE_RESIZE_ECHO` defers its grid
/// reflow to this marker instead of reflowing the moment its window changed:
/// the channel can hold megabytes of old-width output (the pane gate allows
/// 16 MiB), and reflowing ahead of it parsed old-width bytes into a new-width
/// grid, garbling the pane on maximize during a burst of output.
///
/// The marker is deliberately not airtight. Old-width bytes the reader thread
/// has read but not yet fanned out — and bytes still sitting in the kernel
/// pipe, which conhost keeps producing until it processes the resize — land
/// after the echo and parse at the new width. That residue is bounded by one
/// 64 KiB read plus the pipe, cannot be attributed to a geometry from here
/// (the bytes carry no tags and ConPTY emits no sync marker of its own), and
/// is the same ambiguity every ConPTY terminal accepts on a mid-output
/// resize. The echo exists to close the 16 MiB window, not that one.
pub(super) fn resize_state(st: &mut PaneState, size: WinSize) {
    st.ring.resize(size);
    if let Some(sub) = &st.subscriber {
        let _ = sub.send(DaemonMsg::Size(size));
    }
    st.observers
        .retain(|obs| obs.tx.send(DaemonMsg::Size(size)).is_ok());
}

pub(super) fn tell_observer(st: &mut PaneState, observer: u64, msg: DaemonMsg) {
    if let Some(obs) = st.observers.iter().find(|obs| obs.id == observer) {
        let _ = obs.tx.send(msg);
    }
}

pub(super) fn tell_controller_lease(st: &PaneState) {
    if !st.lease_watch {
        return;
    }
    if let Some(sub) = &st.subscriber {
        let _ = sub.send(DaemonMsg::Lease(st.lease.as_ref().map(|l| l.by.clone())));
    }
}

/// An observer takes the pane at `size`, or changes the size it holds it at.
/// Returns the size to put the pty at, once the state lock is let go.
///
/// A second observer taking it over displaces the first, who is told, and the
/// size the desk asked for carries over, so ending the lease still restores
/// the desk rather than the first observer's size.
pub(super) fn lease_take(st: &mut PaneState, observer: u64, size: WinSize, by: String) -> Option<WinSize> {
    if !st.observers.iter().any(|obs| obs.id == observer) {
        return None;
    }
    let desk = match st.lease.take() {
        Some(prev) => {
            if prev.observer != observer {
                tell_observer(st, prev.observer, DaemonMsg::Lease(None));
            }
            prev.desk
        }
        None => st.ring.size(),
    };
    st.lease = Some(SizeLease {
        observer,
        by: by.clone(),
        desk,
    });
    tell_controller_lease(st);
    // The observers see the pane at the leased size, sealed into the ring so
    // a replay does too. The controller keeps its grid and gets no Size: a
    // client that is told its pane changed size resizes back to fit its
    // window, and the two would take turns.
    st.ring.resize(size);
    st.observers
        .retain(|obs| obs.tx.send(DaemonMsg::Size(size)).is_ok());
    tell_observer(st, observer, DaemonMsg::Lease(Some(by)));
    Some(size)
}

/// Ends the lease, whoever holds it, and puts the pane back at the size the
/// desk last asked for. Returns that size for the pty, or `None` if there was
/// no lease.
pub(super) fn lease_end(st: &mut PaneState) -> Option<WinSize> {
    let lease = st.lease.take()?;
    tell_observer(st, lease.observer, DaemonMsg::Lease(None));
    tell_controller_lease(st);
    resize_state(st, lease.desk);
    Some(lease.desk)
}

/// Ends the lease if `observer` holds it: it let go, or it went away.
pub(super) fn lease_release(st: &mut PaneState, observer: u64) -> Option<WinSize> {
    if st.lease.as_ref()?.observer != observer {
        return None;
    }
    lease_end(st)
}

/// The controller's resize. Under a lease it is only remembered, for when the
/// lease ends, and echoed to the controller alone so it reflows its own grid.
/// Returns the size for the pty if it is to change now.
pub(super) fn controller_resize(st: &mut PaneState, size: WinSize) -> Option<WinSize> {
    if let Some(lease) = &mut st.lease {
        lease.desk = size;
        if let Some(sub) = &st.subscriber {
            let _ = sub.send(DaemonMsg::Size(size));
        }
        return None;
    }
    resize_state(st, size);
    Some(size)
}

/// One gated message to the controller and to every observer.
///
/// A send error just means that client is gone; ignore it and let the next
/// attach install a new sender. Successful sends are counted against the
/// pane gate, and the connection's writer thread credits them back. Observers
/// each carry their own gate and their own budget: one that has stopped
/// draining is dropped rather than allowed to hold the pane's output forever.
fn fan_out_one(st: &mut PaneState, msg: DaemonMsg, len: usize, gate: &OutputGate) {
    if let Some(sub) = &st.subscriber {
        if sub.send(msg.clone()).is_ok() {
            gate.add(len);
        }
    }
    st.observers.retain(|obs| send_to_observer(obs, &msg, len));
}

/// Fan one PTY read out to the controller and every observer.
///
/// On the no-graphics fast path `frames` is empty and the whole passthrough
/// goes as a single `Output`. When a chunk carried graphics the frames are
/// forwarded in stream order instead, so an image lands at the same cursor cell
/// the sender drew it at. Each `Output` is gated on its own length; an image
/// frame is gated too (it rode the same PTY read); a delete is tiny and
/// ungated, matching the drain accounting in `server.rs`.
pub(super) fn fan_out_output(st: &mut PaneState, bytes: &[u8], frames: Vec<GraphicsFrame>, gate: &OutputGate) {
    if frames.is_empty() {
        if !bytes.is_empty() {
            fan_out_one(st, DaemonMsg::Output(bytes.to_vec()), bytes.len(), gate);
        }
        return;
    }
    for frame in frames {
        match frame {
            GraphicsFrame::Output(b) => {
                if !b.is_empty() {
                    let len = b.len();
                    fan_out_one(st, DaemonMsg::Output(b), len, gate);
                }
            }
            GraphicsFrame::Image(frame) => {
                let len = frame.len();
                fan_out_one(st, DaemonMsg::Image(frame), len, gate);
            }
            // Ungated, and `notify` already holds observers to their budget.
            GraphicsFrame::Delete(sel) => notify(st, DaemonMsg::DeleteImage(sel)),
            // Clipboard writes are side effects for the controlling GUI only.
            // Observers must never overwrite their own machine's clipboard just
            // because they are watching this pane.
            GraphicsFrame::ClipboardWrite(frame) => {
                let len = frame.len();
                if let Some(sub) = &st.subscriber
                    && sub.send(DaemonMsg::ClipboardWrite(frame)).is_ok()
                {
                    gate.add(len);
                }
            }
        }
    }
}

#[cfg(test)]
pub(super) fn attach_subscriber(st: &mut PaneState, subscriber: Sender<DaemonMsg>) -> u64 {
    attach_subscriber_with_permissions(st, subscriber, &OutputGate::new(), false, false)
}

/// The one place a pane's clipboard permission is decided. A pane that carries
/// its own spec keeps that spec's answer whatever the controller claims; every
/// other pane is exactly as permitted as the client controlling it says.
pub(super) fn set_clipboard_permission(st: &mut PaneState, controller_says: bool) {
    st.allow_remote_clipboard_write = st.clipboard_write_from_spec.unwrap_or(controller_says);
}

/// The new controller's gate starts from what its replay queued, under the
/// state lock: output fanned out after the lock is released is the new
/// stream's, and a reset done later would wipe its charge.
pub(super) fn attach_subscriber_with_permissions(
    st: &mut PaneState,
    subscriber: Sender<DaemonMsg>,
    gate: &OutputGate,
    allow_remote_clipboard_write: bool,
    foreground_command: bool,
) -> u64 {
    st.subscriber_epoch += 1;
    st.lease_watch = false;
    set_clipboard_permission(st, allow_remote_clipboard_write);
    gate.reset();
    gate.add(replay_state(st, &subscriber, foreground_command));
    st.subscriber = Some(subscriber);
    st.subscriber_epoch
}

pub(super) fn observe_subscriber(
    st: &mut PaneState,
    observer: Sender<DaemonMsg>,
    gate: Arc<OutputGate>,
    foreground_command: bool,
) -> u64 {
    st.observer_seq += 1;
    // The replay just queued the whole ring into this channel. Charge it, or
    // the first budget check would read zero while a full scrollback is already
    // sitting there unread — an observer that never drains would be allowed a
    // ring plus a full budget before anyone noticed. Its writer credits it back
    // as it goes out; until it did, a full ring alone was the whole budget, and
    // the first byte of live output dropped the observer for good.
    gate.add(replay_state(st, &observer, foreground_command));
    st.observers.push(Observer {
        id: st.observer_seq,
        tx: observer,
        gate,
    });
    st.observer_seq
}
