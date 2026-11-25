//! SQLite persistence layer (`<config_dir>/history.db`) for command history.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rusqlite::{Connection, OptionalExtension as _, params};

use super::parsers::{
    RawEntry, decode_history_file, local_shell_history_paths, parse_history_file, parse_own_line,
};
use super::{MAX_DB_ROWS, RecordInput, Scope, ScopeSnapshot, should_ignore_command};

const SCHEMA_VERSION: i32 = 1;

const SOURCE_ANTERM: i64 = 0;
const SOURCE_SHELL_IMPORT: i64 = 1;
const SOURCE_LEGACY_MIGRATION: i64 = 2;

/// Maximum age (seconds) for dormant `session_aliases` entries before pruning.
const SESSION_ALIAS_TTL_SECS: u64 = 90 * 24 * 60 * 60;

pub struct HistoryDb {
    conn: Connection,
    path: Option<PathBuf>,
}

impl HistoryDb {
    /// Open the user's history database at `<config_dir>/history.db`.
    pub fn open_default() -> rusqlite::Result<Self> {
        let Some(path) = crate::config::config_path("history.db") else {
            return Self::open_in_memory();
        };
        Self::open_at(path)
    }

    /// Open (or create) a history database at a specific filesystem path,
    /// enforcing `0o600` permissions on Unix.
    pub fn open_at(path: PathBuf) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Pre-create with 0o600 on Unix before SQLite opens it so there is
        // never a world-readable window.
        #[cfg(unix)]
        ensure_private_file(&path);

        let conn = Connection::open(&path)?;
        let mut db = Self {
            conn,
            path: Some(path),
        };
        db.init()?;
        #[cfg(unix)]
        db.tighten_permissions();
        Ok(db)
    }

    /// In-memory database for fast isolated unit tests.
    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        let mut db = Self { conn, path: None };
        db.init()?;
        Ok(db)
    }

    fn init(&mut self) -> rusqlite::Result<()> {
        // Set auto_vacuum before creating tables on a fresh database.
        let version: i32 = self
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version == 0 {
            let _ = self.conn.pragma_update(None, "auto_vacuum", "INCREMENTAL");
        }
        self.conn.pragma_update(None, "journal_mode", "WAL")?;
        self.conn.pragma_update(None, "synchronous", "NORMAL")?;
        self.conn.pragma_update(None, "busy_timeout", 5000)?;
        self.conn.pragma_update(None, "temp_store", "MEMORY")?;

        if version < SCHEMA_VERSION {
            self.conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS commands (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    scope       TEXT NOT NULL,
                    session_id  TEXT NOT NULL DEFAULT '',
                    cmd         TEXT NOT NULL,
                    cwd         TEXT,
                    ts          INTEGER NOT NULL,
                    duration_ms INTEGER,
                    exit_code   INTEGER,
                    source      INTEGER NOT NULL DEFAULT 0,
                    dedup_key   TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_cmd_scope_ts
                    ON commands(scope, ts DESC, id DESC);
                CREATE INDEX IF NOT EXISTS idx_cmd_scope_sess_ts
                    ON commands(scope, session_id, ts DESC, id DESC);
                CREATE INDEX IF NOT EXISTS idx_cmd_scope_cwd_ts
                    ON commands(scope, cwd, ts DESC, id DESC);
                CREATE UNIQUE INDEX IF NOT EXISTS idx_cmd_import_dedup
                    ON commands(scope, dedup_key) WHERE dedup_key IS NOT NULL;

                CREATE TABLE IF NOT EXISTS import_cursors (
                    scope       TEXT NOT NULL,
                    path        TEXT NOT NULL,
                    mtime       INTEGER NOT NULL,
                    file_size   INTEGER NOT NULL,
                    line_count  INTEGER NOT NULL DEFAULT 0,
                    imported_at INTEGER NOT NULL,
                    PRIMARY KEY (scope, path)
                );

                CREATE TABLE IF NOT EXISTS session_aliases (
                    pane_key    TEXT PRIMARY KEY,
                    session_id  TEXT NOT NULL,
                    updated_at  INTEGER NOT NULL
                );

                PRAGMA user_version = 1;
                ",
            )?;
        }
        Ok(())
    }

    #[cfg(unix)]
    fn tighten_permissions(&self) {
        let Some(path) = &self.path else {
            return;
        };
        set_mode_0600(path);
        for suffix in ["-wal", "-shm"] {
            let mut side = path.as_os_str().to_owned();
            side.push(suffix);
            let p = PathBuf::from(side);
            if p.exists() {
                set_mode_0600(&p);
            }
        }
    }

    /// Record a freshly executed command and return its row id so `finish_command`
    /// can later attach the exit code and duration.
    pub fn record_command(&mut self, rec: &RecordInput) -> rusqlite::Result<Option<i64>> {
        if should_ignore_command(&rec.cmd) {
            return Ok(None);
        }
        let scope_key = rec.scope.key();
        self.conn.execute(
            "INSERT INTO commands (scope, session_id, cmd, cwd, ts, duration_ms, exit_code, source, dedup_key)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)",
            params![
                scope_key,
                rec.session_id,
                rec.cmd,
                rec.cwd,
                rec.ts as i64,
                rec.duration_ms.map(|d| d as i64),
                rec.exit,
                SOURCE_ANTERM,
            ],
        )?;
        Ok(Some(self.conn.last_insert_rowid()))
    }

    /// Update exit code and elapsed duration for an already-recorded command.
    pub fn finish_command(
        &mut self,
        row_id: i64,
        exit_code: Option<i32>,
        duration_ms: Option<u64>,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE commands
                SET exit_code = COALESCE(?2, exit_code),
                    duration_ms = COALESCE(?3, duration_ms)
              WHERE id = ?1",
            params![row_id, exit_code, duration_ms.map(|d| d as i64)],
        )?;
        Ok(())
    }

    /// Delete every row matching `cmd` (either in `scope` or across all scopes
    /// when `scope` is `None`).
    pub fn delete_command(&mut self, scope: Option<&Scope>, cmd: &str) -> rusqlite::Result<usize> {
        match scope {
            Some(scope) => self.conn.execute(
                "DELETE FROM commands WHERE scope = ?1 AND cmd = ?2",
                params![scope.key(), cmd],
            ),
            None => self
                .conn
                .execute("DELETE FROM commands WHERE cmd = ?1", params![cmd]),
        }
    }

    /// Resolve a pane key (`"h<host>-p<id>"`) to its logical history session id.
    /// Defaults to `pane_key` itself if no prior pane handed its session over.
    pub fn resolve_session(&self, pane_key: &str) -> rusqlite::Result<String> {
        let mapped: Option<String> = self
            .conn
            .query_row(
                "SELECT session_id FROM session_aliases WHERE pane_key = ?1",
                params![pane_key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(mapped.unwrap_or_else(|| pane_key.to_string()))
    }

    /// Bind `new_pane_key` to the same logical history session as `old_pane_key`
    /// (preserving per-pane history across daemon restarts and tab hibernation
    /// without duplicating or mutating any command rows).
    pub fn carry_session(
        &mut self,
        old_pane_key: &str,
        new_pane_key: &str,
        now: u64,
    ) -> rusqlite::Result<String> {
        let session_id = self.resolve_session(old_pane_key)?;
        self.conn.execute(
            "INSERT INTO session_aliases (pane_key, session_id, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(pane_key) DO UPDATE SET session_id = excluded.session_id, updated_at = excluded.updated_at",
            params![new_pane_key, session_id, now as i64],
        )?;
        Ok(session_id)
    }

    /// One-time automatic migration of anterm's legacy flat TSV file for `scope`
    /// (`<config_dir>/history` for Local, `<config_dir>/history.d/<stem>` for Remote).
    /// Safe when `<config_dir>/history` is absent or is a directory.
    pub fn migrate_legacy_scope(&mut self, scope: &Scope) -> rusqlite::Result<usize> {
        let Some(path) = scope.legacy_file() else {
            return Ok(0);
        };
        self.migrate_legacy_file(scope, &path)
    }

    pub fn migrate_legacy_file(&mut self, scope: &Scope, path: &Path) -> rusqlite::Result<usize> {
        if !path.is_file() {
            return Ok(0);
        }
        let cursor_key = format!("__legacy__:{}", path.display());
        let scope_key = scope.key();
        let already: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM import_cursors WHERE scope = ?1 AND path = ?2",
                params![scope_key, cursor_key],
                |row| row.get(0),
            )
            .optional()?;
        if already.is_some() {
            return Ok(0);
        }

        let Ok(bytes) = std::fs::read(path) else {
            return Ok(0);
        };
        let text = String::from_utf8_lossy(&bytes);
        let now = unix_now();
        let tx = self.conn.transaction()?;
        let mut inserted = 0usize;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO commands
                    (scope, session_id, cmd, cwd, ts, duration_ms, exit_code, source, dedup_key)
                 VALUES (?1, '', ?2, ?3, ?4, NULL, ?5, ?6, ?7)",
            )?;
            for (idx, line) in text.lines().enumerate() {
                let raw = parse_own_line(line);
                if should_ignore_command(&raw.cmd) {
                    continue;
                }
                let dedup = format!(
                    "legacy:{idx}:{:016x}",
                    crate::host::fnv1a64(raw.cmd.as_bytes())
                );
                let n = stmt.execute(params![
                    scope_key,
                    raw.cmd,
                    raw.cwd,
                    raw.ts.unwrap_or(0) as i64,
                    raw.exit,
                    SOURCE_LEGACY_MIGRATION,
                    dedup,
                ])?;
                inserted += n;
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO import_cursors (scope, path, mtime, file_size, line_count, imported_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                scope_key,
                cursor_key,
                file_mtime_secs(path).unwrap_or(0) as i64,
                bytes.len() as i64,
                inserted as i64,
                now as i64,
            ],
        )?;
        tx.commit()?;

        // Rename the migrated flat file aside so it is not re-read and cannot
        // conflict with anything else; keep the `.migrated` backup safely.
        let mut backup = path.as_os_str().to_owned();
        backup.push(".migrated");
        let _ = std::fs::rename(path, PathBuf::from(backup));

        Ok(inserted)
    }

    /// Incrementally import all local shell history files whose `(mtime, size)`
    /// changed since the last import.
    pub fn import_local_shell_files(&mut self) -> rusqlite::Result<usize> {
        if cfg!(test) {
            return Ok(0);
        }
        let paths = local_shell_history_paths();
        let mut total = 0;
        for path in paths {
            total += self.import_shell_file_at(&Scope::Local, &path)?;
        }
        Ok(total)
    }

    /// Incrementally import one local shell history file on disk.
    pub fn import_shell_file_at(&mut self, scope: &Scope, path: &Path) -> rusqlite::Result<usize> {
        let Ok(meta) = std::fs::metadata(path) else {
            return Ok(0);
        };
        if !meta.is_file() {
            return Ok(0);
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let file_size = meta.len();
        let path_key = path.to_string_lossy().into_owned();
        let scope_key = scope.key();

        let prev: Option<(i64, i64, i64)> = self
            .conn
            .query_row(
                "SELECT mtime, file_size, line_count FROM import_cursors WHERE scope = ?1 AND path = ?2",
                params![scope_key, path_key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;

        if let Some((prev_mtime, prev_size, _)) = prev
            && prev_mtime == mtime as i64
            && prev_size == file_size as i64
        {
            return Ok(0);
        }

        let Ok(bytes) = std::fs::read(path) else {
            return Ok(0);
        };
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let prev_lines = prev.map(|(_, _, lines)| lines.max(0) as usize);
        self.import_parsed_shell_bytes(scope, &path_key, &name, &bytes, mtime, prev_lines)
    }

    /// Import shell history files fetched over SFTP for a remote scope.
    pub fn import_remote_shell_files(
        &mut self,
        scope: &Scope,
        files: &[(String, Vec<u8>)],
    ) -> rusqlite::Result<usize> {
        let now = unix_now();
        let mut total = 0;
        for (name, bytes) in files {
            let path_key = format!("remote-shell:{name}");
            let scope_key = scope.key();
            let prev: Option<(i64, i64)> = self
                .conn
                .query_row(
                    "SELECT file_size, line_count FROM import_cursors WHERE scope = ?1 AND path = ?2",
                    params![scope_key, path_key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((prev_size, _)) = prev
                && prev_size == bytes.len() as i64
            {
                continue;
            }
            let prev_lines = prev.map(|(_, l)| l.max(0) as usize);
            total += self.import_parsed_shell_bytes(
                scope,
                &path_key,
                name,
                bytes,
                now,
                prev_lines,
            )?;
        }
        Ok(total)
    }

    fn import_parsed_shell_bytes(
        &mut self,
        scope: &Scope,
        path_key: &str,
        file_name: &str,
        bytes: &[u8],
        mtime: u64,
        prev_line_count: Option<usize>,
    ) -> rusqlite::Result<usize> {
        let decoded = decode_history_file(file_name, bytes);
        let mut parsed = Vec::new();
        parse_history_file(file_name, &decoded, &mut parsed);

        let scope_key = scope.key();
        let path_hash = crate::host::fnv1a64(path_key.as_bytes());
        let now = unix_now();

        // Snapshot recent anterm-recorded `(cmd, ts)` rows so we do not double-
        // count commands that anterm already wrote and the shell also flushed
        // to `~/.zsh_history` / `~/.bash_history`.
        let anterm_recent: Vec<(String, i64)> = {
            let mut stmt = self.conn.prepare(
                "SELECT cmd, ts FROM commands
                  WHERE scope = ?1 AND source = ?2
                  ORDER BY id DESC LIMIT 4000",
            )?;
            let rows = stmt.query_map(params![scope_key, SOURCE_ANTERM], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            rows.flatten().collect()
        };
        let mut anterm_by_cmd: std::collections::HashMap<&str, Vec<i64>> =
            std::collections::HashMap::new();
        for (cmd, ts) in &anterm_recent {
            anterm_by_cmd.entry(cmd.as_str()).or_default().push(*ts);
        }

        let tx = self.conn.transaction()?;
        let mut inserted = 0usize;
        let total_entries = parsed.len();

        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO commands
                    (scope, session_id, cmd, cwd, ts, duration_ms, exit_code, source, dedup_key)
                 VALUES (?1, '', ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;

            for (idx, entry) in parsed.into_iter().enumerate() {
                if should_ignore_command(&entry.cmd) {
                    continue;
                }
                // Untimestamped shell files (e.g. bash without HISTTIMEFORMAT,
                // PSReadLine): on initial bulk import, keep ts = 0 so ancient
                // lines never hijack recent timestamped zsh/anterm history. On
                // incremental growth (entries past `prev_line_count`), stamp
                // ONLY the newly appended tail with the file's mtime.
                let ts = match entry.ts {
                    Some(t) => t,
                    None => match prev_line_count {
                        Some(prev_n) if idx >= prev_n => mtime,
                        _ => 0,
                    },
                };

                // Skip double-counting if anterm itself recorded the exact same
                // command within ±5 seconds.
                if ts > 0
                    && let Some(times) = anterm_by_cmd.get(entry.cmd.as_str())
                    && times.iter().any(|&t| (t - ts as i64).abs() <= 5)
                {
                    continue;
                }

                let dedup = if entry.ts.is_some() {
                    format!(
                        "sh:{path_hash:016x}:{ts}:{:016x}",
                        crate::host::fnv1a64(entry.cmd.as_bytes())
                    )
                } else {
                    format!(
                        "sh:{path_hash:016x}:#{idx}:{:016x}",
                        crate::host::fnv1a64(entry.cmd.as_bytes())
                    )
                };

                let n = stmt.execute(params![
                    scope_key,
                    entry.cmd,
                    entry.cwd,
                    ts as i64,
                    entry.duration_ms.map(|d| d as i64),
                    entry.exit,
                    SOURCE_SHELL_IMPORT,
                    dedup,
                ])?;
                inserted += n;
            }
        }

        tx.execute(
            "INSERT OR REPLACE INTO import_cursors (scope, path, mtime, file_size, line_count, imported_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                scope_key,
                path_key,
                mtime as i64,
                bytes.len() as i64,
                total_entries as i64,
                now as i64,
            ],
        )?;
        tx.commit()?;
        Ok(inserted)
    }

    /// Build a hot [`ScopeSnapshot`] for `scope` (newest `max_hot` distinct
    /// commands plus execution counts, cwds, metadata, and per-session slices).
    pub fn load_scope_snapshot(
        &self,
        scope: &Scope,
        max_hot: usize,
    ) -> rusqlite::Result<ScopeSnapshot> {
        let scope_key = scope.key();
        // Pull up to 5× `max_hot` raw rows so deduplication & counts remain
        // rich while keeping cold startup bounded to a few milliseconds.
        let raw_limit = (max_hot.saturating_mul(5)).max(1_000) as i64;
        let mut stmt = self.conn.prepare(
            "SELECT cmd, cwd, ts, duration_ms, exit_code, session_id
               FROM commands
              WHERE scope = ?1
              ORDER BY ts DESC, id DESC
              LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![scope_key, raw_limit], |row| {
            let ts: i64 = row.get(2)?;
            let dur: Option<i64> = row.get(3)?;
            let sess: String = row.get(5)?;
            Ok(RawEntry {
                cmd: row.get(0)?,
                cwd: row.get(1)?,
                ts: (ts > 0).then_some(ts as u64),
                duration_ms: dur.filter(|&d| d >= 0).map(|d| d as u64),
                exit: row.get(4)?,
                session_id: (!sess.is_empty()).then_some(sess),
            })
        })?;
        let mut raw: Vec<RawEntry> = rows.flatten().collect();
        // `from_raw` expects chronological order (oldest first) for ties.
        raw.reverse();
        Ok(ScopeSnapshot::from_raw(raw, max_hot))
    }

    /// Build a cross-scope [`ScopeSnapshot`] for Ctrl+R's "All" filter mode.
    pub fn load_all_scopes_snapshot(&self, max_hot: usize) -> rusqlite::Result<ScopeSnapshot> {
        let raw_limit = (max_hot.saturating_mul(3)).max(1_000) as i64;
        let mut stmt = self.conn.prepare(
            "SELECT cmd, cwd, ts, duration_ms, exit_code, session_id
               FROM commands
              ORDER BY ts DESC, id DESC
              LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![raw_limit], |row| {
            let ts: i64 = row.get(2)?;
            let dur: Option<i64> = row.get(3)?;
            let sess: String = row.get(5)?;
            Ok(RawEntry {
                cmd: row.get(0)?,
                cwd: row.get(1)?,
                ts: (ts > 0).then_some(ts as u64),
                duration_ms: dur.filter(|&d| d >= 0).map(|d| d as u64),
                exit: row.get(4)?,
                session_id: (!sess.is_empty()).then_some(sess),
            })
        })?;
        let mut raw: Vec<RawEntry> = rows.flatten().collect();
        raw.reverse();
        Ok(ScopeSnapshot::from_raw(raw, max_hot))
    }

    /// Enforce retention cap (`max_rows`, defaults to [`MAX_DB_ROWS`]) and
    /// prune stale `session_aliases`, reclaiming space incrementally.
    pub fn prune(&mut self, max_rows: usize) -> rusqlite::Result<usize> {
        let cap = max_rows.max(1) as i64;
        let deleted = self.conn.execute(
            "DELETE FROM commands
              WHERE id NOT IN (
                  SELECT id FROM commands ORDER BY ts DESC, id DESC LIMIT ?1
              )",
            params![cap],
        )?;
        let cutoff = unix_now().saturating_sub(SESSION_ALIAS_TTL_SECS) as i64;
        let _ = self.conn.execute(
            "DELETE FROM session_aliases WHERE updated_at < ?1",
            params![cutoff],
        );
        if deleted > 0 {
            let _ = self.conn.execute_batch("PRAGMA incremental_vacuum(64);");
        }
        Ok(deleted)
    }

    /// Total row count in `commands` (useful for diagnostics and tests).
    pub fn total_rows(&self) -> rusqlite::Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM commands", [], |r| r.get(0))?;
        Ok(n.max(0) as usize)
    }

    /// Collect all distinct scope keys present in the DB.
    pub fn known_scopes(&self) -> rusqlite::Result<HashSet<String>> {
        let mut stmt = self.conn.prepare("SELECT DISTINCT scope FROM commands")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.flatten().collect())
    }

    pub fn prune_default(&mut self) -> rusqlite::Result<usize> {
        self.prune(MAX_DB_ROWS)
    }
}

fn file_mtime_secs(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(unix)]
fn ensure_private_file(path: &Path) {
    use std::os::unix::fs::OpenOptionsExt as _;
    if !path.exists() {
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .mode(0o600)
            .open(path);
    }
}

#[cfg(unix)]
fn set_mode_0600(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}
