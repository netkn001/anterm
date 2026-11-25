use super::paths::{quarantine_candidates, resolve_config_dir};
use super::*;

#[test]
fn notify_mode_holds_back_only_what_is_watched_under_unfocused() {
    for watched in [false, true] {
        assert!(!NotifyMode::Never.allows(watched));
        assert!(NotifyMode::Always.allows(watched));
        assert_eq!(NotifyMode::Unfocused.allows(watched), !watched);
    }
}

#[test]
fn a_shell_without_a_program_is_the_login_shell_not_a_broken_file() {
    for shell in [
        r#"{"args": ["-l"]}"#,
        r#"{"program": "", "args": []}"#,
        r#"{"program": 5}"#,
        "null",
    ] {
        let text = format!(r#"{{"font_size": 17.0, "shell": {shell}}}"#);
        let cfg: Config = serde_json::from_str(&text).expect("the file still parses");
        assert_eq!(cfg.shell, None, "{shell}");
        assert_eq!(cfg.font_size, 17.0, "the rest of the file is kept");
    }
    let cfg: Config =
        serde_json::from_str(r#"{"shell": {"program": "fish", "args": ["-l"]}}"#).unwrap();
    assert_eq!(
        cfg.shell,
        Some(ShellConfig {
            program: "fish".into(),
            args: vec!["-l".into()]
        })
    );
}

#[test]
fn profile_usage_score_ranks_frequency_and_recency() {
    let now = 100_000_000u64;
    let day = 86_400u64;
    assert_eq!(ProfileUsage::default().score(now), 0.0);
    let a = ProfileUsage {
        count: 10,
        last_used: now,
    };
    let b = ProfileUsage {
        count: 2,
        last_used: now,
    };
    assert!(a.score(now) > b.score(now));
    let recent = ProfileUsage {
        count: 3,
        last_used: now,
    };
    let stale = ProfileUsage {
        count: 3,
        last_used: now - 30 * day,
    };
    assert!(recent.score(now) > stale.score(now));
}

/// A config from before the setting keeps asking exactly what it asked
/// before, and a value this build does not know falls back to that too
/// rather than failing the file.
#[test]
fn confirm_close_defaults_to_asking_when_busy() {
    assert_eq!(Config::default().confirm_close, ConfirmClose::WhenBusy);
    let cfg: Config = serde_json::from_str(r#"{"restore_session": false}"#).unwrap();
    assert_eq!(cfg.confirm_close, ConfirmClose::WhenBusy);
    let cfg: Config = serde_json::from_str(r#"{"confirm_close": "sometimes"}"#).unwrap();
    assert_eq!(cfg.confirm_close, ConfirmClose::WhenBusy);
}

#[test]
fn confirm_close_round_trips_under_its_written_names() {
    for (mode, name) in [
        (ConfirmClose::Never, "never"),
        (ConfirmClose::WhenBusy, "when-busy"),
        (ConfirmClose::Always, "always"),
    ] {
        let cfg = Config {
            confirm_close: mode,
            ..Config::default()
        };
        let json = serde_json::to_value(&cfg).unwrap();
        assert_eq!(json["confirm_close"], name);
        let back: Config = serde_json::from_value(json).unwrap();
        assert_eq!(back.confirm_close, mode);
    }
}

#[test]
fn ssh_warn_on_close_and_frecency_round_trip() {
    let mut cfg = Config::default();
    assert!(!cfg.ssh_warn_on_close);
    cfg.ssh_warn_on_close = true;
    let id = uuid::Uuid::new_v4();
    cfg.ssh_profile_frecency.insert(
        id,
        ProfileUsage {
            count: 4,
            last_used: 42,
        },
    );
    // The count travels in the servers file now (#911), so the round
    // trip is through disk rather than through `config.json`'s serde.
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.json");
    cfg.try_save_to(&config).unwrap();
    let (back, _) = Config::load_from(&config);
    assert!(back.ssh_warn_on_close);
    assert_eq!(back.ssh_profile_frecency.get(&id).unwrap().count, 4);
}

/// The upgrade path, which is the whole reason `link_file_open` is an
/// `Option`: whichever way a config written before this key existed is
/// read, it has to keep doing what it did yesterday.
#[test]
fn link_file_open_takes_its_default_from_the_config_it_upgrades() {
    let mut had_a_command: Config =
        serde_json::from_str(r#"{"link_file_command":"code --goto {path}"}"#).unwrap();
    had_a_command.sanitize();
    assert_eq!(
        had_a_command.file_open_mode(),
        LinkFileOpen::Command,
        "someone who pointed this at their own editor asked for it and keeps it"
    );
    assert_eq!(
        had_a_command.link_file_command.as_deref(),
        Some("code --goto {path}")
    );

    let mut had_none: Config = serde_json::from_str(r#"{"font_size": 15.0}"#).unwrap();
    had_none.sanitize();
    assert_eq!(
        had_none.file_open_mode(),
        LinkFileOpen::Internal,
        "everyone else was silently getting the OS file association"
    );

    // A command that only ever held whitespace is no command at all —
    // `sanitize` drops it, and the mode must not be decided on the corpse.
    let mut blank: Config = serde_json::from_str(r#"{"link_file_command":"   "}"#).unwrap();
    blank.sanitize();
    assert_eq!(blank.link_file_command, None);
    assert_eq!(blank.file_open_mode(), LinkFileOpen::Internal);

    let mut chosen: Config =
        serde_json::from_str(r#"{"link_file_open":"system","link_file_command":"code"}"#)
            .unwrap();
    chosen.sanitize();
    assert_eq!(
        chosen.file_open_mode(),
        LinkFileOpen::System,
        "an explicit choice outranks whatever the command box holds"
    );

    let json = serde_json::to_string(&chosen).unwrap();
    assert!(json.contains("\"link_file_open\":\"system\""), "persisted");
    let back: Config = serde_json::from_str(&json).unwrap();
    assert_eq!(back.file_open_mode(), LinkFileOpen::System);

    // Unknown values fall back rather than refusing the whole file.
    let garbage: Config = serde_json::from_str(r#"{"link_file_open":"emacs"}"#).unwrap();
    assert_eq!(garbage.file_open_mode(), LinkFileOpen::Internal);
}

#[test]
fn removed_sidebar_auto_grouping_key_is_ignored_on_load() {
    let legacy: Config =
        serde_json::from_str(r#"{"sidebar_auto_grouping":false,"font_size":15.0}"#)
            .expect("legacy config with sidebar_auto_grouping still parses");
    assert_eq!(legacy.font_size, 15.0);
    let json = serde_json::to_string(&legacy).unwrap();
    assert!(!json.contains("sidebar_auto_grouping"));
}

#[test]
fn dim_inactive_panes_defaults_on_and_round_trips() {
    assert!(Config::default().dim_inactive_panes);

    let old: Config = serde_json::from_str(r#"{"font_size": 15.0}"#).unwrap();
    assert!(old.dim_inactive_panes);

    let off: Config = serde_json::from_str(r#"{"dim_inactive_panes": false}"#).unwrap();
    assert!(!off.dim_inactive_panes);
    let json = serde_json::to_string(&off).unwrap();
    let back: Config = serde_json::from_str(&json).unwrap();
    assert!(!back.dim_inactive_panes);
}

#[test]
fn auto_hide_titlebar_buttons_defaults_off_and_round_trips() {
    assert!(!Config::default().auto_hide_titlebar_buttons);

    // A file from before the key existed keeps its buttons on screen.
    let old: Config = serde_json::from_str(r#"{"font_size": 15.0}"#).unwrap();
    assert!(!old.auto_hide_titlebar_buttons);

    let on: Config = serde_json::from_str(r#"{"auto_hide_titlebar_buttons": true}"#).unwrap();
    assert!(on.auto_hide_titlebar_buttons);
    let json = serde_json::to_string(&on).unwrap();
    let back: Config = serde_json::from_str(&json).unwrap();
    assert!(back.auto_hide_titlebar_buttons, "persisted");
}

#[test]
fn theme_follow_system_defaults_and_round_trips() {
    let cfg: Config = serde_json::from_str(r#"{"theme_preset":"dracula"}"#).unwrap();
    assert!(!cfg.theme_follow_system);
    assert_eq!(cfg.theme_preset_light, "light");
    assert_eq!(cfg.theme_preset_dark, "dark");
    assert_eq!(cfg.theme_preset, "dracula");
    // The palette rescue defaults on, and survives a round trip either way.
    assert!(cfg.theme_legible_palette);

    let mut cfg = Config::default();
    cfg.theme_follow_system = true;
    cfg.theme_preset_light = "one_light".to_string();
    cfg.theme_preset_dark = "dracula".to_string();
    let json = serde_json::to_string(&cfg).unwrap();
    let back: Config = serde_json::from_str(&json).unwrap();
    assert!(back.theme_follow_system);
    assert_eq!(back.theme_preset_light, "one_light");
    assert_eq!(back.theme_preset_dark, "dracula");

    let off: Config = serde_json::from_str(r#"{"theme_legible_palette":false}"#).unwrap();
    assert!(!off.theme_legible_palette);
    let json = serde_json::to_string(&off).unwrap();
    let back: Config = serde_json::from_str(&json).unwrap();
    assert!(!back.theme_legible_palette);
}

#[test]
fn font_features_are_optional_and_parse_as_gpui_features() {
    let cfg: Config =
        serde_json::from_str(r#"{"font_features":{"calt":true,"liga":1}}"#).unwrap();
    let features = cfg.font_features.expect("font features should parse");
    assert_eq!(features.is_calt_enabled(), Some(true));
    assert!(
        features
            .tag_value_list()
            .iter()
            .any(|(tag, value)| tag == "liga" && *value == 1)
    );

    let default_cfg = Config::default();
    assert!(default_cfg.font_features.is_none());
}

#[test]
fn font_thicken_defaults_on_and_reads_an_explicit_off() {
    // On is what anterm drew before the key existed, so a config that never
    // names it must keep rendering exactly as it did.
    let cfg: Config = serde_json::from_str("{}").unwrap();
    assert!(cfg.font_thicken);
    assert!(Config::default().font_thicken);

    let cfg: Config = serde_json::from_str(r#"{"font_thicken":false}"#).unwrap();
    assert!(!cfg.font_thicken);
}

#[test]
fn font_features_round_trip_to_integer_valued_json() {
    let features: FontFeatures =
        serde_json::from_str(r#"{"calt":true,"liga":1,"ss01":0,"zero":false}"#).unwrap();
    assert_eq!(
        features.tag_value_list(),
        &[
            ("calt".to_string(), 1),
            ("liga".to_string(), 1),
            ("ss01".to_string(), 0),
            ("zero".to_string(), 0),
        ]
    );

    let json = serde_json::to_string(&features).unwrap();
    assert_eq!(json, r#"{"calt":1,"liga":1,"ss01":0,"zero":0}"#);
    let back: FontFeatures = serde_json::from_str(&json).unwrap();
    assert_eq!(back, features);
    assert_eq!(serde_json::to_string(&back).unwrap(), json);
}

#[test]
fn font_features_skip_bad_tags_and_values_instead_of_failing() {
    let features: FontFeatures = serde_json::from_str(
        r#"{"toolong":1,"ss":1,"calt":true,"liga":null,"kern":-1,"dlig":1.5}"#,
    )
    .unwrap();
    assert_eq!(features.tag_value_list(), &[("calt".to_string(), 1)]);
}

#[test]
fn font_features_survive_a_config_round_trip() {
    let cfg: Config =
        serde_json::from_str(r#"{"font_features":{"calt":true,"liga":1}}"#).unwrap();
    let json = serde_json::to_string(&cfg).unwrap();
    let back: Config = serde_json::from_str(&json).unwrap();
    assert_eq!(back.font_features, cfg.font_features);
    assert_eq!(
        back.font_features.as_ref().map(|f| f.tag_value_list()),
        Some(&[("calt".to_string(), 1), ("liga".to_string(), 1)][..])
    );
}

#[test]
fn stale_override_keys_are_ignored() {
    let cfg: Config = serde_json::from_str(
        r##"{"font_size": 20.0, "colors": {"border": "#fff"}, "ansi_colors": {"color1": "#f00"}}"##,
    )
    .expect("stale override keys must be ignored");
    assert_eq!(cfg.font_size, 20.0);
    assert_eq!(cfg.theme_preset, "light");
}

#[test]
fn sanitize_clamps_degenerate_font_metrics() {
    let sanitized = |font_size: f32, line_height: f32| {
        let mut cfg = Config {
            font_size,
            line_height,
            ..Config::default()
        };
        cfg.sanitize();
        (cfg.font_size, cfg.line_height)
    };

    let (fs, lh) = sanitized(0.0, 0.0);
    assert!(fs >= 4.0, "font_size clamped above zero");
    assert!(lh >= 0.5, "line_height clamped above zero");

    let (fs, lh) = sanitized(f32::NAN, f32::INFINITY);
    assert!(fs.is_finite() && fs > 0.0);
    assert!(lh.is_finite() && lh > 0.0);

    assert_eq!(sanitized(15.0, 1.4), (15.0, 1.4));
}

#[test]
fn sanitize_clamps_to_the_same_bounds_the_gui_steps_within() {
    // The GUI used to clamp to its own, narrower pair, so a config-legal
    // value landed outside the stepper's range and one click pushed it the
    // wrong way. The bounds are one shared set now: `AntermApp::set_font_size`
    // and `set_line_height` clamp to these very constants, which is what
    // makes the steppers and the Ctrl+=/Ctrl+- keys agree with the file
    // (#550).
    //
    // The numbers themselves are the support surface
    // `docs/reference/configuration.mdx` publishes, so pin them: narrowing
    // either side is a documented behaviour change, not a refactor.
    assert_eq!((FONT_SIZE_MIN, FONT_SIZE_MAX), (4.0, 256.0));
    assert_eq!((LINE_HEIGHT_MIN, LINE_HEIGHT_MAX), (0.5, 4.0));

    // A value at either edge survives sanitize unchanged, so the stepper
    // has somewhere to stop rather than a value that keeps being rewritten.
    let mut cfg = Config {
        font_size: FONT_SIZE_MIN,
        line_height: LINE_HEIGHT_MAX,
        ..Config::default()
    };
    cfg.sanitize();
    assert_eq!(cfg.font_size, FONT_SIZE_MIN);
    assert_eq!(cfg.line_height, LINE_HEIGHT_MAX);

    // And a value past an edge lands *on* that edge — the direction the
    // step was going — rather than anywhere else.
    let mut cfg = Config {
        font_size: 1_000.0,
        line_height: 0.1,
        ..Config::default()
    };
    cfg.sanitize();
    assert_eq!(cfg.font_size, FONT_SIZE_MAX, "over the top clamps down");
    assert_eq!(
        cfg.line_height, LINE_HEIGHT_MIN,
        "under the floor clamps up"
    );

    // The values the issue was reported with: both are legal, so sanitize
    // leaves them alone, and the steppers now step from where they are.
    let mut cfg = Config {
        font_size: 50.0,
        line_height: 3.0,
        ..Config::default()
    };
    cfg.sanitize();
    assert_eq!((cfg.font_size, cfg.line_height), (50.0, 3.0));
    // Both are inside the range the steppers clamp to, which is the whole
    // point: `50 + 1` and `3.0 - 0.05` are legal, so neither click can be
    // turned around by a clamp.
    assert!(50.0 + 1.0 <= FONT_SIZE_MAX && 3.0 - 0.05 >= LINE_HEIGHT_MIN);
}

#[test]
fn ui_font_family_defaults_to_none_and_sanitizes_blank_to_none() {
    let cfg: Config = serde_json::from_str(r#"{"font_size": 15.0}"#).unwrap();
    assert_eq!(cfg.ui_font_family, None);

    let mut cfg: Config = serde_json::from_str(r#"{"ui_font_family": "  "}"#).unwrap();
    cfg.sanitize();
    assert_eq!(cfg.ui_font_family, None);

    let mut cfg: Config = serde_json::from_str(r#"{"ui_font_family": "Inter"}"#).unwrap();
    cfg.sanitize();
    assert_eq!(cfg.ui_font_family, Some("Inter".to_string()));
}

#[test]
fn a_config_written_before_ui_font_size_existed_keeps_the_chrome_it_had() {
    // The whole interface is laid out against this, so a missing field
    // must not resolve to serde's 0.0 — that collapses every window the
    // user already has, on upgrade, without them touching a setting.
    let cfg: Config = serde_json::from_str(r#"{"font_size": 15.0}"#).unwrap();
    assert_eq!(cfg.ui_font_size, UI_FONT_SIZE_DEFAULT);
}

#[test]
fn sanitize_holds_ui_font_size_to_sizes_the_layout_survives() {
    let sanitized = |ui_font_size: f32| {
        let mut cfg = Config {
            ui_font_size,
            ..Config::default()
        };
        cfg.sanitize();
        cfg.ui_font_size
    };

    assert_eq!(sanitized(0.0), UI_FONT_SIZE_DEFAULT);
    assert_eq!(sanitized(f32::NAN), UI_FONT_SIZE_DEFAULT);
    assert_eq!(sanitized(-3.0), UI_FONT_SIZE_DEFAULT);
    assert_eq!(sanitized(1000.0), UI_FONT_SIZE_MAX);
    assert_eq!(sanitized(2.0), UI_FONT_SIZE_MIN);
    // A size the user actually picked comes back untouched.
    assert_eq!(sanitized(18.0), 18.0);
}

struct TestDir(std::path::PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("anterm-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
#[test]
fn write_atomic_writes_through_a_symlink_instead_of_replacing_it() {
    let dir = TestDir::new("symlink");
    let real = dir.path().join("dotfiles-config.json");
    std::fs::write(&real, b"old").unwrap();
    let link = dir.path().join("config.json");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    write_atomic(&link, b"new").unwrap();
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "config.json must stay a symlink"
    );
    assert_eq!(std::fs::read(&real).unwrap(), b"new");
}

#[cfg(unix)]
#[test]
fn write_atomic_follows_relative_and_dangling_symlinks() {
    let dir = TestDir::new("symlink-relative");
    std::fs::create_dir(dir.path().join("dotfiles")).unwrap();
    let link = dir.path().join("config.json");
    // Relative to the link's own directory, and not created yet.
    std::os::unix::fs::symlink("dotfiles/config.json", &link).unwrap();
    write_atomic(&link, b"fresh").unwrap();
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read(dir.path().join("dotfiles/config.json")).unwrap(),
        b"fresh"
    );
    let leftover: Vec<_> = std::fs::read_dir(dir.path().join("dotfiles"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
        .collect();
    assert!(leftover.is_empty(), "temp file should be renamed away");
}

#[test]
fn write_atomic_replaces_contents_and_leaves_no_temp() {
    let dir = TestDir::new("atomic");
    let target = dir.path().join("data.json");
    write_atomic(&target, b"first").unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "first");
    write_atomic(&target, b"second-longer-and-then-short").unwrap();
    write_atomic(&target, b"3rd").unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "3rd");
    let leftover: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
        .collect();
    assert!(leftover.is_empty(), "temp file should be renamed away");
}

#[test]
fn behavior_enums_fall_back_leniently_on_bad_values() {
    let cfg: Config = serde_json::from_str(
        r#"{"font_size": 20.0, "new_tab_position": "middle", "notify_on_command_finish": "sometimes", "tab_bar_position": "diagonal"}"#,
    )
    .expect("a bad enum value must not fail the whole parse");
    assert_eq!(cfg.font_size, 20.0);
    assert_eq!(cfg.new_tab_position, NewTabPosition::AfterCurrent);
    assert_eq!(cfg.notify_on_command_finish, NotifyMode::Unfocused);
    assert_eq!(cfg.tab_bar_position, TabBarPosition::Left);

    let cfg: Config = serde_json::from_str(
        r#"{"new_tab_position": "end", "notify_on_command_finish": "always", "tab_bar_position": "top"}"#,
    )
    .unwrap();
    assert_eq!(cfg.new_tab_position, NewTabPosition::End);
    assert_eq!(cfg.notify_on_command_finish, NotifyMode::Always);
    assert_eq!(cfg.tab_bar_position, TabBarPosition::Top);
}

#[test]
fn working_directory_defaults_to_inherit_and_parses_kebab() {
    let cfg = Config::default();
    assert_eq!(cfg.working_directory.strategy, WdStrategy::Inherit);
    assert!(cfg.working_directory.path.is_empty());

    let cfg: Config = serde_json::from_str(
        r#"{"working_directory": {"strategy": "custom", "path": "/tmp/x"}}"#,
    )
    .unwrap();
    assert_eq!(cfg.working_directory.strategy, WdStrategy::Custom);
    assert_eq!(cfg.working_directory.path, "/tmp/x");

    let cfg: Config =
        serde_json::from_str(r#"{"working_directory": {"strategy": "elsewhere"}}"#).unwrap();
    assert_eq!(cfg.working_directory.strategy, WdStrategy::Inherit);
}

#[test]
fn sanitize_clamps_scroll_multiplier_into_band() {
    let clamp = |m: f32| {
        let mut cfg = Config {
            mouse_scroll_multiplier: m,
            ..Config::default()
        };
        cfg.sanitize();
        cfg.mouse_scroll_multiplier
    };
    assert_eq!(clamp(1.0), 1.0);
    assert_eq!(clamp(0.0), 1.0);
    assert_eq!(clamp(-3.0), 1.0);
    assert_eq!(clamp(100.0), 10.0);
    assert_eq!(clamp(0.01), 0.1);
}

/// A width the user dropped the divider at has to come back where they left
/// it. `sanitize` holds `document_ratio` to a band; the divider clamps to
/// the same one, through these constants, so nothing it can write is
/// something the next launch moves. The regression this pins: the drag used
/// to clamp in pixels alone, so a column pushed against either edge of a
/// wide window was saved outside the band and reopened hundreds of points
/// from where it was dropped.
#[test]
fn sanitize_holds_document_ratio_to_the_band_the_divider_clamps_to() {
    let clamp = |r: f32| {
        let mut cfg = Config {
            document_ratio: r,
            ..Config::default()
        };
        cfg.sanitize();
        cfg.document_ratio
    };
    // Both edges are legal, so a divider dropped on one has somewhere to
    // stop rather than a value that keeps being rewritten.
    assert_eq!(clamp(DOCUMENT_RATIO_MIN), DOCUMENT_RATIO_MIN);
    assert_eq!(clamp(DOCUMENT_RATIO_MAX), DOCUMENT_RATIO_MAX);
    for stop in DOCUMENT_RATIO_STOPS {
        assert_eq!(clamp(stop), stop, "a named width must survive the file");
    }
    assert_eq!(clamp(0.05), DOCUMENT_RATIO_MIN);
    assert_eq!(clamp(0.95), DOCUMENT_RATIO_MAX);
    assert_eq!(clamp(0.0), default_document_ratio());
    assert_eq!(clamp(-1.0), default_document_ratio());
    assert_eq!(clamp(f32::NAN), default_document_ratio());
}

#[test]
fn sanitize_clamps_window_opacity_override() {
    let clamp = |o: Option<f32>| {
        let mut cfg = Config {
            window_opacity: o,
            ..Config::default()
        };
        cfg.sanitize();
        cfg.window_opacity
    };
    assert_eq!(clamp(None), None);
    assert_eq!(clamp(Some(0.8)), Some(0.8));
    assert_eq!(clamp(Some(0.0)), Some(0.2));
    assert_eq!(clamp(Some(2.0)), Some(1.0));
    assert_eq!(clamp(Some(f32::NAN)), None);
}

#[test]
fn window_backdrop_defaults_and_round_trips_leniently() {
    let cfg = Config::default();
    assert_eq!(cfg.window_backdrop, WindowBackdrop::Auto);

    let text = serde_json::to_string(&Config {
        window_backdrop: WindowBackdrop::MicaAlt,
        ..Config::default()
    })
    .unwrap();
    assert!(text.contains("\"window_backdrop\":\"mica-alt\""));

    let restored: Config = serde_json::from_str(&text).unwrap();
    assert_eq!(restored.window_backdrop, WindowBackdrop::MicaAlt);

    let blur: Config = serde_json::from_str(r#"{"window_backdrop":"blur"}"#).unwrap();
    assert_eq!(blur.window_backdrop, WindowBackdrop::Blur);

    // Unknown values fall back to Auto instead of rejecting the whole config.
    let lenient: Config = serde_json::from_str(r#"{"window_backdrop":"nope"}"#).unwrap();
    assert_eq!(lenient.window_backdrop, WindowBackdrop::Auto);
}

#[test]
fn sanitize_clamps_scrollback_into_band() {
    let clamp = |n: usize| {
        let mut cfg = Config {
            scrollback_limit: n,
            ..Config::default()
        };
        cfg.sanitize();
        cfg.scrollback_limit
    };
    assert_eq!(clamp(0), 100);
    assert_eq!(clamp(10_000), 10_000);
    assert_eq!(clamp(usize::MAX), MAX_SCROLLBACK);
}

#[test]
fn new_terminal_prefs_default_and_parse_leniently() {
    let cfg = Config::default();
    assert!(cfg.restore_session);
    assert!(cfg.mouse_reporting);
    assert!(cfg.prompt_editor);
    assert!(cfg.tab_completion);
    assert!(cfg.history_search);
    assert_eq!(cfg.notify_threshold_secs, 10);
    assert_eq!(cfg.bell, BellMode::Visual);

    let cfg: Config = serde_json::from_str(r#"{"font_size": 15.0}"#).unwrap();
    assert!(cfg.restore_session);
    assert!(cfg.mouse_reporting);
    assert!(cfg.prompt_editor);
    assert!(cfg.tab_completion);
    assert!(cfg.history_search);
    assert_eq!(cfg.notify_threshold_secs, 10);
    assert_eq!(cfg.bell, BellMode::Visual);

    let cfg: Config = serde_json::from_str(r#"{"tab_completion": false}"#).unwrap();
    assert!(!cfg.tab_completion);
    let cfg: Config = serde_json::from_str(r#"{"history_search": false}"#).unwrap();
    assert!(!cfg.history_search);
    let cfg: Config = serde_json::from_str(r#"{"prompt_editor": false}"#).unwrap();
    assert!(!cfg.prompt_editor);

    let cfg: Config = serde_json::from_str(
        r#"{"restore_session": false, "mouse_reporting": false, "bell": "audible"}"#,
    )
    .unwrap();
    assert!(!cfg.restore_session);
    assert!(!cfg.mouse_reporting);
    assert_eq!(cfg.bell, BellMode::Audible);

    let cfg: Config = serde_json::from_str(r#"{"bell": "both"}"#).unwrap();
    assert_eq!(cfg.bell, BellMode::Both);

    let cfg: Config = serde_json::from_str(r#"{"bell": "loud"}"#).unwrap();
    assert_eq!(cfg.bell, BellMode::Visual);

    assert_eq!(serde_json::to_string(&BellMode::Both).unwrap(), "\"both\"");
}

/// #668: a config written on a Mac travels to a Linux box, where the
/// platform modifier is a different key — so the *choice* is stored, not
/// the key it resolves to. Unset, or unknown, the wheel does not zoom.
#[test]
fn the_zoom_modifier_round_trips_and_falls_back() {
    let cfg: Config = serde_json::from_str(r#"{"mouse_zoom_modifier": "none"}"#).unwrap();
    assert_eq!(cfg.mouse_zoom_modifier, MouseZoomModifier::None);

    let cfg: Config = serde_json::from_str(r#"{"mouse_zoom_modifier": "alt"}"#).unwrap();
    assert_eq!(cfg.mouse_zoom_modifier, MouseZoomModifier::Alt);

    let cfg: Config = serde_json::from_str(r#"{"mouse_zoom_modifier": "platform"}"#).unwrap();
    assert_eq!(cfg.mouse_zoom_modifier, MouseZoomModifier::Platform);

    let cfg: Config = serde_json::from_str(r#"{"font_size": 15.0}"#).unwrap();
    assert_eq!(cfg.mouse_zoom_modifier, MouseZoomModifier::None);

    let cfg: Config = serde_json::from_str(r#"{"mouse_zoom_modifier": "meta"}"#).unwrap();
    assert_eq!(cfg.mouse_zoom_modifier, MouseZoomModifier::None);

    assert_eq!(
        serde_json::to_string(&MouseZoomModifier::None).unwrap(),
        "\"none\""
    );
}

#[test]
fn sanitize_clamps_notify_threshold_into_band() {
    let clamp = |n: u64| {
        let mut cfg = Config {
            notify_threshold_secs: n,
            ..Config::default()
        };
        cfg.sanitize();
        cfg.notify_threshold_secs
    };
    assert_eq!(clamp(0), 1);
    assert_eq!(clamp(10), 10);
    assert_eq!(clamp(100_000), 3600);
}

#[test]
fn sanitize_normalizes_blank_http_proxy_to_none() {
    let normalize = |proxy: Option<&str>| {
        let mut cfg = Config {
            http_proxy: proxy.map(String::from),
            ..Config::default()
        };
        cfg.sanitize();
        cfg.http_proxy
    };
    assert_eq!(normalize(None), None);
    assert_eq!(normalize(Some("")), None);
    assert_eq!(normalize(Some("   ")), None);
    assert_eq!(
        normalize(Some("  http://127.0.0.1:7890 ")),
        Some("http://127.0.0.1:7890".to_string())
    );
}

#[test]
fn gui_language_defaults_to_english_and_rejects_unsupported_values() {
    let cfg = Config::default();
    assert_eq!(cfg.gui_language, "en");

    let cfg: Config = serde_json::from_str(r#"{"gui_language": "zh-CN"}"#).unwrap();
    assert_eq!(cfg.gui_language, "zh-CN");

    for unsupported in ["ja-JP", "ru-RU", "ko"] {
        let mut cfg: Config =
            serde_json::from_str(&format!(r#"{{"gui_language": "{unsupported}"}}"#)).unwrap();
        cfg.sanitize();
        assert_eq!(cfg.gui_language, "en");
    }
}

#[test]
fn keybinding_preset_and_prefix_default_and_round_trip() {
    let cfg = Config::default();
    assert_eq!(cfg.keybinding_preset, "default");
    assert_eq!(cfg.prefix, "ctrl-b");

    let cfg: Config = serde_json::from_str(r#"{"font_size": 15.0}"#).unwrap();
    assert_eq!(cfg.keybinding_preset, "default");
    assert_eq!(cfg.prefix, "ctrl-b");

    let cfg: Config =
        serde_json::from_str(r#"{"keybinding_preset": "tmux", "prefix": "ctrl-a"}"#).unwrap();
    assert_eq!(cfg.keybinding_preset, "tmux");
    assert_eq!(cfg.prefix, "ctrl-a");
}

#[test]
fn default_font_fallbacks_are_platform_appropriate() {
    let defaults = default_font_fallbacks();
    assert!(!defaults.is_empty());

    for name in platform_last_resort_fallbacks() {
        assert!(
            defaults.iter().any(|f| f == name),
            "default chain {defaults:?} omits stock face {name}"
        );
    }

    let maple = defaults.iter().position(|f| f == "Maple Mono NF CN");
    let maple = maple.expect("the exact-fit CJK face must stay in the chain");
    for name in platform_last_resort_fallbacks() {
        let stock = defaults.iter().position(|f| f == name).unwrap();
        assert!(maple < stock, "{name} must not preempt Maple Mono NF CN");
    }

    if !cfg!(target_os = "macos") {
        for name in ["Menlo", "Apple Color Emoji"] {
            assert!(
                !defaults.iter().any(|f| f == name),
                "{name} ships only with macOS"
            );
        }
    }
}

#[test]
fn config_deserialize_fills_missing_fields_from_defaults() {
    let cfg: Config = serde_json::from_str(r#"{"font_size": 20.0}"#).unwrap();
    assert_eq!(cfg.font_size, 20.0);
    assert_eq!(cfg.line_height, 1.4);
    assert_eq!(cfg.font_family, "Hack");
    assert_eq!(cfg.theme_preset, "light");
    assert!(cfg.keybindings.is_empty());
}

#[test]
fn keybindings_take_a_chord_or_a_list_and_write_back_what_they_read() {
    // A string is the shape every config written before #868 has, from the
    // Settings page and by hand; a list is the one that replaces an
    // action's chords outright. Both have to load, and a save must not turn
    // one into the other — the two mean different things.
    let written = serde_json::json!({
        "NextTab": "cmd-shift-]",
        "AlternatePaste": "",
        "PrevTab": ["cmd-shift-[", "ctrl-shift-tab"],
        "SplitRight": [],
    });
    let cfg: Config =
        serde_json::from_value(serde_json::json!({ "keybindings": written.clone() }))
            .expect("both shapes load");
    assert_eq!(cfg.keybindings.len(), 4);
    assert_eq!(serde_json::to_value(&cfg.keybindings).unwrap(), written);
}

#[test]
fn a_keybinding_line_that_cannot_be_read_does_not_take_the_config_with_it() {
    // #901: one unreadable line used to fail the whole `Config`. The loader
    // then quarantines config.json and starts on built-in defaults, so
    // every rebinding in the file — the point of the file — came back as
    // the shipped default, and the next settings write made that permanent.
    let cfg: Config = serde_json::from_str(
        r#"{"font_size": 20.0,
            "keybindings": {"ActivateTab1": null, "ActivateTab2": 2,
                            "ActivateTab3": {"key": "alt-shift-3"},
                            "ActivateTab4": [], "NextTab": "ctrl-alt-]"}}"#,
    )
    .expect("a bad keybinding line must not fail the file");
    assert_eq!(cfg.font_size, 20.0, "the rest of the file still loads");
    assert_eq!(
        serde_json::to_value(&cfg.keybindings).unwrap(),
        serde_json::json!({"ActivateTab4": [], "NextTab": "ctrl-alt-]"}),
        "the readable lines survive and the unreadable ones are dropped"
    );

    // And a `keybindings` that is not a map at all is no reason to hand
    // the user back default fonts, themes and everything else.
    let odd: Config = serde_json::from_str(r#"{"font_size": 20.0, "keybindings": []}"#)
        .expect("a keybindings of the wrong shape must not fail the file");
    assert_eq!(odd.font_size, 20.0);
    assert!(odd.keybindings.is_empty());
}

fn pin_config_dir() {
    let dir = std::env::temp_dir().join(format!("anterm-covtest-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok();
    set_config_dir(dir);
}

static CONFIG_FILE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_config_file() -> std::sync::MutexGuard<'static, ()> {
    CONFIG_FILE.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn save_load_and_shell_command_round_trip_through_disk() {
    let _guard = lock_config_file();
    pin_config_dir();
    let mut cfg = Config {
        font_size: 18.0,
        ..Config::default()
    };
    cfg.shell = Some(ShellConfig {
        program: "fish".to_string(),
        args: vec!["-l".to_string()],
    });
    let mut profile = crate::ssh_profile::SshProfile::new("prod-web");
    profile.host = "10.0.0.5".to_string();
    profile.user = "deploy".to_string();
    profile.port = 2222;
    profile.auth = crate::ssh_profile::AuthMode::PublicKey;
    profile.credential_ref = Some(crate::keychain::CredentialRef::password(
        "deploy", "10.0.0.5", 2222,
    ));
    cfg.ssh_profiles = vec![profile.clone()];
    cfg.save();

    let loaded = Config::load();
    assert_eq!(loaded.font_size, 18.0);
    assert_eq!(
        loaded.shell.as_ref().map(|s| s.program.as_str()),
        Some("fish")
    );
    assert_eq!(loaded.ssh_profiles, vec![profile]);

    let (program, args) = shell_command().expect("shell override present");
    assert_eq!(program, "fish");
    assert_eq!(args, vec!["-l".to_string()]);
}

#[test]
fn a_utf8_bom_does_not_silently_reset_the_config() {
    let _guard = lock_config_file();
    pin_config_dir();
    let path = Config::path().expect("pinned config dir");
    let text = "\u{FEFF}{\"font_size\": 21.0, \"restore_session\": false}";
    write_atomic(&path, text.as_bytes()).unwrap();

    let loaded = Config::load();
    assert_eq!(loaded.font_size, 21.0);
    assert!(!loaded.restore_session);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_corrupt_config_is_kept_aside_and_never_overwritten() {
    let _guard = lock_config_file();
    pin_config_dir();
    let path = Config::path().expect("pinned config dir");
    let aside = path.with_extension("json.corrupt");
    clear_quarantines(&path);
    std::fs::write(&path, "{ not json").unwrap();

    let (loaded, outcome) = Config::load_with_outcome();
    assert_eq!(outcome, LoadOutcome::Quarantined);
    assert!(loaded.quarantined, "the stand-in must say what it is");
    assert_eq!(
        std::fs::read_to_string(&aside).as_deref().ok(),
        Some("{ not json"),
        "the next save overwrites config.json wholesale, so the old \
         contents must already be parked beside it"
    );

    // The save every sidebar drag issues must not turn one typo into
    // permanent loss: a quarantined config refuses to write, and the file
    // on disk stays exactly the user's own.
    loaded.save();
    assert_eq!(
        std::fs::read_to_string(&path).as_deref().ok(),
        Some("{ not json"),
        "a quarantined config must never overwrite the file it stood in for"
    );

    // Fixing the file is what re-arms writes.
    std::fs::write(&path, r#"{"font_size": 19.0}"#).unwrap();
    let (fixed, outcome) = Config::load_with_outcome();
    assert_eq!(outcome, LoadOutcome::Parsed);
    assert!(!fixed.quarantined);
    fixed.save();
    assert!(std::fs::read_to_string(&path).unwrap().contains("19.0"));

    std::fs::remove_file(&path).ok();
    clear_quarantines(&path);
}

#[test]
fn a_config_that_stays_broken_is_parked_once_not_once_per_read() {
    let _guard = lock_config_file();
    pin_config_dir();
    let path = Config::path().expect("pinned config dir");
    clear_quarantines(&path);
    std::fs::write(&path, "{ not json").unwrap();

    // `Config::load` runs on every pane spawn and every palette command,
    // so a file left broken is read dozens of times a session. Each read
    // used to leave another copy, filling the config directory and then
    // overwriting the oldest one.
    for _ in 0..12 {
        let _ = Config::load();
    }
    let parked: Vec<_> = quarantine_candidates(&path)
        .filter(|candidate| candidate.exists())
        .collect();
    assert_eq!(
        parked.len(),
        1,
        "one broken file, one copy — found {parked:?}"
    );

    // A *different* broken version is still worth keeping.
    std::fs::write(&path, "{ also not json").unwrap();
    let _ = Config::load();
    let parked: Vec<_> = quarantine_candidates(&path)
        .filter(|candidate| candidate.exists())
        .collect();
    assert_eq!(parked.len(), 2, "found {parked:?}");
    assert_eq!(
        std::fs::read_to_string(&parked[0]).unwrap(),
        "{ not json",
        "the first rescue copy is still the first one"
    );

    std::fs::remove_file(&path).ok();
    clear_quarantines(&path);
}

#[cfg(unix)]
#[test]
fn an_unreadable_config_suppresses_writes_without_parking_a_copy() {
    use std::os::unix::fs::PermissionsExt as _;

    let _guard = lock_config_file();
    pin_config_dir();
    let path = Config::path().expect("pinned config dir");
    clear_quarantines(&path);
    std::fs::write(&path, r#"{"font_size": 21.0}"#).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_to_string(&path).is_ok() {
        std::fs::remove_file(&path).ok();
        return;
    }

    let (loaded, outcome) = Config::load_with_outcome();
    // Not `Quarantined`: nothing was parked, so the notification must not
    // point at a `config.json.corrupt` that was never written.
    assert_eq!(outcome, LoadOutcome::Unreadable);
    assert!(outcome.failed());
    assert!(
        loaded.quarantined,
        "what cannot be read must not be written"
    );
    assert!(
        quarantine_candidates(&path).all(|candidate| !candidate.exists()),
        "there was nothing readable to copy"
    );

    loaded.save();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        r#"{"font_size": 21.0}"#,
        "the file the app could not read is the file the user still has"
    );
    std::fs::remove_file(&path).ok();
}

fn clear_quarantines(path: &std::path::Path) {
    for candidate in quarantine_candidates(path) {
        std::fs::remove_file(candidate).ok();
    }
}

#[test]
fn a_missing_config_file_is_absent_not_quarantined() {
    let _guard = lock_config_file();
    pin_config_dir();
    let path = Config::path().expect("pinned config dir");
    std::fs::remove_file(&path).ok();

    let (loaded, outcome) = Config::load_with_outcome();
    assert_eq!(outcome, LoadOutcome::Absent);
    // A first run saves its defaults without anyone calling that loss.
    assert!(!loaded.quarantined);
}

#[test]
fn the_quarantined_flag_never_reaches_disk() {
    let cfg = Config {
        quarantined: true,
        ..Config::default()
    };
    let text = serde_json::to_string(&cfg).unwrap();
    assert!(!text.contains("quarantined"));
    // And a hand-written `"quarantined": true` in the file does not
    // suppress saves either — the flag belongs to the loader, not the file.
    let parsed: Config =
        serde_json::from_str(r#"{"quarantined": true, "font_size": 20.0}"#).unwrap();
    assert!(!parsed.quarantined);
    assert_eq!(parsed.font_size, 20.0);
}

#[test]
fn strip_bom_only_removes_a_leading_marker() {
    assert_eq!(strip_bom("{}"), "{}");
    assert_eq!(strip_bom("\u{FEFF}{}"), "{}");
    assert_eq!(strip_bom("\u{FEFF}\u{FEFF}{}"), "\u{FEFF}{}");
    let inner = "{\"tab_title\":\"\u{FEFF}\"}";
    assert_eq!(strip_bom(inner), inner);
    assert_eq!(strip_bom(""), "");
}

#[test]
fn ssh_profiles_default_empty_and_parse_from_json() {
    let cfg = Config::default();
    assert!(cfg.ssh_profiles.is_empty());
    let cfg: Config = serde_json::from_str(r#"{"font_size": 15.0}"#).unwrap();
    assert!(cfg.ssh_profiles.is_empty());

    let cfg: Config = serde_json::from_str(
        r#"{"ssh_profiles":[{"name":"a","host":"h","auth":"bogus","port":2200}]}"#,
    )
    .expect("a bad per-profile enum must not fail the whole config parse");
    assert_eq!(cfg.ssh_profiles.len(), 1);
    assert_eq!(cfg.ssh_profiles[0].name, "a");
    assert_eq!(cfg.ssh_profiles[0].port, 2200);
    assert_eq!(
        cfg.ssh_profiles[0].auth,
        crate::ssh_profile::AuthMode::Auto
    );
}

#[test]
fn a_mistyped_custom_shell_does_not_cost_the_user_the_rest_of_the_config() {
    let cfg = Config::default();
    assert!(cfg.custom_shells.is_empty());

    let cfg: Config =
        serde_json::from_str(r#"{"font_size": 15.0, "custom_shells": {"Ubuntu": "wsl.exe"}}"#)
            .expect("a bad custom_shells must not fail the whole config parse");
    assert!(cfg.custom_shells.is_empty());
    // The point of the leniency: `Config::load` hands back defaults for a
    // config that fails to parse, and the next settings write persists them
    // over the file. Everything the user wrote beside the typo survives.
    assert_eq!(cfg.font_size, 15.0);

    let cfg: Config = serde_json::from_str(
        r#"{"custom_shells":[{"label":"Ubuntu","program":"wsl.exe","args":["-d","Ubuntu"]}]}"#,
    )
    .expect("a well-formed list parses");
    assert_eq!(cfg.custom_shells.len(), 1);
    assert_eq!(cfg.custom_shells[0].program, "wsl.exe");
    assert_eq!(cfg.custom_shells[0].args, ["-d", "Ubuntu"]);
}

#[test]
fn config_path_resolves_under_the_pinned_dir() {
    pin_config_dir();
    let p = config_path("config.json").expect("config path resolves");
    assert!(p.ends_with("config.json"));
    assert_eq!(p.parent(), config_dir_path().as_deref());
}

#[test]
fn config_dir_resolution_prefers_explicit_over_portable_over_default() {
    let dir = |name: &str| Some(PathBuf::from(name));
    let default = || dir("default");

    assert_eq!(
        resolve_config_dir(dir("flag"), dir("env"), dir("portable"), default),
        dir("flag"),
        "--config-dir wins over everything"
    );
    assert_eq!(
        resolve_config_dir(None, dir("env"), dir("portable"), default),
        dir("env"),
        "$ANTERM_CONFIG_DIR still redirects a portable copy"
    );
    assert_eq!(
        resolve_config_dir(None, None, dir("portable"), default),
        dir("portable"),
        "a portable install keeps its data beside the executables"
    );
    assert_eq!(
        resolve_config_dir(None, None, None, default),
        dir("default")
    );
    assert_eq!(resolve_config_dir(None, None, None, || None), None);
}

#[test]
fn portable_data_needs_both_the_marker_and_the_folder() {
    let exe_dir = tempfile::tempdir().unwrap();
    let data = exe_dir.path().join(PORTABLE_DATA_DIR);

    assert_eq!(portable_data_dir(exe_dir.path()), None, "a plain directory");

    // Every portable ZIP ships the marker for the updater, so the marker
    // alone must not relocate an existing portable user's data.
    std::fs::write(exe_dir.path().join(PORTABLE_MARKER), "portable-v1").unwrap();
    assert_eq!(portable_data_dir(exe_dir.path()), None, "marker only");

    std::fs::create_dir(&data).unwrap();
    assert_eq!(portable_data_dir(exe_dir.path()), Some(data.clone()));

    // A `data` folder without the marker — an installer layout, or any
    // directory someone ran a build from — is not a portable install.
    std::fs::remove_file(exe_dir.path().join(PORTABLE_MARKER)).unwrap();
    assert_eq!(portable_data_dir(exe_dir.path()), None, "folder only");
}

#[test]
fn a_file_named_data_is_not_a_portable_data_folder() {
    let exe_dir = tempfile::tempdir().unwrap();
    std::fs::write(exe_dir.path().join(PORTABLE_MARKER), "portable-v1").unwrap();
    std::fs::write(exe_dir.path().join(PORTABLE_DATA_DIR), "").unwrap();
    assert_eq!(portable_data_dir(exe_dir.path()), None);
}

#[test]
fn launch_overrides_teach_the_daemon_their_wrappers_as_the_file_changes() {
    let _guard = lock_config_file();
    pin_config_dir();
    let mut cfg = Config::default();
    cfg.agent_launch
        .insert("claude".to_string(), "cc --fast".to_string());
    cfg.save();
    let aliases = agent_detection_aliases();
    assert_eq!(aliases.get("cc").map(String::as_str), Some("claude"));

    // An edit made while the daemon runs is picked up on the next read,
    // and an explicit `agent_commands` entry outranks an implied one.
    cfg.agent_launch
        .insert("codex".to_string(), "/opt/bin/cx".to_string());
    cfg.agent_commands
        .insert("CC".to_string(), "gemini".to_string());
    cfg.save();
    let aliases = agent_detection_aliases();
    assert_eq!(aliases.get("cc").map(String::as_str), Some("gemini"));
    assert_eq!(aliases.get("cx").map(String::as_str), Some("codex"));

    let back: Config = serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
    assert_eq!(back.agent_launch, cfg.agent_launch);
    Config::default().save();
}

fn host(name: &str, address: &str) -> crate::ssh_profile::SshProfile {
    let mut profile = crate::ssh_profile::SshProfile::new(name);
    profile.host = address.to_string();
    profile.user = "deploy".to_string();
    profile
}

/// A `config.json` from before #911, holding hosts, a usage count, a key
/// this build has never heard of, and the handful of settings the user
/// actually wrote.
fn legacy_config(profile: &crate::ssh_profile::SshProfile) -> String {
    let usage = serde_json::json!({ profile.id.to_string(): { "count": 3, "last_used": 5 } });
    serde_json::to_string_pretty(&serde_json::json!({
        "font_size": 18.0,
        "a_key_from_a_newer_build": { "x": 1 },
        "ssh_profiles": [profile],
        "ssh_profile_frecency": usage,
        "keybindings": { "SplitRight": "cmd-d" },
    }))
    .unwrap()
}

#[test]
fn the_hosts_move_out_of_config_json_once_and_nothing_else_moves() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.json");
    let servers = dir.path().join(SERVERS_FILE);
    let profile = host("prod-web", "10.0.0.5");
    std::fs::write(&config, legacy_config(&profile)).unwrap();

    let (cfg, outcome) = Config::load_from(&config);
    assert_eq!(outcome, LoadOutcome::Parsed);
    assert_eq!(cfg.ssh_profiles, vec![profile.clone()]);
    assert_eq!(cfg.ssh_profile_frecency[&profile.id].count, 3);
    assert_eq!(cfg.font_size, 18.0);

    let moved: ServersFile =
        serde_json::from_str(&std::fs::read_to_string(&servers).unwrap()).unwrap();
    assert_eq!(moved.ssh_profiles, vec![profile.clone()]);
    assert_eq!(moved.ssh_profile_frecency[&profile.id].count, 3);

    let left: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    let left = left.as_object().unwrap();
    assert!(!left.contains_key("ssh_profiles"));
    assert!(!left.contains_key("ssh_profile_frecency"));
    // The rest of the file is exactly what it was: nothing the user did
    // not write appears, and nothing this build cannot read disappears.
    assert_eq!(left["font_size"], serde_json::json!(18.0));
    assert_eq!(
        left["a_key_from_a_newer_build"],
        serde_json::json!({ "x": 1 })
    );
    assert_eq!(
        left["keybindings"],
        serde_json::json!({ "SplitRight": "cmd-d" })
    );
    assert_eq!(left.len(), 3, "the move must not expand the file: {left:?}");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&servers).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // Once is once: a second load reads the new file and writes nothing.
    let config_after = std::fs::read(&config).unwrap();
    let servers_after = std::fs::read(&servers).unwrap();
    let (again, _) = Config::load_from(&config);
    assert_eq!(again.ssh_profiles, vec![profile]);
    assert_eq!(std::fs::read(&config).unwrap(), config_after);
    assert_eq!(std::fs::read(&servers).unwrap(), servers_after);
}

#[test]
fn with_both_files_present_the_servers_file_wins() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.json");
    let servers = dir.path().join(SERVERS_FILE);
    let current = host("current", "10.0.0.1");
    let stale = host("stale", "10.0.0.2");
    let cfg = Config {
        ssh_profiles: vec![current.clone()],
        ..Config::default()
    };
    cfg.try_save_to(&config).unwrap();
    // An older anterm, or a synced file, puts hosts back in config.json.
    std::fs::write(&config, legacy_config(&stale)).unwrap();
    let servers_before = std::fs::read(&servers).unwrap();

    let (loaded, _) = Config::load_from(&config);
    assert_eq!(loaded.ssh_profiles, vec![current.clone()]);
    assert!(loaded.ssh_profile_frecency.is_empty());
    assert_eq!(std::fs::read(&servers).unwrap(), servers_before);
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        legacy_config(&stale),
        "loading is not the moment to rewrite config.json"
    );

    // The stale copy falls out at the next ordinary save.
    loaded.try_save_to(&config).unwrap();
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(!text.contains("ssh_profiles"), "{text}");
    assert_eq!(Config::load_from(&config).0.ssh_profiles, vec![current]);
}

#[test]
fn a_config_json_without_hosts_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.json");
    std::fs::write(&config, "{\"font_size\": 18}").unwrap();

    let (cfg, _) = Config::load_from(&config);
    assert!(cfg.ssh_profiles.is_empty());
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        "{\"font_size\": 18}"
    );
    assert!(!dir.path().join(SERVERS_FILE).exists());
}

#[test]
fn an_empty_hosts_key_is_dropped_without_making_a_servers_file() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.json");
    std::fs::write(&config, "{\"font_size\": 18, \"ssh_profiles\": []}").unwrap();

    Config::load_from(&config);
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(!text.contains("ssh_profiles"), "{text}");
    assert!(text.contains("font_size"), "{text}");
    assert!(!dir.path().join(SERVERS_FILE).exists());
}

#[test]
fn saving_writes_the_hosts_to_their_own_file_and_nowhere_else() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.json");
    let servers = dir.path().join(SERVERS_FILE);

    // No hosts, no file.
    Config::default().try_save_to(&config).unwrap();
    assert!(!servers.exists());

    let profile = host("prod-web", "10.0.0.5");
    let mut cfg = Config {
        ssh_profiles: vec![profile.clone()],
        ..Config::default()
    };
    cfg.ssh_profile_frecency.insert(
        profile.id,
        ProfileUsage {
            count: 2,
            last_used: 9,
        },
    );
    cfg.try_save_to(&config).unwrap();
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(!text.contains("ssh_profile"), "{text}");
    assert!(!text.contains("10.0.0.5"), "{text}");

    let (loaded, _) = Config::load_from(&config);
    assert_eq!(loaded.ssh_profiles, vec![profile]);
    assert_eq!(loaded.ssh_profile_frecency, cfg.ssh_profile_frecency);

    // Deleting the last host empties the file rather than leaving it be.
    let mut emptied = loaded.clone();
    emptied.ssh_profiles.clear();
    emptied.ssh_profile_frecency.clear();
    emptied.try_save_to(&config).unwrap();
    assert!(Config::load_from(&config).0.ssh_profiles.is_empty());
}

#[test]
fn a_broken_servers_file_is_kept_aside_and_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.json");
    let servers = dir.path().join(SERVERS_FILE);
    let profile = host("prod-web", "10.0.0.5");
    std::fs::write(&config, legacy_config(&profile)).unwrap();
    std::fs::write(&servers, "{ nope").unwrap();

    let (cfg, outcome) = Config::load_from(&config);
    assert_eq!(outcome, LoadOutcome::Parsed, "config.json itself is fine");
    assert!(cfg.servers_unreadable);
    assert_eq!(
        std::fs::read_to_string(servers.with_extension("json.corrupt")).unwrap(),
        "{ nope"
    );
    // Nothing moved on top of it, and nothing saves over either file.
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        legacy_config(&profile)
    );
    assert!(cfg.try_save_to(&config).is_err());
    assert_eq!(std::fs::read_to_string(&servers).unwrap(), "{ nope");
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        legacy_config(&profile)
    );
}

#[test]
fn the_hosts_never_serialize_into_config_json_but_an_old_file_still_reads() {
    let mut cfg: Config = serde_json::from_str(
        r#"{"ssh_profiles":[{"name":"a","host":"h"}],"verify_host_keys":false}"#,
    )
    .unwrap();
    assert_eq!(cfg.ssh_profiles.len(), 1);
    assert!(!cfg.verify_host_keys);
    cfg.ssh_profile_frecency
        .insert(cfg.ssh_profiles[0].id, ProfileUsage::default());
    let value = serde_json::to_value(&cfg).unwrap();
    assert!(value.get("ssh_profiles").is_none());
    assert!(value.get("ssh_profile_frecency").is_none());
    // The SSH preferences are settings like any other and stay put.
    assert_eq!(value["verify_host_keys"], serde_json::json!(false));
}

#[test]
fn prompt_cursor_style_defaults_to_following_cursor_style() {
    assert_eq!(
        Config::default().prompt_cursor_style,
        PromptCursorStyle::Follow
    );
    assert_eq!(PromptCursorStyle::Follow.shape(), None);

    let cfg: Config =
        serde_json::from_str(r#"{"cursor_style": "block", "prompt_cursor_style": "bar"}"#)
            .unwrap();
    assert_eq!(cfg.cursor_style, CursorStyle::Block);
    assert_eq!(cfg.prompt_cursor_style.shape(), Some(CursorStyle::Bar));

    // A value this build does not know falls back rather than failing the
    // whole config.
    let cfg: Config = serde_json::from_str(r#"{"prompt_cursor_style": "beam"}"#).unwrap();
    assert_eq!(cfg.prompt_cursor_style, PromptCursorStyle::Follow);
}
