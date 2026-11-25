mod state;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, Context, Div, EventEmitter, ExternalPaths, FontWeight, PathPromptOptions,
    SharedString, Stateful, Window, div, prelude::*, px, rems, size,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, InteractiveElementExt as _, Sizable as _,
    h_flex, v_flex, v_virtual_list,
};

use anterm_core::daemon::protocol::{
    SftpEntry, SftpEntryKind, SftpJobProgress, SftpJobState, SftpOp, SftpOpResult,
    SftpTransferKind, SftpTransferSpec,
};
use anterm_core::daemon::ssh::sftp::{remote_basename, remote_join, remote_parent, safe_local_name};
use anterm_ui_kit::panel_tokens::{
    HEADING, META, ROW_FILL_RADIUS, ROW_GLYPH, ROW_INSET, TEXT, TEXT_INSET, panel_search_box,
};
use anterm_ui_kit::window_chrome::{CONTENT_INSET, TILE_GLYPH_SM, TILE_SIZE_SM};
use anterm_ui_kit::intent::WorkspaceIntent;
use anterm_i18n::{L10nKey, t, t_fmt};

pub use state::{
    SftpEdit, SftpEvent, SftpPanelView, SftpRoute, breadcrumb_segments, free_local_path,
    local_download_dir,
};
#[cfg(test)]
pub use state::sorted_filtered_entries;
use state::{
    SftpMenuAction, apply_poll, human_size, is_dir_like, mode_string, sftp_start_dir,
    sorted_filtered_indices, uploads_still_running,
};

/// A remote directory row: the local file tree's 26px row, on the panel's
/// `ROW_FILL_RADIUS`, so the Files tab keeps its rhythm whichever machine it
/// is showing.
const SFTP_ROW_H: f32 = 26.;

/// The gap between a row's icon, name and size — the file tree's.
const SFTP_ROW_GAP: f32 = 6.;

/// The transfer tray's disclosure chevron — the file tree's.
const TRAY_CHEVRON: f32 = 10.;

impl EventEmitter<SftpEvent> for SftpPanelView {}

impl SftpPanelView {
    pub fn is_open(&self) -> bool {
        self.open_pane_id.is_some()
    }

    pub fn open_pane_id(&self) -> Option<u64> {
        self.open_pane_id
    }

    pub fn mark_panel_closed(&mut self) {
        self.panel_was_closed = true;
    }

    pub fn refresh_locale(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter_input.update(cx, |state, cx| {
            state.set_placeholder(t(L10nKey::SearchFiles), window, cx);
        });
    }

    /// Synchronizes the SFTP browser against the active SSH pane, returning
    /// `true` when the Files panel should display SFTP rather than the local
    /// file tree.
    pub fn sync_pane(
        &mut self,
        remote: Option<(u64, String)>,
        shell_cwd: Option<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((pane_id, host)) = remote else {
            if self.open_pane_id.is_some() {
                self.close_browser(cx);
            }
            return false;
        };
        if self.host_label != host {
            self.host_label = host;
        }
        if self.open_pane_id != Some(pane_id) {
            self.open_at(pane_id, shell_cwd, cx);
        }
        true
    }

    pub fn close_browser(&mut self, cx: &mut Context<Self>) {
        self.open_pane_id = None;
        self.host_label.clear();
        self.entries.clear();
        self.entries_rev = self.entries_rev.wrapping_add(1);
        self.error = None;
        // No `Window` here, and none needed: the browser itself is going away
        // or being re-pointed at another pane, so focus is settled by whoever
        // did that, not by the form.
        let _ = self.close_edit();
        self.editing_path = None;
        self.editing_path_sub.clear();
        self.jobs.clear();
        self.jobs_error = None;
        self.poll_gen = self.poll_gen.wrapping_add(1);
        cx.notify();
    }

    fn route(&self) -> SftpRoute {
        SftpRoute {
            pane_id: self.open_pane_id.unwrap_or_default(),
        }
    }

    fn open_at(&mut self, pane_id: u64, shell_cwd: Option<String>, cx: &mut Context<Self>) {
        self.open_pane_id = Some(pane_id);
        self.entries.clear();
        self.entries_rev = self.entries_rev.wrapping_add(1);
        self.error = None;
        // No `Window` here, and none needed: the browser itself is going away
        // or being re-pointed at another pane, so focus is settled by whoever
        // did that, not by the form.
        let _ = self.close_edit();
        self.editing_path = None;
        self.editing_path_sub.clear();
        self.show_history = false;
        self.poll_jobs(cx);
        self.start_polling(cx);

        let fresh_open = std::mem::take(&mut self.panel_was_closed);
        let remembered = self.cwds.get(&pane_id).cloned();
        if let Some(start) = sftp_start_dir(fresh_open, shell_cwd, remembered) {
            self.navigate(start, cx);
            return;
        }
        self.navigate_login_dir(pane_id, cx);
    }

    fn navigate_login_dir(&mut self, pane_id: u64, cx: &mut Context<Self>) {
        self.loading = true;
        let route = self.route();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { route.op(SftpOp::Realpath { path: ".".into() }) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.open_pane_id != Some(pane_id) {
                    return;
                }
                let home = match result {
                    SftpOpResult::Link(path) if path.starts_with('/') => path,
                    _ => "/".to_string(),
                };
                this.navigate(home, cx);
            });
        })
        .detach();
    }

    pub fn navigate(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(pane_id) = self.open_pane_id else {
            return;
        };
        self.nav_gen = self.nav_gen.wrapping_add(1);
        let generation = self.nav_gen;
        self.loading = true;
        cx.notify();

        let list_path = path.clone();
        let route = self.route();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { route.list(&list_path) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.open_pane_id != Some(pane_id) || this.nav_gen != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(mut entries) => {
                        entries.sort_by(|a, b| a.name.cmp(&b.name));
                        this.cwds.insert(pane_id, path.clone());
                        this.cwd = path;
                        this.entries = entries;
                        this.entries_rev = this.entries_rev.wrapping_add(1);
                        this.error = None;
                        this.editing_path = None;
                        this.editing_path_sub.clear();
                    }
                    Err(e) => {
                        this.error = Some(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let cwd = self.cwd.clone();
        self.navigate(cwd, cx);
    }

    pub fn up(&mut self, cx: &mut Context<Self>) {
        let parent = remote_parent(&self.cwd);
        self.navigate(parent, cx);
    }

    pub fn begin_edit_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_pane_id.is_none() {
            return;
        }
        let cwd = self.cwd.clone();
        let input = anterm_ui_kit::prefill::filled_box(cwd, window, cx);
        input.update(cx, |s, cx| s.focus(window, cx));
        let sub = cx.subscribe_in(
            &input,
            window,
            |this, _input, ev: &InputEvent, _window, cx| match ev {
                InputEvent::PressEnter { .. } => this.commit_edit_path(cx),
                InputEvent::Blur => this.cancel_edit_path(cx),
                _ => {}
            },
        );
        self.editing_path = Some(input);
        self.editing_path_sub = vec![sub];
        cx.notify();
    }

    pub fn commit_edit_path(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.editing_path.take() else {
            return;
        };
        self.editing_path_sub.clear();
        let value = input.read(cx).value().trim().to_string();
        if value.is_empty() {
            cx.notify();
            return;
        }
        self.navigate(value, cx);
    }

    pub fn cancel_edit_path(&mut self, cx: &mut Context<Self>) {
        self.editing_path = None;
        self.editing_path_sub.clear();
        cx.notify();
    }

    pub fn clear_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter_input
            .update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    /// A click on a row and the row menu's first item both land here.
    /// A directory opens in place; a file emits [`SftpEvent::OpenRemoteFile`]
    /// so the workspace opens it in the built-in editor.
    pub fn open_entry(&mut self, entry: SftpEntry, cx: &mut Context<Self>) {
        let target = remote_join(&self.cwd, &entry.name);
        if is_dir_like(&entry) {
            self.navigate(target, cx);
        } else if let Some(host) = self.editor_host() {
            cx.emit(SftpEvent::OpenRemoteFile {
                host,
                path: PathBuf::from(target),
            });
        }
    }

    /// The [`Host`] the editor reads and saves this pane's files through.
    ///
    /// Handed to the editor rather than filed in `HostRegistry`: that table
    /// means "a machine this window has a link to", and its entries are
    /// listed as machines and swept when no workspace is left holding one.
    /// An SFTP channel borrowed from a pane is neither, so the buffer holds
    /// the host itself and stays saveable for as long as it is open.
    ///
    /// [`Host`]: anterm_ui_kit::host_ops::Host
    fn editor_host(&self) -> Option<anterm_ui_kit::host_ops::SharedHost> {
        self.open_pane_id?;
        Some(std::sync::Arc::new(crate::sftp_host::SftpHost::new(
            self.route(),
        )))
    }

    pub fn download_entry(&mut self, entry: SftpEntry, cx: &mut Context<Self>) {
        let Some(pane_id) = self.open_pane_id else {
            return;
        };
        if !safe_local_name(&entry.name) {
            self.error = Some(t_fmt(
                L10nKey::SftpErrorUnsafeRemoteName,
                &[("name", &format!("{:?}", entry.name))],
            ));
            cx.notify();
            return;
        }
        let remote = remote_join(&self.cwd, &entry.name);
        self.claimed_downloads.retain(|p| !p.exists());
        let Some(local) =
            free_local_path(&local_download_dir(), &entry.name, &self.claimed_downloads)
        else {
            self.error = Some(t_fmt(
                L10nKey::SftpErrorNoFreeLocalName,
                &[("name", &entry.name)],
            ));
            cx.notify();
            return;
        };
        self.claimed_downloads.insert(local.clone());
        let recursive = matches!(entry.kind, SftpEntryKind::Dir);
        let spec = SftpTransferSpec {
            pane_id,
            kind: SftpTransferKind::Download,
            local,
            remote,
            recursive,
        };
        match self.route().transfer_start(spec) {
            Ok(_) => self.error = None,
            Err(e) => self.error = Some(e),
        }
        self.poll_jobs(cx);
        self.start_polling(cx);
    }

    pub fn delete_entry(
        &mut self,
        entry: SftpEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane_id) = self.open_pane_id else {
            return;
        };
        let path = remote_join(&self.cwd, &entry.name);
        let is_dir = matches!(entry.kind, SftpEntryKind::Dir);
        let host = if self.host_label.is_empty() {
            t(L10nKey::AppLocalServerName).to_string()
        } else {
            self.host_label.clone()
        };
        let body = t_fmt(
            match is_dir {
                true => L10nKey::SftpDeleteFolderBody,
                false => L10nKey::SftpDeleteFileBody,
            },
            &[("host", &host)],
        );
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &t_fmt(L10nKey::FileTreeDeleteTitle, &[("name", &entry.name)]),
            Some(&body),
            &anterm_ui_kit::confirm_answers(t(L10nKey::Delete), t(L10nKey::Cancel)),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(0) = answer.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                if this.open_pane_id != Some(pane_id) {
                    return;
                }
                let op = match is_dir {
                    true => SftpOp::RemoveDir { path },
                    false => SftpOp::RemoveFile { path },
                };
                this.run_op(pane_id, op, window, cx);
            });
        })
        .detach();
    }

    pub fn follow_symlink(&mut self, entry: SftpEntry, cx: &mut Context<Self>) {
        let Some(pane_id) = self.open_pane_id else {
            return;
        };
        let cwd = self.cwd.clone();
        let path = remote_join(&cwd, &entry.name);
        let route = self.route();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { route.op(SftpOp::Readlink { path }) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.open_pane_id != Some(pane_id) {
                    return;
                }
                match result {
                    SftpOpResult::Link(target) => {
                        let resolved = if target.starts_with('/') {
                            target
                        } else {
                            remote_join(&cwd, &target)
                        };
                        let dest = if entry.target_is_dir {
                            resolved
                        } else {
                            remote_parent(&resolved)
                        };
                        this.navigate(dest, cx);
                    }
                    SftpOpResult::Error(e) => {
                        this.error = Some(e);
                        cx.notify();
                    }
                    _ => {}
                }
            });
        })
        .detach();
    }

    fn run_op(&mut self, pane_id: u64, op: SftpOp, window: &mut Window, cx: &mut Context<Self>) {
        let route = self.route();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { route.op(op) }).await;
            let _ = this.update_in(cx, |this, _window, cx| {
                if this.open_pane_id != Some(pane_id) {
                    return;
                }
                match result {
                    SftpOpResult::Error(e) => {
                        this.error = Some(e);
                        cx.notify();
                    }
                    _ => {
                        this.close_edit_and_restore_focus(cx);
                        this.refresh(cx);
                    }
                }
            });
        })
        .detach();
    }

    fn open_edit(
        &mut self,
        input: gpui::Entity<InputState>,
        edit: SftpEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        input.update(cx, |s, cx| s.focus(window, cx));
        let sub = cx.subscribe_in(
            &input,
            window,
            |this, _input, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } => this.commit_edit(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        );
        self.editing = Some(edit);
        self.editing_sub = vec![sub];
        cx.notify();
    }

    pub fn begin_new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(anterm_i18n::t(anterm_i18n::L10nKey::NewFolderName))
        });
        self.open_edit(input.clone(), SftpEdit::NewFolder(input), window, cx);
    }

    pub fn begin_new_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(anterm_i18n::t(anterm_i18n::L10nKey::NewFileName))
        });
        self.open_edit(input.clone(), SftpEdit::NewFile(input), window, cx);
    }

    pub fn begin_rename(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = anterm_ui_kit::prefill::filled_box(name.clone(), window, cx);
        let edit = SftpEdit::Rename {
            original: name,
            input: input.clone(),
        };
        self.open_edit(input, edit, window, cx);
    }

    pub fn begin_chmod(
        &mut self,
        entry: SftpEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let octal = format!("{:o}", entry.permissions & 0o777);
        let readable = mode_string(entry.permissions);
        let path = remote_join(&self.cwd, &entry.name);
        let input = anterm_ui_kit::prefill::filled_box(octal, window, cx);
        let edit = SftpEdit::Chmod {
            path,
            readable,
            input: input.clone(),
        };
        self.open_edit(input, edit, window, cx);
    }

    #[must_use]
    fn close_edit(&mut self) -> bool {
        let was_open = self.editing.is_some();
        self.editing = None;
        self.editing_sub.clear();
        was_open
    }

    fn close_edit_and_restore_focus(&mut self, cx: &mut Context<Self>) {
        if self.close_edit() {
            cx.emit(SftpEvent::Intent(WorkspaceIntent::FocusActiveTerminal));
        }
    }

    pub fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        self.close_edit_and_restore_focus(cx);
        cx.notify();
    }

    pub fn commit_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane_id) = self.open_pane_id else {
            return;
        };
        let op = match &self.editing {
            Some(SftpEdit::NewFolder(input)) => {
                let name = input.read(cx).value().trim().to_string();
                if name.is_empty() {
                    return;
                }
                Some(SftpOp::Mkdir {
                    path: remote_join(&self.cwd, &name),
                })
            }
            Some(SftpEdit::NewFile(input)) => {
                let name = input.read(cx).value().trim().to_string();
                if name.is_empty() {
                    return;
                }
                Some(SftpOp::CreateFile {
                    path: remote_join(&self.cwd, &name),
                })
            }
            Some(SftpEdit::Rename { original, input }) => {
                let name = input.read(cx).value().trim().to_string();
                if name.is_empty() || name == *original {
                    self.close_edit_and_restore_focus(cx);
                    cx.notify();
                    return;
                }
                Some(SftpOp::Rename {
                    from: remote_join(&self.cwd, original),
                    to: remote_join(&self.cwd, &name),
                })
            }
            Some(SftpEdit::Chmod { path, input, .. }) => {
                match u32::from_str_radix(input.read(cx).value().trim(), 8) {
                    Ok(mode) => Some(SftpOp::Chmod {
                        path: path.clone(),
                        mode,
                    }),
                    Err(_) => {
                        self.error = Some(t(L10nKey::SftpErrorInvalidOctalMode).to_string());
                        cx.notify();
                        return;
                    }
                }
            }
            None => None,
        };
        if let Some(op) = op {
            self.run_op(pane_id, op, window, cx);
        }
    }

    pub fn pick_upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_pane_id.is_none() {
            return;
        }
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.upload_paths(paths, window, cx);
                });
            }
        })
        .detach();
    }

    pub fn upload_paths(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let clashes: Vec<String> = paths
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .filter(|name| self.entries.iter().any(|e| &e.name == name))
            .collect();
        if clashes.is_empty() {
            self.upload_paths_confirmed(paths, cx);
            return;
        }
        let names = match clashes.len() {
            1..=3 => clashes.join(", "),
            _ => format!("{}, …", clashes[..3].join(", ")),
        };
        let body = anterm_i18n::t_plural(
            L10nKey::SftpReplaceBody,
            clashes.len(),
            &[("names", &names)],
        );
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            t(L10nKey::SftpReplaceTitle),
            Some(&body),
            &anterm_ui_kit::confirm_answers(t(L10nKey::Replace), t(L10nKey::Cancel)),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(0) = answer.await else { return };
            let _ = this.update(cx, |this, cx| this.upload_paths_confirmed(paths, cx));
        })
        .detach();
    }

    fn upload_paths_confirmed(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(pane_id) = self.open_pane_id else {
            return;
        };
        let cwd = self.cwd.clone();
        for path in paths {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let recursive = path.is_dir();
            let spec = SftpTransferSpec {
                pane_id,
                kind: SftpTransferKind::Upload,
                local: path,
                remote: remote_join(&cwd, &name),
                recursive,
            };
            match self.route().transfer_start(spec) {
                Ok(job_id) => {
                    self.uploads_awaiting_listing.insert(job_id);
                }
                Err(e) => self.error = Some(e),
            }
        }
        self.poll_jobs(cx);
        self.start_polling(cx);
    }

    pub fn cancel_job(&mut self, job_id: u64, cx: &mut Context<Self>) {
        self.jobs = anterm_ui_kit::daemon_rpc::sftp_transfer_cancel(job_id);
        cx.notify();
    }

    pub fn toggle_history(&mut self, cx: &mut Context<Self>) {
        self.show_history = !self.show_history;
        cx.notify();
    }

    pub fn toggle_tray(&mut self, cx: &mut Context<Self>) {
        self.tray_expanded = !self.tray_expanded;
        cx.notify();
    }

    pub fn dismiss_tray(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<u64> = self.jobs.iter().map(|j| j.job_id).collect();
        self.dismissed_jobs.extend(ids);
        self.show_history = false;
        cx.notify();
    }

    pub fn reveal_download(&self, local: String, cx: &mut Context<Self>) {
        let local = Path::new(&local);
        cx.reveal_path(&anterm_ui_kit::path_display::native_separators(local));
    }

    fn poll_jobs(&mut self, cx: &mut Context<Self>) {
        if self.open_pane_id.is_some() {
            let jobs = self.route().transfer_list();
            self.apply_jobs(jobs, cx);
        }
    }

    fn apply_jobs(
        &mut self,
        reply: Result<Vec<SftpJobProgress>, String>,
        cx: &mut Context<Self>,
    ) {
        let previous = std::mem::take(&mut self.jobs);
        let (jobs, failure) = apply_poll(previous, reply);
        let failed = failure.is_some();
        self.jobs = jobs;
        self.jobs_error = failure;
        if failed {
            cx.notify();
            return;
        }
        let owed = &self.uploads_awaiting_listing;
        let still_running = uploads_still_running(owed, &self.jobs);
        let settled = still_running.len() != owed.len();
        self.uploads_awaiting_listing = still_running;
        cx.notify();
        if settled {
            self.refresh(cx);
        }
    }

    fn start_polling(&mut self, cx: &mut Context<Self>) {
        self.poll_gen = self.poll_gen.wrapping_add(1);
        let generation = self.poll_gen;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                let pane = this
                    .update(cx, |this, _cx| {
                        if this.poll_gen != generation {
                            return None;
                        }
                        this.open_pane_id.is_some().then(|| this.route())
                    })
                    .ok()
                    .flatten();
                let Some(route) = pane else { break };
                let jobs = cx
                    .background_spawn(async move { route.transfer_list() })
                    .await;
                let keep = this
                    .update(cx, |this, cx| {
                        if this.poll_gen != generation {
                            return false;
                        }
                        this.apply_jobs(jobs, cx);
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    /// Trailing refresh + menu action tiles placed in the right-panel header.
    pub fn render_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let history = self.show_history;
        let tile = |button: Button, selected: bool, cx: &mut Context<Self>| {
            anterm_ui_kit::window_chrome::chrome_tile_sized(
                button,
                TILE_SIZE_SM,
                TILE_GLYPH_SM,
                selected,
                cx,
            )
            .rounded_md()
        };

        h_flex()
            .items_center()
            .gap(px(2.))
            .child(
                div().occlude().child(
                    tile(
                        Button::new("panel-sftp-refresh")
                            .icon(Icon::empty().path("icons/refresh.svg")),
                        false,
                        cx,
                    )
                    .tooltip(t(L10nKey::SftpTooltipRefresh))
                    .on_click(cx.listener(|this, _, _w, cx| this.refresh(cx))),
                ),
            )
            .child(
                div().occlude().child(
                    tile(
                        Button::new("panel-sftp-menu")
                            .icon(Icon::empty().path("icons/ellipsis.svg")),
                        false,
                        cx,
                    )
                    .tooltip(t(L10nKey::SftpTooltipMore))
                    .dropdown_menu_with_anchor(gpui::Anchor::TopRight, {
                        let panel = cx.entity().downgrade();
                        move |menu, _window, _cx| {
                            let mut menu = menu.min_w(px(190.));
                            for (label, action) in [
                                (t(L10nKey::SftpMenuNewFolder), SftpMenuAction::NewFolder),
                                (t(L10nKey::SftpMenuNewFile), SftpMenuAction::NewFile),
                                (t(L10nKey::SftpMenuUpload), SftpMenuAction::Upload),
                                (
                                    t(L10nKey::SftpMenuGotoShellCwd),
                                    SftpMenuAction::GotoShellCwd,
                                ),
                            ] {
                                menu = menu.item(PopupMenuItem::new(label).on_click({
                                    let panel = panel.clone();
                                    move |_, window, cx| {
                                        let _ = panel.update(cx, |this, cx| {
                                            this.menu_action(action, window, cx)
                                        });
                                    }
                                }));
                            }
                            menu.separator().item(
                                PopupMenuItem::new(if history {
                                    t(L10nKey::SftpMenuHideTransferHistory)
                                } else {
                                    t(L10nKey::SftpMenuTransferHistory)
                                })
                                .on_click({
                                    let panel = panel.clone();
                                    move |_, window, cx| {
                                        let _ = panel.update(cx, |this, cx| {
                                            this.menu_action(
                                                SftpMenuAction::ToggleHistory,
                                                window,
                                                cx,
                                            )
                                        });
                                    }
                                }),
                            )
                        }
                    }),
                ),
            )
            .into_any_element()
    }

    fn menu_action(
        &mut self,
        action: SftpMenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            SftpMenuAction::NewFolder => self.begin_new_folder(window, cx),
            SftpMenuAction::NewFile => self.begin_new_file(window, cx),
            SftpMenuAction::Upload => self.pick_upload(window, cx),
            SftpMenuAction::GotoShellCwd => {
                if let Some(pane_id) = self.open_pane_id {
                    cx.emit(SftpEvent::GotoShellCwd { pane_id });
                }
            }
            SftpMenuAction::ToggleHistory => self.toggle_history(cx),
        }
    }

    fn render_breadcrumb(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        if let Some(input) = &self.editing_path {
            return h_flex()
                .id("sftp-path-edit")
                .px(px(CONTENT_INSET))
                .pb(px(2.))
                .child(Input::new(input).xsmall())
                .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, _window, cx| {
                    if ev.keystroke.key == "escape" {
                        this.cancel_edit_path(cx);
                    }
                }));
        }

        let foreground = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let mut row = h_flex()
            .id("sftp-breadcrumb")
            .flex_wrap()
            .items_center()
            .gap_0p5()
            .px(px(TEXT_INSET))
            .pb(px(4.))
            .on_double_click(cx.listener(|this, _, window, cx| this.begin_edit_path(window, cx)));
        let segments = breadcrumb_segments(&self.cwd);
        let last = segments.len().saturating_sub(1);
        for (i, (label, path)) in segments.into_iter().enumerate() {
            if i > 0 {
                row = row.child(div().text_xs().text_color(muted).child("›"));
            }
            let is_current = i == last;
            let label = if i == 0 { "/".to_string() } else { label };
            let weight = if i == 0 || is_current {
                FontWeight::MEDIUM
            } else {
                FontWeight::NORMAL
            };
            let color = if is_current { foreground } else { muted };
            let seg_id = SharedString::from(format!("sftp-crumb-{path}"));
            row = row.child(
                div()
                    .id(seg_id)
                    .text_xs()
                    .font_weight(weight)
                    .text_color(color)
                    .cursor_pointer()
                    .hover(|s| s.text_color(foreground).underline())
                    .child(label)
                    .on_click(cx.listener(move |this, _, _w, cx| this.navigate(path.clone(), cx))),
            );
        }
        row.child(div().flex_1().min_w(px(20.)).h(px(16.)))
    }

    fn render_edit_form(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let well = cx.theme().muted;
        let foreground = cx.theme().foreground;
        let surface: gpui::Hsla =
            gpui::rgb(cx.global::<anterm_ui_kit::presets::Surfaces>().sidebar.base).into();
        let (title, input): (String, _) = match self.editing.as_ref()? {
            SftpEdit::NewFolder(input) => (t(L10nKey::SftpEditNewFolder).to_string(), input),
            SftpEdit::NewFile(input) => (t(L10nKey::SftpEditNewFile).to_string(), input),
            SftpEdit::Rename { input, .. } => (t(L10nKey::SftpEditRename).to_string(), input),
            SftpEdit::Chmod {
                readable, input, ..
            } => (
                t_fmt(L10nKey::SftpEditPermissions, &[("mode", readable)]),
                input,
            ),
        };
        let can_commit = !input.read(cx).value().trim().is_empty();
        Some(
            v_flex()
                .id("panel-sftp-edit")
                .gap(px(6.))
                .mx(px(CONTENT_INSET))
                .mb(px(4.))
                .p(px(ROW_INSET))
                .bg(well)
                .rounded(px(7.))
                .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, _window, cx| {
                    if ev.keystroke.key == "escape" {
                        this.cancel_edit(cx);
                    }
                }))
                .child(
                    div()
                        .text_size(rems(HEADING))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(foreground)
                        .child(title),
                )
                .child(Input::new(input).xsmall())
                .child(
                    h_flex()
                        .gap(px(4.))
                        .justify_end()
                        .child(
                            Button::new("sftp-edit-cancel")
                                .label(t(L10nKey::Cancel))
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(|this, _, _w, cx| this.cancel_edit(cx))),
                        )
                        .child(
                            Button::new("sftp-edit-ok")
                                .label(t(L10nKey::Ok))
                                .xsmall()
                                .custom(anterm_ui_kit::theme::inverted_button(surface, cx))
                                .disabled(!can_commit)
                                .on_click(
                                    cx.listener(|this, _, w, cx| this.commit_edit(w, cx)),
                                ),
                        ),
                ),
        )
    }

    fn ensure_filtered_view(&mut self, filter: &str, show_go_up: bool) {
        if self.filtered_key.0 != self.entries_rev || self.filtered_key.1 != filter {
            self.filtered_indices = sorted_filtered_indices(&self.entries, filter);
            self.filtered_key = (self.entries_rev, filter.to_string());
        }
        let total = usize::from(show_go_up) + self.filtered_indices.len();
        if self.item_sizes.len() != total {
            self.item_sizes = Rc::new(vec![size(px(0.), px(SFTP_ROW_H)); total]);
        }
    }

    fn render_list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let danger = cx.theme().danger;
        let muted = cx.theme().muted_foreground;
        let note = |text: SharedString, color| {
            div()
                .id("sftp-list-note")
                .flex_1()
                .min_h_0()
                .px(px(CONTENT_INSET))
                .pb(px(4.))
                .child(
                    div()
                        .px(px(ROW_INSET))
                        .py(px(4.))
                        .text_size(rems(TEXT))
                        .text_color(color)
                        .child(text),
                )
                .into_any_element()
        };

        if let Some(err) = &self.error {
            return note(err.clone().into(), danger);
        }

        let filter = self.filter_input.read(cx).value().to_string();
        let show_go_up = self.cwd != "/" && filter.trim().is_empty();
        self.ensure_filtered_view(&filter, show_go_up);

        if self.filtered_indices.is_empty() && !show_go_up {
            let text: SharedString = if self.loading {
                t(L10nKey::SftpLoading).into()
            } else if !filter.trim().is_empty() {
                t_fmt(L10nKey::SettingsNothingMatches, &[("query", filter.trim())]).into()
            } else {
                t(L10nKey::SftpEmptyDirectory).into()
            };
            return note(text, muted);
        }

        let list = v_virtual_list(
            cx.entity(),
            "sftp-list",
            self.item_sizes.clone(),
            move |this, range, _window, cx| {
                range
                    .map(|ix| {
                        #[cfg(test)]
                        row_probe::record();
                        let inner = if show_go_up && ix == 0 {
                            this.render_go_up_row(cx)
                        } else {
                            let entry_pos = ix - usize::from(show_go_up);
                            match this
                                .filtered_indices
                                .get(entry_pos)
                                .and_then(|&ei| this.entries.get(ei))
                            {
                                Some(entry) => this.render_row(entry, cx),
                                None => div().into_any_element(),
                            }
                        };
                        div()
                            .w_full()
                            .px(px(CONTENT_INSET))
                            .child(inner)
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&self.scroll)
        .gap(px(1.))
        .pt(px(2.))
        .pb(px(4.));

        anterm_ui_kit::scrollbar::with_vertical_scrollbar(
            "sftp-list-scrollbar",
            list,
            &self.scroll,
        )
    }

    fn render_go_up_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (name_ink, muted) = (theme.sidebar_foreground, theme.muted_foreground);
        let sf = cx.global::<anterm_ui_kit::presets::Surfaces>().popover;
        h_flex()
            .id("sftp-go-up")
            .w_full()
            .items_center()
            .gap(px(SFTP_ROW_GAP))
            .h(px(SFTP_ROW_H))
            .px(px(ROW_INSET))
            .rounded(ROW_FILL_RADIUS)
            .cursor_pointer()
            .hover(|s| s.bg(gpui::rgb(sf.hover)))
            .child(
                Icon::new(IconName::FolderOpen)
                    .size(px(ROW_GLYPH))
                    .text_color(muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(rems(TEXT))
                    .text_color(name_ink)
                    .child(".."),
            )
            .on_click(cx.listener(|this, _, _w, cx| this.up(cx)))
            .into_any_element()
    }

    fn render_row(&self, entry: &SftpEntry, cx: &mut Context<Self>) -> AnyElement {
        let name_ink = cx.theme().sidebar_foreground;
        let muted = cx.theme().muted_foreground;
        let list_hover =
            gpui::rgb(cx.global::<anterm_ui_kit::presets::Surfaces>().popover.hover);
        let entry = entry.clone();
        let dir_like = is_dir_like(&entry);
        let icon = if dir_like {
            IconName::Folder
        } else {
            IconName::File
        };
        let is_symlink = matches!(entry.kind, SftpEntryKind::Symlink);
        let size = if dir_like {
            String::new()
        } else {
            human_size(entry.size)
        };
        let name_label = if is_symlink {
            format!("{} →", entry.name)
        } else {
            entry.name.clone()
        };
        let row_id = SharedString::from(format!("sftp-row-{}", entry.name));

        let open_entry = entry.clone();
        let menu_entry = entry.clone();
        let panel = cx.entity().downgrade();

        h_flex()
            .id(row_id)
            .w_full()
            .items_center()
            .gap(px(SFTP_ROW_GAP))
            .h(px(SFTP_ROW_H))
            .px(px(ROW_INSET))
            .rounded(ROW_FILL_RADIUS)
            .cursor_pointer()
            .hover(move |s| s.bg(list_hover))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.open_entry(open_entry.clone(), cx);
                }),
            )
            .child(Icon::new(icon).size(px(ROW_GLYPH)).text_color(muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(rems(TEXT))
                    .text_color(name_ink)
                    .truncate()
                    .child(name_label),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(rems(META))
                    .font_features(anterm_ui_kit::theme::tabular_figures())
                    .text_color(muted)
                    .child(size),
            )
            .context_menu(move |menu, _window, cx| {
                let danger = cx.theme().danger;
                Self::row_context_menu(menu, &menu_entry, dir_like, is_symlink, danger, &panel)
            })
            .into_any_element()
    }

    fn row_context_menu(
        menu: gpui_kit::component::menu::PopupMenu,
        entry: &SftpEntry,
        dir_like: bool,
        is_symlink: bool,
        danger: gpui::Hsla,
        panel: &gpui::WeakEntity<Self>,
    ) -> gpui_kit::component::menu::PopupMenu {
        let mut menu = menu.min_w(px(180.));

        let primary_label = if dir_like {
            t(L10nKey::SftpContextOpen)
        } else {
            t(L10nKey::SftpContextEdit)
        };
        menu = menu.item(PopupMenuItem::new(primary_label).on_click({
            let panel = panel.clone();
            let entry = entry.clone();
            move |_, _window, cx| {
                let entry = entry.clone();
                let _ = panel.update(cx, |this, cx| this.open_entry(entry, cx));
            }
        }));

        if !dir_like {
            menu = menu.item(PopupMenuItem::new(t(L10nKey::Download)).on_click({
                let panel = panel.clone();
                let entry = entry.clone();
                move |_, _window, cx| {
                    let entry = entry.clone();
                    let _ = panel.update(cx, |this, cx| this.download_entry(entry, cx));
                }
            }));
        }

        if is_symlink {
            menu = menu.item(
                PopupMenuItem::new(t(L10nKey::SftpContextFollowSymlink)).on_click({
                    let panel = panel.clone();
                    let entry = entry.clone();
                    move |_, _window, cx| {
                        let entry = entry.clone();
                        let _ = panel.update(cx, |this, cx| this.follow_symlink(entry, cx));
                    }
                }),
            );
        }

        menu = menu
            .item(PopupMenuItem::new(t(L10nKey::SftpContextRename)).on_click({
                let panel = panel.clone();
                let name = entry.name.clone();
                move |_, window, cx| {
                    let name = name.clone();
                    let _ = panel.update(cx, |this, cx| this.begin_rename(name, window, cx));
                }
            }))
            .item(PopupMenuItem::new(t(L10nKey::SftpContextChmod)).on_click({
                let panel = panel.clone();
                let entry = entry.clone();
                move |_, window, cx| {
                    let entry = entry.clone();
                    let _ = panel.update(cx, |this, cx| this.begin_chmod(entry, window, cx));
                }
            }))
            .separator();

        menu.item(
            PopupMenuItem::element(move |_window, _cx| {
                div().text_color(danger).child(t(L10nKey::Delete))
            })
            .on_click({
                let panel = panel.clone();
                let entry = entry.clone();
                move |_, window, cx| {
                    let entry = entry.clone();
                    let _ = panel.update(cx, |this, cx| this.delete_entry(entry, window, cx));
                }
            }),
        )
    }

    pub fn render_transfers_footer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.open_pane_id?;
        let history = self.show_history;
        let jobs: Vec<&SftpJobProgress> = self
            .jobs
            .iter()
            .filter(|j| history || !self.dismissed_jobs.contains(&j.job_id))
            .collect();
        let jobs_error = self.jobs_error.as_ref();
        if jobs.is_empty() && !history && jobs_error.is_none() {
            return None;
        }

        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let (ink, track) = (cx.theme().foreground, cx.theme().muted);
        let divider = cx.theme().sidebar_border;
        let hover =
            gpui::rgb(cx.global::<anterm_ui_kit::presets::Surfaces>().sidebar.hover);
        let expanded = self.tray_expanded || history;

        let running = jobs
            .iter()
            .filter(|j| matches!(j.state, SftpJobState::Running))
            .count();
        let (done, total): (u64, u64) = jobs
            .iter()
            .filter(|j| matches!(j.state, SftpJobState::Running))
            .fold((0, 0), |(d, t), j| (d + j.bytes_done, t + j.bytes_total));
        let failed = jobs
            .iter()
            .filter(|j| matches!(j.state, SftpJobState::Error))
            .count();
        let pct = if total > 0 {
            ((done as f64 / total as f64) * 100.0).min(100.0)
        } else {
            0.0
        };
        let summary = if let Some(e) = jobs_error {
            t_fmt(L10nKey::SftpTransferListFailed, &[("error", e)])
        } else if running > 0 {
            t_fmt(
                L10nKey::SftpTransferSummaryRunning,
                &[
                    ("count", &running.to_string()),
                    ("pct", &format!("{pct:.0}")),
                ],
            )
        } else if failed > 0 {
            t_fmt(
                L10nKey::SftpTransferSummaryFailed,
                &[("count", &failed.to_string())],
            )
        } else {
            t(L10nKey::SftpTransferSummaryIdle).to_string()
        };
        let summary_color = if jobs_error.is_some() || (running == 0 && failed > 0) {
            danger
        } else {
            muted
        };

        let head = h_flex()
            .id("sftp-transfers-summary")
            .items_center()
            .gap(px(6.))
            .pl(px(TEXT_INSET - TRAY_CHEVRON - 6.))
            .pr(px(CONTENT_INSET))
            .h(px(28.))
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .on_click(cx.listener(|this, _, _w, cx| this.toggle_tray(cx)))
            .child(
                Icon::new(match expanded {
                    true => IconName::ChevronDown,
                    false => IconName::ChevronRight,
                })
                .size(px(TRAY_CHEVRON))
                .text_color(muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(rems(META))
                    .font_features(anterm_ui_kit::theme::tabular_figures())
                    .text_color(summary_color)
                    .child(summary),
            )
            .child(
                div()
                    .flex_none()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        anterm_ui_kit::window_chrome::chrome_tile(
                            Button::new("sftp-tray-close")
                                .icon(IconName::Close)
                                .xsmall(),
                            false,
                            cx,
                        )
                        .w(px(anterm_ui_kit::window_chrome::MIN_TARGET))
                        .h(px(anterm_ui_kit::window_chrome::MIN_TARGET))
                        .rounded(px(anterm_ui_kit::window_chrome::RAIL_TILE_RADIUS))
                        .tooltip(t(L10nKey::Dismiss))
                        .on_click(cx.listener(|this, _, _w, cx| this.dismiss_tray(cx))),
                    ),
            );

        let underline = div().h(px(2.)).w_full().bg(track).child(
            div()
                .h_full()
                .w(gpui::relative((pct / 100.0) as f32))
                .bg(if failed > 0 { danger } else { ink }),
        );

        let body = expanded.then(|| {
            let inner: Div = if jobs.is_empty() {
                let (text, color): (SharedString, _) = match jobs_error {
                    Some(e) => (
                        t_fmt(L10nKey::SftpTransferListFailed, &[("error", e)]).into(),
                        danger,
                    ),
                    None => (t(L10nKey::SftpNoTransfers).into(), muted),
                };
                v_flex().child(
                    div()
                        .px(px(TEXT_INSET))
                        .py(px(3.))
                        .text_size(rems(META))
                        .text_color(color)
                        .child(text),
                )
            } else {
                let mut list = v_flex().px(px(TEXT_INSET)).pb(px(8.)).gap(px(8.));
                for job in jobs {
                    list = list.child(self.render_job(job, cx));
                }
                list
            };
            anterm_ui_kit::scrollbar::over_vertical_scroll(
                "sftp-transfers-scrollbar",
                div()
                    .id("sftp-transfers-list")
                    .max_h(px(200.))
                    .overflow_y_scroll()
                    .track_scroll(&self.transfers_scroll)
                    .child(inner),
                &self.transfers_scroll,
            )
        });

        Some(
            v_flex()
                .flex_none()
                .border_t_1()
                .border_color(divider)
                .child(head)
                .when(running > 0 && !expanded, |this| this.child(underline))
                .children(body)
                .into_any_element(),
        )
    }

    fn render_job(&self, job: &SftpJobProgress, cx: &mut Context<Self>) -> Div {
        let foreground = cx.theme().foreground;
        let track = cx.theme().muted;
        let danger = cx.theme().danger;
        let success = cx.theme().success;
        let muted = cx.theme().muted_foreground;
        let arrow = match job.kind {
            SftpTransferKind::Upload => "↑",
            SftpTransferKind::Download => "↓",
        };
        let name = remote_basename(&job.remote);
        let pct = if job.bytes_total > 0 {
            ((job.bytes_done as f64 / job.bytes_total as f64) * 100.0).min(100.0)
        } else {
            0.0
        };
        let status = match job.state {
            SftpJobState::Running => t_fmt(
                L10nKey::SftpTransferProgress,
                &[
                    ("done", &human_size(job.bytes_done)),
                    ("total", &human_size(job.bytes_total)),
                    ("pct", &format!("{pct:.0}")),
                ],
            ),
            SftpJobState::Done => t(L10nKey::SftpTransferDone).to_string(),
            SftpJobState::Cancelled => t(L10nKey::SftpTransferCancelled).to_string(),
            SftpJobState::Error => job
                .error
                .clone()
                .unwrap_or_else(|| t(L10nKey::SftpTransferError).to_string()),
        };
        let status_color = match job.state {
            SftpJobState::Error => danger,
            SftpJobState::Done => success,
            _ => muted,
        };
        let bar_color = if matches!(job.state, SftpJobState::Error) {
            danger
        } else {
            foreground
        };
        let job_id = job.job_id;
        let running = matches!(job.state, SftpJobState::Running);
        let done_download = matches!(job.state, SftpJobState::Done)
            && matches!(job.kind, SftpTransferKind::Download)
            && !job.local.is_empty();
        let local = job.local.clone();

        v_flex()
            .gap_0p5()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(rems(META))
                            .text_color(foreground)
                            .truncate()
                            .child(format!("{arrow} {name}")),
                    )
                    .when(done_download, |this| {
                        this.child(
                            anterm_ui_kit::window_chrome::hit_target(
                                Button::new(("sftp-reveal-job", job_id as usize))
                                    .icon(IconName::FolderOpen)
                                    .xsmall()
                                    .ghost(),
                            )
                            .tooltip(anterm_ui_kit::panel_tokens::reveal_label())
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.reveal_download(local.clone(), cx)
                            })),
                        )
                    })
                    .when(running, |this| {
                        this.child(
                            anterm_ui_kit::window_chrome::hit_target(
                                Button::new(("sftp-cancel-job", job_id as usize))
                                    .icon(IconName::Close)
                                    .xsmall()
                                    .ghost(),
                            )
                            .tooltip(t(L10nKey::Cancel))
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.cancel_job(job_id, cx)
                            })),
                        )
                    }),
            )
            .child(
                div().h(px(3.)).w_full().rounded_full().bg(track).child(
                    div()
                        .h_full()
                        .w(gpui::relative((pct / 100.0) as f32))
                        .rounded_full()
                        .bg(bar_color),
                ),
            )
            .child(
                div()
                    .text_size(rems(META))
                    .font_features(anterm_ui_kit::theme::tabular_figures())
                    .text_color(status_color)
                    .child(status),
            )
    }
}

impl Render for SftpPanelView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let breadcrumb = self.render_breadcrumb(cx);
        let filter = div()
            .id("panel-sftp-filter")
            .child(panel_search_box(&self.filter_input, None, cx))
            .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape" {
                    this.clear_filter(window, cx);
                }
            }));
        let form = self.render_edit_form(cx);
        let list = self.render_list(cx);

        v_flex()
            .id("panel-sftp")
            .flex_1()
            .min_h_0()
            .child(breadcrumb)
            .child(filter)
            .children(form)
            .child(list)
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.upload_paths(paths.paths().to_vec(), window, cx);
            }))
    }
}

/// Counts rows built by `v_virtual_list` so tests can verify that a large
/// remote directory only builds rows for the visible viewport.
#[cfg(test)]
pub mod row_probe {
    use std::cell::Cell;

    thread_local! {
        static BUILT: Cell<u64> = const { Cell::new(0) };
    }

    pub fn record() {
        BUILT.set(BUILT.get() + 1);
    }

    pub fn take() -> u64 {
        BUILT.replace(0)
    }
}
