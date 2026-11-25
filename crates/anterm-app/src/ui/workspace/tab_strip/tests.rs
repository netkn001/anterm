use super::*;
use crate::core::shells::DetectedShell;
use gpui::Edges;
use gpui::TestAppContext;
use std::path::Path;
use unicode_segmentation::UnicodeSegmentation;

/// #1169: under Linux client-side decorations the viewport measures the
/// whole surface, shadow included, while the title bar row sits inside the
/// frame padding. A strip sized off the raw viewport overran the frame by
/// both paddings and pushed the close button out past the right edge.
#[test]
fn the_strip_and_window_buttons_fit_inside_the_csd_frame() {
    let (lead, controls) = (12., 102.);
    let pad = Edges::all(px(12.));
    let strip = strip_width(px(1200.), pad, lead + controls, 140.);
    assert!(
        px(lead) + strip + px(controls) <= px(1200. - 12. - 12.),
        "strip {strip:?} leaves the window buttons outside the frame"
    );
    // No frame padding (server-side decorations, macOS, Windows, or a
    // maximized/tiled CSD window): the strip keeps the width it always had.
    let bare = strip_width(px(1200.), Edges::all(px(0.)), lead + controls, 140.);
    assert_eq!(bare, px(1200. - lead - controls));
}

/// Most of these tests are about where a title is *cut*, not about what
/// `~` means: the paths they pass either already start with `~` or are
/// nowhere near anybody's home. Naming no home keeps the assertions off
/// the process environment — and is what a title of unknown provenance
/// gets in the app too (#580).
fn short_title(raw: &str) -> String {
    super::short_title(raw, None)
}

fn host(name: &str, user: &str, addr: &str) -> crate::core::ssh_profile::SshProfile {
    let mut p = crate::core::ssh_profile::SshProfile::new(name);
    p.user = user.to_string();
    p.host = addr.to_string();
    p
}

#[test]
fn the_new_tab_menu_stops_naming_hosts_before_it_becomes_a_list() {
    // Frecency has already put the useful ones first by the time this
    // runs, so the cut can only ever drop the tail.
    let many: Vec<_> = (0..MENU_HOSTS + 4)
        .map(|i| host(&format!("box-{i}"), "dev", &format!("10.0.0.{i}")))
        .collect();
    let rows = menu_hosts(many);
    assert_eq!(rows.len(), MENU_HOSTS);
    assert_eq!(rows[0].1, "box-0", "the order handed in is the order shown");
    assert_eq!(rows[0].2, "dev@10.0.0.0");
}

#[test]
fn a_host_saved_without_a_name_still_says_where_it_goes() {
    // Settings lets a host be saved on its address alone. A row that led
    // with an empty name would be a blank line you could click.
    let rows = menu_hosts(vec![host("", "root", "build.lan")]);
    assert_eq!(rows[0].1, "root@build.lan");
    // And it says it once: the endpoint has already led the row, so
    // repeating it in the note column would be a row talking to itself.
    assert_eq!(rows[0].2, "");
}

#[test]
fn a_host_named_after_its_own_address_does_not_say_it_twice() {
    // Quick Connect saves a host under the target that was typed, so this
    // is the ordinary shape of a host nobody has renamed.
    let rows = menu_hosts(vec![host("deploy@10.0.0.5", "deploy", "10.0.0.5")]);
    assert_eq!(rows[0].1, "deploy@10.0.0.5");
    assert_eq!(rows[0].2, "");
}

#[test]
fn only_alt_turns_a_new_tab_row_into_a_split() {
    use crate::ui::workspace::app::SpawnWhere;
    use gpui::Modifiers;
    assert_eq!(
        SpawnWhere::from_modifiers(Modifiers::none()),
        SpawnWhere::NewTab
    );
    assert_eq!(
        SpawnWhere::from_modifiers(Modifiers::alt()),
        SpawnWhere::Split
    );
    // One modifier means one thing. Every other one a hand might be
    // resting on leaves the row doing what it says it does.
    assert_eq!(
        SpawnWhere::from_modifiers(crate::platform::secondary_key()),
        SpawnWhere::NewTab
    );
    assert_eq!(
        SpawnWhere::from_modifiers(Modifiers::shift()),
        SpawnWhere::NewTab
    );
}

#[test]
fn every_visible_agent_state_has_words_for_it() {
    use crate::core::cli_agent::AgentStatus;
    anterm_i18n::set_locale("en");
    // Idle draws no dot, so it has nothing to name.
    assert_eq!(agent_status_label(None), None);
    assert_eq!(agent_status_label(Some(AgentStatus::Idle)), None);
    // Every state that does draw a dot can be read out loud.
    for status in [
        AgentStatus::Working,
        AgentStatus::Waiting,
        AgentStatus::Done,
    ] {
        assert!(status.dot_rgb().is_some());
        assert!(
            agent_status_label(Some(status)).is_some_and(|s| !s.is_empty()),
            "{status:?} paints a dot with no words behind it"
        );
    }
    // Waiting is the state worth acting on; it must not read as Done.
    assert_ne!(
        agent_status_label(Some(AgentStatus::Waiting)),
        agent_status_label(Some(AgentStatus::Done))
    );
}

#[test]
fn a_brand_disc_that_matches_the_window_gets_an_edge() {
    use crate::ui::chrome::presets::needs_edge;
    let dark: gpui::Hsla = gpui::rgb(0x111111).into();
    let light: gpui::Hsla = gpui::rgb(0xffffff).into();
    let codex = crate::core::cli_agent::CLIAgent::Codex.accent_rgb();
    let claude = crate::core::cli_agent::CLIAgent::Claude.accent_rgb();

    assert_eq!(codex, 0x000000, "Codex's disc is pure black");
    assert!(
        needs_edge(codex, dark),
        "a black disc on a dark window is not a disc"
    );
    assert!(!needs_edge(codex, light));
    assert!(!needs_edge(claude, dark) && !needs_edge(claude, light));
}

#[test]
fn short_title_strips_user_host_and_shows_shallow_path_in_full() {
    assert_eq!(short_title("user@host:~/projects/app"), "~/projects/app");
    // Debian's stock bash title, which spaces the path off the colon.
    assert_eq!(short_title("user@host: ~/projects/app"), "~/projects/app");
    assert_eq!(short_title("/usr/local/bin"), "/usr/local/bin");
    assert_eq!(short_title("plain"), "plain");
}

/// A title shortens under the home of the machine it came from, and
/// under no other (#580).
#[test]
fn short_title_shortens_under_the_home_it_was_given() {
    let server = Path::new("/home/deploy");
    assert_eq!(
        super::short_title("/home/deploy/app", Some(server)),
        "~/app"
    );
    // This machine's home is not a stand-in for the server's: the same
    // path stays whole when the home naming it is somewhere else.
    assert_eq!(
        super::short_title("/home/deploy/app", Some(Path::new("/Users/thomas"))),
        "/home/deploy/app"
    );
    // And a pane nothing here can place — no link to its host, or a
    // shell that has ssh'd on — shortens against nothing.
    assert_eq!(
        super::short_title("/home/deploy/app", None),
        "/home/deploy/app"
    );
}

/// The name a freshly dialled SSH pane wears until the remote shell says
/// otherwise. Cutting at the colon left the tab reading "2222" (#438).
#[test]
fn short_title_keeps_an_ssh_address_whole() {
    assert_eq!(short_title("deploy@10.0.0.5:2222"), "deploy@10.0.0.5:2222");
    assert_eq!(short_title("root@prod"), "root@prod");
    assert_eq!(short_title("prod-web"), "prod-web");
    // Only a port stops the cut: a drive letter is still a path, and this
    // is the title anterm's own pwsh integration writes on Windows.
    assert_eq!(short_title(r"ann@BOX:C:/Users/app"), r"C:/Users/app");
}

#[test]
fn short_title_truncates_deep_paths_to_trailing_segments() {
    assert_eq!(short_title("user@host:~/repo/025/anterm"), "…/repo/025/anterm");
    assert_eq!(short_title("/usr/local/share/man"), "…/local/share/man");
    assert_eq!(short_title("a/b/c/d"), "…/b/c/d");
}

#[test]
fn short_title_keeps_home_tilde_and_normalizes_trailing_slash() {
    assert_eq!(short_title("user@host:~"), "~");
    assert_eq!(short_title("~"), "~");
    assert_eq!(short_title("a/b/c/"), "a/b/c");
}

#[test]
fn short_title_blank_input_is_empty_and_long_names_are_clamped() {
    assert_eq!(short_title("   "), "");
    let long = "a".repeat(50);
    let out = short_title(&long);
    assert_eq!(out.chars().count(), 41);
    assert!(out.ends_with('…'));
}

/// `TestAppContext` shapes through gpui's `NoopTextSystem`, where every
/// glyph is exactly one em — weight-agnostic and, more to the point,
/// CJK-agnostic. That keeps these tests identical on all three CI targets,
/// but it also means they cannot speak to the proportional and mixed-script
/// widths the elision exists for: what they pin is the contract — which
/// parts of a label must survive, and that the result fits its budget.
fn elide_setup(cx: &mut TestAppContext) -> (gpui::WindowTextSystem, gpui::Font, f32) {
    let size = 14.;
    (
        gpui::WindowTextSystem::new(cx.text_system().clone()),
        gpui::Font::default(),
        size,
    )
}

#[gpui::test]
fn elide_path_fits_shallow_paths_untouched(cx: &mut TestAppContext) {
    let (ts, font, size) = elide_setup(cx);
    let path = "~/anterm";
    let max = measure_text(&ts, &font, size, path) + 1.;
    assert_eq!(elide_path_keep_tail(&ts, &font, size, path, max), "~/anterm");
}

#[gpui::test]
fn elide_path_shows_the_whole_deep_path_when_the_budget_allows(cx: &mut TestAppContext) {
    let (ts, font, size) = elide_setup(cx);
    // A wide sidebar must not elide a deep path: only the width may
    // decide, never a fixed segment cap.
    let path = "E:/work/toolbox/crates/anterm-core/src/client";
    let max = measure_text(&ts, &font, size, path) + 1.;
    assert_eq!(elide_path_keep_tail(&ts, &font, size, path, max), path);
}

#[gpui::test]
fn elide_path_keeps_drive_tail_and_budget(cx: &mut TestAppContext) {
    let (ts, font, size) = elide_setup(cx);
    let path = "E:/work/toolbox/src/ui/tab_sidebar.rs";
    let max = 200.;
    assert!(
        measure_text(&ts, &font, size, path) > max,
        "the fixture has to be wider than the budget to exercise elision"
    );
    let out = elide_path_keep_tail(&ts, &font, size, path, max);
    assert!(out.starts_with("E:/…/"), "drive letter survives: {out}");
    assert!(
        out.ends_with("tab_sidebar.rs"),
        "the file name always survives: {out}"
    );
    assert!(
        measure_text(&ts, &font, size, &out) <= max,
        "the elided label fits the budget"
    );
}

#[gpui::test]
fn elide_path_keeps_tilde_and_leading_slash(cx: &mut TestAppContext) {
    let (ts, font, size) = elide_setup(cx);
    let home = "~/projects/toolbox/src/ui/tab_sidebar.rs";
    assert!(
        measure_text(&ts, &font, size, home) > 200.,
        "the fixture has to be wider than the budget to exercise elision"
    );
    let out = elide_path_keep_tail(&ts, &font, size, home, 200.);
    assert!(out.starts_with("~/…/"), "tilde root survives: {out}");
    assert!(out.ends_with("tab_sidebar.rs"));

    let abs = "/usr/local/share/man/man1/git.1";
    assert!(
        measure_text(&ts, &font, size, abs) > 120.,
        "the fixture has to be wider than the budget to exercise elision"
    );
    let out = elide_path_keep_tail(&ts, &font, size, abs, 120.);
    assert!(out.starts_with("/…/"), "absolute root survives: {out}");
    assert!(out.ends_with("git.1"));
}

#[gpui::test]
fn elide_path_tears_only_the_last_segment_as_a_last_resort(cx: &mut TestAppContext) {
    let (ts, font, size) = elide_setup(cx);
    let path = "E:/supercalifragilisticexpialidocious";
    let max = 60.;
    assert!(measure_text(&ts, &font, size, path) > max);
    let out = elide_path_keep_tail(&ts, &font, size, path, max);
    assert!(out.starts_with('…'), "a torn segment reads as torn: {out}");
    assert!(
        out.chars().nth(1) != Some('/'),
        "no slash after a torn segment: {out}"
    );
    assert!(out.ends_with('s'), "the word's tail survives: {out}");
    assert!(measure_text(&ts, &font, size, &out) <= max);
}

#[gpui::test]
fn elide_edges_keeps_both_ends_of_a_branch(cx: &mut TestAppContext) {
    let (ts, font, size) = elide_setup(cx);
    let branch = "window-transparency-backdrop";
    let max = 140.;
    assert!(measure_text(&ts, &font, size, branch) > max);
    let out = elide_keep_edges(&ts, &font, size, branch, max);
    assert!(out.starts_with("window-"), "head survives: {out}");
    assert!(out.ends_with("backdrop"), "tail survives: {out}");
    assert!(out.contains('…'));
    assert!(measure_text(&ts, &font, size, &out) <= max);
    assert!(out.chars().count() < branch.chars().count());
}

#[gpui::test]
fn elide_end_keeps_the_head_of_a_branch(cx: &mut TestAppContext) {
    let (ts, font, size) = elide_setup(cx);
    let branch = "fix/rpc-proxy-and-error-classification";
    let max = 140.;
    let out = elide_end_clusters(&ts, &font, size, branch, max);
    assert!(out.starts_with("fix/rpc-"), "head survives: {out}");
    assert!(out.ends_with('…') && out.matches('…').count() == 1, "{out}");
    assert!(measure_text(&ts, &font, size, &out) <= max);
    let fits = measure_text(&ts, &font, size, branch);
    assert_eq!(elide_end_clusters(&ts, &font, size, branch, fits), branch);
}

#[gpui::test]
fn elide_edges_leaves_short_branches_alone(cx: &mut TestAppContext) {
    let (ts, font, size) = elide_setup(cx);
    let branch = "main";
    let max = measure_text(&ts, &font, size, branch) + 1.;
    assert_eq!(elide_keep_edges(&ts, &font, size, branch, max), "main");
}

#[gpui::test]
fn elide_edges_falls_back_to_a_tail_sliver_when_the_head_cannot_fit(cx: &mut TestAppContext) {
    let (ts, font, size) = elide_setup(cx);
    let branch = "window-transparency-backdrop";
    let out = elide_keep_edges(&ts, &font, size, branch, 30.);
    assert!(out.starts_with('…'));
    assert!(measure_text(&ts, &font, size, &out) <= 30.);
}

#[gpui::test]
fn elide_path_cuts_windows_backslash_paths_on_segments(cx: &mut TestAppContext) {
    let (ts, font, size) = elide_setup(cx);
    let path = r"C:\Users\dev\AppData\Local\Temp\verify-build";
    let max = 200.;
    assert!(measure_text(&ts, &font, size, path) > max);
    let out = elide_path_keep_tail(&ts, &font, size, path, max);
    assert!(out.starts_with(r"C:\…\"), "drive letter survives: {out}");
    assert!(
        out.ends_with("verify-build"),
        "the leaf segment survives: {out}"
    );
    assert!(measure_text(&ts, &font, size, &out) <= max);
}

/// One tab must not spell its location two ways depending on how wide the
/// sidebar happens to be.
#[gpui::test]
fn elide_path_keeps_the_separator_the_path_arrived_with(cx: &mut TestAppContext) {
    let (ts, font, size) = elide_setup(cx);
    let windows = r"C:\Users\dev\projects\toolbox\src\ui\app.rs";
    let wide = measure_text(&ts, &font, size, windows) + 1.;
    assert_eq!(
        elide_path_keep_tail(&ts, &font, size, windows, wide),
        windows,
        "a path that fits is left exactly as it arrived"
    );
    let out = elide_path_keep_tail(&ts, &font, size, windows, 120.);
    assert!(!out.contains('/'), "no forward slash creeps in: {out}");

    let unix = "/home/dev/projects/toolbox/src/ui/app.rs";
    let out = elide_path_keep_tail(&ts, &font, size, unix, 120.);
    assert!(!out.contains('\\'), "no backslash creeps in: {out}");
}

/// A branch with no `-`, `_`, `/` or `.` in reach used to lose its head
/// entirely, which is the one thing this function promises not to do.
#[gpui::test]
fn elide_edges_keeps_a_head_on_a_separatorless_token(cx: &mut TestAppContext) {
    let (ts, font, size) = elide_setup(cx);
    let branch = "verylongbranchnamewithoutseps";
    for max in [60., 80., 100., 120.] {
        let out = elide_keep_edges(&ts, &font, size, branch, max);
        assert!(
            out.starts_with('v'),
            "head survives at {max}px: {out}",
            max = max
        );
        assert!(out.ends_with('s'), "tail survives at {max}px: {out}");
        assert!(measure_text(&ts, &font, size, &out) <= max);
    }
}

/// A head that fits but leaves nothing behind the ellipsis says less than
/// a shorter head that keeps the identifying tail.
#[gpui::test]
fn elide_edges_gives_up_head_room_to_keep_a_tail(cx: &mut TestAppContext) {
    let (ts, font, size) = elide_setup(cx);
    let branch = "feature/some-really-long-thing";
    let max = 80.;
    assert!(measure_text(&ts, &font, size, branch) > max);
    let out = elide_keep_edges(&ts, &font, size, branch, max);
    assert!(
        !out.ends_with('…'),
        "the tail is never traded away for a longer head: {out}"
    );
    assert!(out.ends_with('g'), "the identifying tail survives: {out}");
    assert!(measure_text(&ts, &font, size, &out) <= max);
}

/// The sidebar title is not always a path. `elide_label` has to notice,
/// because the path rule drops the head — and for a command line or a name
/// the user typed, the head is the part that names it.
#[gpui::test]
fn elide_label_keeps_the_head_of_a_non_path(cx: &mut TestAppContext) {
    let (ts, font, size) = elide_setup(cx);
    for text in ["Backend server logs", "npm run dev -- --watch"] {
        let max = 90.;
        assert!(measure_text(&ts, &font, size, text) > max);
        let out = elide_label(&ts, &font, size, text, max);
        let first = text.chars().next().unwrap();
        assert!(
            out.starts_with(first),
            "a non-path keeps its head: {out} (from {text})"
        );
        assert!(measure_text(&ts, &font, size, &out) <= max);
    }
}

/// …while a path still gets the tail-first treatment through the same
/// entry point.
#[gpui::test]
fn elide_label_still_keeps_the_tail_of_a_path(cx: &mut TestAppContext) {
    let (ts, font, size) = elide_setup(cx);
    let path = "~/projects/toolbox/src/ui/tab_sidebar.rs";
    let out = elide_label(&ts, &font, size, path, 200.);
    assert!(out.starts_with("~/…/"), "root survives: {out}");
    assert!(out.ends_with("tab_sidebar.rs"), "leaf survives: {out}");
}

#[test]
fn short_title_cuts_windows_paths_on_backslashes() {
    assert_eq!(
        short_title(r"C:\Users\dev\projects\app"),
        r"…\dev\projects\app"
    );
    assert_eq!(
        short_title(r"C:\Users\dev\repo\deep\path\src\ui"),
        r"…\path\src\ui"
    );
    // A shallow Windows path keeps its drive and its backslashes.
    assert_eq!(short_title(r"C:\Users\app"), r"C:\Users\app");
}

/// Every way of slicing `text` that lands on a grapheme-cluster boundary.
fn cluster_prefixes(text: &str) -> Vec<String> {
    let clusters: Vec<&str> = text.graphemes(true).collect();
    (0..=clusters.len())
        .map(|n| clusters[..n].concat())
        .collect()
}

fn cluster_suffixes(text: &str) -> Vec<String> {
    let clusters: Vec<&str> = text.graphemes(true).collect();
    (0..=clusters.len())
        .map(|n| clusters[clusters.len() - n..].concat())
        .collect()
}

/// An elision may only drop whole grapheme clusters, so whatever survives
/// on either side of the ellipsis has to be a cluster-aligned prefix and
/// suffix of what went in. Slicing by `char` instead passes every width
/// check and still tears `👨‍👩‍👧` into a dangling joiner, strips the
/// variation selector off `❤️`, or leaves half of `🇨🇳` to render as a
/// bare letter.
#[track_caller]
fn assert_cut_on_cluster_boundaries(input: &str, out: &str, max: f32) {
    let Some((head, tail)) = out.split_once('…') else {
        assert_eq!(out, input, "an unelided label comes back verbatim");
        return;
    };
    assert!(
        cluster_prefixes(input).iter().any(|p| p == head),
        "head {head:?} is not a cluster-aligned prefix of {input:?} (@{max}px)"
    );
    assert!(
        cluster_suffixes(input).iter().any(|s| s == tail),
        "tail {tail:?} is not a cluster-aligned suffix of {input:?} (@{max}px)"
    );
}

/// Fixtures whose clusters are wider than one `char`, placed so that a
/// `char`-indexed cut lands inside one at some width.
const CLUSTER_FIXTURES: [&str; 7] = [
    "ab\u{1F468}\u{200d}\u{1F469}\u{200d}\u{1F467}cdefghijklmnop",
    "release-notes-final-ab\u{1F468}\u{200d}\u{1F469}\u{200d}\u{1F467}",
    "abcdef\u{2764}\u{fe0f}ghijklmnopqr",
    "long-branch-name-x\u{2764}\u{fe0f}",
    "abcdef\u{1F1E8}\u{1F1F3}ghijklmnopqr",
    "abcde\u{301}fghijklmnopqrst",
    "review-\u{1F44D}\u{1F3FD}-approved-changes",
];

#[gpui::test]
fn elide_edges_cuts_only_on_cluster_boundaries(cx: &mut TestAppContext) {
    let (ts, font, size) = elide_setup(cx);
    for text in CLUSTER_FIXTURES {
        let mut max = 30.;
        while max <= 200. {
            let out = elide_keep_edges(&ts, &font, size, text, max);
            assert_cut_on_cluster_boundaries(text, &out, max);
            assert!(
                measure_text(&ts, &font, size, &out) <= max,
                "{out:?} still has to fit its budget (@{max}px)"
            );
            max += 2.;
        }
    }
}

/// The path elision tears its last segment character by character as a
/// last resort; that resort has to respect clusters too.
#[gpui::test]
fn elide_path_tears_its_last_segment_on_cluster_boundaries(cx: &mut TestAppContext) {
    let (ts, font, size) = elide_setup(cx);
    for leaf in CLUSTER_FIXTURES {
        let path = format!("~/projects/toolbox/{leaf}");
        let mut max = 30.;
        while max <= 120. {
            let out = elide_path_keep_tail(&ts, &font, size, &path, max);
            // Whatever it settled on, the tail after the ellipsis has to
            // be cluster-aligned against the path it came from.
            if let Some((_, tail)) = out.split_once('…') {
                let tail = tail.trim_start_matches('/');
                assert!(
                    cluster_suffixes(&path).iter().any(|s| s == tail),
                    "tail {tail:?} is not a cluster-aligned suffix of {path:?} (@{max}px)"
                );
            }
            max += 2.;
        }
    }
}

/// `short_title`'s 40-glyph clamp is the other `char`-indexed cut.
#[test]
fn short_title_clamps_on_cluster_boundaries() {
    for tail in ["\u{1F1E8}\u{1F1F3}-suffix", "\u{2764}\u{fe0f}-suffix"] {
        for pad in 37..=41 {
            let name = format!("{}{tail}", "a".repeat(pad));
            let out = short_title(&name);
            let Some(body) = out.strip_suffix('…') else {
                continue;
            };
            assert!(
                cluster_prefixes(&name).iter().any(|p| p == body),
                "clamped to {body:?}, not a cluster-aligned prefix of {name:?}"
            );
        }
    }
}

/// One chip is 100 wide plus a 6 gap, so this is "room for exactly four".
const FOUR_CHIPS: f32 = 4. * (CHIP_MIN_W + CHIP_GAP);

#[test]
fn every_chip_is_drawn_while_they_all_fit() {
    let order: Vec<usize> = (0..4).collect();
    assert_eq!(visible_chips(&order, 0, FOUR_CHIPS), order);
    assert_eq!(visible_chips(&order, 3, FOUR_CHIPS), order);
    assert_eq!(visible_chips(&[0, 1], 1, FOUR_CHIPS), vec![0, 1]);
}

#[test]
fn the_run_stays_put_until_the_active_chip_would_fall_off() {
    let order: Vec<usize> = (0..9).collect();
    // Anchored at the first tab for as long as the active one is inside it.
    assert_eq!(visible_chips(&order, 0, FOUR_CHIPS), vec![0, 1, 2, 3]);
    assert_eq!(visible_chips(&order, 3, FOUR_CHIPS), vec![0, 1, 2, 3]);
    // Then it slides by exactly as much as it has to.
    assert_eq!(visible_chips(&order, 4, FOUR_CHIPS), vec![1, 2, 3, 4]);
    assert_eq!(visible_chips(&order, 8, FOUR_CHIPS), vec![5, 6, 7, 8]);
}

#[test]
fn the_active_chip_is_always_among_the_drawn_ones() {
    let order: Vec<usize> = (0..40).collect();
    for active in 0..40 {
        for avail in [0., 1., 80., FOUR_CHIPS, 4000.] {
            let shown = visible_chips(&order, active, avail);
            assert!(
                shown.contains(&active),
                "active {active} missing at {avail}px: {shown:?}"
            );
        }
    }
}

#[test]
fn a_reordered_run_is_sliced_in_its_own_order() {
    // Mid-drag the strip renders `preview.order`, not 0..n.
    let order = vec![3, 0, 1, 2, 4, 5];
    assert_eq!(visible_chips(&order, 5, FOUR_CHIPS), vec![1, 2, 4, 5]);
}

#[test]
fn configured_shell_arguments_remain_user_authored_in_the_menu() {
    let shell = DetectedShell {
        label: "custom".into(),
        program: "custom-shell".into(),
        args: vec!["--login".into()],
        args_are_anterm_defaults: false,
        user_authored: false,
    };
    let spec = shell_spec(&shell);

    assert_eq!(spec.program, "custom-shell");
    assert_eq!(spec.args, ["--login"]);
    assert!(!spec.args_are_anterm_defaults);
}

/// A tab of this window as the strip reads it: `tab_label` is nothing but
/// [`label_of`] over the [`TabView`](crate::ui::remote::machine_mirror::TabView)
/// that [`Tab::label_view`](crate::ui::workspace::app::Tab::label_view) builds from
/// the live leaf, so naming one here climbs the same ladder a real tab
/// climbs. `title` is the placeholder `label_view` fills that slot with —
/// the machine tree puts a process name there, a live pane has only the
/// name it answers to before anything has spoken.
fn strip_tab() -> crate::ui::remote::machine_mirror::TabView {
    crate::ui::remote::machine_mirror::TabView {
        id: anterm_core::machine::TabId::new(),
        name: None,
        title: crate::terminal::view::DEFAULT_TITLE.to_string(),
        osc_title: None,
        cwd: None,
        agent: None,
        status: None,
        live: true,
        panes: 1,
    }
}

/// The home the paths below are measured against — named rather than read
/// off this machine, so the assertions do not depend on who is running
/// them (#580).
fn home() -> &'static Path {
    Path::new("/Users/x")
}

#[test]
fn a_renamed_tab_keeps_its_name_over_every_other_answer() {
    let mut tab = strip_tab();
    tab.name = Some("  build  ".into());
    tab.osc_title = Some("vim — main.rs".into());
    tab.cwd = Some("/Users/x/repo/anterm".into());

    assert_eq!(label_of(&tab, 0, Some(home())), "build");
}

#[test]
fn a_pane_showing_a_title_is_named_by_it_and_not_by_its_directory() {
    let mut tab = strip_tab();
    tab.osc_title = Some("vim — main.rs".into());
    tab.cwd = Some("/Users/x/repo/anterm".into());

    assert_eq!(label_of(&tab, 0, Some(home())), "vim — main.rs");

    // Including the title an SSH pane answers to before the far shell has
    // said anything (#438): `label_view` hands that up here, so a window
    // full of them still reads as hosts rather than as directories.
    tab.osc_title = Some("prod-web".into());
    assert_eq!(label_of(&tab, 0, Some(home())), "prod-web");
}

/// #740: every shell anterm ships integration for except PowerShell reports
/// its directory over OSC 7 and never sets a title, which left the tab
/// reading "anterm" — the app's own name — while the switcher listing the
/// very same tab showed the directory.
#[test]
fn a_pane_that_has_only_said_where_it_is_is_named_after_that() {
    let mut tab = strip_tab();
    tab.cwd = Some("/Users/x/repo/anterm".into());

    assert_eq!(label_of(&tab, 0, Some(home())), "~/repo/anterm");
    // Through the same shortener as a title, so a deep directory is cut
    // where a deep path in a title would be.
    tab.cwd = Some("/Users/x/repo/anterm/crates/anterm-core/src".into());
    assert_eq!(
        label_of(&tab, 0, Some(home())),
        super::short_title("/Users/x/repo/anterm/crates/anterm-core/src", Some(home())),
    );
}

/// A tooltip exists to say what the row had to leave out. One that repeats
/// the row is worse than none, and the label and the raw string it came
/// from are not comparable until both have been abbreviated: `~/repo` and
/// `/Users/x/repo` are the same name spelled two ways, and reading them as
/// a difference hung a tooltip on every tab named after a directory under
/// the home — which, after this change, is most of them.
#[test]
fn a_tab_named_after_a_directory_says_nothing_more_on_hover_unless_it_was_cut() {
    let mut tab = strip_tab();
    tab.cwd = Some("/Users/x/repo".into());

    assert_eq!(label_of(&tab, 0, Some(home())), "~/repo");
    assert_eq!(
        tooltip_of(&tab, 0, Some(home())),
        None,
        "the row is already showing the whole directory"
    );

    // Cut down to its last three segments, so the head is worth having.
    tab.cwd = Some("/Users/x/repo/crates/anterm-core/src".into());
    assert_eq!(label_of(&tab, 0, Some(home())), "…/crates/anterm-core/src");
    assert_eq!(
        tooltip_of(&tab, 0, Some(home())).as_deref(),
        Some("~/repo/crates/anterm-core/src")
    );

    // The same holds for a title that happens to be a path — the rung this
    // guard was already getting wrong before a directory could reach it.
    let mut titled = strip_tab();
    titled.osc_title = Some("/Users/x/repo".into());
    assert_eq!(tooltip_of(&titled, 0, Some(home())), None);

    // A shell integration's `user@host:` head is not in the label, so it
    // is still worth spelling out.
    titled.osc_title = Some("me@box:/Users/x/repo".into());
    assert_eq!(
        tooltip_of(&titled, 0, Some(home())).as_deref(),
        Some("me@box:/Users/x/repo")
    );
}

/// The one test that fails if any of the wiring is put back: a real tab,
/// built the way the window builds one, named through `tab_label` — and
/// checked against what the switcher renders from the machine tree's view
/// of that very same pane. Before this change the strip said "anterm" and
/// the switcher said the directory (#740).
#[gpui::test]
fn the_strip_names_a_titleless_pane_exactly_as_the_switcher_does(cx: &mut TestAppContext) {
    use crate::ui::workspace::pane::{Pane, PaneSlot};

    let (app, mut vcx) = crate::ui::workspace::app::test_window::harness(cx);
    let _stream = app.update_in(&mut vcx, |app, window, cx| {
        let (view, stream) = crate::terminal::view::quiet_test_pane(1, window, cx);
        // A pane that has reported where it is over OSC 7 and has never
        // titled itself — every shell anterm ships integration for except
        // PowerShell.
        view.read(cx)
            .terminal
            .seed_cwd(Some(std::path::PathBuf::from("/work/repo")));
        app.tabs
            .push(crate::ui::workspace::app::Tab::new(Pane::leaf(PaneSlot::Ready(view))));
        app.active = app.tabs.len() - 1;
        stream
    });
    vcx.background_executor.run_until_parked();

    app.update_in(&mut vcx, |app, window, cx| {
        let index = app.active;
        let tab = &app.tabs[index];
        let (view, home) = tab.label_view(Some(window), cx);
        assert_eq!(view.osc_title, None, "the pane never titled itself");
        assert_eq!(view.cwd.as_deref(), Some("/work/repo"));

        let strip = app.tab_label(tab, index, Some(window), cx);
        assert_eq!(strip, "/work/repo");
        assert_ne!(
            strip,
            crate::terminal::view::DEFAULT_TITLE,
            "and is not named after the app any more"
        );

        // The machine tree's reading of the same pane, which is all the
        // switcher ever has: no title was seen, the cwd is the one above,
        // and `title` is the foreground process name.
        let from_tree = crate::ui::remote::machine_mirror::TabView {
            id: tab.tree_id.get(),
            name: None,
            title: "zsh".into(),
            osc_title: None,
            cwd: Some("/work/repo".into()),
            agent: None,
            status: None,
            live: true,
            panes: 1,
        };
        assert_eq!(
            strip,
            label_of(&from_tree, index, home.as_deref()),
            "the two columns name the same tab the same way"
        );

        assert_eq!(
            app.tab_title_tooltip(tab, index, Some(window), cx),
            None,
            "and the row is showing the whole path, so it stays quiet"
        );
    });
}

#[test]
fn a_pane_with_nothing_to_say_falls_back_the_way_it_always_did() {
    // No title and no directory: the placeholder, exactly as before.
    let tab = strip_tab();
    assert_eq!(label_of(&tab, 0, Some(home())), "anterm");

    // And a tab holding no live pane at all is still numbered.
    let mut empty = strip_tab();
    empty.title = String::new();
    assert!(label_of(&empty, 2, Some(home())).contains('3'));
}

/// The rung under the shortener, which the two surfaces reach holding
/// different things. A shell that has said who and where it is but not
/// *where* — `user@host:` with nothing after the colon — leaves nothing to
/// show, and whatever stands in has to be something the tab does not
/// already say: the switcher has the foreground process name, and a tab of
/// this window has only the placeholder, which is the answer #740 removed.
#[test]
fn a_title_that_shortens_away_never_puts_the_app_name_back_on_the_tab() {
    let mut strip = strip_tab();
    strip.osc_title = Some("user@host:".into());
    assert_ne!(
        label_of(&strip, 0, Some(home())),
        crate::terminal::view::DEFAULT_TITLE
    );
    assert!(
        label_of(&strip, 0, Some(home())).contains('1'),
        "the numbered placeholder, which is what the strip showed here \
         before it shared this renderer"
    );

    // The switcher arrives with a real process name in that slot, and it
    // is still worth more than a number.
    let from_tree = crate::ui::remote::machine_mirror::TabView {
        title: "zsh".into(),
        osc_title: Some("user@host:".into()),
        ..strip_tab()
    };
    assert_eq!(label_of(&from_tree, 0, Some(home())), "zsh");
}

/// A path is spelled the way the machine it is on spells it, and which
/// machine that is has nothing to do with which one anterm is running on: a
/// remote pane reports POSIX to a Windows client, and a Windows pane
/// reports backslashes to a client that has never seen one (#580).
#[test]
fn a_cwd_is_cut_in_its_own_spelling_whichever_client_is_reading_it() {
    let windows_home = Path::new(r"C:\Users\x");

    // A Windows pane: shortened under its own home, and a path too deep to
    // fit is rejoined with its own separator rather than with `/`.
    let mut win = strip_tab();
    win.cwd = Some(r"C:\Users\x\repo".into());
    assert_eq!(label_of(&win, 0, Some(windows_home)), "~/repo");
    win.cwd = Some(r"D:\work\a\b\proj".into());
    assert_eq!(label_of(&win, 0, Some(windows_home)), r"…\a\b\proj");

    // A remote pane's cwd is POSIX even when the client reading it is the
    // Windows one: no drive to hang it off, no `~` borrowed from this
    // machine's home, and no backslash anywhere in the answer.
    let mut remote = strip_tab();
    remote.cwd = Some("/srv/app".into());
    assert_eq!(label_of(&remote, 0, Some(windows_home)), "/srv/app");
    remote.cwd = Some("/home/deploy/app".into());
    assert_eq!(
        label_of(&remote, 0, Some(Path::new("/home/deploy"))),
        "~/app",
        "measured against the home of the host it is on, not of this one"
    );

    // The root of a filesystem is a directory like any other: a tab
    // sitting in it says so, and says nothing more on hover.
    let mut root = strip_tab();
    root.cwd = Some("/".into());
    assert_eq!(label_of(&root, 0, Some(home())), "/");
    assert_eq!(tooltip_of(&root, 0, Some(home())), None);
}

fn inventory(labels: &[&str]) -> Vec<DetectedShell> {
    labels
        .iter()
        .map(|l| DetectedShell {
            label: l.to_string(),
            program: format!("/bin/{l}"),
            args: Vec::new(),
            args_are_anterm_defaults: true,
            user_authored: false,
        })
        .collect()
}

fn used(
    entries: &[(&str, u32, u64)],
) -> std::collections::HashMap<String, crate::core::config::ProfileUsage> {
    entries
        .iter()
        .map(|(l, count, last_used)| {
            (
                l.to_string(),
                crate::core::config::ProfileUsage {
                    count: *count,
                    last_used: *last_used,
                },
            )
        })
        .collect()
}

fn labels(shells: &[&DetectedShell]) -> Vec<String> {
    shells.iter().map(|s| s.label.clone()).collect()
}

const STOCK_MAC: &[&str] = &["zsh", "bash", "sh", "csh", "tcsh", "ksh", "dash"];

#[test]
fn the_new_tab_menu_names_the_default_and_what_has_been_opened_by_frecency() {
    let now = 100_000_000u64;
    let day = 86_400u64;
    let shells = inventory(STOCK_MAC);
    // `bash` used often and lately; `dash` used more but a year ago;
    // `ksh` once, two months back; and the default opened too.
    let usage = used(&[
        ("zsh", 50, now),
        ("ksh", 1, now - 60 * day),
        ("bash", 9, now - day),
        ("dash", 20, now - 365 * day),
    ]);
    let sorted = shells_by_frecency(&shells, "zsh", &usage, now);
    assert_eq!(
        labels(&sorted),
        ["zsh", "bash", "dash", "ksh", "sh", "csh", "tcsh"],
        "default first, then by frecency, then the inventory's own order"
    );
    let menu = menu_shells(&sorted, "zsh", &usage, now);
    assert_eq!(menu.len(), MENU_SHELLS);
    assert_eq!(labels(&menu), ["zsh", "bash", "dash"]);
}

#[test]
fn the_default_shell_leads_the_menu_even_when_it_has_never_been_opened() {
    let now = 100_000_000u64;
    let shells = inventory(STOCK_MAC);
    let usage = used(&[("tcsh", 3, now), ("sh", 7, now), ("ksh", 5, now)]);
    let sorted = shells_by_frecency(&shells, "bash", &usage, now);
    let menu = menu_shells(&sorted, "bash", &usage, now);
    assert_eq!(labels(&menu), ["bash", "sh", "ksh"]);
}

#[test]
fn a_shell_nobody_has_opened_is_not_named_just_because_there_is_room() {
    // A fresh install: nothing has been opened, so the menu names the
    // default alone and leaves the rest to the palette.
    let now = 100_000_000u64;
    let shells = inventory(STOCK_MAC);
    let none = used(&[]);
    let sorted = shells_by_frecency(&shells, "zsh", &none, now);
    assert_eq!(labels(&menu_shells(&sorted, "zsh", &none, now)), ["zsh"]);

    // A usage record at zero counts as never opened.
    let zeroed = used(&[("bash", 0, now)]);
    let sorted = shells_by_frecency(&shells, "zsh", &zeroed, now);
    assert_eq!(labels(&menu_shells(&sorted, "zsh", &zeroed, now)), ["zsh"]);

    // An inventory that does not list the default still offers its head,
    // which is the shell a new tab opens with.
    let sorted = shells_by_frecency(&shells, "fish", &none, now);
    assert_eq!(labels(&menu_shells(&sorted, "fish", &none, now)), ["zsh"]);
}

#[test]
fn other_shells_is_offered_only_when_the_inventory_holds_more_than_the_menu_names() {
    let now = 100_000_000u64;
    let more = |inv: &[&str], usage: &[(&str, u32, u64)]| {
        let shells = inventory(inv);
        let usage = used(usage);
        let default = inv.first().copied().unwrap_or("");
        let sorted = shells_by_frecency(&shells, default, &usage, now);
        menu_shells(&sorted, default, &usage, now).len() < sorted.len()
    };
    assert!(more(STOCK_MAC, &[]), "a stock box names one of seven");
    assert!(
        !more(&["pwsh"], &[]),
        "the default alone is the whole inventory"
    );
    assert!(
        !more(&["zsh", "bash"], &[("bash", 1, now)]),
        "every shell is already a row"
    );
    assert!(
        !more(
            &["zsh", "bash", "fish"],
            &[("bash", 1, now), ("fish", 2, now)]
        ),
        "three shells, all named"
    );
    assert!(
        more(&["zsh", "bash", "fish"], &[("bash", 1, now)]),
        "`fish` has never been opened, so it is left to the palette"
    );
    assert!(
        !more(&[], &[]),
        "no inventory: the fallback row, nothing more"
    );
}

mod ssh_host_row_tests {
    use crate::core::config::Config;
    use crate::core::ssh_profile::SshProfile;
    use crate::daemon::protocol::SshProxy;
    use crate::terminal::view::{
        quiet_test_pane, quiet_test_ssh_pane, quiet_test_ssh_pane_of, quiet_test_ssh_pane_with,
    };
    use crate::ui::workspace::app::{Tab, test_window::harness};
    use anterm_i18n::{L10nKey, set_locale, t};
    use crate::ui::workspace::pane::{Pane, PaneSlot};
    use crate::ui::remote::ssh_connect::TabHostForm;
    use gpui::TestAppContext;

    #[gpui::test]
    fn only_a_tab_on_an_ssh_host_is_offered_the_host_form(cx: &mut TestAppContext) {
        set_locale("en");
        let (app, mut vcx) = harness(cx);
        let saved = uuid::Uuid::new_v4();

        // Held for the life of the test: dropping the daemon end of a pane's
        // transport tears the pane down under the assertions.
        let _ends = app.update_in(&mut vcx, |app, window, cx| {
            let mut cfg = cx.global::<Config>().clone();
            let mut profile = SshProfile::new("build-box");
            profile.id = saved;
            profile.user = "me".to_string();
            profile.host = "build-box".to_string();
            cfg.ssh_profiles = vec![profile];
            cx.set_global(cfg);

            let (local, a) = quiet_test_pane(1, window, cx);
            let (dialled, b) = quiet_test_ssh_pane(2, window, cx);
            let (from_host, c) = quiet_test_ssh_pane_of(3, Some(saved), window, cx);
            for view in [local, dialled, from_host] {
                app.tabs.push(Tab::new(Pane::leaf(PaneSlot::Ready(view))));
            }
            app.active = 0;
            cx.notify();
            (a, b, c)
        });
        vcx.background_executor.run_until_parked();

        app.update_in(&mut vcx, |app, window, cx| {
            // A local shell has no connection to edit, so the menu it opens is
            // the one it always was.
            assert_eq!(
                app.tab_ssh_host_form(0, window, cx),
                None,
                "a local tab was offered an SSH host form"
            );

            // An address typed by hand is worth keeping, not editing: there is
            // no saved host behind it yet, so the live session itself is what
            // the form opens on.
            let (form, label) = app
                .tab_ssh_host_form(1, window, cx)
                .expect("a tab dialled by hand offers to save the host");
            let TabHostForm::Unsaved(spec) = form else {
                panic!("a hand-dialled tab must offer its own session, not a bare address");
            };
            assert_eq!(
                (spec.user.as_str(), spec.host.as_str(), spec.port),
                ("me", "build-box", 22)
            );
            assert_eq!(label, t(L10nKey::SwitcherSaveAsHost));

            // One opened from a saved host edits that host — by its id, so the
            // form lands on the record the connection actually came from.
            let (form, label) = app
                .tab_ssh_host_form(2, window, cx)
                .expect("a tab on a saved host offers to edit it");
            assert_eq!(form, TabHostForm::Saved(saved));
            assert_eq!(label, t(L10nKey::SwitcherEditHost));
        });
    }

    #[gpui::test]
    fn a_host_deleted_under_a_live_tab_is_offered_back_as_a_new_one(cx: &mut TestAppContext) {
        // The id a pane carries is the one it was spawned with, and a quick
        // connection is handed a fresh uuid on its way to the daemon. Trusting
        // the id alone would open the form on a host that is not there.
        set_locale("en");
        let (app, mut vcx) = harness(cx);
        let _end = app.update_in(&mut vcx, |app, window, cx| {
            let (view, end) = quiet_test_ssh_pane_of(1, Some(uuid::Uuid::new_v4()), window, cx);
            app.tabs.push(Tab::new(Pane::leaf(PaneSlot::Ready(view))));
            app.active = 0;
            cx.notify();
            end
        });
        vcx.background_executor.run_until_parked();

        app.update_in(&mut vcx, |app, window, cx| {
            let (form, label) = app
                .tab_ssh_host_form(0, window, cx)
                .expect("an unresolvable profile id still names a host worth keeping");
            let TabHostForm::Unsaved(spec) = form else {
                panic!("a dangling profile id must not open a form on a host that is gone");
            };
            assert_eq!(
                (spec.user.as_str(), spec.host.as_str(), spec.port),
                ("me", "build-box", 22)
            );
            assert_eq!(label, t(L10nKey::SwitcherSaveAsHost));
        });
    }

    /// The row says "Save as SSH Host", and a host saved without the proxy it
    /// was reached through is a host that will not connect. What the session
    /// was dialled with has to reach the form whole — an address is only the
    /// part of it that fits in `user@host:port`.
    #[gpui::test]
    fn saving_a_hand_dialled_tab_keeps_what_it_was_dialled_with(cx: &mut TestAppContext) {
        set_locale("en");
        let (app, mut vcx) = harness(cx);
        let _end = app.update_in(&mut vcx, |app, window, cx| {
            let mut spec: crate::daemon::protocol::NativeSshSpec = serde_json::from_str(
                r#"{"host":"build-box","port":2222,"user":"me","auth_mode":"auto"}"#,
            )
            .expect("a minimal NativeSshSpec decodes");
            spec.proxy = SshProxy::Socks {
                host: "127.0.0.1".to_string(),
                port: 1080,
            };
            spec.identity_files = vec!["/keys/id_ed25519".to_string()];
            spec.login_script = vec!["tmux attach".to_string()];
            let (view, end) = quiet_test_ssh_pane_with(1, spec, window, cx);
            app.tabs.push(Tab::new(Pane::leaf(PaneSlot::Ready(view))));
            app.active = 0;
            cx.notify();
            end
        });
        vcx.background_executor.run_until_parked();

        app.update_in(&mut vcx, |app, window, cx| {
            let (form, _) = app
                .tab_ssh_host_form(0, window, cx)
                .expect("a hand-dialled tab offers to save the host");
            let TabHostForm::Unsaved(spec) = form else {
                panic!("nothing here is saved, so nothing here is an edit");
            };
            assert_eq!(
                spec.proxy,
                SshProxy::Socks {
                    host: "127.0.0.1".to_string(),
                    port: 1080,
                },
                "the proxy the session was reached through was dropped on the way to the form"
            );
            assert_eq!(spec.identity_files, vec!["/keys/id_ed25519".to_string()]);
            assert_eq!(spec.login_script, vec!["tmux attach".to_string()]);
            assert_eq!(spec.port, 2222, "a non-default port is part of the address");
        });
    }
}
