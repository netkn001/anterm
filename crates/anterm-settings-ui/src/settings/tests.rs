use super::*;
use anterm_core::ssh_profile::HostPort;

fn locale_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn the_catalog_has_unique_titles_and_default_values_are_unmodified() {
    let defaults = Config::default();
    for (i, entry) in settings_search_entries().iter().enumerate() {
        assert!(
            !settings_search_entries()[..i]
                .iter()
                .any(|e| e.title == entry.title),
            "duplicate {:?}",
            entry.title
        );
        assert!(
            !entry.modified(&defaults),
            "default {:?} is modified",
            entry.title
        );
        assert!(SettingsSection::ALL.contains(&entry.section));
    }
    assert_eq!(SettingsSection::ALL.len(), 7);
    assert!(!SettingsSection::ALL.contains(&SettingsSection::Keybindings));
}

#[test]
fn config_keys_and_cross_language_names_reach_the_same_setting() {
    let _locale = locale_lock();
    for locale in ["en", "zh-CN"] {
        anterm_i18n::set_locale(locale);
        for (query, title, section) in [
            (
                "gui_language",
                L10nKey::SettingsLanguage,
                SettingsSection::General,
            ),
            (
                "mouse_zoom_modifier",
                L10nKey::SettingsMouseZoom,
                SettingsSection::KeyboardMouse,
            ),
            (
                "per_pane_history",
                L10nKey::SettingsPerPaneHistory,
                SettingsSection::Terminal,
            ),
            (
                "ui_font_size",
                L10nKey::SettingsUiFontSize,
                SettingsSection::Appearance,
            ),
            (
                "Shell program",
                L10nKey::SettingsProgram,
                SettingsSection::Terminal,
            ),
        ] {
            let entry = settings_search_entries()
                .iter()
                .find(|e| e.title == title)
                .unwrap();
            assert!(entry_matches(entry, query), "{locale}: {query}");
            assert_eq!(
                best_matching_section(query).unwrap().profile_label(),
                section.profile_label()
            );
        }
    }
    anterm_i18n::set_locale("en");
}

#[test]
fn settings_row_ids_survive_language_changes() {
    let _locale = locale_lock();
    for key in [
        L10nKey::SettingsFontSize,
        L10nKey::SettingsLanguage,
        L10nKey::SettingsMouseZoom,
    ] {
        anterm_i18n::set_locale("en");
        let expected = settings_row_id(t(key), "");
        for locale in ["zh-CN"] {
            anterm_i18n::set_locale(locale);
            assert_eq!(settings_row_id(t(key), ""), expected);
        }
    }
    anterm_i18n::set_locale("en");
}

#[test]
fn modified_settings_compare_their_own_values_only() {
    let mut cfg = Config::default();
    cfg.notify_threshold_secs += 1;
    let changed = settings_search_entries()
        .iter()
        .filter(|e| e.modified(&cfg))
        .map(|e| e.title)
        .collect::<Vec<_>>();
    assert_eq!(changed, vec![L10nKey::SettingsNotifyThreshold]);
}

/// The title-bar switch is found under Appearance by its config key and by
/// its own name in every language, and flipping it marks that row — and
/// only that row — as changed, so its reset link resets the right thing.
#[test]
fn the_titlebar_buttons_switch_is_found_by_key_and_name_and_marks_only_itself() {
    let _locale = locale_lock();
    let entry = settings_search_entries()
        .iter()
        .find(|e| e.title == L10nKey::SettingsAutoHideTitlebarButtons)
        .unwrap();
    assert!(entry.section == SettingsSection::Appearance);
    for locale in ["en", "zh-CN"] {
        anterm_i18n::set_locale(locale);
        let name = t(L10nKey::SettingsAutoHideTitlebarButtons);
        for query in ["auto_hide_titlebar_buttons", name] {
            assert!(entry_matches(entry, query), "{locale}: {query}");
            assert_eq!(
                best_matching_section(query).unwrap().profile_label(),
                SettingsSection::Appearance.profile_label(),
                "{locale}: {query}"
            );
        }
    }
    anterm_i18n::set_locale("en");

    let mut cfg = Config::default();
    cfg.auto_hide_titlebar_buttons = true;
    let changed = settings_search_entries()
        .iter()
        .filter(|e| e.modified(&cfg))
        .map(|e| e.title)
        .collect::<Vec<_>>();
    assert_eq!(changed, vec![L10nKey::SettingsAutoHideTitlebarButtons]);
}

/// A shortcut is the first thing someone searching a settings window for a
/// feature by name is after, and the Keybindings page was the one page the
/// search could not see into: searching "split" found the settings that
/// merely mention splits and never the row labelled exactly that (#444).
#[test]
fn searching_for_a_feature_finds_its_shortcut() {
    let _locale = locale_lock();
    anterm_i18n::set_locale("en");
    let kb = SettingsSection::Keybindings;

    // Split Right and Split Down, at least — the index carries no entry
    // for either, so before this every one of these counts was zero.
    assert!(
        section_match_count(kb, "split") >= 2,
        "got {}",
        section_match_count(kb, "split")
    );
    assert!(keybinding_matches_query("SplitRight", "split right"));

    // The action name is what the docs and `keybindings.json` spell, so a
    // reader arriving from either finds the row they read about.
    assert!(keybinding_matches_query("ScmSync", "scmsync"));

    // An empty query matches no row, or clearing the box would filter the
    // page down to nothing rather than back to every row. (The count above
    // it answers `contains("")` for the one indexed entry this section has
    // always carried, which is why the page gates on the query itself
    // before it consults either.)
    assert!(!keybinding_matches_query("SplitRight", ""));
    assert_eq!(keybinding_match_count(""), 0);

    // A query this page cannot answer leaves it alone — the page only
    // filters itself when it has something to show.
    assert_eq!(section_match_count(kb, "no such action anywhere"), 0);
}

/// The password lives in the keychain under the address, not in the
/// profile — so saving has to decide two things the config file cannot
/// record: whether the entry the form read is now stranded, and whether
/// there is anything new to write.
#[test]
fn saving_moves_a_password_with_the_address_it_belongs_to() {
    let plan = |was, typed, moved| password_plan(was, typed, moved);

    // A form nobody typed in writes nothing at all.
    assert_eq!(
        plan("hunter2", "hunter2", false),
        PasswordPlan {
            drop_old: false,
            store: false
        }
    );
    // A new secret replaces the old one in place.
    assert_eq!(
        plan("hunter2", "correct horse", false),
        PasswordPlan {
            drop_old: false,
            store: true
        }
    );
    // Clearing the box is how a saved password is let go of.
    assert_eq!(
        plan("hunter2", "", false),
        PasswordPlan {
            drop_old: true,
            store: false
        }
    );
    // Retargeting the host carries the secret across and leaves nothing
    // behind under the old address — even when the secret itself is
    // untouched, because the account it is filed under is the address.
    assert_eq!(
        plan("hunter2", "hunter2", true),
        PasswordPlan {
            drop_old: true,
            store: true
        }
    );
    // A host that never had one, and still does not.
    assert_eq!(
        plan("", "", true),
        PasswordPlan {
            drop_old: false,
            store: false
        }
    );
    // The first password a host is given.
    assert_eq!(
        plan("", "hunter2", false),
        PasswordPlan {
            drop_old: false,
            store: true
        }
    );
}

/// A key picked from the system dialog arrives as an absolute path under
/// the home directory. Written back that way it names the right file on
/// this machine and the wrong one everywhere else — and `~` is how the
/// rest of the field, and `~/.ssh/config` itself, spells it.
#[test]
fn a_picked_key_is_written_the_way_the_config_spells_it() {
    let home = Some("/Users/ada");
    assert_eq!(
        tildify_with("/Users/ada/.ssh/id_ed25519", home),
        "~/.ssh/id_ed25519"
    );
    // Outside the home directory there is nothing to shorten.
    assert_eq!(tildify_with("/etc/ssh/key", home), "/etc/ssh/key");
    // And a sibling that merely starts with the same letters is not
    // inside it.
    assert_eq!(
        tildify_with("/Users/adalovelace/key", home),
        "/Users/adalovelace/key"
    );
    // A trailing separator on the home directory changes nothing.
    assert_eq!(
        tildify_with("/Users/ada/.ssh/id_rsa", Some("/Users/ada/")),
        "~/.ssh/id_rsa"
    );
    // Nowhere to anchor against leaves the path as it came.
    assert_eq!(
        tildify_with("/Users/ada/.ssh/id_rsa", None),
        "/Users/ada/.ssh/id_rsa"
    );
}

/// The passphrase box is about one key at a time: the first one named that
/// is actually on disk, with `%h` and `%r` filled in the way the daemon
/// will fill them. A path that is not there can hold no passphrase.
#[test]
fn the_passphrase_follows_the_first_key_that_is_really_there() {
    let dir = std::env::temp_dir().join(format!("anterm-keyform-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let real = dir.join("id_example.com");
    std::fs::write(&real, b"key").unwrap();
    let missing = dir.join("absent").to_string_lossy().to_string();
    let pattern = dir.join("id_%h").to_string_lossy().to_string();

    assert_eq!(
        first_readable_key_in(&[missing.clone(), pattern], "example.com", "ada"),
        Some(real.to_string_lossy().to_string())
    );
    assert_eq!(
        first_readable_key_in(&[missing.clone()], "example.com", "ada"),
        None
    );
    // An empty field falls back to the defaults the handshake offers —
    // and a named key, even a missing one, replaces them entirely.
    let defaults = || vec![missing.clone(), real.to_string_lossy().to_string()];
    assert_eq!(
        first_readable_key_or(&[], "example.com", "ada", defaults),
        Some(real.to_string_lossy().to_string())
    );
    assert_eq!(
        first_readable_key_or(&[missing.clone()], "example.com", "ada", defaults),
        None
    );
    assert_eq!(
        first_readable_key_or(&[], "example.com", "ada", Vec::new),
        None
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_row_stacks_once_its_label_and_control_stop_fitting() {
    assert!(settings_row_width(1440., 1.) >= STACK_ROW_BELOW);
    assert!(settings_row_width(900., 1.) >= STACK_ROW_BELOW);
    // At the narrowest window that turns up the nav gives back what it can
    // and the page is still under the width a label and a control share.
    assert_eq!(nav_width(640., 1.), NAV_W_MIN);
    assert!(settings_row_width(640., 1.) < STACK_ROW_BELOW);
    // Capped at the reading column, so a wider window never widens the row.
    assert_eq!(settings_row_width(4000., 1.), READING_COLUMN);
    // And never goes negative on a window narrower than its own chrome.
    assert_eq!(settings_row_width(100., 1.), 0.);
}

/// A preset row lights up the bucket the value *is*, and nothing when it
/// is none of them — the range match it used to do labelled a hand-set
/// value with a number the config did not hold, and the row carried no
/// digits anywhere to correct it (#550).
#[test]
fn a_preset_row_highlights_only_the_bucket_the_value_actually_is() {
    // The default lands on a bucket, so the common case still reads as a
    // plain radio row.
    let (sel, custom) = preset_choice(
        &SCROLLBACK_BUCKETS,
        Config::default().scrollback_limit,
        group_thousands,
    );
    assert_eq!((sel, custom), (Some(1), None));

    // 50,000 is the value `docs/reference/configuration.mdx` puts in its
    // example config, so this is what following the documentation shows.
    let (sel, custom) = preset_choice(&SCROLLBACK_BUCKETS, 50_000, group_thousands);
    assert_eq!(sel, None, "50,000 is not one of the presets");
    let custom = custom.expect("a value off the presets names itself");
    assert!(
        custom.contains("50,000"),
        "the custom cell has to carry the real value, got {custom:?}"
    );

    // Boundaries: the old range match lit "10,000" for everything from
    // 1,001 up, and "100,000" for everything above that.
    assert_eq!(
        preset_choice(&SCROLLBACK_BUCKETS, 1_001, group_thousands).0,
        None
    );
    assert_eq!(
        preset_choice(&SCROLLBACK_BUCKETS, 100_000, group_thousands).0,
        Some(2)
    );

    // Same rule on the notify row, where 20s used to light up "30s".
    let (sel, custom) = preset_choice(&NOTIFY_THRESHOLD_BUCKETS, 20, |secs| format!("{secs}s"));
    assert_eq!(sel, None);
    assert!(custom.is_some_and(|c| c.contains("20s")));
    assert_eq!(
        preset_choice(&NOTIFY_THRESHOLD_BUCKETS, 60, |secs| format!("{secs}s")).0,
        Some(3),
        "60s is the '1m' cell, not a custom value"
    );
}

/// Each preset cell has to name the number clicking it writes, and the
/// custom cell has to be written the same way as the cells beside it.
#[test]
fn preset_row_labels_name_the_value_they_write() {
    assert_eq!(SCROLLBACK_BUCKETS.len(), SCROLLBACK_LABELS.len());
    for (bucket, label) in SCROLLBACK_BUCKETS.iter().zip(SCROLLBACK_LABELS) {
        assert_eq!(group_thousands(*bucket), label);
    }
    assert_eq!(
        NOTIFY_THRESHOLD_BUCKETS.len(),
        NOTIFY_THRESHOLD_LABELS.len()
    );
    // Grouping starts at four digits and repeats every three.
    assert_eq!(group_thousands(0), "0");
    assert_eq!(group_thousands(999), "999");
    assert_eq!(group_thousands(1_000_000), "1,000,000");
}

/// The thresholds are widths a *label* needs, and a reader who scaled the
/// interface up scaled every label with it while the slider beside it kept
/// the px width it was built at. A window that reads fine at the default
/// font is a starved label column at the largest one.
#[test]
fn the_stacking_width_follows_the_interface_font() {
    use anterm_ui_kit::config::UI_FONT_SIZE_MAX;
    let large = UI_FONT_SIZE_MAX / UI_FONT_SIZE_DEFAULT;
    // Side by side at the default font...
    assert!(settings_row_width(900., 1.) >= STACK_ROW_BELOW);
    // ...and stacked at the largest, where the same row holds half as much.
    assert!(
        settings_row_width(900., large) < STACK_ROW_BELOW * large,
        "a 900pt window at the largest interface font has to stack"
    );
    // The nav keeps its labels readable at the larger font.
    assert!(nav_width(1440., large) > NAV_W);
    // The reading column grows with the font, so a wide window does not.
    assert!(settings_row_width(1600., large) >= STACK_ROW_BELOW * large);
}

#[test]
fn a_row_is_marked_by_its_label_or_by_the_keywords_behind_it() {
    // Straight label hit.
    assert!(row_matches_query(
        SettingsSection::Appearance,
        "Blur",
        "blur"
    ));
    // Keyword hit: the label says "Theme" and nothing more, but the index
    // says that row answers "palette".
    assert!(row_matches_query(
        SettingsSection::Appearance,
        t(L10nKey::SettingsThemeIntroTitle),
        "palette"
    ));
    // A row on some other page is not a hit just because the query matches
    // an entry elsewhere.
    assert!(!row_matches_query(
        SettingsSection::Terminal,
        "Blur",
        "palette"
    ));
    // An empty query marks nothing at all, so no page ever renders greyed
    // out just because the field is focused.
    assert!(!row_matches_query(SettingsSection::Appearance, "Blur", ""));
}

#[test]
fn a_query_that_matches_nothing_is_distinguishable_from_one_that_does() {
    assert_eq!(total_match_count("zzqqxx"), 0);
    assert!(total_match_count("blur") > 0);
    assert!(total_match_count("palette") > 0);
}

#[test]
fn settings_row_identity_depends_only_on_its_stable_label() {
    assert_eq!(
        settings_row_id("Claude Code", "Installingâ€¦"),
        settings_row_id("Claude Code", "Installed in C:\\tools")
    );
    assert_ne!(
        settings_row_id("Claude Code", "Installed"),
        settings_row_id("Codex", "Installed")
    );
}

#[test]
fn synced_windows_backdrop_is_only_a_local_override_on_windows() {
    let mut config = Config::default();
    config.window_backdrop = WindowBackdrop::MicaAlt;

    assert!(window_overrides_active(&config, true));
    assert!(!window_overrides_active(&config, false));
}

#[test]
fn opacity_and_blur_are_local_overrides_on_every_platform() {
    let mut opacity = Config::default();
    opacity.window_opacity = Some(0.8);
    opacity.window_backdrop = WindowBackdrop::Mica;
    let mut blur = Config::default();
    blur.window_blur = Some(true);
    blur.window_backdrop = WindowBackdrop::Acrylic;

    assert!(window_overrides_active(&opacity, false));
    assert!(window_overrides_active(&blur, false));
}

#[test]
fn every_section_has_search_entries() {
    for section in SettingsSection::ALL {
        let n = settings_search_entries()
            .iter()
            .filter(|e| e.section == section)
            .count();
        assert!(
            n > 0,
            "section {:?} has no search entries",
            section.profile_label()
        );
    }
}

#[test]
fn best_matching_section_can_reach_every_section() {
    for section in SettingsSection::ALL {
        let entry = settings_search_entries()
            .iter()
            .find(|e| e.section == section)
            .expect("checked by every_section_has_search_entries");
        let query = t(entry.title).to_lowercase();
        let landed = best_matching_section(&query);
        assert!(
            landed.is_some(),
            "query {query:?} matched nothing at all (section {:?})",
            section.profile_label()
        );
    }
}

#[test]
fn previously_unsearchable_settings_are_findable() {
    use SettingsSection::*;
    let cases: Vec<(&str, SettingsSection)> = vec![
        ("opacity", Appearance),
        ("blur", Appearance),
        ("completion", Terminal),
        ("ctrl-r", Terminal),
        ("threshold", General),
        ("report mouse", KeyboardMouse),
        ("nushell", Terminal),
        ("open files with", Terminal),
        ("bell", Terminal),
        ("known_hosts", Ssh),
        ("claude", Agents),
        // Rows the index had no entry for at all, so the query counted
        // nothing, no badge appeared and no row lit up: the whole Updates
        // group, and Smooth scrolling between two rows that were both
        // findable.
        ("smooth", Terminal),
        ("nightly", About),
        ("channel", About),
        ("metered", About),
        ("automatic", About),
        // A headline feature the index had never heard of: "background
        // image" matched nothing, and typing it walked the page to About
        // because "background" alone hits Download updates in the
        // background.
        ("background image", Appearance),
        ("wallpaper", Appearance),
        ("image opacity", Appearance),
    ];
    #[cfg(target_os = "windows")]
    let cases = cases.into_iter().chain([
        ("material", Appearance),
        ("mica", Appearance),
        ("acrylic", Appearance),
    ]);
    for (query, expected) in cases {
        assert_eq!(
            best_matching_section(query).map(|s| s.profile_label()),
            Some(expected.profile_label()),
            "query {query:?} should land on {:?}",
            expected.profile_label()
        );
    }
}

#[test]
fn index_titles_match_rendered_row_labels() {
    for title in [
        "Starting directory",
        "Restore last layout",
        "Terminal bell",
        "Report mouse to apps",
        "Open files with",
        "Tab completion",
        "Command history search",
        "Dim inactive panes",
        "Show title bar buttons on hover",
        "Option (⌥) acts as Meta",
    ] {
        if title == "Option (⌥) acts as Meta" && !cfg!(target_os = "macos") {
            continue;
        }
        assert!(
            settings_search_entries()
                .iter()
                .any(|e| t(e.title) == title),
            "no index entry titled {title:?}"
        );
    }
}

#[test]
fn agent_rows_are_in_the_search_index() {
    for agent in anterm_core::agent_hooks::HookAgent::ALL {
        assert!(
            settings_search_entries()
                .iter()
                .any(|e| e.section == SettingsSection::Agents
                    && t(e.title) == agent.display_name()),
            "no Agents index entry titled {:?}",
            agent.display_name()
        );
    }
}

#[test]
fn humanize_action_splits_on_capitals() {
    assert_eq!(humanize_action("NewTab"), "New Tab");
    assert_eq!(
        humanize_action("ToggleMaximizePane"),
        "Toggle Maximize Pane"
    );
    assert_eq!(humanize_action("Quit"), "Quit");
}

#[test]
fn the_host_filter_matches_name_address_and_port() {
    let mut p = SshProfile::new("prod-web");
    p.host = "10.0.1.21".to_string();
    p.user = "deploy".to_string();
    p.port = 2222;

    assert!(ssh_row_matches(&p, ""), "an empty query keeps everything");
    assert!(ssh_row_matches(&p, "prod"));
    assert!(ssh_row_matches(&p, "10.0.1"));
    assert!(ssh_row_matches(&p, "deploy"));
    assert!(ssh_row_matches(&p, "2222"));
    assert!(!ssh_row_matches(&p, "staging"));
}

#[test]
fn the_host_filter_ignores_case() {
    let mut p = SshProfile::new("Prod-Web");
    p.host = "Example.COM".to_string();
    assert!(ssh_row_matches(&p, "prod"));
    assert!(ssh_row_matches(&p, "example.com"));
}

#[test]
fn group_buckets_sort_imported_first_and_ungrouped_last() {
    let mut keys = vec!["", "Work", anterm_core::ssh_config::IMPORTED_GROUP];
    keys.sort_by_key(|k| ssh_group_rank(k));
    assert_eq!(
        keys,
        vec![anterm_core::ssh_config::IMPORTED_GROUP, "Work", ""]
    );
}

#[test]
fn group_labels_name_the_file_and_the_app() {
    assert_eq!(
        ssh_group_label(anterm_core::ssh_config::IMPORTED_GROUP),
        "~/.ssh/config"
    );
    assert_eq!(ssh_group_label(""), "anterm settings");
    assert_eq!(ssh_group_label("Work"), "Work");
}

#[test]
fn group_key_falls_back_to_the_ungrouped_bucket() {
    let mut p = SshProfile::new("a");
    assert_eq!(ssh_group_key(&p), "");
    p.group = Some("Work".to_string());
    assert_eq!(ssh_group_key(&p), "Work");
}

#[test]
fn parse_host_port_handles_blank_and_ports() {
    assert!(
        parse_host_port_checked("  ", DEFAULT_SOCKS_PORT)
            .unwrap()
            .is_none()
    );
    let hp = parse_host_port_checked("example.com:2222", DEFAULT_SOCKS_PORT)
        .unwrap()
        .unwrap();
    assert_eq!(hp.host, "example.com");
    assert_eq!(hp.port, 2222);
    // Used to be port 0, which no proxy answers on.
    assert_eq!(
        parse_host_port_checked("host", DEFAULT_SOCKS_PORT)
            .unwrap()
            .unwrap()
            .port,
        DEFAULT_SOCKS_PORT
    );
}

/// A form with the one field that is genuinely required, and nothing else.
fn draft_with_host() -> SshFormDraft {
    SshFormDraft {
        host: "example.com".to_string(),
        ..Default::default()
    }
}

#[test]
fn a_profile_with_no_host_is_not_saveable() {
    let (_, errors) = validate_ssh_draft(SshFormDraft::default(), &[]);
    assert_eq!(errors.host, Some(SshFieldError::HostMissing));
    assert!(!errors.is_empty());
}

#[test]
fn spaces_are_not_a_host() {
    let draft = SshFormDraft {
        host: "   ".to_string(),
        ..Default::default()
    };
    let (profile, errors) = validate_ssh_draft(draft, &[]);
    assert_eq!(errors.host, Some(SshFieldError::HostMissing));
    assert_eq!(profile.host, "");
}

#[test]
fn a_name_is_not_required() {
    // Every host imported from ~/.ssh/config arrives without one, and the
    // list falls back to the address.
    let (profile, errors) = validate_ssh_draft(draft_with_host(), &[]);
    assert_eq!(profile.name, "");
    assert!(errors.is_empty());
}

#[test]
fn a_blank_port_still_means_22() {
    let (profile, errors) = validate_ssh_draft(draft_with_host(), &[]);
    assert_eq!(profile.port, 22);
    assert_eq!(errors.port, None);
}

#[test]
fn a_port_that_is_not_a_port_is_refused() {
    // "0" parses as a u16 and used to be saved as written; the other two
    // failed to parse and were silently rewritten to 22.
    for text in ["0", "abc", "70000", "-1", "22 "] {
        let draft = SshFormDraft {
            port: text.to_string(),
            ..draft_with_host()
        };
        let (profile, errors) = validate_ssh_draft(draft, &[]);
        match text {
            "22 " => {
                assert_eq!(errors.port, None, "{text:?} is a port with spare space");
                assert_eq!(profile.port, 22);
            }
            _ => {
                assert_eq!(errors.port, Some(SshFieldError::PortRange), "{text:?}");
                assert!(!errors.is_empty());
            }
        }
    }
}

#[test]
fn a_jump_host_that_exists_is_kept_by_id() {
    let bastion = SshProfile::new("bastion");
    let draft = SshFormDraft {
        jump: "bastion".to_string(),
        ..draft_with_host()
    };
    let (profile, errors) = validate_ssh_draft(draft, &[bastion.clone()]);
    assert_eq!(profile.jump_host, Some(bastion.id));
    assert!(errors.is_empty());
}

#[test]
fn a_mistyped_jump_host_says_which_name_it_could_not_find() {
    let draft = SshFormDraft {
        jump: "bastian".to_string(),
        ..draft_with_host()
    };
    let (profile, errors) = validate_ssh_draft(draft, &[SshProfile::new("bastion")]);
    assert_eq!(
        errors.jump,
        Some(SshFieldError::JumpUnknown("bastian".to_string()))
    );
    assert_eq!(
        profile.jump_host, None,
        "a typo never saves as a direct connection"
    );
}

#[test]
fn a_host_cannot_jump_through_itself() {
    let me = SshProfile::new("prod");
    let draft = SshFormDraft {
        id: me.id,
        jump: "prod".to_string(),
        ..draft_with_host()
    };
    let (profile, errors) = validate_ssh_draft(draft, &[me]);
    assert_eq!(errors.jump, Some(SshFieldError::JumpIsSelf));
    assert_eq!(profile.jump_host, None);
}

#[test]
fn a_bare_proxy_host_takes_the_scheme_default_port() {
    let draft = SshFormDraft {
        socks: "socks.example.com".to_string(),
        http: "http.example.com".to_string(),
        ..draft_with_host()
    };
    let (profile, errors) = validate_ssh_draft(draft, &[]);
    assert_eq!(
        profile.socks_proxy,
        Some(HostPort::new("socks.example.com", 1080))
    );
    assert_eq!(
        profile.http_proxy,
        Some(HostPort::new("http.example.com", 8080))
    );
    assert!(errors.is_empty());
}

#[test]
fn a_proxy_address_with_a_colon_and_no_port_is_refused() {
    for text in ["proxy.example.com:", "proxy.example.com:abc", "proxy:0"] {
        let draft = SshFormDraft {
            socks: text.to_string(),
            ..draft_with_host()
        };
        let (profile, errors) = validate_ssh_draft(draft, &[]);
        assert_eq!(
            errors.socks,
            Some(SshFieldError::ProxyPortRange),
            "{text:?}"
        );
        assert_eq!(profile.socks_proxy, None, "{text:?}");
    }
}

#[test]
fn a_form_that_cannot_be_saved_still_reports_what_it_would_save() {
    // The Escape prompt asks whether the form differs from the config, so
    // an invalid form has to hand back a profile to compare — otherwise a
    // half-typed new host looks identical to the nothing on disk and
    // Escape throws it away without asking.
    let draft = SshFormDraft {
        name: "half typed".to_string(),
        ..Default::default()
    };
    let (profile, errors) = validate_ssh_draft(draft, &[]);
    assert!(!errors.is_empty());
    assert_eq!(profile.name, "half typed");
}

fn profile_at(name: &str, user: &str, host: &str, port: u16) -> SshProfile {
    let mut p = SshProfile::new(name);
    p.user = user.to_string();
    p.host = host.to_string();
    p.port = port;
    p
}

/// The saved password belongs to `user@host:port`, so what counts as
/// "shared" is exactly that triple — a different name or a jump host in
/// front of it changes nothing, and a different port makes it a different
/// secret entirely.
#[test]
fn the_same_endpoint_under_two_names_counts_as_shared() {
    let direct = profile_at("direct", "ana", "build.example.com", 22);
    let mut via_jump = profile_at("via bastion", "ana", "build.example.com", 22);
    via_jump.jump_host = Some(direct.id);
    let staging = profile_at("staging", "ana", "build.example.com", 2222);
    let other_user = profile_at("root", "root", "build.example.com", 22);

    let mut cfg = Config::default();
    let (direct_id, jump_id, staging_id) = (direct.id, via_jump.id, staging.id);
    cfg.ssh_profiles = vec![direct, via_jump, staging, other_user];

    // The two that reach the same endpoint see each other, and neither
    // counts itself.
    assert_eq!(profiles_sharing_endpoint(&cfg, direct_id), 1);
    assert_eq!(profiles_sharing_endpoint(&cfg, jump_id), 1);
    // A port apart is a keychain entry apart, so this one is alone even
    // though the user and host match two of the others.
    assert_eq!(profiles_sharing_endpoint(&cfg, staging_id), 0);
    // A profile that is no longer on the list shares with nobody.
    assert_eq!(profiles_sharing_endpoint(&cfg, Uuid::new_v4()), 0);
}

mod window_tests {
    use super::super::window::SettingsWindow;
    use super::super::{SettingsSection, ui_font_default_label};
    use anterm_ui_kit::config::{Config, MouseZoomModifier};
    use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext, px, size};
    use gpui_kit::component::Theme;

    fn mount_settings(cx: &mut TestAppContext) -> (Entity<SettingsWindow>, VisualTestContext) {
        anterm_ui_kit::config::pin_test_config_dir();
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Config::default());
            anterm_ui_kit::presets::load_registry(cx);
            anterm_ui_kit::theme::apply_theme(None, cx);
            anterm_ui_kit::keymap::init(cx);
        });
        let mut slot = None;
        let window = cx.add_window(|window, cx| {
            let view = cx.new(|cx| SettingsWindow::new_standalone(window, cx));
            slot = Some(view.clone());
            gpui_kit::component::Root::new(view, window, cx)
        });
        let vcx = VisualTestContext::from_window(window.into(), cx);
        (slot.unwrap(), vcx)
    }

    fn wait_for_binding(vcx: &mut VisualTestContext, action: &str, expected: serde_json::Value) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            vcx.run_until_parked();
            let got = vcx.update(|_, cx| {
                serde_json::to_value(cx.global::<Config>().keybindings.get(action))
                    .expect("a binding serializes")
            });
            if got == expected {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "binding for {action} never became {expected:?} (last {got:?})"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[gpui::test]
    fn every_category_and_search_layouts_standalone_at_minimum_width(cx: &mut TestAppContext) {
        let (view, mut vcx) = mount_settings(cx);
        for section in SettingsSection::ALL {
            view.update_in(&mut vcx, |s, window, cx| {
                s.navigate_settings(section, None, window, cx);
            });
            vcx.simulate_resize(size(px(720.), px(560.)));
            vcx.run_until_parked();
        }
        view.update_in(&mut vcx, |s, window, cx| {
            let search = s.state.search.clone();
            search.update(cx, |i, cx| i.set_value("font", window, cx));
            s.autoselect_settings_search(cx);
            assert!(s.state.search_active);
            s.set_mouse_zoom_modifier(MouseZoomModifier::Platform, cx);
            s.set_notify_threshold(71, cx);
            s.reset_settings_value(anterm_i18n::L10nKey::SettingsNotifyThreshold, window, cx);
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert_eq!(
                cx.global::<Config>().mouse_zoom_modifier,
                MouseZoomModifier::Platform
            );
            assert_eq!(
                cx.global::<Config>().notify_threshold_secs,
                Config::default().notify_threshold_secs
            );
        });
    }

    #[gpui::test]
    fn failed_writes_and_theme_draft_lifecycle_work_standalone(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("draft.yaml");
        let (view, mut vcx) = mount_settings(cx);
        view.update_in(&mut vcx, |s, window, cx| {
            // Quarantine write failure surfaces save_error until retry succeeds.
            cx.global_mut::<Config>().quarantined = true;
            s.set_notify_threshold(73, cx);
            assert!(s.state.save_error.is_some());
            cx.global_mut::<Config>().quarantined = false;
            s.persist_settings_config(cx);
            assert!(s.state.save_error.is_none());
            assert_eq!(cx.global::<Config>().notify_threshold_secs, 73);

            // Theme draft edits preview in memory, cancel restores, save persists.
            let mut theme = anterm_ui_kit::presets::all(cx).remove(0);
            theme.id = "settings-test-theme".into();
            theme.path = Some(path.clone());
            anterm_ui_kit::presets::write_theme_file(&theme).unwrap();
            let original_file = std::fs::read(&path).unwrap();
            let original_color = theme.accent;
            let mut themes = anterm_ui_kit::presets::all(cx);
            themes.push(theme);
            cx.set_global(anterm_ui_kit::presets::Themes(themes));
            cx.global_mut::<Config>().theme_follow_system = false;
            cx.global_mut::<Config>().theme_preset = "settings-test-theme".into();
            s.navigate_settings(SettingsSection::Appearance, None, window, cx);

            s.edit_active_theme(
                anterm_ui_kit::intent::ThemeEdit::Accent,
                gpui::rgb(0x123456).into(),
                window,
                cx,
            );
            assert!(s.theme_draft_dirty());
            assert_eq!(std::fs::read(&path).unwrap(), original_file);

            s.cancel_theme_draft(window, cx);
            assert!(!s.theme_draft_dirty());
            assert_eq!(
                anterm_ui_kit::presets::by_id(cx, "settings-test-theme").accent,
                original_color
            );

            s.edit_active_theme(
                anterm_ui_kit::intent::ThemeEdit::Accent,
                gpui::rgb(0x654321).into(),
                window,
                cx,
            );
            assert!(s.save_theme_draft(window, cx));
            assert_ne!(std::fs::read(&path).unwrap(), original_file);
            assert!(!s.theme_draft_dirty());
        });
    }

    #[gpui::test]
    fn keybinding_recording_unbind_reset_and_escape_work_standalone(cx: &mut TestAppContext) {
        let (view, mut vcx) = mount_settings(cx);

        // Record a single chord.
        view.update_in(&mut vcx, |s, window, cx| {
            s.select_settings_section(SettingsSection::Keybindings, cx);
            s.start_recording_key("NewTab".into(), window, cx);
        });
        vcx.simulate_keystrokes("secondary-shift-f9");
        wait_for_binding(&mut vcx, "NewTab", serde_json::json!(["secondary-shift-f9"]));
        assert!(view.read_with(&vcx, |s, _| s.state.recording.is_none()));

        // Backspace on empty capture unbinds; reset restores shipped binding.
        let shipped = vcx
            .update(|_, cx| anterm_ui_kit::keymap::effective_key("ActivateTab1", cx))
            .expect("ActivateTab1 has default chord");
        view.update_in(&mut vcx, |s, window, cx| {
            s.start_recording_key("ActivateTab1".into(), window, cx);
        });
        vcx.simulate_keystrokes("backspace");
        wait_for_binding(&mut vcx, "ActivateTab1", serde_json::json!([]));
        view.update(&mut vcx, |s, cx| {
            s.reset_keybinding("ActivateTab1".into(), cx);
        });
        assert_eq!(
            vcx.update(|_, cx| anterm_ui_kit::keymap::effective_key("ActivateTab1", cx)),
            Some(shipped)
        );

        // Escape cancels capture without writing.
        view.update_in(&mut vcx, |s, window, cx| {
            s.start_recording_key("CloseActiveTab".into(), window, cx);
        });
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert!(view.read_with(&vcx, |s, _| s.state.recording.is_none()));
        assert!(!vcx.update(|_, cx| cx.global::<Config>().keybindings.contains_key("CloseActiveTab")));
    }

    #[gpui::test]
    fn clearing_ui_font_restores_stock_font_and_ssh_password_marks_form_dirty(
        cx: &mut TestAppContext,
    ) {
        let (view, mut vcx) = mount_settings(cx);
        let stock = vcx.update(|_, cx| Theme::global(cx).font_family.clone());

        view.update_in(&mut vcx, |s, window, cx| {
            s.commit_ui_font_family("Courier".into(), window, cx);
        });
        assert_eq!(
            vcx.update(|_, cx| Theme::global(cx).font_family.as_ref().to_string()),
            "Courier"
        );
        view.update_in(&mut vcx, |s, window, cx| {
            s.commit_ui_font_family(ui_font_default_label().into(), window, cx);
        });
        assert_eq!(
            vcx.update(|_, cx| Theme::global(cx).font_family.clone()),
            stock
        );

        // Typing a password in an SSH profile form marks the form dirty.
        view.update_in(&mut vcx, |s, window, cx| {
            s.navigate_settings(SettingsSection::Ssh, None, window, cx);
            let profile = anterm_core::ssh_profile::SshProfile::new("blank");
            s.update_config(cx, |cfg| cfg.ssh_profiles.push(profile.clone()));
            s.ssh_form_load(&profile, window, cx);
            assert!(!s.ssh_form_dirty(cx));
            let pw = s.state.ssh_form.as_ref().unwrap().password.clone();
            pw.update(cx, |i, cx| i.set_value("hunter2", window, cx));
            assert!(s.ssh_form_dirty(cx));
        });
    }

    #[gpui::test]
    fn every_category_layouts_cleanly_at_12_14_18_ui_font_scales(cx: &mut TestAppContext) {
        let (view, mut vcx) = mount_settings(cx);
        for scale in [12.0_f32, 14.0_f32, 18.0_f32] {
            view.update_in(&mut vcx, |s, window, cx| {
                s.update_config(cx, |cfg| cfg.ui_font_size = scale);
                anterm_ui_kit::theme::apply_theme(Some(window), cx);
                assert_eq!(
                    Theme::global(cx).font_size,
                    gpui::px(scale),
                    "Theme::update must carry ui_font_size={scale}px"
                );
            });
            vcx.update(|window, cx| window.draw(cx).clear(cx));
            vcx.update(|window, _| {
                assert_eq!(
                    window.rem_size(),
                    gpui::px(scale),
                    "Root prepare hook must propagate rem_size={scale}px"
                );
            });
            for section in SettingsSection::ALL {
                view.update_in(&mut vcx, |s, window, cx| {
                    s.navigate_settings(section, None, window, cx);
                });
                vcx.update(|window, cx| window.draw(cx).clear(cx));
            }
            view.update_in(&mut vcx, |s, window, cx| {
                let search = s.state.search.clone();
                search.update(cx, |i, cx| i.set_value("theme", window, cx));
            });
            vcx.update(|window, cx| window.draw(cx).clear(cx));
            view.update_in(&mut vcx, |s, window, cx| {
                let search = s.state.search.clone();
                search.update(cx, |i, cx| i.set_value("", window, cx));
            });
        }
    }

}
