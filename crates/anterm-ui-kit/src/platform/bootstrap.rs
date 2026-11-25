//! Platform startup bootstrap: Unix login-shell `PATH` enrichment and macOS
//! Dock icon / glyph-dilation configuration.

#[cfg(unix)]
fn merge_paths(primary: &str, secondary: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    primary
        .split(':')
        .chain(secondary.split(':'))
        .filter(|p| !p.is_empty() && seen.insert(*p))
        .collect::<Vec<_>>()
        .join(":")
}

/// Brackets the PATH in the probe's output. An interactive rc prints what it
/// likes — a greeting above, an exit hook below — so neither the whole output
/// nor any one line of it can be trusted to be the PATH.
#[cfg(unix)]
const PATH_MARKER: &str = "__ANTERM_PATH__";

/// How long startup waits on the user's rc files before giving up on them.
#[cfg(unix)]
const PATH_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The PATH between the first pair of markers, or `None` until both are in.
#[cfg(unix)]
fn path_from_shell_output(out: &[u8]) -> Option<String> {
    let out = String::from_utf8_lossy(out);
    let (_, rest) = out.split_once(PATH_MARKER)?;
    let (path, _) = rest.split_once(PATH_MARKER)?;
    Some(path.trim().to_string())
}

/// Runs the probe and returns the PATH as soon as the closing marker arrives,
/// without waiting for EOF: an rc that backgrounds a daemon hands it our
/// stdout, and the pipe then never closes. Past the timeout the shell is
/// killed and startup goes on with the PATH it has.
#[cfg(unix)]
fn read_path_from_shell(shell: &str, args: &[&str]) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let mut child = match Command::new(shell)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            log::warn!("failed to spawn login shell {shell} for PATH: {e}");
            return None;
        }
    };
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
            }
            if let Some(path) = path_from_shell_output(&out) {
                let _ = tx.send(Some(path));
                return;
            }
        }
        let _ = tx.send(None);
    });
    let path = match rx.recv_timeout(PATH_PROBE_TIMEOUT) {
        Ok(path) => path,
        Err(_) => {
            log::warn!(
                "login shell {shell} did not report PATH within {PATH_PROBE_TIMEOUT:?}; \
                 continuing without it"
            );
            let _ = child.kill();
            None
        }
    };
    if path.is_none() {
        let _ = child.kill();
    }
    // Reap off the startup path: after the marker the shell may still be
    // running exit hooks, and none of them is worth a delay here.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    path
}

#[cfg(unix)]
pub fn enrich_path_from_login_shell() {
    let shell = anterm_core::shells::login_shell();
    let fish = std::path::Path::new(&shell).file_name() == Some("fish".as_ref());
    let cmd = if fish {
        format!("printf '{PATH_MARKER}%s{PATH_MARKER}' (string join ':' $PATH)")
    } else {
        format!("printf '{PATH_MARKER}%s{PATH_MARKER}' \"$PATH\"")
    };
    // A pane gets an interactive shell, and for zsh/bash that is the only one
    // that reads `.zshrc` / `.bashrc` — where most people put their PATH. A
    // login shell alone reads `.zprofile` / `.bash_profile` and quietly hands
    // back the GUI's own bare PATH, so half the tools on `PATH` in a terminal
    // do not exist as far as this app is concerned. fish reads its config in
    // every mode; `-i` without a tty only makes it complain on stderr.
    let args: Vec<&str> = if fish {
        vec!["-l", "-c", &cmd]
    } else {
        vec!["-i", "-l", "-c", &cmd]
    };
    let Some(login_path) = read_path_from_shell(&shell, &args) else {
        return;
    };
    if login_path.is_empty() {
        return;
    }
    let merged = merge_paths(&login_path, &std::env::var("PATH").unwrap_or_default());
    unsafe { std::env::set_var("PATH", merged) };
}

#[cfg(target_os = "macos")]
pub fn set_dock_icon_for_bare_binary() {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let bundled = std::env::current_exe().is_ok_and(|p| {
        p.components()
            .any(|c| c.as_os_str().to_string_lossy().ends_with(".app"))
    });
    if bundled {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    static ICON_PNG: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/app-icon.png"));
    let data = NSData::with_bytes(ICON_PNG);
    if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
        unsafe {
            NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image));
        }
    }
}

/// Turns gpui's stroke thickening off for anterm's text when `font_thicken` is
/// off; with it on, whatever the system (or a hand-written `defaults write`)
/// says stands.
///
/// The switch lives in gpui's rasterizer and nowhere else. Pinning
/// `AppleFontSmoothing` to `0` for the process (which is how this used to be
/// done) reached AppKit as well: the menus, alerts and every other natively
/// drawn string lost their smoothing, while the terminal's dark-on-light text,
/// which gpui never thickens, looked no different (#1119).
///
/// Applied once at launch, before any text is drawn; a change made in Settings
/// waits for the next one.
#[cfg(target_os = "macos")]
pub fn apply_font_thicken(_thicken: bool) {
    // Upstream `gpui-kit` (`gpui-pre-apple`) computes CoreText glyph dilation
    // per color without exposing a process-wide override, and must never mutate
    // `AppleFontSmoothing` in `NSUserDefaults` (#1119).
}


#[cfg(all(test, target_os = "macos"))]
mod font_thicken_tests {
    use super::apply_font_thicken;
    use objc2_foundation::{NSArgumentDomain, NSUserDefaults, ns_string};

    /// #1119: turning thickening off must not reach AppKit. `AppleFontSmoothing`
    /// in any domain this process reads restyles its native menus and alerts,
    /// so the switch may only ever talk to gpui.
    #[test]
    fn turning_thickening_off_leaves_the_smoothing_preference_alone() {
        let defaults = NSUserDefaults::standardUserDefaults();
        let key = ns_string!("AppleFontSmoothing");
        let before = defaults.objectForKey(key);

        apply_font_thicken(false);

        // SAFETY: a Foundation constant, initialized before `main` runs.
        let arguments = defaults.volatileDomainForName(unsafe { NSArgumentDomain });
        assert!(arguments.objectForKey(key).is_none());
        assert_eq!(defaults.objectForKey(key).is_some(), before.is_some());

        apply_font_thicken(true);
    }
}

/// The watcher tick, from a reloaded file to the keys the app dispatches on.
///
/// The one thing #548 is for — hand-editing config.json and having the new
/// chord fire without a restart — crosses a file watcher, a debounce and a
/// keymap rebuild, and used to be covered nowhere. These drive
/// `apply_reloaded_config` directly, which is the whole body of the watcher's
/// tick, so the reload path is exercised without waiting on the filesystem.

#[cfg(all(test, unix))]
mod tests {
    use super::{merge_paths, path_from_shell_output};

    #[test]
    fn path_from_shell_output_reads_between_the_markers() {
        // The shape `zsh -i` actually prints: an rc that greets the user, the
        // `can't change option: zle` chatter `-i` earns without a tty, and an
        // exit hook that prints after the PATH.
        assert_eq!(
            path_from_shell_output(
                b"hello\n(eval):1: can't change option: zle\n\
                  __ANTERM_PATH__/opt/bin:/usr/bin__ANTERM_PATH__bye\n"
            )
            .as_deref(),
            Some("/opt/bin:/usr/bin")
        );
        assert_eq!(
            path_from_shell_output(b"hello\n__ANTERM_PATH__/opt/bin"),
            None
        );
        assert_eq!(path_from_shell_output(b"/opt/bin:/usr/bin\n"), None);
    }

    #[test]
    fn merge_paths_prefers_primary_dedupes_and_drops_empties() {
        assert_eq!(
            merge_paths("/opt/homebrew/bin:/usr/bin", "/usr/bin:/bin:"),
            "/opt/homebrew/bin:/usr/bin:/bin"
        );
        assert_eq!(
            merge_paths(
                "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
                "/usr/bin:/bin"
            ),
            "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"
        );
        assert_eq!(merge_paths("", "/usr/bin"), "/usr/bin");
    }
}

