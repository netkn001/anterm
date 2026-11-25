use super::*;
use crate::git::test_support::{PINS, pin_repo_config};

fn commit(sha: &str, parents: &[&str]) -> (Oid, SmallVec<[Oid; 2]>) {
    (
        sha.to_string(),
        parents.iter().map(|p| p.to_string()).collect(),
    )
}

fn lay_out(page: &[(Oid, SmallVec<[Oid; 2]>)]) -> Vec<GraphRow> {
    let mut rows = Vec::new();
    LaneAlloc::new().push(page, &mut rows);
    rows
}

fn pass_at(lane: Lane) -> Edge {
    Edge::Pass { lane, color: lane }
}

fn in_at(lane: Lane) -> Edge {
    Edge::In {
        from: lane,
        color: lane,
    }
}

fn out_at(lane: Lane) -> Edge {
    Edge::Out {
        to: lane,
        color: lane,
    }
}

/// Lanes crossing the row's top edge, sorted.
fn top(row: &GraphRow) -> Vec<Lane> {
    let mut lanes: Vec<Lane> = row
        .edges
        .iter()
        .filter_map(|e| match *e {
            Edge::Pass { lane, .. } => Some(lane),
            Edge::In { from, .. } => Some(from),
            Edge::Out { .. } => None,
        })
        .collect();
    lanes.sort_unstable();
    lanes
}

/// Lanes crossing the row's bottom edge, sorted and folded.
///
/// Folded, because one lane can legally carry a `Pass` *and* an `Out`: a
/// merge whose second parent already has a lane reserved by another child
/// sends its `Out` onto that lane, joining the line rather than opening a
/// second one. Below the row that is a single line in a single colour (an
/// `Out`'s colour is its lane), so the cut sees one line — which the
/// assertions below verify before folding.
fn bottom(row: &GraphRow) -> Vec<Lane> {
    let mut lanes: Vec<Lane> = row
        .edges
        .iter()
        .filter_map(|e| match *e {
            Edge::Pass { lane, .. } => Some(lane),
            Edge::Out { to, .. } => Some(to),
            Edge::In { .. } => None,
        })
        .collect();
    lanes.sort_unstable();
    lanes.dedup();
    lanes
}

/// The property the whole layout rests on: at any horizontal cut through
/// the graph a lane carries at most one visible line, and what leaves a
/// row's bottom is exactly what enters the next row's top. Together those
/// two mean colour-by-lane can never put two visible lines in one colour.
///
/// "Visible" carries the one nuance: an `Out` may land on a lane a `Pass`
/// already crosses — a join, see [`bottom`] — and that pair is one line.
/// Two `Pass`es or two `Out`s on one lane are still bugs.
fn assert_lanes_line_up(rows: &[GraphRow]) {
    for (i, row) in rows.iter().enumerate() {
        let once = |mut lanes: Vec<Lane>| {
            lanes.sort_unstable();
            let len = lanes.len();
            lanes.dedup();
            assert_eq!(lanes.len(), len, "row {i} doubles up a lane: {row:?}");
        };
        once(top(row));
        once(
            row.edges
                .iter()
                .filter_map(|e| match *e {
                    Edge::Pass { lane, .. } => Some(lane),
                    _ => None,
                })
                .collect(),
        );
        once(
            row.edges
                .iter()
                .filter_map(|e| match *e {
                    Edge::Out { to, .. } => Some(to),
                    _ => None,
                })
                .collect(),
        );
    }
    for (i, pair) in rows.windows(2).enumerate() {
        assert_eq!(
            bottom(&pair[0]),
            top(&pair[1]),
            "row {i} does not hand its lanes to row {}",
            i + 1
        );
    }
}

#[test]
fn a_linear_chain_stays_in_one_lane() {
    let page = [commit("a", &["b"]), commit("b", &["c"]), commit("c", &[])];
    let rows = lay_out(&page);

    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r.node == 0 && r.color == 0));
    assert_eq!(rows[0].edges.as_slice(), [out_at(0)]);
    assert_eq!(rows[1].edges.as_slice(), [in_at(0), out_at(0)]);
    assert_eq!(rows[2].edges.as_slice(), [in_at(0)]);
    assert_eq!(rows[2].parents, 0, "the last one is a root");
    assert_lanes_line_up(&rows);
}

#[test]
fn a_fork_and_a_merge_open_and_close_one_lane() {
    let page = [
        commit("m", &["a", "b"]),
        commit("a", &["base"]),
        commit("b", &["base"]),
        commit("base", &[]),
    ];
    let rows = lay_out(&page);

    assert_eq!(rows[0].node, 0);
    assert_eq!(rows[0].parents, 2);
    assert_eq!(
        rows[0].edges.as_slice(),
        [out_at(0), out_at(1)],
        "the merge leaves on its own lane and on a fresh one"
    );
    assert_eq!(rows[1].edges.as_slice(), [pass_at(1), in_at(0), out_at(0)]);
    assert_eq!(rows[2].node, 1, "the second parent kept the lane it opened");
    assert_eq!(rows[3].node, 0);
    assert_eq!(
        rows[3].edges.as_slice(),
        [in_at(0), in_at(1)],
        "both sides come back together at the base"
    );
    assert_eq!(rows[3].parents, 0);
    assert_lanes_line_up(&rows);
}

#[test]
fn an_octopus_merge_leaves_on_one_lane_per_parent() {
    let page = [
        commit("m", &["p1", "p2", "p3"]),
        commit("p1", &[]),
        commit("p2", &[]),
        commit("p3", &[]),
    ];
    let rows = lay_out(&page);

    assert_eq!(rows[0].parents, 3);
    assert_eq!(rows[0].edges.as_slice(), [out_at(0), out_at(1), out_at(2)]);
    assert_eq!(
        rows.iter().map(|r| r.node).collect::<Vec<_>>(),
        [0, 0, 1, 2]
    );
    assert_lanes_line_up(&rows);
}

#[test]
fn a_parent_outside_the_window_leaves_its_lane_open() {
    let page = [commit("a", &["b"])];
    let mut alloc = LaneAlloc::new();
    let mut rows = Vec::new();
    alloc.push(&page, &mut rows);

    assert_eq!(alloc.open_lanes(), [0], "b is below the page boundary");
    assert_eq!(alloc.width(), 1);
    assert!(!alloc.truncated());
}

#[test]
fn two_independent_roots_end_their_own_lanes() {
    let page = [
        commit("a", &["a1"]),
        commit("b", &["b1"]),
        commit("a1", &[]),
        commit("b1", &[]),
    ];
    let mut alloc = LaneAlloc::new();
    let mut rows = Vec::new();
    alloc.push(&page, &mut rows);

    assert_eq!(
        rows.iter().map(|r| r.node).collect::<Vec<_>>(),
        [0, 1, 0, 1],
        "the two histories never share a lane"
    );
    assert_eq!(rows[2].parents, 0);
    assert_eq!(rows[3].parents, 0);
    assert!(
        alloc.open_lanes().is_empty(),
        "both lanes died at their root"
    );
    assert_lanes_line_up(&rows);
}

#[test]
fn a_dead_lane_is_reused_without_two_live_lines_sharing_it() {
    let page = [
        commit("a", &["a1"]),
        commit("b", &["b1"]),
        commit("b1", &[]),
        commit("c", &["c1"]),
    ];
    let rows = lay_out(&page);

    assert_eq!(rows[1].node, 1);
    assert_eq!(rows[3].node, 1, "the freed lane was handed to the new tip");
    assert!(
        !bottom(&rows[2]).contains(&1),
        "the old line is gone from the band before the new one starts"
    );
    assert!(!top(&rows[3]).contains(&1), "the new tip has nothing above");
    assert_lanes_line_up(&rows);
}

#[test]
fn splitting_a_page_in_two_lays_out_identically() {
    let page = [
        commit("m", &["a", "b"]),
        commit("a", &["base"]),
        commit("b", &["base"]),
        commit("t", &["q"]),
        commit("base", &["p"]),
        commit("p", &["q"]),
        commit("q", &[]),
    ];

    let whole = lay_out(&page);

    let mut alloc = LaneAlloc::new();
    let mut paged = Vec::new();
    alloc.push(&page[..3], &mut paged);
    let first_page = paged.clone();
    alloc.push(&page[3..], &mut paged);

    assert_eq!(first_page, whole[..3], "the first page is a prefix");
    assert_eq!(paged, whole, "and loading more never re-flows it");
    assert_lanes_line_up(&whole);
}

#[test]
fn a_second_parent_avoids_the_lane_this_row_just_freed() {
    let page = [
        commit("t0", &["c"]),
        commit("t1", &["x"]),
        commit("t2", &["c"]),
        commit("c", &["p0", "p1"]),
    ];
    let rows = lay_out(&page);

    let merge = &rows[3];
    assert_eq!(merge.node, 0);
    assert_eq!(top(merge), [0, 1, 2], "two lines land here, one passes by");
    assert!(
        !merge.edges.contains(&out_at(2)),
        "lane 2 just ended here; leaving on it would draw a V: {merge:?}"
    );
    assert!(merge.edges.contains(&out_at(3)));
    assert_lanes_line_up(&rows);
}

/// The commonest merge topology of all: "merge main into topic after main
/// advanced". The merge's second parent (`c`) already has a lane reserved
/// by another child (`x`), so the merge's `Out` *joins* that lane instead
/// of opening a second one to the same commit — the row legally carries a
/// `Pass` and an `Out` on lane 0, one line below the cut, not two.
#[test]
fn a_second_parent_joins_a_line_another_child_opened() {
    let page = [
        commit("x", &["c"]),
        commit("m", &["a", "c"]),
        commit("a", &["c"]),
        commit("c", &[]),
    ];
    let rows = lay_out(&page);

    assert_eq!(rows[0].edges.as_slice(), [out_at(0)]);
    let merge = &rows[1];
    assert_eq!(merge.node, 1, "the merge tips a lane of its own");
    assert_eq!(
        merge.edges.as_slice(),
        [pass_at(0), out_at(1), out_at(0)],
        "first parent inherits the node's lane; the second joins lane 0"
    );
    assert_eq!(
        rows[3].edges.as_slice(),
        [in_at(0), in_at(1)],
        "both lines still converge on the shared parent"
    );
    assert_lanes_line_up(&rows);
}

#[test]
fn more_parents_than_lanes_truncates_instead_of_panicking() {
    let parents: Vec<String> = (0..40).map(|i| format!("p{i}")).collect();
    let refs: Vec<&str> = parents.iter().map(String::as_str).collect();
    let mut page = vec![commit("m", &refs)];
    page.extend(parents.iter().map(|p| commit(p, &[])));

    let mut alloc = LaneAlloc::new();
    let mut rows = Vec::new();
    alloc.push(&page, &mut rows);

    assert!(alloc.truncated());
    assert_eq!(rows[0].parents, 40);
    assert!(
        rows.iter().all(|r| r.node < MAX_LANES),
        "every row stayed inside the lane budget"
    );
    assert!(rows.iter().flat_map(bottom).all(|lane| lane < MAX_LANES));
}

const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const SHA_C: &str = "cccccccccccccccccccccccccccccccccccccccc";

fn record(fields: &[&str]) -> String {
    format!("\x1e{}", fields.join("\x1f"))
}

fn one(oid: &str, parents: &str, deco: &str, subject: &str, body: &str) -> String {
    record(&[
        oid,
        parents,
        "Ada",
        "ada@example.com",
        "2026-08-09T14:03:11+08:00",
        "Grace",
        "grace@example.com",
        "2026-08-09T15:00:00+08:00",
        deco,
        subject,
        body,
    ])
}

/// A parse that could not read everything must say so — `load_page` turns
/// `truncated` into `complete: false`, and a truncated graph that claimed
/// to be the end of history would freeze paging on it forever.
#[test]
fn a_stream_the_parse_cannot_finish_is_never_called_complete() {
    // One record past MAX_RECORD: dropped whole by the splitter.
    let huge_body = "x".repeat(super::super::MAX_RECORD + 1);
    let stream = [
        one(SHA_A, SHA_B, "", "kept", ""),
        one(SHA_B, "", "", "monster", &huge_body),
    ]
    .concat();
    let parsed = parse_log(stream.as_bytes());
    assert_eq!(parsed.commits.len(), 1, "the readable record survives");
    assert!(parsed.truncated);

    // Cumulative bytes past MAX_LOG_BYTES: the tail is clipped.
    let body = "y".repeat(512 * 1024);
    let stream: String = (0..40)
        .map(|i| one(&format!("{i:040}"), "", "", "big", &body))
        .collect();
    let parsed = parse_log(stream.as_bytes());
    assert!(parsed.commits.len() < 40);
    assert!(parsed.truncated);

    let parsed = parse_log(one(SHA_A, "", "", "small", "fine").as_bytes());
    assert!(!parsed.truncated, "an ordinary stream is read in full");
}

#[test]
fn a_multi_line_body_survives_the_record_split() {
    let stream = [
        one(
            SHA_A,
            SHA_B,
            "",
            "first",
            "line one\nline two\n\nline four\n",
        ),
        one(SHA_B, "", "", "second", ""),
    ]
    .join("\n");
    let commits = parse_log(stream.as_bytes()).commits;

    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].summary, "first");
    assert_eq!(commits[0].body, "line one\nline two\n\nline four");
    assert_eq!(commits[0].author.name, "Ada");
    assert_eq!(commits[0].committer.email, "grace@example.com");
    assert_eq!(commits[1].body, "");
    assert_eq!(commits[1].parents.len(), 0);
    assert!(!commits[0].is_merge());
}

#[test]
fn a_merge_records_both_parents() {
    let stream = one(SHA_A, &format!("{SHA_B} {SHA_C}"), "", "merge", "");
    let commits = parse_log(stream.as_bytes()).commits;

    assert_eq!(commits[0].parents.as_slice(), [SHA_B, SHA_C]);
    assert!(commits[0].is_merge());
    assert_eq!(commits[0].short(), "aaaaaaa");
}

#[test]
fn decorations_map_to_their_ref_kinds() {
    let deco = "HEAD -> refs/heads/main, refs/remotes/origin/main, tag: refs/tags/v1.0";
    let stream = one(SHA_A, "", deco, "subject", "");
    let refs = parse_log(stream.as_bytes()).commits.remove(0).refs;

    assert_eq!(refs.len(), 3);
    assert_eq!(refs[0].kind, RefKind::LocalBranch);
    assert_eq!(refs[0].short, "main");
    assert_eq!(refs[0].full, "refs/heads/main");
    assert!(refs[0].is_head);
    assert_eq!(refs[1].kind, RefKind::RemoteBranch);
    assert_eq!(refs[1].short, "origin/main");
    assert!(!refs[1].is_head);
    assert_eq!(refs[2].kind, RefKind::Tag);
    assert_eq!(refs[2].short, "v1.0");

    let detached = one(SHA_A, "", "HEAD, refs/tags/v2", "subject", "");
    let refs = parse_log(detached.as_bytes()).commits.remove(0).refs;
    assert_eq!(refs[0].kind, RefKind::Head);
    assert!(refs[0].is_head);
}

#[test]
fn a_unit_separator_inside_a_body_does_not_shift_fields() {
    let body = "before\x1fafter\x1fand\x1fmore";
    let stream = one(SHA_A, "", "", "subject", body);
    let commits = parse_log(stream.as_bytes()).commits;

    assert_eq!(
        commits[0].summary, "subject",
        "the subject is still field 10"
    );
    assert_eq!(commits[0].body, body, "the body swallowed the extra ones");
}

#[test]
fn a_record_that_does_not_start_with_a_sha_is_dropped() {
    let stream = [
        record(&["not a sha", "", "who", "", "", "", "", "", "", "junk", ""]),
        one(SHA_A, "", "", "real", ""),
        record(&[SHA_B, "only two fields"]),
    ]
    .join("\n");
    let commits = parse_log(stream.as_bytes()).commits;

    assert_eq!(commits.len(), 1, "{commits:?}");
    assert_eq!(commits[0].summary, "real");
}

#[test]
fn iso_8601_offsets_and_leap_days_parse() {
    assert_eq!(
        parse_iso8601("1970-01-01T00:00:00Z"),
        Some(OffsetTs {
            unix: 0,
            offset_minutes: 0
        })
    );
    assert_eq!(
        parse_iso8601("1970-01-01T00:00:00+00:00"),
        Some(OffsetTs {
            unix: 0,
            offset_minutes: 0
        })
    );
    // Same instant, written from two sides of the planet.
    assert_eq!(
        parse_iso8601("2026-08-09T14:03:11+08:00"),
        Some(OffsetTs {
            unix: 1_786_255_391,
            offset_minutes: 480
        })
    );
    assert_eq!(
        parse_iso8601("2026-08-09T02:03:11-04:00"),
        Some(OffsetTs {
            unix: 1_786_255_391,
            offset_minutes: -240
        })
    );
    assert_eq!(
        parse_iso8601("2026-08-09T11:33:11+05:30").map(|t| t.unix),
        Some(1_786_255_391)
    );
    assert_eq!(
        parse_iso8601("2024-02-29T00:00:00Z").map(|t| t.unix),
        Some(1_709_164_800),
        "2024 is a leap year"
    );
    assert_eq!(
        parse_iso8601("2000-02-29T00:00:00Z").map(|t| t.unix),
        Some(951_782_400),
        "and so is 2000, the four-hundred-year exception"
    );
    assert_eq!(parse_iso8601("2023-02-29T00:00:00Z"), None);
    assert_eq!(parse_iso8601("1900-02-29T00:00:00Z"), None);
    assert_eq!(parse_iso8601("2026-13-01T00:00:00Z"), None);
    assert_eq!(parse_iso8601("2026-08-09T24:00:00Z"), None);
    assert_eq!(parse_iso8601("nope"), None);
    assert_eq!(parse_iso8601("2026-08-09T14:03:11 08:00"), None);
}

#[test]
fn an_oversized_subject_and_body_are_cut_on_a_char_boundary() {
    let subject = "提".repeat(400);
    let body = "交".repeat(4000);
    let stream = one(SHA_A, "", "", &subject, &body);
    let commit = parse_log(stream.as_bytes()).commits.remove(0);

    assert_eq!(
        commit.summary.len(),
        MAX_SUBJECT_BYTES - MAX_SUBJECT_BYTES % 3,
        "cut back to the last whole character"
    );
    assert!(commit.summary.chars().all(|c| c == '提'));
    assert_eq!(commit.body.len(), MAX_BODY_BYTES - MAX_BODY_BYTES % 3);
    assert!(commit.body.chars().all(|c| c == '交'));
}

#[test]
fn an_annotated_tag_lands_on_the_commit_not_the_tag_object() {
    let tag_object = "1111111111111111111111111111111111111111";
    let lines = [
        format!(
            "{SHA_A}\x1frefs/heads/main\x1fmain\x1frefs/remotes/origin/main\x1f*\x1fcommit\x1f"
        ),
        format!("{tag_object}\x1frefs/tags/v9\x1fv9\x1f\x1f \x1ftag\x1f{SHA_B}"),
        format!("{SHA_C}\x1frefs/remotes/origin/dev\x1forigin/dev\x1f\x1f \x1fcommit\x1f"),
    ];
    let by_oid = parse_refs(lines.join("\n").as_bytes());

    assert_eq!(by_oid[SHA_A][0].kind, RefKind::LocalBranch);
    assert!(by_oid[SHA_A][0].is_head, "the `*` column marks HEAD");
    assert!(
        !by_oid.contains_key(tag_object),
        "the tag object itself is never a graph row"
    );
    assert_eq!(by_oid[SHA_B][0].kind, RefKind::Tag);
    assert_eq!(by_oid[SHA_B][0].short, "v9");
    assert_eq!(by_oid[SHA_C][0].short, "origin/dev");
}

#[test]
fn a_branch_keeps_the_upstream_it_tracks() {
    let lines = [
        format!(
            "{SHA_A}\x1frefs/heads/main\x1fmain\x1frefs/remotes/origin/main\x1f*\x1fcommit\x1f"
        ),
        // A branch nobody has published tracks nothing, and an empty
        // `%(upstream)` has to stay `None` rather than become `Some("")`.
        format!("{SHA_B}\x1frefs/heads/local-only\x1flocal-only\x1f\x1f \x1fcommit\x1f"),
        format!("{SHA_C}\x1frefs/tags/v9\x1fv9\x1f\x1f \x1fcommit\x1f"),
    ];
    let by_oid = parse_refs(lines.join("\n").as_bytes());

    assert_eq!(
        by_oid[SHA_A][0].upstream.as_deref(),
        Some("refs/remotes/origin/main")
    );
    assert_eq!(by_oid[SHA_B][0].upstream, None);
    assert_eq!(by_oid[SHA_C][0].upstream, None);
    // `%D` cannot carry an upstream at all, so a decoration parsed out of
    // a log record must not claim one.
    let logged = parse_log(one(SHA_A, "", "refs/heads/main", "s", "").as_bytes()).commits;
    assert_eq!(logged[0].refs[0].upstream, None);
}

#[test]
fn branch_names_come_back_one_per_line() {
    let out = b"main\nfeature/a\n\n  spaced  \n";
    assert_eq!(
        parse_branch_names(out),
        ["main", "feature/a", "spaced"],
        "blank lines are not branches, and git pads nothing"
    );
    assert!(parse_branch_names(b"").is_empty());
    let many: String = (0..MAX_REFS + 50)
        .map(|i| format!("b{i}\n"))
        .collect::<Vec<_>>()
        .concat();
    assert_eq!(parse_branch_names(many.as_bytes()).len(), MAX_REFS);
}

/// Every shape `-z` can produce, from the streams git actually emits —
/// each of these was captured from git 2.50.1 rather than guessed.
#[test]
fn a_commits_file_list_joins_the_two_z_streams() {
    // A rename, a path with a space, a path outside ASCII, and a binary.
    let numstat = b"1\t0\tbin.dat\x000\t0\t\x00a.txt\x00renamed.txt\x001\t0\twith space.txt\x002\t3\t\xe4\xb8\xad\xe6\x96\x87\xe5\x90\x8d.txt\x00";
    let name_status = b"A\x00bin.dat\x00R100\x00a.txt\x00renamed.txt\x00A\x00with space.txt\x00M\x00\xe4\xb8\xad\xe6\x96\x87\xe5\x90\x8d.txt\x00";
    let files = join_commit_files(numstat, name_status);

    assert_eq!(files.len(), 4, "{files:?}");
    assert_eq!(files[0].path, "bin.dat");
    assert_eq!(files[0].status, FileStatus::Added);
    assert_eq!((files[0].added, files[0].removed), (Some(1), Some(0)));

    // The rename: `--numstat` spells it as an empty third field followed
    // by two records of its own, and the counts belong to the new name.
    assert_eq!(files[1].path, "renamed.txt");
    assert_eq!(files[1].orig_path.as_deref(), Some("a.txt"));
    assert_eq!(files[1].status, FileStatus::Renamed);
    assert_eq!((files[1].added, files[1].removed), (Some(0), Some(0)));

    assert_eq!(
        files[2].path, "with space.txt",
        "a space is not a separator"
    );
    assert_eq!(files[3].path, "中文名.txt");
    assert_eq!((files[3].added, files[3].removed), (Some(2), Some(3)));
    assert!(files.iter().all(|f| !f.binary));
}

#[test]
fn a_binary_file_reports_no_counts_rather_than_zero() {
    let files = join_commit_files(b"-\t-\tbin2.dat\x00", b"A\x00bin2.dat\x00");
    assert_eq!(files.len(), 1);
    assert!(files[0].binary);
    assert_eq!(
        (files[0].added, files[0].removed),
        (None, None),
        "`0 0` is a real answer and `-\t-` is not, so they must not read alike"
    );
    // A pure rename really does change nothing, and says so.
    let renamed = join_commit_files(b"0\t0\t\x00a\x00b\x00", b"R100\x00a\x00b\x00");
    assert!(!renamed[0].binary);
    assert_eq!((renamed[0].added, renamed[0].removed), (Some(0), Some(0)));
}

#[test]
fn one_stream_going_missing_degrades_instead_of_emptying_the_list() {
    // No counts: every row still knows what happened to it.
    let no_numstat = join_commit_files(b"", b"M\x00a.txt\x00D\x00b.txt\x00");
    assert_eq!(no_numstat.len(), 2);
    assert_eq!(no_numstat[1].status, FileStatus::Deleted);
    assert!(no_numstat.iter().all(|f| f.added.is_none()));

    // No letters: the paths are the more important half, so they are kept
    // and given the one status that claims the least.
    let no_names = join_commit_files(b"1\t2\tb.txt\x003\t4\ta.txt\x00", b"");
    assert_eq!(
        no_names.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        ["a.txt", "b.txt"],
        "with no order to inherit the leftovers are sorted, not shuffled"
    );
    assert!(no_names.iter().all(|f| f.status == FileStatus::Modified));
    assert_eq!((no_names[0].added, no_names[0].removed), (Some(3), Some(4)));

    assert!(join_commit_files(b"", b"").is_empty());
}

#[test]
fn a_truncated_or_unknown_record_is_dropped_rather_than_shifting_the_parse() {
    // A status letter with no path behind it ends the read; anything
    // already parsed still stands.
    let cut = join_commit_files(b"", b"M\x00a.txt\x00D\x00");
    assert_eq!(cut.len(), 1);
    assert_eq!(cut[0].path, "a.txt");

    // `X` is git's own "something went wrong". Its path is consumed so the
    // records after it stay aligned.
    let unknown = join_commit_files(b"", b"X\x00weird\x00A\x00good.txt\x00");
    assert_eq!(unknown.len(), 1);
    assert_eq!(unknown[0].path, "good.txt");

    // A rename cut off after its old name leaves nothing to attach to.
    assert!(join_commit_files(b"0\t0\t\x00a\x00", b"R100\x00a\x00").is_empty());
}

#[test]
fn a_file_list_is_capped_without_losing_its_first_rows() {
    let mut numstat = Vec::new();
    let mut name_status = Vec::new();
    for i in 0..MAX_COMMIT_FILES + 20 {
        numstat.extend_from_slice(format!("1\t0\tf{i:05}.rs\0").as_bytes());
        name_status.extend_from_slice(format!("A\0f{i:05}.rs\0").as_bytes());
    }
    let files = join_commit_files(&numstat, &name_status);
    assert_eq!(files.len(), MAX_COMMIT_FILES);
    assert_eq!(files[0].path, "f00000.rs");
}

#[test]
fn a_rev_that_could_be_read_as_an_option_never_reaches_git() {
    assert!(is_rev("HEAD"));
    assert!(is_rev(SHA_A));
    assert!(is_rev("v1.0^{commit}"));
    assert!(!is_rev(""));
    assert!(!is_rev("--upload-pack=touch /tmp/pwned"));
    assert!(!is_rev("-n"));
    assert!(!is_rev("HEAD\nrm -rf"));
}

// ----- against a real repository -------------------------------------
//
// Both of these build the history they assert on rather than reading the
// anterm checkout they were compiled in. CI clones shallow (one commit) and
// checks a pull request out as a detached HEAD with no local branch, so
// "how deep is the history" and "is there a branch" are facts about the
// runner, not about this code — and a source tarball has no repository at
// all. Owning the fixture is what lets the counts below be exact.

struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> Option<Scratch> {
    // The pid keeps two concurrent `cargo test` runs off each other's
    // fixture, since the directory is wiped on the way in.
    let dir = std::env::temp_dir().join(format!("anterm-scm-log-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).ok()?;
    Some(Scratch(dir))
}

/// Runs git with the identity, signing and line-ending settings pinned: a
/// CI runner has no `user.name` at all and would refuse to commit, a
/// developer may have signing turned on globally, and Git for Windows has
/// `core.autocrlf=true` in its system config.
fn run(host: &dyn Host, cwd: &Path, args: &[&str]) -> bool {
    let mut full = PINS.to_vec();
    full.extend_from_slice(args);
    host.git(cwd, &full).map(|o| o.success()).unwrap_or(false)
}

/// `git init` on a branch named here rather than left to whatever
/// `init.defaultBranch` says, with the same pins written into the
/// repository so the code under test reads them too. `false` means there is
/// no git on this machine, which is not a failure.
fn init_repo(host: &dyn Host, repo: &Path) -> bool {
    if !run(host, repo, &["init", "--quiet"]) {
        return false;
    }
    assert!(pin_repo_config(repo));
    // Not `init -b`: that is git 2.28+, and the branch name is asserted on.
    assert!(run(
        host,
        repo,
        &["symbolic-ref", "HEAD", "refs/heads/main"]
    ));
    true
}

fn commit_file(host: &dyn Host, repo: &Path, path: &str, body: &str, message: &str) {
    std::fs::write(repo.join(path), body).unwrap();
    assert!(run(host, repo, &["add", "--", path]));
    assert!(run(host, repo, &["commit", "--quiet", "-m", message]));
}

#[test]
fn a_real_repository_answers_for_one_commit_and_its_files() {
    let host = crate::host::local::LocalHost::new();
    let Some(scratch) = scratch("one-commit") else {
        return;
    };
    let repo = &scratch.0;
    if !init_repo(&*host, repo) {
        return; // no git on this machine
    }

    std::fs::write(repo.join("kept.txt"), "one\n").unwrap();
    std::fs::write(repo.join("moved.txt"), "a\nb\nc\nd\ne\nf\ng\nh\n").unwrap();
    std::fs::write(repo.join("gone.txt"), "bye\n").unwrap();
    assert!(run(&*host, repo, &["add", "--", "."]));
    assert!(run(&*host, repo, &["commit", "--quiet", "-m", "base"]));

    // One commit with four different things in it, because the join of
    // `--numstat` and `--name-status` is only exercised by a commit that
    // touches more than one path, and the rename is the record whose two
    // halves are shaped differently in the two streams.
    std::fs::write(repo.join("kept.txt"), "one\ntwo\n").unwrap();
    std::fs::write(repo.join("added.txt"), "new\n").unwrap();
    assert!(run(&*host, repo, &["mv", "moved.txt", "renamed.txt"]));
    assert!(run(&*host, repo, &["rm", "--quiet", "--", "gone.txt"]));
    assert!(run(&*host, repo, &["add", "--", "."]));
    assert!(run(
        &*host,
        repo,
        &["commit", "--quiet", "-m", "feat: four ways at once"]
    ));
    // A second branch, so `local_branches` has to return more than the one
    // HEAD happens to be on.
    assert!(run(&*host, repo, &["branch", "side"]));

    let page = load_page(&*host, repo, &GraphScope::Head, 2)
        .expect("a repository was just created here");
    let head = page.commits.first().expect("two commits were just made");

    let shown = load_commit(&*host, repo, &head.oid).expect("HEAD is a commit");
    assert_eq!(shown.oid, head.oid);
    assert_eq!(shown.summary, head.summary, "the two formats are the same");
    assert_eq!(shown.parents.as_slice(), head.parents.as_slice());
    assert_eq!(shown.author.at, head.author.at);
    assert_eq!(load_commit(&*host, repo, "-n"), None);

    let files = commit_files(&*host, repo, &head.oid).expect("HEAD touched something");
    assert!(
        files.iter().all(|f| !f.path.is_empty()),
        "an empty path means the join lost a record: {files:?}"
    );
    let mut got: Vec<(&str, FileStatus)> =
        files.iter().map(|f| (f.path.as_str(), f.status)).collect();
    got.sort_by_key(|(path, _)| *path);
    assert_eq!(
        got,
        [
            ("added.txt", FileStatus::Added),
            ("gone.txt", FileStatus::Deleted),
            ("kept.txt", FileStatus::Modified),
            ("renamed.txt", FileStatus::Renamed),
        ],
        "every path the commit touched, with the letter git gave it: {files:?}"
    );

    fn file<'a>(files: &'a [CommitFile], path: &str) -> &'a CommitFile {
        files.iter().find(|f| f.path == path).unwrap()
    }
    // The whole reason the two commands are run separately: the counts come
    // from `--numstat` and the letters from `--name-status`, so a row that
    // has both is a row the join put back together.
    assert_eq!(
        (
            file(&files, "kept.txt").added,
            file(&files, "kept.txt").removed
        ),
        (Some(1), Some(0))
    );
    assert_eq!(
        (
            file(&files, "gone.txt").added,
            file(&files, "gone.txt").removed
        ),
        (Some(0), Some(1))
    );
    let renamed = file(&files, "renamed.txt");
    assert_eq!(renamed.orig_path.as_deref(), Some("moved.txt"));
    assert_eq!((renamed.added, renamed.removed), (Some(0), Some(0)));
    assert!(files.iter().all(|f| !f.binary));

    let branches = local_branches(&*host, repo);
    assert_eq!(branches, ["main", "side"], "both of them, in refname order");
    assert!(branches.iter().all(|b| !b.starts_with("refs/heads/")));
}

/// The lane layout against a history with a shape, not a straight line:
///
/// ```text
///   top          main
///   merge
///   |    \
///   main2 side2
///   main1 side1
///   |    /
///   root
/// ```
///
/// Seven commits, which is also what makes the paging assertion at the end
/// honest — a page of five cannot be the whole history.
#[test]
fn a_real_repository_lays_out_one_row_per_commit() {
    let host = crate::host::local::LocalHost::new();
    let Some(scratch) = scratch("layout") else {
        return;
    };
    let repo = &scratch.0;
    if !init_repo(&*host, repo) {
        return; // no git on this machine
    }

    commit_file(&*host, repo, "root.txt", "0\n", "root");
    assert!(run(&*host, repo, &["checkout", "--quiet", "-b", "side"]));
    commit_file(&*host, repo, "side.txt", "1\n", "side one");
    commit_file(&*host, repo, "side.txt", "2\n", "side two");
    assert!(run(&*host, repo, &["checkout", "--quiet", "main"]));
    commit_file(&*host, repo, "main.txt", "1\n", "main one");
    commit_file(&*host, repo, "main.txt", "2\n", "main two");
    // The two branches touch different files, so this merges clean.
    assert!(run(
        &*host,
        repo,
        &[
            "merge",
            "--quiet",
            "--no-ff",
            "--no-edit",
            "-m",
            "merge side",
            "side"
        ]
    ));
    commit_file(&*host, repo, "main.txt", "3\n", "after the merge");

    let page =
        load_page(&*host, repo, &GraphScope::Head, 30).expect("a repository was just created");

    assert_eq!(page.rows.len(), page.commits.len());
    assert_eq!(
        page.commits.len(),
        7,
        "the root, two on the side branch, two on main, the merge, and the one on top"
    );
    assert!(page.complete, "thirty asked for, seven exist");
    assert!(
        page.max_lanes >= 2,
        "a branch that forks and merges back needs a second lane: {page:?}"
    );
    assert!(!page.truncated_lanes);
    assert!(page.rows.iter().all(|r| r.node < page.max_lanes));
    assert!(page.commits.iter().all(|c| is_hex_oid(&c.oid)));
    assert!(
        page.commits
            .iter()
            .all(|c| c.author.at.unix > 1_600_000_000),
        "every commit got a real timestamp"
    );
    assert_lanes_line_up(&page.rows);

    // Row *i* is commit *i*, which is only worth checking where the two
    // could come apart: the merge is the one commit with two parents, and
    // its row is the one that sends a line to each of them.
    let merges: Vec<usize> = page
        .commits
        .iter()
        .enumerate()
        .filter(|(_, c)| c.parents.len() == 2)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(merges.len(), 1, "one merge in the fixture");
    let row = &page.rows[merges[0]];
    assert_eq!(row.parents, 2, "the row agrees with the commit beside it");
    assert_eq!(
        row.edges
            .iter()
            .filter(|e| matches!(e, Edge::Out { .. }))
            .count(),
        2,
        "the merge row leaves for both parents: {row:?}"
    );

    let page = load_page(&*host, repo, &GraphScope::HeadAndUpstream, 5).unwrap();
    assert_eq!(page.commits.len(), 5);
    assert!(!page.complete, "five of the seven is not the whole history");
}
