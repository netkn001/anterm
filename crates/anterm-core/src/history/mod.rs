//! Unified, SQLite-backed command history engine (Atuin / Warp architecture).
//!
//! All scopes (local and every remote host) and all panes share one WAL-mode
//! SQLite database (`<config_dir>/history.db`, mode `0600`). Per-pane history is
//! modeled as a lightweight logical `session_id` column rather than copying
//! multi-thousand-line files per pane:
//!
//! - Opening and closing panes writes **zero** bytes unless the user actually
//!   executes a command.
//! - Multi-line commands are stored verbatim.
//! - Native shell histories (`zsh`, `bash`, `fish`, `PSReadLine`) are imported
//!   incrementally using `(mtime, file_size)` cursors without double-counting
//!   commands already recorded by anterm.
//! - Retention pruning keeps total database size strictly bounded.

pub mod db;
pub mod fuzzy;
pub mod parsers;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub use db::HistoryDb;
pub use fuzzy::{FuzzyMatch, FuzzyMatcher, match_line};
pub use parsers::{
    FISH_HISTORY_FILE, FISH_HISTORY_UNDER_DATA_DIR, FISH_HISTORY_UNDER_HOME,
    PSREADLINE_HISTORY_FILE, PSREADLINE_HISTORY_UNDER_APPDATA, PSREADLINE_HISTORY_UNDER_DATA_DIR,
    PSREADLINE_HISTORY_UNDER_HOME, REPORTED_HISTORY_FILES, RawEntry, decode_history_file,
    local_shell_history_paths, parse_history_file, parse_own_line, parse_reported_history_files,
    shell_history_name_for_shell, shell_history_names,
};

/// Maximum distinct commands kept in the in-memory hot snapshot per scope.
pub const MAX_HOT_ENTRIES: usize = 10_000;

/// Default hard row cap in SQLite before oldest cold rows are pruned.
pub const MAX_DB_ROWS: usize = 100_000;

const FREQ_WEIGHT: f64 = 0.6;
const CWD_BONUS: f64 = 1.2;
const SESSION_BONUS: f64 = 0.8;

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct EntryMeta {
    pub ts: Option<u64>,
    pub exit: Option<i32>,
    pub duration_ms: Option<u64>,
}

/// Which host/environment a command ran on.
#[derive(Clone, Default, PartialEq, Eq, Hash, Debug)]
pub enum Scope {
    #[default]
    Local,
    Remote(String),
}

impl Scope {
    pub fn remote(label: &str) -> Scope {
        let label = label.trim();
        if label.is_empty() {
            Scope::Local
        } else {
            Scope::Remote(label.to_string())
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Scope::Local)
    }

    /// Canonical SQLite key for this scope.
    pub fn key(&self) -> String {
        match self {
            Scope::Local => "local".to_string(),
            Scope::Remote(label) => format!("remote:{label}"),
        }
    }

    /// Where anterm used to keep this scope's flat TSV file before the SQLite
    /// migration. Used only for one-time automatic migration.
    pub fn legacy_file(&self) -> Option<PathBuf> {
        match self {
            Scope::Local => crate::config::config_path("history"),
            Scope::Remote(label) => {
                crate::config::config_path("history.d").map(|d| d.join(legacy_file_stem(label)))
            }
        }
    }
}

pub fn legacy_file_stem(label: &str) -> String {
    let mut safe: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    safe.truncate(48);
    format!("{safe}-{:016x}", crate::host::fnv1a64(label.as_bytes()))
}

/// Filter dimension in the Ctrl+R reverse search overlay (Atuin / Warp style).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum HistoryFilter {
    /// Only commands executed in the current pane's logical session.
    Session,
    /// Only commands executed in the current working directory on this host.
    Directory,
    /// All commands executed on the current host (local or remote target).
    #[default]
    Host,
    /// All commands across every host/scope in the database.
    All,
}

impl HistoryFilter {
    pub const ALL_MODES: [HistoryFilter; 4] = [
        HistoryFilter::Session,
        HistoryFilter::Directory,
        HistoryFilter::Host,
        HistoryFilter::All,
    ];

    pub fn next(self) -> Self {
        match self {
            Self::Session => Self::Directory,
            Self::Directory => Self::Host,
            Self::Host => Self::All,
            Self::All => Self::Session,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Session => Self::All,
            Self::Directory => Self::Session,
            Self::Host => Self::Directory,
            Self::All => Self::Host,
        }
    }
}

/// Input payload for recording a freshly submitted command.
#[derive(Clone, Debug)]
pub struct RecordInput {
    pub scope: Scope,
    pub session_id: String,
    pub cmd: String,
    pub cwd: Option<String>,
    pub ts: u64,
    pub duration_ms: Option<u64>,
    pub exit: Option<i32>,
}

/// Whether a command line should be excluded from persistence.
///
/// Honors the universal shell `ignorespace` convention: commands starting with
/// leading ASCII space or tab (commonly used to keep secrets/tokens out of
/// history) and blank lines are never stored.
pub fn should_ignore_command(cmd: &str) -> bool {
    cmd.starts_with([' ', '\t']) || cmd.trim().is_empty()
}

/// Shared, read-mostly hot history snapshot for one [`Scope`] (or all scopes),
/// wrapped in `Arc` by the GUI so N open panes share 1 copy in memory.
#[derive(Clone, Default, Debug)]
pub struct ScopeSnapshot {
    /// Distinct commands ordered oldest → newest (capped at [`MAX_HOT_ENTRIES`]).
    pub entries: Vec<String>,
    /// Execution frequency per distinct command.
    pub counts: HashMap<String, u32>,
    /// Working directories where each command has been run.
    pub cwds: HashMap<String, HashSet<String>>,
    /// Most recent execution metadata (timestamp, exit code, duration).
    pub meta: HashMap<String, EntryMeta>,
    /// Distinct commands per logical pane `session_id`, ordered oldest → newest.
    /// Empty panes have no map entry at all (zero overhead).
    pub sessions: HashMap<String, Vec<String>>,
    /// Monotonically bumped on every mutation so views know when to invalidate
    /// derived caches (such as inline ghost suggestions).
    pub generation: u64,
}

impl ScopeSnapshot {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a deduplicated, timestamp-ordered snapshot from raw entries.
    pub fn from_raw(mut raw: Vec<RawEntry>, max_hot: usize) -> Self {
        raw.sort_by(|a, b| match (a.ts, b.ts) {
            (Some(ta), Some(tb)) => ta.cmp(&tb),
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
        });

        let mut counts: HashMap<String, u32> = HashMap::new();
        let mut cwds: HashMap<String, HashSet<String>> = HashMap::new();
        let mut meta: HashMap<String, EntryMeta> = HashMap::new();
        let mut sessions: HashMap<String, Vec<String>> = HashMap::new();
        let mut session_seen: HashMap<String, HashSet<String>> = HashMap::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<String> = Vec::new();

        for r in raw.into_iter().rev() {
            let line = r.cmd.trim_end_matches('\r');
            if line.is_empty() {
                continue;
            }
            let owned = line.to_string();
            *counts.entry(owned.clone()).or_insert(0) += 1;
            if let Some(cwd) = r.cwd {
                cwds.entry(owned.clone()).or_default().insert(cwd);
            }
            // Merge metadata from newest to oldest: prefer the newest run's ts,
            // and backfill exit/duration if a slightly newer shell-import row
            // lacked them while an immediately adjacent anterm row had them.
            if r.ts.is_some() || r.exit.is_some() || r.duration_ms.is_some() {
                match meta.get_mut(&owned) {
                    None => {
                        meta.insert(
                            owned.clone(),
                            EntryMeta {
                                ts: r.ts,
                                exit: r.exit,
                                duration_ms: r.duration_ms,
                            },
                        );
                    }
                    Some(existing) => {
                        if existing.exit.is_none()
                            && r.exit.is_some()
                            && match (existing.ts, r.ts) {
                                (Some(a), Some(b)) => a.abs_diff(b) <= 5,
                                _ => true,
                            }
                        {
                            existing.exit = r.exit;
                        }
                        if existing.duration_ms.is_none()
                            && r.duration_ms.is_some()
                            && match (existing.ts, r.ts) {
                                (Some(a), Some(b)) => a.abs_diff(b) <= 5,
                                _ => true,
                            }
                        {
                            existing.duration_ms = r.duration_ms;
                        }
                    }
                }
            }
            if let Some(sess) = r.session_id.filter(|s| !s.is_empty()) {
                let s_seen = session_seen.entry(sess.clone()).or_default();
                if s_seen.insert(owned.clone()) {
                    sessions.entry(sess).or_default().push(owned.clone());
                }
            }
            if seen.insert(owned.clone()) {
                out.push(owned);
            }
        }

        out.reverse();
        for list in sessions.values_mut() {
            list.reverse();
        }
        if out.len() > max_hot {
            let cut = out.len() - max_hot;
            let mut dropped: HashSet<String> = HashSet::with_capacity(cut);
            for r in out.drain(0..cut) {
                counts.remove(&r);
                cwds.remove(&r);
                meta.remove(&r);
                dropped.insert(r);
            }
            for list in sessions.values_mut() {
                list.retain(|cmd| !dropped.contains(cmd));
            }
            sessions.retain(|_, list| !list.is_empty());
        }

        Self {
            entries: out,
            counts,
            cwds,
            meta,
            sessions,
            generation: 1,
        }
    }

    /// Incrementally record a command into this snapshot (used for instant
    /// optimistic UI updates before SQLite acknowledges the write).
    pub fn apply_record(
        &mut self,
        session_id: &str,
        cmd: String,
        cwd: Option<String>,
        ts: u64,
        exit: Option<i32>,
        duration_ms: Option<u64>,
    ) {
        if cmd.trim().is_empty() {
            return;
        }
        *self.counts.entry(cmd.clone()).or_insert(0) += 1;
        if let Some(dir) = cwd {
            self.cwds.entry(cmd.clone()).or_default().insert(dir);
        }
        self.meta.insert(
            cmd.clone(),
            EntryMeta {
                ts: Some(ts),
                exit,
                duration_ms,
            },
        );
        self.entries.retain(|h| *h != cmd);
        self.entries.push(cmd.clone());
        if !session_id.is_empty() {
            let list = self.sessions.entry(session_id.to_string()).or_default();
            list.retain(|h| *h != cmd);
            list.push(cmd);
        }
        if self.entries.len() > MAX_HOT_ENTRIES {
            let cut = self.entries.len() - MAX_HOT_ENTRIES;
            for dropped in self.entries.drain(0..cut) {
                self.counts.remove(&dropped);
                self.cwds.remove(&dropped);
                self.meta.remove(&dropped);
                for list in self.sessions.values_mut() {
                    list.retain(|c| *c != dropped);
                }
            }
            self.sessions.retain(|_, list| !list.is_empty());
        }
        self.generation = self.generation.wrapping_add(1);
    }

    /// Backfill the exit code and duration once a running command finishes.
    pub fn update_finish(
        &mut self,
        cmd: &str,
        exit: Option<i32>,
        duration_ms: Option<u64>,
    ) -> bool {
        if let Some(m) = self.meta.get_mut(cmd) {
            let mut changed = false;
            if exit.is_some() && m.exit != exit {
                m.exit = exit;
                changed = true;
            }
            if duration_ms.is_some() && m.duration_ms != duration_ms {
                m.duration_ms = duration_ms;
                changed = true;
            }
            if changed {
                self.generation = self.generation.wrapping_add(1);
            }
            return changed;
        }
        false
    }

    /// Remove a command everywhere in this snapshot (after user deletes it).
    pub fn remove_command(&mut self, cmd: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e != cmd);
        if self.entries.len() == before {
            return false;
        }
        self.counts.remove(cmd);
        self.cwds.remove(cmd);
        self.meta.remove(cmd);
        for list in self.sessions.values_mut() {
            list.retain(|e| e != cmd);
        }
        self.sessions.retain(|_, list| !list.is_empty());
        self.generation = self.generation.wrapping_add(1);
        true
    }

    /// Commands run in `session_id`, oldest → newest.
    pub fn session_entries(&self, session_id: &str) -> &[String] {
        self.sessions
            .get(session_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Compute frecency scores aligned with `self.entries`, optionally boosting
    /// commands run in `cwd` and/or in `session_id`.
    pub fn frecency_scores(&self, cwd: Option<&str>, session_id: Option<&str>) -> Vec<f64> {
        let session_set: Option<HashSet<&str>> = session_id
            .and_then(|sid| self.sessions.get(sid))
            .filter(|list| !list.is_empty())
            .map(|list| list.iter().map(String::as_str).collect());
        frecency_scores_with_session(
            &self.entries,
            &self.counts,
            &self.cwds,
            cwd,
            session_set.as_ref(),
        )
    }
}

pub fn frecency_scores(
    entries: &[String],
    counts: &HashMap<String, u32>,
    cwds: &HashMap<String, HashSet<String>>,
    cwd: Option<&str>,
) -> Vec<f64> {
    frecency_scores_with_session(entries, counts, cwds, cwd, None)
}

fn frecency_scores_with_session(
    entries: &[String],
    counts: &HashMap<String, u32>,
    cwds: &HashMap<String, HashSet<String>>,
    cwd: Option<&str>,
    session_cmds: Option<&HashSet<&str>>,
) -> Vec<f64> {
    let n = entries.len();
    entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let recency = if n <= 1 {
                1.0
            } else {
                i as f64 / (n - 1) as f64
            };
            let count = f64::from(*counts.get(e).unwrap_or(&1));
            let mut score = recency + FREQ_WEIGHT * (1.0 + count).ln();
            if let Some(cwd) = cwd
                && cwds.get(e).is_some_and(|dirs| dirs.contains(cwd))
            {
                score += CWD_BONUS;
            }
            if let Some(sess) = session_cmds
                && sess.contains(e.as_str())
            {
                score += SESSION_BONUS;
            }
            score
        })
        .collect()
}

pub fn format_ago(now: u64, ts: u64) -> String {
    let s = now.saturating_sub(ts);
    let (n, unit) = if s < 60 {
        return "now".to_string();
    } else if s < 3600 {
        (s / 60, "m")
    } else if s < 86_400 {
        (s / 3600, "h")
    } else if s < 7 * 86_400 {
        (s / 86_400, "d")
    } else if s < 30 * 86_400 {
        (s / (7 * 86_400), "w")
    } else if s < 365 * 86_400 {
        (s / (30 * 86_400), "mo")
    } else {
        (s / (365 * 86_400), "y")
    };
    format!("{n}{unit}")
}

/// Format an elapsed command duration concisely for the history menu
/// (`"120ms"`, `"2.4s"`, `"1m12s"`, `"1h05m"`). Returns `None` for trivially
/// fast commands (< 100 ms) so rows stay visually quiet.
pub fn format_duration(duration_ms: u64) -> Option<String> {
    if duration_ms < 100 {
        return None;
    }
    if duration_ms < 1_000 {
        return Some(format!("{duration_ms}ms"));
    }
    if duration_ms < 60_000 {
        let tenths = duration_ms / 100;
        return Some(format!("{}.{}s", tenths / 10, tenths % 10));
    }
    let total_secs = duration_ms / 1_000;
    if total_secs < 3_600 {
        return Some(format!("{}m{:02}s", total_secs / 60, total_secs % 60));
    }
    let hours = total_secs / 3_600;
    let mins = (total_secs % 3_600) / 60;
    Some(format!("{hours}h{mins:02}m"))
}
