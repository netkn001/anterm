//! Unified command history facade over `anterm_core::history`.
//!
//! All parsing, deduplication, frecency scoring, and SQLite persistence live in
//! `anterm_core::history` (behind the `history-db` feature); this module
//! re-exports those types for `anterm-app` and retains regression coverage for
//! every shell history format.

#[allow(unused_imports)]
pub use anterm_core::history::{
    EntryMeta, FISH_HISTORY_FILE, FISH_HISTORY_UNDER_DATA_DIR, FISH_HISTORY_UNDER_HOME,
    HistoryDb, HistoryFilter, MAX_DB_ROWS, MAX_HOT_ENTRIES, PSREADLINE_HISTORY_FILE,
    PSREADLINE_HISTORY_UNDER_APPDATA, PSREADLINE_HISTORY_UNDER_DATA_DIR,
    PSREADLINE_HISTORY_UNDER_HOME, REPORTED_HISTORY_FILES, RecordInput, Scope, ScopeSnapshot,
    format_ago, format_duration, frecency_scores, legacy_file_stem, shell_history_name_for_shell,
    shell_history_names, should_ignore_command,
};

#[cfg(test)]
mod tests {
    use super::*;
    use anterm_core::history::parsers::{
        RawEntry, decode_history_file, parse_history_file, parse_own_line,
        parse_reported_history_files,
    };
    use std::collections::{HashMap, HashSet};
    use std::path::{Path, PathBuf};

    fn parse(name: &str, content: &str) -> Vec<String> {
        let mut out = Vec::new();
        parse_history_file(name, content, &mut out);
        out.into_iter().map(|r| r.cmd).collect()
    }

    fn parse_ts(name: &str, content: &str) -> Vec<(String, Option<u64>)> {
        let mut out = Vec::new();
        parse_history_file(name, content, &mut out);
        out.into_iter().map(|r| (r.cmd, r.ts)).collect()
    }

    #[test]
    fn fish_history_entries_carry_cmd_and_timestamp() {
        let content =
            "- cmd: git status\n  when: 1700000000\n- cmd: cargo build\n  when: 1700000005\n";
        assert_eq!(
            parse_ts(FISH_HISTORY_FILE, content),
            [
                ("git status".to_string(), Some(1_700_000_000)),
                ("cargo build".to_string(), Some(1_700_000_005)),
            ]
        );
    }

    #[test]
    fn fish_writes_commands_unquoted_so_yaml_punctuation_is_just_text() {
        let content = concat!(
            "- cmd: git commit -m \"fix: crash on start\"\n  when: 1700000000\n",
            "- cmd: echo foo: bar\n  when: 1700000001\n",
            "- cmd: [ -f x ]; and echo y\n  when: 1700000002\n",
            "- cmd: rg --files # every file\n  when: 1700000003\n",
            "- cmd: this: is: not: broken\n  when: 1700000004\n",
        );
        assert_eq!(
            parse_ts(FISH_HISTORY_FILE, content),
            [
                (
                    "git commit -m \"fix: crash on start\"".to_string(),
                    Some(1_700_000_000)
                ),
                ("echo foo: bar".to_string(), Some(1_700_000_001)),
                ("[ -f x ]; and echo y".to_string(), Some(1_700_000_002)),
                ("rg --files # every file".to_string(), Some(1_700_000_003)),
                ("this: is: not: broken".to_string(), Some(1_700_000_004)),
            ]
        );
    }

    #[test]
    fn fish_escapes_only_backslash_and_newline_and_preserves_multiline() {
        assert_eq!(
            parse_ts(
                FISH_HISTORY_FILE,
                "- cmd: grep \"a\\\\b\" f\n  when: 1700000000\n"
            ),
            [("grep \"a\\b\" f".to_string(), Some(1_700_000_000))]
        );
        assert_eq!(
            parse_ts(FISH_HISTORY_FILE, "- cmd: printf 'a\\tb'\n"),
            [("printf 'a\\tb'".to_string(), None)]
        );
        let content =
            "- cmd: for f in *\\necho $f\\nend\n  when: 1700000000\n- cmd: ls\n  when: 1700000001\n";
        assert_eq!(
            parse_ts(FISH_HISTORY_FILE, content),
            [
                ("for f in *\necho $f\nend".to_string(), Some(1_700_000_000)),
                ("ls".to_string(), Some(1_700_000_001)),
            ]
        );
    }

    #[test]
    fn fish_history_paths_and_garbage_are_tolerated() {
        let content = concat!(
            "- cmd: vim src/main.rs\n",
            "  when: 1700000000\n",
            "  paths:\n",
            "    - src/main.rs\n",
            "\n",
            "  when: 9999999999\n",
        );
        assert_eq!(
            parse_ts(FISH_HISTORY_FILE, content),
            [("vim src/main.rs".to_string(), Some(1_700_000_000))]
        );
        assert_eq!(parse_ts(FISH_HISTORY_FILE, "not a record\n- cmd:\n"), []);
    }

    #[test]
    fn the_fish_and_psreadline_paths_are_spelled_the_same_way_everywhere() {
        assert_eq!(
            FISH_HISTORY_UNDER_HOME,
            format!(".local/share/{FISH_HISTORY_UNDER_DATA_DIR}")
        );
        assert!(shell_history_names().contains(&FISH_HISTORY_UNDER_HOME));
        assert_eq!(
            PSREADLINE_HISTORY_UNDER_HOME,
            format!("AppData/Roaming/{PSREADLINE_HISTORY_UNDER_APPDATA}")
        );
        assert!(shell_history_names().contains(&PSREADLINE_HISTORY_UNDER_HOME));
    }

    #[test]
    fn psreadline_bom_and_entries_are_parsed() {
        let bytes = "\u{FEFF}git pull\r\ncargo build --release\r\n".as_bytes();
        let text = decode_history_file(PSREADLINE_HISTORY_FILE, bytes);
        assert_eq!(
            parse(PSREADLINE_HISTORY_FILE, &text),
            ["git pull".to_string(), "cargo build --release".to_string()]
        );
    }

    #[test]
    fn reported_history_files_filter_relative_and_anterm_internal_paths() {
        let files = parse_reported_history_files(
            "/a/.zsh_history\nrelative\n/tmp/anterm-zdotdir-1/.zsh_history\n/home/me/.config/anterm/pane-history/pane-1\n  /b/hist  \n",
        );
        assert_eq!(
            files,
            vec![PathBuf::from("/a/.zsh_history"), PathBuf::from("/b/hist")]
        );
    }

    #[test]
    fn zsh_and_bash_multiline_commands_are_joined() {
        let zsh = ": 1700000000:3;for f in *; do\\\necho $f\\\ndone\n";
        let mut out = Vec::new();
        parse_history_file(".zsh_history", zsh, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].cmd, "for f in *; do\necho $f\ndone");
        assert_eq!(out[0].ts, Some(1_700_000_000));
        assert_eq!(out[0].duration_ms, Some(3_000));
    }

    #[test]
    fn parse_own_line_reads_all_generations() {
        let r = parse_own_line("1700000000\t0\t/home/me\tgit status");
        assert_eq!(r.cmd, "git status");
        assert_eq!(r.cwd.as_deref(), Some("/home/me"));
        assert_eq!(r.ts, Some(1_700_000_000));
        assert_eq!(r.exit, Some(0));

        let r = parse_own_line("/home/me\tgit status");
        assert_eq!(
            (r.cmd.as_str(), r.cwd.as_deref(), r.ts),
            ("git status", Some("/home/me"), None)
        );
        assert_eq!(parse_own_line("ls -la").cmd, "ls -la");
    }

    #[test]
    fn frecency_ranks_frequent_and_current_directory_higher() {
        let entries = vec![
            "git status".to_string(),
            "ls".to_string(),
            "oops typo".to_string(),
        ];
        let mut counts = HashMap::new();
        counts.insert("git status".to_string(), 40);
        counts.insert("ls".to_string(), 5);
        counts.insert("oops typo".to_string(), 1);
        let scores = frecency_scores(&entries, &counts, &HashMap::new(), None);
        assert!(scores[0] > scores[1]);
        assert!(scores[0] > scores[2]);

        let entries = vec!["npm test".to_string(), "cargo build".to_string()];
        let mut cwds: HashMap<String, HashSet<String>> = HashMap::new();
        cwds.entry("cargo build".to_string())
            .or_default()
            .insert("/work/proj".to_string());
        let here = frecency_scores(&entries, &HashMap::new(), &cwds, Some("/work/proj"));
        let neutral = frecency_scores(&entries, &HashMap::new(), &cwds, None);
        assert!(here[1] > here[0]);
        assert!(here[1] - here[0] > neutral[1] - neutral[0]);
    }

    #[test]
    fn format_ago_picks_readable_units() {
        let now = 1_700_000_000;
        assert_eq!(format_ago(now, now - 5), "now");
        assert_eq!(format_ago(now, now - 300), "5m");
        assert_eq!(format_ago(now, now - 2 * 3600), "2h");
        assert_eq!(format_ago(now, now - 3 * 86_400), "3d");
        assert_eq!(format_ago(now, now - 20 * 86_400), "2w");
        assert_eq!(format_ago(now, now - 90 * 86_400), "3mo");
        assert_eq!(format_ago(now, now - 800 * 86_400), "2y");
        assert_eq!(format_ago(now, now + 100), "now");
    }

    #[test]
    fn scope_remote_handles_blank_labels_and_safe_stems() {
        assert_eq!(Scope::remote(""), Scope::Local);
        assert_eq!(Scope::remote("   "), Scope::Local);
        let stem = legacy_file_stem("me@box:/srv/../weird");
        assert!(!stem.contains(['/', '\\', ':', '@']));
        assert!(legacy_file_stem(&"x".repeat(400)).len() < 80);
    }

    #[test]
    fn zsh_unmetafy_decodes_cjk_and_leaves_bash_untouched() {
        fn metafy(s: &str) -> Vec<u8> {
            let mut out = Vec::new();
            for &b in s.as_bytes() {
                if b == 0 || (0x83..=0xA2).contains(&b) {
                    out.push(0x83);
                    out.push(b ^ 0x20);
                } else {
                    out.push(b);
                }
            }
            out
        }
        let cjk = "echo \"读取令牌失败\"";
        let mut zsh = b": 1700000000:0;".to_vec();
        zsh.extend(metafy(cjk));
        zsh.push(b'\n');
        let decoded = decode_history_file(".zsh_history", &zsh);
        assert_eq!(parse(".zsh_history", &decoded), vec![cjk.to_string()]);

        let bash = "echo ー\n".as_bytes();
        let decoded_bash = decode_history_file(".bash_history", bash);
        assert_eq!(parse(".bash_history", &decoded_bash), vec!["echo ー".to_string()]);
    }

    #[test]
    fn snapshot_from_raw_orders_by_timestamp_and_caps_hot_entries() {
        let raw = vec![
            RawEntry {
                cmd: "recent shell".into(),
                cwd: None,
                ts: Some(200),
                duration_ms: None,
                exit: None,
                session_id: None,
            },
            RawEntry {
                cmd: "old anterm".into(),
                cwd: Some("/tmp".into()),
                ts: Some(100),
                duration_ms: Some(450),
                exit: Some(0),
                session_id: Some("local:1".into()),
            },
        ];
        let snap = ScopeSnapshot::from_raw(raw, MAX_HOT_ENTRIES);
        assert_eq!(snap.entries, ["old anterm", "recent shell"]);
        assert_eq!(snap.session_entries("local:1"), ["old anterm"]);
        assert_eq!(
            snap.meta.get("old anterm"),
            Some(&EntryMeta {
                ts: Some(100),
                exit: Some(0),
                duration_ms: Some(450),
            })
        );
    }

    #[test]
    fn sqlite_db_round_trips_remote_and_local_scopes_without_cross_leakage() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = HistoryDb::open_at(dir.path().join("history.db")).unwrap();
        let local = Scope::Local;
        let remote = Scope::remote("me@box");

        db.record_command(&RecordInput {
            scope: local.clone(),
            session_id: "local:1".into(),
            cmd: "cargo test".into(),
            cwd: Some("/repo".into()),
            ts: 1_700_000_000,
            duration_ms: None,
            exit: None,
        })
        .unwrap();
        db.record_command(&RecordInput {
            scope: remote.clone(),
            session_id: "local:2".into(),
            cmd: "systemctl status nginx".into(),
            cwd: None,
            ts: 1_700_000_001,
            duration_ms: None,
            exit: Some(0),
        })
        .unwrap();

        let local_snap = db.load_scope_snapshot(&local, MAX_HOT_ENTRIES).unwrap();
        let remote_snap = db.load_scope_snapshot(&remote, MAX_HOT_ENTRIES).unwrap();
        assert_eq!(local_snap.entries, ["cargo test"]);
        assert_eq!(remote_snap.entries, ["systemctl status nginx"]);
        assert!(!Path::new(&dir.path().join("history")).exists());
    }
}
