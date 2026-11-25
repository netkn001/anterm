use super::*;
use gpui::Action as _;

/// The actions a keymap built from `action_bindings` dispatches for `keys`
/// typed in `context`, in precedence order — the same lookup gpui performs
/// on a real keypress.
///
/// Bindings are asserted through this rather than through a mirror of the
/// table, so a chord the app would not really install, or would install in
/// another context, cannot pass.
fn dispatched(effective: &[(String, String)], keys: &str, context: &str) -> Vec<&'static str> {
    let input: Vec<Keystroke> = keys
        .split(' ')
        .map(|k| Keystroke::parse_with_os(k, crate::platform::current_os()).expect("the typed keystroke parses"))
        .collect();
    dispatched_keystrokes(effective, &input, context)
}

/// The same lookup for input a spec cannot spell — the shifted-glyph shape
/// a backend hands over for `secondary-shift-]` has no written form that
/// `Keystroke::parse` turns back into it.
fn dispatched_keystrokes(
    effective: &[(String, String)],
    input: &[Keystroke],
    context: &str,
) -> Vec<&'static str> {
    let mut keymap = gpui::Keymap::default();
    keymap.add_bindings(action_bindings(effective));
    let context = [gpui::KeyContext::parse(context).expect("the context parses")];
    keymap
        .bindings_for_input(input, &context)
        .0
        .iter()
        .map(|b| b.action().name())
        .collect()
}

/// What every gpui backend delivers when the secondary modifier is held
/// over a key whose shifted half is `glyph`: the glyph, and no shift flag —
/// the modifier is already spent on the character.
fn delivered_with_secondary(glyph: &str) -> Keystroke {
    Keystroke {
        modifiers: gpui::Modifiers {
            control: cfg!(not(target_os = "macos")),
            platform: cfg!(target_os = "macos"),
            shift: false,
            alt: false,
            function: false,
        },
        key: glyph.to_string(),
        key_char: None,
    }
}

#[test]
fn every_dispatchable_action_has_a_slot_to_bind_it_in() {
    // `make_binding` is what turns an action name into a real binding, and
    // `default_bindings` is the only list `set_binding` will write into. An
    // action in the first but not the second cannot be bound at all — not
    // from config.json, which drops it silently, and not from the
    // Keybindings page, which reads the second.
    let bindable: std::collections::HashSet<&str> =
        default_bindings().into_iter().map(|(a, _)| a).collect();
    for action in [
        "ShowRightPanelInfo",
        "ShowRightPanelChanges",
        "ShowRightPanelFiles",
        "ShowRightPanelSearch",
    ] {
        assert!(bindable.contains(action), "{action} has nowhere to bind to");
        assert!(
            make_binding(action, "ctrl-1").is_some(),
            "{action} has a slot but nothing to dispatch"
        );
    }
}

#[test]
fn every_action_has_an_authored_name() {
    anterm_i18n::set_locale("en");
    let missing: Vec<&str> = default_bindings()
        .into_iter()
        .map(|(a, _)| a)
        .filter(|a| authored_entry(a).is_none())
        .collect();
    assert!(
        missing.is_empty(),
        "these actions would render on the Keybindings page as a CamelCase \
         split of their internal name, in English, in a three-locale app: {missing:?}"
    );
}

#[test]
fn the_page_speaks_the_same_words_as_the_rest_of_the_app() {
    anterm_i18n::set_locale("en");
    // The four names the critique found for one action; the menu bar, the
    // palette and the docs all say Zoom Pane.
    assert_eq!(action_entry("ToggleMaximizePane").1, "Zoom Pane");
    assert_eq!(action_entry("CloseActiveTab").1, "Close Pane / Tab");
    assert_eq!(action_entry("NewWindow").1, "New Window");
    assert_eq!(action_entry("CloseWindow").1, "Close Window");
    assert_eq!(action_entry("ClearScrollback").1, "Clear Scrollback");
    assert_eq!(action_entry("TogglePalette").1, "Search…");
    // The numbered families are templated, not nine strings per locale.
    assert_eq!(action_entry("ActivateTab3").1, "Go to Tab 3");
    // And they land in the section they belong to.
    assert_eq!(action_entry("ActivateTab3").0, CommandGroup::TabsPanes);
    assert_eq!(action_entry("ToggleSftp").0, CommandGroup::Ssh);
    assert_eq!(action_entry("ForkAgentSessionUp").0, CommandGroup::Agents);
}

#[test]
fn new_window_ships_a_chord_only_where_one_is_free() {
    let mut effective: Vec<(String, String)> = default_bindings()
        .into_iter()
        .map(|(a, k)| (a.to_string(), k.to_string()))
        .collect();
    let default = effective
        .iter()
        .find(|(action, _)| action == "NewWindow")
        .map(|(_, key)| key.clone())
        .expect("NewWindow has to be listed here or it cannot be bound at all");
    assert_eq!(
        default,
        if cfg!(target_os = "macos") {
            "secondary-n"
        } else {
            ""
        },
        "macOS gets Cmd+N; off macOS this ships unbound on purpose"
    );

    // Ask the keymap rather than the table. A default can look bound and
    // dispatch nothing — gpui folds shift into the punctuation glyph, so
    // `secondary-shift-]` reached no action at all off macOS (#750). An
    // exact match is also the conflict check: any other action holding
    // this chord would show up in the list.
    if !default.is_empty() {
        assert_eq!(
            dispatched(&effective, &default, "Terminal"),
            vec![NewWindow::name_for_type()],
            "{default} must reach NewWindow, and nothing else may answer it"
        );
    }

    // Unbound still has to mean bindable. `set_binding` only writes into
    // slots this table already has, and `make_binding` is what turns the
    // name back into a dispatchable binding; miss either and the action
    // sits on the Keybindings page, takes a key, and does nothing — which
    // is the whole complaint in #710, not just the missing default.
    effective
        .iter_mut()
        .find(|(action, _)| action == "NewWindow")
        .expect("found once already")
        .1 = "ctrl-alt-shift-n".to_string();
    assert_eq!(
        dispatched(&effective, "ctrl-alt-shift-n", "Terminal"),
        vec![NewWindow::name_for_type()],
        "a chord the user assigns to NewWindow has to reach it"
    );
}

#[cfg(target_os = "macos")]
const SECONDARY: &str = "⌘";
#[cfg(not(target_os = "macos"))]
const SECONDARY: &str = "Ctrl";
#[cfg(target_os = "macos")]
const SHIFT: &str = "⇧";
#[cfg(not(target_os = "macos"))]
const SHIFT: &str = "Shift";
#[cfg(target_os = "macos")]
const CTRL: &str = "⌃";
#[cfg(not(target_os = "macos"))]
const CTRL: &str = "Ctrl";

#[test]
fn key_tokens_maps_modifiers_to_glyphs() {
    assert_eq!(key_tokens("secondary-t"), vec![SECONDARY, "T"]);
    #[cfg(target_os = "macos")]
    assert_eq!(key_tokens("secondary-shift-d"), vec![SHIFT, SECONDARY, "D"]);
    #[cfg(not(target_os = "macos"))]
    assert_eq!(key_tokens("secondary-shift-d"), vec![SECONDARY, SHIFT, "D"]);
    assert_eq!(key_tokens("secondary-enter"), vec![SECONDARY, "↵"]);
}

#[test]
fn key_tokens_keeps_the_minus_key_as_the_final_token() {
    assert_eq!(key_tokens("secondary--"), vec![SECONDARY, "−"]);
    assert_eq!(key_tokens("secondary-="), vec![SECONDARY, "="]);
    assert_eq!(key_tokens("secondary-,"), vec![SECONDARY, ","]);
}

#[test]
fn key_chords_splits_a_sequence_into_keycap_groups() {
    assert_eq!(
        key_chords("ctrl-b n"),
        vec![
            vec![CTRL.to_string(), "B".to_string()],
            vec!["N".to_string()]
        ]
    );
    assert_eq!(key_chords("secondary-t"), vec![vec![SECONDARY, "T"]]);
}

#[test]
fn every_default_action_has_a_binding_builder_or_is_unbound() {
    for (action, key) in default_bindings() {
        if !key.is_empty() {
            assert!(
                keystroke_is_valid(key),
                "default keystroke for {action} is invalid: {key:?}"
            );
            assert!(
                make_binding(action, key).is_some(),
                "no make_binding arm for action {action}"
            );
        }
    }
}

#[test]
fn every_action_has_a_binding_arm() {
    // The sibling test above only reaches actions that ship with a default
    // keystroke, which leaves the unbound ones — the majority — free to be
    // listed in `default_bindings` with no `make_binding` arm behind them.
    // Nothing surfaces that: the action shows up in Settings, the user
    // assigns a key, and the key silently does nothing.
    for (action, _) in default_bindings() {
        assert!(
            make_binding(action, "ctrl-f12").is_some(),
            "no make_binding arm for action {action}; \
             anyone who binds a key to it in Settings gets nothing"
        );
    }
}

#[test]
fn the_commit_key_only_fires_inside_the_commit_box() {
    // `secondary-enter` is `ToggleFullscreen` on macOS. The two coexist
    // only because the commit binding is scoped; drop the context and
    // committing steals full screen everywhere.
    assert_eq!(action_context("ScmCommit"), Some("ScmCommit"));
    assert_eq!(action_context("ScmCommitAmend"), Some("ScmCommit"));
    assert_eq!(action_context("ToggleFullscreen"), None);
}

#[test]
fn tmux_preset_keystrokes_all_parse_and_map_to_actions() {
    for (action, key) in tmux_preset("ctrl-b") {
        assert!(
            keystroke_is_valid(&key),
            "tmux preset keystroke for {action} does not parse: {key:?}"
        );
        assert!(
            make_binding(&action, &key).is_some(),
            "tmux preset action {action} has no make_binding arm"
        );
    }
}

#[test]
fn insert_newline_ships_both_default_chords() {
    let effective: Vec<(String, String)> = default_bindings()
        .into_iter()
        .map(|(a, k)| (a.to_string(), k.to_string()))
        .collect();
    assert_eq!(
        effective
            .iter()
            .find(|(a, _)| a == "InsertNewline")
            .map(|(_, k)| k.as_str()),
        Some("shift-enter")
    );
    assert!(extra_keystrokes(&effective).contains(&("InsertNewline", "alt-enter")));
    for key in ["shift-enter", "alt-enter"] {
        assert!(keystroke_is_valid(key), "{key} does not parse");
        assert!(
            dispatched(&effective, key, "Terminal").contains(&InsertNewline::name_for_type()),
            "{key} does not reach InsertNewline in a terminal"
        );
    }
    assert_eq!(key_tokens("shift-enter"), vec![SHIFT, "↵"]);
}

#[test]
fn shift_enter_fallback_retires_with_an_insert_newline_rebind() {
    assert!(is_default_insert_newline_binding(
        "InsertNewline",
        "shift-enter"
    ));
    assert!(!is_default_insert_newline_binding(
        "InsertNewline",
        "ctrl-o"
    ));
    assert!(!is_default_insert_newline_binding("InsertNewline", ""));
}

#[test]
fn shift_enter_prefix_binding_still_waits_for_its_second_key() {
    let mut effective = default_bindings()
        .into_iter()
        .map(|(action, key)| (action.to_string(), key.to_string()))
        .collect::<Vec<_>>();
    effective
        .iter_mut()
        .find(|(action, _)| action == "OpenSettings")
        .unwrap()
        .1 = "shift-enter x".to_string();

    let mut keymap = gpui::Keymap::default();
    keymap.add_bindings(action_bindings(&effective));
    let input = [gpui::Keystroke::parse_with_os("shift-enter", crate::platform::current_os()).unwrap()];
    let context = [gpui::KeyContext::parse("Terminal").unwrap()];
    let (_, pending) = keymap.bindings_for_input(&input, &context);

    assert!(pending, "the keymap must wait for the second key");

    let input = [
        gpui::Keystroke::parse_with_os("shift-enter", crate::platform::current_os()).unwrap(),
        gpui::Keystroke::parse_with_os("x", crate::platform::current_os()).unwrap(),
    ];
    let (matched, pending) = keymap.bindings_for_input(&input, &context);
    assert!(!pending);
    assert!(
        matched
            .first()
            .is_some_and(|binding| binding.action().partial_eq(&OpenSettings))
    );
}

#[test]
fn paste_ships_both_terminal_chords_off_macos_and_retires_together() {
    let effective: Vec<(String, String)> = default_bindings()
        .into_iter()
        .map(|(a, k)| (a.to_string(), k.to_string()))
        .collect();
    if cfg!(target_os = "macos") {
        assert!(
            !extra_keystrokes(&effective)
                .iter()
                .any(|(a, _)| *a == "PasteText"),
            "macOS pastes with Cmd+V; no extra chord should install there"
        );
        return;
    }
    assert_eq!(
        effective
            .iter()
            .find(|(a, _)| a == "PasteText")
            .map(|(_, k)| k.as_str()),
        Some("ctrl-shift-v")
    );
    assert_eq!(
        effective
            .iter()
            .find(|(a, _)| a == "CopyText")
            .map(|(_, k)| k.as_str()),
        Some("ctrl-shift-c")
    );
    assert!(extra_keystrokes(&effective).contains(&("PasteText", "shift-insert")));
    for key in ["ctrl-shift-v", "shift-insert"] {
        assert!(
            dispatched(&effective, key, "Terminal").contains(&PasteText::name_for_type()),
            "{key} must paste in a terminal"
        );
        assert!(
            !dispatched(&effective, key, "Workspace").contains(&PasteText::name_for_type()),
            "{key} is a terminal chord and must not paste outside one"
        );
    }
    // Plain Ctrl+V is `AlternatePaste`, and only where no full-screen
    // program is running: on the alternate screen the chord belongs to
    // that program and reaches it as SYN.
    assert!(
        dispatched(&effective, "ctrl-v", "Terminal").contains(&AlternatePaste::name_for_type()),
        "ctrl-v pastes at a prompt off macOS"
    );
    assert!(
        dispatched(&effective, "ctrl-v", "Terminal alt_screen").is_empty(),
        "a full-screen program owns ctrl-v"
    );
    assert!(
        dispatched(&effective, "ctrl-shift-v", "Terminal alt_screen")
            .contains(&PasteText::name_for_type()),
        "Ctrl+Shift+V is the paste that works on every screen"
    );
    // The two ways out, both of which the control-code validator has to
    // let through: retire the chord, or hand it the whole screen. Both are
    // one line in a `config.json`, so both are asserted against the whole
    // default table with that line applied — a bare one-entry table would
    // pass either assertion without the escape hatch working at all.
    let with = |action: &str, value: KeybindingOverride| {
        let overrides = std::collections::HashMap::from([(action.to_string(), value)]);
        flatten_chords(&resolve_chords("default", "ctrl-b", &overrides))
    };
    let retired = with("AlternatePaste", KeybindingOverride::Add(String::new()));
    assert!(
        dispatched(&retired, "ctrl-v", "Terminal").is_empty(),
        "an emptied AlternatePaste gives Ctrl+V back to the shell"
    );
    let everywhere = with("PasteText", KeybindingOverride::Add("ctrl-v".to_string()));
    for context in ["Terminal", "Terminal alt_screen"] {
        assert!(
            dispatched(&everywhere, "ctrl-v", context).contains(&PasteText::name_for_type()),
            "a user may put Paste itself on Ctrl+V and have it on every screen"
        );
    }
    let rebound = vec![("PasteText".to_string(), "ctrl-alt-v".to_string())];
    assert!(
        !extra_keystrokes(&rebound)
            .iter()
            .any(|(a, _)| *a == "PasteText"),
        "moving Paste off its default must retire Shift+Insert with it"
    );
}

#[test]
fn rebinding_insert_newline_retires_both_default_chords() {
    let effective = vec![("InsertNewline".to_string(), "ctrl-o".to_string())];
    assert!(extra_keystrokes(&effective).is_empty());
    assert_eq!(
        dispatched(&effective, "ctrl-o", "Terminal"),
        vec![InsertNewline::name_for_type()],
        "the chord the config asks for is the one the keymap dispatches"
    );
    for retired in ["shift-enter", "alt-enter"] {
        assert!(
            dispatched(&effective, retired, "Terminal").is_empty(),
            "{retired} is off the action now and must dispatch nothing"
        );
    }

    let unbound = vec![("InsertNewline".to_string(), String::new())];
    assert!(extra_keystrokes(&unbound).is_empty());
    assert!(action_bindings(&unbound).is_empty());
}

#[test]
fn each_binding_lands_in_the_context_its_action_is_scoped_to() {
    let effective = vec![
        ("InsertNewline".to_string(), "shift-enter".to_string()),
        ("NewTab".to_string(), "secondary-t".to_string()),
    ];
    // The newline is a terminal chord, and its default ships the fallback
    // beside it; both are scoped, so neither reaches the rest of the app.
    let mut newline = dispatched(&effective, "shift-enter", "Terminal");
    newline.sort_unstable();
    assert_eq!(
        newline,
        vec![
            InsertNewline::name_for_type(),
            InsertNewlineFallback::name_for_type(),
        ]
    );
    assert!(dispatched(&effective, "shift-enter", "Workspace").is_empty());
    assert_eq!(
        dispatched(&effective, "alt-enter", "Terminal"),
        vec![InsertNewline::name_for_type()],
        "the extra chord follows its action's scope"
    );
    // A window action has no context: it has to work from a terminal too.
    for context in ["Terminal", "Workspace"] {
        assert_eq!(
            dispatched(&effective, "secondary-t", context),
            vec![NewTab::name_for_type()],
            "NewTab must not be scoped to {context}"
        );
    }
}

#[test]
fn action_context_matches_the_scope_make_binding_installs() {
    let extra_actions = extra_keystrokes(
        &default_bindings()
            .into_iter()
            .map(|(a, k)| (a.to_string(), k.to_string()))
            .collect::<Vec<_>>(),
    );
    let actions = default_bindings()
        .into_iter()
        .map(|(a, _)| a)
        .chain(extra_actions.into_iter().map(|(a, _)| a));
    for action in actions {
        let binding =
            make_binding(action, "f13").unwrap_or_else(|| panic!("no arm for {action}"));
        let installed = binding.predicate().map(|p| p.to_string());
        assert_eq!(
            installed.as_deref(),
            action_context(action),
            "{action} is installed in a different context than `action_context` reports"
        );
    }
}

#[test]
fn secondary_enter_chords_are_distinct_from_insert_newline() {
    let defaults = default_bindings();
    let key_of = |action: &str| {
        defaults
            .iter()
            .find(|(a, _)| *a == action)
            .map(|(_, k)| *k)
            .unwrap()
    };
    assert_eq!(
        key_of("ToggleFullscreen"),
        per_platform("secondary-enter", "f11")
    );
    assert_eq!(key_of("ToggleMaximizePane"), "secondary-shift-enter");
    for window_chord in ["secondary-enter", "secondary-shift-enter"] {
        assert_ne!(window_chord, INSERT_NEWLINE_DEFAULT);
        assert_ne!(window_chord, INSERT_NEWLINE_ALT_DEFAULT);
    }
    for chord in [INSERT_NEWLINE_DEFAULT, INSERT_NEWLINE_ALT_DEFAULT] {
        assert_ne!(chord, "shift-alt-enter");
        assert_ne!(chord, "alt-shift-enter");
    }
}

#[test]
fn no_default_binding_sits_on_a_terminal_control_code() {
    // The invariant is "no default may *swallow* a terminal control code".
    // The exceptions are named and justified in
    // `control_code_binding_allowed`; anything new needs a fall-through of
    // its own to join them.
    for (action, spec) in default_bindings() {
        // Both spellings, because `push_binding` installs both. "Window
        // actions belong on ctrl-shift-*" stops being an escape over the
        // digits and the punctuation the moment the fold clears the Shift
        // again: `ctrl-shift-2` goes into the keymap as `ctrl-@` (#750).
        let folded = folded_spec(spec);
        for spelling in [Some(spec), folded.as_deref()].into_iter().flatten() {
            for chord in spelling.split_whitespace() {
                Keystroke::parse_with_os(chord, crate::platform::current_os()).expect("default chords parse");
                assert!(
                    !steals_a_control_code(chord)
                        || control_code_binding_allowed(action, chord),
                    "{action} is bound to {spec}, installed as {chord}, which the shell \
                     needs as a control code \
                     (Ctrl+[ is ESC, Ctrl+D is EOF, Ctrl+W deletes a word, \
                     Ctrl+2..8 are NUL/ESC/FS/GS/RS/US/DEL). \
                     Window actions belong on ctrl-shift-* off macOS — over a letter, \
                     where the platform keeps the Shift."
                );
            }
        }
    }
}

/// F1..F12 now encode (#834), so a default sitting on one takes it away
/// from the shell the same way a Ctrl binding takes a control code: gpui
/// matches bindings before the pane's key handler runs, so a bound
/// function key never reaches the PTY at all.
///
/// The three that do are named here rather than left to be discovered,
/// and each answers for the shell key it stands on:
///
/// * F11 keeps fullscreen outright. It is the chord Windows Terminal,
///   GNOME Terminal and konsole all use, and no shell binds it —
///   PSReadLine's F keys are F3, F7 and F8.
/// * F3 and Shift+F3 are Find Next / Previous, and they fall through:
///   with no find bar open the listener in `terminal::view` calls
///   `cx.propagate()`, so PSReadLine's CharacterSearch still gets the key.
///
/// A fourth needs a fall-through of its own to join them.
#[test]
fn f11_is_the_only_default_on_a_bare_function_key() {
    let mut bound = Vec::new();
    for (action, spec) in default_bindings() {
        for chord in spec.split_whitespace() {
            let ks = Keystroke::parse_with_os(chord, crate::platform::current_os()).expect("default chords parse");
            if ks.key.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()).is_some_and(|n| (1..=24).contains(&n)) {
                bound.push((action, chord));
            }
        }
    }
    let expected: &[(&str, &str)] = if cfg!(target_os = "macos") {
        &[]
    } else {
        &[
            ("FindNext", "f3"),
            ("FindPrevious", "shift-f3"),
            ("ToggleFullscreen", "f11"),
        ]
    };
    bound.sort();
    let mut expected = expected.to_vec();
    expected.sort();
    assert_eq!(
        bound, expected,
        "a default on a function key hides it from the shell; \
         PSReadLine wants F3, F7 and F8, readline's `bind -x` any of them"
    );
}

#[test]
fn the_control_code_rule_knows_what_the_shell_needs() {
    // The keys with a C0 byte behind them, and the modifier shape that
    // reaches it: Ctrl alone. This is the predicate the defaults are held
    // to above and the one `action_bindings` warns on.
    for chord in [
        "ctrl-d",
        "ctrl-c",
        "ctrl-[",
        "ctrl-2",
        "ctrl-6",
        "ctrl-8",
        "ctrl-/",
        "ctrl-space",
    ] {
        assert!(steals_a_control_code(chord), "{chord} is a control code");
    }
    for chord in [
        "ctrl-shift-v",
        "ctrl-alt-v",
        "secondary-shift-t",
        "ctrl-1",
        "ctrl-9",
        "ctrl--",
        "ctrl-f3",
        "alt-enter",
    ] {
        assert!(
            !steals_a_control_code(chord),
            "{chord} carries no control code"
        );
    }
    // Ctrl+V is allowed to anyone off macOS — it is how the default paste
    // reaches the chord, and how a user moves the full-screen paste onto
    // it — while Ctrl+D stays refused whoever asks.
    assert_eq!(
        control_code_binding_allowed("PasteText", "ctrl-v"),
        cfg!(not(target_os = "macos"))
    );
    assert!(!control_code_binding_allowed("PasteText", "ctrl-d"));
    assert!(control_code_binding_allowed("EditorSave", "secondary-s"));
    assert!(control_code_binding_allowed("EditorGoToLine", "ctrl-g"));
}

#[test]
fn every_default_chord_is_claimed_by_exactly_one_action() {
    // Per context, not globally: gpui resolves a keystroke by walking the
    // focus chain outwards, so a chord bound inside a narrow context and
    // again with no context is not a clash — the narrow one wins while
    // that element has focus and the global one applies everywhere else.
    // `ScmCommit` and `ToggleFullscreen` both take secondary-enter on that
    // basis. Two bindings sharing a chord *and* a context is still a bug,
    // because then which one fires is arbitrary.
    // `same_chord`, not string equality: `secondary-shift-]` and
    // `secondary-}` are two spellings of one keystroke and both install it.
    let mut seen: Vec<(&str, &str, Option<&'static str>)> = Vec::new();
    for (action, spec) in default_bindings() {
        if spec.is_empty() {
            continue;
        }
        let context = action_context(action);
        if let Some((other, _, _)) = seen
            .iter()
            .find(|(_, s, c)| same_chord(s, spec) && *c == context)
        {
            panic!("{action} and {other} both claim {spec} in context {context:?}");
        }
        seen.push((action, spec, context));
    }
}

#[test]
fn spec_from_keystroke_round_trips_through_parse() {
    for spec in [
        "secondary-t",
        "secondary-shift-t",
        "secondary-alt-left",
        "ctrl-shift-tab",
        "secondary--",
    ] {
        let ks = Keystroke::parse_with_os(spec, crate::platform::current_os()).unwrap();
        let round = spec_from_keystroke(&ks).expect("real key produces a spec");
        let reparsed = Keystroke::parse_with_os(&round, crate::platform::current_os()).unwrap();
        assert_eq!(
            (reparsed.modifiers, reparsed.key),
            (ks.modifiers, ks.key),
            "round trip diverged for {spec}"
        );
    }
}

#[test]
fn a_shifted_punctuation_chord_dispatches_however_it_is_spelled() {
    // Recording ⌘⇧] wrote `secondary-}` — correctly, since that is the
    // keystroke it was handed — while the default table and every config
    // in the wild spell the same chord `secondary-shift-]`. Only one of
    // the two used to reach the dispatcher (#750); all three do now.
    let typed = delivered_with_secondary("}");
    let recorded = spec_from_keystroke(&typed).expect("the recorder spells the chord");
    assert_eq!(recorded, "secondary-}");
    for spec in [recorded.as_str(), "secondary-shift-]", "secondary-shift-}"] {
        let effective = [("NextTab".to_string(), spec.to_string())];
        assert_eq!(
            dispatched_keystrokes(&effective, std::slice::from_ref(&typed), ""),
            vec![NextTab::name_for_type()],
            "{spec} never reaches the dispatcher"
        );
    }
}

#[test]
fn the_default_pane_cycle_chord_reaches_its_action() {
    // `secondary-]` on macOS, `secondary-shift-]` everywhere else — and
    // either way the backend reports the chord as the bare glyph. The
    // second spelling installed a binding nothing could press.
    let effective: Vec<(String, String)> = default_bindings()
        .into_iter()
        .map(|(a, k)| (a.to_string(), k.to_string()))
        .collect();
    let glyph = if cfg!(target_os = "macos") { "]" } else { "}" };
    assert!(
        dispatched_keystrokes(&effective, &[delivered_with_secondary(glyph)], "")
            .contains(&FocusNextPane::name_for_type()),
        "the default pane-cycle chord dispatches nothing"
    );
}

#[test]
fn folding_leaves_every_other_spelling_alone() {
    // Shift over a letter is a real modifier, and the named keys have no
    // shifted half at all — folding either would break far more than it
    // fixed.
    for spec in [
        "secondary-shift-t",
        "shift-enter",
        "shift-insert",
        "shift-tab",
        "ctrl-shift-up",
        "secondary-t",
        "secondary--",
        "ctrl-b n",
    ] {
        assert_eq!(folded_spec(spec), None, "{spec} was rewritten");
    }
    // A prefix chord folds per chord, not as a whole.
    assert_eq!(folded_spec("ctrl-b shift-5").as_deref(), Some("ctrl-b %"));
}

#[test]
fn a_chord_the_platform_delivers_with_shift_intact_stays_bound() {
    // Windows leaves Shift on the numpad's `/ * + -` — they are absent
    // from gpui's `need_to_convert_to_shifted_key`, so Ctrl+Shift+numpad-/
    // arrives as `ctrl-shift-/` and the recorder writes exactly that.
    // Folding is an addition, never a substitution, so that spec has to
    // survive the treatment `secondary-shift-]` gets (#750).
    let numpad = Keystroke {
        modifiers: gpui::Modifiers {
            control: cfg!(not(target_os = "macos")),
            platform: cfg!(target_os = "macos"),
            shift: true,
            alt: false,
            function: false,
        },
        key: "/".to_string(),
        key_char: None,
    };
    assert_eq!(
        spec_from_keystroke(&numpad).as_deref(),
        Some("secondary-shift-/")
    );
    let effective = [("NextTab".to_string(), "secondary-shift-/".to_string())];
    assert_eq!(
        dispatched_keystrokes(&effective, std::slice::from_ref(&numpad), ""),
        vec![NextTab::name_for_type()],
        "the numpad chord lost the binding written for it"
    );
    // And the main row's `?`, which the same spec also stands for, still
    // reaches the action — the fold added it rather than taking over.
    assert_eq!(
        dispatched_keystrokes(&effective, &[delivered_with_secondary("?")], ""),
        vec![NextTab::name_for_type()],
    );
}

#[test]
fn two_spellings_of_one_chord_are_recognised_as_one() {
    // What `assign_keybinding` leans on to displace the action already
    // holding a chord, whichever way either of them is written down.
    assert!(same_chord("secondary-shift-]", "secondary-}"));
    assert!(same_chord("secondary-}", "secondary-shift-]"));
    assert!(same_chord("secondary-shift-}", "secondary-shift-]"));
    assert!(same_chord("secondary-t", "secondary-t"));
    assert!(!same_chord("secondary-shift-]", "secondary-shift-["));
    assert!(!same_chord("secondary-}", "secondary-{"));
}

#[test]
fn the_control_code_guard_follows_the_folded_chord() {
    // The half of the control-code rule that only warns. A chord carrying
    // Shift reads as safe on its own — `steals_a_control_code` recognises a
    // bare Ctrl and nothing else — and it *was* safe while nothing could
    // press it. The fold makes it live, and Ctrl+Shift+2 is delivered as
    // Ctrl+@, which is the NUL the program on the far end is waiting for.
    // So the guard is asked about the spelling that goes into the keymap
    // (#750); `action_bindings` runs exactly this pair.
    for (written, folded) in [
        ("ctrl-shift-2", per_platform("ctrl-@", "secondary-@")),
        ("ctrl-shift-6", per_platform("ctrl-^", "secondary-^")),
        ("ctrl-shift--", per_platform("ctrl-_", "secondary-_")),
        ("ctrl-shift-/", per_platform("ctrl-?", "secondary-?")),
    ] {
        assert!(
            !steals_a_control_code(written),
            "{written} reads as safe as written, which is why the fold has to be checked"
        );
        assert_eq!(folded_spec(written).as_deref(), Some(folded));
        assert!(
            steals_a_control_code(folded),
            "{written} is installed as {folded} and takes a control code"
        );
    }
}

#[test]
fn a_key_the_us_table_does_not_know_is_left_alone() {
    // `SHIFTED_GLYPHS` is a US keyboard, and every other layout prints
    // something else on the shifted half: `?` is Shift+ß on a German
    // keyboard and Shift+, on a French one. The fold cannot know that, so
    // it declines rather than guesses — a key it has never seen keeps its
    // Shift, and the binding written for it stays exactly as written. That
    // is also what keeps the fold additive: it can widen a spec's reach but
    // never move it onto a chord the user did not ask for.
    for spec in [
        "secondary-shift-ß",
        "secondary-shift-é",
        "secondary-shift-ä",
        "secondary-shift-ç",
        "secondary-shift-ñ",
    ] {
        assert_eq!(folded_spec(spec), None, "{spec} was guessed at");
    }
    // And the recorded spelling, which is what a non-US layout actually
    // produces, needs no fold to reach the dispatcher in the first place.
    let effective = [("NextTab".to_string(), "secondary-?".to_string())];
    assert_eq!(
        dispatched_keystrokes(&effective, &[delivered_with_secondary("?")], ""),
        vec![NextTab::name_for_type()],
    );
}

#[test]
fn spec_from_keystroke_ignores_a_lone_modifier() {
    let ks = Keystroke::parse_with_os("secondary", crate::platform::current_os()).unwrap();
    assert_eq!(spec_from_keystroke(&ks), None);
}

#[test]
fn new_agent_tab_ships_a_chord_only_where_one_is_free() {
    let mut effective: Vec<(String, String)> = default_bindings()
        .into_iter()
        .map(|(a, k)| (a.to_string(), k.to_string()))
        .collect();
    let default = effective
        .iter()
        .find(|(action, _)| action == "NewAgentTab")
        .map(|(_, key)| key.clone())
        .expect("NewAgentTab has to be listed or it cannot be bound");
    if cfg!(target_os = "macos") {
        assert_eq!(default, "secondary-shift-a");
        assert_eq!(
            dispatched(&effective, &default, "Terminal"),
            vec![NewAgentTab::name_for_type()],
            "{default} must reach NewAgentTab, and nothing else may answer it"
        );
    } else {
        assert_eq!(default, "", "Ctrl+Shift+A is select-all off macOS");
    }

    // Each agent's own launch is a slot of its own, and a key bound to it
    // dispatches that agent.
    let name = crate::actions::launch_action_name(
        anterm_core::cli_agent::CLIAgent::Codex,
    );
    effective
        .iter_mut()
        .find(|(action, _)| action == name)
        .expect("every agent has a keymap slot")
        .1 = "ctrl-alt-shift-c".to_string();
    let mut keymap = gpui::Keymap::default();
    keymap.add_bindings(action_bindings(&effective));
    let typed = [Keystroke::parse_with_os("ctrl-alt-shift-c", crate::platform::current_os()).unwrap()];
    let context = [gpui::KeyContext::parse("Terminal").unwrap()];
    let hits = keymap.bindings_for_input(&typed, &context).0;
    assert_eq!(hits.len(), 1);
    assert!(hits[0].action().partial_eq(&LaunchAgent {
        agent: anterm_core::cli_agent::CLIAgent::Codex,
    }));
    assert_eq!(action_entry(name).0, CommandGroup::Agents);
}

mod gpui_tests {
    use super::super::*;
    use crate::config::Config;
    use gpui::TestAppContext;

    #[gpui::test]
    fn init_then_rebind_installs_the_merged_table(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Config::default());
            init(cx);

            {
                let cfg = cx.global_mut::<Config>();
                cfg.keybinding_preset = "tmux".to_string();
                cfg.keybindings.insert(
                    "NewTab".to_string(),
                    KeybindingOverride::Add("secondary-shift-n".to_string()),
                );
            }
            rebind(cx);

            let eff = effective_bindings(cx);
            let keys_of = |action: &str| {
                eff.iter()
                    .filter(|(a, _)| a == action)
                    .map(|(_, k)| k.clone())
                    .collect::<Vec<_>>()
            };
            assert_eq!(keys_of("NewTab"), ["ctrl-b c", "secondary-shift-n"]);
            assert_eq!(keys_of("SplitRight"), ["ctrl-b %"]);
            assert_eq!(
                keys_of("TogglePalette"),
                [per_platform("secondary-p", "secondary-shift-p")]
            );

            cx.global_mut::<Config>().keybinding_preset = "default".to_string();
            rebind(cx);
            let eff = effective_bindings(cx);
            assert_eq!(
                eff.iter()
                    .find(|(a, _)| a == "SplitRight")
                    .map(|(_, k)| k.as_str()),
                Some(per_platform("secondary-d", "secondary-shift-d"))
            );
        });
    }

    #[gpui::test]
    fn the_keybinding_triple_only_moves_when_a_binding_does(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Config::default());
            init(cx);

            let before = keybinding_config(cx);
            assert_eq!(
                keybinding_config(cx),
                before,
                "reading the config twice changes nothing"
            );

            // An unrelated edit leaves the triple alone — this is the gate the
            // watcher compares, and a sidebar drag must not rebind.
            cx.global_mut::<Config>().dim_inactive_panes = false;
            assert_eq!(keybinding_config(cx), before);

            // A real binding edit moves it.
            cx.global_mut::<Config>().keybindings.insert(
                "RenameTab".to_string(),
                KeybindingOverride::Add("ctrl-shift-r".to_string()),
            );
            assert_ne!(keybinding_config(cx), before);

            // So does the preset, and the prefix.
            let with_override = keybinding_config(cx);
            cx.global_mut::<Config>().keybinding_preset = "tmux".to_string();
            assert_ne!(keybinding_config(cx), with_override);
            let with_preset = keybinding_config(cx);
            cx.global_mut::<Config>().prefix = "ctrl-a".to_string();
            assert_ne!(keybinding_config(cx), with_preset);
        });
    }

    /// An app whose `config.json` reads `json`, keymap and all. Parsed from
    /// text rather than built in Rust, because the file is the surface #868 is
    /// about: what a hand-written line means is the thing under test.
    fn running_on_json(cx: &mut gpui::App, json: &str) {
        gpui_kit::init(cx);
        cx.set_global(Config(
            serde_json::from_str(json).expect("the config parses"),
        ));
        init(cx);
    }

    /// What the live keymap dispatches for `keys` typed in a terminal, best
    /// match first — the first entry is the action a real keypress runs.
    fn fired(cx: &gpui::App, keys: &str) -> Vec<&'static str> {
        let input: Vec<Keystroke> = keys
            .split(' ')
            .map(|k| Keystroke::parse_with_os(k, crate::platform::current_os()).expect("the typed keystroke parses"))
            .collect();
        let context = [gpui::KeyContext::parse("Terminal").expect("the context parses")];
        cx.key_bindings()
            .borrow()
            .bindings_for_input(&input, &context)
            .0
            .iter()
            .map(|b| b.action().name())
            .collect()
    }

    /// The code editor's line commands sit on chords the app also spends —
    /// ⌘/ is the shortcut sheet, ⌘↵ fullscreen, ⌘⇧↵ maximize, ⌥↑/⌥↓ pane
    /// focus off macOS. `fixed_bindings` re-adds them on the `CodeEditor`
    /// context the editor declares beside `Input`, after the app's table, so
    /// inside the editor they win and everywhere else nothing changes.
    #[gpui::test]
    fn the_code_editor_chords_win_inside_the_editor(cx: &mut TestAppContext) {
        cx.update(|cx| {
            running_on_json(cx, "{}");
            let first = |keys: &str, stack: &[&str]| {
                let input: Vec<Keystroke> = keys
                    .split(' ')
                    .map(|k| Keystroke::parse_with_os(k, crate::platform::current_os()).expect("the typed keystroke parses"))
                    .collect();
                let stack: Vec<gpui::KeyContext> = stack
                    .iter()
                    .map(|c| gpui::KeyContext::parse(c).expect("the context parses"))
                    .collect();
                cx.key_bindings()
                    .borrow()
                    .bindings_for_input(&input, &stack)
                    .0
                    .first()
                    .map(|b| b.action().name())
            };
            let editor = ["Workspace", "Input CodeEditor"];
            let join_lines =
                cfg!(target_os = "macos").then_some(("ctrl-j", "code_editor::JoinLines"));
            for (keys, action) in join_lines.into_iter().chain([
                ("secondary-/", "code_editor::ToggleLineComment"),
                ("alt-up", "code_editor::MoveLineUp"),
                ("alt-down", "code_editor::MoveLineDown"),
                ("alt-shift-up", "code_editor::CopyLineUp"),
                ("alt-shift-down", "code_editor::CopyLineDown"),
                ("secondary-shift-k", "code_editor::DeleteLine"),
                ("secondary-enter", "code_editor::InsertLineBelow"),
                ("secondary-shift-enter", "code_editor::InsertLineAbove"),
                ("secondary-l", "code_editor::SelectLine"),
                ("secondary-shift-\\", "code_editor::MoveToMatchingBracket"),
            ]) {
                assert_eq!(first(keys, &editor), Some(action), "{keys} in the editor");
                assert_ne!(
                    first(keys, &["Workspace", "Input"]),
                    Some(action),
                    "{keys} in a plain text field"
                );
                assert_ne!(
                    first(keys, &["Workspace", "Terminal"]),
                    Some(action),
                    "{keys} in a terminal"
                );
            }
        });
    }

    #[gpui::test]
    fn a_chord_added_in_config_keeps_the_default_one(cx: &mut TestAppContext) {
        use gpui::Action as _;
        cx.update(|cx| {
            // The report, word for word: a second way to reach the tab switcher
            // took the first one away (#868).
            running_on_json(
                cx,
                r#"{"keybindings": {"NextTab": "ctrl-alt-]", "PrevTab": "ctrl-alt-["}}"#,
            );
            assert_eq!(
                fired(cx, "ctrl-tab").first(),
                Some(&NextTab::name_for_type()),
                "the default chord survives a chord added beside it"
            );
            assert_eq!(
                fired(cx, "ctrl-shift-tab").first(),
                Some(&PrevTab::name_for_type())
            );
            assert_eq!(
                fired(cx, "ctrl-alt-]").first(),
                Some(&NextTab::name_for_type()),
                "and the added chord works too"
            );
            assert_eq!(
                fired(cx, "ctrl-alt-[").first(),
                Some(&PrevTab::name_for_type())
            );
            // The palette hint and the menus still name the chord the app
            // ships with.
            assert_eq!(effective_key("NextTab", cx).as_deref(), Some("ctrl-tab"));
        });
    }

    #[gpui::test]
    fn an_empty_chord_still_unbinds_the_action(cx: &mut TestAppContext) {
        cx.update(|cx| {
            // What config files already say to retire a default — the docs spell
            // `"AlternatePaste": ""` — and what Settings used to write for an
            // action that lost its chord. It has to keep meaning "no key".
            running_on_json(cx, r#"{"keybindings": {"NextTab": ""}}"#);
            assert!(fired(cx, "ctrl-tab").is_empty());
            assert_eq!(effective_key("NextTab", cx), None);
        });
    }

    #[gpui::test]
    fn a_list_is_the_whole_set_of_chords_for_an_action(cx: &mut TestAppContext) {
        use gpui::Action as _;
        cx.update(|cx| {
            running_on_json(
                cx,
                r#"{"keybindings": {"NextTab": ["ctrl-alt-]", "ctrl-alt-n"]}}"#,
            );
            assert!(
                fired(cx, "ctrl-tab").is_empty(),
                "a list replaces the default rather than joining it"
            );
            for chord in ["ctrl-alt-]", "ctrl-alt-n"] {
                assert_eq!(
                    fired(cx, chord).first(),
                    Some(&NextTab::name_for_type()),
                    "{chord} is in the list"
                );
            }
        });
    }

    #[gpui::test]
    fn an_empty_list_unbinds_the_action(cx: &mut TestAppContext) {
        cx.update(|cx| {
            running_on_json(cx, r#"{"keybindings": {"NextTab": []}}"#);
            assert!(fired(cx, "ctrl-tab").is_empty());
            assert_eq!(effective_key("NextTab", cx), None);
        });
    }

    /// #901: Alt+1…9 is how vim switches tabs, and anterm's default was eating
    /// the whole row of them. Both halves of what the reporter wanted have to
    /// hold, and hold across a restart — the complaint was that the default
    /// "keeps coming back".
    #[gpui::test]
    fn alt_digits_can_be_moved_off_the_tab_actions_for_good(cx: &mut TestAppContext) {
        use gpui::Action as _;
        cx.update(|cx| {
            let default_chord = per_platform("secondary-1", "alt-1");
            running_on_json(
                cx,
                r#"{"keybindings": {"ActivateTab1": ["alt-shift-1"],
                                    "ActivateTab2": []}}"#,
            );
            assert!(
                fired(cx, default_chord).is_empty(),
                "a list replaces the shipped chord, so the digit reaches the shell"
            );
            assert!(
                fired(cx, per_platform("secondary-2", "alt-2")).is_empty(),
                "and an empty list leaves the action with no chord at all"
            );
            assert_eq!(
                fired(cx, "alt-shift-1").first(),
                Some(&ActivateTab1::name_for_type()),
                "the chord asked for in its place is live"
            );
            assert_eq!(effective_key("ActivateTab2", cx), None);

            // The half that reads as "老是自动恢复": whatever the app saves has
            // to load back as the same thing. A `Config` serialized and read
            // again is exactly what a restart does with `config.json`.
            let saved =
                serde_json::to_string(&**cx.global::<Config>()).expect("the config serializes");
            cx.set_global(Config(
                serde_json::from_str(&saved).expect("the saved config parses"),
            ));
            rebind(cx);
            assert!(
                fired(cx, default_chord).is_empty(),
                "the shipped chord must not come back across a save and reload"
            );
            assert_eq!(
                fired(cx, "alt-shift-1").first(),
                Some(&ActivateTab1::name_for_type()),
            );
            assert_eq!(effective_key("ActivateTab2", cx), None);
        });
    }

    /// The hold the tab-number badges answer to is the one the tab chords are
    /// on, not the platform's secondary: off macOS the nine ship on Alt, where
    /// a badge raised by Ctrl hints at nothing that switches a tab.
    #[gpui::test]
    fn the_badge_hold_follows_the_tab_chord(cx: &mut TestAppContext) {
        cx.update(|cx| {
            running_on_json(cx, "{}");
            let shipped = Keystroke::parse_with_os(
                per_platform("secondary-1", "alt-1"),
                crate::platform::current_os(),
            )
            .expect("the shipped chord parses");
            assert_eq!(
                tab_badge_hold(cx),
                Some(shipped.modifiers),
                "the shipped hold is the shipped chord's modifier"
            );

            for (json, expected) in [
                (
                    r#"{"keybindings": {"ActivateTab1": ["alt-1"]}}"#,
                    Some(Modifiers::alt()),
                ),
                // Nothing to advertise: no chord at all, a chord with no
                // modifier to hold, a key that is not the digit the badge
                // draws, and the tmux preset's two-chord `prefix 1`.
                (r#"{"keybindings": {"ActivateTab1": []}}"#, None),
                (r#"{"keybindings": {"ActivateTab1": ["1"]}}"#, None),
                (r#"{"keybindings": {"ActivateTab1": ["alt-q"]}}"#, None),
                (r#"{"keybinding_preset": "tmux"}"#, None),
            ] {
                cx.set_global(Config(
                    serde_json::from_str(json).expect("the config parses"),
                ));
                assert_eq!(tab_badge_hold(cx), expected, "{json}");
            }
        });
    }

    #[gpui::test]
    fn a_chord_added_under_the_tmux_preset_joins_the_preset_chord(cx: &mut TestAppContext) {
        use gpui::Action as _;
        cx.update(|cx| {
            running_on_json(
                cx,
                r#"{"keybinding_preset": "tmux",
                    "keybindings": {"SelectNextTab": "ctrl-alt-]", "SplitRight": ["ctrl-alt-d"]}}"#,
            );
            // The preset is a scheme, and it still replaces the default chord.
            assert!(fired(cx, per_platform("secondary-shift-]", "ctrl-pagedown")).is_empty());
            // A chord added on top of it is added to the preset's chord.
            assert_eq!(
                fired(cx, "ctrl-b n").first(),
                Some(&SelectNextTab::name_for_type())
            );
            assert_eq!(
                fired(cx, "ctrl-alt-]").first(),
                Some(&SelectNextTab::name_for_type())
            );
            // The MRU switcher the preset no longer takes over keeps Ctrl+Tab.
            assert_eq!(
                fired(cx, "ctrl-tab").first(),
                Some(&NextTab::name_for_type())
            );
            // A list replaces the preset's chord the way it replaces a default.
            assert!(fired(cx, "ctrl-b %").is_empty());
            assert_eq!(
                fired(cx, "ctrl-alt-d").first(),
                Some(&SplitRight::name_for_type())
            );
        });
    }

    #[gpui::test]
    fn a_chord_the_user_adds_wins_over_a_default_already_on_it(cx: &mut TestAppContext) {
        use gpui::Action as _;
        cx.update(|cx| {
            // `ctrl-tab` is `NextTab`'s default, and `NewTab` sits above it in the
            // table. The keymap resolves a tie to the binding added last, so a
            // user chord laid down in table order lost to the default whenever
            // its action happened to come first — the line in config.json was
            // read and did nothing.
            running_on_json(cx, r#"{"keybindings": {"NewTab": "ctrl-tab"}}"#);
            assert_eq!(
                fired(cx, "ctrl-tab").first(),
                Some(&NewTab::name_for_type()),
                "the chord the user asked for is the one that runs"
            );
            assert_eq!(
                fired(cx, per_platform("secondary-t", "secondary-shift-t")).first(),
                Some(&NewTab::name_for_type()),
                "and NewTab keeps its own default"
            );
        });
    }

    #[cfg(target_os = "macos")]
    #[gpui::test]
    fn cmd_shift_w_closes_the_window_with_its_shells_still_running(cx: &mut TestAppContext) {
        use gpui::Action as _;
        cx.update(|cx| {
            running_on_json(cx, "{}");
            assert_eq!(
                fired(cx, "cmd-shift-w").first(),
                Some(&CloseWindow::name_for_type()),
                "Cmd+Q stops the server, so a Mac needs a chord that only puts anterm away"
            );
        });
    }

    #[gpui::test]
    fn repeated_rebinds_do_not_grow_the_keymap(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Config::default());
            init(cx);
            let size = cx.key_bindings().borrow().bindings().len();
            // A rebind that clears first stays the same size however many
            // times it runs; the append-only one this replaced grew by a full
            // table each call, and every keystroke walks the whole map (#548).
            for _ in 0..3 {
                rebind(cx);
            }
            assert_eq!(
                cx.key_bindings().borrow().bindings().len(),
                size,
                "rebind clears before it binds, so the map does not grow"
            );
        });
    }

    #[gpui::test]
    fn a_rebuild_keeps_the_bindings_anterm_did_not_install(cx: &mut TestAppContext) {
        cx.update(|cx| {
            // gpui-component binds the whole `Input` editing table — backspace
            // among it — from its own `init`, which runs once, before ours. A
            // rebuild replaces the map, so it has to carry them; without this
            // the first rebind left every text field in the app unable to
            // delete a character (#548).
            gpui_kit::init(cx);
            cx.set_global(Config::default());

            let backspace = |cx: &App| {
                let typed = [Keystroke::parse_with_os("backspace", crate::platform::current_os()).unwrap()];
                cx.key_bindings()
                    .borrow()
                    .all_bindings_for_input(&typed)
                    .iter()
                    .map(|b| b.action().name().to_string())
                    .collect::<Vec<_>>()
            };

            let inherited = backspace(cx);
            assert!(
                !inherited.is_empty(),
                "gpui-component binds backspace before anterm touches the keymap"
            );

            init(cx);
            assert_eq!(backspace(cx), inherited, "init must not drop them");
            rebind(cx);
            assert_eq!(backspace(cx), inherited, "nor may a rebind");
        });
    }

    /// The gutter's next / previous change chords live only in an `Input`:
    /// the editor gets them, a terminal never loses a function key to them.
    #[gpui::test]
    fn editor_change_chords_are_bound_only_in_an_input(cx: &mut TestAppContext) {
        use gpui::Action as _;
        cx.update(|cx| {
            running_on_json(cx, "{}");
            let hits = |keys: &str, contexts: &[&str]| -> Vec<&'static str> {
                let input = [Keystroke::parse_with_os(keys, crate::platform::current_os()).expect("the keystroke parses")];
                let context: Vec<_> = contexts
                    .iter()
                    .map(|c| gpui::KeyContext::parse(c).expect("the context parses"))
                    .collect();
                cx.key_bindings()
                    .borrow()
                    .bindings_for_input(&input, &context)
                    .0
                    .iter()
                    .map(|b| b.action().name())
                    .collect()
            };
            assert_eq!(
                hits("alt-f5", &["Workspace", "Input"]).first(),
                Some(&EditorNextChange::name_for_type())
            );
            assert_eq!(
                hits("shift-alt-f5", &["Workspace", "Input"]).first(),
                Some(&EditorPrevChange::name_for_type())
            );
            assert_eq!(
                hits("alt-f3", &["Workspace", "Input"]).first(),
                Some(&EditorPeekChange::name_for_type())
            );
            assert!(hits("alt-f5", &["Workspace"]).is_empty());
        });
    }

    /// The code editor's multi-cursor chords sit on keys anterm ships for panes
    /// (⌘D split, ⌘⌥↑/↓ focus). `fixed_bindings` re-adds them after the
    /// pane table so they rank first inside an `Input`; any other text field
    /// has no handler for them, and gpui falls through to the pane binding
    /// listed after. A terminal never sees them.
    #[gpui::test]
    fn editor_multi_cursor_chords_outrank_the_pane_bindings(cx: &mut TestAppContext) {
        use gpui::Action as _;
        use crate::actions::editor_text_actions::{
            AddCursorAbove, AddCursorBelow, SelectAllOccurrences, SelectNextOccurrence,
        };
        cx.update(|cx| {
            running_on_json(cx, "{}");
            let in_input = |keys: &str| -> Vec<&'static str> {
                let input = [Keystroke::parse_with_os(keys, crate::platform::current_os()).expect("the keystroke parses")];
                let context = [
                    gpui::KeyContext::parse("Workspace").expect("the context parses"),
                    gpui::KeyContext::parse("Input").expect("the context parses"),
                ];
                cx.key_bindings()
                    .borrow()
                    .bindings_for_input(&input, &context)
                    .0
                    .iter()
                    .map(|b| b.action().name())
                    .collect()
            };
            let cases = [
                ("secondary-d", SelectNextOccurrence::name_for_type()),
                ("secondary-shift-l", SelectAllOccurrences::name_for_type()),
                ("secondary-alt-up", AddCursorAbove::name_for_type()),
                ("secondary-alt-down", AddCursorBelow::name_for_type()),
            ];
            for (keys, editor_action) in cases {
                let hits = in_input(keys);
                assert_eq!(hits.first(), Some(&editor_action), "{keys} in the editor");
            }
            if cfg!(target_os = "macos") {
                assert!(in_input("secondary-d").contains(&SplitRight::name_for_type()));
                assert_eq!(
                    fired(cx, "secondary-d").first(),
                    Some(&SplitRight::name_for_type()),
                    "a terminal still splits on ⌘D"
                );
            }

            // ⌘K ⌘D in the code editor skips to the next occurrence, and ⌘K
            // there waits for the chord's second key.
            let editor = [
                gpui::KeyContext::parse("Workspace").expect("the context parses"),
                gpui::KeyContext::parse("Input CodeEditor").expect("the context parses"),
            ];
            let keymap = cx.key_bindings();
            let keymap = keymap.borrow();
            let chord = [
                Keystroke::parse_with_os("secondary-k", crate::platform::current_os()).unwrap(),
                Keystroke::parse_with_os("secondary-d", crate::platform::current_os()).unwrap(),
            ];
            let (hits, _) = keymap.bindings_for_input(&chord, &editor);
            assert_eq!(
                hits.first().map(|b| b.action().name()),
                Some(crate::actions::editor_text_actions::SkipOccurrence::name_for_type())
            );
            let (_, pending) =
                keymap.bindings_for_input(&[Keystroke::parse_with_os("secondary-k", crate::platform::current_os()).unwrap()], &editor);
            assert!(pending, "⌘K waits for the chord in the code editor");
            // A plain text field isn't held up.
            let field = [
                gpui::KeyContext::parse("Workspace").expect("the context parses"),
                gpui::KeyContext::parse("Input").expect("the context parses"),
            ];
            let (_, pending) =
                keymap.bindings_for_input(&[Keystroke::parse_with_os("secondary-k", crate::platform::current_os()).unwrap()], &field);
            assert!(!pending, "⌘K goes straight through in other fields");
        });
    }

    /// Go to Symbol and Back/Forward sit on chords the window already uses
    /// (⌘⇧O is the workspace switcher; off macOS Alt+←/→ move between
    /// panes). Bound at the text field's depth and after the window's table,
    /// they rank first inside the editor, and a terminal never sees them.
    #[gpui::test]
    fn editor_navigation_chords_outrank_the_window_inside_a_text_field(cx: &mut TestAppContext) {
        use gpui::Action as _;
        cx.update(|cx| {
            running_on_json(cx, "{}");
            let in_input = |keys: &str| -> Vec<&'static str> {
                let input = [Keystroke::parse_with_os(keys, crate::platform::current_os()).expect("the keystroke parses")];
                let context = [
                    gpui::KeyContext::parse("Workspace").expect("the context parses"),
                    gpui::KeyContext::parse("Input").expect("the context parses"),
                ];
                cx.key_bindings()
                    .borrow()
                    .bindings_for_input(&input, &context)
                    .0
                    .iter()
                    .map(|b| b.action().name())
                    .collect()
            };
            let (back, forward) = match cfg!(target_os = "macos") {
                true => ("ctrl--", "ctrl-shift--"),
                false => ("alt-left", "alt-right"),
            };
            for (keys, action) in [
                ("secondary-shift-o", EditorGoToSymbol::name_for_type()),
                (back, EditorNavigateBack::name_for_type()),
                (forward, EditorNavigateForward::name_for_type()),
                ("secondary-\\", EditorSplitRight::name_for_type()),
                (
                    per_platform("secondary-alt-left", "ctrl-alt-left"),
                    EditorFocusLeftGroup::name_for_type(),
                ),
                (
                    per_platform("secondary-alt-right", "ctrl-alt-right"),
                    EditorFocusRightGroup::name_for_type(),
                ),
            ] {
                assert_eq!(
                    in_input(keys).first(),
                    Some(&action),
                    "{keys} in the editor"
                );
                assert!(!fired(cx, keys).contains(&action), "{keys} in a terminal");
            }
        });
    }
}
