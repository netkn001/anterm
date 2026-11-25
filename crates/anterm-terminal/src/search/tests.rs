use std::path::{Path, PathBuf};

use super::links::{MAX_FILE_CANDIDATES, MAX_QUOTED_PATH, expand_home, windows_prefix};
use super::*;

/// The best reading of the token under `col`, which is what most of the
/// carving tests are about.
fn file_candidate_at(text: &str, col: usize) -> Option<FileCandidate> {
    file_candidates_at(text, col).into_iter().next()
}

#[test]
fn a_capped_match_count_says_it_is_capped() {
    // Same rule the count uses. Below the ceiling the number is the truth;
    // at the ceiling it is a floor, and has to read like one.
    let mark = |total: usize| match total >= MAX_MATCHES {
        true => "+",
        false => "",
    };
    assert_eq!(mark(0), "");
    assert_eq!(mark(MAX_MATCHES - 1), "");
    assert_eq!(mark(MAX_MATCHES), "+");
}

#[test]
fn the_bar_hides_whole_rows_however_tall_they_are() {
    // 8px inset + 34px tall, so at the default 17px line height the bar
    // sits over rows 0, 1 and part of 2 — all three count as hidden.
    assert_eq!(rows_under_the_bar(17.), 3);
    assert_eq!(rows_under_the_bar(42.), 1);
    assert_eq!(rows_under_the_bar(43.), 1);
    assert_eq!(rows_under_the_bar(10.), 5);
    assert_eq!(rows_under_the_bar(0.), 0, "a zero height must not divide");
}

#[test]
fn a_match_under_the_bar_counts_as_off_screen() {
    // 24 rows on screen, sitting at the live end, bar over the first 3.
    let at = |line| reveal(line, 0, 24, 3);
    assert_eq!(at(0), Reveal::Back(3), "row 0 is fully covered");
    assert_eq!(at(2), Reveal::Back(1), "row 2 is partly covered");
    assert_eq!(at(3), Reveal::Stay, "row 3 is the first readable one");
    assert_eq!(at(23), Reveal::Stay, "the last row is still on screen");
    assert_eq!(at(24), Reveal::ToPoint, "one past the end is not");
}

#[test]
fn scrolled_back_into_history_the_readable_band_moves_with_it() {
    // 10 lines of history above the viewport: rows now run -10..=13.
    let at = |line| reveal(line, 10, 24, 3);
    assert_eq!(at(-10), Reveal::Back(3), "the top row is behind the bar");
    assert_eq!(at(-7), Reveal::Stay);
    assert_eq!(at(13), Reveal::Stay);
    assert_eq!(at(14), Reveal::ToPoint);
}

#[test]
fn without_a_bar_nothing_on_screen_is_moved() {
    assert_eq!(reveal(0, 0, 24, 0), Reveal::Stay);
    assert_eq!(reveal(-1, 0, 24, 0), Reveal::Back(1))
}

#[test]
fn regex_escape_neutralizes_metacharacters() {
    assert_eq!(regex_escape("a.b*c"), r"a\.b\*c");
    assert_eq!(regex_escape("foo(bar)"), r"foo\(bar\)");
    assert_eq!(regex_escape("1+1=2"), r"1\+1=2");
    assert_eq!(regex_escape("hello"), "hello");
}

#[test]
fn url_at_detects_http_and_strips_trailing_punct() {
    let line = "go https://example.com, now";
    assert_eq!(url_at(line, 6).as_deref(), Some("https://example.com"));
}

#[test]
fn url_at_promotes_bare_www_and_ignores_plain_words() {
    assert_eq!(
        url_at("visit www.rust-lang.org now", 8).as_deref(),
        Some("https://www.rust-lang.org")
    );
    assert_eq!(url_at("just a word", 6), None);
    assert_eq!(url_at("word ", 4), None);
    assert_eq!(url_at("word", 99), None);
}

#[test]
fn url_span_at_reports_inclusive_columns_without_trailing_punct() {
    let line = "go https://example.com, now";
    assert_eq!(
        url_span_at(line, 10),
        Some((3, 21, "https://example.com".to_string()))
    );
    assert_eq!(&line[3..=21], "https://example.com");
}

#[test]
fn url_span_at_accepts_file_and_ftp_schemes() {
    assert_eq!(
        url_at("open file:///etc/hosts here", 5).as_deref(),
        Some("file:///etc/hosts")
    );
    assert_eq!(
        url_at("get ftp://host/pub done", 4).as_deref(),
        Some("ftp://host/pub")
    );
}

#[test]
fn url_span_at_strips_various_trailing_punctuation() {
    assert_eq!(
        url_at("open https://a.com] done", 7).as_deref(),
        Some("https://a.com")
    );
    assert_eq!(
        url_at("open https://a.com> done", 7).as_deref(),
        Some("https://a.com")
    );
    assert_eq!(
        url_at("open https://a.com';: done", 7).as_deref(),
        Some("https://a.com")
    );
}

#[test]
fn url_span_at_strips_leading_wrappers() {
    assert_eq!(
        url_at("see (https://a.com) ok", 8).as_deref(),
        Some("https://a.com")
    );
    assert_eq!(
        url_at("see [https://a.com] ok", 8).as_deref(),
        Some("https://a.com")
    );
    assert_eq!(
        url_at("see <https://a.com> ok", 8).as_deref(),
        Some("https://a.com")
    );
    assert_eq!(
        url_at("say \"https://a.com\" ok", 8).as_deref(),
        Some("https://a.com")
    );
    assert_eq!(
        url_at("(www.rust-lang.org)", 5).as_deref(),
        Some("https://www.rust-lang.org")
    );
}

#[test]
fn url_span_at_reports_trimmed_span_after_stripping_both_ends() {
    let line = "log [https://a.com] end";
    let (start, end, url) = url_span_at(line, 8).expect("URL inside the brackets");
    assert_eq!(url, "https://a.com");
    assert_eq!(&line[start..=end], "https://a.com");
    assert_eq!(&line[start - 1..start], "[");
    assert_eq!(&line[end + 1..end + 2], "]");
}

#[test]
fn url_at_detects_url_glued_to_cjk_prefix() {
    let url = "https://github.com/acme/app/pull/42";
    let line = format!("已创建：{url}");
    let scheme_col = 4;
    assert_eq!(url_at(&line, scheme_col).as_deref(), Some(url));
    assert_eq!(url_at(&line, 12).as_deref(), Some(url));
    let (start, end, got) = url_span_at(&line, scheme_col).expect("URL after prefix");
    assert_eq!(start, scheme_col);
    assert_eq!(got, url);
    assert_eq!(end, line.chars().count() - 1);

    let row = format!("PR 已创建:{url} 🎉收尾:删除临时");
    let h = row.chars().position(|c| c == 'h').expect("scheme start");
    assert_eq!(url_at(&row, h).as_deref(), Some(url));
    assert_eq!(url_at(&row, 0), None);
}

#[test]
fn url_at_ignores_hover_on_cjk_prefix_before_url() {
    let line = "已创建：https://a.com";
    assert_eq!(url_at(line, 0), None);
    assert_eq!(url_at(line, 3), None);
    assert_eq!(url_at(line, 4).as_deref(), Some("https://a.com"));
}

#[test]
fn url_at_strips_full_width_trailing_punctuation() {
    assert_eq!(
        url_at("见（https://a.com）", 3).as_deref(),
        Some("https://a.com")
    );
    assert_eq!(
        url_at("详见 https://a.com。", 5).as_deref(),
        Some("https://a.com")
    );
}

#[test]
fn url_at_stops_at_full_width_open_bracket_glued_after_url() {
    let url = "https://github.com/acme/app/pull/343";
    let line = format!("PR 已创建：{url}（fix/cache-write-tokens → dev）");
    let h = line.chars().position(|c| c == 'h').expect("scheme start");
    assert_eq!(url_at(&line, h).as_deref(), Some(url));
    let (start, end, got) = url_span_at(&line, h + 10).expect("URL before bracket");
    assert_eq!(got, url);
    assert_eq!(start, h);
    assert_eq!(line.chars().nth(end + 1), Some('（'));
    let f = line.chars().position(|c| c == 'f').expect("`fix` start");
    assert_eq!(url_at(&line, f), None);
}

#[test]
fn url_at_keeps_ascii_parens_inside_a_url() {
    let url = "https://en.wikipedia.org/wiki/Rust_(programming_language)/history";
    assert_eq!(url_at(url, 40).as_deref(), Some(url));
    let url = "https://en.wikipedia.org/wiki/Rust_(programming_language)";
    assert_eq!(url_at(url, 40).as_deref(), Some(url));
    let line = format!("see ({url}) ok");
    assert_eq!(url_at(&line, 8).as_deref(), Some(url));
    let url = "http://[::1]:8080/status";
    let line = format!("probe [{url}] done");
    assert_eq!(url_at(&line, 10).as_deref(), Some(url));
}

#[test]
fn url_at_stops_at_unbalanced_close_paren_glued_after_url() {
    let url = "https://github.com/netkn001/anterm/pull/43";
    let line = format!("PR 已开:#43 ({url})(Fixes #42),分支 fix-x。");
    let h = line.chars().position(|c| c == 'h').expect("scheme start");
    assert_eq!(url_at(&line, h).as_deref(), Some(url));
    let (start, end, got) = url_span_at(&line, h + 10).expect("URL inside parens");
    assert_eq!(got, url);
    assert_eq!(line.chars().nth(start - 1), Some('('));
    assert_eq!(line.chars().nth(end + 1), Some(')'));
    let f = line.chars().position(|c| c == 'F').expect("`Fixes` start");
    assert_eq!(url_at(&line, f), None);
    assert_eq!(
        url_at("read https://a.com/x]next now", 8).as_deref(),
        Some("https://a.com/x")
    );
}

#[test]
fn url_span_at_rejects_www_without_a_dot_and_empty_tokens() {
    assert_eq!(url_at("www near text", 1), None);
    assert_eq!(url_at("...", 1), None);
    assert_eq!(url_at("httpsomething", 3), None);
}

fn temp_file(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("anterm-link-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temporary link-test dir");
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create temporary parent dir");
    }
    std::fs::write(&path, b"").expect("create temporary file");
    path
}

/// `link_at` against the real filesystem, the way a local pane resolves.
fn local_link_at(line: &str, col: usize, roots: &LinkRoots, files: bool) -> Option<LinkMatch> {
    link_at(line, col, roots, files, &mut local_probe)
}

fn one_root(cwd: &Path) -> LinkRoots {
    LinkRoots::local(vec![cwd.to_path_buf()])
}

fn assert_file_link(
    line: &str,
    col: usize,
    cwd: &Path,
    expected_path: &Path,
    expected_line: Option<u32>,
    expected_column: Option<u32>,
) {
    let link = local_link_at(line, col, &one_root(cwd), true).expect("file link under cursor");
    match link.target {
        LinkTarget::File {
            path, line, column, ..
        } => {
            assert_eq!(path, expected_path);
            assert_eq!(line, expected_line);
            assert_eq!(column, expected_column);
        }
        LinkTarget::Url(url) => panic!("expected file link, got URL {url}"),
    }
}

#[test]
fn link_at_detects_relative_file_paths_from_cwd() {
    let path = temp_file("src/main.rs");
    let cwd = path.parent().and_then(Path::parent).unwrap();

    assert_file_link(
        "error src/main.rs:10:2 failed",
        8,
        cwd,
        &path,
        Some(10),
        Some(2),
    );

    let link = local_link_at("error src/main.rs:10:2 failed", 8, &one_root(cwd), true)
        .expect("file link under cursor");
    assert_eq!((link.start, link.end), (6, 21));
}

/// Ungated with the rest: what a `~` in a POSIX pane stands for is that
/// pane's business and not its client's, and the `#[cfg(unix)]` this used
/// to carry described the wrong machine.
#[test]
fn tilde_expansion_prefers_home_inferred_from_the_pane_cwd() {
    let roots = LinkRoots {
        dirs: vec![PathBuf::from("/Users/alice/clone/anterm")],
        local_home: true,
        style: PathStyle::Posix,
    };
    assert_eq!(
        expand_home("~/clone/anterm/src/main.rs", &roots),
        Some("/Users/alice/clone/anterm/src/main.rs".to_string())
    );
}

/// A cwd that reveals no home is the end of the road for a pane on another
/// machine: this machine's `$HOME` describes nobody there, and a path built
/// out of it would be asked about — and possibly answered — on the far side.
#[test]
fn tilde_expansion_does_not_borrow_this_machines_home_for_another_one() {
    let elsewhere = |cwd: &str| LinkRoots {
        dirs: vec![PathBuf::from(cwd)],
        local_home: false,
        style: PathStyle::Posix,
    };
    assert_eq!(expand_home("~/.zshrc", &elsewhere("/srv/app")), None);
    assert_eq!(
        expand_home("~/.zshrc", &elsewhere("/home/deploy/app")),
        Some("/home/deploy/.zshrc".to_string()),
        "a cwd that does reveal the home needs nothing from us"
    );
    assert!(
        expand_home(
            "~/.zshrc",
            &LinkRoots::local(vec![PathBuf::from("/srv/app")])
        )
        .is_some(),
        "a local pane still falls back to the environment"
    );
}

#[test]
fn link_at_detects_absolute_file_paths_and_single_line_suffix() {
    let path = temp_file("absolute.log");
    let line = format!("open {}:42 now", path.display());
    let col = line.chars().position(|c| c == '/').unwrap_or(5);

    assert_file_link(&line, col, Path::new("/"), &path, Some(42), None);
}

/// An agent reporting the file it wrote puts a CJK label and a colon
/// straight in front of the path, full-width or not.
#[test]
fn link_at_peels_a_cjk_label_off_an_absolute_path() {
    let path = temp_file("cjk-label/mock.html");
    for label in ["原型:", "原型：", "图:", "图："] {
        let line = format!("交互 HTML {label}{}", path.display());
        let col = line.chars().position(|c| c == '/').unwrap() + 3;
        assert_file_link(&line, col, Path::new("/"), &path, None, None);
    }
}

#[test]
fn link_at_trims_wrappers_and_trailing_punctuation_around_file_paths() {
    let path = temp_file("wrapped/src/lib.rs");
    let cwd = path.parent().and_then(Path::parent).unwrap();
    let line = "see (src/lib.rs:7), now";

    let link = local_link_at(line, 7, &one_root(cwd), true).expect("wrapped file link");
    assert_eq!((link.start, link.end), (5, 16));
    match link.target {
        LinkTarget::File {
            path: got,
            line,
            column,
            is_dir,
        } => {
            assert_eq!(got, path);
            assert_eq!(line, Some(7));
            assert_eq!(column, None);
            assert!(!is_dir);
        }
        LinkTarget::Url(url) => panic!("expected file link, got URL {url}"),
    }
}

#[test]
fn link_at_rejects_missing_files_and_file_detection_can_be_disabled() {
    let cwd = std::env::temp_dir();

    assert_eq!(
        local_link_at("missing src/nope.rs:1", 9, &one_root(&cwd), true),
        None
    );
    assert_eq!(
        local_link_at("missing src/nope.rs:1", 9, &one_root(&cwd), false),
        None
    );

    let path = temp_file("disabled.rs");
    let line = format!("open {}", path.display());
    assert!(local_link_at(&line, 6, &one_root(&cwd), false).is_none());
}

#[test]
fn link_at_keeps_url_detection_ahead_of_file_detection() {
    let url = "https://example.com/src/main.rs";
    let link = local_link_at(url, 10, &one_root(Path::new("/")), true).expect("URL link");
    assert_eq!(
        link,
        LinkMatch {
            start: 0,
            end: url.len() - 1,
            target: LinkTarget::Url(url.to_string()),
        }
    );
}

#[test]
fn link_at_detects_directory_paths() {
    let file = temp_file("dircase/nested/inner.txt");
    let dir = file.parent().unwrap();
    let cwd = dir.parent().and_then(Path::parent).unwrap();

    let link = local_link_at("artifacts in dircase/nested here", 14, &one_root(cwd), true)
        .expect("directory link");
    assert_eq!((link.start, link.end), (13, 26));
    match link.target {
        LinkTarget::File {
            path,
            line,
            column,
            is_dir,
        } => {
            assert_eq!(path, dir);
            assert_eq!(line, None);
            assert_eq!(column, None);
            assert!(is_dir, "a directory link says so, so the tree can take it");
        }
        LinkTarget::Url(url) => panic!("expected directory link, got URL {url}"),
    }

    assert!(local_link_at("ls dircase/nested/ done", 5, &one_root(cwd), true).is_some());
    assert!(
        local_link_at(
            "artifacts in dircase/nested here",
            14,
            &one_root(cwd),
            false
        )
        .is_none()
    );
}

#[test]
fn a_relative_path_falls_back_to_the_repository_root() {
    // What `cargo` prints from a member directory: the path is measured
    // from the workspace root, not from where the shell happens to be.
    let path = temp_file("multiroot/crates/inner/src/lib.rs");
    let root = path
        .ancestors()
        .nth(4)
        .expect("multiroot/ above crates/inner/src")
        .to_path_buf();
    let cwd = root.join("crates/inner");
    std::fs::create_dir_all(cwd.join("empty")).expect("create a cwd with no src/");

    let line = "warning: unused import crates/inner/src/lib.rs:3";
    let col = line.find("crates").expect("path start");
    assert_eq!(local_link_at(line, col, &one_root(&cwd), true), None);

    let roots = LinkRoots::local(vec![cwd.clone(), root.clone()]);
    let link = local_link_at(line, col, &roots, true).expect("resolved from the repo root");
    match link.target {
        LinkTarget::File { path: got, .. } => assert_eq!(got, path),
        LinkTarget::Url(url) => panic!("expected file link, got URL {url}"),
    }
}

#[test]
fn the_nearer_root_wins_when_both_hold_the_name() {
    let near = temp_file("ambiguous/crate/src/main.rs");
    let far = temp_file("ambiguous/src/main.rs");
    let cwd = near
        .ancestors()
        .nth(2)
        .expect("ambiguous/crate above src/")
        .to_path_buf();
    let root = far.ancestors().nth(2).expect("ambiguous/").to_path_buf();

    let roots = LinkRoots::local(vec![cwd, root]);
    let link = local_link_at("at src/main.rs:1", 3, &roots, true).expect("file link");
    match link.target {
        LinkTarget::File { path, .. } => assert_eq!(
            path, near,
            "the pane's own directory is tried before the repository around it"
        ),
        LinkTarget::Url(url) => panic!("expected file link, got URL {url}"),
    }
}

#[test]
fn an_unanswered_probe_resolves_to_nothing_and_asks_about_every_root() {
    let mut asked: Vec<PathBuf> = Vec::new();
    let roots = LinkRoots::local(vec![PathBuf::from("/work/crate"), PathBuf::from("/work")]);
    let link = link_at("see src/lib.rs:9 there", 5, &roots, true, &mut |path, _| {
        asked.push(path.to_path_buf());
        Probe::Unknown
    });

    assert_eq!(link, None, "nothing underlines until the host has answered");
    assert_eq!(
        asked,
        vec![
            PathBuf::from("/work/crate/src/lib.rs"),
            PathBuf::from("/work/src/lib.rs"),
        ],
        "every root is asked about, so one round trip covers them all"
    );
}

#[test]
fn a_path_shaped_token_is_kept_apart_from_a_bare_word() {
    let path_shaped = file_candidate_at("wrote scratchpad/notes.md now", 8).expect("candidate");
    assert_eq!(path_shaped.path, "scratchpad/notes.md");
    assert!(path_shaped.looks_like_a_path(PathStyle::NATIVE));

    let word = file_candidate_at("wrote notes now", 8).expect("candidate");
    assert_eq!(word.path, "notes");
    assert!(
        !word.looks_like_a_path(PathStyle::NATIVE),
        "a bare word must not raise a notification on every modifier-click"
    );

    let escaped = file_candidate_at(r"wrote a\b now", 6).expect("candidate");
    assert_eq!(escaped.path, r"a\b");
    assert!(
        escaped.looks_like_a_path(PathStyle::Windows),
        "a backslash separates directories in a Windows pane"
    );
    assert!(
        !escaped.looks_like_a_path(PathStyle::Posix),
        "and escapes a space in a POSIX one, whatever this client runs"
    );
}

#[test]
fn a_relative_candidate_has_no_paths_without_a_root() {
    let candidate = file_candidate_at("see src/lib.rs here", 5).expect("candidate");
    assert!(
        candidate.paths(&LinkRoots::default()).is_empty(),
        "a pane that never said where it is cannot measure a relative path"
    );
    assert_eq!(
        candidate.paths(&LinkRoots::local(vec![
            PathBuf::from("/w"),
            PathBuf::from("/w")
        ])),
        vec![PathBuf::from("/w/src/lib.rs")],
        "a repo root equal to the cwd is one root, not two probes"
    );
}

#[test]
fn an_absolute_candidate_ignores_the_roots_entirely() {
    let candidate = file_candidate_at("open /etc/hosts now", 6).expect("candidate");
    assert_eq!(
        candidate.paths(&LinkRoots::local(vec![PathBuf::from("/w")])),
        vec![PathBuf::from("/etc/hosts")]
    );
}

/// `is_rooted` decides whether a report about an unresolved token may name
/// a directory it was "looked for under", so it has to agree with
/// [`FileCandidate::paths`] about when the roots are consulted at all.
///
/// Both used to ask `Path::is_absolute`, which answers for the machine
/// anterm runs on rather than the one the pane's paths are on — so this only
/// held on Unix and was gated to it. Both now ask the pane's own dialect,
/// and the agreement holds on every client.
#[test]
fn a_rooted_candidate_is_told_apart_from_one_measured_from_a_root() {
    let posix_roots = LinkRoots {
        dirs: vec![PathBuf::from("/home/u/proj")],
        local_home: false,
        style: PathStyle::Posix,
    };
    for line in ["open /etc/hosts now", "open ~/.zshrc now"] {
        let candidate = file_candidate_at(line, 6).expect("candidate");
        assert!(
            candidate.is_rooted(PathStyle::Posix),
            "{line} says for itself where it starts"
        );
        let paths = candidate.paths(&posix_roots);
        assert!(
            !paths.iter().any(|p| p.starts_with("/home/u/proj")),
            "{line} was not measured from the pane's directory: {paths:?}"
        );
    }
    for line in [r"open C:\Windows\win.ini now", "open ~/.gitconfig now"] {
        assert!(
            file_candidate_at(line, 6)
                .expect("candidate")
                .is_rooted(PathStyle::Windows),
            "{line} says for itself where it starts"
        );
    }
    for style in [PathStyle::Posix, PathStyle::Windows] {
        assert!(
            !file_candidate_at("see src/lib.rs here", 5)
                .expect("candidate")
                .is_rooted(style),
            "a relative path is only ever found by measuring from somewhere"
        );
    }
}

/// The bug this whole [`PathStyle`] exists for: a Windows anterm looking at
/// a Linux pane — a remote workspace, a native-SSH pane, or a WSL distro
/// reached as a host — used to read `/etc/hosts` as a *relative* path,
/// because `Path::is_absolute` speaks for the client and a leading `/` is
/// not absolute on Windows. The pane's paths were then measured from its
/// own directory and the far side was asked about something it never
/// printed, so nothing ever underlined.
#[test]
fn a_posix_pane_roots_its_own_paths_on_every_client() {
    let roots = LinkRoots {
        dirs: vec![PathBuf::from("/home/u/proj")],
        local_home: false,
        style: PathStyle::Posix,
    };
    let candidate = file_candidate_at("open /etc/hosts now", 6).expect("candidate");
    assert_eq!(
        candidate.paths(&roots),
        vec![PathBuf::from("/etc/hosts")],
        "the pane said where the path starts; the roots have nothing to add"
    );

    // And with no cwd reported at all there is still exactly one thing it
    // can mean. This is the case that failed outright: no roots meant no
    // paths, so the host was never even asked.
    let roots = LinkRoots {
        dirs: Vec::new(),
        local_home: false,
        style: PathStyle::Posix,
    };
    assert_eq!(candidate.paths(&roots), vec![PathBuf::from("/etc/hosts")]);
}

/// A relative path is the other half, and it fails more quietly: the join
/// used to reach for the *client's* separator, so a Windows anterm asked a
/// Linux host about `/home/u/proj\src/lib.rs`.
#[test]
fn a_posix_pane_joins_a_relative_path_with_its_own_separator() {
    let roots = LinkRoots {
        dirs: vec![PathBuf::from("/home/u/proj"), PathBuf::from("/home/u")],
        local_home: false,
        style: PathStyle::Posix,
    };
    let paths = file_candidate_at("see src/lib.rs here", 5)
        .expect("candidate")
        .paths(&roots);
    assert_eq!(
        paths
            .iter()
            .map(|p| p.to_string_lossy())
            .collect::<Vec<_>>(),
        vec!["/home/u/proj/src/lib.rs", "/home/u/src/lib.rs"],
        "the string the far side is asked about has to be one it can stat"
    );
}

/// `~` in a POSIX pane is read out of the pane's own cwd — on every
/// client. The rule was gated to Unix ones, so a Windows anterm could not
/// resolve `~/.zshrc` in any pane it was looking at.
#[test]
fn a_posix_pane_reads_a_tilde_out_of_its_own_cwd() {
    let roots = LinkRoots {
        dirs: vec![PathBuf::from("/home/u/proj")],
        local_home: false,
        style: PathStyle::Posix,
    };
    let paths = file_candidate_at("open ~/.zshrc now", 6)
        .expect("candidate")
        .paths(&roots);
    assert_eq!(
        paths
            .iter()
            .map(|p| p.to_string_lossy())
            .collect::<Vec<_>>(),
        vec!["/home/u/.zshrc"]
    );

    let no_home = LinkRoots {
        dirs: vec![PathBuf::from("/srv/app")],
        local_home: false,
        style: PathStyle::Posix,
    };
    assert!(
        file_candidate_at("open ~/.zshrc now", 6)
            .expect("candidate")
            .paths(&no_home)
            .is_empty(),
        "a cwd that reveals no home may not borrow this machine's (#568)"
    );
}

/// The deliberate other half: a pane *on this machine* keeps this OS's
/// reading of its own output. `/etc` in a `cmd.exe` pane sitting on `C:`
/// means `C:\etc`, the way `cd /etc` does there — so it is measured from
/// the pane's root and not turned into a link to a file Windows has not
/// got. Pinned because the temptation is to make every `/`-rooted token
/// stand alone, and that would underline `/etc/hosts` in a PowerShell pane
/// with nothing behind it.
#[test]
fn a_local_pane_reads_its_own_output_the_way_its_own_os_does() {
    let candidate = file_candidate_at("open /etc/hosts now", 6).expect("candidate");
    assert_eq!(
        candidate.is_rooted(PathStyle::NATIVE),
        cfg!(unix),
        "a leading slash roots a path on Unix and names a drive-relative \
         directory on Windows"
    );

    let roots = LinkRoots::local(vec![PathBuf::from("/w")]);
    assert_eq!(
        roots.style,
        PathStyle::NATIVE,
        "a pane on this machine spells paths this machine's way"
    );
    assert_eq!(
        candidate.paths(&roots),
        vec![PathBuf::from("/etc/hosts")],
        "which comes to the same thing under a root with no drive letter"
    );
}

/// The Windows half of the rule above, spelled out where it can be: the
/// drive the pane is on is what a leading `/` there is measured from.
/// Cannot be asserted on a Unix client, where `PathBuf` has no notion of a
/// drive at all — the *rule* is pinned ungated above, this is the reading.
#[test]
#[cfg(windows)]
fn a_local_windows_pane_measures_a_leading_slash_from_its_own_drive() {
    let roots = LinkRoots::local(vec![PathBuf::from(r"C:\proj")]);
    assert_eq!(
        file_candidate_at("open /etc/hosts now", 6)
            .expect("candidate")
            .paths(&roots),
        vec![PathBuf::from(r"C:\etc\hosts")],
        "`/etc` in a cmd.exe pane on C: is C:\\etc, the way `cd /etc` is"
    );
}

#[test]
fn a_windows_pane_roots_a_drive_letter_and_a_share() {
    let style = PathStyle::Windows;
    for rooted in [r"C:\Windows", "C:/Windows", r"\\server\share\x"] {
        assert!(style.is_absolute(rooted), "{rooted} names its own root");
    }
    for measured in ["/etc/hosts", "C:notes.txt", r"src\lib.rs", "", "C:"] {
        assert!(
            !style.is_absolute(measured),
            "{measured} has to be measured from somewhere"
        );
    }
    for measured in [r"C:\Windows", r"src\lib.rs", ""] {
        assert!(
            !PathStyle::Posix.is_absolute(measured),
            "{measured} is not a POSIX root"
        );
    }
}

#[test]
fn a_hosts_dialect_is_read_off_the_directory_it_reports() {
    assert_eq!(
        PathStyle::of_dir(Path::new("/home/u/proj")),
        PathStyle::Posix
    );
    assert_eq!(
        PathStyle::of_dir(Path::new(r"C:\Users\u\proj")),
        PathStyle::Windows
    );
    assert_eq!(
        PathStyle::of_dir(Path::new(r"\\wsl$\Ubuntu\home\u")),
        PathStyle::Windows
    );
    // A directory that names no root at all says nothing about the host,
    // so it has to fall the way no directory does — POSIX. Reading it as
    // Windows would hand a Linux host `\`-joined paths on the strength of
    // a cwd it never really reported.
    for nothing_to_go_on in ["", "proj", "~/proj", "C:notes"] {
        assert_eq!(
            PathStyle::of_dir(Path::new(nothing_to_go_on)),
            PathStyle::Posix,
            "{nothing_to_go_on:?} names no Windows root"
        );
    }
}

#[test]
fn link_at_requires_a_file_when_a_line_suffix_is_present() {
    let file = temp_file("localhost/keep.txt");
    let cwd = file.parent().and_then(Path::parent).unwrap();

    assert_eq!(
        local_link_at("listening on localhost:8080", 15, &one_root(cwd), true),
        None
    );
    assert!(local_link_at("listening on localhost", 15, &one_root(cwd), true).is_some());
}

/// The span a link underlines, as `(start, end)` inclusive columns.
fn link_span(line: &str, col: usize, cwd: &Path) -> (usize, usize) {
    let link = local_link_at(line, col, &one_root(cwd), true).expect("file link under cursor");
    (link.start, link.end)
}

/// A path is rarely written on its own: build tools glue a flag, a label
/// or a diff side onto the front of it, and none of that is the path.
#[test]
fn a_path_is_found_inside_the_prefix_glued_onto_it() {
    let path = temp_file("prefixed/src/main.rs");
    let cwd = path.parent().and_then(Path::parent).unwrap();
    let at = |line: &str| {
        let byte = line.find("src/main.rs").expect("path start");
        let col = line[..byte].chars().count();
        (link_span(line, col, cwd), col)
    };

    for line in [
        "rustc --emit=metadata --file=src/main.rs",
        "note:src/main.rs",
        "--- a/src/main.rs",
        "+++ b/src/main.rs",
        "├──src/main.rs",
    ] {
        let ((start, end), col) = at(line);
        assert_eq!(
            (start, end),
            (col, col + "src/main.rs".chars().count() - 1),
            "{line:?} must underline the path and nothing around it"
        );
        assert_file_link(line, col, cwd, &path, None, None);
    }
}

/// `ls -F` says what it is listing by writing the type onto the name.
#[test]
fn an_ls_type_marker_is_not_part_of_the_name() {
    let path = temp_file("marked/run.sh");
    let cwd = path.parent().unwrap();

    for marker in ['@', '*', '=', '|'] {
        let line = format!("run.sh{marker}  other");
        assert_file_link(&line, 2, cwd, &path, None, None);
        assert_eq!(link_span(&line, 2, cwd), (0, 5), "{marker} is not a name");
    }
}

/// What every compiler that grew up on Windows prints, `tsc` included.
#[test]
fn a_parenthesised_line_and_column_is_read_as_a_location() {
    let path = temp_file("msvc/app.ts");
    let cwd = path.parent().and_then(Path::parent).unwrap();

    assert_file_link(
        "msvc/app.ts(10,2): error TS2304",
        2,
        cwd,
        &path,
        Some(10),
        Some(2),
    );
    assert_file_link("msvc/app.ts(10): warning", 2, cwd, &path, Some(10), None);
    assert_file_link("msvc/app.ts#L7 pasted", 2, cwd, &path, Some(7), None);
}

/// The same spelling names a file on a machine that has downloaded
/// something twice, so the written-out reading is tried first.
#[test]
fn a_filename_that_looks_like_a_location_wins_over_the_location() {
    let path = temp_file("dupes/backup(1)");
    let cwd = path.parent().and_then(Path::parent).unwrap();

    assert_file_link("kept dupes/backup(1) here", 5, cwd, &path, None, None);
}

/// Python puts the line number two tokens away from the file, and that is
/// the single place where clicking a path is worth the most.
#[test]
fn a_line_number_written_beside_a_path_is_still_a_location() {
    let path = temp_file("beside/handlers.py");
    let cwd = path.parent().and_then(Path::parent).unwrap();
    // Found by the whole path rather than by its first separator: a
    // Windows temp path keeps the `/` this one was built with, and looking
    // for a separator lands three quarters of the way along it.
    let shown = path.display().to_string();
    let quoted = format!("  File \"{shown}\", line 214, in dispatch");
    let byte = quoted.find(&shown).expect("the path in the line");
    let col = quoted[..byte].chars().count();

    assert_file_link(&quoted, col, Path::new("/"), &path, Some(214), None);
    assert_eq!(
        link_span(&quoted, col, Path::new("/")),
        (col, col + path.to_string_lossy().chars().count() - 1),
        "the underline stays on the path, not on the prose after it"
    );

    assert_file_link(
        "beside/handlers.py: line 12: bad substitution",
        2,
        cwd,
        &path,
        Some(12),
        None,
    );
    assert_file_link("beside/handlers.py lineage 12", 2, cwd, &path, None, None);
}

/// A directory has no line to land on, so nothing beside it is read as
/// one.
#[test]
fn nothing_beside_a_directory_is_read_as_a_line_number() {
    let file = temp_file("nolines/inner/keep.txt");
    let dir = file.parent().unwrap();
    let cwd = dir.parent().and_then(Path::parent).unwrap();

    assert_file_link("nolines/inner, line 12", 2, cwd, dir, None, None);
}

/// A trailing period ends a sentence, except where it is the directory.
#[test]
fn a_dot_that_names_a_directory_survives_the_trailing_trim() {
    let file = temp_file("dotdir/inner/keep.txt");
    let inner = file.parent().unwrap();
    let cwd = inner.parent().unwrap();

    let link = local_link_at("copied to inner/. now", 10, &one_root(cwd), true)
        .expect("a dot directory is a directory");
    match link.target {
        LinkTarget::File { path, is_dir, .. } => {
            assert_eq!(path, cwd.join("inner/."));
            assert!(is_dir);
        }
        LinkTarget::Url(url) => panic!("expected directory link, got URL {url}"),
    }
}

/// A label in front of a colon only peels off when what follows is
/// written like a path. Otherwise a prompt segment turns into a link the
/// moment the directory happens to hold a folder by that name.
#[test]
fn a_label_in_front_of_a_bare_word_is_not_a_path() {
    let file = temp_file("labelled/main/keep.txt");
    // `<tmp>/labelled`, the one directory that holds a `main/`.
    let labelled = file.parent().and_then(Path::parent).unwrap();

    assert!(
        local_link_at("on main/keep.txt", 4, &one_root(labelled), true).is_some(),
        "the directory itself is still reachable"
    );
    assert!(
        local_link_at("branch:main", 8, &one_root(labelled), true).is_none(),
        "but the branch a prompt is reporting is not a link"
    );
    assert!(
        local_link_at("note:main/keep.txt", 6, &one_root(labelled), true).is_some(),
        "a label in front of something written like a path still peels"
    );
}

/// Every reading of one token is worth at most a handful of questions.
#[test]
fn the_candidate_ladder_stays_short() {
    let dense = "a/b=note:d/e/f.rs:10:2";
    let candidates = file_candidates_at(dense, 12);
    assert!(
        candidates.len() <= MAX_FILE_CANDIDATES,
        "{} readings of {dense:?}",
        candidates.len()
    );
    assert!(
        candidates.iter().any(|c| c.path == "d/e/f.rs"),
        "the reading behind the `note:` label is still on the ladder"
    );
}

/// A Windows pane sitting in `C:\proj`, the way the reporter of #965 saw it.
fn windows_roots() -> LinkRoots {
    LinkRoots {
        dirs: vec![PathBuf::from(r"C:\proj")],
        local_home: false,
        style: PathStyle::Windows,
    }
}

/// `link_at` over `line` with the pointer on the first char of `needle`,
/// against a filesystem holding exactly `exists` — and every path it was
/// asked about, so a test can say what was *not* looked up.
fn link_with(
    line: &str,
    needle: &str,
    roots: &LinkRoots,
    exists: &str,
) -> (Option<LinkMatch>, Vec<String>) {
    let byte = line.find(needle).expect("needle in line");
    let col = line[..byte].chars().count();
    let mut asked = Vec::new();
    let mut probe = |path: &Path, _: bool| {
        let path = path.to_string_lossy().into_owned();
        let hit = path == exists;
        asked.push(path);
        match hit {
            true => Probe::Hit { is_dir: false },
            false => Probe::Miss,
        }
    };
    (link_at(line, col, roots, true, &mut probe), asked)
}

/// The path and span a Windows pane underlines under `needle`.
fn windows_link(line: &str, needle: &str, exists: &str) -> (String, usize, usize) {
    let (link, asked) = link_with(line, needle, &windows_roots(), exists);
    let link = link.unwrap_or_else(|| panic!("{line:?}: no link, asked {asked:?}"));
    match link.target {
        LinkTarget::File { path, .. } => {
            (path.to_string_lossy().into_owned(), link.start, link.end)
        }
        LinkTarget::Url(url) => panic!("{line:?}: expected a file link, got {url}"),
    }
}

/// The columns `needle` occupies in `line`, inclusive.
fn span_of(line: &str, needle: &str) -> (usize, usize) {
    let byte = line.find(needle).expect("needle in line");
    let start = line[..byte].chars().count();
    (start, start + needle.chars().count() - 1)
}

/// #965: the path exactly as the reporter pasted it, and its backslashed
/// twin. Either separator names the same file, and the file is handed on
/// spelled the way Explorer will take it.
#[test]
fn a_drive_letter_path_is_a_link_with_either_separator() {
    let want = r"C:\Users\xxxxx\images\gen_gptimage2_gen_20260925_220725.png";
    for written in [
        "c:/Users/xxxxx/images/gen_gptimage2_gen_20260925_220725.png",
        r"C:\Users\xxxxx\images\gen_gptimage2_gen_20260925_220725.png",
        r"C:/Users\xxxxx/images\gen_gptimage2_gen_20260925_220725.png",
    ] {
        let line = format!("saved {written} ok");
        let (path, start, end) = windows_link(&line, "images", want);
        assert_eq!(path, want, "{line:?}");
        assert_eq!((start, end), span_of(&line, written), "{line:?}");
    }
}

/// Prose and markup around a Windows path is not part of it — the same
/// trimming a POSIX path gets.
#[test]
fn a_drive_letter_path_sheds_the_punctuation_around_it() {
    let want = r"C:\out\a.png";
    for (line, path) in [
        ("wrote c:/out/a.png.", "c:/out/a.png"),
        (r"wrote C:\out\a.png, then quit", r"C:\out\a.png"),
        (r"(see C:\out\a.png)", r"C:\out\a.png"),
        (r#"open "C:\out\a.png";"#, r"C:\out\a.png"),
        (r"`C:\out\a.png`", r"C:\out\a.png"),
        ("「c:/out/a.png」。", "c:/out/a.png"),
        ("<c:/out/a.png>", "c:/out/a.png"),
        ("│ C:/out/a.png │", "C:/out/a.png"),
        (r"C:\out\a.png:", r"C:\out\a.png"),
    ] {
        let (got, start, end) = windows_link(line, "out", want);
        assert_eq!(got, want, "{line:?}");
        assert_eq!((start, end), span_of(line, path), "{line:?}");
    }
}

/// Compilers on Windows write the location onto the path; the drive's own
/// colon must not be mistaken for one.
#[test]
fn a_drive_letter_path_keeps_its_line_and_column() {
    let want = r"C:\src\main.rs";
    for (line, loc) in [
        (r"C:\src\main.rs:10:2: error", (Some(10), Some(2))),
        ("c:/src/main.rs:10", (Some(10), None)),
        (r"C:\src\main.rs(10,2): error C2065", (Some(10), Some(2))),
        ("c:/src/main.rs#L10", (Some(10), None)),
        (r#"File "C:\src\main.rs", line 7"#, (Some(7), None)),
    ] {
        let (link, asked) = link_with(line, "src", &windows_roots(), want);
        match link.map(|l| l.target) {
            Some(LinkTarget::File {
                path,
                line: l,
                column,
                ..
            }) => {
                assert_eq!(path, PathBuf::from(want), "{line:?}");
                assert_eq!((l, column), loc, "{line:?}");
            }
            other => panic!("{line:?}: {other:?}, asked {asked:?}"),
        }
    }
}

/// Chinese output glues a path straight onto the sentence in front of it,
/// often behind a full-width colon — which is how an agent reports the
/// file it just wrote in #965. The sentence is not part of the path.
#[test]
fn a_drive_letter_path_is_found_inside_the_prose_glued_onto_it() {
    let want = r"C:\Users\me\a.png";
    for (line, path) in [
        ("图片已保存到：c:/Users/me/a.png", "c:/Users/me/a.png"),
        ("图片已生成c:/Users/me/a.png", "c:/Users/me/a.png"),
        (r"路径:C:\Users\me\a.png", r"C:\Users\me\a.png"),
        ("--out=c:/Users/me/a.png", "c:/Users/me/a.png"),
        ("output:c:/Users/me/a.png", "c:/Users/me/a.png"),
    ] {
        let (got, start, end) = windows_link(line, "Users", want);
        assert_eq!(got, want, "{line:?}");
        assert_eq!((start, end), span_of(line, path), "{line:?}");
    }
}

/// An agent prints the image it made as Markdown. The target is the link;
/// the brackets and alt text are not.
#[test]
fn a_markdown_link_target_is_the_path() {
    let want = r"C:\out\chart.png";
    for line in [
        "![chart](c:/out/chart.png)",
        "see [the chart](c:/out/chart.png).",
    ] {
        let (got, start, end) = windows_link(line, "out", want);
        assert_eq!(got, want, "{line:?}");
        assert_eq!((start, end), span_of(line, "c:/out/chart.png"), "{line:?}");
    }
    let posix = LinkRoots {
        dirs: vec![PathBuf::from("/home/u/proj")],
        local_home: false,
        style: PathStyle::Posix,
    };
    let line = "[notes](docs/notes.md)";
    let (link, asked) = link_with(line, "docs", &posix, "/home/u/proj/docs/notes.md");
    let link = link.unwrap_or_else(|| panic!("no link, asked {asked:?}"));
    assert_eq!((link.start, link.end), span_of(line, "docs/notes.md"));
}

#[test]
fn a_unc_share_is_a_link_that_stands_alone() {
    let want = r"\\server\share\team\a.png";
    let line = r"copied to \\server\share\team\a.png";
    let (got, start, end) = windows_link(line, "server", want);
    assert_eq!(got, want);
    assert_eq!((start, end), span_of(line, want));
}

/// Text that merely has a letter and a colon in it is not a drive path,
/// and must never be looked up as one.
#[test]
fn a_letter_and_a_colon_is_not_a_drive_path() {
    for (line, needle) in [
        // Drive-relative, or a bare drive: nothing a pane can resolve.
        ("a:b", "a:b"),
        ("x:y and more", "x:y"),
        ("C:notes.txt", "C:notes"),
        ("see C: now", "C:"),
        // A time, a ratio, a port.
        ("at 12:30 today", "12:30"),
        ("ratio 16:9", "16:9"),
        ("localhost:8080", "localhost"),
        // A word that ends in a letter is not a drive behind it.
        ("abc:/x/y", "abc"),
        ("http:/x", "http"),
    ] {
        let (link, asked) = link_with(line, needle, &windows_roots(), "");
        assert_eq!(link, None, "{line:?}");
        for path in &asked {
            assert!(
                path.starts_with(r"C:\proj\"),
                "{line:?} asked about {path:?}, which is not under the pane's directory"
            );
        }
    }
    // The drive-relative ones are not asked about at all.
    for line in ["a:b", "C:notes.txt"] {
        let (_, asked) = link_with(line, line, &windows_roots(), "");
        assert!(asked.is_empty(), "{line:?} asked about {asked:?}");
    }
    // And nothing in `abc:/x/y` is read as drive `c:`.
    assert!(
        file_candidates_at("abc:/x/y", 0)
            .iter()
            .all(|c| !c.path.starts_with("c:")),
        "{:?}",
        file_candidates_at("abc:/x/y", 0)
    );
}

/// A Windows path that does not exist is not underlined: the probe is
/// the gate, exactly as for a POSIX one.
#[test]
fn a_missing_drive_letter_path_is_not_a_link() {
    let (link, asked) = link_with("wrote c:/out/gone.png", "out", &windows_roots(), "");
    assert_eq!(link, None);
    assert_eq!(asked, vec![r"C:\out\gone.png".to_string()]);
    let candidate =
        unresolved_candidate("wrote c:/out/gone.png", 8, PathStyle::Windows).unwrap();
    assert!(candidate.looks_like_a_path(PathStyle::Windows));
    assert!(candidate.is_rooted(PathStyle::Windows));
}

/// A POSIX pane has no drives, but a WSL distro can reach the Windows
/// machine it runs on through `/mnt/<drive>`, so that is what it is asked.
#[test]
fn a_posix_pane_reads_a_drive_letter_through_the_wsl_mount() {
    let roots = LinkRoots {
        dirs: vec![PathBuf::from("/home/u/proj")],
        local_home: false,
        style: PathStyle::Posix,
    };
    let want = "/mnt/c/Users/me/a.png";
    for written in ["c:/Users/me/a.png", r"C:\Users\me\a.png"] {
        let line = format!("wrote {written}");
        let (link, asked) = link_with(&line, "Users", &roots, want);
        assert_eq!(asked.first().map(String::as_str), Some(want), "{line:?}");
        let link = link.unwrap_or_else(|| panic!("{line:?}: no link"));
        assert_eq!((link.start, link.end), span_of(&line, written));
        let candidate = file_candidate_at(&line, 8).unwrap();
        assert!(candidate.is_rooted(PathStyle::Posix), "{line:?}");
        assert!(candidate.looks_like_a_path(PathStyle::Posix), "{line:?}");
    }
    // Where there is no such mount the probe misses and nothing underlines.
    let (link, _) = link_with("wrote c:/Users/me/a.png", "Users", &roots, "");
    assert_eq!(link, None);
}

#[test]
fn a_rooted_token_is_spelled_the_way_its_host_looks_it_up() {
    let w = PathStyle::Windows;
    assert_eq!(w.rooted("c:/Users/me").as_deref(), Some(r"C:\Users\me"));
    assert_eq!(w.rooted(r"D:\x/y").as_deref(), Some(r"D:\x\y"));
    assert_eq!(
        w.rooted(r"\\server\share\x").as_deref(),
        Some(r"\\server\share\x")
    );
    for relative in ["src/lib.rs", r"src\lib.rs", "C:x", "C:", "/etc/hosts", ""] {
        assert_eq!(w.rooted(relative), None, "{relative:?}");
    }

    let p = PathStyle::Posix;
    assert_eq!(p.rooted("/etc/hosts").as_deref(), Some("/etc/hosts"));
    assert_eq!(p.rooted(r"C:\Users\me").as_deref(), Some("/mnt/c/Users/me"));
    assert_eq!(p.rooted("d:/").as_deref(), Some("/mnt/d/"));
    for relative in ["src/lib.rs", "C:x", r"\\server\share", "12:30"] {
        assert_eq!(p.rooted(relative), None, "{relative:?}");
    }
}

/// The Windows join is textual so a Mac looking at a Windows host asks it
/// about a path it can stat, and so it can be pinned here at all.
#[test]
fn a_windows_join_spells_the_whole_path_the_windows_way() {
    let w = PathStyle::Windows;
    let join =
        |root: &str, rel: &str| w.join(Path::new(root), rel).to_string_lossy().into_owned();
    assert_eq!(join(r"C:\proj", "src/lib.rs"), r"C:\proj\src\lib.rs");
    assert_eq!(join(r"C:\proj\", r"src\lib.rs"), r"C:\proj\src\lib.rs");
    assert_eq!(join("c:/proj", "a.txt"), r"C:\proj\a.txt");
    assert_eq!(join(r"C:\", "a.txt"), r"C:\a.txt");
    // A leading separator is rooted on the root's own drive or share.
    assert_eq!(join(r"C:\proj", "/etc/hosts"), r"C:\etc\hosts");
    assert_eq!(join(r"D:\proj", r"\tmp\x"), r"D:\tmp\x");
    assert_eq!(join(r"\\srv\share\proj", r"\x"), r"\\srv\share\x");
    assert_eq!(join(r"\\srv\share\proj", "x"), r"\\srv\share\proj\x");
    assert_eq!(windows_prefix(r"\\srv\share"), r"\\srv\share");
    assert_eq!(windows_prefix("C:"), "C:");
    assert_eq!(windows_prefix("proj"), "");
}

#[test]
fn a_path_handed_to_the_os_is_spelled_its_way() {
    assert_eq!(
        spelled_for(PathStyle::Windows, Path::new("c:/Users/me/a b.png")),
        PathBuf::from(r"C:\Users\me\a b.png")
    );
    assert_eq!(
        spelled_for(PathStyle::Windows, Path::new(r"\\srv/share/x")),
        PathBuf::from(r"\\srv\share\x")
    );
    assert_eq!(
        spelled_for(PathStyle::Posix, Path::new("/Users/me/a\\b.png")),
        PathBuf::from("/Users/me/a\\b.png"),
        "a POSIX backslash is a filename character, not a separator"
    );
}

fn posix_roots() -> LinkRoots {
    LinkRoots {
        dirs: vec![PathBuf::from("/home/u/proj")],
        local_home: false,
        style: PathStyle::Posix,
    }
}

/// The file link under the first char of `needle`, as (path, line,
/// column, start, end) — or `None`, with what was asked about.
fn file_link_with(
    line: &str,
    needle: &str,
    roots: &LinkRoots,
    exists: &str,
) -> Result<(String, Option<u32>, Option<u32>, usize, usize), Vec<String>> {
    let (link, asked) = link_with(line, needle, roots, exists);
    match link {
        Some(LinkMatch {
            start,
            end,
            target:
                LinkTarget::File {
                    path, line, column, ..
                },
        }) => Ok((
            path.to_string_lossy().into_owned(),
            line,
            column,
            start,
            end,
        )),
        _ => Err(asked),
    }
}

/// A quote is where a path is allowed its spaces: every word of it is the
/// same link, and the quotes are not part of it.
#[test]
fn a_quoted_windows_path_keeps_its_spaces() {
    let want = r"C:\Program Files\foo\bar.exe";
    for line in [
        r#"run "C:\Program Files\foo\bar.exe" now"#,
        r"run 'C:\Program Files\foo\bar.exe' now",
        r"run `C:\Program Files\foo\bar.exe` now",
        r#"run "c:/Program Files/foo/bar.exe" now"#,
    ] {
        let written = &line[5..line.len() - 5];
        for needle in ["Program", "Files", "bar.exe"] {
            let got = file_link_with(line, needle, &windows_roots(), want);
            assert_eq!(
                got,
                Ok((
                    want.to_string(),
                    None,
                    None,
                    span_of(line, written).0,
                    span_of(line, written).1
                )),
                "{line:?} at {needle:?}"
            );
        }
    }
}

#[test]
fn a_quoted_posix_path_keeps_its_spaces_and_its_location() {
    let roots = posix_roots();
    let docs = "/Users/me/My Docs/a.txt";
    let line = "open '/Users/me/My Docs/a.txt' please";
    let (start, end) = span_of(line, docs);
    assert_eq!(
        file_link_with(line, "Docs", &roots, docs),
        Ok((docs.to_string(), None, None, start, end))
    );

    let file = "/home/u/proj/src/my file.rs";
    for (line, loc) in [
        (
            r#"error: "src/my file.rs:10:2" failed"#,
            (Some(10), Some(2)),
        ),
        (
            r#"error: "src/my file.rs":10:2: failed"#,
            (Some(10), Some(2)),
        ),
        (
            r#"error: "src/my file.rs"(10,2): failed"#,
            (Some(10), Some(2)),
        ),
        (
            r#"File "src/my file.rs", line 7, in <module>"#,
            (Some(7), None),
        ),
        (r#"see "src/my file.rs"."#, (None, None)),
    ] {
        let got = file_link_with(line, "file", &roots, file)
            .unwrap_or_else(|asked| panic!("{line:?}: no link, asked {asked:?}"));
        assert_eq!(got.0, file, "{line:?}");
        assert_eq!((got.1, got.2), loc, "{line:?}");
        let byte = line.find("src/").unwrap();
        assert_eq!(
            got.3,
            line[..byte].chars().count(),
            "{line:?} starts inside the quote"
        );
    }
}

/// Outside a quote a space still ends a path — the part after it is not
/// guessed onto the part before.
#[test]
fn an_unquoted_space_still_ends_a_path() {
    let want = r"C:\Program Files\foo\bar.exe";
    let line = r"run C:\Program Files\foo\bar.exe now";
    for needle in ["Program", "Files", "bar.exe"] {
        assert!(
            file_link_with(line, needle, &windows_roots(), want).is_err(),
            "{needle:?}"
        );
    }
    let docs = "/Users/me/My Docs/a.txt";
    assert!(
        file_link_with("open /Users/me/My Docs/a.txt", "Docs", &posix_roots(), docs).is_err()
    );
}

/// Quotes around ordinary prose are not a path, and one that looks enough
/// like one to be asked about is asked about once.
#[test]
fn a_quote_around_prose_is_not_a_path() {
    let spaced = |line: &str, needle: &str| {
        let byte = line.find(needle).unwrap();
        file_candidates_at(line, line[..byte].chars().count())
            .into_iter()
            .filter(|c| c.path.contains(' '))
            .collect::<Vec<_>>()
    };
    for (line, needle) in [
        (r#"say "hello world" twice"#, "world"),
        (r#"say " /a b/c " twice"#, "b/c"),
        (r#"mismatched "/a b/c' here"#, "b/c"),
        (r#"a "x" and "y/z w""#, "and"),
        ("it's a /tmp/x dir", "tmp"),
        ("tab\"/a\tb/c\"", "b/c"),
    ] {
        assert_eq!(spaced(line, needle), Vec::new(), "{line:?}");
    }
    let long = format!("\"/{} x/y\"", "a".repeat(MAX_QUOTED_PATH));
    assert_eq!(spaced(&long, "x/y"), Vec::new(), "past the cap");
    let at_cap = format!("\"/{} x/y\"", "a".repeat(MAX_QUOTED_PATH - 6));
    assert_eq!(spaced(&at_cap, "x/y").len(), 1, "at the cap");

    // A quoted sentence that clears the shape check and names nothing
    // is one question, not one per word.
    let line = r#"note "see the docs/ folder and the src/ tree" ok"#;
    for needle in ["see", "folder", "tree"] {
        let (link, asked) = link_with(line, needle, &posix_roots(), "");
        assert_eq!(link, None);
        let spaced: Vec<_> = asked.iter().filter(|p| p.contains(' ')).collect();
        assert_eq!(spaced.len(), 1, "{needle:?} asked {asked:?}");
    }
}
