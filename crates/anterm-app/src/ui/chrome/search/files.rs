//! Workspace wiring for quick-open file indexing (`AntermApp` methods).
//! The pure file index, walker, ranker, and store live in `anterm_ui_kit::search::files`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use gpui::{App, Context, Window};

pub(crate) use anterm_ui_kit::search::files::*;

use super::SearchTab;
use crate::ui::remote::host_ops::HostOps;
use crate::ui::workspace::app::AntermApp;

impl AntermApp {
    /// Search Everywhere on its Files tab — or closed, when that is what it
    /// is already showing, so the chord that opens it also puts it away.
    pub(crate) fn quick_open_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.search.clone() {
            if view.read(cx).tab() == SearchTab::Files {
                self.close_search(window, cx);
            } else {
                view.update(cx, |view, cx| {
                    view.set_tab(SearchTab::Files, None, window, cx)
                });
            }
            return;
        }
        self.open_search(SearchTab::Files, "", window, cx);
    }

    /// Opens a row of the Files tab, and has the tree follow along the way a
    /// file link in the grid does.
    pub(crate) fn open_indexed_file(
        &mut self,
        path: &Path,
        line: Option<u32>,
        column: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_file_in_editor_at(path, line, column, window, cx);
        self.file_tree_reveal_path(path, cx);
    }

    fn file_index_key(&self, cx: &App) -> IndexKey {
        let host = self.spawn_host(cx);
        let mut cwds: Vec<PathBuf> = self
            .tabs
            .get(self.active)
            .map(|tab| tab.pane.terminals())
            .unwrap_or_default()
            .iter()
            .filter(|leaf| leaf.read(cx).host_id() == host)
            // `files_cwd`, for the reason the Files panel roots itself there:
            // it follows an agent into its worktree, and it spells a WSL
            // pane's directory the way the host can read it.
            .filter_map(|leaf| leaf.read(cx).files_cwd())
            .collect();
        cwds.sort();
        cwds.dedup();
        IndexKey { host, cwds }
    }

    /// What the Files tab opens on: the last walk, if it was of the folders
    /// the panes are in now.
    pub(crate) fn file_list_now(&self, cx: &App) -> FileList {
        let key = self.file_index_key(cx);
        if key.cwds.is_empty() {
            return FileList::NoRoots;
        }
        match &self.right_panel.read(cx).file_tree.read(cx).quick_open.last {
            Some(last) if last.key == key => last.list.clone(),
            _ => FileList::Indexing,
        }
    }

    /// Walks the project again, unless a walk of it just landed or is
    /// already on its way, and hands the result to the open search.
    pub(crate) fn refresh_file_index(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.file_index_key(cx);
        if key.cwds.is_empty() {
            return;
        }
        let file_tree = self.right_panel.read(cx).file_tree.clone();
        {
            let store = &file_tree.read(cx).quick_open;
            if store.walking.as_ref().is_some_and(|(_, k)| *k == key) {
                return;
            }
            if store
                .last
                .as_ref()
                .is_some_and(|l| l.key == key && l.at.elapsed() < REVALIDATE_AFTER)
            {
                return;
            }
        }
        let Some(host) = self.active_host(cx) else {
            return;
        };
        let home = key
            .host
            .is_local()
            .then(|| std::env::var_os("HOME").map(PathBuf::from))
            .flatten();
        let generation = file_tree.update(cx, |ft, _| {
            let store = &mut ft.quick_open;
            store.generation += 1;
            let generation = store.generation;
            store.walking = Some((generation, key.clone()));
            generation
        });
        let cwds = key.cwds.clone();
        HostOps::run_in(
            host,
            window,
            cx,
            move |h| walk(h, &cwds, home.as_deref()),
            move |app, list, window, cx| {
                let file_tree = app.right_panel.read(cx).file_tree.clone();
                let stale = file_tree.update(cx, |ft, _| {
                    let store = &mut ft.quick_open;
                    if store.walking.as_ref().map(|(g, _)| *g) != Some(generation) {
                        return true;
                    }
                    store.walking = None;
                    let keep_old = matches!(list, FileList::Failed)
                        && store
                            .last
                            .as_ref()
                            .is_some_and(|l| l.key == key && matches!(l.list, FileList::Ready(_)));
                    if !keep_old {
                        store.last = Some(Walked {
                            key: key.clone(),
                            list,
                            at: Instant::now(),
                        });
                    }
                    false
                });
                if stale || app.file_index_key(cx) != key {
                    return;
                }
                let list = app.file_list_now(cx);
                if let Some(view) = app.search.clone() {
                    view.update(cx, |view, cx| view.set_files(list, window, cx));
                }
            },
        );
    }
}
