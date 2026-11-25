use super::*;
use std::path::Path;

fn rec(scope: Scope, session: &str, cmd: &str, cwd: Option<&str>, ts: u64) -> RecordInput {
    RecordInput {
        scope,
        session_id: session.to_string(),
        cmd: cmd.to_string(),
        cwd: cwd.map(str::to_string),
        ts,
        duration_ms: None,
        exit: None,
    }
}

#[test]
fn sqlite_records_multiline_commands_and_updates_exit_and_duration() {
    let mut db = HistoryDb::open_in_memory().unwrap();
    let multi = "for f in *.rs; do\n  echo \"$f\"\ndone";
    let id = db
        .record_command(&rec(Scope::Local, "s1", multi, Some("/work"), 1_700_000_010))
        .unwrap()
        .expect("multiline command is persisted");
    db.finish_command(id, Some(0), Some(1_250)).unwrap();

    let snap = db.load_scope_snapshot(&Scope::Local, 100).unwrap();
    assert_eq!(snap.entries, vec![multi.to_string()]);
    assert_eq!(
        snap.meta.get(multi),
        Some(&EntryMeta {
            ts: Some(1_700_000_010),
            exit: Some(0),
            duration_ms: Some(1_250),
        })
    );
    assert_eq!(snap.session_entries("s1"), &[multi.to_string()]);
}

#[test]
fn leading_space_commands_are_ignored_for_privacy() {
    assert!(should_ignore_command(" export API_KEY=secret"));
    assert!(should_ignore_command("\tsecret_cmd"));
    assert!(should_ignore_command("   "));
    assert!(!should_ignore_command("git status"));

    let mut db = HistoryDb::open_in_memory().unwrap();
    assert_eq!(
        db.record_command(&rec(Scope::Local, "s1", " export SECRET=1", None, 100))
            .unwrap(),
        None
    );
    assert_eq!(db.total_rows().unwrap(), 0);
}

#[test]
fn session_carry_chains_across_multiple_restarts_without_mutating_rows() {
    let mut db = HistoryDb::open_in_memory().unwrap();
    let s1 = db.resolve_session("h0-p1").unwrap();
    assert_eq!(s1, "h0-p1");

    db.record_command(&rec(Scope::Local, &s1, "cargo check", None, 100))
        .unwrap();

    // Pane 1 restarts as pane 5, then hibernates & wakes as pane 12.
    let s5 = db.carry_session("h0-p1", "h0-p5", 200).unwrap();
    let s12 = db.carry_session("h0-p5", "h0-p12", 300).unwrap();
    assert_eq!(s5, "h0-p1");
    assert_eq!(s12, "h0-p1");

    db.record_command(&rec(Scope::Local, &s12, "cargo test", None, 310))
        .unwrap();

    let snap = db.load_scope_snapshot(&Scope::Local, 100).unwrap();
    assert_eq!(
        snap.session_entries(&s12),
        &["cargo check".to_string(), "cargo test".to_string()]
    );
}

#[test]
fn scopes_are_isolated_and_all_scopes_view_unifies_them() {
    let mut db = HistoryDb::open_in_memory().unwrap();
    let remote = Scope::remote("deploy@prod");
    db.record_command(&rec(Scope::Local, "s1", "cargo build", None, 100))
        .unwrap();
    db.record_command(&rec(remote.clone(), "s2", "systemctl status nginx", None, 200))
        .unwrap();

    let local = db.load_scope_snapshot(&Scope::Local, 100).unwrap();
    let prod = db.load_scope_snapshot(&remote, 100).unwrap();
    let all = db.load_all_scopes_snapshot(100).unwrap();

    assert_eq!(local.entries, vec!["cargo build"]);
    assert_eq!(prod.entries, vec!["systemctl status nginx"]);
    assert_eq!(all.entries, vec!["cargo build", "systemctl status nginx"]);
}

#[test]
fn delete_command_removes_entries_from_db_and_snapshot() {
    let mut db = HistoryDb::open_in_memory().unwrap();
    db.record_command(&rec(Scope::Local, "s1", "curl -H secret", None, 100))
        .unwrap();
    db.record_command(&rec(Scope::Local, "s1", "git status", None, 101))
        .unwrap();
    db.delete_command(Some(&Scope::Local), "curl -H secret")
        .unwrap();

    let snap = db.load_scope_snapshot(&Scope::Local, 100).unwrap();
    assert_eq!(snap.entries, vec!["git status"]);
}

#[test]
fn retention_prune_bounds_total_row_count() {
    let mut db = HistoryDb::open_in_memory().unwrap();
    for i in 0..50 {
        db.record_command(&rec(Scope::Local, "s1", &format!("cmd-{i}"), None, 1000 + i))
            .unwrap();
    }
    assert_eq!(db.total_rows().unwrap(), 50);
    let dropped = db.prune(10).unwrap();
    assert_eq!(dropped, 40);
    assert_eq!(db.total_rows().unwrap(), 10);

    let snap = db.load_scope_snapshot(&Scope::Local, 100).unwrap();
    assert_eq!(snap.entries.first().map(String::as_str), Some("cmd-40"));
    assert_eq!(snap.entries.last().map(String::as_str), Some("cmd-49"));
}

#[test]
fn legacy_tsv_migration_imports_all_generations_once_and_renames_file() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join("history");
    std::fs::write(
        &legacy,
        "1700000001\t0\t/work\tgit status\n/old\tmake\nbare command\n",
    )
    .unwrap();

    let mut db = HistoryDb::open_at(dir.path().join("history.db")).unwrap();
    let n = db.migrate_legacy_file(&Scope::Local, &legacy).unwrap();
    assert_eq!(n, 3);
    assert!(!legacy.exists(), "legacy file moved to .migrated");
    assert!(dir.path().join("history.migrated").exists());

    let snap = db.load_scope_snapshot(&Scope::Local, 100).unwrap();
    assert!(snap.entries.iter().any(|e| e == "git status"));
    assert!(snap.entries.iter().any(|e| e == "make"));
    assert!(snap.entries.iter().any(|e| e == "bare command"));
    assert_eq!(
        snap.meta.get("git status"),
        Some(&EntryMeta {
            ts: Some(1_700_000_001),
            exit: Some(0),
            duration_ms: None,
        })
    );
}

#[test]
fn incremental_shell_import_avoids_double_counting_and_mtime_hijack() {
    let dir = tempfile::tempdir().unwrap();
    let bash_hist = dir.path().join(".bash_history");
    std::fs::write(&bash_hist, "ancient bash cmd\n").unwrap();

    let mut db = HistoryDb::open_at(dir.path().join("history.db")).unwrap();
    // Anterm already recorded a recent command at ts=1_700_000_500.
    db.record_command(&rec(Scope::Local, "s1", "recent anterm", None, 1_700_000_500))
        .unwrap();

    // First bulk import of untimestamped bash history must NOT stamp old lines
    // with today's file mtime and outrank recent anterm commands.
    db.import_shell_file_at(&Scope::Local, &bash_hist).unwrap();
    let snap = db.load_scope_snapshot(&Scope::Local, 100).unwrap();
    assert_eq!(
        snap.entries.last().map(String::as_str),
        Some("recent anterm"),
        "untimestamped bulk bash import must not hijack recent timestamped history: {:?}",
        snap.entries
    );

    // Zsh history containing the same command within ±5s of anterm's record must
    // not double-count frequency.
    let zsh_hist = dir.path().join(".zsh_history");
    std::fs::write(&zsh_hist, ": 1700000501:0;recent anterm\n: 1700000600:3;cargo test\n").unwrap();
    db.import_shell_file_at(&Scope::Local, &zsh_hist).unwrap();

    let snap = db.load_scope_snapshot(&Scope::Local, 100).unwrap();
    assert_eq!(snap.counts.get("recent anterm"), Some(&1));
    assert_eq!(snap.entries.last().map(String::as_str), Some("cargo test"));
    assert_eq!(
        snap.meta.get("cargo test").and_then(|m| m.duration_ms),
        Some(3_000)
    );
}

#[test]
fn parsers_join_multiline_zsh_bash_and_fish_commands() {
    let zsh = ": 1700000000:0;for f in *; do\\\necho $f\\\ndone\n: 1700000010:0;ls\n";
    let mut out = Vec::new();
    parsers::parse_shell_history(zsh, &mut out);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].cmd, "for f in *; do\necho $f\ndone");
    assert_eq!(out[1].cmd, "ls");

    let fish = "- cmd: for f in *\\necho $f\\nend\n  when: 1700000000\n- cmd: pwd\n  when: 1700000005\n";
    let mut fout = Vec::new();
    parsers::parse_fish_history(fish, &mut fout);
    assert_eq!(fout.len(), 2);
    assert_eq!(fout[0].cmd, "for f in *\necho $f\nend");
    assert_eq!(fout[1].cmd, "pwd");
}

#[test]
fn fuzzy_matcher_reuses_buffers_and_matches_identically_across_lines() {
    let mut matcher = FuzzyMatcher::new();
    let q = fuzzy::PreparedQuery::new("git st").unwrap();
    let hit = matcher.match_prepared("git status --short", &q).unwrap();
    assert_eq!(hit.positions, vec![0, 1, 2, 4, 5]);
    assert!(matcher.match_prepared("cargo test", &q).is_none());
    let unicode_q = fuzzy::PreparedQuery::new("文档 ls").unwrap();
    let uhit = matcher.match_prepared("ls ~/文档/notes", &unicode_q).unwrap();
    assert_eq!(uhit.positions, vec![0, 1, 5, 6]);
}

#[test]
fn format_duration_omits_sub_100ms_and_formats_readable_units() {
    assert_eq!(format_duration(0), None);
    assert_eq!(format_duration(99), None);
    assert_eq!(format_duration(250), Some("250ms".to_string()));
    assert_eq!(format_duration(2_450), Some("2.4s".to_string()));
    assert_eq!(format_duration(72_000), Some("1m12s".to_string()));
    assert_eq!(format_duration(3_900_000), Some("1h05m".to_string()));
}

#[cfg(unix)]
#[test]
fn sqlite_history_file_is_private_0600() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("history.db");
    let _db = HistoryDb::open_at(db_path.clone()).unwrap();
    let mode = std::fs::metadata(&db_path).unwrap().permissions().mode();
    assert_eq!(mode & 0o077, 0, "history.db must be 0600");
}

#[test]
fn shell_history_name_for_shell_targets_single_file() {
    assert_eq!(shell_history_name_for_shell("/bin/zsh"), Some(".zsh_history"));
    assert_eq!(shell_history_name_for_shell("/usr/bin/bash"), Some(".bash_history"));
    assert_eq!(
        shell_history_name_for_shell("/opt/homebrew/bin/fish"),
        Some(FISH_HISTORY_UNDER_HOME)
    );
    assert_eq!(
        shell_history_name_for_shell("pwsh.exe"),
        Some(PSREADLINE_HISTORY_UNDER_HOME)
    );
    assert_eq!(shell_history_name_for_shell("/bin/sh"), None);
    assert!(Path::new(FISH_HISTORY_UNDER_HOME).ends_with(FISH_HISTORY_FILE));
}
