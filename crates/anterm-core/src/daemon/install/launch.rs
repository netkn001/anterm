use std::time::Duration;

use crate::daemon::ssh::SshConnection;

use super::asset::{self, RemotePlatform};
use super::installer::{RemoteOps, RemoteStat};
use super::windows_host;

/// Finding the running server takes two shapes because `/proc` is a Linux
/// thing. On Linux, `/proc/<pid>/exe` is the honest answer: a symlink to the
/// file that is actually executing, whatever anyone did to `argv[0]`. macOS —
/// the only other machine [`asset::asset_for_uname`] will install onto — has no
/// `/proc` at all, so fall back to `ps`.
///
/// Its `comm` is not quite `exe`'s equal: on Darwin it reports `argv[0]`, so it
/// is a path only because [`launch_command`] launches by absolute path, and a
/// server started some other way would be invisible to it. It is still the
/// better of the two answers available there. Linux's `comm` is not an answer
/// at all — the name truncated to 15 characters, one short of
/// `anterm-server-c7p6` — which is why the fallback stays a fallback and `/proc`
/// keeps first refusal.
///
/// Neither arm reaches past the connecting user: `readlink` on another user's
/// `exe` is refused, and `ps` without `-A` lists only our own processes. The
/// pattern is anchored at a `/` so it cannot match a name that merely ends in
/// one of ours.
///
/// Only the daemon counts, which is why both arms look past the executable at
/// the command line. The same binary also runs as `--stdio --pane` bridges —
/// one per SSH pane link, for as long as the link is up — and as `--stdio`
/// control sessions. Taking the first `anterm-server-*` the loop met meant that
/// whenever a bridge sorted ahead of the daemon (`/proc/[0-9]*` expands
/// lexically, so a 7-digit pid or a wrapped one comes first) the restart sent
/// its SIGTERM to the bridge and then waited out its timeout on a daemon
/// nobody had signalled (#1181). The daemon is always launched as
/// `<binary> --daemon` ([`launch_command`]), by every dialect so far; on Linux
/// that is an exact argument in `cmdline`, and through `ps` the `args` of the
/// process have to be its `comm` followed by `--daemon`.
///
/// The `/proc` glob lives *inside* the `[ -d /proc ]` arm on purpose. The far
/// end runs these through the user's login shell, and zsh — the default on
/// macOS — aborts the whole command line when a glob matches nothing, so with
/// the loop at top level the trailing `true` never ran and every one of these
/// was a silent no-op on every Mac. That is what left `restart_daemon` waiting
/// out its ten seconds for a daemon nobody had asked to stop.
pub(super) const RUNNING_EXE_COMMAND: &str = r#"if [ -d /proc ]; then for p in /proc/[0-9]*; do e=$(readlink "$p/exe" 2>/dev/null) || continue; case "$e" in */anterm-server-*) tr '\000' '\n' 2>/dev/null <"$p/cmdline" | grep -qx -e --daemon || continue; printf '%s' "${e% (deleted)}"; break;; esac; done; else ps -xwwo pid=,comm= 2>/dev/null | while read -r pid e; do case "$e" in */anterm-server-*) case "$(ps -p "$pid" -wwo args= 2>/dev/null)" in "$e --daemon"|"$e --daemon "*) printf '%s' "$e"; break;; esac;; esac; done; fi; true"#;

/// Starts the line [`super::Installer::quick_prove`]'s script prints once the server
/// answered, so nothing a login script prints can pass for it.
pub(super) const QUICK_PROVE_MARK: &str = "anterm-quick-prove ";

/// [`RUNNING_EXE_COMMAND`]'s twin, signalling instead of printing — and
/// signalling *every* daemon it finds rather than stopping at the first, so
/// that an older dialect's daemon left running beside ours is stopped too.
pub(super) const TERMINATE_RUNNING_COMMAND: &str = r#"if [ -d /proc ]; then for p in /proc/[0-9]*; do e=$(readlink "$p/exe" 2>/dev/null) || continue; case "$e" in */anterm-server-*) tr '\000' '\n' 2>/dev/null <"$p/cmdline" | grep -qx -e --daemon && kill -TERM "${p#/proc/}" 2>/dev/null;; esac; done; else ps -xwwo pid=,comm= 2>/dev/null | while read -r pid e; do case "$e" in */anterm-server-*) case "$(ps -p "$pid" -wwo args= 2>/dev/null)" in "$e --daemon"|"$e --daemon "*) kill -TERM "$pid" 2>/dev/null;; esac;; esac; done; fi; true"#;

/// The two files a launch writes beside the server binary. Named here because
/// `is_first_install` reads the same directory and must not mistake one of
/// these for an installed server.
pub(crate) const STARTUP_LOG_SUFFIX: &str = ".startup.log";
pub(crate) const STARTUP_EXIT_SUFFIX: &str = ".startup.exit";

/// Where a launched daemon's output and exit status land on the far end.
///
/// One pair of files per binary, truncated at each launch rather than named
/// uniquely per attempt: these exist to be read seconds later by the client
/// that wrote them, and a unique name per launch would leave a file behind on
/// every reconnect for nobody to ever delete.
///
/// What makes the fixed name safe is the nonce. A restart tells the old daemon
/// to stop and the new one to start, and the old one's wrapper records its
/// status only once the old process is fully gone — which is after its socket
/// stopped answering, so it can land *after* the new launch truncated the file.
/// Reading that as the new daemon's status would fail a restart that is going
/// fine. The status is written with the nonce that asked for it, and a status
/// carrying anyone else's is not an answer to this launch.
pub(crate) struct StartupLog {
    pub(super) log: String,
    pub(super) exit: String,
    pub(super) nonce: String,
}

/// How much of the far end's startup log to fetch.
const TAIL_BYTES: usize = 4096;

/// How much of it reaches the error string. The whole tail goes to this
/// client's log; a status strip gets the last few lines, which is where the
/// reason always is.
const TAIL_LINES: usize = 4;

impl StartupLog {
    pub(crate) fn for_binary(binary: &str) -> Self {
        Self {
            log: format!("{binary}{STARTUP_LOG_SUFFIX}"),
            exit: format!("{binary}{STARTUP_EXIT_SUFFIX}"),
            nonce: uuid::Uuid::new_v4().simple().to_string(),
        }
    }

    /// The exit status the wrapper recorded for *this* launch, or `None` while
    /// the daemon is still running. Only the wrapper writes this file, and only
    /// once it has waited for the daemon, so a status carrying this launch's
    /// nonce is the death certificate.
    pub(super) fn exit_status(&self, ops: &dyn RemoteOps) -> Option<String> {
        let cmd = if asset::is_windows_sftp_path(&self.exit) {
            windows_host::read_exit_command(&self.exit)
        } else {
            format!("cat {} 2>/dev/null", shell_quote(&self.exit))
        };
        let out = ops.run(&cmd).ok()?;
        let (nonce, status) = out.stdout.trim().split_once(' ')?;
        (nonce == self.nonce && !status.is_empty()).then(|| status.to_string())
    }

    fn tail(&self, ops: &dyn RemoteOps) -> String {
        if asset::is_windows_sftp_path(&self.log) {
            let cmd = windows_host::tail_command(&self.log, TAIL_BYTES);
            return ops.run(&cmd).map(|out| out.stdout).unwrap_or_default();
        }
        let cmd = format!(
            "tail -c {TAIL_BYTES} {} 2>/dev/null",
            shell_quote(&self.log)
        );
        ops.run(&cmd).map(|out| out.stdout).unwrap_or_default()
    }

    /// Why the daemon never answered, in a sentence a status strip can show.
    ///
    /// The full tail goes to this client's log; the message keeps the last few
    /// lines of it, because the interesting one is always the last thing the
    /// server managed to say before it gave up.
    pub(super) fn explain(
        &self,
        ops: &dyn RemoteOps,
        binary: &str,
        probe: &str,
        exit: Option<String>,
        waited: Duration,
    ) -> String {
        let tail = self.tail(ops);
        if !tail.trim().is_empty() {
            log::warn!("remote {binary} startup log ({}):\n{tail}", self.log);
        }
        let what = match &exit {
            Some(status) => format!("{binary} exited with status {status} before it answered"),
            None => format!("{binary} was still not answering after {waited:?}"),
        };
        let mut out = format!("{what} on the control socket");
        if !probe.trim().is_empty() {
            out.push_str(&format!("; last probe said: {}", probe.trim()));
        }
        let said: Vec<&str> = tail
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .rev()
            .take(TAIL_LINES)
            .collect();
        if said.is_empty() {
            out.push_str(&format!("; it logged nothing to {}", self.log));
        } else {
            let said: Vec<&str> = said.into_iter().rev().collect();
            out.push_str(&format!(
                "; it said: {} (full log at {})",
                said.join(" / "),
                self.log
            ));
        }
        out
    }
}

/// Start the daemon detached, keeping what it says and how it ends.
///
/// The output used to go to `/dev/null`, which is why a remote that refused to
/// come up could only ever be reported as silence. Everything the server
/// prints on the way up — the control socket it bound, the listener it could
/// not bind, the other server already holding the single-server lock — is a
/// `startup_note!` on stderr, and all of it was being discarded.
pub(super) fn launch_command(binary: &str, log: &StartupLog) -> String {
    let bin = shell_quote(binary);
    let log_path = shell_quote(&log.log);
    let exit_path = shell_quote(&log.exit);
    let nonce = &log.nonce;
    // The wrapper outlives the daemon by one line: it waits, then records the
    // status, stamped with the nonce that asked for it.
    let supervised = shell_quote(&format!(
        "{bin} --daemon; s=$?; printf '%s %s' {nonce} \"$s\" > {exit_path}; exit \"$s\""
    ));
    // Three things this preamble has to get right, each of them a way to break
    // a machine that used to work:
    //
    // - The `umask` is scoped to the subshell. Left bare it would apply to the
    //   whole login shell, so the daemon would inherit it and so would every
    //   pane shell it forks — and a `git clone` in a remote pane would produce
    //   files nobody but the owner can read.
    // - Both files are removed and re-created here, under that umask, and only
    //   truncated by the wrapper. `>` on an existing file keeps whatever mode
    //   that file already had, so truncating alone would let one left at 0644 —
    //   by an older build, by anything — stay that way forever.
    // - A home that is full, read-only, or not ours must not stop the daemon
    //   from starting. If the files cannot be made, the redirect falls back to
    //   `/dev/null` and the launch goes ahead without diagnostics, which is
    //   exactly what it did before. The whole attempt sits inside a subshell so
    //   that a redirection failure cannot take the login shell down with it —
    //   on a POSIX shell a redirection error on the special builtin `:` ends a
    //   non-interactive shell outright, and `|| true` never gets to run.
    format!(
        "out={log_path}; \
         (umask 077; rm -f {log_path} {exit_path}; : > {log_path}; : > {exit_path}) 2>/dev/null \
           || out=/dev/null; \
         if command -v setsid >/dev/null 2>&1; then \
           setsid sh -c {supervised} < /dev/null >> \"$out\" 2>&1 & \
         else \
           nohup sh -c {supervised} < /dev/null >> \"$out\" 2>&1 & \
         fi"
    )
}

pub(super) fn launch_script(binary: &str, log: &StartupLog, settle: Option<String>) -> String {
    let launch = launch_command(binary, log);
    match settle {
        Some(settle) => format!("{launch}\n{settle}"),
        None => launch,
    }
}

pub(super) fn unique_temp(shared: &str) -> String {
    let pid = std::process::id();
    if let Some(stem) = shared.strip_suffix(".tmp.exe") {
        return format!("{stem}.{pid}.tmp.exe");
    }
    match shared.strip_suffix(".tmp") {
        Some(stem) => format!("{stem}.{pid}.tmp"),
        None => format!("{shared}.{pid}"),
    }
}

/// Whether a file SFTP describes can be run as the server. Unix wants the
/// owner's execute bit; Windows has none to want — an `.exe` runs because of
/// its name, and SFTP's mode bits there are a translation of ACLs that says
/// nothing reliable about it.
pub(super) fn runnable(platform: RemotePlatform, stat: RemoteStat) -> bool {
    !stat.is_dir
        && match platform {
            RemotePlatform::Unix => stat.mode & 0o100 != 0,
            RemotePlatform::Windows => true,
        }
}

pub(super) const MOVED_ASIDE_SUFFIX: &str = ".old";

/// Where a running Windows image is renamed to make room for its upgrade: a
/// hidden sibling, so [`super::Installer::is_first_install`] never counts it, and
/// unique per attempt: an image still running from an earlier move-aside
/// holds its name, and a second upgrade must not need it.
pub(super) fn moved_aside(binary: &str) -> String {
    let (dir, name) = binary.rsplit_once('/').unwrap_or(("", binary));
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("{dir}/.{name}.{}{MOVED_ASIDE_SUFFIX}", &id[..12])
}

/// The command a routed link runs on the remote to reach its server:
/// `'<binary>' --stdio` for a POSIX shell, `"<binary>" --stdio` for the
/// `cmd.exe` Windows OpenSSH runs commands through. See
/// [`windows_host::stdio_command`] for why not PowerShell.
pub fn server_stdio_command(binary: &str) -> String {
    if asset::is_windows_sftp_path(binary) {
        windows_host::stdio_command(binary)
    } else {
        format!("{} --stdio", shell_quote(binary))
    }
}

/// One word, read back byte for byte by a POSIX shell *and* by fish.
///
/// The two agree on single quotes except for the backslash: POSIX keeps it
/// literal, fish reads `\'` and `\\` inside quotes as escapes. Any backslash
/// that ends up next to a quote — which is every quote this function has
/// already escaped, once a quoted script is quoted again for `sh -c` — then
/// either swallows the closing quote or loses itself. So a backslash never sits
/// inside the quotes: it is closed out and written as `\\`, which both read as
/// one backslash.
pub(crate) fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\'' => out.push_str(r"'\''"),
            '\\' => out.push_str(r"'\\'"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// `script` as a command line any login shell hands to `sh`.
///
/// What `exec` sends over SSH is parsed by the user's login shell before
/// anything runs, and that shell need not be a POSIX one: on a machine whose
/// shell is fish, `out=…`, `if …; then` and `$(…)` are each a syntax error,
/// the command dies on its first line, and the caller is left waiting on a
/// daemon nobody started. Every script past a single plain command goes out
/// through this.
pub(crate) fn under_sh(script: &str) -> String {
    format!("sh -c {}", shell_quote(script))
}

pub(super) fn connection_label(conn: &SshConnection) -> String {
    conn.key().as_str().to_string()
}
