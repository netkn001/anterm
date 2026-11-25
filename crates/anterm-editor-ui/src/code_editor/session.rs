//! Unsaved-change prompts, save waiters, orphan adoption, and editor session persistence.

use std::collections::HashSet;
use std::path::PathBuf;

use anterm_core::machine::TabId;
use anterm_i18n::{L10nKey, t, t_fmt};
use gpui::{Context, PromptLevel, SharedString, Window};
use gpui_kit::component::WindowExt as _;

use super::io::{Loaded, load_file};
use super::{AfterUnsaved, BufferId, OpenFile, SaveWaiter, TabCode};
use crate::editor_session::{self, TabEditor};
use crate::{CodeEditorEvent, CodeEditorWorkspace};
use anterm_ui_kit::host_ops::HostOps;

impl CodeEditorWorkspace {
    // ---- Unsaved changes standing in the way of a close ----

    /// Asks about unsaved buffers before `then` goes ahead. Returns `true`
    /// when there was something to ask about: the caller stops there, and the
    /// answer carries the close on — after saving, after discarding, or not
    /// at all.
    pub fn editor_guard_unsaved(
        &mut self,
        ids: Vec<BufferId>,
        then: AfterUnsaved,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if ids.is_empty() {
            return false;
        }
        if self.editor.unsaved_prompt_open {
            return true;
        }
        self.editor.unsaved_prompt_open = true;
        let names: Vec<SharedString> = ids
            .iter()
            .filter_map(|id| self.buffer(*id).map(OpenFile::label))
            .collect();
        let title = match names.as_slice() {
            [one] => t_fmt(L10nKey::EditorUnsavedChanges, &[("name", one)]),
            _ => t_fmt(
                L10nKey::EditorUnsavedChangesMany,
                &[("count", &names.len().to_string())],
            ),
        };
        let mut body: Vec<String> = names.iter().take(8).map(|n| n.to_string()).collect();
        if names.len() > 8 {
            body.push("…".into());
        }
        let body = (names.len() > 1).then(|| body.join("\n"));
        let answer = window.prompt(
            PromptLevel::Warning,
            &title,
            body.as_deref(),
            // The same arrangement as closing a single file, and for the same
            // reason: Discard as far from Return as the dialog allows.
            &[
                gpui::PromptButton::ok(if names.len() > 1 {
                    t(L10nKey::EditorSaveAll)
                } else {
                    t(L10nKey::Save)
                }),
                gpui::PromptButton::cancel(t(L10nKey::Cancel)),
                gpui::PromptButton::ok(t(L10nKey::EditorDiscard)),
            ],
            cx,
        );
        // Showing it is part of the question: the file being asked about may
        // be behind the terminal, in a tab that is not the one in front.
        if let Some(tab_id) = ids.first().and_then(|id| {
            self.tabs
                .iter()
                .find_map(|(&tid, t)| t.code.as_deref().is_some_and(|c| c.shows(*id)).then_some(tid))
        }) && Some(tab_id) == self.active_tab_id()
            && let Some(code) = self.tab_code_mut()
            && let Some(pos) = {
                if !code.files.contains(&ids[0]) {
                    code.swap_focus();
                }
                code.files.iter().position(|f| *f == ids[0])
            }
        {
            code.active = pos;
            code.visible = true;
        }
        cx.spawn_in(window, async move |app, cx| {
            let choice = answer.await;
            let _ = app.update_in(cx, |app, window, cx| {
                app.editor.unsaved_prompt_open = false;
                match choice {
                    Ok(0) => {
                        app.editor.waiters.push(SaveWaiter {
                            ids: ids.clone(),
                            then,
                        });
                        for id in ids {
                            app.editor_save_file(id, false, false, window, cx);
                        }
                    }
                    Ok(2) => {
                        for id in ids {
                            app.editor_drop_buffer(id, cx);
                        }
                        app.editor_continue(then, window, cx);
                    }
                    _ => cx.notify(),
                }
            });
        })
        .detach();
        true
    }

    /// Runs whatever close was waiting on saves that have now all landed.
    pub(super) fn editor_saves_landed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (ready, waiting): (Vec<SaveWaiter>, Vec<SaveWaiter>) =
            std::mem::take(&mut self.editor.waiters)
                .into_iter()
                .partition(|w| {
                    w.ids.iter().all(|id| {
                        self.buffer(*id)
                            .is_none_or(|f| !f.dirty && !f.saving && f.untitled.is_none())
                    })
                });
        self.editor.waiters = waiting;
        for w in ready {
            self.editor_continue(w.then, window, cx);
        }
    }

    /// A save that was part of a close did not happen, so neither does the
    /// close: whatever it would have thrown away is still unsaved.
    pub(super) fn editor_saves_failed(&mut self, id: BufferId) {
        self.editor.waiters.retain(|w| !w.ids.contains(&id));
    }

    fn editor_continue(&mut self, then: AfterUnsaved, _window: &mut Window, cx: &mut Context<Self>) {
        match then {
            AfterUnsaved::CloseFiles(tab, ids) => {
                for id in ids {
                    self.editor_remove_from_tab(tab, id, cx);
                }
            }
            other => cx.emit(CodeEditorEvent::ContinueAfterUnsaved(other)),
        }
    }

    // ---- Keeping the registry, the tabs and the session store in step ----

    /// Once per frame: buffers no tab shows any more are dropped — or, if
    /// they hold unsaved work, handed to the tab in front rather than lost —
    /// the active tab gets back the files it had open last time, and any
    /// tab whose files changed is recorded for next time.
    ///
    /// Done here rather than at each place a tab can go — a close, a shell
    /// that exits, a tab dragged into another, a workspace switch, a server
    /// restart — because this sees all of them, including ones added later.
    pub fn editor_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A tab that left and came back under the same id — a server restart
        // rebuilds every tab that way — gets its files back like a relaunch.
        if self.editor.restored.len() > self.ctx.live_tabs.len() {
            let live: HashSet<TabId> = self.ctx.live_tabs.iter().copied().collect();
            self.editor.restored.retain(|id| live.contains(id));
        }
        self.editor_adopt_orphans(window, cx);
        self.editor_restore_active(window, cx);
        self.editor_record_sessions(cx);
    }

    fn editor_adopt_orphans(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // No tabs at all is the gap in the middle of a server restart or a
        // workspace switch, not a verdict on the buffers: wait for the tabs.
        if self.editor.buffers.is_empty() || self.ctx.live_tabs.is_empty() {
            return;
        }
        let shown: HashSet<BufferId> = self
            .tabs
            .values()
            .filter_map(|t| t.code.as_deref())
            .flat_map(TabCode::all_files)
            .collect();
        let orphans: Vec<(BufferId, bool)> = self
            .editor
            .buffers
            .iter()
            .filter(|b| !shown.contains(&b.id()))
            .map(|b| (b.id(), b.dirty))
            .collect();
        if orphans.is_empty() {
            return;
        }
        let mut changed = false;
        for (id, dirty) in orphans {
            if !dirty {
                self.editor.buffers.retain(|b| b.id() != id);
                self.editor.nav.forget_buffer(id);
                changed = true;
                continue;
            }
            // Nowhere to put it (every tab gone mid-restart): keep it, and it
            // is adopted on a later frame.
            let Some(tab) = self.active_tab_id() else {
                continue;
            };
            let name = self.buffer(id).map(OpenFile::label).unwrap_or_default();
            self.editor_show_in_tab(tab, id, false, window, cx);
            window.push_notification(t_fmt(L10nKey::EditorOrphanAdopted, &[("name", &name)]), cx);
            changed = true;
        }
        if changed {
            self.editor_rebuild_watcher(cx);
            cx.notify();
        }
    }

    pub(super) fn editor_restore_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        if !self.editor.restored.insert(tab_id) {
            return;
        }
        if self
            .tabs
            .get(&tab_id)
            .and_then(|t| t.code.as_deref())
            .is_some_and(|c| !c.files.is_empty())
        {
            return;
        }
        let Some(state) = editor_session::get(cx, tab_id) else {
            return;
        };
        // The left group can be empty while the right is not: it held only
        // files that are not recorded (untitled, remote).
        if state.files.is_empty() && state.split.as_ref().is_none_or(|s| s.files.is_empty()) {
            return;
        }
        let Some(host) = self.active_host(cx) else {
            return;
        };
        // What was recorded is what is being put back; recording it again
        // before the files have loaded would write down an empty tab.
        self.editor.recorded.insert(tab_id, state.clone());
        // Both groups' files in one trip, the left group's first.
        let files: Vec<PathBuf> = state
            .files
            .iter()
            .chain(state.split.iter().flat_map(|s| s.files.iter()))
            .cloned()
            .collect();
        let requested = files.clone();
        HostOps::run_in(
            host.clone(),
            window,
            cx,
            move |h| {
                files
                    .into_iter()
                    .map(|p| load_file(h, p).ok())
                    .collect::<Vec<_>>()
            },
            move |app, loaded: Vec<Option<Loaded>>, window, cx| {
                let mut opened: Vec<(PathBuf, BufferId)> = Vec::new();
                for (path, l) in requested.into_iter().zip(loaded) {
                    if let Some(l) = l {
                        let id = app.editor_install(host.clone(), l, tab_id, false, window, cx);
                        opened.push((path, id));
                    }
                }
                let group = |paths: &[PathBuf], active: usize| -> (Vec<BufferId>, usize) {
                    let front = paths.get(active);
                    let mut ids: Vec<BufferId> = Vec::new();
                    let mut at = 0;
                    for path in paths {
                        let Some(&(_, id)) = opened.iter().find(|(p, _)| p == path) else {
                            continue;
                        };
                        if ids.contains(&id) {
                            continue;
                        }
                        if Some(path) == front {
                            at = ids.len();
                        }
                        ids.push(id);
                    }
                    (ids, at)
                };
                let (left, left_active) = group(&state.files, state.active);
                let right = state
                    .split
                    .as_ref()
                    .map(|s| (group(&s.files, s.active), s.focused));
                let Some(code) = app.tabs.get_mut(&tab_id).and_then(|t| t.code.as_deref_mut()) else {
                    return;
                };
                let restored: Vec<BufferId> = opened.iter().map(|(_, id)| *id).collect();
                // Merged into what the tab has now, not written over it: a
                // file opened, or a split made, while these were loading stays.
                let untouched = code.restore_groups(
                    &restored,
                    (left, left_active),
                    right.map(|((files, active), focused)| (files, active, focused)),
                );
                let visible = state.visible && !code.files.is_empty();
                code.visible |= visible;
                // Only a restore nobody has touched takes the keyboard; one the
                // reader has moved on from leaves it where they put it.
                if untouched && Some(tab_id) == app.active_tab_id() && visible {
                    app.focus_editor(window, cx);
                }
                cx.notify();
            },
        );
    }

    pub(super) fn editor_record_sessions(&mut self, cx: &mut Context<Self>) {
        let spawn_host = self.spawn_host(cx);
        let mut changed: Vec<(TabId, TabEditor)> = Vec::new();
        for (&tab_id, tab) in &self.tabs {
            let Some(code) = tab.code.as_deref() else {
                continue;
            };
            // A tab still waiting for its files to come back has nothing to
            // say about them yet.
            if !self.editor.restored.contains(&tab_id) {
                continue;
            }
            // Only files on the window's own machine: an SFTP buffer's host
            // is a connection that will not exist next launch.
            let group = |ids: &[BufferId], front: usize| -> (Vec<PathBuf>, usize) {
                let mut files = Vec::new();
                let mut active = 0;
                for (pos, id) in ids.iter().enumerate() {
                    let Some(f) = self.buffer(*id) else { continue };
                    if f.untitled.is_some() || f.host.id() != spawn_host {
                        continue;
                    }
                    if pos == front {
                        active = files.len();
                    }
                    files.push(f.path.clone());
                }
                (files, active)
            };
            let sides = code.sides();
            let (files, active) = group(&sides[0].files, sides[0].active);
            let split = sides.get(1).and_then(|right| {
                let (files, active) = group(&right.files, right.active);
                (!files.is_empty()).then_some(editor_session::SplitEditor {
                    files,
                    active,
                    focused: right.focused,
                })
            });
            let state = TabEditor {
                files,
                active,
                visible: code.visible,
                split,
            };
            if self.editor.recorded.get(&tab_id) != Some(&state) {
                changed.push((tab_id, state));
            }
        }
        for (tab_id, state) in changed {
            self.editor.recorded.insert(tab_id, state.clone());
            editor_session::put(cx, tab_id, state);
        }
    }

    /// A tab is being closed: the files only it showed go with it. Unsaved
    /// ones were asked about before the close got here — except when a shell
    /// exiting took the tab, which cannot ask; those stay, and are handed to
    /// the tab in front.
    pub fn editor_close_tab_files(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let Some(ids) = self
            .tabs
            .get(&tab)
            .and_then(|t| t.code.as_deref())
            .map(TabCode::all_files)
        else {
            return;
        };
        for id in ids {
            if self.buffer_tabs(id) == 1 && self.buffer(id).is_some_and(|b| !b.dirty) {
                self.editor_drop_buffer(id, cx);
            }
        }
    }

    /// A tab was closed for good: forget what it had open.
    pub fn editor_forget_tab(&mut self, tab: TabId, cx: &mut Context<Self>) {
        self.editor.recorded.remove(&tab);
        self.editor.nav.forget_tab(tab);
        editor_session::remove(cx, tab);
    }
}
