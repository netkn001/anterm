//! Early CLI argument parsing, single-instance path forwarding, and
//! external OS open/URL dispatch (`file:`, `ssh:`, `x-man-page:`).

use gpui::App;

fn strip_os_arg_prefix(arg: &std::ffi::OsStr, prefix: &str) -> Option<std::ffi::OsString> {
    let suffix = arg.as_encoded_bytes().strip_prefix(prefix.as_bytes())?;
    // SAFETY: `prefix` is ASCII and is removed only from the beginning of an
    // existing platform-encoded OsStr. ASCII bytes are self-synchronizing in
    // Windows WTF-8 and Unix byte strings, so the suffix keeps valid encoding.
    Some(unsafe { std::ffi::OsString::from_encoded_bytes_unchecked(suffix.to_vec()) })
}

fn config_dir_from(
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Option<std::path::PathBuf> {
    while let Some(arg) = args.next() {
        if let Some(path) = strip_os_arg_prefix(&arg, "--config-dir=") {
            return Some(path.into());
        }
        if arg == std::ffi::OsStr::new("--config-dir") {
            return args.next().map(Into::into);
        }
    }
    None
}

pub(crate) fn apply_config_dir_arg(args: &[std::ffi::OsString]) {
    if let Some(path) = config_dir_from(args.iter().cloned()) {
        crate::core::config::set_config_dir(path);
    }
}

pub(crate) fn open_path_from(
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Option<std::path::PathBuf> {
    while let Some(arg) = args.next() {
        if let Some(path) = strip_os_arg_prefix(&arg, "--open-path=") {
            return Some(path.into());
        }
        if arg == std::ffi::OsStr::new("--open-path") {
            return args.next().map(Into::into);
        }
    }
    None
}

/// The directory an installer is about to replace, from
/// `--stop-daemon --update-install-dir <dir>`. Meaningful only next to
/// `--stop-daemon`; the caller checks that flag first.
#[cfg(windows)]
pub(crate) fn update_install_dir_from(
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Option<std::path::PathBuf> {
    while let Some(arg) = args.next() {
        if arg == std::ffi::OsStr::new("--update-install-dir") {
            return args.next().map(Into::into);
        }
    }
    None
}

/// `Some(true)` to register the Explorer verbs, `Some(false)` to remove them.
pub(crate) fn explorer_menu_action_from(args: &[std::ffi::OsString]) -> Option<bool> {
    args.iter().find_map(|arg| match arg.as_os_str() {
        a if a == std::ffi::OsStr::new("--register-explorer-menu") => Some(true),
        a if a == std::ffi::OsStr::new("--unregister-explorer-menu") => Some(false),
        _ => None,
    })
}

/// Offers an explicit launch request to an already running local GUI.
///
/// The dispatcher is injected so the startup decision can be tested without
/// opening a real daemon connection. Only an explicit `Bool(true)` means the
/// request reached a GUI; every other response keeps the current app alive so
/// it can perform the normal first-window startup. A pathless request asks the
/// GUI to surface itself — restore its most recent workspace, or activate the
/// window it already has — which is what lets a second launch hand the tray
/// its window back instead of opening a second process.
///
/// `daemon_gone` reports whether the recorded daemon process is known dead.
/// The probe connects to that daemon's control listener, so a dead daemon
/// guarantees the request cannot reach a GUI — and the connect would only pay
/// the OS's refusal delay on the stale `control.port` that `ensure_running`
/// clears later. The probe is skipped instead of run.
fn forward_open_path_with(
    open_path: Option<&std::path::Path>,
    daemon_gone: impl FnOnce() -> bool,
    dispatch: impl FnOnce(Option<String>) -> std::io::Result<anterm_core::daemon::control::ReplyOk>,
) -> bool {
    use anterm_core::daemon::control::ReplyOk;

    if daemon_gone() {
        return false;
    }

    let wire_path = match open_path {
        None => None,
        Some(path) => {
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                match std::env::current_dir() {
                    Ok(current_dir) => current_dir.join(path),
                    Err(error) => {
                        log::debug!("could not resolve the relative GUI open path: {error}");
                        return false;
                    }
                }
            };

            // Keep the native PathBuf in this process. Returning false
            // continues normal startup, which opens the path without crossing
            // the protocol.
            let Some(wire_path) = path.to_str().map(str::to_owned) else {
                log::debug!("the explicit GUI open path is not valid UTF-8; opening it locally");
                return false;
            };
            Some(wire_path)
        }
    };

    match dispatch(wire_path) {
        Ok(ReplyOk::Bool(true)) => true,
        Ok(ReplyOk::Bool(false)) => false,
        Ok(other) => {
            log::debug!("the daemon answered the early GuiOpen request with {other:?}");
            false
        }
        Err(error) => {
            log::debug!("the early GuiOpen request was not delivered: {error}");
            false
        }
    }
}

/// Uses a transient host-RPC connection instead of a GUI connection.
///
/// A GUI connection is long-lived and registers itself as the daemon's event
/// target. This short probe must never replace the actual running GUI while it
/// asks that GUI to open the requested folder.
pub(crate) fn forward_open_path(open_path: Option<&std::path::Path>) -> bool {
    use anterm_core::client::ControlClient;
    use anterm_core::daemon::control::{ControlHello, ControlRequest};

    forward_open_path_with(
        open_path,
        crate::daemon::spawn::recorded_daemon_is_dead,
        |path| {
            let hello = ControlHello::host_rpc(
                format!("anterm-app-open-{}", std::process::id()),
                "this computer",
            );
            let client = ControlClient::connect(&hello)?;
            let reply = client.request(ControlRequest::GuiOpen { path });
            client.close();
            reply
        },
    )
}


/// Delivers Finder document opens and LaunchServices URL opens after gpui has
/// created the application. The native callback queues requests; UI state is
/// then changed on gpui's application loop.
pub(crate) fn handle_external_opens(urls: Vec<String>, cx: &mut App) {
    use crate::core::default_terminal::{ExternalOpen, parse_open_url};

    for raw in urls {
        let request = match parse_open_url(&raw) {
            Ok(request) => request,
            Err(error) => {
                log::warn!("ignored external open request {raw:?}: {error}");
                continue;
            }
        };
        match request {
            ExternalOpen::Folder(path) => crate::ui::workspace::windows::open_from_cli(cx, Some(path)),
            ExternalOpen::Runnable(path) => {
                let Some(parent) = path.parent().map(std::path::Path::to_path_buf) else {
                    log::warn!(
                        "ignored runnable without a parent directory: {}",
                        path.display()
                    );
                    continue;
                };
                let command =
                    crate::core::shell_quote::quote_for_shell(&path.to_string_lossy(), None);
                crate::ui::workspace::windows::run_local_command(cx, parent, command);
            }
            ExternalOpen::Ssh(ssh) => crate::ui::workspace::windows::quick_connect_from_url(cx, ssh),
            ExternalOpen::ManPage { section, page } => {
                let mut command = String::from("man");
                for argument in section.iter().chain(std::iter::once(&page)) {
                    command.push(' ');
                    command.push_str(&crate::core::shell_quote::quote_for_shell(argument, None));
                }
                crate::ui::workspace::windows::run_local_command(cx, std::env::temp_dir(), command);
            }
        }
    }
}


#[cfg(test)]
mod argument_tests {
    use super::{
        config_dir_from, explorer_menu_action_from, forward_open_path_with, open_path_from,
    };
    use std::ffi::OsString;
    use std::path::PathBuf;
    use anterm_core::daemon::control::ReplyOk;

    /// The installer passes exactly one of these; every other launch — above
    /// all a plain `--open-path` from the very menu they register — must fall
    /// through to normal startup instead of rewriting the registry and exiting.
    #[test]
    fn explorer_menu_flags_are_distinguished_and_otherwise_absent() {
        assert_eq!(
            explorer_menu_action_from(&[OsString::from("--register-explorer-menu")]),
            Some(true)
        );
        assert_eq!(
            explorer_menu_action_from(&[OsString::from("--unregister-explorer-menu")]),
            Some(false)
        );
        assert_eq!(
            explorer_menu_action_from(&[OsString::from("--open-path"), OsString::from("/work")]),
            None
        );
        assert_eq!(explorer_menu_action_from(&[]), None);
    }

    #[test]
    fn open_path_accepts_separate_and_equals_forms() {
        let separate = open_path_from(
            ["--config-dir", "/cfg", "--open-path", "/work"]
                .into_iter()
                .map(OsString::from),
        );
        assert_eq!(separate, Some(PathBuf::from("/work")));

        let equals = open_path_from([OsString::from("--open-path=C:\\work")].into_iter());
        assert_eq!(equals, Some(PathBuf::from("C:\\work")));
    }

    #[test]
    fn config_dir_accepts_native_separate_and_equals_forms() {
        let separate = config_dir_from(
            [OsString::from("--config-dir"), OsString::from("C:\\cfg")].into_iter(),
        );
        assert_eq!(separate, Some(PathBuf::from("C:\\cfg")));

        let equals = config_dir_from([OsString::from("--config-dir=C:\\cfg")].into_iter());
        assert_eq!(equals, Some(PathBuf::from("C:\\cfg")));
    }

    #[cfg(windows)]
    #[test]
    fn open_path_preserves_unpaired_utf16_from_windows_arguments() {
        use std::os::windows::ffi::OsStringExt as _;

        let native_path =
            OsString::from_wide(&[b'C' as u16, b':' as u16, b'\\' as u16, 0xD800, b'x' as u16]);
        let parsed =
            open_path_from([OsString::from("--open-path"), native_path.clone()].into_iter());
        assert_eq!(parsed, Some(PathBuf::from(native_path.clone())));

        let mut equals_arg = OsString::from("--open-path=");
        equals_arg.push(&native_path);
        assert_eq!(
            open_path_from([equals_arg].into_iter()),
            Some(PathBuf::from(native_path))
        );
    }

    #[test]
    fn a_pathless_forward_asks_the_gui_to_surface_and_exits_only_on_bool_true() {
        let delivered = forward_open_path_with(
            None,
            || false,
            |actual| {
                assert_eq!(actual, None, "a pathless request carries no wire path");
                Ok(ReplyOk::Bool(true))
            },
        );
        assert!(delivered, "a GUI accepted the surface request");

        assert!(!forward_open_path_with(
            None,
            || false,
            |_| Ok(ReplyOk::Bool(false))
        ));
        assert!(!forward_open_path_with(
            None,
            || false,
            |_| Ok(ReplyOk::Unit)
        ));
        assert!(!forward_open_path_with(
            None,
            || false,
            |_| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionRefused,
                    "daemon is not running",
                ))
            }
        ));
    }

    #[test]
    fn early_dispatch_is_skipped_when_the_recorded_daemon_is_dead() {
        // A dead recorded daemon cannot be routing for a registered GUI, so the
        // probe must not run — the connect would only wait out the OS's refusal
        // delay on the stale control port before `ensure_running` clears it.
        assert!(!forward_open_path_with(
            None,
            || true,
            |_| -> std::io::Result<ReplyOk> {
                panic!("the probe must not dispatch when the daemon is dead")
            },
        ));
        assert!(!forward_open_path_with(
            Some(std::path::Path::new("/work")),
            || true,
            |_| -> std::io::Result<ReplyOk> {
                panic!("the probe must not dispatch when the daemon is dead")
            },
        ));
    }

    #[test]
    fn early_dispatch_exits_only_after_a_gui_accepts_the_path() {
        let path = std::env::current_dir().unwrap().join("folder with spaces");
        let expected = path.to_str().unwrap().to_owned();
        let delivered = forward_open_path_with(
            Some(&path),
            || false,
            |actual| {
                assert_eq!(actual, Some(expected));
                Ok(ReplyOk::Bool(true))
            },
        );

        assert!(delivered);
        assert!(!forward_open_path_with(
            Some(&path),
            || false,
            |_| Ok(ReplyOk::Bool(false))
        ));
        assert!(!forward_open_path_with(
            Some(&path),
            || false,
            |_| Ok(ReplyOk::Unit)
        ));
        assert!(!forward_open_path_with(
            Some(&path),
            || false,
            |_| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionRefused,
                    "daemon is not running",
                ))
            }
        ));
    }

    #[test]
    fn early_dispatch_resolves_relative_paths_before_cross_process_delivery() {
        let relative = std::path::Path::new("workspace");
        let expected = std::env::current_dir()
            .unwrap()
            .join(relative)
            .to_str()
            .unwrap()
            .to_owned();

        assert!(forward_open_path_with(
            Some(relative),
            || false,
            |actual| {
                assert_eq!(actual, Some(expected));
                Ok(ReplyOk::Bool(true))
            }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn early_dispatch_keeps_non_utf8_paths_in_the_current_process() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt as _;

        let path = std::env::temp_dir().join(OsString::from_vec(b"anterm-\xff".to_vec()));
        let delivered = forward_open_path_with(
            Some(&path),
            || false,
            |_| panic!("a non-UTF-8 path must never be changed and sent over the string protocol"),
        );

        assert!(!delivered);
    }
}
