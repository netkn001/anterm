//! Test pane constructors gated behind `#[cfg(any(test, feature = "test-support"))]`.
#![allow(clippy::unwrap_used)]

use gpui::AppContext as _;

pub fn test_stream_pair() -> (
    anterm_core::daemon::transport::Stream,
    anterm_core::daemon::transport::Stream,
) {
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

pub fn quiet_test_pane(
    pane_id: u64,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> (gpui::Entity<super::TerminalView>, anterm_core::daemon::transport::Stream) {
    let (client_side, daemon_side) = test_stream_pair();
    let terminal = crate::remote::RemoteTerminal::from_stream(client_side, crate::TermSize::new(80, 24))
        .expect("quiet test terminal");
    let view = cx.new(|cx| super::TerminalView::with_terminal(terminal, pane_id, window, cx));
    (view, daemon_side)
}

/// The same pane, but reattached rather than spawned — what restoring last
/// session's tabs builds, and the only shape in which the daemon replays state
/// the pane already had.
pub fn quiet_reattached_test_pane(
    pane_id: u64,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> (gpui::Entity<super::TerminalView>, anterm_core::daemon::transport::Stream) {
    let (client_side, daemon_side) = test_stream_pair();
    let terminal = crate::remote::RemoteTerminal::from_stream_reattached(client_side, crate::TermSize::new(80, 24))
        .expect("quiet reattached test terminal");
    let view = cx.new(|cx| super::TerminalView::with_terminal(terminal, pane_id, window, cx));
    (view, daemon_side)
}

/// A quiet pane that was dialled by hand, with no saved host behind it.
///
/// Ungated on purpose: the transport this hands back is already
/// platform-neutral, and gating it left every test that wanted an SSH pane
/// silently skipped on Windows.
pub fn quiet_test_ssh_pane(
    pane_id: u64,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> (gpui::Entity<super::TerminalView>, anterm_core::daemon::transport::Stream) {
    quiet_test_ssh_pane_of(pane_id, None, window, cx)
}

/// The same, for a pane opened from a saved host — `profile_id` is what tells
/// the two apart everywhere the connection is offered back to the user.
pub fn quiet_test_ssh_pane_of(
    pane_id: u64,
    profile_id: Option<uuid::Uuid>,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> (gpui::Entity<super::TerminalView>, anterm_core::daemon::transport::Stream) {
    let mut spec: anterm_core::daemon::protocol::NativeSshSpec =
        serde_json::from_str(r#"{"host":"build-box","port":22,"user":"me","auth_mode":"auto"}"#)
            .expect("a minimal NativeSshSpec decodes");
    spec.profile_id = profile_id.map(|id| id.to_string());
    quiet_test_ssh_pane_with(pane_id, spec, window, cx)
}

/// The same again, over a spec the caller shaped — for everything a live
/// connection carries beyond its address.
pub fn quiet_test_ssh_pane_with(
    pane_id: u64,
    spec: anterm_core::daemon::protocol::NativeSshSpec,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> (gpui::Entity<super::TerminalView>, anterm_core::daemon::transport::Stream) {
    let (view, stream) = quiet_test_pane(pane_id, window, cx);
    view.update(cx, |view, _| {
        view.session.ssh_spec = Some(Box::new(spec));
    });
    (view, stream)
}

