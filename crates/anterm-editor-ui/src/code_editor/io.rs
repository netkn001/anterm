use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{Context, PromptLevel, Window};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::{RopeExt as _, TabSize};

use super::{
    BarKind, BufferId, DiskConflict, EDITORCONFIG_DEPTH, Fingerprint, MAX_FILE_BYTES, TabCode,
    language_for_path, replace_buffer_text,
};
use crate::CodeEditorWorkspace;
use anterm_editor::text::{self as editor_text, EditorConfig, Indent, TextFormat};
use anterm_ui_kit::host_ops::{HostId, HostOps, MTime, WatchSub};
use anterm_i18n::{L10nKey, t, t_fmt};

/// Why the built-in editor could not take a file.
pub(super) enum EditorOpenError {
    /// Not text, so the editor was never the right place for it.
    NotText(PathBuf),
    /// Something already worded for the user.
    Message(String),
}

/// A file read off its host, decoded, with everything needed to edit it.
pub(super) struct Loaded {
    pub(super) path: PathBuf,
    pub(super) text: String,
    pub(super) format: TextFormat,
    pub(super) mtime: Option<MTime>,
    pub(super) config: EditorConfig,
}

/// Reads, sizes, decodes and configures a file, on the host's own thread.
pub(super) fn load_file(h: &dyn anterm_core::host::Host, requested: PathBuf) -> Result<Loaded, EditorOpenError> {
    let path = h.canonicalize(&requested).unwrap_or(requested);
    let meta = h.stat(&path).map_err(|e| {
        EditorOpenError::Message(t_fmt(
            L10nKey::EditorCantOpen,
            &[("path", &path.display().to_string()), ("e", &e.to_string())],
        ))
    })?;
    if meta.len > MAX_FILE_BYTES {
        return Err(EditorOpenError::Message(t_fmt(
            L10nKey::EditorFileTooLarge,
            &[
                ("path", &path.display().to_string()),
                ("size", &(meta.len / (1024 * 1024)).to_string()),
            ],
        )));
    }
    let bytes = h.read_file(&path, MAX_FILE_BYTES).map_err(|e| {
        EditorOpenError::Message(t_fmt(
            L10nKey::EditorCantRead,
            &[("path", &path.display().to_string()), ("e", &e.to_string())],
        ))
    })?;
    let Some(decoded) = editor_text::decode(&bytes) else {
        return Err(EditorOpenError::NotText(path));
    };
    let config = read_editorconfig(h, &path);
    Ok(Loaded {
        path,
        text: decoded.text,
        format: decoded.format,
        mtime: meta.mtime,
        config,
    })
}

/// Collects the `.editorconfig` files that apply to `path`, nearest first,
/// stopping at the first one that declares itself the root.
fn read_editorconfig(h: &dyn anterm_core::host::Host, path: &Path) -> EditorConfig {
    let mut configs = Vec::new();
    for dir in path.ancestors().skip(1).take(EDITORCONFIG_DEPTH) {
        let file = h.join(dir, editor_text::EDITORCONFIG);
        let Ok(bytes) = h.read_file(&file, 64 * 1024) else {
            continue;
        };
        let contents = String::from_utf8_lossy(&bytes).into_owned();
        let root = editor_text::is_root(&contents);
        configs.push((dir.to_path_buf(), contents));
        if root {
            break;
        }
    }
    editor_text::editorconfig_for(path, &configs)
}

pub(super) fn tab_size(indent: Indent) -> TabSize {
    TabSize {
        tab_size: indent.size.max(1),
        hard_tabs: indent.hard_tabs,
    }
}

/// Whether handing this path to the desktop would run it rather than show it.
///
/// The execute bit is what `open` reads to decide between displaying a file
/// and launching it; Windows has no such bit, so there the extension is the
/// only thing that says so.
pub(super) fn is_program(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return std::fs::metadata(path)
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
    }
    #[cfg(not(unix))]
    {
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            return false;
        };
        matches!(
            ext.to_ascii_lowercase().as_str(),
            "exe"
                | "com"
                | "bat"
                | "cmd"
                | "scr"
                | "pif"
                | "msi"
                | "ps1"
                | "vbs"
                | "js"
                | "jse"
                | "wsf"
                | "wsh"
                | "cpl"
                | "msc"
                | "hta"
                | "reg"
                | "lnk"
        )
    }
}

/// A file a browser renders rather than showing its source.
pub(super) fn opens_in_browser(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "html" | "htm" | "xhtml" | "svg"
        )
    })
}

/// Opens a local file in the default web browser. On macOS that is asked
/// for by name: what `.html` is associated with is as often an editor. The
/// other desktops go by the association.
pub(super) fn open_in_browser(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    if let Some(bundle) = default_browser_bundle_id() {
        std::process::Command::new("open")
            .arg("-b")
            .arg(bundle)
            .arg(path)
            .spawn()?;
        return Ok(());
    }
    anterm_ui_kit::platform::open::open_file_path(path)
}

/// The app that handles `https:` links — the default browser.
#[cfg(target_os = "macos")]
fn default_browser_bundle_id() -> Option<String> {
    use core_foundation::base::TCFType as _;
    use core_foundation::string::{CFString, CFStringRef};

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn LSCopyDefaultHandlerForURLScheme(scheme: CFStringRef) -> CFStringRef;
    }
    let scheme = CFString::new("https");
    let handler = unsafe { LSCopyDefaultHandlerForURLScheme(scheme.as_concrete_TypeRef()) };
    // A Copy function: the string is ours to release, which the wrapper does.
    (!handler.is_null()).then(|| unsafe { CFString::wrap_under_create_rule(handler) }.to_string())
}

/// The lines a selection covers, 1-based and inclusive. One that ends at the
/// very start of a line — a whole-line selection — does not take that line.
pub(super) fn selected_lines(
    text: &gpui_kit::component::input::Rope,
    range: std::ops::Range<usize>,
) -> Option<(usize, usize)> {
    if range.is_empty() {
        return None;
    }
    let start = text.offset_to_point(range.start);
    let end = text.offset_to_point(range.end);
    let last = match end.column == 0 && end.row > start.row {
        true => end.row - 1,
        false => end.row,
    };
    Some((start.row + 1, last + 1))
}

/// What a watcher saw at a path: the file's modification time, or that it
/// is not there any more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Observed {
    Present(Option<MTime>),
    Missing,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ExternalChange {
    Ignore,
    Conflict,
    Deleted,
    Reload,
}

pub(super) fn classify_external_change(
    saving: bool,
    dirty: bool,
    disk_mtime: Option<MTime>,
    observed: Observed,
) -> ExternalChange {
    if saving {
        return ExternalChange::Ignore;
    }
    let observed = match observed {
        Observed::Missing => return ExternalChange::Deleted,
        Observed::Present(m) => m,
    };
    if observed.is_some() && observed == disk_mtime {
        return ExternalChange::Ignore;
    }
    if dirty {
        ExternalChange::Conflict
    } else {
        ExternalChange::Reload
    }
}

/// How a save attempt ended, as reported back from the host's thread.
enum SaveOutcome {
    Saved(Option<MTime>),
    /// The file changed on disk since the buffer last matched it; nothing
    /// was written.
    Conflict(Option<MTime>),
    Failed(std::io::Error),
}

/// Writes `bytes` to `target`, first making sure nothing else wrote it since
/// the buffer last matched it. `expect` is `None` to write regardless — a
/// Save As, an Overwrite the user chose, or a file being recreated.
///
/// This is also the only external-change detection a host without a watcher
/// gets: an SFTP buffer never hears about someone else's edit until now.
fn write_checked(
    h: &dyn anterm_core::host::Host,
    target: &Path,
    bytes: &[u8],
    expect: Option<Option<MTime>>,
) -> SaveOutcome {
    if let Some(expected) = expect {
        match h.stat(target) {
            Ok(meta) if meta.mtime != expected => return SaveOutcome::Conflict(meta.mtime),
            Ok(_) => {}
            // Gone since it was opened: saving puts it back, which is what a
            // save of a deleted file means.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return SaveOutcome::Failed(e),
        }
    }
    match h.write_file(target, bytes) {
        Ok(meta) => SaveOutcome::Saved(meta.mtime),
        Err(e) => SaveOutcome::Failed(e),
    }
}


fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

impl CodeEditorWorkspace {
    pub(super) fn editor_rebuild_watcher(&mut self, cx: &mut Context<Self>) {
        // Only files on the host the watch itself runs on. A path from
        // another machine — an SFTP file, say — does not exist under that
        // watcher's feet, and would either miss or, worse, match a local file
        // that happens to share its name. Those are checked when they are
        // saved instead: see `write_checked`.
        let watch_host = self.spawn_host(cx);
        let files: HashSet<PathBuf> = self
            .editor
            .buffers
            .iter()
            .filter(|f| f.untitled.is_none() && f.host.id() == watch_host)
            .map(|f| f.path.clone())
            .collect();
        let dirs: HashSet<PathBuf> = files
            .iter()
            .filter_map(|p| p.parent().map(Path::to_path_buf))
            .collect();
        self.editor.watched_files = files;
        // Every change to which files are open, or where they live, passes
        // through here — which is exactly what the language servers follow.
        self.lsp_sync_buffers(cx);
        if dirs == self.editor.watched_dirs {
            return;
        }
        self.editor.watched_dirs = dirs;
        self.editor_watch_apply(cx);
    }

    fn editor_watch_apply(&mut self, cx: &mut Context<Self>) {
        let want: Vec<PathBuf> = self.editor.watched_dirs.iter().cloned().collect();
        let Some(host) = self.active_host(cx) else {
            return;
        };

        if !self
            .editor
            .watch_host
            .as_ref()
            .is_some_and(|opened_with| Arc::ptr_eq(opened_with, &host))
        {
            self.editor.watch = None;
            self.editor.watch_host = None;
            self.editor.watch_busy = false;
            self.editor.watch_dirty = false;
        }

        if let Some(sub) = self.editor.watch.clone() {
            if self.editor.watch_busy {
                self.editor.watch_dirty = true;
                return;
            }
            self.editor.watch_busy = true;
            HostOps::run(
                host,
                cx,
                move |_| sub.set_dirs(&want),
                |app: &mut Self, result: std::io::Result<()>, cx| {
                    app.editor.watch_busy = false;
                    if let Err(e) = result {
                        log::warn!("editor: could not update the watched set: {e}");
                    }
                    if std::mem::take(&mut app.editor.watch_dirty) {
                        app.editor_watch_apply(cx);
                    }
                },
            );
            return;
        }
        if self.editor.watch_opening {
            return;
        }
        self.editor.watch_opening = true;
        let opened_host = Arc::clone(&host);
        let opened_with = self.editor.watched_dirs.clone();
        HostOps::run(
            host,
            cx,
            {
                let want = want.clone();
                move |h| h.watch(&want).map(Arc::new)
            },
            move |app, result: std::io::Result<Arc<WatchSub>>, cx| {
                app.editor.watch_opening = false;
                let sub = match result {
                    Ok(sub) => sub,
                    Err(e) => {
                        log::warn!("editor: external-change watcher unavailable: {e}");
                        return;
                    }
                };
                let events = sub.events().clone();
                app.editor.watch = Some(sub);
                app.editor.watch_host = Some(opened_host);
                cx.spawn(async move |app, cx| {
                    while let Ok(batch) = events.recv().await {
                        let ok = app.update(cx, |app, _cx| {
                            let _ = app.editor.events_tx.try_send(batch);
                        });
                        if ok.is_err() {
                            break;
                        }
                    }
                })
                .detach();
                if app.editor.watched_dirs != opened_with {
                    app.editor_watch_apply(cx);
                }
            },
        );
    }

    pub fn editor_save_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.tab_code().and_then(TabCode::active_id) else {
            return;
        };
        self.editor_save_file(id, false, false, window, cx);
    }

    pub fn editor_save_as_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.tab_code().and_then(TabCode::active_id) else {
            return;
        };
        self.editor_save_as(id, false, window, cx);
    }

    /// Applies the file's `.editorconfig` save rules — trailing whitespace,
    /// final newline — to the buffer itself, before it is written, so what is
    /// on screen is what is on disk.
    fn apply_save_rules(&mut self, id: BufferId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.buffer(id) else {
            return;
        };
        let text = f.input.read(cx).text().to_string();
        if let Some(fixed) = editor_text::apply_save_rules(&text, &f.config) {
            let input = f.input.clone();
            replace_buffer_text(&input, &fixed, window, cx);
        }
    }

    pub(super) fn editor_save_file(
        &mut self,
        id: BufferId,
        then_close: bool,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(f) = self.buffer_mut(id) else {
            return;
        };
        if f.untitled.is_some() {
            self.editor_save_as(id, then_close, window, cx);
            return;
        }
        f.save_then_close |= then_close;
        if f.saving {
            f.save_pending = true;
            return;
        }
        self.apply_save_rules(id, window, cx);
        let Some(f) = self.buffer_mut(id) else {
            return;
        };
        let text = f.input.read(cx).text().to_string();
        // Pasted text can carry its own `\r\n`; the buffer's line ending is
        // the file's, and encoding adds it back uniformly.
        let text = if text.contains("\r\n") {
            text.replace("\r\n", "\n")
        } else {
            text
        };
        let bytes = match editor_text::encode(&text, &f.format) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.editor_offer_utf8(id, then_close, e.unmappable, window, cx);
                return;
            }
        };
        let Some(f) = self.buffer_mut(id) else {
            return;
        };
        f.saving = true;
        let expect = match (force, f.conflict) {
            (true, _) | (_, Some(DiskConflict::Deleted)) => None,
            _ => Some(f.disk_mtime),
        };
        let written = Fingerprint::of_str(&text);
        let format = f.format;
        let host = f.host.clone();
        let target = f.path.clone();
        let host_id = host.id();
        let saved_in = target.parent().map(Path::to_path_buf);
        HostOps::run_in(
            host,
            window,
            cx,
            move |h| write_checked(h, &target, &bytes, expect),
            move |app, outcome: SaveOutcome, window, cx| {
                let Some(f) = app.buffer_mut(id) else {
                    return;
                };
                f.saving = false;
                let pending = std::mem::take(&mut f.save_pending);
                match outcome {
                    SaveOutcome::Saved(mtime) => {
                        f.disk_mtime = mtime;
                        f.saved = written;
                        f.saved_format = format;
                        f.conflict = None;
                        app.editor_note_edit(id, cx);
                        app.lsp_buffer_saved(id, cx);
                        app.editor_gutter_refetch(id);
                        // A save is a working-tree edit the `.git` watch cannot
                        // see, and the file tree only sees it while it happens
                        // to be showing that directory.
                        if let Some(dir) = &saved_in {
                            anterm_ui_kit::git::data::scm_invalidate_cwd(host_id, dir, cx);
                        }
                        if pending {
                            app.editor_save_file(id, false, false, window, cx);
                            return;
                        }
                        let close = app
                            .buffer_mut(id)
                            .is_some_and(|f| std::mem::take(&mut f.save_then_close) && !f.dirty);
                        if close {
                            app.editor_drop_buffer(id, cx);
                        }
                        app.editor_saves_landed(window, cx);
                    }
                    SaveOutcome::Conflict(observed) => {
                        f.save_then_close = false;
                        f.conflict = Some(DiskConflict::Changed(observed));
                        app.editor_saves_failed(id);
                        app.editor_ask_overwrite(id, window, cx);
                    }
                    SaveOutcome::Failed(e) => {
                        f.save_then_close = false;
                        // "Save failed" did not say which file, and with more
                        // than one editor tab open that is the first thing you
                        // need to know.
                        let context = t_fmt(L10nKey::EditorSaveFailed, &[("name", &f.label())]);
                        HostOps::notify_err(window, cx, &context, &e);
                        app.editor_saves_failed(id);
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    /// Asked when a save finds the file changed underneath it.
    fn editor_ask_overwrite(&mut self, id: BufferId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.buffer(id) else {
            return;
        };
        let name = f.label();
        let answer = window.prompt(
            PromptLevel::Warning,
            &t_fmt(L10nKey::EditorSaveConflictTitle, &[("name", &name)]),
            Some(t(L10nKey::EditorSaveConflictBody)),
            &anterm_ui_kit::confirm_answers(t(L10nKey::EditorOverwrite), t(L10nKey::Cancel)),
            cx,
        );
        cx.spawn_in(window, async move |app, cx| {
            if !matches!(answer.await, Ok(0)) {
                return;
            }
            let _ = app.update_in(cx, |app, window, cx| {
                app.editor_save_file(id, false, true, window, cx);
            });
        })
        .detach();
    }

    /// Asked when the text holds a character the file's encoding cannot.
    fn editor_offer_utf8(
        &mut self,
        id: BufferId,
        then_close: bool,
        unmappable: char,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(f) = self.buffer_mut(id) else {
            return;
        };
        f.save_then_close = false;
        let name = f.label();
        let encoding = f.format.encoding_label();
        self.editor_saves_failed(id);
        let answer = window.prompt(
            PromptLevel::Warning,
            &t_fmt(
                L10nKey::EditorEncodeFailedTitle,
                &[("name", &name), ("encoding", encoding)],
            ),
            Some(&t_fmt(
                L10nKey::EditorEncodeFailedBody,
                &[("ch", &unmappable.to_string()), ("encoding", encoding)],
            )),
            &anterm_ui_kit::confirm_answers(t(L10nKey::EditorSaveAsUtf8), t(L10nKey::Cancel)),
            cx,
        );
        cx.spawn_in(window, async move |app, cx| {
            if !matches!(answer.await, Ok(0)) {
                return;
            }
            let _ = app.update_in(cx, |app, window, cx| {
                if let Some(f) = app.buffer_mut(id) {
                    f.format = TextFormat {
                        line_ending: f.format.line_ending,
                        ..TextFormat::default()
                    };
                }
                app.editor_save_file(id, then_close, false, window, cx);
            });
        })
        .detach();
    }

    /// Save As: the native panel for a file on this machine, the path bar for
    /// one on a machine the panel cannot browse.
    pub(super) fn editor_save_as(
        &mut self,
        id: BufferId,
        then_close: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(f) = self.buffer(id) else {
            return;
        };
        let suggested = if f.untitled.is_some() {
            PathBuf::from(format!("{}.txt", f.label()))
        } else {
            PathBuf::from(f.label().to_string())
        };
        let dir = match f.path.parent() {
            Some(dir) if f.untitled.is_none() => dir.to_path_buf(),
            _ => self
                .active_tree_roots(cx)
                .into_iter()
                .next()
                .unwrap_or_default(),
        };
        if !f.host.id().is_local() {
            let start = if dir.as_os_str().is_empty() {
                suggested
            } else {
                f.host.join(&dir, &suggested.to_string_lossy())
            };
            self.editor_open_bar(
                BarKind::SaveAs { id, then_close },
                start.display().to_string(),
                window,
                cx,
            );
            return;
        }
        let dir = if dir.as_os_str().is_empty() {
            dirs_home()
        } else {
            dir
        };
        let rx = cx.prompt_for_new_path(&dir, suggested.to_str());
        cx.spawn_in(window, async move |app, cx| {
            let Ok(Ok(Some(path))) = rx.await else {
                // Cancelled: a close that was waiting for this save must not
                // go ahead on some later, unrelated one.
                let _ = app.update(cx, |app, _cx| app.editor_saves_failed(id));
                return;
            };
            let _ = app.update_in(cx, |app, window, cx| {
                app.editor_save_to(id, path, then_close, window, cx);
            });
        })
        .detach();
    }

    /// Writes a buffer to a new path and points it there from then on.
    pub(super) fn editor_save_to(
        &mut self,
        id: BufferId,
        path: PathBuf,
        then_close: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(f) = self.buffer(id) else {
            return;
        };
        let host = f.host.clone();
        if let Some(other) = self.buffer_at(host.id(), &path)
            && other != id
        {
            window.push_notification(
                t_fmt(
                    L10nKey::EditorAlreadyOpen,
                    &[("path", &path.display().to_string())],
                ),
                cx,
            );
            return;
        }
        if let Some(f) = self.buffer_mut(id) {
            f.path = path.clone();
            f.untitled = None;
            f.conflict = None;
            // A new file keeps the format the buffer had; its own
            // `.editorconfig` is read the next time it opens.
            let language = language_for_path(&path);
            f.input
                .update(cx, |st, cx| st.set_highlighter(language, cx));
        }
        self.editor_rebuild_watcher(cx);
        // Written as a save to a file already known to be the one intended:
        // the native panel has asked about replacing an existing file, and
        // the path bar asks before it gets here.
        self.editor_save_file(id, then_close, true, window, cx);
    }

    // ---- The file tree moving files out from under their buffers ----

    /// A file or folder was renamed through anterm: buffers at or under the old
    /// path follow it. Without this the buffer kept the old name, and the next
    /// save wrote a second copy there.
    pub fn editor_path_moved(
        &mut self,
        host: HostId,
        from: &Path,
        to: &Path,
        cx: &mut Context<Self>,
    ) {
        let mut moved = false;
        for f in self.editor.buffers.iter_mut() {
            if f.untitled.is_some() || f.host.id() != host {
                continue;
            }
            let Ok(rest) = f.path.strip_prefix(from) else {
                continue;
            };
            f.path = if rest.as_os_str().is_empty() {
                to.to_path_buf()
            } else {
                to.join(rest)
            };
            let language = language_for_path(&f.path);
            f.input
                .update(cx, |st, cx| st.set_highlighter(language, cx));
            moved = true;
        }
        if moved {
            self.editor_rebuild_watcher(cx);
            cx.notify();
        }
    }

    /// A file or folder was deleted through anterm: its buffers say so rather
    /// than waiting for the watcher, which a remote host may not have.
    pub fn editor_path_removed(
        &mut self,
        host: HostId,
        path: &Path,
        cx: &mut Context<Self>,
    ) {
        for f in self.editor.buffers.iter_mut() {
            if f.untitled.is_none() && f.host.id() == host && f.path.starts_with(path) {
                f.conflict = Some(DiskConflict::Deleted);
                f.disk_mtime = None;
            }
        }
        cx.notify();
    }

    // ---- Changes made by something else ----

    pub fn editor_handle_external_change(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The watcher's own host: the events came from its machine, whatever
        // the window has moved on to since.
        let Some(host) = self.editor.watch_host.clone() else {
            return;
        };
        let host_id = host.id();
        let p = path.to_path_buf();
        let landed = p.clone();
        HostOps::run_in(
            host,
            window,
            cx,
            move |h| match h.stat(&p) {
                Ok(m) => Observed::Present(m.mtime),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Observed::Missing,
                // Unreadable for some other reason: say nothing rather than
                // call a file deleted that may well still be there.
                Err(_) => Observed::Present(None),
            },
            move |app, observed, window, cx| {
                app.editor_apply_external_change(host_id, &landed, observed, window, cx)
            },
        );
    }

    fn editor_apply_external_change(
        &mut self,
        host: HostId,
        path: &Path,
        observed: Observed,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.buffer_at(host, path) else {
            return;
        };
        let Some(f) = self.buffer_mut(id) else {
            return;
        };
        match classify_external_change(f.saving, f.dirty, f.disk_mtime, observed) {
            ExternalChange::Ignore => {}
            ExternalChange::Conflict => {
                let Observed::Present(mtime) = observed else {
                    return;
                };
                f.conflict = Some(DiskConflict::Changed(mtime));
                cx.notify();
            }
            ExternalChange::Deleted => {
                f.conflict = Some(DiskConflict::Deleted);
                f.disk_mtime = None;
                cx.notify();
            }
            ExternalChange::Reload => self.editor_reload_from_disk(id, window, cx),
        }
    }

    pub fn editor_reload_from_disk(
        &mut self,
        id: BufferId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(f) = self.buffer_mut(id) else {
            return;
        };
        if f.untitled.is_some() {
            return;
        }
        let target = f.path.clone();
        let host = f.host.clone();
        f.reload_seq = f.reload_seq.wrapping_add(1);
        let seq = f.reload_seq;
        HostOps::run_in(
            host,
            window,
            cx,
            move |h| {
                let bytes = h.read_file(&target, MAX_FILE_BYTES)?;
                let decoded = editor_text::decode(&bytes).ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "no longer text")
                })?;
                let mtime = h.stat(&target).ok().and_then(|m| m.mtime);
                Ok((decoded, mtime))
            },
            move |app,
                  result: std::io::Result<(editor_text::Decoded, Option<MTime>)>,
                  window,
                  cx| {
                let Some(f) = app.buffer_mut(id) else {
                    return;
                };
                if f.reload_seq != seq {
                    return;
                }
                let (decoded, mtime) = match result {
                    Ok(ok) => ok,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        f.conflict = Some(DiskConflict::Deleted);
                        f.disk_mtime = None;
                        cx.notify();
                        return;
                    }
                    // Unreadable for a moment — mid-write, say — is not a
                    // reason to touch the buffer; the next change event tries
                    // again.
                    Err(e) => {
                        log::warn!("editor: reload of {} failed: {e}", f.path.display());
                        return;
                    }
                };
                f.disk_mtime = mtime;
                f.conflict = None;
                f.saved = Fingerprint::of_str(&decoded.text);
                f.format = decoded.format;
                f.saved_format = decoded.format;
                let input = f.input.clone();
                replace_buffer_text(&input, &decoded.text, window, cx);
                // The replace reports its own change, but only once effects
                // flush; settle it now so nothing reads a stale flag first.
                app.editor_note_edit(id, cx);
            },
        );
    }
}
