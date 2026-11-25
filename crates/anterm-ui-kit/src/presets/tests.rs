use super::*;
use super::themes::{FillFile, ThemeFile, id_and_name, load_iterm_theme, parse_hex, parse_rgb};

#[test]
fn the_current_tab_row_stays_legible_in_every_builtin_theme() {
    for theme in builtins() {
        let s = theme.surfaces().sidebar;
        assert!(
            contrast(s.selected, s.text_selected) >= 4.5,
            "{} current tab contrast",
            theme.id
        );
        assert_ne!(s.selected, s.base, "{} selection", theme.id);
    }
}

#[test]
fn foreground_is_legible_on_background() {
    for t in builtins() {
        let ratio = contrast(t.background_color(), t.foreground);
        assert!(
            ratio >= 4.0,
            "{}: fg/bg contrast too low ({ratio:.2})",
            t.id
        );
    }
}

#[test]
fn dark_is_inferred_from_background() {
    let dark: Vec<_> = builtins()
        .into_iter()
        .filter(|t| t.dark)
        .map(|t| t.id)
        .collect();
    assert_eq!(
        dark,
        [
            "dark",
            "dracula",
            "harbor",
            "one_dark_pro",
            "rose_pine",
            "catppuccin_mocha",
            "gruvbox_dark",
            "nord",
            "tokyo_night",
        ]
    );
}

#[test]
fn bright_slots_clear_the_text_floor_on_their_background() {
    // The renderer reads this palette for every pane, so every bright slot
    // must be legible on the theme background — SGR 90-97 is the family
    // PSReadLine paints parameters/operators/members with.
    for t in builtins() {
        let bg = t.background_color();
        let ap = t.active_palette(true);
        for (i, c) in ap.ansi16.iter().enumerate().skip(8) {
            let ink = (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
            assert!(
                contrast(ink, bg) >= TEXT_FLOOR - 0.01,
                "{}/ansi16[{i}]: bright slot {ink:#08x} only {:.2}:1 on the background",
                t.id,
                contrast(ink, bg)
            );
        }
    }
}

#[test]
fn legible_bright_slots_keep_their_authored_values() {
    // The lift is a rescue, not a restyle: a bright slot that already
    // clears the floor must come out of `active_palette` untouched.
    for t in builtins() {
        let bg = t.background_color();
        let ap = t.active_palette(true);
        for (i, (r, g, b)) in t.ansi16.iter().enumerate().skip(8) {
            let authored = (*r as u32) << 16 | (*g as u32) << 8 | *b as u32;
            let rendered = (ap.ansi16[i].r as u32) << 16
                | (ap.ansi16[i].g as u32) << 8
                | ap.ansi16[i].b as u32;
            if contrast(authored, bg) >= TEXT_FLOOR {
                assert_eq!(
                    rendered, authored,
                    "{}/ansi16[{i}]: a legible bright slot was touched",
                    t.id
                );
            }
        }
    }
}

#[test]
fn palette_renders_as_authored_when_legibility_is_off() {
    // The Settings → Appearance switch turns the rescue off entirely: the
    // renderer then sees every slot byte-for-byte as authored.
    for t in builtins() {
        let ap = t.active_palette(false);
        for (i, (r, g, b)) in t.ansi16.iter().enumerate() {
            let rendered = (ap.ansi16[i].r as u32) << 16
                | (ap.ansi16[i].g as u32) << 8
                | ap.ansi16[i].b as u32;
            assert_eq!(
                rendered,
                (*r as u32) << 16 | (*g as u32) << 8 | *b as u32,
                "{}/ansi16[{i}]: palette changed with legibility off",
                t.id
            );
        }
    }
}

#[test]
fn dark_bright_black_is_lifted_past_the_floor() {
    // The reported bug: PSReadLine paints parameters/operators/members
    // with DarkGray (SGR 90 → bright black), and the dark builtins author
    // that slot far below the floor, so the tokens vanished on the dark
    // background. The palette the renderer sees must come out lifted.
    for (id, authored) in [
        ("dracula", 0x555555),
        ("harbor", 0x292929),
        ("one_dark_pro", 0x5c6370),
        ("rose_pine", 0x6e6a86),
    ] {
        let t = builtins().into_iter().find(|t| t.id == id).unwrap();
        let c = t.active_palette(true).ansi16[8];
        let rendered = (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
        assert_ne!(rendered, authored, "{id}: brightBlack was left as authored");
        assert!(
            contrast(rendered, t.background_color()) >= TEXT_FLOOR - 0.01,
            "{id}: brightBlack {rendered:#08x} still only {:.2}:1 on the background",
            contrast(rendered, t.background_color())
        );
    }
    // Light themes hit the same wall from the other side: a pale
    // bright-black vanishes on a near-white background.
    let latte = builtins()
        .into_iter()
        .find(|t| t.id == "catppuccin_latte")
        .unwrap();
    let c = latte.active_palette(true).ansi16[8];
    let rendered = (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
    assert_ne!(
        rendered, 0xacb0be,
        "catppuccin_latte: brightBlack was left as authored"
    );
    assert!(
        contrast(rendered, latte.background_color()) >= TEXT_FLOOR - 0.01,
        "catppuccin_latte: brightBlack {rendered:#08x} still only {:.2}:1",
        contrast(rendered, latte.background_color())
    );
}

#[test]
fn selection_surface_stays_on_the_background_side() {
    for t in builtins() {
        let ap = t.active_palette(true);
        let sel = (ap.sel_bg.r as u32) << 16 | (ap.sel_bg.g as u32) << 8 | ap.sel_bg.b as u32;
        let to_bg = contrast(sel, t.background_color());
        let to_fg = contrast(sel, t.foreground);
        assert!(
            to_fg > to_bg,
            "{}: selection surface sits closer to the foreground",
            t.id
        );
    }
}

#[test]
fn state_ladder_is_separable_on_every_surface() {
    for t in builtins() {
        let s = t.surfaces();
        // The side panels run their own lighter rungs;
        // `panel_state_ladder_…` holds those apart instead.
        for (name, sf) in [("window", s.window), ("popover", s.popover)] {
            let sel_base = contrast(sf.selected, sf.base);
            let sel_hover = contrast(sf.selected, sf.hover);
            let hover_base = contrast(sf.hover, sf.base);
            let cursor_sel = contrast(sf.cursor, sf.selected);
            assert!(
                sel_base >= 1.25,
                "{}/{name}: selected is only {sel_base:.2}:1 from the surface",
                t.id
            );
            assert!(
                sel_hover >= 1.08,
                "{}/{name}: selected is only {sel_hover:.2}:1 from hover",
                t.id
            );
            assert!(
                cursor_sel >= 1.2,
                "{}/{name}: cursor is only {cursor_sel:.2}:1 from the resting selection",
                t.id
            );
            assert!(
                hover_base >= 1.1,
                "{}/{name}: hover is only {hover_base:.2}:1 from the surface",
                t.id
            );
            assert!(
                contrast(sf.pressed, sf.base) > sel_base,
                "{}/{name}: pressed must read past selected",
                t.id
            );
        }
    }
}

#[test]
fn panel_state_ladder_is_separable_in_every_theme() {
    for t in builtins() {
        let s = t.surfaces();
        for (name, sf) in [("sidebar", s.sidebar), ("rail", s.rail)] {
            let hover_base = contrast(sf.hover, sf.base);
            let sel_base = contrast(sf.selected, sf.base);
            assert!(
                hover_base >= state::PANEL_HOVER - 0.02,
                "{}/{name}: hover is only {hover_base:.2}:1 from the surface",
                t.id
            );
            assert!(
                sel_base >= state::PANEL_SELECTED - 0.02 && sel_base > hover_base,
                "{}/{name}: selected is only {sel_base:.2}:1 from the surface",
                t.id
            );
            assert!(
                contrast(sf.pressed, sf.base) > sel_base,
                "{}/{name}: pressed must read past selected",
                t.id
            );
        }
    }
}

#[test]
fn state_ladder_is_theme_independent() {
    let ratios: Vec<f32> = builtins()
        .iter()
        .map(|t| {
            let w = t.surfaces().window;
            contrast(w.selected, w.base)
        })
        .collect();
    let (lo, hi) = ratios
        .iter()
        .fold((f32::MAX, 0.0f32), |(l, h), r| (l.min(*r), h.max(*r)));
    assert!(
        hi - lo < 0.05,
        "selected step drifts across themes: {lo:.2}:1 … {hi:.2}:1"
    );
    assert!(
        (lo - state::SELECTED).abs() < 0.05,
        "selected step {lo:.2}:1 missed its {:.2}:1 target",
        state::SELECTED
    );
}

#[test]
fn dracula_selection_matches_the_signed_off_greys() {
    let dracula = builtins().into_iter().find(|t| t.id == "dracula").unwrap();
    let bg = dracula.background_color();
    let s = dracula.surfaces();
    // The resting rung is checked as `state::SELECTED` on the sidebar
    // fill it was signed off on — the old rail, 3% toward the foreground —
    // rather than on today's rail, which has since gone flat onto the
    // window fill. This pin is here to catch the constant drifting, not
    // that decision.
    let fg = legible_foreground(bg, dracula.foreground);
    let signed_off_rail = mix(bg, fg, 0.03);
    for (what, now, legacy) in [
        (
            "resting",
            raise(signed_off_rail, fg, state::SELECTED),
            mix(bg, dracula.foreground, 0.12),
        ),
        ("cursor", s.window.cursor, mix(bg, dracula.foreground, 0.17)),
    ] {
        assert!(
            contrast(now, legacy) < 1.05,
            "Dracula's {what} selection moved: {now:#08x} vs the tuned {legacy:#08x}"
        );
    }
}

/// CIE L*a*b* for a packed sRGB colour, D65.
///
/// Contrast is a luminance ratio and says nothing about hue: two lanes can
/// both clear 3:1 against the background and still be the same colour to
/// look at. ΔE is the measure that catches that, and it needs Lab.
fn lab(c: u32) -> (f32, f32, f32) {
    fn linear(v: u32) -> f32 {
        let s = v as f32 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    }
    let (r, g, b) = (
        linear(c >> 16 & 0xff),
        linear(c >> 8 & 0xff),
        linear(c & 0xff),
    );
    // sRGB → XYZ, then normalised by the D65 white point.
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    let f = |t: f32| {
        if t > 0.008856 {
            t.cbrt()
        } else {
            7.787 * t + 16.0 / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    (116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz))
}

fn delta_e76(a: u32, b: u32) -> f32 {
    let (l1, a1, b1) = lab(a);
    let (l2, a2, b2) = lab(b);
    ((l1 - l2).powi(2) + (a1 - a2).powi(2) + (b1 - b2).powi(2)).sqrt()
}

#[test]
fn lane_colours_clear_the_floor_on_every_surface() {
    for t in builtins() {
        let m = t.neutrals();
        let lanes = t.lanes();
        for (name, surface) in [
            ("background", m.background),
            ("sidebar", m.sidebar),
            ("popover", m.popover),
        ] {
            for (slot, ink) in lanes.ink.iter().enumerate() {
                let ratio = contrast(*ink, surface);
                assert!(
                    ratio >= ACCENT_FLOOR,
                    "{}/{name}: lane {slot} is only {ratio:.2}:1",
                    t.id
                );
            }
            let ratio = contrast(lanes.overflow, surface);
            assert!(
                ratio >= ACCENT_FLOOR,
                "{}/{name}: the overflow lane is only {ratio:.2}:1",
                t.id
            );
        }
    }
}

#[test]
fn adjacent_lanes_are_never_the_same_colour() {
    // A just-noticeable difference is around 2.3. The floor is set far
    // above it because these are 1.5px lines a few pixels apart, not
    // patches side by side, and the eye is much worse at hairlines.
    const FLOOR: f32 = 12.0;
    for t in builtins() {
        let lanes = t.lanes();
        for slot in 0..LANE_SLOTS - 1 {
            let d = delta_e76(lanes.ink[slot], lanes.ink[slot + 1]);
            assert!(
                d >= FLOOR,
                "{}: lanes {slot} and {} are ΔE {d:.1} apart",
                t.id,
                slot + 1
            );
        }
    }
}

#[test]
fn lane_colours_are_deterministic() {
    for t in builtins() {
        assert_eq!(
            t.lanes().ink,
            t.lanes().ink,
            "{}: lane derivation is not a pure function",
            t.id
        );
    }
}

/// The seeds were chosen so that no two neighbours share a hue family and
/// red never sits beside green. Both are properties of the *order*, so a
/// reshuffle has to fail here rather than only looking slightly worse.
#[test]
fn the_lane_seed_order_keeps_red_and_green_apart() {
    let seeds = [4usize, 3, 5, 2, 6, 1];
    let red = seeds.iter().position(|s| *s == 1).expect("red is a seed");
    let green = seeds.iter().position(|s| *s == 2).expect("green is a seed");
    assert!(
        red.abs_diff(green) > 1,
        "red and green ended up adjacent at slots {red} and {green}"
    );
    assert_eq!(red, LANE_SLOTS - 1, "red should be the last slot reached");
}

#[test]
fn resting_labels_stay_readable() {
    for t in builtins() {
        let s = t.surfaces();
        for (name, sf) in [("window", s.window), ("popover", s.popover)] {
            let ratio = contrast(sf.text_resting, sf.base);
            assert!(
                ratio >= 4.5,
                "{}/{name}: resting label only {ratio:.2}:1",
                t.id
            );
        }
    }
}

#[test]
fn label_channel_is_readable_and_stepped() {
    for t in builtins() {
        let s = t.surfaces();
        for (name, sf) in [
            ("window", s.window),
            ("sidebar", s.sidebar),
            ("popover", s.popover),
        ] {
            let on_fill = contrast(sf.text_selected, sf.selected);
            assert!(
                on_fill >= 4.5,
                "{}/{name}: selected label only {on_fill:.2}:1 on its own fill",
                t.id
            );
            let step = contrast(sf.text_selected, sf.text_resting);
            assert!(
                step >= 1.35,
                "{}/{name}: label step is only {step:.2}:1 — the channel says nothing",
                t.id
            );
        }
    }
}

#[test]
fn selected_label_is_stepped_off_the_resting_one() {
    let (base, fill, fg) = (0x2f333b, 0x40454d, 0xabb2bf);
    let resting = dim(fg, base, state::TEXT_RESTING);

    let plain = ink_on(fill, fg, state::TEXT_RESTING);
    assert!(
        contrast(plain, resting) < state::TEXT_STEP,
        "the case this floor exists for no longer collapses — pick another"
    );

    let ink = stepped_ink(fill, base, fg, resting);
    assert!(
        contrast(ink, resting) >= state::TEXT_STEP - 0.01,
        "stepped to {ink:#08x}, still only {:.2}:1 off the resting label",
        contrast(ink, resting)
    );
    assert!(
        contrast(ink, fill) >= state::TEXT_RESTING - 0.01,
        "stepping cost the label its fill: {:.2}:1 on {fill:#08x}",
        contrast(ink, fill)
    );
}

#[test]
fn switch_tracks_and_knob_stay_legible() {
    for t in builtins() {
        let m = t.neutrals();
        let unchecked = t.surfaces().window.selected;
        let checked = m.accent;
        let knob = if is_lighter(m.background, m.foreground) {
            m.background
        } else {
            m.foreground
        };
        assert!(
            contrast(knob, unchecked) >= 1.25,
            "{}: knob {knob:#08x} lost on the unchecked track {unchecked:#08x}",
            t.id
        );
        assert!(
            contrast(knob, checked) >= 1.25,
            "{}: knob {knob:#08x} lost on the checked track {checked:#08x}",
            t.id
        );
        assert!(
            contrast(checked, unchecked) >= 1.3,
            "{}: checked and unchecked tracks are {:.2}:1 apart",
            t.id,
            contrast(checked, unchecked)
        );
    }
}

#[test]
fn semantic_colors_clear_their_floors() {
    for t in builtins() {
        let bg = t.background_color();
        let m = t.neutrals();
        let s = t.semantics();
        for (name, c) in [
            ("danger", s.danger),
            ("warning", s.warning),
            ("success", s.success),
            ("info", s.info),
            ("link", s.link),
        ] {
            // Every neutral fill the ink can land on, not just the window:
            // a failure notice is usually read inside a popover.
            for (surface, fill) in [
                ("background", bg),
                ("sidebar", m.sidebar),
                ("popover", m.popover),
            ] {
                assert!(
                    contrast(c.ink, fill) >= TEXT_FLOOR - 0.01,
                    "{}/{name}: ink {:#08x} only {:.2}:1 on the {surface}",
                    t.id,
                    c.ink,
                    contrast(c.ink, fill)
                );
                assert!(
                    contrast(c.fill, fill) >= ACCENT_FLOOR - 0.01,
                    "{}/{name}: fill {:#08x} only {:.2}:1 on the {surface}",
                    t.id,
                    c.fill,
                    contrast(c.fill, fill)
                );
            }
            assert!(
                contrast(c.on_fill, c.fill) >= TEXT_FLOOR - 0.01,
                "{}/{name}: text on its own fill is only {:.2}:1",
                t.id,
                contrast(c.on_fill, c.fill)
            );
        }
        for (a, b, pair) in [
            (s.danger.ink, s.success.ink, "danger/success"),
            (s.danger.ink, s.warning.ink, "danger/warning"),
            (s.success.ink, s.warning.ink, "success/warning"),
        ] {
            assert!(
                channel_distance(a, b) >= 40,
                "{}: {pair} collapsed to nearly the same colour ({a:#08x} vs {b:#08x})",
                t.id
            );
        }
    }
}

#[test]
fn semantic_roles_are_independent_of_terminal_ansi() {
    let mut theme = builtins().into_iter().find(|t| t.id == "dracula").unwrap();
    let before = theme.semantics();
    theme.ansi16.swap(1, 2);
    theme.ansi16.swap(3, 6);
    let after = theme.semantics();
    assert_eq!(before.danger.ink, after.danger.ink);
    assert_eq!(before.success.ink, after.success.ink);
    assert_eq!(before.warning.ink, after.warning.ink);
    assert_eq!(before.link.ink, after.link.ink);
    assert_ne!(
        theme.active_palette(false).ansi16[1],
        theme.active_palette(false).ansi16[2]
    );
}

#[test]
fn interaction_roles_have_readable_labels_in_all_states() {
    for theme in builtins() {
        let c = theme.interactions();
        for fill in [c.primary.fill, c.primary_hover, c.primary_pressed] {
            assert!(
                contrast(fill, c.primary.on_fill) >= 4.5 - 0.01,
                "{} primary",
                theme.id
            );
        }
        assert!(
            contrast(c.choice, c.choice_ink) >= 4.5,
            "{} choice",
            theme.id
        );
        assert_ne!(
            c.input_border, c.choice,
            "{} field boundaries must not use selected-row fills",
            theme.id
        );
    }
}

#[test]
fn captions_are_readable_on_every_surface_they_are_used_on() {
    for theme in builtins() {
        let m = theme.neutrals();
        for surface in [m.background, m.sidebar, m.rail, m.popover] {
            assert!(
                contrast(m.muted_foreground, surface) >= TEXT_FLOOR,
                "{}: caption {:#08x} on {surface:#08x}",
                theme.id,
                m.muted_foreground,
            );
        }
    }
}

#[test]
fn sidebar_title_is_stepped_off_its_caption() {
    // The three rungs of a sidebar row — title, branch line, group
    // header — all sat at the same 4.5:1 grey once; this is the guard
    // against that column flattening again. The caption is dimmed on the
    // window and painted on the sidebar, so measure it where it lands.
    for t in builtins() {
        let m = t.neutrals();
        let step = contrast(m.sidebar_fg, m.muted_foreground);
        assert!(
            step >= state::TEXT_STEP - 0.01,
            "{}: title {:#08x} is only {step:.2}:1 off the caption {:#08x}",
            t.id,
            m.sidebar_fg,
            m.muted_foreground
        );
    }
}

#[test]
fn resting_semantic_ink_keeps_the_text_floor() {
    for t in builtins() {
        let m = t.neutrals();
        let sem = t.semantics();
        let beside: Hsla = gpui::rgb(m.muted_foreground).into();
        for (name, ink) in [("success", sem.success.ink), ("danger", sem.danger.ink)] {
            for (surface_name, surface) in [
                ("window", m.background),
                ("sidebar", m.sidebar),
                ("popover", m.popover),
            ] {
                let resting = pack(resting_ink(
                    gpui::rgb(ink).into(),
                    beside,
                    gpui::rgb(surface).into(),
                ));
                let ratio = contrast(resting, surface);
                assert!(
                    ratio >= TEXT_FLOOR - 0.02,
                    "{}/{surface_name}: resting {name} {resting:#08x} is only {ratio:.2}:1",
                    t.id
                );
            }
        }
    }
}

#[test]
fn sidebar_text_reads_on_the_fill_it_is_painted_on() {
    for t in builtins() {
        let m = t.neutrals();
        let ratio = contrast(m.sidebar_fg, m.sidebar);
        assert!(
            ratio >= TITLE_FLOOR - 0.01,
            "{}: sidebar text {:#08x} is only {ratio:.2}:1 on the sidebar fill {:#08x}",
            t.id,
            m.sidebar_fg,
            m.sidebar
        );
        let ratio = contrast(m.rail_fg, m.rail);
        assert!(
            ratio >= TITLE_FLOOR - 0.01,
            "{}: rail text {:#08x} is only {ratio:.2}:1 on the rail fill {:#08x}",
            t.id,
            m.rail_fg,
            m.rail
        );
    }
}

#[test]
fn the_rail_is_tinted_and_the_right_panel_is_not() {
    for t in builtins() {
        let m = t.neutrals();
        assert_eq!(m.sidebar, m.background, "{}: right panel fill", t.id);
        assert_ne!(m.rail, m.background, "{}: rail fill", t.id);
        let s = t.surfaces().rail;
        assert!(
            contrast(s.selected, s.text_selected) >= TEXT_FLOOR,
            "{}: current rail row",
            t.id
        );
    }
    let light = builtins().into_iter().find(|t| t.id == "light").unwrap();
    assert!(channel_distance(light.neutrals().rail, 0xf5f5f3) <= 1);
    let dark = builtins().into_iter().find(|t| t.id == "dark").unwrap();
    assert!(channel_distance(dark.neutrals().rail, 0x1e1e20) <= 1);
}

#[test]
fn carets_stay_visible_on_the_background() {
    for t in builtins() {
        let m = t.neutrals();
        let ratio = contrast(m.caret, m.background);
        assert!(
            ratio >= ACCENT_FLOOR - 0.01,
            "{}: caret {:#08x} is only {ratio:.2}:1 on the background",
            t.id,
            m.caret
        );
    }
    // The default theme used to ship an orange caret that read at 2.07:1
    // on pure white. It now takes the accent, like the focus ring.
    let light = builtins().into_iter().find(|t| t.id == DEFAULT_ID).unwrap();
    assert_eq!(light.neutrals().caret, light.neutrals().accent);
}

#[test]
fn hairlines_are_worth_the_same_in_every_theme() {
    for t in builtins() {
        let m = t.neutrals();
        for (tier, ink, floor) in [
            ("border", m.border, BORDER_FLOOR),
            ("divider", m.divider, DIVIDER_FLOOR),
        ] {
            for (name, surface) in [
                ("background", m.background),
                ("sidebar", m.sidebar),
                ("popover", m.popover),
            ] {
                let ratio = contrast(ink, surface);
                assert!(
                    ratio >= floor - 0.01,
                    "{}: {tier} {ink:#08x} is only {ratio:.2}:1 on the {name}",
                    t.id
                );
                // A floor, not a target — a hairline that shouts is worse
                // than one that whispers.
                assert!(
                    ratio <= 2.2,
                    "{}: {tier} {ink:#08x} is {ratio:.2}:1 on the {name} and reads as a frame",
                    t.id
                );
            }
        }
    }
}

/// The two tiers have to stay apart, or the split is decoration. A divider
/// that lands on the same value as the border is the state this replaced.
#[test]
fn a_divider_is_lighter_than_a_border_in_every_theme() {
    for t in builtins() {
        let m = t.neutrals();
        assert!(
            contrast(m.divider, m.sidebar) < contrast(m.border, m.sidebar),
            "{}: divider {:#08x} is not lighter than border {:#08x}",
            t.id,
            m.divider,
            m.border
        );
    }
}

#[test]
fn accents_are_conditioned_to_carry_ink() {
    for t in builtins() {
        let bg = t.background_color();
        let a = t.neutrals().accent;
        let ratio = contrast(a, bg);
        assert!(
            ratio >= ACCENT_FLOOR - 0.01,
            "{}: accent {a:#08x} only {ratio:.2}:1 on the background",
            t.id
        );
    }
    let rose = builtins()
        .into_iter()
        .find(|t| t.id == "rose_pine")
        .unwrap();
    assert_eq!(rose.neutrals().accent, rose.accent);
}

#[test]
fn a_match_wash_is_visible_on_a_white_theme_and_a_black_one() {
    // The old fixed alpha: 0.32 of a tint 24% off the background.
    let faint = |bg: u32, fg: u32| {
        let tint = mix(bg, fg, 0.24);
        contrast(mix(bg, tint, 0.32), bg)
    };
    for (bg, fg) in [(0xffffff, 0x111111), (0x111111, 0xffffff)] {
        assert!(
            faint(bg, fg) < 1.25,
            "the old wash was {:.2}:1 on {bg:06x} — that is what made it vanish",
            faint(bg, fg)
        );
        let tint = mix(bg, fg, 0.24);
        let (hit, current) = match_wash_targets(bg, fg);
        let w = wash(bg, tint, hit);
        assert!(
            contrast(w, bg) >= hit - 0.02,
            "{:.2}:1 on {bg:06x}",
            contrast(w, bg)
        );
        let cur = wash(bg, tint, current);
        assert!(
            contrast(cur, bg) > contrast(w, bg),
            "the current match has to stand out from the rest"
        );
    }
}

#[test]
fn a_low_contrast_theme_gets_a_gentler_wash_than_a_high_contrast_one() {
    let (soft_hit, soft_cur) = match_wash_targets(0x282c34, 0xabb2bf);
    let (hard_hit, hard_cur) = match_wash_targets(0x000000, 0xffffff);
    assert!(
        soft_cur < hard_cur && soft_hit < hard_hit,
        "the wash spends the theme's own contrast budget, so it has to \
         scale with it: {soft_cur:.2} vs {hard_cur:.2}"
    );
    assert!(
        soft_hit >= 1.45,
        "even the gentle end has to beat the flat constant it replaced"
    );
    assert!(hard_cur <= WASH_CEILING, "a highlight, not a solid block");
}

#[test]
fn a_match_wash_leaves_the_glyph_painted_on_top_of_it_readable() {
    // The wash is opaque and the text is drawn over it, so every step up in
    // visibility is a step down in the contrast the glyph has left. This is
    // the ceiling both targets are chosen against — on every builtin, and
    // in the accent the terminal actually washes with.
    for t in builtins() {
        let n = t.neutrals();
        let (hit_t, cur_t) = match_wash_targets(n.background, n.foreground);
        let hit = wash(n.background, n.accent, hit_t);
        let cur = wash(n.background, n.accent, cur_t);
        for (label, fill) in [("hit", hit), ("current", cur)] {
            // `wash` bisects to *at least* its target and 8-bit channels do
            // not divide evenly, so a fill can sit a hair past where the
            // target asked for it — and the glyph pays that hair.
            assert!(
                contrast(n.foreground, fill) >= GLYPH_FLOOR - 0.05,
                "{}: text on a {label} is {:.2}:1",
                t.id,
                contrast(n.foreground, fill)
            );
        }
        assert!(
            contrast(hit, n.background) > 1.5,
            "{}: a hit only shifts {:.2}:1 off the background — a hairline",
            t.id,
            contrast(hit, n.background)
        );
        assert!(
            contrast(cur, hit) >= 1.2,
            "{}: the current match only reads {:.2}:1 apart from the rest",
            t.id,
            contrast(cur, hit)
        );
    }
}

#[test]
fn a_wash_whose_tint_cannot_reach_the_target_still_lands_on_something_legible() {
    // A theme whose selection colour is its background: no alpha gets there.
    let (hit, _) = match_wash_targets(0xffffff, 0x111111);
    let w = wash(0xffffff, 0xfefefe, hit);
    assert!(contrast(w, 0xffffff) >= hit - 0.02, "{w:06x}");
}

#[test]
fn contrast_bisection_hits_its_target() {
    const SLACK: f32 = 0.05;
    let f = raise(0x000000, 0xffffff, 2.0);
    assert!((2.0..2.0 + SLACK).contains(&contrast(f, 0x000000)));
    let f = raise(0xffffff, 0x000000, 2.0);
    assert!((2.0..2.0 + SLACK).contains(&contrast(f, 0xffffff)));
    let d = dim(0xffffff, 0x000000, 4.5);
    assert!((contrast(d, 0x000000) - 4.5).abs() < SLACK);
    assert_eq!(raise(0x000000, 0x808080, 21.0), 0x808080);
}

#[test]
fn semantic_conditioning_survives_a_midtone_background() {
    let bg = 0x808080;
    for seed in [0xff5555u32, 0x50fa7b, 0xf1fa8c, 0x8be9fd] {
        let ink = legible_ink(bg, seed, TEXT_FLOOR);
        assert!(
            contrast(ink, bg) >= TEXT_FLOOR - 0.01,
            "{seed:#08x} conditioned to {ink:#08x}, only {:.2}:1 on a midtone ground",
            contrast(ink, bg)
        );
    }
}

#[test]
fn legible_foreground_rescues_unreadable_text() {
    assert_eq!(legible_foreground(0xffffff, 0xeeeeee), 0x000000);
    assert_eq!(legible_foreground(0xffffff, 0x111111), 0x111111);
    assert_eq!(legible_foreground(0x000000, 0x222222), 0xffffff);
}

#[test]
fn parse_hex_accepts_optional_hash_and_rejects_junk() {
    assert_eq!(parse_hex("#123456").unwrap(), 0x123456);
    assert_eq!(parse_hex("abcdef").unwrap(), 0xabcdef);
    assert!(parse_hex("#fff").is_err());
    assert!(parse_hex("nope!!").is_err());
}

#[test]
fn yaml_theme_parses_normal_then_bright() {
    let yaml = r##"
background: "#101010"
foreground: "#e0e0e0"
accent: "#ff8800"
ansi:
  normal: ["#000000","#111111","#222222","#333333","#444444","#555555","#666666","#777777"]
  bright: ["#888888","#999999","#aaaaaa","#bbbbbb","#cccccc","#dddddd","#eeeeee","#ffffff"]
"##;
    let file: ThemeFile = serde_yaml::from_str(yaml).unwrap();
    assert!(matches!(file.background, FillFile::Solid(_)));
    let bg = file.background.into_fill().unwrap().color();
    assert_eq!(bg, 0x101010);
    assert_eq!(parse_rgb(&file.ansi.normal[0]).unwrap(), (0, 0, 0));
    assert_eq!(parse_rgb(&file.ansi.bright[7]).unwrap(), (0xff, 0xff, 0xff));
}

#[test]
fn yaml_gradient_background_parses() {
    let file: ThemeFile = serde_yaml::from_str(
        r##"
background: { top: "#001122", bottom: "#334455" }
foreground: "#ffffff"
accent: "#ff0000"
ansi:
  normal: ["#000000","#000000","#000000","#000000","#000000","#000000","#000000","#000000"]
  bright: ["#ffffff","#ffffff","#ffffff","#ffffff","#ffffff","#ffffff","#ffffff","#ffffff"]
"##,
    )
    .unwrap();
    let fill = file.background.into_fill().unwrap();
    assert_eq!(
        fill,
        Fill::Vertical {
            top: 0x001122,
            bottom: 0x334455
        }
    );
    assert_eq!(fill.color(), 0x001122);
}

#[test]
fn to_yaml_round_trips_window_and_image_fields() {
    let mut theme = builtins().into_iter().next().unwrap();
    theme.background = Fill::Vertical {
        top: 0x001122,
        bottom: 0x334455,
    };
    theme.opacity = Some(0.85);
    theme.blur = true;
    theme.image = Some(Image {
        path: PathBuf::from("/pictures/koi.jpg"),
        opacity: 0.4,
    });
    let file: ThemeFile = serde_yaml::from_str(&to_yaml(&theme)).unwrap();
    assert_eq!(
        file.background.into_fill().unwrap(),
        theme.background,
        "gradient background lost"
    );
    assert_eq!(file.opacity, Some(0.85));
    assert!(file.blur);
    let img = file.background_image.expect("image field lost");
    assert_eq!(img.path, "/pictures/koi.jpg");
    assert_eq!(img.opacity, 0.4);
}

#[test]
fn iterm_theme_accepts_integer_components_and_no_alpha() {
    let entry = |key: String, r: &str| {
        format!(
            "<key>{key}</key><dict><key>Red Component</key>{r}\
             <key>Green Component</key><real>0.5</real>\
             <key>Blue Component</key><integer>0</integer></dict>"
        )
    };
    let mut body: String = (0..16)
        .map(|i| entry(format!("Ansi {i} Color"), "<integer>1</integer>"))
        .collect();
    body += &entry("Background Color".into(), "<integer>0</integer>");
    body += &entry("Foreground Color".into(), "<real>1.0</real>");
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <plist version=\"1.0\"><dict>{body}</dict></plist>"
    );
    let path =
        std::env::temp_dir().join(format!("anterm-int-{}.itermcolors", std::process::id()));
    std::fs::write(&path, xml).unwrap();
    let theme = load_iterm_theme(&path);
    let _ = std::fs::remove_file(&path);
    let theme = theme.unwrap();
    assert_eq!(theme.ansi16[2], (0xff, 0x80, 0x00));
    assert_eq!(theme.background_color(), 0x008000);
    assert_eq!(theme.foreground, 0xff8000);
}

#[test]
fn id_and_name_titlecases_the_stem() {
    let (id, name) = id_and_name(std::path::Path::new("/x/solarized_dark.yaml"));
    assert_eq!(id, "solarized_dark");
    assert_eq!(name, "Solarized Dark");
}

#[test]
fn theme_names_stay_english_under_a_translated_gui() {
    anterm_i18n::set_locale("zh-CN");
    // A stem of only separators leaves nothing to title-case.
    let (_, name) = id_and_name(std::path::Path::new("/x/_.yaml"));
    assert_eq!(name, "Theme");
    anterm_i18n::set_locale("en");
}

#[test]
fn mix_blends_channels() {
    assert_eq!(mix(0x000000, 0xffffff, 0.0), 0x000000);
    assert_eq!(mix(0x000000, 0xffffff, 1.0), 0xffffff);
    assert_eq!(mix(0x000000, 0xffffff, 0.5), 0x808080);
}
