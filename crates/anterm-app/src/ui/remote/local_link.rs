use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use gpui::{App, Global};
use anterm_core::daemon::control::{ControlClient, ControlEvent};

pub(crate) const PUMP_TICK: Duration = Duration::from_millis(250);
const RECONNECT_FIRST: Duration = Duration::from_secs(1);
const RECONNECT_CAP: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Backoff {
    attempt: u32,
}

impl Backoff {
    pub(crate) fn attempt(&self) -> u32 {
        self.attempt
    }

    pub(crate) fn delay(&self) -> Duration {
        let secs = 1u64.checked_shl(self.attempt).unwrap_or(u64::MAX);
        RECONNECT_FIRST
            .saturating_mul(u32::try_from(secs).unwrap_or(u32::MAX))
            .min(RECONNECT_CAP)
    }

    pub(crate) fn advance(&mut self) -> Duration {
        let delay = self.delay();
        self.attempt = self.attempt.saturating_add(1);
        delay
    }

    pub(crate) fn reset(&mut self) {
        self.attempt = 0;
    }
}

static EVENTS: Mutex<Vec<ControlEvent>> = Mutex::new(Vec::new());

pub(crate) fn install_event_observer() {
    anterm_core::daemon::control::set_event_observer(Arc::new(|_host, event| {
        if let Ok(mut queue) = EVENTS.lock() {
            queue.push(event);
        }
    }));
}

pub(crate) fn drain_events(cx: &mut App) {
    let events = match EVENTS.lock() {
        Ok(mut queue) => std::mem::take(&mut *queue),
        Err(_) => return,
    };
    for event in events {
        match event {
            ControlEvent::Layout { delta } => {
                crate::ui::remote::tree_sync::on_layout_delta(cx, delta);
            }
            ControlEvent::LayoutResync => {
                log::info!("local daemon dropped layout deltas for this client; re-pulling");
                crate::ui::remote::machine_mirror::MachineMirrors::refresh(cx);
                crate::ui::remote::tree_sync::resync_local_windows_from_tree(cx);
            }
            ControlEvent::GuiOpen { path } => {
                crate::ui::workspace::windows::open_from_cli(
                    cx,
                    path.map(std::path::PathBuf::from),
                );
            }
            other => log::debug!("unhandled control event: {other:?}"),
        }
    }
}

pub(crate) fn client_hostname() -> String {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        #[cfg(windows)]
        if let Some(name) = std::env::var_os("COMPUTERNAME") {
            let name = name.to_string_lossy().trim().to_string();
            if !name.is_empty() {
                return name;
            }
        }
        let mut cmd = std::process::Command::new("hostname");
        anterm_core::proc::hide_console(&mut cmd)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "a anterm client".to_string())
    })
    .clone()
}

#[derive(Default)]
pub struct LocalLink {
    client: Option<Arc<ControlClient>>,
    /// The daemon process the current link is talking to, by its hello
    /// instance — empty until the first connect records one. Survives
    /// `invalidate` on purpose: forgetting it there would make every
    /// reconnect a first sighting, and a daemon that came back as a
    /// *different* process would never be noticed (#553).
    instance: String,
    backoff: Backoff,
    next_attempt: Option<std::time::Instant>,
    attempting: bool,
    pumping: bool,
    shutting_down: bool,
}

impl Global for LocalLink {}

impl LocalLink {
    pub fn install(cx: &mut App) {
        install_event_observer();
        let link = cx.default_global::<LocalLink>();
        if link.pumping || link.shutting_down {
            return;
        }
        link.pumping = true;
        cx.spawn(async move |cx| {
            loop {
                let stop = cx.update(|cx| {
                    if cx.default_global::<LocalLink>().shutting_down {
                        return true;
                    }
                    Self::tick(cx);
                    drain_events(cx);
                    false
                });
                if stop {
                    break;
                }
                cx.background_executor().timer(PUMP_TICK).await;
            }
        })
        .detach();
    }

    pub fn client(cx: &mut App) -> Option<Arc<ControlClient>> {
        let link = cx.default_global::<LocalLink>();
        link.client.as_ref().filter(|c| c.is_connected()).cloned()
    }

    /// Whether the local daemon advertised `feature` on its control hello.
    /// `false` while the link is down.
    pub fn supports(cx: &App, feature: &str) -> bool {
        cx.try_global::<LocalLink>()
            .and_then(|link| link.client.as_ref())
            .is_some_and(|c| c.is_connected() && c.hello().has_feature(feature))
    }

    /// Stops reconnecting to the local daemon when the application is quitting.
    ///
    /// Without this, `LocalLink::tick` observes the daemon dying during
    /// `daemon::spawn::stop`, calls `ensure_running`, and spawns a fresh
    /// `--daemon` process right before the GUI exits.
    pub fn shutdown(cx: &mut App) {
        let link = cx.default_global::<LocalLink>();
        link.shutting_down = true;
        link.client = None;
        link.next_attempt = None;
    }

    /// Drops the cached client without waiting for its reader to notice.
    ///
    /// The remembered hello `instance` deliberately survives: the reconnect
    /// compares against it to tell "same daemon, link hiccuped" from "new
    /// daemon process" (#553).
    pub fn invalidate(cx: &mut App) {
        let link = cx.default_global::<LocalLink>();
        if link.client.take().is_some() {
            log::info!("dropped the control link to the local daemon; it was restarted");
        }
        link.backoff.reset();
        link.next_attempt = None;
    }

    fn tick(cx: &mut App) {
        let now = std::time::Instant::now();
        let link = cx.default_global::<LocalLink>();
        if link.shutting_down || link.attempting {
            return;
        }
        if let Some(client) = &link.client {
            if client.is_connected() {
                return;
            }
            log::info!("lost the control link to the local daemon; reconnecting");
            link.client = None;
        }
        match due(
            link.next_attempt,
            link.backoff.attempt(),
            link.backoff.delay(),
            now,
        ) {
            Due::Now => {}
            Due::Wait => return,
            Due::ScheduleAt(at) => {
                link.next_attempt = Some(at);
                return;
            }
        }
        link.next_attempt = None;
        link.attempting = true;
        let _ = link.backoff.advance();

        cx.spawn(async move |cx| {
            let connected = cx
                .background_executor()
                .spawn(async move { connect_blocking() })
                .await;
            cx.update(|cx| {
                let link = cx.default_global::<LocalLink>();
                link.attempting = false;
                if link.shutting_down {
                    return;
                }
                match connected {
                    Ok(client) => {
                        log::info!("control link to the local daemon is up");
                        let restarted = {
                            let link = cx.default_global::<LocalLink>();
                            crate::ui::remote::tree_sync::note_instance(
                                &mut link.instance,
                                &client.hello().instance,
                            )
                        };
                        let link = cx.default_global::<LocalLink>();
                        link.client = Some(client);
                        link.backoff.reset();
                        link.next_attempt = None;
                        crate::ui::remote::machine_mirror::MachineMirrors::refresh(cx);
                        if restarted {
                            crate::ui::remote::tree_sync::resync_local_windows_from_tree(cx);
                        } else {
                            crate::ui::remote::tree_sync::on_link_up(cx);
                        }
                    }
                    Err(e) => match dialect_refusal(&e) {
                        Some(refusal) => {
                            log::warn!(
                                "the local server refused this build's control dialect: {e}"
                            );
                            crate::daemon::spawn::note_daemon_mismatch(
                                crate::daemon::spawn::DaemonMismatch::Dialect(refusal),
                            );
                        }
                        None => log::debug!("local control link attempt failed: {e}"),
                    },
                }
            });
        })
        .detach();
    }
}

/// What a tick should do about reconnecting.
#[derive(Debug, PartialEq, Eq)]
enum Due {
    /// Try now.
    Now,
    /// Something is already scheduled and is not due yet.
    Wait,
    /// Nothing was scheduled; put the next attempt here and come back.
    ScheduleAt(std::time::Instant),
}

fn due(
    scheduled: Option<std::time::Instant>,
    attempts_so_far: u32,
    delay: Duration,
    now: std::time::Instant,
) -> Due {
    match scheduled {
        None if attempts_so_far == 0 => Due::Now,
        None => Due::ScheduleAt(now + delay),
        Some(at) if at > now => Due::Wait,
        Some(_) => Due::Now,
    }
}

fn connect_blocking() -> std::io::Result<Arc<ControlClient>> {
    use anterm_core::daemon::control::ControlHello;

    crate::daemon::spawn::ensure_running().map_err(std::io::Error::other)?;
    let hello = ControlHello::gui(uuid::Uuid::new_v4().to_string(), client_hostname());
    let sink: anterm_core::daemon::control::EventSink = Box::new(local_event_sink);
    #[cfg(unix)]
    let client = {
        let stream = std::os::unix::net::UnixStream::connect(
            anterm_core::host::server::control_socket_path()?,
        )?;
        anterm_core::daemon::transport::tune(&stream);
        ControlClient::over_unix(stream, &hello, sink)?
    };
    #[cfg(windows)]
    let client =
        ControlClient::over_tcp(anterm_core::host::server::connect_control()?, &hello, sink)?;
    Ok(Arc::new(client))
}

fn dialect_refusal(e: &std::io::Error) -> Option<anterm_core::daemon::control::DialectRefusal> {
    anterm_core::daemon::control::parse_dialect_refusal(&e.to_string())
}

fn local_event_sink(event: ControlEvent) {
    anterm_core::daemon::control::observe_event(anterm_core::host::HostId::LOCAL, event);
}

#[cfg(test)]
mod schedule_tests {
    use super::*;
    use std::time::{Duration, Instant};

    const D: Duration = Duration::from_secs(2);

    #[test]
    fn the_very_first_attempt_goes_out_immediately() {
        assert_eq!(due(None, 0, D, Instant::now()), Due::Now);
    }

    #[test]
    fn a_later_attempt_with_nothing_scheduled_gets_scheduled() {
        let now = Instant::now();
        assert_eq!(due(None, 1, D, now), Due::ScheduleAt(now + D));
        assert_eq!(
            due(None, 7, Duration::from_secs(30), now),
            Due::ScheduleAt(now + Duration::from_secs(30)),
            "the delay is the backoff's to decide, not this function's"
        );
    }

    #[test]
    fn a_scheduled_attempt_in_the_future_waits() {
        let now = Instant::now();
        assert_eq!(due(Some(now + D), 3, D, now), Due::Wait);
    }

    #[test]
    fn a_scheduled_attempt_that_has_come_due_fires() {
        let now = Instant::now();
        assert_eq!(due(Some(now - D), 3, D, now), Due::Now);
        assert_eq!(
            due(Some(now), 3, D, now),
            Due::Now,
            "exactly due counts as due, or a tick landing on the instant waits a whole round"
        );
    }

    #[test]
    fn ticking_repeatedly_does_not_move_a_pending_deadline() {
        let start = Instant::now();
        let at = start + D;
        for step in [0u64, 1, 100, 500] {
            let now = start + Duration::from_millis(step);
            assert_eq!(due(Some(at), 3, D, now), Due::Wait, "at +{step}ms");
        }
        assert_eq!(due(Some(at), 3, D, start + D), Due::Now);
    }

    #[gpui::test]
    fn shutdown_suppresses_reconnect_ticks(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            LocalLink::shutdown(cx);
            LocalLink::tick(cx);
            let link = cx.global::<LocalLink>();
            assert!(
                !link.attempting,
                "LocalLink::tick must not start a reconnect attempt after shutdown"
            );
            assert!(
                link.next_attempt.is_none(),
                "LocalLink::tick must not schedule a reconnect after shutdown"
            );
        });
    }
}
