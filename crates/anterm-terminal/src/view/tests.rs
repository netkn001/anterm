/// What the label ladder asks a pane: are you showing a name of your own,
/// or still standing under the app's? (#740)
#[test]
fn initial_grid_from_cell_metrics_matches_prepaint_floor_and_falls_back_on_tiny_bounds() {
    use super::{GRID_PAD_X, GRID_PAD_Y, InitialGrid};
    use crate::TermSize;
    use gpui::{px, size};

    // Exact realistic pane (1200x752) on a 2x Retina display with 8x17 logical cells:
    // inner = (1200 - 16) x (752 - 8) = 1184 x 744 => 148 x 43, device cell = 16 x 34.
    let grid = InitialGrid::from_cell_metrics(size(px(1200.), px(752.)), px(8.), px(17.), 2.0);
    assert_eq!(grid.size, TermSize::new(148, 43));
    assert_eq!((grid.cell_w, grid.cell_h), (16, 34));

    // Fractional logical cell width floors cols/rows and rounds device pixels.
    let frac = InitialGrid::from_cell_metrics(size(px(1000.), px(600.)), px(7.8), px(18.), 2.0);
    let expected_cols = ((1000. - 2. * GRID_PAD_X) / 7.8).floor() as usize;
    let expected_rows = ((600. - 2. * GRID_PAD_Y) / 18.).floor() as usize;
    assert_eq!(frac.size, TermSize::new(expected_cols, expected_rows));
    assert_eq!((frac.cell_w, frac.cell_h), (16, 36));

    // Zero or degenerate bounds fall back safely to 80x24 while preserving cell metrics.
    for tiny in [
        size(px(0.), px(0.)),
        size(px(2. * GRID_PAD_X), px(2. * GRID_PAD_Y)),
        size(px(20.), px(20.)),
    ] {
        let fallback = InitialGrid::from_cell_metrics(tiny, px(8.), px(17.), 2.0);
        assert_eq!(fallback.size, TermSize::new(80, 24));
        assert_eq!((fallback.cell_w, fallback.cell_h), (16, 34));
    }
}

#[test]
fn a_pane_states_a_title_whenever_it_is_not_the_placeholder() {
    use super::stated_title;

    // Nothing has spoken — this is the pane a directory stands in for.
    assert_eq!(stated_title("anterm"), None);
    assert_eq!(stated_title("  anterm  "), None);
    assert_eq!(stated_title("   "), None);

    // A title from the program running in it.
    assert_eq!(stated_title("vim — main.rs"), Some("vim — main.rs"));
    assert_eq!(stated_title(" user@host:~/repo "), Some("user@host:~/repo"));
    // A default anterm chose for the pane itself is a name, not the absence
    // of one: an SSH pane answers to its host (#438) and a workspace pane
    // to its workspace, and neither gives way to a directory.
    assert_eq!(stated_title("prod-web"), Some("prod-web"));
    // So does the state a finished pane is left showing.
    assert_eq!(
        stated_title("anterm — process exited"),
        Some("anterm — process exited")
    );
}

#[test]
fn an_unfocused_input_caret_is_always_a_steady_outline() {
    use super::{InputCaretPaint, input_caret_paint};
    use anterm_ui_kit::config::CursorStyle;

    for blink_on in [false, true] {
        for style in [CursorStyle::Bar, CursorStyle::Block, CursorStyle::Underline] {
            assert_eq!(
                input_caret_paint(false, blink_on, style),
                Some(InputCaretPaint::Outline),
            );
        }
    }
}

#[test]
fn a_focused_input_caret_keeps_its_configured_shape_and_blink_phase() {
    use super::{InputCaretPaint, input_caret_paint};
    use anterm_ui_kit::config::CursorStyle;

    assert_eq!(input_caret_paint(true, false, CursorStyle::Block), None);
    assert_eq!(
        input_caret_paint(true, true, CursorStyle::Bar),
        Some(InputCaretPaint::Bar),
    );
    assert_eq!(
        input_caret_paint(true, true, CursorStyle::Block),
        Some(InputCaretPaint::Block),
    );
    assert_eq!(
        input_caret_paint(true, true, CursorStyle::Underline),
        Some(InputCaretPaint::Underline),
    );
}

#[test]
fn a_busy_command_name_undoes_the_shell_escaping() {
    use super::unescape_mark_text;
    // The integration escapes % and the four bytes that would break OSC
    // framing; everything else is the line as typed.
    assert_eq!(unescape_mark_text("printf '100%25'"), "printf '100%'");
    assert_eq!(unescape_mark_text("a%1Bb%07c%0Dd"), "abcd");
    assert_eq!(unescape_mark_text("one%0Atwo"), "one two");
    // A bare % the shell somehow left alone must survive rather than eat
    // the next two characters.
    assert_eq!(unescape_mark_text("50% off"), "50% off");
    assert_eq!(unescape_mark_text("cargo build"), "cargo build");
    // PowerShell `[Uri]::EscapeDataString` percent-encodes spaces, slashes,
    // and non-ASCII UTF-8 bytes inside OSC 133;C.
    assert_eq!(unescape_mark_text("git%20status"), "git status");
    assert_eq!(unescape_mark_text("cat%20foo%2Fbar"), "cat foo/bar");
    assert_eq!(unescape_mark_text("echo%20%E4%B8%AD%E6%96%87"), "echo 中文");
}

#[test]
fn a_busy_command_name_stays_short_enough_to_read() {
    use super::{BUSY_COMMAND_MAX, clamp_command};
    assert_eq!(clamp_command("  sleep 300  "), "sleep 300");
    let long = "cargo ".repeat(40);
    let out = clamp_command(&long);
    assert!(out.ends_with('…'), "{out:?}");
    assert!(out.chars().count() <= BUSY_COMMAND_MAX + 1, "{out:?}");
    // No dangling space before the ellipsis.
    assert!(!out.contains(" …"), "{out:?}");
}
use super::{
    COMPLETION_MENU_MAX_W, LoopbackPlan, PortRoute, RawInput, SelectEndCopy, Typeahead,
    WheelRoute, clipboard_paths, compose_notification_title, cwd_is_on_host, display_width,
    link_path_style, loopback_plan, observe_typeahead_for_owner, typeahead_boundary,
};
use super::{SCROLL_ANIM_FRAME, scroll_anim_step};
use super::{
    TitleSettle, files_cwd, local_path_for_pane, remote_paste_spec, settle_title,
    stages_clipboard_image, staging_cache, staging_dir_is_safe, wsl_path, wsl_share_distro,
    wsl_share_path,
};
use super::{
    before_input, description_budget, drag_scroll_step, elide, encode_mouse,
    expand_file_command_template, fallback_chain, fig_icon_emoji, fig_icon_glyph,
    focus_report_bytes, highlight_runs, input_cells, input_char_positions,
    input_overflow_shift, input_overlay_rows, input_start, menu_layout, paste_bytes,
    select_end_copy, should_show_context_menu, smooth_scroll_step, submit_bytes,
    trim_trailing_spaces, wheel_route, wrapped_click_index,
};
use alacritty_terminal::term::TermMode;
use gpui::{ClipboardEntry, ClipboardItem, ExternalPaths, Modifiers};
use gpui_kit::component::IconName;
use std::path::{Path, PathBuf};

use anterm_core::daemon::protocol::RemoteKind;

/// #1142: a native SSH host pane runs anterm's prompt editor over the far
/// shell, so the far shell's own history is never consulted — the
/// editor's list is all ↑, Ctrl+R and the ghost suggestion have. That
/// list has to be seeded from the far end's files, over the pane's own
/// connection.
#[test]
fn a_native_ssh_pane_seeds_its_history_from_the_far_end() {
    use super::{HistorySeedSource, history_seed_source};
    assert_eq!(
        history_seed_source(false, Some(RemoteKind::NativeSsh)),
        HistorySeedSource::PaneSftp,
    );
}

#[test]
fn history_seeding_keeps_to_the_machine_the_scope_names() {
    use super::{HistorySeedSource, history_seed_source};
    for kind in [RemoteKind::Ssh, RemoteKind::Wsl] {
        assert_eq!(
            history_seed_source(false, Some(kind)),
            HistorySeedSource::None,
            "{kind:?}",
        );
    }
    assert_eq!(
        history_seed_source(true, None),
        HistorySeedSource::None,
    );
}

#[test]
fn a_title_the_tab_already_shows_drops_the_one_that_was_waiting() {
    // The revert is what cancels a flash: the pane is told the title it is
    // already displaying, and the queued command name goes away with it.
    assert_eq!(
        settle_title("~/dev", true, "~/dev"),
        TitleSettle::Revert,
        "a revert cancels the pending title rather than queueing behind it"
    );
    assert_eq!(
        settle_title("~/dev", false, "~/dev"),
        TitleSettle::Revert,
        "and asks for no wait when there was nothing pending either"
    );
    // A second title while one is already waiting rides the wait in
    // flight; restarting it is what would let a title that keeps changing
    // put the tab's next update off forever.
    assert_eq!(settle_title("~/dev", true, "wget 40%"), TitleSettle::Queue);
    assert_eq!(
        settle_title("~/dev", false, "wget 1%"),
        TitleSettle::QueueAndWait
    );
}

#[test]
fn a_spinner_frame_that_only_changes_the_status_mark_asks_for_no_repaint() {
    // An agent rewrites its title every few hundred milliseconds while it
    // works, and the tab never draws the mark, so none of these may wait
    // and repaint the whole window for the same label.
    assert_eq!(
        settle_title(
            "\u{25D0} fixing the switcher",
            false,
            "\u{25D1} fixing the switcher"
        ),
        TitleSettle::Revert
    );
    assert_eq!(
        settle_title(
            "\u{25D0} fixing the switcher",
            true,
            "\u{2733} fixing the switcher"
        ),
        TitleSettle::Revert,
        "and a frame that lands while another title waits drops it, as a revert does"
    );
    assert_eq!(
        settle_title(
            "\u{25D0} fixing the switcher",
            false,
            "\u{25D1} fixing the parser"
        ),
        TitleSettle::QueueAndWait,
        "a different title still lands"
    );
}

#[test]
fn a_notification_title_keeps_at_most_two_segments() {
    let ws = || Some("anterm".to_string());
    assert_eq!(
        compose_notification_title(None, Some("build-box".into()), ws()),
        "build-box · anterm"
    );
    // An agent takes the machine's place rather than adding a third part.
    assert_eq!(
        compose_notification_title(Some("Claude".into()), Some("build-box".into()), ws()),
        "Claude · anterm"
    );
    // A local pane has no machine label; a nameless workspace has no name.
    assert_eq!(compose_notification_title(None, None, ws()), "anterm");
    assert_eq!(
        compose_notification_title(Some("Claude".into()), None, None),
        "Claude"
    );
    assert_eq!(compose_notification_title(None, None, None), "anterm");
}

#[test]
fn alt_screen_exit_discards_the_boundary_input_before_recording_shell_text() {
    let mut typeahead = Typeahead::new();
    let mut last_blocked = true;
    typeahead.observe(RawInput::Text("stale"), false);

    observe_typeahead_for_owner(
        &mut typeahead,
        &mut last_blocked,
        RawInput::Key {
            key: "c",
            plain: false,
        },
        false,
    );
    assert_eq!(
        typeahead.drain(),
        None,
        "the Ctrl-C crossing TUI exit must not become a tainted shell record"
    );

    observe_typeahead_for_owner(
        &mut typeahead,
        &mut last_blocked,
        RawInput::Text("ls"),
        false,
    );
    assert_eq!(typeahead.drain(), Some("ls".to_string()));
}

#[test]
fn agent_interrupt_discards_typeahead_without_an_alt_screen_transition() {
    let mut typeahead = Typeahead::new();
    let mut last_blocked = false;
    typeahead.observe(RawInput::Text("agent input"), false);
    typeahead.observe(
        RawInput::Key {
            key: "up",
            plain: true,
        },
        false,
    );

    observe_typeahead_for_owner(
        &mut typeahead,
        &mut last_blocked,
        RawInput::Interrupt,
        false,
    );
    assert_eq!(
        typeahead.drain(),
        None,
        "Ctrl-C must cancel a stable non-ALT_SCREEN agent gap"
    );

    observe_typeahead_for_owner(
        &mut typeahead,
        &mut last_blocked,
        RawInput::Text("ls"),
        false,
    );
    assert_eq!(typeahead.drain(), Some("ls".to_string()));
}

#[test]
fn known_agent_input_is_discarded_at_both_ownership_boundaries() {
    let mut typeahead = Typeahead::new();
    let mut last_blocked = false;
    typeahead.observe(RawInput::Text("stale shell gap"), false);

    observe_typeahead_for_owner(
        &mut typeahead,
        &mut last_blocked,
        RawInput::Text("agent input"),
        true,
    );
    observe_typeahead_for_owner(
        &mut typeahead,
        &mut last_blocked,
        RawInput::Key {
            key: "up",
            plain: true,
        },
        true,
    );
    assert_eq!(typeahead.drain(), None);

    observe_typeahead_for_owner(
        &mut typeahead,
        &mut last_blocked,
        RawInput::Text("boundary input"),
        false,
    );
    assert_eq!(typeahead.drain(), None);

    observe_typeahead_for_owner(
        &mut typeahead,
        &mut last_blocked,
        RawInput::Text("ls"),
        false,
    );
    assert_eq!(typeahead.drain(), Some("ls".to_string()));
}

#[test]
fn ctrl_c_and_ctrl_d_discard_foreground_typeahead() {
    let ctrl = Modifiers {
        control: true,
        ..Default::default()
    };
    assert!(matches!(
        typeahead_boundary("c", &ctrl),
        Some(RawInput::Interrupt)
    ));
    assert!(matches!(
        typeahead_boundary("d", &ctrl),
        Some(RawInput::EndOfInput)
    ));
    assert!(typeahead_boundary("u", &ctrl).is_none());

    let ctrl_alt = Modifiers {
        control: true,
        alt: true,
        ..Default::default()
    };
    assert!(typeahead_boundary("c", &ctrl_alt).is_none());
    assert!(typeahead_boundary("d", &ctrl_alt).is_none());
}

#[test]
fn local_pane_opens_localhost_directly() {
    assert_eq!(loopback_plan(true, None, 1), LoopbackPlan::Direct);
}

#[test]
fn ssh_pane_forwards_on_the_pane() {
    assert_eq!(
        loopback_plan(true, Some(RemoteKind::NativeSsh), 7),
        LoopbackPlan::ForwardOnPane(7)
    );
    assert_eq!(
        loopback_plan(true, Some(RemoteKind::Wsl), 7),
        LoopbackPlan::Direct
    );
}

#[test]
fn a_remote_port_is_a_forward_away_rather_than_out_of_reach() {
    use super::port_route_of;
    assert_eq!(
        port_route_of(&LoopbackPlan::ForwardOnPane(7), false),
        PortRoute::Forward
    );
    assert_eq!(
        port_route_of(&LoopbackPlan::Direct, true),
        PortRoute::Direct,
        "this machine's ports open with no help"
    );
    assert_eq!(
        port_route_of(&LoopbackPlan::Direct, false),
        PortRoute::Blocked,
        "another machine's port with forwarding turned off is not ours to \
         open — opening it here would reach some unrelated local service"
    );
}

/// The SSH user a paste would be uploaded for, or `None` when the pane
/// keeps the local-path behavior.
fn remote_paste_user<'a>(
    ssh_spec: Option<&'a anterm_core::daemon::protocol::NativeSshSpec>,
) -> Option<&'a str> {
    remote_paste_spec(ssh_spec).map(|s| s.user.as_str())
}

fn native_spec() -> anterm_core::daemon::protocol::NativeSshSpec {
    serde_json::from_str(r#"{"host":"dev.box","port":22,"user":"me","auth_mode":"auto"}"#)
        .unwrap()
}

#[test]
fn local_pane_pastes_the_local_image_path() {
    assert_eq!(remote_paste_user(None), None);
}

#[test]
fn standalone_ssh_panes_upload_images_for_the_ssh_user() {
    let spec = native_spec();
    assert_eq!(remote_paste_user(Some(&spec)), Some("me"));
}

fn wsl_context(distro: &str) -> anterm_core::daemon::protocol::RemoteContext {
    anterm_core::daemon::protocol::RemoteContext {
        kind: RemoteKind::Wsl,
        argv: Vec::new(),
        target: distro.to_string(),
    }
}

#[test]
fn a_wsl_exe_pane_on_this_machine_completes_over_its_own_share() {
    assert_eq!(
        wsl_share_distro(Some(&wsl_context("Ubuntu-24.04")), true),
        Some("Ubuntu-24.04".to_string())
    );
}

#[test]
fn a_remote_hosts_wsl_exe_pane_leaves_tab_to_the_shell() {
    assert_eq!(
        wsl_share_distro(Some(&wsl_context("Ubuntu-24.04")), false),
        None
    );
}

#[test]
fn panes_with_no_distro_of_their_own_have_no_share() {
    assert_eq!(wsl_share_distro(None, true), None);
}

#[test]
fn a_staging_dir_is_only_safe_when_it_is_a_private_directory_we_own() {
    use anterm_core::daemon::protocol::SftpEntryKind;
    // `chmod 0700` succeeded and the mode came back as asked: ours.
    assert!(staging_dir_is_safe(
        false,
        Some(SftpEntryKind::Dir),
        0o040700
    ));
    // A mode anyone else can enter is one anyone else can read pastes from.
    assert!(!staging_dir_is_safe(
        false,
        Some(SftpEntryKind::Dir),
        0o040755
    ));
    assert!(!staging_dir_is_safe(
        false,
        Some(SftpEntryKind::Dir),
        0o040701
    ));
    // Sticky/setgid bits mean someone else set the terms.
    assert!(!staging_dir_is_safe(
        false,
        Some(SftpEntryKind::Dir),
        0o041700
    ));
    // A symlink is judged by its target by `stat`, so refuse it outright.
    assert!(!staging_dir_is_safe(
        true,
        Some(SftpEntryKind::Dir),
        0o040700
    ));
    // A file (or a path that vanished) is not a staging dir.
    assert!(!staging_dir_is_safe(
        false,
        Some(SftpEntryKind::File),
        0o100700
    ));
    assert!(!staging_dir_is_safe(false, None, 0o040700));
}

#[test]
fn a_remote_pane_stages_its_clipboard_image_on_every_platform() {
    // The SYN path leans on the agent sharing a clipboard with the pane,
    // which a remote agent never does — it reads the clipboard of the host
    // it runs on, and this machine's screenshot is not in it.
    assert!(
        stages_clipboard_image(true),
        "a remote agent cannot see this machine's clipboard"
    );
    // Locally the platform decides: macOS hands the agent the clipboard
    // itself, which is higher fidelity than a staged file.
    assert_eq!(
        stages_clipboard_image(false),
        cfg!(not(target_os = "macos"))
    );
}

/// A pane restored by attaching to its id carries no copy of the spec that
/// dialled the host. The daemon still has one, and still refuses a write
/// the profile forbids — so a missing copy here is not a verdict.
#[test]
fn a_pane_without_its_own_spec_defers_to_the_daemons_verdict() {
    let mut spec: anterm_core::daemon::protocol::NativeSshSpec = serde_json::from_str(
        r#"{"host":"build-box","port":22,"user":"me","auth_mode":"auto"}"#,
    )
    .unwrap();
    assert!(!super::allows_remote_clipboard_write(Some(&spec)));
    spec.remote_clipboard_write = true;
    assert!(super::allows_remote_clipboard_write(Some(&spec)));
    assert!(super::allows_remote_clipboard_write(None));
}

#[test]
fn remote_clipboard_images_must_match_their_declared_mime() {
    let pixel = image::RgbaImage::from_pixel(1, 1, image::Rgba([4, 5, 6, 255]));
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(pixel)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();

    let valid = anterm_core::clipboard::ClipboardWrite {
        mime: "image/png".into(),
        data: png.clone(),
        id: None,
    };
    assert!(super::validate_remote_clipboard_image(valid).is_ok());

    let mismatched = anterm_core::clipboard::ClipboardWrite {
        mime: "image/jpeg".into(),
        data: png,
        id: None,
    };
    assert!(super::validate_remote_clipboard_image(mismatched).is_err());
}

#[test]
fn a_wsl_pane_gets_the_automount_path_not_the_windows_one() {
    // The staged file really is on the pane's own disk — only its name
    // differs — so this is a rewrite, not an upload.
    assert_eq!(
        local_path_for_pane(
            r"C:\Users\me\AppData\Local\Temp\anterm-clipboard\paste-1.png",
            true
        ),
        "/mnt/c/Users/me/AppData/Local/Temp/anterm-clipboard/paste-1.png"
    );
    assert_eq!(wsl_path(r"D:\x\y.png").as_deref(), Some("/mnt/d/x/y.png"));

    // No automount mapping: the Windows path at least says where it went.
    let unc = r"\\server\share\paste-1.png";
    assert_eq!(wsl_path(unc), None);
    assert_eq!(local_path_for_pane(unc, true), unc);
    // Drive-relative, not absolute — `C:x` means "x under C:'s cwd".
    assert_eq!(wsl_path(r"C:paste-1.png"), None);

    // Every other pane keeps the path exactly as staged.
    assert_eq!(
        local_path_for_pane("/tmp/anterm-clipboard/paste-1.png", false),
        "/tmp/anterm-clipboard/paste-1.png"
    );
    assert_eq!(
        local_path_for_pane(r"C:\Temp\paste-1.png", false),
        r"C:\Temp\paste-1.png"
    );
}

#[test]
fn a_wsl_cwd_gets_a_windows_spelling_the_completion_engine_can_list() {
    let share = |posix: &str| wsl_share_path("Ubuntu-24.04", posix);

    // A distro-native path goes through the share.
    assert_eq!(
        share("/home/me/repo"),
        Some(PathBuf::from(r"\\wsl$\Ubuntu-24.04\home\me\repo"))
    );
    assert_eq!(share("/"), Some(PathBuf::from(r"\\wsl$\Ubuntu-24.04\")));

    // The automount stays on the share too: a drive-spelled cwd would
    // send an absolute word (`ls /etc<Tab>`) to `C:\etc` instead of the
    // distro's /etc, because a rooted word completes against its cwd's
    // path prefix.
    assert_eq!(
        share("/mnt/c/Users/me"),
        Some(PathBuf::from(r"\\wsl$\Ubuntu-24.04\mnt\c\Users\me"))
    );

    // No absolute POSIX path, no translation — and a distro name that
    // could break out of the share is refused outright.
    assert_eq!(share("relative/path"), None);
    assert_eq!(wsl_share_path("", "/home/me"), None);
    assert_eq!(wsl_share_path(r"evil\distro", "/home/me"), None);
    assert_eq!(wsl_share_path("evil/distro", "/home/me"), None);
}

/// #896: the Files panel handed a WSL pane's `/home/me` to the local
/// `read_dir`, which on Windows is `C:\home\me`. The share is the only
/// spelling this machine can list — and drops into the tree land there too.
#[test]
fn a_wsl_panes_files_root_goes_through_its_distros_share() {
    assert_eq!(
        files_cwd(None, Some("Ubuntu"), Some(PathBuf::from("/home/me/repo"))),
        Some(PathBuf::from(r"\\wsl$\Ubuntu\home\me\repo"))
    );
    assert_eq!(
        files_cwd(None, Some("Ubuntu"), Some(PathBuf::from("/mnt/c/Users/me"))),
        Some(PathBuf::from(r"\\wsl$\Ubuntu\mnt\c\Users\me"))
    );
}

/// A cwd the host resolves itself — a local shell, a workspace pane on its
/// own server, a WSL workspace's distro daemon — is never rewritten.
#[test]
fn a_cwd_on_the_panes_own_host_roots_the_tree_as_is() {
    let native = Some(PathBuf::from("/home/me/repo"));
    assert_eq!(files_cwd(native.clone(), None, None), native);
    assert_eq!(
        files_cwd(
            native.clone(),
            Some("Ubuntu"),
            Some(PathBuf::from("/elsewhere"))
        ),
        native
    );
}

/// wsl.exe's own cwd on the Windows side is already a local path.
#[test]
fn a_wsl_pane_with_a_windows_cwd_keeps_it() {
    assert_eq!(
        files_cwd(None, Some("Ubuntu"), Some(PathBuf::from(r"C:\Users\me"))),
        Some(PathBuf::from(r"C:\Users\me"))
    );
}

/// No host spelling and no local distro — a shell ssh'd onward, or a WSL
/// pane whose distro could not be named — roots nothing rather than a
/// path the tree could only fail to read.
#[test]
fn a_cwd_no_host_can_read_roots_nothing() {
    assert_eq!(files_cwd(None, None, Some(PathBuf::from("/home/me"))), None);
    assert_eq!(
        files_cwd(None, Some(""), Some(PathBuf::from("/home/me"))),
        None
    );
    assert_eq!(files_cwd(None, Some("Ubuntu"), None), None);
}

#[test]
fn a_failed_staging_preparation_is_retried_rather_than_latched() {
    assert_eq!(
        staging_cache(&Ok("/home/me/.cache/anterm/clipboard".to_string())),
        Some("/home/me/.cache/anterm/clipboard".to_string())
    );
    // Nothing was created, so the next paste must try again instead of
    // handing out a path under a directory that does not exist.
    assert_eq!(staging_cache(&Err("link is down".to_string())), None);
}

#[test]
fn staged_images_land_under_the_remote_users_own_home() {
    let mut dir = "/home/me".to_string();
    for component in super::REMOTE_CLIPBOARD_PATH {
        dir = anterm_core::daemon::ssh::sftp::remote_join(&dir, component);
    }
    assert_eq!(dir, "/home/me/.cache/anterm/clipboard");
    assert!(
        !dir.starts_with("/tmp"),
        "a world-writable staging dir is exactly what this avoids"
    );
    assert_eq!(super::REMOTE_CLIPBOARD_MODE, 0o700);
}

#[test]
fn the_pasted_image_name_stands_alone_as_a_remote_path_component() {
    use anterm_core::daemon::ssh::sftp::safe_local_name;
    use gpui::{Image, ImageFormat};

    let pixel = image::RgbaImage::from_pixel(1, 1, image::Rgba([4, 5, 6, 255]));
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(pixel)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let path = super::write_clipboard_image(&Image::from_bytes(ImageFormat::Png, png)).unwrap();
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    assert!(safe_local_name(&name), "{name} must not traverse or nest");
}

#[test]
fn the_off_switch_disables_every_route() {
    assert_eq!(
        loopback_plan(false, Some(RemoteKind::NativeSsh), 7),
        LoopbackPlan::Direct
    );
}

#[test]
fn file_command_template_substitutes_path_line_and_column() {
    let argv = expand_file_command_template(
        "herdr edit {path} --line={line} --column={column}",
        Path::new("/tmp/foo.rs"),
        Some(42),
        Some(7),
    );
    assert_eq!(
        argv,
        vec!["herdr", "edit", "/tmp/foo.rs", "--line=42", "--column=7",]
    );
}

#[test]
fn file_command_template_drops_tokens_for_absent_values() {
    let argv = expand_file_command_template(
        "herdr edit {path} --line={line} --column={column}",
        Path::new("/tmp/foo.rs"),
        None,
        None,
    );
    assert_eq!(argv, vec!["herdr", "edit", "/tmp/foo.rs"]);

    let argv = expand_file_command_template(
        "herdr edit {path} --line={line} --column={column}",
        Path::new("/tmp/foo.rs"),
        Some(42),
        None,
    );
    assert_eq!(argv, vec!["herdr", "edit", "/tmp/foo.rs", "--line=42"]);
}

#[test]
fn file_command_template_keeps_path_only_token_and_unknown_placeholder() {
    let argv = expand_file_command_template(
        "code --goto {path}:{line} {other}",
        Path::new("/tmp/foo.rs"),
        None,
        None,
    );
    assert_eq!(argv, vec!["code", "--goto", "{other}"]);
}

/// #542's contract: a failed open is an `Err` the click site can toast,
/// not a line in a logfile nobody is watching.
#[test]
fn a_file_command_that_cannot_spawn_comes_back_as_an_error() {
    let path = Path::new("/tmp/wherever.rs");
    // The binary does not exist, so the spawn itself fails.
    assert!(super::run_file_command("anterm-no-such-binary {path}", path, None, None).is_err());
    // A template whose tokens all expand to nothing is a config error,
    // not a silent no-op.
    assert!(super::run_file_command("{line}", path, None, None).is_err());
}

#[test]
fn clipboard_image_transcodes_bmp_to_png_and_passes_png_through() {
    use gpui::{Image, ImageFormat};

    let pixel = image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255]));
    let mut bmp = Vec::new();
    image::DynamicImage::ImageRgba8(pixel)
        .write_to(&mut std::io::Cursor::new(&mut bmp), image::ImageFormat::Bmp)
        .unwrap();
    let path = super::write_clipboard_image(&Image::from_bytes(ImageFormat::Bmp, bmp)).unwrap();
    assert_eq!(path.extension().unwrap(), "png");
    assert_eq!(&std::fs::read(&path).unwrap()[..8], b"\x89PNG\r\n\x1a\n");

    let png = std::fs::read(&path).unwrap();
    let out = super::write_clipboard_image(&Image::from_bytes(ImageFormat::Png, png.clone()))
        .unwrap();
    assert_eq!(out.extension().unwrap(), "png");
    assert_eq!(std::fs::read(&out).unwrap(), png);
}

/// A macOS screenshot reaches the pasteboard as TIFF, which agent vision
/// rejects the same way it rejects a Windows BMP. The transcode depends on
/// `image`'s TIFF decoder being compiled in — without it this fails at
/// runtime by quietly staging nothing, which reads as "paste did nothing".
#[test]
fn a_macos_tiff_screenshot_is_staged_as_png() {
    use gpui::{Image, ImageFormat};

    let pixel = image::RgbaImage::from_pixel(1, 1, image::Rgba([4, 5, 6, 255]));
    let mut tiff = Vec::new();
    image::DynamicImage::ImageRgba8(pixel)
        .write_to(
            &mut std::io::Cursor::new(&mut tiff),
            image::ImageFormat::Tiff,
        )
        .unwrap();
    let path =
        super::write_clipboard_image(&Image::from_bytes(ImageFormat::Tiff, tiff)).unwrap();
    assert_eq!(path.extension().unwrap(), "png");
    assert_eq!(&std::fs::read(&path).unwrap()[..8], b"\x89PNG\r\n\x1a\n");
}

#[test]
fn fallback_chain_pins_bundled_hack_last() {
    let configured = vec!["Menlo".to_string(), "Apple Color Emoji".to_string()];

    let chain = fallback_chain("JetBrains Mono", &configured);
    assert_eq!(chain[..2], ["Menlo", "Apple Color Emoji"]);
    assert_eq!(chain.last().unwrap(), "Hack");

    let chain = fallback_chain("Hack", &configured);
    assert_eq!(chain[..2], ["Menlo", "Apple Color Emoji"]);
    assert!(!chain.iter().any(|f| f == "Hack"));

    let with_hack = vec!["Hack".to_string(), "Menlo".to_string()];
    let chain = fallback_chain("SF Mono", &with_hack);
    assert_eq!(chain[..2], ["Hack", "Menlo"]);

    assert_eq!(
        fallback_chain("Hack Nerd Font", &[]).last().unwrap(),
        "Hack",
        "a Hack-prefixed family name must not suppress the bundled anchor"
    );
}

#[test]
fn apply_fallback_chain_reaches_every_face_a_view_has() {
    // Bold and italic are the ones at risk: they hold a copy taken off the
    // regular face when `alt_font` built them, so a rebuild that wrote only
    // the regular face would strand them on the chain it replaced.
    let mut font = gpui::font("Hack");
    let mut bold = Some(gpui::font("Hack Bold"));
    let mut italic = Some(gpui::font("Hack Italic"));

    super::apply_fallback_chain(vec!["Menlo".to_string()], &mut font, &mut bold, &mut italic);

    for face in [&font, bold.as_ref().unwrap(), italic.as_ref().unwrap()] {
        assert_eq!(face.fallbacks.as_ref().unwrap().fallback_list(), ["Menlo"]);
    }

    // Neither is configured by default, and a view carries `None` for one
    // it was never given.
    let (mut none_bold, mut none_italic) = (None, None);
    super::apply_fallback_chain(
        vec!["Menlo".to_string()],
        &mut font,
        &mut none_bold,
        &mut none_italic,
    );
    assert_eq!(font.fallbacks.unwrap().fallback_list(), ["Menlo"]);
}

#[test]
fn fallback_chain_appends_platform_stock_faces() {
    let stock = anterm_ui_kit::config::platform_last_resort_fallbacks();
    assert!(!stock.is_empty(), "every platform needs a CJK last resort");

    let legacy = vec![
        "Menlo".to_string(),
        "Hasklug Nerd Font Mono".to_string(),
        "Maple Mono NF CN".to_string(),
        "Apple Color Emoji".to_string(),
    ];
    let chain = fallback_chain("Hack", &legacy);
    for name in stock {
        assert!(
            chain.iter().any(|f| f == name),
            "{name} missing from repaired chain {chain:?}"
        );
    }

    assert_eq!(chain[..legacy.len()], legacy[..]);

    let explicit = vec![stock[0].to_string()];
    let chain = fallback_chain("Hack", &explicit);
    assert_eq!(
        chain.iter().filter(|f| *f == stock[0]).count(),
        1,
        "stock face duplicated in {chain:?}"
    );

    assert!(!fallback_chain(stock[0], &[]).iter().any(|f| f == stock[0]));
}

#[test]
fn wheel_routes_by_negotiated_mode_with_reporting_first() {
    let mouse = TermMode::MOUSE_REPORT_CLICK;
    assert_eq!(
        wheel_route(mouse, false, true),
        WheelRoute::Report { base: 64 }
    );
    assert_eq!(
        wheel_route(mouse, false, false),
        WheelRoute::Report { base: 65 }
    );

    let alt = TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL;
    assert_eq!(
        wheel_route(alt, false, true),
        WheelRoute::Arrows { seq: b"\x1b[A" }
    );
    assert_eq!(
        wheel_route(alt, false, false),
        WheelRoute::Arrows { seq: b"\x1b[B" }
    );
    assert_eq!(
        wheel_route(alt | TermMode::APP_CURSOR, false, true),
        WheelRoute::Arrows { seq: b"\x1bOA" }
    );
    assert_eq!(
        wheel_route(alt | TermMode::APP_CURSOR, false, false),
        WheelRoute::Arrows { seq: b"\x1bOB" }
    );

    assert_eq!(
        wheel_route(mouse | alt, false, true),
        WheelRoute::Report { base: 64 }
    );

    assert_eq!(
        wheel_route(TermMode::empty(), false, true),
        WheelRoute::Scrollback
    );
}

#[test]
fn wheel_ignores_alternate_scroll_outside_the_alt_screen() {
    assert_eq!(
        wheel_route(TermMode::ALTERNATE_SCROLL, false, true),
        WheelRoute::Scrollback
    );
}

#[test]
fn shift_wheel_always_scrolls_the_local_scrollback() {
    let everything = TermMode::MOUSE_MOTION
        | TermMode::ALT_SCREEN
        | TermMode::ALTERNATE_SCROLL
        | TermMode::APP_CURSOR;
    assert_eq!(wheel_route(everything, true, true), WheelRoute::Scrollback);
    assert_eq!(wheel_route(everything, true, false), WheelRoute::Scrollback);
}

#[test]
fn right_click_opens_the_host_menu_when_the_app_is_not_reporting_mouse() {
    assert!(should_show_context_menu(false, false));
    assert!(should_show_context_menu(false, true));
}

#[test]
fn right_click_belongs_to_the_app_while_mouse_reporting_is_active() {
    assert!(
        !should_show_context_menu(true, false),
        "an unmodified right-click in mouse mode is the application's event, \
         so anterm must not also pop its own menu over it"
    );
}

#[test]
fn shift_right_click_stays_the_escape_hatch_in_mouse_mode() {
    assert!(
        should_show_context_menu(true, true),
        "Shift is the documented way to reach anterm's own menu inside a \
         mouse-reporting TUI"
    );
}

#[test]
fn context_menu_and_mouse_report_never_fire_for_the_same_click() {
    // The render-time menu gate and the mouse-down forwarder in
    // `TerminalElement` read the same predicate from opposite sides, so
    // every (mouse_mode, shift) pair must land in exactly one of them.
    for mouse_mode in [false, true] {
        for shift in [false, true] {
            let menu = should_show_context_menu(mouse_mode, shift);
            let reported = mouse_mode && !shift;
            assert_ne!(
                menu, reported,
                "mouse_mode={mouse_mode} shift={shift} must route to exactly one consumer"
            );
        }
    }
}

#[test]
fn copy_on_select_copies_the_buffer_the_gesture_touched() {
    assert_eq!(select_end_copy(false, true, false), SelectEndCopy::None);
    assert_eq!(select_end_copy(false, false, true), SelectEndCopy::None);

    assert_eq!(select_end_copy(true, true, false), SelectEndCopy::Grid);
    assert_eq!(select_end_copy(true, false, true), SelectEndCopy::Editor);

    assert_eq!(select_end_copy(true, false, false), SelectEndCopy::None);

    assert_eq!(select_end_copy(true, true, true), SelectEndCopy::Grid);
}

#[test]
fn sgr_mouse_reports_one_based_decimal_with_modifier_bits() {
    let plain = Modifiers::default();
    assert_eq!(
        encode_mouse(true, 0, &plain, 4, 8, true).unwrap(),
        b"\x1b[<0;5;9M".to_vec()
    );
    assert_eq!(
        encode_mouse(true, 2, &plain, 4, 8, false).unwrap(),
        b"\x1b[<2;5;9m".to_vec()
    );
    let all = Modifiers {
        shift: true,
        alt: true,
        control: true,
        ..Modifiers::default()
    };
    assert_eq!(
        encode_mouse(true, 0, &all, 0, 0, true).unwrap(),
        b"\x1b[<28;1;1M".to_vec()
    );
    assert_eq!(
        encode_mouse(true, 64, &plain, 10, 3, true).unwrap(),
        b"\x1b[<64;11;4M".to_vec()
    );
    assert_eq!(
        encode_mouse(true, 35, &plain, 1, 1, true).unwrap(),
        b"\x1b[<35;2;2M".to_vec()
    );
}

#[test]
fn sgr_mouse_has_no_coordinate_cap() {
    let plain = Modifiers::default();
    assert_eq!(
        encode_mouse(true, 0, &plain, 500, 300, true).unwrap(),
        b"\x1b[<0;501;301M".to_vec()
    );
}

#[test]
fn x10_mouse_packs_bytes_and_drops_button_on_release() {
    let plain = Modifiers::default();
    assert_eq!(
        encode_mouse(false, 0, &plain, 4, 8, true).unwrap(),
        vec![0x1b, b'[', b'M', 32, 32 + 1 + 4, 32 + 1 + 8]
    );
    assert_eq!(
        encode_mouse(false, 2, &plain, 4, 8, false).unwrap(),
        vec![0x1b, b'[', b'M', 32 + 3, 32 + 1 + 4, 32 + 1 + 8]
    );
    let ctrl = Modifiers {
        control: true,
        ..Modifiers::default()
    };
    assert_eq!(
        encode_mouse(false, 1, &ctrl, 0, 0, true).unwrap(),
        vec![0x1b, b'[', b'M', 32 + 1 + 16, 33, 33]
    );
}

#[test]
fn x10_mouse_drops_out_of_range_coordinates_whole() {
    let plain = Modifiers::default();
    assert!(encode_mouse(false, 0, &plain, 223, 0, true).is_none());
    assert!(encode_mouse(false, 0, &plain, 0, 223, true).is_none());
    let last = encode_mouse(false, 0, &plain, 222, 222, true).unwrap();
    assert_eq!(&last[4..], &[255, 255]);
}

#[test]
fn fig_icon_emoji_takes_bare_emoji_and_template_badge_only() {
    assert_eq!(fig_icon_emoji("⚙️"), Some("⚙️"));
    assert_eq!(
        fig_icon_emoji("fig://template?color=2ecc71&badge=🔥"),
        Some("🔥")
    );
    assert_eq!(fig_icon_emoji("fig://icon?type=git"), None);
    assert_eq!(fig_icon_emoji("fig://template?color=2ecc71"), None);
    assert_eq!(fig_icon_emoji(""), None);
}

#[test]
fn fig_icon_glyph_maps_known_types_and_falls_back_otherwise() {
    assert!(matches!(
        fig_icon_glyph("fig://icon?type=folder"),
        Some(IconName::Folder)
    ));
    assert!(matches!(
        fig_icon_glyph("fig://icon?type=file"),
        Some(IconName::File)
    ));
    assert!(matches!(
        fig_icon_glyph("fig://icon?type=git"),
        Some(IconName::Github)
    ));
    assert!(fig_icon_glyph("fig://icon?type=docker").is_none());
    assert!(fig_icon_glyph("⚙️").is_none());
}

#[test]
fn focus_reports_only_when_the_app_opted_in() {
    assert_eq!(focus_report_bytes(TermMode::empty(), true), None);
    assert_eq!(focus_report_bytes(TermMode::empty(), false), None);
    let mode = TermMode::FOCUS_IN_OUT;
    assert_eq!(focus_report_bytes(mode, true), Some(b"\x1b[I".as_slice()));
    assert_eq!(focus_report_bytes(mode, false), Some(b"\x1b[O".as_slice()));
    assert_eq!(focus_report_bytes(TermMode::MOUSE_MOTION, true), None);
}

#[test]
fn smooth_scroll_step_accumulates_and_clamps() {
    assert_eq!(smooth_scroll_step(0, 0.0, 0.4, 100), (0, 0.4));
    let (jump, frac) = smooth_scroll_step(0, 0.4, 0.8, 100);
    assert_eq!(jump, 1);
    assert!((frac - 0.2).abs() < 1e-4);
    let (jump, frac) = smooth_scroll_step(5, 0.2, -0.5, 100);
    assert_eq!(jump, -1);
    assert!((frac - 0.7).abs() < 1e-4);
    assert_eq!(smooth_scroll_step(3, 0.5, -10.0, 100), (-3, 0.0));
    assert_eq!(smooth_scroll_step(98, 0.0, 7.3, 100), (2, 0.0));
    assert_eq!(smooth_scroll_step(0, 0.0, 2.5, 0), (0, 0.0));
}

#[test]
fn scroll_anim_step_converges_and_lands() {
    let frame = SCROLL_ANIM_FRAME;
    // A notch is spread over frames instead of being applied whole.
    let (step, last) = scroll_anim_step(3.0, frame);
    assert!(!last);
    assert!(
        step > 0. && step < 3.0,
        "took {step} of 3 lines in one frame"
    );

    // And it converges: no notch is left hanging.
    let mut remaining = 3.0_f32;
    let mut frames = 0u32;
    loop {
        let (step, last) = scroll_anim_step(remaining, frame);
        remaining -= step;
        frames += 1;
        if last {
            break;
        }
        assert!(frames < 200, "still {remaining} lines short after {frames}");
    }
    assert!(remaining.abs() < 1e-4, "landed {remaining} lines off");
    // Slow enough to read as motion, fast enough not to feel like lag.
    let ms = frames * SCROLL_ANIM_FRAME.as_millis() as u32;
    assert!((60..=200).contains(&ms), "a 3-line notch took {ms}ms");
}

#[test]
fn scroll_anim_step_covers_dropped_frames() {
    // A late tick has to make up the ground it missed, or a busy pane would
    // scroll slower than an idle one.
    let (one, _) = scroll_anim_step(10.0, SCROLL_ANIM_FRAME);
    let (four, _) = scroll_anim_step(10.0, SCROLL_ANIM_FRAME * 4);
    assert!(four > one * 2., "{four} should far outpace {one}");
    // But the catch-up is capped, so a tick after a long stall never
    // overshoots what was actually asked for.
    let (stalled, _) = scroll_anim_step(10.0, std::time::Duration::from_secs(5));
    assert!(
        stalled > four && stalled <= 10.0,
        "stalled tick took {stalled}"
    );
}

#[test]
fn scroll_anim_step_lands_on_a_negligible_remainder() {
    let (step, last) = scroll_anim_step(-0.005, SCROLL_ANIM_FRAME);
    assert!(last);
    assert_eq!(step, -0.005);
}

#[test]
fn drag_scroll_step_scales_with_overshoot_and_caps() {
    assert_eq!(drag_scroll_step(0.2), 1);
    assert_eq!(drag_scroll_step(-0.2), -1);
    assert_eq!(drag_scroll_step(3.5), 4);
    assert_eq!(drag_scroll_step(-3.5), -4);
    assert_eq!(drag_scroll_step(50.0), 8);
    assert_eq!(drag_scroll_step(-50.0), -8);
}

#[test]
fn trim_trailing_spaces_strips_per_line_and_preserves_structure() {
    assert_eq!(trim_trailing_spaces("a  \nb\t\nc"), "a\nb\nc");
    assert_eq!(trim_trailing_spaces("a  \n"), "a\n");
    assert_eq!(trim_trailing_spaces("a  "), "a");
    assert_eq!(trim_trailing_spaces("  a  "), "  a");
}

#[test]
fn paste_bytes_strips_esc_to_prevent_bracketed_paste_escape() {
    assert_eq!(
        paste_bytes("ls -la", true),
        b"\x1b[200~ls -la\x1b[201~".to_vec()
    );

    let evil = "foo\x1b[201~\nrm -rf ~\n";
    let out = paste_bytes(evil, true);
    let end = b"\x1b[201~";
    let markers = out.windows(end.len()).filter(|w| *w == end).count();
    assert_eq!(markers, 1);
    let inner = &out[b"\x1b[200~".len()..out.len() - end.len()];
    assert!(!inner.contains(&0x1b));
    assert_eq!(inner, b"foo[201~\nrm -rf ~\n");

    assert_eq!(paste_bytes("a\x1b[201~b", false), b"a\x1b[201~b".to_vec());
}

#[test]
fn paste_bytes_normalizes_newlines_to_cr_without_bracketed_paste() {
    assert_eq!(paste_bytes("a\nb\r\nc\n", false), b"a\rb\rc\r".to_vec());
    assert_eq!(
        paste_bytes("a\nb", true),
        b"\x1b[200~a\nb\x1b[201~".to_vec()
    );
}

#[test]
fn paste_bytes_folds_crlf_so_a_windows_clipboard_pastes_like_any_other() {
    assert_eq!(
        paste_bytes("a\r\nb\r\n", true),
        b"\x1b[200~a\nb\n\x1b[201~".to_vec(),
        "CRLF must reach the app as one line break, not two"
    );
    assert_eq!(
        paste_bytes("a\r\nb", true),
        paste_bytes("a\nb", true),
        "a Windows clipboard must paste exactly like a Unix one"
    );
}

#[test]
fn submit_bytes_sends_a_multi_line_command_as_one_bracketed_paste() {
    assert_eq!(
        submit_bytes("echo a\necho b\necho c", true, false),
        b"\x1b[200~echo a\necho b\necho c\x1b[201~\r".to_vec()
    );
    let out = submit_bytes("a\nb\nc\nd", true, false);
    assert_eq!(out.iter().filter(|&&b| b == b'\r').count(), 1);
}

#[test]
fn submit_bytes_types_a_plain_single_line_instead_of_pasting_it() {
    // #660: inside a bracketed paste fish never runs `expand-abbr`, so an
    // abbreviation with arguments reached the shell verbatim and `j build`
    // died as "command not found". Raw bytes are what typing produces, and
    // that is the delivery every input-time expansion -- fish
    // abbreviations, zsh magic-space -- is bound to.
    assert_eq!(submit_bytes("j build", true, false), b"j build\r".to_vec());
    assert_eq!(
        submit_bytes("echo 'a  b' | cat", true, false),
        b"echo 'a  b' | cat\r".to_vec()
    );
    // Non-ASCII is text, not a control character.
    assert_eq!(
        submit_bytes("echo 中文", true, false),
        "echo 中文\r".as_bytes()
    );

    // A control character would be acted on by the shell's binding table
    // rather than inserted -- a Tab completes, a ^U kills the line -- so
    // those keep the paste framing.
    assert_eq!(
        submit_bytes("echo a\tb", true, false),
        b"\x1b[200~echo a\tb\x1b[201~\r".to_vec()
    );
    assert_eq!(
        submit_bytes("echo a\x15b", true, false),
        b"\x1b[200~echo a\x15b\x1b[201~\r".to_vec()
    );

    // Past the length bound the flat cost of a paste wins over the shell's
    // per-byte redraw. The bound is a latency ceiling, not a knee in the
    // curve -- see `types_cleanly`.
    let at_bound = format!("echo {}", "y".repeat(507));
    assert_eq!(at_bound.len(), 512);
    assert_eq!(
        submit_bytes(&at_bound, true, false),
        [at_bound.as_bytes(), b"\r"].concat()
    );
    let over_bound = format!("echo {}", "y".repeat(508));
    assert_eq!(over_bound.len(), 513);
    assert_eq!(
        submit_bytes(&over_bound, true, false),
        [b"\x1b[200~", over_bound.as_bytes(), b"\x1b[201~\r"].concat()
    );
}

#[test]
fn submit_bytes_keeps_pasted_text_inside_a_bracketed_paste() {
    // A paste's contract is that what went in is what runs. The typed path
    // hands the line to the shell's binding table, and a printable key can
    // be bound there: a fish user with `abbr -a l 'ls -la'` who pastes
    // `l /tmp` out of their notes must not run `ls -la /tmp`, and a zsh
    // user with `bindkey . rationalise-dot` must not have a pasted
    // `echo a...b` become `echo a../..b`. Both were reproduced on a pty.
    assert_eq!(
        submit_bytes("l /tmp", true, true),
        b"\x1b[200~l /tmp\x1b[201~\r".to_vec()
    );
    assert_eq!(
        submit_bytes("echo a...b", true, true),
        b"\x1b[200~echo a...b\x1b[201~\r".to_vec()
    );
    // Same text typed rather than pasted takes the typed path -- that is
    // the whole point, and it is the same delivery the user's own shell
    // prompt would have given it.
    assert_eq!(submit_bytes("l /tmp", true, false), b"l /tmp\r".to_vec());
    // A shell with no bracketed paste has no framing to fall back on, so
    // the mark changes nothing there.
    assert_eq!(submit_bytes("l /tmp", false, true), b"l /tmp\r".to_vec());
    // An empty buffer still skips the markers, pasted or not.
    assert_eq!(submit_bytes("", true, true), b"\r".to_vec());
}

#[test]
fn submit_bytes_falls_back_to_per_line_cr_without_bracketed_paste() {
    assert_eq!(submit_bytes("a\nb", false, false), b"a\rb\r".to_vec());
    assert_eq!(submit_bytes("a\r\nb", false, false), b"a\rb\r".to_vec());
}

#[test]
fn submit_bytes_normalizes_line_breaks_inside_the_paste() {
    assert_eq!(
        submit_bytes("a\r\nb", true, false),
        b"\x1b[200~a\nb\x1b[201~\r".to_vec()
    );
    assert_eq!(
        submit_bytes("a\rb", true, false),
        b"\x1b[200~a\nb\x1b[201~\r".to_vec()
    );
    assert_eq!(submit_bytes("a\rb", false, false), b"a\rb\r".to_vec());
}

#[test]
fn submit_bytes_strips_esc_and_skips_markers_on_an_empty_line() {
    let out = submit_bytes("foo\x1b[201~\nrm -rf ~", true, false);
    let end = b"\x1b[201~";
    assert_eq!(out.windows(end.len()).filter(|w| *w == end).count(), 1);
    assert_eq!(out, b"\x1b[200~foo[201~\nrm -rf ~\x1b[201~\r".to_vec());
    assert_eq!(submit_bytes("a\x1bb", false, false), b"ab\r".to_vec());
    // The same smuggling attempt on the typed path is just literal text at
    // the prompt: there is no paste to break out of, and no ESC survives to
    // reach the line editor as a command.
    assert_eq!(
        submit_bytes("foo\x1b[201~; rm -rf ~", true, false),
        b"foo[201~; rm -rf ~\r".to_vec()
    );

    assert_eq!(submit_bytes("", true, false), b"\r".to_vec());
}

#[test]
fn a_copied_file_is_pasted_by_path_quoted_and_space_joined() {
    let item = ClipboardItem {
        entries: vec![ClipboardEntry::ExternalPaths(ExternalPaths(
            vec![
                PathBuf::from("/Users/me/My File.txt"),
                PathBuf::from("/tmp/b.log"),
            ]
            .into(),
        ))],
    };
    let paths = clipboard_paths(&item);
    assert_eq!(
        paths,
        [
            PathBuf::from("/Users/me/My File.txt"),
            PathBuf::from("/tmp/b.log")
        ]
    );
    let spelled: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        super::pasted_paths_text(&spelled, Some("zsh")),
        "'/Users/me/My File.txt' /tmp/b.log "
    );

    // Text is text: nothing on the clipboard names a file.
    let text = ClipboardItem::new_string("echo hi".to_string());
    assert!(clipboard_paths(&text).is_empty());
}

/// Two pastes of `report.xlsx` in the same instant must not land in the
/// same directory, or the second overwrites the file the first pasted.
#[test]
fn every_paste_stages_into_a_directory_of_its_own() {
    let a = super::paste_dir_name();
    let b = super::paste_dir_name();
    assert_ne!(a, b);
    assert!(anterm_core::daemon::ssh::sftp::safe_local_name(&a));
}

#[test]
fn staged_pastes_expire_after_a_week_and_never_on_an_unknown_age() {
    let now = 1_800_000_000;
    let week = super::REMOTE_CLIPBOARD_TTL_SECS;
    assert!(!super::staged_entry_expired(now - 60, now));
    assert!(!super::staged_entry_expired(now - week, now));
    assert!(super::staged_entry_expired(now - week - 1, now));
    // No mtime from the server says nothing about age.
    assert!(!super::staged_entry_expired(0, now));
    // A remote clock ahead of ours is young, not negative.
    assert!(!super::staged_entry_expired(now + 3600, now));
}

#[test]
fn display_width_ascii_and_control_are_narrow() {
    assert_eq!(display_width('a'), 1);
    assert_eq!(display_width(' '), 1);
    assert_eq!(display_width('~'), 1);
    assert_eq!(display_width('\t'), 1);
}

#[test]
fn display_width_cjk_and_kana_are_wide() {
    assert_eq!(display_width('你'), 2);
    assert_eq!(display_width('한'), 2);
    assert_eq!(display_width('あ'), 2);
    assert_eq!(display_width('　'), 2);
}

#[test]
fn display_width_emoji_are_wide() {
    assert_eq!(display_width('🚀'), 2);
    assert_eq!(display_width('🎉'), 2);
}

#[test]
fn display_width_latin_accents_stay_narrow() {
    assert_eq!(display_width('é'), 1);
    assert_eq!(display_width('©'), 1);
    assert_eq!(display_width('±'), 1);
}

/// Wide characters the grid reserves two columns for, scattered outside the
/// ranges anyone would think to hand-write: mahjong and playing cards below
/// the Miscellaneous Symbols and Pictographs block, and the handful of Wide
/// code points stranded in Misc Technical and Dingbats.
#[test]
fn display_width_covers_wide_chars_outside_the_main_emoji_blocks() {
    assert_eq!(display_width('🀄'), 2);
    assert_eq!(display_width('🃏'), 2);
    assert_eq!(display_width('⌚'), 2);
    assert_eq!(display_width('⏰'), 2);
    assert_eq!(display_width('✅'), 2);
    assert_eq!(display_width('❌'), 2);
}

/// Combining marks ride on the cell of the character they decorate. Giving
/// one a column of its own shifts the rest of the line and hands the mark
/// to the shaper alone, with no base to attach to.
#[test]
fn display_width_combining_marks_take_no_column() {
    // COMBINING ACUTE ACCENT — the second half of a decomposed `é`.
    assert_eq!(display_width('\u{0301}'), 0);
    // VARIATION SELECTOR-16, which asks for the emoji glyph.
    assert_eq!(display_width('\u{FE0F}'), 0);
    // ZERO WIDTH JOINER, the glue inside 👩‍💻.
    assert_eq!(display_width('\u{200D}'), 0);
}

/// The width table feeds the bar's own line breaking, so a character
/// counted short pulls everything after it one column left.
#[test]
fn input_char_positions_reserve_two_columns_for_wide_chars() {
    let chars: Vec<char> = "a🀄b".chars().collect();
    let (positions, _, _) = input_char_positions(&chars, (0, 0), 80);
    assert_eq!(positions, vec![(0, 0, 1), (0, 1, 2), (0, 3, 1)]);
}

/// Geometry and drawing read the same cells, so a sequence the bar draws
/// two columns wide is two columns wide to wrapping and clicks as well. Two
/// width tables would put `X` under the right half of the heart.
#[test]
fn input_char_positions_agree_with_the_cells_the_bar_draws() {
    for text in [
        "a🀄b",
        "e\u{0301}X",
        "\u{2764}\u{FE0F}X",
        "\u{0301}ab",
        "a\nb",
    ] {
        let chars: Vec<char> = text.chars().collect();
        let (positions, _, _) = input_char_positions(&chars, (0, 0), 80);
        assert_eq!(positions.len(), chars.len(), "{text:?}");
        for cell in input_cells(&chars) {
            let drawn = if chars[cell.start] == '\n' {
                0
            } else {
                cell.width
            };
            assert_eq!(positions[cell.start].2, drawn, "{text:?} at {}", cell.start);
            for i in cell.start + 1..cell.end {
                assert_eq!(positions[i].2, 0, "{text:?} at {i}");
            }
        }
    }
}

/// `❤️` is two columns in the bar, so the character after it starts at
/// column 2 — and a click on either half of it lands on the heart.
#[test]
fn input_char_positions_reserve_two_columns_for_an_emoji_presentation_sequence() {
    let chars: Vec<char> = "\u{2764}\u{FE0F}X".chars().collect();
    let (positions, _, _) = input_char_positions(&chars, (0, 0), 80);
    assert_eq!(positions, vec![(0, 0, 2), (0, 2, 0), (0, 2, 1)]);
    assert_eq!(click("\u{2764}\u{FE0F}X", 0, 80, 0, 0), Some(0));
    assert_eq!(click("\u{2764}\u{FE0F}X", 0, 80, 1, 0), Some(0));
    assert_eq!(click("\u{2764}\u{FE0F}X", 0, 80, 2, 0), Some(2));
}

/// A mark with no base gets a cell of its own on screen, so it has to get a
/// column here too — otherwise everything after it clicks one column off.
#[test]
fn input_char_positions_give_a_stranded_combining_mark_a_column() {
    let chars: Vec<char> = "\u{0301}ab".chars().collect();
    let (positions, _, _) = input_char_positions(&chars, (0, 0), 80);
    assert_eq!(positions, vec![(0, 0, 1), (0, 1, 1), (0, 2, 1)]);
}

/// A cell wraps whole. Splitting `❤️` across rows would draw its two
/// columns on one row and count them on two.
#[test]
fn input_char_positions_wrap_a_cell_without_splitting_it() {
    let chars: Vec<char> = "abc\u{2764}\u{FE0F}".chars().collect();
    let (positions, r, c) = input_char_positions(&chars, (0, 0), 4);
    assert_eq!(positions[3], (1, 0, 2));
    assert_eq!((r, c), (1, 2));
}

/// Clicking the right half of a wide character lands on that character, not
/// on the one after it.
#[test]
fn wrapped_click_index_hits_both_halves_of_a_wide_char() {
    assert_eq!(click("a🀄b", 0, 80, 1, 0), Some(1));
    assert_eq!(click("a🀄b", 0, 80, 2, 0), Some(1));
    assert_eq!(click("a🀄b", 0, 80, 3, 0), Some(2));
}

/// A combining mark shares its base's column, so a click there hits the
/// base — there is nowhere on screen that is the mark and not the base.
#[test]
fn wrapped_click_index_lands_on_the_base_not_its_combining_mark() {
    // e + COMBINING ACUTE ACCENT + X.
    assert_eq!(click("e\u{0301}X", 0, 80, 0, 0), Some(0));
    assert_eq!(click("e\u{0301}X", 0, 80, 1, 0), Some(2));
}

fn cells(text: &str) -> Vec<(usize, usize, String, usize)> {
    let chars: Vec<char> = text.chars().collect();
    input_cells(&chars)
        .into_iter()
        .map(|c| (c.start, c.end, c.text, c.width))
        .collect()
}

#[test]
fn input_cells_keep_a_combining_mark_with_its_base() {
    assert_eq!(
        cells("e\u{0301}X"),
        vec![
            (0, 2, "e\u{0301}".to_string(), 1),
            (2, 3, "X".to_string(), 1),
        ]
    );
}

/// An emoji presentation sequence is two columns wide even though its base
/// is one on its own — the same re-scoring the terminal grid does.
#[test]
fn input_cells_widen_an_emoji_presentation_sequence() {
    assert_eq!(
        cells("\u{2764}\u{FE0F}X"),
        vec![
            (0, 2, "\u{2764}\u{FE0F}".to_string(), 2),
            (2, 3, "X".to_string(), 1),
        ]
    );
    // U+FE0E asks for the text glyph, and stays one column.
    assert_eq!(
        cells("\u{2764}\u{FE0E}"),
        vec![(0, 2, "\u{2764}\u{FE0E}".to_string(), 1)]
    );
}

/// A ZWJ sequence stays two cells, because that is what the grid does with
/// it: the joiner rides on the first emoji and the second one still claims
/// its own two columns. Composing the pair into one glyph is a separate
/// problem (#209) and fixing it here would put the bar a column off from
/// the row the text lands on.
#[test]
fn input_cells_split_a_zwj_sequence_the_way_the_grid_does() {
    assert_eq!(
        cells("\u{1F469}\u{200D}\u{1F4BB}"),
        vec![
            (0, 2, "\u{1F469}\u{200D}".to_string(), 2),
            (2, 3, "\u{1F4BB}".to_string(), 2),
        ]
    );
}

/// A mark with no base ahead of it still needs a column, or it is invisible
/// and the caret has nowhere to sit. A newline is not a base to hang one on.
#[test]
fn input_cells_give_a_stranded_combining_mark_a_column() {
    assert_eq!(cells("\u{0301}"), vec![(0, 1, "\u{0301}".to_string(), 1)]);
    assert_eq!(
        cells("\n\u{0301}"),
        vec![(0, 1, String::new(), 0), (1, 2, "\u{0301}".to_string(), 1),]
    );
}

fn click(text: &str, scol: usize, cols: usize, col: usize, row: usize) -> Option<usize> {
    let chars: Vec<char> = text.chars().collect();
    wrapped_click_index(&chars, (0, scol), cols, col, row, false)
}

#[test]
fn before_input_covers_the_prompt_and_nothing_after_it() {
    // ` anterm git:(main) ` — the editor starts at column 17.
    assert!(before_input((0, 17), 12, 0));
    assert!(before_input((0, 17), 0, 0));
    assert!(!before_input((0, 17), 17, 0));
    assert!(!before_input((0, 17), 30, 0));
    assert!(!before_input((0, 17), 0, 1));
    // A prompt too long to share its row pushes the editor down one.
    assert!(before_input((1, 0), 70, 0));
    assert!(!before_input((1, 0), 0, 1));
}

#[test]
fn wrapped_click_index_hits_chars_on_the_first_row() {
    assert_eq!(click("git", 4, 80, 4, 0), Some(0));
    assert_eq!(click("git", 4, 80, 6, 0), Some(2));
    assert_eq!(click("git", 4, 80, 1, 0), Some(0));
    assert_eq!(click("git", 4, 80, 40, 0), Some(3));
}

#[test]
fn wrapped_click_index_maps_wrapped_rows() {
    assert_eq!(click("abcdef", 8, 10, 9, 0), Some(1));
    assert_eq!(click("abcdef", 8, 10, 0, 1), Some(2));
    assert_eq!(click("abcdef", 8, 10, 3, 1), Some(5));
    assert_eq!(click("a你", 2, 4, 3, 0), Some(1));
    assert_eq!(click("abcdef", 8, 10, 9, 1), Some(6));
}

#[test]
fn wrapped_click_index_respects_wide_chars() {
    assert_eq!(click("你好", 2, 80, 2, 0), Some(0));
    assert_eq!(click("你好", 2, 80, 3, 0), Some(0));
    assert_eq!(click("你好", 2, 80, 4, 0), Some(1));
    assert_eq!(click("你", 4, 5, 0, 1), Some(0));
    assert_eq!(click("你", 4, 5, 1, 1), Some(0));
}

#[test]
fn wrapped_click_index_rows_past_the_input_need_clamp() {
    let chars: Vec<char> = "ls".chars().collect();
    assert_eq!(wrapped_click_index(&chars, (0, 4), 80, 3, 2, false), None);
    assert_eq!(wrapped_click_index(&chars, (0, 4), 80, 3, 2, true), Some(2));
    assert_eq!(wrapped_click_index(&[], (0, 4), 80, 30, 0, false), Some(0));
    assert_eq!(wrapped_click_index(&chars, (0, 4), 80, 3, 1, false), None);
}

#[test]
fn wrapped_click_index_covers_the_wrapped_caret_slot() {
    assert_eq!(click("abcdef", 4, 10, 0, 1), Some(6));
    assert_eq!(click("abcdef", 4, 10, 7, 1), Some(6));
    let chars: Vec<char> = "abcdef".chars().collect();
    assert_eq!(wrapped_click_index(&chars, (0, 4), 10, 0, 2, false), None);
}

#[test]
fn wrapped_click_index_treats_newlines_as_hard_breaks() {
    assert_eq!(click("a\nbc", 4, 80, 4, 0), Some(0));
    assert_eq!(click("a\nbc", 4, 80, 0, 1), Some(2));
    assert_eq!(click("a\nbc", 4, 80, 1, 1), Some(3));
    assert_eq!(click("a\nbc", 4, 80, 40, 0), Some(1));
    assert_eq!(click("a\nbc", 4, 80, 40, 1), Some(4));
    assert_eq!(click("a\n\nb", 4, 80, 3, 1), Some(2));
    assert_eq!(click("a\n\nb", 4, 80, 0, 2), Some(3));
}

#[test]
fn input_overlay_rows_counts_wraps_slot_marked_and_newlines() {
    let rows = |text: &str, cursor: usize, marked: &str, scol: usize, cols: usize| {
        let chars: Vec<char> = text.chars().collect();
        input_overlay_rows(&chars, cursor, marked, (0, scol), cols)
    };
    assert_eq!(rows("", 0, "", 3, 8), (1, 0));
    assert_eq!(rows("aaaaaaaaaa", 10, "", 6, 8), (3, 2));
    assert_eq!(rows("aaaaaaaaaa", 3, "", 6, 8), (2, 1));
    assert_eq!(rows("ab\ncd", 5, "", 0, 8), (2, 1));
    assert_eq!(rows("ab", 1, "漢", 6, 8), (2, 1));
}

#[test]
fn the_input_stays_beside_an_ordinary_prompt() {
    // `user@host ~/src/app % ` in an 80-column pane: 55 columns left.
    assert_eq!(input_start(25, 80), (0, 25));
    // No prompt at all, and a bare `$ `.
    assert_eq!(input_start(0, 80), (0, 0));
    assert_eq!(input_start(2, 143), (0, 2));
    // Exactly a third left is enough.
    assert_eq!(input_start(60, 90), (0, 60));
}

#[test]
fn a_prompt_that_eats_the_row_pushes_the_input_below_it() {
    // #767: a 119-column prompt in a 143-column pane left 24.
    assert_eq!(input_start(119, 143), (1, 0));
    // One column short of a third.
    assert_eq!(input_start(61, 90), (1, 0));
    // A narrow pane falls back to the 20-column floor, not a third of it.
    assert_eq!(input_start(25, 40), (1, 0));
    assert_eq!(input_start(20, 40), (0, 20));
    // A cursor parked in the last column, or past it, has no room at all.
    assert_eq!(input_start(79, 80), (1, 0));
    assert_eq!(input_start(80, 80), (1, 0));
}

#[test]
fn a_new_row_has_to_gain_something() {
    // A 20-column pane: a `$ ` prompt leaves 18, under the floor, but a
    // fresh row would give only two more.
    assert_eq!(input_start(2, 20), (0, 2));
    // Half the row is the break-even point.
    assert_eq!(input_start(9, 20), (0, 9));
    assert_eq!(input_start(10, 20), (1, 0));
}

/// Everything that reads the layout — the overlay's height, the caret's
/// row, clicks — sees the input a row down, with the prompt's row empty.
#[test]
fn input_below_the_prompt_is_laid_out_from_the_next_row() {
    let start = input_start(119, 143);
    let chars: Vec<char> = "git status".chars().collect();
    let (positions, r, c) = input_char_positions(&chars, start, 143);
    assert_eq!(positions[0], (1, 0, 1));
    assert_eq!((r, c), (1, 10));
    // Two rows tall — the prompt's and the input's — with the caret on
    // the second, so an overflow shift keeps the input on screen.
    assert_eq!(input_overlay_rows(&chars, 10, "", start, 143), (2, 1));
    assert_eq!(input_overlay_rows(&[], 0, "", start, 143), (2, 1));
    // Wrapping uses the whole width of the new row.
    let long: Vec<char> = "a".repeat(150).chars().collect();
    assert_eq!(input_overlay_rows(&long, 150, "", start, 143), (3, 2));
    // A click on the prompt's row lands before the first character; on
    // the input row it hits the character under it.
    assert_eq!(
        wrapped_click_index(&chars, start, 143, 130, 0, false),
        Some(0)
    );
    assert_eq!(
        wrapped_click_index(&chars, start, 143, 4, 1, false),
        Some(4)
    );
    assert_eq!(
        wrapped_click_index(&chars, start, 143, 60, 1, false),
        Some(10)
    );
    assert_eq!(wrapped_click_index(&chars, start, 143, 0, 2, false), None);
}

#[test]
fn input_overflow_shift_keeps_the_tail_and_caret_visible() {
    assert_eq!(input_overflow_shift(5, 2, 3, 22), 0);
    assert_eq!(input_overflow_shift(20, 2, 3, 22), 1);
    assert_eq!(input_overflow_shift(21, 29, 30, 22), 29);
    assert_eq!(input_overflow_shift(21, 0, 30, 22), 21);
}

#[test]
fn a_description_that_fits_is_left_alone() {
    assert_eq!(elide("Show commit logs", 40), "Show commit logs");
    // Exactly the budget is still a fit; nothing is spent on an ellipsis.
    assert_eq!(elide("abcd", 4), "abcd");
}

#[test]
fn an_overlong_description_ends_in_an_ellipsis_inside_its_budget() {
    let out = elide("Move or rename a file, a directory, or a symlink", 12);
    assert_eq!(out.chars().count(), 12, "the ellipsis is inside the budget");
    assert!(out.ends_with('…'));
    assert!(out.starts_with("Move or ren"));
}

#[test]
fn a_budget_with_no_room_to_say_anything_says_nothing() {
    // A lone "…" reads as a description that is there but unreadable.
    assert_eq!(elide("Show commit logs", 1), "");
    assert_eq!(elide("Show commit logs", 0), "");
}

#[test]
fn the_description_budget_is_what_the_name_leaves_of_the_menu() {
    const W: f32 = COMPLETION_MENU_MAX_W;
    // A 9px cell: 46px of row chrome, so a bare row leaves (480-46)/9 cells.
    assert_eq!(description_budget(9., 0, W), 48);
    // Every cell the name takes is one the description does not get.
    assert_eq!(description_budget(9., 10, W), 38);
    // A name that fills the menu on its own leaves nothing, and never
    // underflows into a huge budget.
    assert_eq!(description_budget(9., 200, W), 0);
    assert_eq!(description_budget(0., 10, W), 0);
    // A pane too narrow for the full menu shrinks the budget with it,
    // instead of eliding to a width the menu never got.
    assert!(description_budget(9., 10, 240.) < description_budget(9., 10, W));
    assert_eq!(description_budget(9., 10, 240.), 11);
}

#[test]
fn menu_layout_prefers_below_and_flips_above_when_cramped() {
    assert_eq!(menu_layout(24, 3, 5, 0, 10), (false, 5, 0));
    assert_eq!(menu_layout(24, 22, 5, 0, 10), (true, 5, 0));
    assert_eq!(menu_layout(6, 4, 10, 0, 10), (true, 2, 0));
    assert_eq!(menu_layout(6, 1, 10, 0, 10), (false, 2, 0));
    let (_, visible, _) = menu_layout(1, 0, 8, 0, 10);
    assert_eq!(visible, 1);
}

#[test]
fn menu_layout_budgets_the_overflow_footers() {
    let (place_above, visible, first) = menu_layout(24, 13, 30, 17, 10);
    assert!(
        place_above,
        "12 needed lines don't fit in the 10 rows below"
    );
    assert_eq!(visible, 10);
    assert!((first..first + visible).contains(&17));
}

#[test]
fn menu_layout_caps_rows_and_windows_around_the_selection() {
    let (_, visible, first) = menu_layout(40, 0, 30, 17, 10);
    assert_eq!(visible, 10);
    assert!((first..first + visible).contains(&17));
    assert_eq!(first, 8);
    let (_, visible, first) = menu_layout(40, 0, 30, 29, 10);
    assert_eq!(first, 20);
    assert_eq!(first + visible, 30);
    assert_eq!(menu_layout(40, 0, 30, 3, 10).2, 0);
}

#[test]
fn a_multiline_entry_draws_on_one_menu_row() {
    // A command composed in the inline editor keeps its newlines in the
    // in-session history. Left alone, gpui breaks the row's text on them
    // and the tail paints over every row below it.
    let line = "curl 'http://x' \\\n  -H 'accept: */*' \\\n  -H 'pragma: no-cache'";
    let runs = highlight_runs(line, &[0, 1, 2, 3]);
    let drawn: String = runs.iter().map(|(run, _)| run.as_str()).collect();
    assert!(
        !drawn.contains('\n'),
        "no run carries a line break: {drawn}"
    );
    assert_eq!(drawn.chars().count(), line.chars().count());
    assert_eq!(drawn.matches('↵').count(), 2);
    assert_eq!(
        runs.first().map(|(r, hit)| (r.as_str(), *hit)),
        Some(("curl", true))
    );
    assert!(runs[1..].iter().all(|(_, hit)| !hit));
}

#[test]
fn highlight_runs_alternate_on_the_matched_characters() {
    let runs = highlight_runs("git status", &[0, 4, 5]);
    let shape: Vec<(&str, bool)> = runs.iter().map(|(r, h)| (r.as_str(), *h)).collect();
    assert_eq!(
        shape,
        [("g", true), ("it ", false), ("st", true), ("atus", false)]
    );
    assert!(highlight_runs("", &[]).is_empty());
}

/// "Open with Default App" sits beside "Open" only where it adds
/// something: on a local file, and when "Open" is not already the OS
/// association.
#[test]
fn the_default_app_row_is_offered_only_where_it_differs_from_open() {
    use super::{LinkFileOpen, offers_default_app_open};
    assert!(offers_default_app_open(true, LinkFileOpen::Internal));
    assert!(offers_default_app_open(true, LinkFileOpen::Command));
    assert!(
        !offers_default_app_open(true, LinkFileOpen::System),
        "it would repeat Open"
    );
    for mode in [
        LinkFileOpen::Internal,
        LinkFileOpen::System,
        LinkFileOpen::Command,
    ] {
        assert!(
            !offers_default_app_open(false, mode),
            "a remote file has nothing here for the OS to open ({mode:?})"
        );
    }
}

#[test]
fn only_a_matching_host_may_answer_for_a_panes_paths() {
    assert!(cwd_is_on_host(false, true));
    assert!(cwd_is_on_host(true, false));

    assert!(!cwd_is_on_host(true, true));
    assert!(!cwd_is_on_host(false, false));
}

/// Which machine's spelling a pane's paths are read in. Ungated on
/// purpose: the bug this settles was a Windows-only one that hid behind a
/// `#[cfg(unix)]` on the test that covered it.
#[test]
fn a_panes_paths_are_read_in_its_own_hosts_spelling() {
    use super::super::search::PathStyle;
    use std::path::Path;

    assert_eq!(
        link_path_style(true, Some(Path::new("/home/u/proj"))),
        PathStyle::NATIVE,
        "a pane on this machine reads its own output this OS's way, \
         whatever its shell spells the cwd like"
    );
    assert_eq!(
        link_path_style(false, Some(Path::new("/home/u/proj"))),
        PathStyle::Posix,
        "an SSH host, a remote workspace or a WSL distro reporting a \
         /-rooted cwd is a POSIX one on every client"
    );
    assert_eq!(
        link_path_style(false, Some(Path::new(r"C:\Users\u\proj"))),
        PathStyle::Windows,
        "and a remote Windows host is not"
    );
    assert_eq!(
        link_path_style(false, None),
        PathStyle::Posix,
        "a remote pane that has not said where it is still has no local \
         drive its paths could hang off"
    );
}

/// The window between the shell reporting a prompt and its line editor
/// actually reading, which is where a fast typist's line goes missing (#433).
///
/// These drive a real `TerminalView` over a pane link on every platform, so
/// they are not gated to unix the way `gpui_tests` is.
mod prompt_handover_tests {
    use super::super::*;
    use anterm_core::daemon::protocol::{ClientMsg, DaemonMsg};
    use anterm_core::daemon::transport::Stream;
    use gpui::{KeyDownEvent, Modifiers, ScrollEvent, TestAppContext};

    fn harness(cx: &mut TestAppContext) -> (gpui::WindowHandle<TerminalView>, Stream) {
        anterm_ui_kit::config::pin_test_config_dir();
        cx.executor().allow_parking();
        let (client_side, daemon_side) = test_stream_pair();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Config::default());
        });
        let window = cx.add_window(|window, cx| {
            let terminal = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24))
                .expect("link-backed terminal");
            TerminalView::with_terminal(terminal, 1, window, cx)
        });
        (window, daemon_side)
    }

    /// Everything the pane has written to the PTY, in order.
    fn drain(daemon: &mut Stream) -> Vec<u8> {
        daemon
            .set_read_timeout(Some(std::time::Duration::from_millis(150)))
            .unwrap();
        let mut out = Vec::new();
        loop {
            match ClientMsg::read(daemon) {
                Ok(ClientMsg::Input(bytes)) => out.extend_from_slice(&bytes),
                Ok(_) => continue,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(e) => panic!("pane link failed: {e}"),
            }
        }
        out
    }

    fn settle(
        cx: &mut TestAppContext,
        window: &gpui::WindowHandle<TerminalView>,
        what: &str,
        f: impl Fn(&TerminalView) -> bool,
    ) {
        for _ in 0..300 {
            cx.run_until_parked();
            if window.update(cx, |view, _, _| f(view)).unwrap() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("never settled: {what}");
    }

    /// Bytes from the pane's pty, the way the daemon forwards them.
    fn output(daemon: &mut Stream, bytes: &[u8]) {
        DaemonMsg::Output(bytes.to_vec()).encode(daemon).unwrap();
    }

    /// Waits for the tab's reading of what the pane calls itself to become
    /// `expect`, running out the wait a new title is held for on each pass.
    fn titled(
        cx: &mut TestAppContext,
        window: &gpui::WindowHandle<TerminalView>,
        expect: Option<&str>,
    ) {
        for _ in 0..300 {
            cx.run_until_parked();
            cx.executor().advance_clock(TITLE_SETTLE * 2);
            cx.run_until_parked();
            let showing = window
                .update(cx, |view, _, _| view.stated_title().map(str::to_string))
                .unwrap();
            if showing.as_deref() == expect {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("the tab never came to read {expect:?}");
    }

    /// #889: a tab went on reading the name Claude Code had left on it long
    /// after Claude exited and the pane was back at its own prompt in a real
    /// directory. An OSC 0/2 had no end — only another OSC 0/2 replaced it —
    /// so the last title any program wrote in a pane outlived it forever.
    #[gpui::test]
    fn a_title_a_command_set_is_retired_when_that_command_finishes(cx: &mut TestAppContext) {
        let (window, mut daemon) = harness(cx);
        output(&mut daemon, b"\x1b]133;A\x07\x1b]133;B\x07");
        output(
            &mut daemon,
            b"\x1b]133;C;claude\x07\x1b]2;\xe2\x9c\xb3 fixing the switcher\x1b\\",
        );
        titled(cx, &window, Some("✳ fixing the switcher"));

        // Claude exits and the shell reports the command finished. Nothing
        // titles the pane after it, so the tab has to fall back down the
        // label ladder — `stated_title` saying nothing is how it does that.
        output(&mut daemon, b"\x1b]133;D;0\x07");
        titled(cx, &window, None);
    }

    /// The two cases a title is *supposed* to outlive: a program that is still
    /// running, and a shell that titles its own prompt (which it does between
    /// the `D` and the `A`, so it is the last word rather than a thing undone).
    #[gpui::test]
    fn a_running_program_and_a_shells_own_prompt_title_both_keep_theirs(cx: &mut TestAppContext) {
        let (window, mut daemon) = harness(cx);
        output(
            &mut daemon,
            b"\x1b]133;C;vim\x07\x1b]2;vim \xe2\x80\x94 main.rs\x1b\\",
        );
        titled(cx, &window, Some("vim — main.rs"));

        output(
            &mut daemon,
            b"\x1b]133;D;0\x07\x1b]0;me@box:~/dev\x07\x1b]133;A\x07\x1b]133;B\x07",
        );
        titled(cx, &window, Some("me@box:~/dev"));
        settle(cx, &window, "the prompt is reading", |view| {
            view.terminal.zle_reading()
        });

        // And that prompt title is nobody's command to retire. Both marks are
        // chased by an edge the pane reports, so the assertion below cannot
        // pass on bytes that have not landed yet.
        output(&mut daemon, b"\x1b]133;C;ls\x07");
        settle(cx, &window, "a command takes the pane", |view| {
            !view.terminal.zle_reading()
        });
        output(&mut daemon, b"\x1b]133;D;0\x07\x1b]133;B\x07");
        settle(cx, &window, "the prompt comes back", |view| {
            view.terminal.zle_reading()
        });
        titled(cx, &window, Some("me@box:~/dev"));
    }

    /// #889 on a reattached window: a long session outran the daemon's replay
    /// ring, so the replay carries the program's title but not the `C` that
    /// started it. The replayed prompt state says a command owns the pane,
    /// and that is enough to know the title is the command's to lose at `D`.
    #[gpui::test]
    fn a_reattached_window_retires_a_title_whose_c_rolled_out_of_the_replay(
        cx: &mut TestAppContext,
    ) {
        anterm_ui_kit::config::pin_test_config_dir();
        cx.executor().allow_parking();
        let (client_side, mut daemon) = test_stream_pair();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Config::default());
        });
        let window = cx.add_window(|window, cx| {
            let terminal =
                RemoteTerminal::from_stream_reattached(client_side, TermSize::new(80, 24))
                    .expect("reattached link-backed terminal");
            TerminalView::with_terminal(terminal, 1, window, cx)
        });

        DaemonMsg::Snapshot(b"\x1b]2;\xe2\x9c\xb3 fixing the switcher\x1b\\redraw".to_vec())
            .encode(&mut daemon)
            .unwrap();
        DaemonMsg::Prompt {
            active: true,
            at_prompt: false,
            last_exit: None,
        }
        .encode(&mut daemon)
        .unwrap();
        titled(cx, &window, Some("✳ fixing the switcher"));

        output(&mut daemon, b"\x1b]133;D;0\x07");
        titled(cx, &window, None);
    }

    /// Printable text arrives the way the platform delivers it — through the
    /// text-input path, which is what the gap hold and the typeahead record see.
    fn type_text(window: &gpui::WindowHandle<TerminalView>, cx: &mut TestAppContext, text: &str) {
        for ch in text.chars() {
            window
                .update(cx, |view, _, cx| view.commit_text(&ch.to_string(), cx))
                .unwrap();
        }
    }

    fn press(window: &gpui::WindowHandle<TerminalView>, cx: &mut TestAppContext, key: &str) {
        window
            .update(cx, |view, window, cx| {
                view.on_key_down(
                    &KeyDownEvent {
                        keystroke: gpui::Keystroke::parse_with_os(key, anterm_ui_kit::platform::current_os()).unwrap(),
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                );
            })
            .unwrap();
    }

    fn prompt(daemon: &mut Stream, at_prompt: bool) {
        DaemonMsg::Prompt {
            active: true,
            at_prompt,
            last_exit: None,
        }
        .encode(daemon)
        .unwrap();
    }

    /// Types `text` into the gap of a running command and lets the hold window
    /// expire, so the bytes go to the PTY and are recorded for replay. Then
    /// puts the pane back at a prompt the way the `D` mark does — before the
    /// prompt is drawn, so the shell's line editor is not reading yet.
    fn typed_into_the_gap_then_handed_back(
        cx: &mut TestAppContext,
        window: &gpui::WindowHandle<TerminalView>,
        daemon: &mut Stream,
        text: &str,
    ) {
        prompt(daemon, true);
        DaemonMsg::Output(b"\x1b]133;B\x07".to_vec())
            .encode(daemon)
            .unwrap();
        settle(cx, window, "the editor takes the first prompt", |view| {
            view.input_active() && view.terminal.zle_reading()
        });

        prompt(daemon, false);
        DaemonMsg::Output(b"\x1b]133;C\x07".to_vec())
            .encode(daemon)
            .unwrap();
        settle(cx, window, "a command takes the pane", |view| {
            !view.input_active()
        });

        type_text(window, cx, text);
        cx.executor().advance_clock(HOLD_WINDOW * 2);
        cx.run_until_parked();
        assert_eq!(
            drain(daemon),
            text.as_bytes(),
            "the hold window gives up and dumps what it held"
        );

        prompt(daemon, true);
        settle(cx, window, "the editor takes the prompt back", |view| {
            view.input_active()
        });
        assert!(
            !window
                .update(cx, |view, _, _| view.terminal.zle_reading())
                .unwrap(),
            "this is the D-to-B window: the shell is not reading its line yet"
        );
    }

    #[gpui::test]
    fn a_line_typed_into_the_gap_survives_a_prompt_that_is_not_reading_yet(
        cx: &mut TestAppContext,
    ) {
        let (window, mut daemon) = harness(cx);
        typed_into_the_gap_then_handed_back(cx, &window, &mut daemon, "echo hi");

        press(&window, cx, "enter");
        cx.run_until_parked();
        assert_eq!(
            drain(&mut daemon),
            b"\x15echo hi\r".to_vec(),
            "the line the shell is holding must be erased and submitted whole, \
             not erased and replaced by an empty command"
        );
    }

    #[gpui::test]
    fn a_prompt_handover_keeps_the_held_text_in_front_of_what_follows_it(cx: &mut TestAppContext) {
        let (window, mut daemon) = harness(cx);
        typed_into_the_gap_then_handed_back(cx, &window, &mut daemon, "echo");

        // Typing carries straight on into the editor that just took the prompt.
        type_text(&window, cx, " hi");
        cx.run_until_parked();
        assert_eq!(
            drain(&mut daemon),
            Vec::<u8>::new(),
            "the editor owns these keys, so none of them reach the PTY"
        );

        press(&window, cx, "enter");
        cx.run_until_parked();
        assert_eq!(
            drain(&mut daemon),
            b"\x15echo hi\r".to_vec(),
            "what the shell was holding leads the line, not the tail alone"
        );
    }

    /// The editor takes the held line over the moment it is touched, before it
    /// edits anything — and takes it over without putting the wipe on the wire,
    /// which is still the shell's line editor's to receive when it starts
    /// reading.
    #[gpui::test]
    fn the_editor_takes_the_held_line_over_before_it_edits_it(cx: &mut TestAppContext) {
        let (window, mut daemon) = harness(cx);
        typed_into_the_gap_then_handed_back(cx, &window, &mut daemon, "echo");

        // `home` moves the caret and nothing else: any editor key is enough.
        press(&window, cx, "home");
        cx.run_until_parked();
        window
            .update(cx, |view, _, _| {
                assert_eq!(
                    view.prompt.cmd.text(),
                    "echo",
                    "the line the shell is sitting on is the editor's line now"
                );
            })
            .unwrap();
        assert_eq!(
            drain(&mut daemon),
            Vec::<u8>::new(),
            "taking the line over owes the wipe, it does not send it early"
        );

        press(&window, cx, "enter");
        cx.run_until_parked();
        assert_eq!(
            drain(&mut daemon),
            b"\x15echo\r".to_vec(),
            "the owed wipe is paid on submit, still in front of the line"
        );
    }

    /// The half of the window the seed alone does not cover: the editor is
    /// live, so the user can *replace* the line before submitting it. Recalling
    /// history and pressing Enter has to run the entry recalled — not that
    /// entry with the text the shell was holding glued to its front.
    #[gpui::test]
    fn recalling_history_in_the_gap_window_replaces_the_held_line(cx: &mut TestAppContext) {
        let (window, mut daemon) = harness(cx);
        typed_into_the_gap_then_handed_back(cx, &window, &mut daemon, "echo");
        window
            .update(cx, |view, _, cx| {
                cx.default_global::<super::super::HistoryStore>()
                    .snapshot_mut(&view.prompt.history_scope)
                    .entries
                    .push("echo from history".to_string());
                view.invalidate_ghost_cache();
            })
            .unwrap();

        press(&window, cx, "up");
        cx.run_until_parked();
        window
            .update(cx, |view, _, _| {
                assert_eq!(
                    view.prompt.cmd.text(),
                    "echo from history",
                    "the recall searches on the whole line, held text included"
                );
            })
            .unwrap();

        press(&window, cx, "enter");
        cx.run_until_parked();
        assert_eq!(
            drain(&mut daemon),
            b"\x15echo from history\r".to_vec(),
            "the recalled entry runs on its own, with the held text replaced \
             rather than prefixed to it"
        );
    }

    /// A paste that landed in the gap is still a paste after the handover. The
    /// record replays it into the editor, and a line that arrives there looking
    /// typed is submitted raw through the shell's binding table (#660) — the
    /// hole `GapHold::pasted` closed for the hold's own route.
    #[gpui::test]
    fn a_paste_held_in_the_gap_is_still_a_paste_after_the_handover(cx: &mut TestAppContext) {
        let (window, mut daemon) = harness(cx);
        // Bracketed paste is what a live prompt advertises; without it there is
        // no framing to lose in the first place.
        DaemonMsg::Output(b"\x1b[?2004h".to_vec())
            .encode(&mut daemon)
            .unwrap();
        settle(cx, &window, "the shell turns bracketed paste on", |view| {
            view.terminal
                .term
                .lock()
                .mode()
                .contains(TermMode::BRACKETED_PASTE)
        });

        prompt(&mut daemon, true);
        DaemonMsg::Output(b"\x1b]133;B\x07".to_vec())
            .encode(&mut daemon)
            .unwrap();
        settle(cx, &window, "the editor takes the first prompt", |view| {
            view.input_active() && view.terminal.zle_reading()
        });
        prompt(&mut daemon, false);
        DaemonMsg::Output(b"\x1b]133;C\x07".to_vec())
            .encode(&mut daemon)
            .unwrap();
        settle(cx, &window, "a command takes the pane", |view| {
            !view.input_active()
        });

        window
            .update(cx, |view, _, cx| view.paste("echo hi".to_string(), cx))
            .unwrap();
        cx.executor().advance_clock(HOLD_WINDOW * 2);
        cx.run_until_parked();
        assert_eq!(
            drain(&mut daemon),
            b"\x1b[200~echo hi\x1b[201~".to_vec(),
            "the hold window gives up and dumps the paste as a paste"
        );

        prompt(&mut daemon, true);
        settle(cx, &window, "the editor takes the prompt back", |view| {
            view.input_active()
        });

        press(&window, cx, "enter");
        cx.run_until_parked();
        assert_eq!(
            drain(&mut daemon),
            b"\x15\x1b[200~echo hi\x1b[201~\r".to_vec(),
            "the replayed line keeps its framing instead of being typed at the \
             shell's binding table"
        );
    }

    /// The same for an emptied line: ⌃U clears what the editor is holding, and
    /// the shell's copy of it goes too instead of coming back at submit.
    #[gpui::test]
    fn clearing_the_line_in_the_gap_window_clears_the_held_text_too(cx: &mut TestAppContext) {
        let (window, mut daemon) = harness(cx);
        typed_into_the_gap_then_handed_back(cx, &window, &mut daemon, "echo");

        press(&window, cx, "ctrl-u");
        cx.run_until_parked();
        window
            .update(cx, |view, _, _| assert_eq!(view.prompt.cmd.text(), ""))
            .unwrap();

        press(&window, cx, "enter");
        cx.run_until_parked();
        assert_eq!(
            drain(&mut daemon),
            b"\x15\r".to_vec(),
            "an emptied line submits empty: the wipe is still owed, the seed is not"
        );
    }

    /// #1131: a wheel turned over one part of a TUI scrolled whichever part
    /// was last clicked. The wheel report carried the cell of the last button
    /// press, and only a click — or motion, for the few programs that ask for
    /// every move — ever updated it. tmux asks for button events only (1002),
    /// so after clicking its status line every wheel notch over the panes
    /// above was reported as being over the status line.
    #[gpui::test]
    fn a_wheel_report_names_the_cell_under_the_pointer_not_the_last_click(cx: &mut TestAppContext) {
        let (window, mut daemon) = harness(cx);
        output(&mut daemon, b"\x1b[?1002h\x1b[?1006h");
        settle(cx, &window, "the program asked for the mouse", |view| {
            view.mouse_mode()
        });

        window
            .update(cx, |view, w, cx| {
                view.render.cell_geom = Some(crate::element::CellGeom {
                    origin: gpui::point(px(0.), px(0.)),
                    cell_width: px(10.),
                    line_height: px(20.),
                    cols: 80,
                    rows: 24,
                });
                // A click on the bottom row, where tmux keeps its window list.
                view.mouse_press(MouseButton::Left, 3, 23, &Modifiers::default());
                view.mouse_release(MouseButton::Left, 3, 23, &Modifiers::default());
                // Then one notch up, with the pointer up in the pane content.
                let ev = ScrollEvent {
                    position: gpui::point(px(105.), px(110.)),
                    delta: gpui::ScrollDelta::Lines(gpui::point(0., 1.)),
                    ..Default::default()
                };
                view.on_scroll(&ev, w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(
            String::from_utf8(drain(&mut daemon)).unwrap(),
            "\x1b[<0;4;24M\x1b[<0;4;24m\x1b[<64;11;6M",
            "the notch is reported at the cell the pointer is over"
        );
    }

    /// #1131: on Linux the candidate window only follows the caret if the
    /// rectangle is pushed, so every move has to be pushed — once — and a
    /// pane coming back into focus has to push again even if the caret never
    /// moved, because another pane's anchor was the last one sent.
    #[gpui::test]
    fn the_ime_anchor_is_pushed_when_it_moves_and_again_after_a_refocus(cx: &mut TestAppContext) {
        let (window, _daemon) = harness(cx);
        let cell = |col: f32, row: f32| {
            Some(gpui::Bounds::new(
                gpui::point(px(col * 10.), px(row * 20.)),
                gpui::size(px(10.), px(20.)),
            ))
        };
        window
            .update(cx, |view, _, _| {
                assert!(view.ime_anchor_moved(cell(2., 0.)), "the first frame");
                assert!(!view.ime_anchor_moved(cell(2., 0.)), "nothing moved");
                assert!(view.ime_anchor_moved(cell(9., 4.)), "the TUI moved it");
                assert!(!view.ime_anchor_moved(None), "focus left: nothing to push");
                assert!(
                    view.ime_anchor_moved(cell(9., 4.)),
                    "focus came back to a caret that never moved"
                );
            })
            .unwrap();
    }

    /// #1182: dragging anywhere in the window — the title bar, the sidebar,
    /// the pane next door — reached every pane whose program had asked for
    /// the mouse. The drag handler listens on the whole window and pinned the
    /// pointer to the nearest cell, so tmux (`mouse on`) saw a drag along its
    /// top row it never saw a press for, dropped into copy-mode and started
    /// selecting. A click anywhere likewise sent a release to every such pane.
    ///
    /// Only a press the pane itself reported starts a drag it reports; that
    /// drag follows the pointer out of the pane (clamped, as xterm does) and
    /// ends with the release, and nothing reaches the pane beside it.
    #[gpui::test]
    fn only_the_pane_a_press_landed_in_hears_the_drag(cx: &mut TestAppContext) {
        use gpui::{MouseMoveEvent, MouseUpEvent, PlatformInput};

        struct Split {
            a: gpui::Entity<TerminalView>,
            b: gpui::Entity<TerminalView>,
        }
        impl Render for Split {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .flex()
                    .flex_row()
                    // Stands in for the sidebar and anything else that is not
                    // a pane: no hitbox of either terminal covers it.
                    .child(div().w(px(200.)).h_full())
                    .child(div().flex_1().h_full().child(self.a.clone()))
                    .child(div().flex_1().h_full().child(self.b.clone()))
            }
        }

        anterm_ui_kit::config::pin_test_config_dir();
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Config::default());
        });
        let (a_client, mut a_pty) = test_stream_pair();
        let (b_client, mut b_pty) = test_stream_pair();
        let window = cx.add_window(|window, cx| {
            let pane = |link, window: &mut Window, cx: &mut Context<Split>| {
                let terminal = RemoteTerminal::from_stream(link, TermSize::new(80, 24))
                    .expect("link-backed terminal");
                cx.new(|cx| TerminalView::with_terminal(terminal, 1, window, cx))
            };
            let a = pane(a_client, window, cx);
            let b = pane(b_client, window, cx);
            Split { a, b }
        });
        let (a, b) = window
            .update(cx, |split, _, _| (split.a.clone(), split.b.clone()))
            .unwrap();

        // Both panes run tmux with `mouse on`.
        output(&mut a_pty, b"\x1b[?1002h\x1b[?1006h");
        output(&mut b_pty, b"\x1b[?1002h\x1b[?1006h");
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
        for _ in 0..300 {
            vcx.update(|window, _| window.refresh());
            vcx.run_until_parked();
            let ready = vcx.update(|_, cx| {
                [&a, &b].iter().all(|v| {
                    let v = v.read(cx);
                    v.mouse_mode() && v.render.cell_geom.is_some()
                })
            });
            if ready {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let geom = |v: &gpui::Entity<TerminalView>, vcx: &mut gpui::VisualTestContext| {
            vcx.update(|_, cx| v.read(cx).render.cell_geom.expect("the pane was painted"))
        };
        let (ga, gb) = (geom(&a, &mut vcx), geom(&b, &mut vcx));
        assert!(
            ga.origin.x > px(200.) && gb.origin.x > ga.origin.x,
            "sidebar, then A, then B: {:?} {:?}",
            ga.origin,
            gb.origin
        );
        // Cell (col, row) of a pane, as a window position in its middle.
        let at = |g: crate::element::CellGeom, col: f32, row: f32| {
            gpui::point(
                g.origin.x + g.cell_width * (col + 0.5),
                g.origin.y + g.line_height * (row + 0.5),
            )
        };

        let hover = |vcx: &mut gpui::VisualTestContext, position| {
            vcx.update(|window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position,
                        pressed_button: None,
                        modifiers: Modifiers::none(),
                    }),
                    cx,
                );
            });
            vcx.run_until_parked();
        };
        let down = |vcx: &mut gpui::VisualTestContext, position| {
            vcx.update(|window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Modifiers::none(),
                        click_count: 1,
                        first_mouse: false,
                    }),
                    cx,
                );
            });
            vcx.run_until_parked();
        };
        let drag = |vcx: &mut gpui::VisualTestContext, position| {
            vcx.update(|window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position,
                        pressed_button: Some(MouseButton::Left),
                        modifiers: Modifiers::none(),
                    }),
                    cx,
                );
            });
            vcx.run_until_parked();
        };
        let up = |vcx: &mut gpui::VisualTestContext, position| {
            vcx.update(|window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseUp(MouseUpEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Modifiers::none(),
                        click_count: 1,
                    }),
                    cx,
                );
            });
            vcx.run_until_parked();
        };
        let text = |pty: &mut Stream| String::from_utf8(drain(pty)).unwrap();
        // Whatever the panes said while settling is not what this is about.
        let _ = (text(&mut a_pty), text(&mut b_pty));

        // 1. A drag across the sidebar, the way one would drag a window by
        //    its title bar: neither pane saw the press, so neither hears it.
        let side = |y: f32| gpui::point(px(50.), px(y));
        hover(&mut vcx, side(100.));
        down(&mut vcx, side(100.));
        drag(&mut vcx, side(140.));
        drag(&mut vcx, side(180.));
        up(&mut vcx, side(180.));
        assert_eq!(
            (text(&mut a_pty), text(&mut b_pty)),
            (String::new(), String::new()),
            "a drag outside every pane is no pane's business"
        );

        // 2. A drag that starts in A and wanders into B: A hears all of it,
        //    pinned to its own edge once the pointer has left it, and B none.
        hover(&mut vcx, at(ga, 2., 3.));
        down(&mut vcx, at(ga, 2., 3.));
        drag(&mut vcx, at(ga, 4., 3.));
        drag(&mut vcx, at(gb, 1., 5.));
        up(&mut vcx, at(gb, 1., 5.));
        let last = ga.cols; // 1-based: the column A's grid ends on.
        assert_eq!(
            text(&mut a_pty),
            format!("\x1b[<0;3;4M\x1b[<32;5;4M\x1b[<32;{last};6M\x1b[<0;{last};6m"),
            "the pane the press landed in keeps the drag, clamped to its edge"
        );
        assert_eq!(
            text(&mut b_pty),
            "",
            "the pane beside it never saw the press, so it hears nothing"
        );
    }

    #[gpui::test]
    fn shared_history_updates_and_loaded_history_preserve_active_navigation_and_stash(
        cx: &mut TestAppContext,
    ) {
        let key = |spec: &str| {
            gpui::Keystroke::parse_with_os(spec, anterm_ui_kit::platform::current_os()).unwrap()
        };
        let (window, _daemon) = harness(cx);
        window
            .update(cx, |view, _, cx| {
                let mut initial = crate::prompt::history::ScopeSnapshot::new();
                initial.entries = vec!["git status".to_string(), "git log".to_string()];
                cx.default_global::<super::super::HistoryStore>()
                    .replace_scope_for_test(&view.prompt.history_scope, initial);
                view.prompt.cmd.set("git ");

                view.handle_editor_key(&key("up"), cx);
                assert_eq!(view.prompt.cmd.text(), "git log");
                assert!(view.prompt.history_nav.is_some());

                // Simulate another pane recording a command into the shared
                // HistoryStore while this pane is actively navigating history.
                cx.default_global::<super::super::HistoryStore>().record(
                    crate::prompt::history::RecordInput {
                        scope: view.prompt.history_scope.clone(),
                        session_id: "local:other".to_string(),
                        cmd: "ls -la".to_string(),
                        cwd: None,
                        ts: 1,
                        duration_ms: None,
                        exit: None,
                    },
                );
                view.sync_shared_history(cx);
                assert!(
                    view.prompt.history_nav.is_some(),
                    "background shared history sync must remap history_nav instead of dropping it"
                );
                assert_eq!(view.prompt.cmd.text(), "git log");

                // Simulate async loaded history replacing the scope snapshot
                // mid-navigation.
                let mut loaded = crate::prompt::history::ScopeSnapshot::new();
                loaded.entries = vec![
                    "echo first".to_string(),
                    "git status".to_string(),
                    "git log".to_string(),
                ];
                view.install_loaded_history(loaded, cx);
                assert!(
                    view.prompt.history_nav.is_some(),
                    "install_loaded_history must remap active navigation index"
                );

                view.handle_editor_key(&key("up"), cx);
                assert_eq!(view.prompt.cmd.text(), "git status");
                view.handle_editor_key(&key("down"), cx);
                assert_eq!(view.prompt.cmd.text(), "git log");
                view.handle_editor_key(&key("down"), cx);
                assert_eq!(
                    view.prompt.cmd.text(),
                    "git ",
                    "stashed typed prefix must still be restored after background history updates"
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn editing_recalled_history_clears_nav_so_next_up_keeps_the_edit(cx: &mut TestAppContext) {
        let key = |spec: &str| {
            gpui::Keystroke::parse_with_os(spec, anterm_ui_kit::platform::current_os()).unwrap()
        };
        let (window, _daemon) = harness(cx);
        window
            .update(cx, |view, _, cx| {
                let mut initial = crate::prompt::history::ScopeSnapshot::new();
                initial.entries = vec!["git status".to_string(), "git log".to_string()];
                cx.default_global::<super::super::HistoryStore>()
                    .replace_scope_for_test(&view.prompt.history_scope, initial);
                view.handle_editor_key(&key("up"), cx);
                assert_eq!(view.prompt.cmd.text(), "git log");

                // Deleting inside the recalled entry begins a fresh edited line.
                view.handle_editor_key(&key("delete"), cx);
                view.prompt.cmd.backspace();
                assert_eq!(view.prompt.cmd.text(), "git lo");

                view.handle_editor_key(&key("up"), cx);
                assert_eq!(view.prompt.cmd.text(), "git log");
                view.handle_editor_key(&key("down"), cx);
                assert_eq!(
                    view.prompt.cmd.text(),
                    "git lo",
                    "edited command must be stashed instead of overwritten by stale history_nav"
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn shorter_submitted_mark_after_longer_pending_is_not_dropped(cx: &mut TestAppContext) {
        let (window, mut daemon) = harness(cx);
        prompt(&mut daemon, true);
        settle(cx, &window, "initial prompt ready", |view| view.terminal.at_prompt());
        output(&mut daemon, b"\x1b]133;C;git%20status\x07\r\n");
        settle(cx, &window, "first percent-encoded command arrives", |view| {
            view.prompt.pending_history
                .as_ref()
                .is_some_and(|p| p.line == "git status")
        });

        let first_seq = window
            .update(cx, |view, _, _| view.terminal.prompt_seq())
            .unwrap();
        prompt(&mut daemon, true);
        settle(cx, &window, "prompt seq advances", |view| {
            view.terminal.prompt_seq() > first_seq
        });
        output(&mut daemon, b"\x1b]133;C;git\x07\r\n");
        for _ in 0..200 {
            if window
                .update(cx, |view, _, cx| {
                    cx.default_global::<super::super::HistoryStore>()
                        .snapshot(&view.prompt.history_scope)
                        .entries
                        .iter()
                        .any(|h| h == "git")
                })
                .unwrap()
            {
                return;
            }
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("timed out waiting for shorter second command to be recorded");
    }

    #[gpui::test]
    fn set_grid_size_reports_layout_changes_and_input_bar_skips_missing_cursor(
        cx: &mut TestAppContext,
    ) {
        let (window, _daemon) = harness(cx);
        window
            .update(cx, |view, _, cx| {
                // Establish initial known size so a width-only or cell-size-only
                // change returns `true` for prepaint relayout notification.
                view.set_grid_size(80, 24, px(8.), px(17.), 1., cx);
                assert!(
                    !view.set_grid_size(80, 24, px(8.), px(17.), 1., cx),
                    "unchanged grid and cell dimensions should not dirty layout"
                );
                assert!(
                    view.set_grid_size(100, 24, px(8.), px(17.), 1., cx),
                    "width change with constant height/slack must still trigger relayout"
                );
                assert!(
                    view.set_grid_size(100, 24, px(9.), px(17.), 1., cx),
                    "cell metric change must trigger relayout"
                );

                // When cursor_cell() is None (cursor scrolled above display_offset),
                // render_input_bar must skip rendering instead of anchoring at (0, 0).
                view.terminal
                    .term
                    .lock()
                    .scroll_display(alacritty_terminal::grid::Scroll::PageUp);
                if view.cursor_cell().is_none() {
                    assert!(view.render_input_bar(true, cx).is_none());
                }
            })
            .unwrap();
    }
}
