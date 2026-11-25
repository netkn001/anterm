use super::*;

use gpui::ClipboardEntry;
use gpui_kit::component::WindowExt as _;

use anterm_core::shell_quote::quote_for_shell;

/// Ring the platform's alert sound, reporting whether one was actually made —
/// `Audible` falls back to a flash when it wasn't, so a silent `false` is the
/// difference between "you heard the bell" and "you saw it instead".
///
/// Linux has no equivalent worth the dependency: the desktop sound APIs
/// (libcanberra, PipeWire) are a runtime link away and X11's `XBell` does
/// nothing under Wayland, so the flash fallback stays the answer there.
pub(super) fn ring_system_bell() -> bool {
    #[cfg(target_os = "macos")]
    {
        objc2_app_kit::NSBeep();
        true
    }
    #[cfg(target_os = "windows")]
    {
        // MB_OK is the "Default Beep" scheme entry, so this follows whatever
        // the user picked in Sound Settings — including "None", which is a
        // deliberate silence and still reports success. Deliberately not
        // `Beep()`, which synthesizes a fixed tone straight at the speaker and
        // ignores the scheme. Returns immediately; the sound plays async.
        use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
        use windows_sys::Win32::UI::WindowsAndMessaging::MB_OK;
        unsafe { MessageBeep(MB_OK) != 0 }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

pub(super) fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    anterm_core::paste::paste_bytes(text.as_bytes(), bracketed)
}

/// A line the shell can be handed byte for byte, as if it had been typed at its
/// own prompt.
///
/// Control characters are what rule a line out. Under bracketed paste the shell
/// inserts every byte literally; delivered raw, each one runs through the line
/// editor's binding table instead, and a Tab completes, a `^U` kills, a `^C`
/// abandons the line. ESC is already stripped upstream; `is_control` covers the
/// rest, embedded newlines included — a multi-line command still goes as one
/// paste, which is what [`submit_bytes`] was built for.
///
/// Printable keys are deliberately *not* excluded, and cannot be: a line editor
/// binding one is exactly the mechanism this exists to reach. fish binds space
/// to `expand-abbr`; zsh users bind `.` to `rationalise-dot` and quotes to
/// zsh-autopair. Reaching the first means reaching the others, which is why the
/// caller keeps pasted text away from this path entirely.
///
/// The length bound is a cost ceiling, not a correctness one, and it is a
/// policy dial rather than a discontinuity in the data. A paste lands in one
/// go; raw bytes make the shell's line editor redraw as it consumes them, so
/// the added latency is linear in length from the very first byte — measured on
/// a pty it rises perfectly smoothly, with no knee to hang a bound on.
///
/// What 512 buys is a ceiling on that latency. The worst configuration measured
/// is zsh with zsh-syntax-highlighting and zsh-autosuggestions, which both
/// re-run per keystroke: ~0.26 ms per byte, so +15 ms on a typical 60-byte
/// command and +126 ms at the bound. Bare zsh is ~0.012 ms per byte (+6 ms at
/// the bound), bash is free at every length, and fish — the shell #660 is about
/// — shows no penalty this harness can resolve. Halving the bound would halve
/// the worst case; the number is a judgement about how much latency a long
/// typed line may pay, not something the curve picks out.
pub(crate) fn types_cleanly(line: &str) -> bool {
    line.len() <= 512 && !line.chars().any(char::is_control)
}

/// Build the byte sequence that submits the local editor's buffer to the shell.
///
/// A plain single-line command the user *typed* goes raw, no paste markers: the
/// shell's own reader then sees the same bytes typing at its prompt would
/// produce, so its input-time expansions run — fish abbreviations (#660), zsh
/// `magic-space`, a readline macro on a printable key. Inside a bracketed paste
/// none of that fires; fish's expand-on-execute only reaches the token under
/// the cursor, so `j` expanded but `j build` ran literally.
///
/// `pasted` is what keeps that from rewriting text the user did not type. A
/// paste's contract is that what went in is what runs, and a fish user with
/// `abbr -a l 'ls -la'` pasting `l /tmp` from their notes must not get
/// `ls -la /tmp`. So a line that has carried clipboard content keeps the paste
/// framing whatever else is true of it — see [`CmdEditor::pasted`].
///
/// Multi-line and control characters keep it too. A multi-line command goes in
/// as one paste and one CR, so it costs one prompt cycle instead of one per
/// line — preexec, the user's precmd chain, a syntax-highlight pass over the
/// whole buffer — and zle keeps the embedded newlines in its buffer, so
/// backslash / open-quote continuation and heredocs still parse as one unit.
///
/// ESC is stripped either way (unlike the paste path): clipboard text carrying
/// its own `ESC[201~` could otherwise close the paste early and have the rest
/// run as typed input, and a raw ESC reaching zle is an editor command.
///
/// An empty buffer skips the markers: zsh's `bracketed-paste-magic` (which
/// oh-my-zsh turns on) errors on a paste with nothing between them.
pub(super) fn submit_bytes(line: &str, bracketed: bool, pasted: bool) -> Vec<u8> {
    let clean: String = line
        .replace("\r\n", "\n")
        .chars()
        .filter(|&c| c != '\x1b')
        .map(|c| if c == '\r' { '\n' } else { c })
        .collect();
    let framed = bracketed && !clean.is_empty() && (pasted || !types_cleanly(&clean));
    let mut bytes = paste_bytes(&clean, framed);
    bytes.push(b'\r');
    bytes
}

pub(super) fn trim_trailing_spaces(text: &str) -> String {
    text.split('\n')
        .map(|line| line.trim_end_matches([' ', '\t']))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The local files the clipboard carries — a Finder or Explorer Copy on a file
/// puts its path there, not its bytes.
pub(super) fn clipboard_paths(item: &ClipboardItem) -> Vec<std::path::PathBuf> {
    item.entries()
        .iter()
        .filter_map(|e| match e {
            ClipboardEntry::ExternalPaths(paths) => Some(paths.paths()),
            _ => None,
        })
        .flatten()
        .cloned()
        .collect()
}

/// Paths as one line of shell words, trailing space included so whatever is
/// typed next starts a word of its own.
pub(crate) fn pasted_paths_text(paths: &[String], shell: Option<&str>) -> String {
    let words: Vec<String> = paths.iter().map(|p| quote_for_shell(p, shell)).collect();
    format!("{} ", words.join(" "))
}

/// Whether the clipboard carries files rather than (only) text.
pub(crate) fn clipboard_has_paths(item: &ClipboardItem) -> bool {
    !clipboard_paths(item).is_empty()
}

pub(super) fn write_clipboard_image(img: &gpui::Image) -> Option<std::path::PathBuf> {
    use gpui::ImageFormat;
    let dir = std::env::temp_dir().join("anterm-clipboard");
    std::fs::create_dir_all(&dir).ok()?;
    let (ext, transcoded) = match img.format {
        ImageFormat::Png => ("png", None),
        ImageFormat::Jpeg => ("jpg", None),
        ImageFormat::Gif => ("gif", None),
        ImageFormat::Webp => ("webp", None),
        other => ("png", Some(transcode_to_png(&img.bytes, other)?)),
    };
    let data: &[u8] = transcoded.as_deref().unwrap_or(&img.bytes);
    let path = dir.join(format!("paste-{:016x}.{ext}", img.id));
    std::fs::write(&path, data).ok()?;
    Some(path)
}

/// The connection a paste should be uploaded over, when this pane runs on a
/// remote host reachable over the daemon's russh stack (a native SSH pane).
pub(super) fn remote_paste_spec<'a>(
    ssh_spec: Option<&'a anterm_core::daemon::protocol::NativeSshSpec>,
) -> Option<&'a anterm_core::daemon::protocol::NativeSshSpec> {
    ssh_spec
}

/// Whether this pane may hand a remote program's image to the system clipboard.
pub(super) fn allows_remote_clipboard_write(
    ssh_spec: Option<&anterm_core::daemon::protocol::NativeSshSpec>,
) -> bool {
    remote_paste_spec(ssh_spec).is_none_or(|spec| spec.remote_clipboard_write)
}

/// Whether a pane stages the clipboard image to a file instead of forwarding
/// SYN and letting the agent read the clipboard itself.
///
/// SYN only works because the agent shares a clipboard with the pane. That
/// holds for a local macOS pane — where it is the better path, carrying the
/// image at full fidelity — and nowhere else: off macOS Claude Code silently
/// drops raw screenshots (anthropics/claude-code#26679), and an agent in a
/// remote pane reads the clipboard of the host it runs on, which never holds
/// this machine's screenshot no matter what the local OS is.
pub(super) fn stages_clipboard_image(is_remote: bool) -> bool {
    cfg!(not(target_os = "macos")) || is_remote
}

/// The WSL view of a Windows path: `C:\x\y` becomes `/mnt/c/x/y`.
///
/// `None` for anything without a drive letter — a UNC temp directory has no
/// automount mapping, and `C:x` is drive-relative rather than absolute.
///
/// The `/mnt` prefix is WSL's default automount root, not a guaranteed one:
/// `[automount] root=` in `/etc/wsl.conf` can move it. Asking the distro
/// (`wslpath -u`) would be exact, but it is a round trip through the daemon on
/// a keystroke path, and a moved automount root is rare enough that a wrong
/// path — which the user sees, in their own line, before they send it — beats
/// making every paste wait on a subprocess.
pub(super) fn wsl_path(windows: &str) -> Option<String> {
    let mut chars = windows.chars();
    let drive = chars.next()?.to_ascii_lowercase();
    if !drive.is_ascii_alphabetic() || chars.next()? != ':' {
        return None;
    }
    if !matches!(chars.next()?, '\\' | '/') {
        return None;
    }
    Some(format!(
        "/mnt/{drive}/{}",
        chars.as_str().replace('\\', "/")
    ))
}

/// The Windows spelling of a WSL pane's POSIX cwd — [`wsl_path`]'s inverse,
/// for reading rather than writing: the distro's `\\wsl$` share is how a
/// local `read_dir` can list a directory this process cannot reach natively.
///
/// Everything stays on the share, `/mnt/<drive>` included. Mapping the
/// automount back to the drive letter would list faster, but an absolute
/// word completes against its *cwd's* path prefix (`resolve_dir` keeps the
/// prefix when a rooted word lands on it) — so a drive-spelled cwd would send
/// `ls /etc<Tab>` to `C:\etc` instead of the distro's `/etc`. One prefix,
/// one meaning. A distro name with a path separator cannot name a share.
pub(super) fn wsl_share_path(distro: &str, posix: &str) -> Option<std::path::PathBuf> {
    if distro.is_empty() || distro.contains(['\\', '/']) {
        return None;
    }
    let rest = posix.strip_prefix('/')?;
    Some(std::path::PathBuf::from(format!(
        r"\\wsl$\{distro}\{}",
        rest.replace('/', "\\")
    )))
}

/// The distro whose `\\wsl$` share holds a pane's filesystem, or `None` when
/// the pane is not a WSL one and Tab belongs to the shell.
pub(super) fn wsl_share_distro(
    remote: Option<&anterm_core::daemon::protocol::RemoteContext>,
    host_is_local: bool,
) -> Option<String> {
    let remote = remote?;
    (remote.kind == anterm_core::daemon::protocol::RemoteKind::Wsl && host_is_local)
        .then(|| remote.target.clone())
}

/// The directory the Files panel lists for a pane, spelled so the pane's
/// host can read it — `None` when no spelling can.
///
/// A cwd the host resolves natively is used as-is. Past that, only a pane
/// whose files sit in a WSL distro on this machine has another way in: its
/// POSIX cwd goes through the distro's `\\wsl$` share (#896 — handed to a
/// local `read_dir` raw, `/home/me` is `C:\home\me`, and the panel showed
/// "Could not be read"). A cwd that is already a Windows path came from the
/// Windows side of the pane and is readable as it stands. Anything else — a
/// shell ssh'd onward to a machine anterm has no link to — has no spelling
/// here, and rooting the tree at it could only list nothing or the wrong
/// machine's files.
pub(super) fn files_cwd(
    host_cwd: Option<std::path::PathBuf>,
    wsl_distro: Option<&str>,
    cwd: Option<std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    if host_cwd.is_some() {
        return host_cwd;
    }
    let distro = wsl_distro?;
    let cwd = cwd?;
    let spelled = cwd.to_string_lossy();
    match spelled.starts_with('/') {
        true => wsl_share_path(distro, &spelled),
        false => Some(cwd),
    }
}

/// A local path — a staged image, a copied or dropped file — as the pane's own
/// filesystem spells it.
///
/// A WSL pane shares this machine's disk but not its path syntax: an agent in
/// there reads `/mnt/c/…` and cannot open `C:\…` at all, which is why the
/// upload route skips WSL — there is nothing to copy, only a name to rewrite.
/// A path with no mapping falls back to the Windows one, which at least tells
/// the user where the file is.
pub(super) fn local_path_for_pane(local: &str, shares_localhost: bool) -> String {
    if shares_localhost {
        return wsl_path(local).unwrap_or_else(|| local.to_string());
    }
    local.to_string()
}

/// Staging pastes under the SSH user's own home keeps them out of the
/// world-writable `/tmp`, where any local account could pre-create the
/// directory, read what lands in it, or swap a pasted file for one of its own
/// before the pane's agent opens it.
pub(super) const REMOTE_CLIPBOARD_PATH: [&str; 3] = [".cache", "anterm", "clipboard"];

/// Owner-only, and *only* owner: a staging directory anyone else can enter is
/// one anyone else can read the pasted files out of.
pub(super) const REMOTE_CLIPBOARD_MODE: u32 = 0o700;

/// How long a paste stays staged on the remote. Long enough to outlive any
/// conversation that still refers to it; a week of pasted spreadsheets is
/// what would otherwise pile up in the user's home forever.
pub(super) const REMOTE_CLIPBOARD_TTL_SECS: u64 = 7 * 24 * 60 * 60;

/// A fresh name for one paste's own directory under the staging directory.
///
/// Each paste gets a directory so every file keeps its real name — which is
/// what the agent reads, and `report.xlsx` says far more than a hash — without
/// two pastes of the same name overwriting each other.
pub(super) fn paste_dir_name() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    format!("paste-{nanos:x}{n:x}")
}

/// Whether a staging entry has outlived [`REMOTE_CLIPBOARD_TTL_SECS`]. An
/// entry with no mtime (`0`) is left alone: its age is unknown, not great.
pub(super) fn staged_entry_expired(mtime: u64, now: u64) -> bool {
    mtime != 0 && now.saturating_sub(mtime) > REMOTE_CLIPBOARD_TTL_SECS
}

/// Remove the staging directory's expired entries. Best effort: a failure
/// here costs disk space, never a paste, so nothing is reported.
fn prune_remote_clipboard_dir(route: &anterm_ui_kit::daemon_rpc::SftpRoute, dir: &str) {
    let Ok(entries) = route.list(dir) else {
        return;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    for entry in entries {
        if entry.name == "." || entry.name == ".." || !staged_entry_expired(entry.mtime, now) {
            continue;
        }
        remove_remote_tree(
            route,
            &anterm_core::daemon::ssh::sftp::remote_join(dir, &entry.name),
            entry.kind,
            0,
        );
    }
}

/// Delete a remote file or directory tree. A symlink is removed as a link and
/// never followed — the tree is ours, where it points is not.
fn remove_remote_tree(
    route: &anterm_ui_kit::daemon_rpc::SftpRoute,
    path: &str,
    kind: anterm_core::daemon::protocol::SftpEntryKind,
    depth: usize,
) {
    use anterm_core::daemon::protocol::{SftpEntryKind, SftpOp};
    // A pasted folder is as deep as it was on disk; this bounds a pathological
    // one rather than any real one.
    const MAX_DEPTH: usize = 64;
    if kind != SftpEntryKind::Dir {
        route.op(SftpOp::RemoveFile {
            path: path.to_string(),
        });
        return;
    }
    if depth < MAX_DEPTH
        && let Ok(children) = route.list(path)
    {
        for child in children {
            if child.name == "." || child.name == ".." {
                continue;
            }
            let child_path = anterm_core::daemon::ssh::sftp::remote_join(path, &child.name);
            remove_remote_tree(route, &child_path, child.kind, depth + 1);
        }
    }
    route.op(SftpOp::RemoveDir {
        path: path.to_string(),
    });
}

/// One local path handed to the remote: where it went and the transfer
/// carrying it, or why it never left.
struct PasteUpload {
    local: std::path::PathBuf,
    started: Result<StartedUpload, String>,
}

struct StartedUpload {
    remote: String,
    job: u64,
    is_dir: bool,
}

/// Start uploading `sources` into a new directory of their own under the
/// staging directory `dir`. Blocking, like everything that talks to the
/// daemon, so this runs off the UI thread. `Err` means the paste's directory
/// could not be made and nothing was started; a source that could not be
/// started on its own is reported in its own entry.
fn start_paste_uploads(
    route: &anterm_ui_kit::daemon_rpc::SftpRoute,
    pane_id: u64,
    dir: &str,
    sources: &[std::path::PathBuf],
) -> Result<Vec<PasteUpload>, String> {
    use anterm_core::daemon::protocol::{SftpOp, SftpOpResult, SftpTransferKind, SftpTransferSpec};
    let paste_dir = anterm_core::daemon::ssh::sftp::remote_join(dir, &paste_dir_name());
    if let SftpOpResult::Error(e) = route.op(SftpOp::Mkdir {
        path: paste_dir.clone(),
    }) {
        return Err(e);
    }
    Ok(sources
        .iter()
        .map(|local| {
            let started = (|| {
                let name = local
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .filter(|n| anterm_core::daemon::ssh::sftp::safe_local_name(n))
                    .ok_or_else(|| format!("{} has no file name", local.display()))?;
                let is_dir = std::fs::metadata(local)
                    .map_err(|e| format!("{}: {e}", local.display()))?
                    .is_dir();
                let remote = anterm_core::daemon::ssh::sftp::remote_join(&paste_dir, &name);
                let job = route.transfer_start(SftpTransferSpec {
                    pane_id,
                    kind: SftpTransferKind::Upload,
                    local: local.clone(),
                    remote: remote.clone(),
                    recursive: is_dir,
                })?;
                Ok(StartedUpload {
                    remote,
                    job,
                    is_dir,
                })
            })();
            PasteUpload {
                local: local.clone(),
                started,
            }
        })
        .collect())
}

/// Whether a prepared staging directory may be uploaded into.
///
/// The mode is what a `stat` reported *after* a `chmod 0700` the daemon
/// watched succeed, which is the ownership proof: POSIX only lets a file's
/// owner change its mode, so a directory anterm can chmod and then observe at
/// exactly `0700` is one the SSH user owns and nobody else can enter. A
/// symlink is refused outright because `stat` follows links, so a link planted
/// at the staging path would otherwise be judged by its target.
pub(super) fn staging_dir_is_safe(
    is_symlink: bool,
    kind: Option<anterm_core::daemon::protocol::SftpEntryKind>,
    mode: u32,
) -> bool {
    use anterm_core::daemon::protocol::SftpEntryKind;
    !is_symlink
        && matches!(kind, Some(SftpEntryKind::Dir))
        && mode & 0o7777 == REMOTE_CLIPBOARD_MODE
}

/// The staging directory to reuse on the next paste. Only a verified directory
/// is cached: a preparation that failed — a dropped link, a squatted path, a
/// remote with no POSIX `/home` — must be retried rather than latched, or
/// every later paste emits a remote path for a directory that was never
/// created.
pub(super) fn staging_cache(prepared: &Result<String, String>) -> Option<String> {
    prepared.as_ref().ok().cloned()
}

/// Create and verify the per-user staging directory, answering the absolute
/// remote path to upload into. Blocking: every step is a daemon round trip
/// over the pane's SSH connection, so this only ever runs off the UI thread.
fn prepare_remote_clipboard_dir(route: &anterm_ui_kit::daemon_rpc::SftpRoute) -> Result<String, String> {
    use anterm_core::daemon::protocol::{SftpOp, SftpOpResult};
    let home = match route.op(SftpOp::Realpath {
        path: ".".to_string(),
    }) {
        SftpOpResult::Link(home) if home.starts_with('/') => home,
        SftpOpResult::Error(e) => return Err(e),
        other => {
            return Err(format!(
                "the remote home directory is not a path: {other:?}"
            ));
        }
    };
    let mut dir = home;
    for component in REMOTE_CLIPBOARD_PATH {
        dir = anterm_core::daemon::ssh::sftp::remote_join(&dir, component);
        // An existing directory fails here with EEXIST; the checks below are
        // what decide whether this one is ours, so the result carries no
        // information worth branching on.
        let _ = route.op(SftpOp::Mkdir { path: dir.clone() });
    }
    if let SftpOpResult::Link(target) = route.op(SftpOp::Readlink { path: dir.clone() }) {
        return Err(format!("{dir} is a symlink to {target}"));
    }
    if let SftpOpResult::Error(e) = route.op(SftpOp::Chmod {
        path: dir.clone(),
        mode: REMOTE_CLIPBOARD_MODE,
    }) {
        return Err(format!("{dir} is not owned by this session: {e}"));
    }
    match route.op(SftpOp::Stat { path: dir.clone() }) {
        SftpOpResult::Stat(entry)
            if staging_dir_is_safe(false, Some(entry.kind), entry.permissions) =>
        {
            Ok(dir)
        }
        SftpOpResult::Stat(entry) => Err(format!(
            "{dir} is not a private directory (mode {:o})",
            entry.permissions & 0o7777
        )),
        SftpOpResult::Error(e) => Err(e),
        other => Err(format!("unexpected reply for {dir}: {other:?}")),
    }
}

fn transcode_to_png(bytes: &[u8], format: gpui::ImageFormat) -> Option<Vec<u8>> {
    use gpui::ImageFormat as G;
    let src = match format {
        G::Png => image::ImageFormat::Png,
        G::Jpeg => image::ImageFormat::Jpeg,
        G::Webp => image::ImageFormat::WebP,
        G::Gif => image::ImageFormat::Gif,
        G::Bmp => image::ImageFormat::Bmp,
        G::Tiff => image::ImageFormat::Tiff,
        G::Ico => image::ImageFormat::Ico,
        G::Pnm => image::ImageFormat::Pnm,
        G::Svg => return None,
    };
    let decoded = image::load_from_memory_with_format(bytes, src).ok()?;
    let mut out = Vec::new();
    decoded
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

pub(super) fn validate_remote_clipboard_image(
    write: anterm_core::clipboard::ClipboardWrite,
) -> Result<(gpui::Image, Option<String>), String> {
    let (gpui_format, image_format) = match write.mime.as_str() {
        "image/png" => (gpui::ImageFormat::Png, image::ImageFormat::Png),
        "image/jpeg" | "image/jpg" => (gpui::ImageFormat::Jpeg, image::ImageFormat::Jpeg),
        "image/gif" => (gpui::ImageFormat::Gif, image::ImageFormat::Gif),
        "image/webp" => (gpui::ImageFormat::Webp, image::ImageFormat::WebP),
        _ => return Err("unsupported image MIME type".into()),
    };
    if image::guess_format(&write.data).ok() != Some(image_format) {
        return Err("image signature does not match its MIME type".into());
    }

    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(&write.data), image_format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(256 << 20);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|e| format!("invalid or oversized image: {e}"))?;

    Ok((gpui::Image::from_bytes(gpui_format, write.data), write.id))
}

impl TerminalView {
    pub(super) fn flash_bell(&mut self, cx: &mut Context<Self>) {
        self.render.bell_epoch += 1;
        let epoch = self.render.bell_epoch;
        self.render.bell_flash = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(150))
                .await;
            let _ = this.update(cx, |view, cx| {
                // A bell rung since this one owns the flash now. Holding
                // Backspace on an empty bash prompt rings at key-repeat rate,
                // and clearing here would blank it every few frames (#874).
                if view.render.bell_epoch != epoch {
                    return;
                }
                view.render.bell_flash = false;
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn drop_files(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        self.paste_local_paths(paths.paths().to_vec(), cx);
    }

    /// A row dragged out of a remote Files tree names a file on that remote,
    /// not here: there is nothing to upload, and the path is pasted as the
    /// tree spells it.
    pub(crate) fn drop_remote_path(
        &mut self,
        drag: &anterm_ui_kit::panel_tokens::RemotePathDrag,
        cx: &mut Context<Self>,
    ) {
        self.paste_paths(vec![drag.path.to_string_lossy().into_owned()], cx);
    }

    /// Paste paths already spelled the way the pane's host reads them: into
    /// the composer as attachments when it has the keyboard, and as shell
    /// words onto the line otherwise. Every route that turns files into
    /// something to paste — a drop, a copied file, a screenshot, an upload to
    /// a remote pane — ends here.
    fn paste_paths(&mut self, spelled: Vec<String>, cx: &mut Context<Self>) {
        let Some(spelled) = self.composer_takes_paths(spelled, cx) else {
            return;
        };
        let text = pasted_paths_text(&spelled, self.shell_program().as_deref());
        self.paste(text, cx);
    }

    /// Paste files that live on this machine so the pane's program can open
    /// them: uploaded first when the pane runs on an SSH host, renamed for a
    /// WSL pane, and pasted as they are everywhere else.
    pub(crate) fn paste_local_paths(
        &mut self,
        paths: Vec<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        // Every step of an upload is a blocking daemon round trip, which a
        // keystroke handler must not make, so a remote pane pastes from a
        // background task and this returns without touching the line.
        if self.upload_for_remote(paths.clone(), cx) {
            return;
        }
        let spelled: Vec<String> = paths
            .iter()
            .map(|p| local_path_for_pane(&p.to_string_lossy(), false))
            .collect();
        self.paste_paths(spelled, cx);
    }

    pub(super) fn paste_clipboard_image(&mut self, img: &gpui::Image, cx: &mut Context<Self>) {
        // A screenshot pasted into the composer is one of its attachments,
        // shown as a chip like a copied file. Forwarded as SYN it would land
        // in the agent's own input, hidden under the box.
        if self.composer_takes_files()
            && let Some(path) = write_clipboard_image(img)
        {
            self.paste_local_paths(vec![path], cx);
            return;
        }
        if self.paste_clipboard_image_as_path(img, cx) {
            return;
        }
        self.terminal.write(vec![0x16]);
        self.terminal.term.lock().selection = None;
        cx.notify();
    }

    /// Stage the clipboard image and paste a path for it, answering whether
    /// this took the paste over. `false` leaves the caller forwarding SYN —
    /// see [`stages_clipboard_image`] for when that is the better path.
    fn paste_clipboard_image_as_path(&mut self, img: &gpui::Image, cx: &mut Context<Self>) -> bool {
        let is_remote = remote_paste_spec(self.session.ssh_spec.as_deref()).is_some();
        if !stages_clipboard_image(is_remote) {
            return false;
        }
        let Some(path) = write_clipboard_image(img) else {
            return false;
        };
        // A macOS pane only gets here when it is remote — a local one returned
        // above — so pasting a path is right on every platform, and staying
        // silent would be the very no-op this route exists to avoid.
        self.paste_local_paths(vec![path], cx);
        true
    }

    /// Upload local files to the pane's remote host and paste their remote
    /// paths, all off the UI thread. Answers whether this pane took the paste
    /// over; `false` means a local or spec-less pane the caller should
    /// paste the local paths for.
    ///
    /// The paste lands as soon as the transfers start — Ctrl+V must not stall
    /// on the wire — and each file appears at its path only once it is whole,
    /// since the upload writes a temporary name and renames it into place. The
    /// transfers are watched to the end, and a failure warns the user that the
    /// path they were handed is dangling.
    fn upload_for_remote(
        &mut self,
        sources: Vec<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(spec) = remote_paste_spec(self.session.ssh_spec.as_deref()) else {
            return false;
        };
        let host = format!("{}@{}", spec.user, spec.host);
        let route = anterm_ui_kit::daemon_rpc::SftpRoute::new(self.pane_id);
        let cached = self.session.remote_clipboard_dir.clone();
        let pane_id = self.pane_id;
        cx.spawn(async move |this, cx| {
            let prepared = match cached {
                Some(dir) => Ok(dir),
                None => {
                    let route = route.clone();
                    cx.background_spawn(async move {
                        let prepared = prepare_remote_clipboard_dir(&route);
                        // Only on the pane's first paste, which is the only
                        // time the directory is prepared: one listing, cheap
                        // next to the upload that follows it.
                        if let Ok(dir) = &prepared {
                            prune_remote_clipboard_dir(&route, dir);
                        }
                        prepared
                    })
                    .await
                }
            };
            let dir = match this.update(cx, |view, _| {
                view.session.remote_clipboard_dir = staging_cache(&prepared);
                view.session.remote_clipboard_dir.clone()
            }) {
                Ok(Some(dir)) => dir,
                Ok(None) => {
                    let reason = prepared.unwrap_or_else(|e| e);
                    Self::paste_local_fallback(&this, cx, &sources, &host, &reason);
                    return;
                }
                Err(_) => return,
            };
            let started = {
                let (route, sources) = (route.clone(), sources.clone());
                cx.background_spawn(
                    async move { start_paste_uploads(&route, pane_id, &dir, &sources) },
                )
                .await
            };
            let uploads = match started {
                Ok(uploads) => uploads,
                Err(reason) => {
                    Self::paste_local_fallback(&this, cx, &sources, &host, &reason);
                    return;
                }
            };
            // A source that never left is pasted by its local path, so the
            // line still says which file was meant.
            let spelled: Vec<String> = uploads
                .iter()
                .map(|u| match &u.started {
                    Ok(s) => s.remote.clone(),
                    Err(_) => u.local.to_string_lossy().into_owned(),
                })
                .collect();
            let pasted = this.update_in(cx, |view, window, cx| {
                view.paste_paths(spelled, cx);
                for u in &uploads {
                    if let Err(reason) = &u.started {
                        view.warn_paste_upload_failed(&u.local, &host, reason, window, cx);
                    }
                }
            });
            if pasted.is_err() {
                return;
            }
            for u in uploads {
                let Ok(started) = u.started else { continue };
                if let Err(reason) = Self::watch_upload(route.clone(), &started, cx).await {
                    let _ = this.update_in(cx, |view, window, cx| {
                        view.warn_paste_upload_failed(&u.local, &host, &reason, window, cx);
                    });
                }
            }
        })
        .detach();
        true
    }

    /// Fall back to the local paths when nothing could be staged on the remote
    /// — the paste is never dropped — and say why they are local.
    fn paste_local_fallback(
        this: &gpui::WeakEntity<Self>,
        cx: &mut gpui::AsyncApp,
        sources: &[std::path::PathBuf],
        host: &str,
        reason: &str,
    ) {
        let spelled: Vec<String> = sources
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let _ = this.update_in(cx, |view, window, cx| {
            view.paste_paths(spelled, cx);
            if let Some(first) = sources.first() {
                view.warn_paste_upload_failed(first, host, reason, window, cx);
            }
        });
    }

    /// Poll a started upload to a terminal state. The transfer history the
    /// SFTP panel reads is only polled while that panel is open, and the
    /// daemon drops finished jobs after 30s, so a paste that no one is
    /// watching would otherwise fail in silence.
    async fn watch_upload(
        route: anterm_ui_kit::daemon_rpc::SftpRoute,
        upload: &StartedUpload,
        cx: &mut gpui::AsyncApp,
    ) -> Result<(), String> {
        use anterm_core::daemon::protocol::{SftpJobState, SftpOp};
        const POLL: std::time::Duration = std::time::Duration::from_millis(500);
        // A pasted file can be any size, so there is no deadline on the whole
        // transfer — only on one that has stopped moving, which is wedged
        // rather than slow.
        const STALLED_POLLS: usize = 600;
        let mut last_done = 0;
        let mut still = 0;
        while still < STALLED_POLLS {
            cx.background_executor().timer(POLL).await;
            let listed = {
                let route = route.clone();
                cx.background_spawn(async move { route.transfer_list() })
                    .await
            };
            // A poll that failed says nothing about the job — keep asking
            // until it answers or the budget above runs out.
            let Ok(listed) = listed else {
                still += 1;
                continue;
            };
            let Some(progress) = listed.into_iter().find(|j| j.job_id == upload.job) else {
                // Pruned after the retention window, or the daemon restarted:
                // there is nothing left to report either way.
                return Ok(());
            };
            match progress.state {
                SftpJobState::Running => {
                    still = if progress.bytes_done == last_done {
                        still + 1
                    } else {
                        0
                    };
                    last_done = progress.bytes_done;
                }
                SftpJobState::Done => {
                    // The staging directory is already owner-only, so this is
                    // belt and braces against a wider umask on the remote. A
                    // folder is left as it was: its contents are behind the
                    // same owner-only directory, and its own layout is the
                    // user's.
                    if !upload.is_dir {
                        let (route, path) = (route.clone(), upload.remote.clone());
                        cx.background_spawn(async move {
                            route.op(SftpOp::Chmod { path, mode: 0o600 })
                        })
                        .await;
                    }
                    return Ok(());
                }
                SftpJobState::Cancelled => return Ok(()),
                SftpJobState::Error => {
                    return Err(progress.error.unwrap_or_else(|| "upload failed".into()));
                }
            }
        }
        Ok(())
    }

    /// One notification per failed file — the pane's line already has a path
    /// in it, and the user is the only one who can tell whether it matters.
    fn warn_paste_upload_failed(
        &self,
        local: &std::path::Path,
        host: &str,
        reason: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        log::warn!(
            "pasted upload of {} to {host} failed: {reason}",
            local.display()
        );
        let name = local
            .file_name()
            .map_or_else(|| local.to_string_lossy(), |n| n.to_string_lossy());
        window.push_notification(
            anterm_i18n::t_fmt(
                anterm_i18n::L10nKey::SftpPasteUploadFailed,
                &[("name", &name), ("host", host), ("error", reason)],
            ),
            cx,
        );
    }
}
