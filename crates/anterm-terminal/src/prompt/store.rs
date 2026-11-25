//! Global, shared command history store for the GUI process.
//!
//! All open panes share one in-memory [`ScopeSnapshot`] per [`Scope`] (avoiding
//! N-pane memory duplication) backed by a single dedicated background worker
//! thread owning `<config_dir>/history.db` (`anterm_core::history::HistoryDb`).
//!
//! Guarantees:
//! - Opening a pane never performs synchronous disk I/O on the UI thread.
//! - Recording, backfilling exit codes/durations, and deleting entries never
//!   block the UI thread and propagate instantaneously to every open pane.
//! - Empty panes write zero bytes to SQLite and allocate zero session entries.
//! - Multiple SSH panes to the same remote host share a 5-minute SFTP seed TTL.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{OnceLock, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anterm_core::history::{
    HistoryDb, MAX_DB_ROWS, MAX_HOT_ENTRIES, RecordInput, Scope, ScopeSnapshot,
    should_ignore_command,
};

use anterm_ui_kit::host_ops::HostId;

/// How long a remote host's SFTP shell-history seed stays fresh so opening
/// multiple panes to the same host does not re-download `.zsh_history` /
/// `.bash_history` over SFTP each time.
const REMOTE_SEED_TTL: Duration = Duration::from_secs(5 * 60);

/// Run retention pruning after every N newly recorded commands.
const PRUNE_EVERY_N_RECORDS: u32 = 250;

/// Maximum pending `req_id -> rowid` entries kept on the worker thread.
const MAX_PENDING_ROWIDS: usize = 512;

enum DbCmd {
    LoadScope {
        scope: Scope,
        shell_files: Vec<(String, Vec<u8>)>,
        reply: smol::channel::Sender<ScopeSnapshot>,
    },
    LoadAll {
        reply: smol::channel::Sender<ScopeSnapshot>,
    },
    Record {
        req_id: u64,
        input: RecordInput,
    },
    Finish {
        req_id: u64,
        exit: Option<i32>,
        duration_ms: Option<u64>,
    },
    Delete {
        scope: Option<Scope>,
        cmd: String,
    },
    CarrySession {
        old_pane_key: String,
        new_pane_key: String,
        now: u64,
        reply: smol::channel::Sender<String>,
    },
    #[cfg(test)]
    Flush {
        reply: mpsc::Sender<()>,
    },
}

/// Lightweight, cloneable handle to the background SQLite worker thread.
///
/// Held by [`HistoryStore`] and optionally by `PendingHistory` so even a
/// `TerminalView::drop` (which has no `&mut App`) can flush an already-known
/// exit code without blocking.
#[derive(Clone)]
pub struct HistoryWorker {
    tx: mpsc::Sender<DbCmd>,
    db_path: Option<PathBuf>,
}

impl HistoryWorker {
    fn spawn(db_path: Option<PathBuf>) -> Self {
        let (tx, rx) = mpsc::channel::<DbCmd>();
        let worker_path = db_path.clone();
        let _ = std::thread::Builder::new()
            .name("anterm-history-db".to_string())
            .spawn(move || run_worker(worker_path, rx));
        Self { tx, db_path }
    }

    pub fn send_finish(&self, req_id: u64, exit: Option<i32>, duration_ms: Option<u64>) {
        let _ = self.tx.send(DbCmd::Finish {
            req_id,
            exit,
            duration_ms,
        });
    }

    pub fn request_scope_load(
        &self,
        scope: Scope,
        shell_files: Vec<(String, Vec<u8>)>,
    ) -> smol::channel::Receiver<ScopeSnapshot> {
        let (reply, rx) = smol::channel::bounded(1);
        let _ = self.tx.send(DbCmd::LoadScope {
            scope,
            shell_files,
            reply,
        });
        rx
    }

    pub fn request_all_load(&self) -> smol::channel::Receiver<ScopeSnapshot> {
        let (reply, rx) = smol::channel::bounded(1);
        let _ = self.tx.send(DbCmd::LoadAll { reply });
        rx
    }

    #[cfg(test)]
    pub fn flush_sync(&self) {
        let (tx, rx) = mpsc::channel();
        if self.tx.send(DbCmd::Flush { reply: tx }).is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(5));
        }
    }
}

fn run_worker(db_path: Option<PathBuf>, rx: mpsc::Receiver<DbCmd>) {
    let mut db = match db_path {
        Some(path) => match HistoryDb::open_at(path.clone()) {
            Ok(db) => db,
            Err(e) => {
                log::warn!(
                    target: "anterm::history",
                    "failed to open {}: {e}; falling back to in-memory history",
                    path.display()
                );
                match HistoryDb::open_in_memory() {
                    Ok(db) => db,
                    Err(_) => return,
                }
            }
        },
        None => match HistoryDb::open_in_memory() {
            Ok(db) => db,
            Err(_) => return,
        },
    };

    let mut pending_rowids: HashMap<u64, i64> = HashMap::new();
    let mut records_since_prune: u32 = 0;

    while let Ok(cmd) = rx.recv() {
        match cmd {
            DbCmd::LoadScope {
                scope,
                shell_files,
                reply,
            } => {
                if let Err(e) = db.migrate_legacy_scope(&scope) {
                    log::debug!(target: "anterm::history", "legacy migration for {scope:?}: {e}");
                }
                if scope.is_local() {
                    if let Err(e) = db.import_local_shell_files() {
                        log::debug!(target: "anterm::history", "local shell import: {e}");
                    }
                } else if !shell_files.is_empty()
                    && let Err(e) = db.import_remote_shell_files(&scope, &shell_files)
                {
                    log::debug!(target: "anterm::history", "remote shell import: {e}");
                }
                let snapshot = db
                    .load_scope_snapshot(&scope, MAX_HOT_ENTRIES)
                    .unwrap_or_default();
                let _ = reply.send_blocking(snapshot);
            }
            DbCmd::LoadAll { reply } => {
                let snapshot = db
                    .load_all_scopes_snapshot(MAX_HOT_ENTRIES)
                    .unwrap_or_default();
                let _ = reply.send_blocking(snapshot);
            }
            DbCmd::Record { req_id, input } => match db.record_command(&input) {
                Ok(Some(row_id)) => {
                    if pending_rowids.len() >= MAX_PENDING_ROWIDS
                        && let Some(&oldest) = pending_rowids.keys().min()
                    {
                        pending_rowids.remove(&oldest);
                    }
                    pending_rowids.insert(req_id, row_id);
                    records_since_prune += 1;
                    if records_since_prune >= PRUNE_EVERY_N_RECORDS {
                        records_since_prune = 0;
                        let _ = db.prune(MAX_DB_ROWS);
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    log::debug!(target: "anterm::history", "record failed: {e}");
                }
            },
            DbCmd::Finish {
                req_id,
                exit,
                duration_ms,
            } => {
                if let Some(row_id) = pending_rowids.remove(&req_id)
                    && let Err(e) = db.finish_command(row_id, exit, duration_ms)
                {
                    log::debug!(target: "anterm::history", "finish failed: {e}");
                }
            }
            DbCmd::Delete { scope, cmd } => {
                if let Err(e) = db.delete_command(scope.as_ref(), &cmd) {
                    log::debug!(target: "anterm::history", "delete failed: {e}");
                }
            }
            DbCmd::CarrySession {
                old_pane_key,
                new_pane_key,
                now,
                reply,
            } => {
                let resolved = db
                    .carry_session(&old_pane_key, &new_pane_key, now)
                    .unwrap_or(old_pane_key);
                let _ = reply.send_blocking(resolved);
            }
            #[cfg(test)]
            DbCmd::Flush { reply } => {
                let _ = reply.send(());
            }
        }
    }
}

struct ScopeState {
    snapshot: ScopeSnapshot,
    ready: bool,
    loading: bool,
    remote_seeded_at: Option<Instant>,
}

impl Default for ScopeState {
    fn default() -> Self {
        Self {
            snapshot: ScopeSnapshot::new(),
            ready: false,
            loading: false,
            remote_seeded_at: None,
        }
    }
}

#[derive(Default)]
pub struct HistoryStore {
    worker: Option<HistoryWorker>,
    scopes: HashMap<Scope, ScopeState>,
    all_snapshot: Option<ScopeSnapshot>,
    all_loading: bool,
    session_aliases: HashMap<String, String>,
    next_req_id: u64,
}

impl gpui::Global for HistoryStore {}

fn empty_snapshot() -> &'static ScopeSnapshot {
    static EMPTY: OnceLock<ScopeSnapshot> = OnceLock::new();
    EMPTY.get_or_init(ScopeSnapshot::new)
}

fn desired_db_path() -> Option<PathBuf> {
    let dir = anterm_ui_kit::config::config_dir_path()?;
    // Never touch the developer's real ~/.config/anterm/history.db from a unit
    // or GPUI test that did not call `pin_test_config_dir()`.
    #[cfg(test)]
    if anterm_ui_kit::config::default_config_dir().as_deref() == Some(dir.as_path()) {
        return None;
    }
    Some(dir.join("history.db"))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl HistoryStore {
    pub fn pane_key(host_id: HostId, pane_id: u64) -> String {
        if host_id.is_local() {
            format!("local:{pane_id}")
        } else {
            format!("h{:016x}:{pane_id}", host_id.0)
        }
    }

    pub fn ensure_worker(&mut self) -> HistoryWorker {
        let target = desired_db_path();
        if let Some(w) = &self.worker
            && w.db_path == target
        {
            return w.clone();
        }
        let worker = HistoryWorker::spawn(target);
        self.worker = Some(worker.clone());
        worker
    }

    pub fn resolve_session(&self, host_id: HostId, pane_id: u64) -> String {
        let key = Self::pane_key(host_id, pane_id);
        self.session_aliases.get(&key).cloned().unwrap_or(key)
    }

    /// Bind `(host_id, new_pane_id)` to `(host_id, old_pane_id)`'s logical
    /// history session immediately in memory and persist the alias in SQLite.
    /// Returns `(optimistic_session_id, db_resolved_rx)`.
    pub fn carry_session(
        &mut self,
        host_id: HostId,
        old_pane_id: u64,
        new_pane_id: u64,
    ) -> (String, smol::channel::Receiver<String>) {
        let old_key = Self::pane_key(host_id, old_pane_id);
        let new_key = Self::pane_key(host_id, new_pane_id);
        let optimistic = self
            .session_aliases
            .get(&old_key)
            .cloned()
            .unwrap_or_else(|| old_key.clone());
        self.session_aliases
            .insert(new_key.clone(), optimistic.clone());

        let worker = self.ensure_worker();
        let (reply, rx) = smol::channel::bounded(1);
        let _ = worker.tx.send(DbCmd::CarrySession {
            old_pane_key: old_key,
            new_pane_key: new_key,
            now: unix_now(),
            reply,
        });
        (optimistic, rx)
    }

    pub fn note_resolved_session(&mut self, host_id: HostId, pane_id: u64, session_id: String) {
        let key = Self::pane_key(host_id, pane_id);
        self.session_aliases.insert(key, session_id);
    }

    pub fn snapshot(&self, scope: &Scope) -> &ScopeSnapshot {
        match self.scopes.get(scope) {
            Some(s) => &s.snapshot,
            None => empty_snapshot(),
        }
    }

    #[cfg(test)]
    pub fn replace_scope_for_test(&mut self, scope: &Scope, mut snapshot: ScopeSnapshot) {
        let state = self.scopes.entry(scope.clone()).or_default();
        let next_gen = state
            .snapshot
            .generation
            .max(snapshot.generation)
            .wrapping_add(1);
        snapshot.generation = next_gen;
        state.snapshot = snapshot;
        state.ready = true;
        state.loading = false;
    }

    #[cfg(test)]
    pub fn snapshot_mut(&mut self, scope: &Scope) -> &mut ScopeSnapshot {
        &mut self.scopes.entry(scope.clone()).or_default().snapshot
    }

    #[cfg(test)]
    pub fn is_scope_ready(&self, scope: &Scope) -> bool {
        self.scopes.get(scope).is_some_and(|s| s.ready)
    }

    pub fn should_seed_remote(&self, scope: &Scope) -> bool {
        if scope.is_local() {
            return false;
        }
        match self.scopes.get(scope).and_then(|s| s.remote_seeded_at) {
            Some(at) => at.elapsed() >= REMOTE_SEED_TTL,
            None => true,
        }
    }

    pub fn mark_remote_seeded(&mut self, scope: &Scope) {
        self.scopes
            .entry(scope.clone())
            .or_default()
            .remote_seeded_at = Some(Instant::now());
    }

    /// Claim the right to kick off a background DB load for `scope`.
    /// Returns `None` when the scope is already loaded (or loading) and
    /// `force` is false, preventing N startup panes from queuing N duplicate
    /// loads.
    pub fn begin_scope_load(
        &mut self,
        scope: &Scope,
        force: bool,
    ) -> Option<HistoryWorker> {
        let already_handled = self
            .scopes
            .get(scope)
            .is_some_and(|s| !force && (s.ready || s.loading));
        if already_handled {
            return None;
        }
        self.scopes.entry(scope.clone()).or_default().loading = true;
        Some(self.ensure_worker())
    }

    /// Merge a completed background DB load into `scope`, preserving any
    /// commands run in-flight while the load was executing so a late load never
    /// erases just-executed commands.
    pub fn install_loaded_scope(&mut self, scope: &Scope, loaded: ScopeSnapshot) {
        let state = self.scopes.entry(scope.clone()).or_default();
        let prev_gen = state.snapshot.generation;
        let previous = std::mem::replace(&mut state.snapshot, loaded);

        if !previous.entries.is_empty() {
            let known: HashSet<String> = state.snapshot.entries.iter().cloned().collect();
            for line in previous.entries {
                if known.contains(&line) {
                    // Keep any exit/duration already learned in-flight if the
                    // loaded row didn't have it yet.
                    if let Some(prev_m) = previous.meta.get(&line)
                        && let Some(cur_m) = state.snapshot.meta.get_mut(&line)
                    {
                        if cur_m.exit.is_none() && prev_m.exit.is_some() {
                            cur_m.exit = prev_m.exit;
                        }
                        if cur_m.duration_ms.is_none() && prev_m.duration_ms.is_some() {
                            cur_m.duration_ms = prev_m.duration_ms;
                        }
                    }
                    continue;
                }
                if let Some(&n) = previous.counts.get(&line) {
                    state.snapshot.counts.insert(line.clone(), n);
                }
                if let Some(dirs) = previous.cwds.get(&line) {
                    state.snapshot.cwds.insert(line.clone(), dirs.clone());
                }
                if let Some(&m) = previous.meta.get(&line) {
                    state.snapshot.meta.insert(line.clone(), m);
                }
                state.snapshot.entries.push(line);
            }
            for (sess, cmds) in previous.sessions {
                let target = state.snapshot.sessions.entry(sess).or_default();
                let target_set: HashSet<String> = target.iter().cloned().collect();
                for cmd in cmds {
                    if !target_set.contains(&cmd) {
                        target.push(cmd);
                    }
                }
            }
        }
        state.snapshot.generation = prev_gen.max(state.snapshot.generation).wrapping_add(1);
        state.ready = true;
        state.loading = false;
    }

    pub fn all_snapshot(&self) -> Option<&ScopeSnapshot> {
        self.all_snapshot.as_ref()
    }

    pub fn begin_all_load(&mut self) -> Option<HistoryWorker> {
        if self.all_loading {
            return None;
        }
        self.all_loading = true;
        Some(self.ensure_worker())
    }

    pub fn install_all_snapshot(&mut self, mut loaded: ScopeSnapshot) {
        let prev_gen = self
            .all_snapshot
            .as_ref()
            .map(|s| s.generation)
            .unwrap_or(0);
        loaded.generation = prev_gen.max(loaded.generation).wrapping_add(1);
        self.all_snapshot = Some(loaded);
        self.all_loading = false;
    }

    /// Optimistically record a command into the shared in-memory snapshot and
    /// enqueue durable persistence on the SQLite worker thread.
    ///
    /// Returns `None` if `ignorespace` or blank-line privacy rules exclude it.
    pub fn record(&mut self, input: RecordInput) -> Option<(u64, HistoryWorker)> {
        if should_ignore_command(&input.cmd) {
            return None;
        }
        self.next_req_id = self.next_req_id.wrapping_add(1).max(1);
        let req_id = self.next_req_id;

        let state = self.scopes.entry(input.scope.clone()).or_default();
        state.snapshot.apply_record(
            &input.session_id,
            input.cmd.clone(),
            input.cwd.clone(),
            input.ts,
            input.exit,
            input.duration_ms,
        );
        if let Some(all) = self.all_snapshot.as_mut() {
            all.apply_record(
                &input.session_id,
                input.cmd.clone(),
                input.cwd.clone(),
                input.ts,
                input.exit,
                input.duration_ms,
            );
        }

        let worker = self.ensure_worker();
        let _ = worker.tx.send(DbCmd::Record { req_id, input });
        Some((req_id, worker))
    }

    /// Backfill exit code and elapsed duration for an already-recorded command.
    pub fn finish(
        &mut self,
        req_id: u64,
        scope: &Scope,
        cmd: &str,
        exit: Option<i32>,
        duration_ms: Option<u64>,
    ) {
        if let Some(state) = self.scopes.get_mut(scope) {
            state.snapshot.update_finish(cmd, exit, duration_ms);
        }
        if let Some(all) = self.all_snapshot.as_mut() {
            all.update_finish(cmd, exit, duration_ms);
        }
        let worker = self.ensure_worker();
        worker.send_finish(req_id, exit, duration_ms);
    }

    /// Delete a command from memory and SQLite (either for one scope or across
    /// all scopes when `scope` is `None`).
    pub fn delete(&mut self, scope: Option<&Scope>, cmd: &str) {
        match scope {
            Some(sc) => {
                if let Some(state) = self.scopes.get_mut(sc) {
                    state.snapshot.remove_command(cmd);
                }
            }
            None => {
                for state in self.scopes.values_mut() {
                    state.snapshot.remove_command(cmd);
                }
            }
        }
        if let Some(all) = self.all_snapshot.as_mut() {
            all.remove_command(cmd);
        }
        let worker = self.ensure_worker();
        let _ = worker.tx.send(DbCmd::Delete {
            scope: scope.cloned(),
            cmd: cmd.to_string(),
        });
    }
}
