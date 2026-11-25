use std::io;
use std::time::{Duration, Instant};

use super::DaemonMismatch;
use crate::daemon::control::DialectRefusal;
use crate::daemon::protocol::{ClientMsg, DaemonMsg, DaemonVersion, PROTOCOL_VERSION};
use crate::daemon::transport;

// A version echo takes microseconds on a healthy daemon; a second is already
// generous. The old two-second budget only made an unresponsive daemon's
// stall that much longer before the bounded reap below takes over.
pub(super) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, PartialEq, Eq)]
pub(super) enum VersionProbe {
    Speaks(DaemonVersion),
    Legacy,
    Unresponsive,
}

/// What one version probe means, apart from the sockets it took to get it.
pub(super) struct ProbeVerdict {
    /// The daemon to remember for feature checks, where it named a version.
    pub(super) version: Option<DaemonVersion>,
    /// What the restart prompt should be holding afterwards.
    pub(super) mismatch: MismatchVerdict,
}

/// What a probe leaves the restart prompt holding.
///
/// Three outcomes rather than an `Option`, because "the daemon is ours" and
/// "the handshake never finished" are different answers that an `Option` spells
/// the same way. Only the first is evidence, and only evidence may wipe what an
/// earlier probe recorded.
pub(super) enum MismatchVerdict {
    /// Ours on both handshakes. Wipe whatever an earlier probe left.
    Clear,
    /// Put this to the user.
    Found(DaemonMismatch),
    /// Nothing conclusive came back — leave the record exactly as it stands.
    ///
    /// The asymmetry is deliberate. Leaving a stale record standing costs one
    /// prompt about a daemon that turned out to be fine, and the next probe a
    /// backoff later takes it away; clearing a live one costs a window that
    /// opens with no tabs and nothing on screen to explain why, which is the
    /// whole reason the record exists.
    Unchanged,
}

/// Read a probe's answer.
///
/// `None` means the daemon never answered, which is not a mismatch to put to
/// anyone: there is nothing to describe and nothing to decide. The caller
/// treats the endpoint as stale and reaps it.
///
/// The dialect is asked for through a closure because asking costs a second
/// connect, and it is only worth spending once the pane protocol has agreed —
/// a daemon that already fails on protocol is going to be restarted whatever
/// the other socket says.
pub(super) fn judge_probe(
    probe: VersionProbe,
    control_dialect: impl FnOnce() -> DialectAnswer,
) -> Option<ProbeVerdict> {
    match probe {
        // Agreeing here is only half the handshake: the control dialect is
        // versioned apart and moves on its own, so a daemon from before a
        // dialect bump passes this check and then refuses every machine-tree
        // call. Ask the other socket before calling the daemon ours, or the
        // window opens with no tabs and nothing to explain why.
        VersionProbe::Speaks(v) if v.protocol == PROTOCOL_VERSION => Some(ProbeVerdict {
            version: Some(v),
            mismatch: match control_dialect() {
                DialectAnswer::Agrees => MismatchVerdict::Clear,
                DialectAnswer::Refuses(refusal) => {
                    MismatchVerdict::Found(DaemonMismatch::Dialect(refusal))
                }
                // Half an answer decides nothing. The pane protocol agreeing
                // says nothing about the socket that stayed silent, and a
                // refusal recorded from a control link that already met this
                // daemon is worth more than our failure to ask it again.
                DialectAnswer::Silent => MismatchVerdict::Unchanged,
            },
        }),
        VersionProbe::Speaks(v) => Some(ProbeVerdict {
            version: Some(v.clone()),
            mismatch: MismatchVerdict::Found(DaemonMismatch::Protocol(Some(v))),
        }),
        VersionProbe::Legacy => Some(ProbeVerdict {
            version: None,
            mismatch: MismatchVerdict::Found(DaemonMismatch::Protocol(None)),
        }),
        VersionProbe::Unresponsive => None,
    }
}

/// What the daemon listening again after a handoff turned out to be.
pub(super) enum HandoffReturn {
    /// This build: the exec took.
    Replaced(DaemonVersion),
    /// It answers, but it is still the image we asked to go away.
    CarriedOn(DaemonVersion),
    /// It does not answer the version handshake at all, so there is no telling
    /// what came back.
    Mute,
}

pub(super) fn judge_handoff_return(probe: VersionProbe) -> HandoffReturn {
    match probe {
        VersionProbe::Speaks(v) if v.build == env!("CARGO_PKG_VERSION") => {
            HandoffReturn::Replaced(v)
        }
        VersionProbe::Speaks(v) => HandoffReturn::CarriedOn(v),
        VersionProbe::Legacy | VersionProbe::Unresponsive => HandoffReturn::Mute,
    }
}

pub(super) fn query_daemon_version(stream: &mut transport::Stream) -> VersionProbe {
    use std::io::Write as _;

    let started = Instant::now();
    let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    if let Err(e) = ClientMsg::Version
        .encode(stream)
        .and_then(|()| stream.flush())
    {
        log::warn!(
            "daemon version request failed after {:?}: {e}",
            started.elapsed()
        );
        return VersionProbe::Unresponsive;
    }
    match DaemonMsg::read(stream) {
        Ok(DaemonMsg::Version(v)) => VersionProbe::Speaks(v),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            ) =>
        {
            log::warn!(
                "daemon version reply timed out after {:?}: {e}",
                started.elapsed()
            );
            VersionProbe::Unresponsive
        }
        _ => VersionProbe::Legacy,
    }
}

/// What the control socket said when asked which dialect it speaks.
///
/// Three answers, not two. `Option<DialectRefusal>` spelled "it agrees" and "it
/// never answered" the same way, which was harmless while silence only meant
/// "record nothing" — and wrong the moment an agreeing probe started *clearing*
/// the record, because a control socket that timed out would then wipe a
/// refusal an earlier probe had recorded.
pub(super) enum DialectAnswer {
    /// It answered, with our own dialect number.
    Agrees,
    /// It answered, with a number that is not ours.
    Refuses(DialectRefusal),
    /// It did not answer: still coming up, unreachable, or gone before the
    /// hello came back. Nothing was learned.
    Silent,
}

/// The control dialect the running daemon speaks.
///
/// A control link would find this out on its own, but only after the first
/// window is already on screen and already empty. Asking here — one handshake
/// on a socket the daemon answers before it touches any state — is what lets
/// launch put the question to the user instead of a log line nobody reads.
///
/// Silence is not agreement: a daemon that is still coming up, or one whose
/// control socket is unreachable, answers nothing and is left alone.
pub(super) fn control_dialect_answer() -> DialectAnswer {
    #[cfg(unix)]
    let sock = match crate::host::server::control_socket_path()
        .ok()
        .and_then(|path| std::os::unix::net::UnixStream::connect(path).ok())
    {
        Some(sock) => sock,
        None => return DialectAnswer::Silent,
    };
    #[cfg(windows)]
    let sock = match crate::host::server::connect_control() {
        Ok(sock) => sock,
        Err(_) => return DialectAnswer::Silent,
    };

    if sock.set_read_timeout(Some(HANDSHAKE_TIMEOUT)).is_err() {
        return DialectAnswer::Silent;
    }
    dialect_of(&sock)
}

/// The dialect half of a control handshake, over a link already open.
pub(super) fn dialect_of<S: io::Read + io::Write>(mut link: S) -> DialectAnswer {
    use crate::daemon::control::{
        CONTROL_VERSION, ControlClientMsg, ControlHello, ControlServerMsg,
    };

    // Not `gui()`: this connection asks one question and hangs up, and a GUI
    // hello is a claim on a workspace.
    let hello =
        ControlHello::host_rpc(crate::daemon::protocol::process_instance(), "this computer");
    if ControlClientMsg::Hello(hello)
        .encode(&mut link)
        .and_then(|()| link.flush())
        .is_err()
    {
        return DialectAnswer::Silent;
    }
    match ControlServerMsg::read(&mut link) {
        Ok(ControlServerMsg::HelloOk(ok)) if ok.control_version != CONTROL_VERSION => {
            DialectAnswer::Refuses(DialectRefusal {
                peer_build: ok.build,
                peer: ok.control_version,
                ours: CONTROL_VERSION,
            })
        }
        Ok(ControlServerMsg::HelloOk(_)) => DialectAnswer::Agrees,
        // Anything else is a link that did not complete the handshake: a peer
        // that hung up, a reply we cannot parse, the read timing out.
        _ => DialectAnswer::Silent,
    }
}
