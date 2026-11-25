use alacritty_terminal::selection::SelectionRange;
use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor, Rgb};
use gpui::{Font, FontStyle, FontWeight};

use super::colors::{
    DIM_OPACITY, build_font, fill_under, premultiplied, resolve, theme_under, to_hsla, unpack_rgb,
};
use super::paint::{
    MAX_ICON_GROWTH, PowerlineShape, RowSeg, RunStep, char_string, fit_scale,
    for_each_special_underline, has_room_after, icon_fit, icon_offset, ink_covers_segment, is_icon,
    lends_to_icon, native_cell_residue, paint_run, powerline_path, powerline_solid_edge,
    preedit_has_ink, run_drift, seg_budget, segment_has_room_after, segment_row,
};
use super::*;

fn caret_colors() -> PaintColors {
    PaintColors {
        default_fg: to_hsla(Rgb {
            r: 17,
            g: 17,
            b: 17,
        }),
        default_bg: to_hsla(Rgb {
            r: 255,
            g: 255,
            b: 255,
        }),
        caret: to_hsla(Rgb {
            r: 0xf5,
            g: 0xa1,
            b: 0x5c,
        }),
        selection_bg: Hsla::default(),
        match_bg: Hsla::default(),
        current_match_bg: Hsla::default(),
        legible_dim: true,
        fg_rgb: Rgb {
            r: 17,
            g: 17,
            b: 17,
        },
        bg_rgb: Rgb {
            r: 255,
            g: 255,
            b: 255,
        },
    }
}

#[test]
fn a_block_cursor_turns_its_cell_into_reverse_video() {
    let colors = caret_colors();
    let mut buf = vec![RenderCell::default(); 6];
    buf[3].c = 'o';
    buf[3].fg = to_hsla(Rgb { r: 1, g: 2, b: 3 });
    invert_cursor_cell(&mut buf, 3, 1, 0, &colors);

    let cell = &buf[3];
    assert!(cell.draw_bg, "the caret fill has to be painted");
    assert_eq!(cell.bg, colors.caret);
    assert_ne!(
        cell.fg,
        to_hsla(Rgb { r: 1, g: 2, b: 3 }),
        "the glyph is redrawn on the caret, not left in its own colour"
    );
    assert_eq!(cell.c, 'o', "the character itself survives");

    for (i, other) in buf.iter().enumerate() {
        if i != 3 {
            assert!(!other.draw_bg, "cell {i} was not the cursor cell");
        }
    }
}

#[test]
fn a_cursor_parked_on_a_wide_char_inverts_the_half_that_holds_the_glyph() {
    let colors = caret_colors();
    let mut buf = vec![RenderCell::default(); 4];
    buf[1].c = '世';
    buf[2].spacer = true;

    // Cursor reported on the trailing spacer: the lead cell is the one
    // that gets the fill, so it can absorb the spacer into its run.
    invert_cursor_cell(&mut buf, 4, 0, 2, &colors);
    assert!(buf[1].draw_bg, "the glyph's own cell carries the caret");
    assert!(!buf[2].draw_bg, "the spacer is absorbed, not painted");
    assert_eq!(buf[1].bg, colors.caret);

    // A spacer in column 0 has no lead to step back to; leave it alone
    // rather than wrapping to the previous row.
    let mut edge = vec![RenderCell::default(); 4];
    edge[0].spacer = true;
    invert_cursor_cell(&mut edge, 4, 0, 0, &colors);
    assert!(edge[0].draw_bg);
}

#[test]
fn caret_ink_takes_whichever_of_the_two_reads_on_the_caret() {
    let colors = caret_colors();
    // A pale caret on a white background: the foreground wins.
    let ink = anterm_ui_kit::presets::caret_ink(colors.caret, colors.default_bg, colors.default_fg);
    assert_eq!(ink, colors.default_fg);

    // A caret far from the background: the background wins, which is the
    // conventional reverse-video pairing.
    let deep = to_hsla(Rgb {
        r: 0x20,
        g: 0x20,
        b: 0x80,
    });
    let ink = anterm_ui_kit::presets::caret_ink(deep, colors.default_bg, colors.default_fg);
    assert_eq!(ink, colors.default_bg);
}

#[test]
fn a_caret_the_colour_of_the_text_still_leaves_its_glyph_readable() {
    // Gruvbox Dark, the theme in #966: the caret is the foreground colour
    // exactly, so drawing the glyph in its own ink on the caret would be
    // cream on cream. Reverse video has to reach for the background.
    let fg = Rgb {
        r: 0xeb,
        g: 0xdb,
        b: 0xb2,
    };
    let bg = Rgb {
        r: 0x28,
        g: 0x28,
        b: 0x28,
    };
    let colors = PaintColors {
        default_fg: to_hsla(fg),
        default_bg: to_hsla(bg),
        caret: to_hsla(fg),
        fg_rgb: fg,
        bg_rgb: bg,
        ..caret_colors()
    };
    let mut buf = vec![RenderCell::default(); 3];
    buf[1].c = 'v';
    buf[1].fg = to_hsla(fg);
    invert_cursor_cell(&mut buf, 3, 0, 1, &colors);
    assert_eq!(buf[1].bg, colors.caret);
    assert_eq!(buf[1].fg, colors.default_bg);
    assert_eq!(buf[1].c, 'v');
}

#[test]
fn a_preedit_with_nothing_to_show_leaves_the_cursor_cell_alone() {
    // Painting any of these would blank the cell under the cursor —
    // character and block caret both — to the theme background (#966).
    for blank in [
        "",
        " ",
        "  ",
        "\u{3000}",
        "\u{200b}",
        "\t",
        "\u{0}",
        " \u{200d} ",
    ] {
        assert!(!preedit_has_ink(blank), "{blank:?} has nothing to draw");
    }
    // Real compositions still paint, including a leading or trailing
    // blank the IME put between syllables.
    for text in ["n", "ni hao", "你好", " a", "に", "ㅎ", "é"] {
        assert!(preedit_has_ink(text), "{text:?} is a real preedit");
    }
}

#[test]
fn to_hsla_normalizes_channels_and_alpha() {
    let black = to_hsla(Rgb { r: 0, g: 0, b: 0 });
    assert_eq!(black.a, 1.0);
    assert!(black.l.abs() < 1e-6, "black has zero lightness");

    let white = to_hsla(Rgb {
        r: 255,
        g: 255,
        b: 255,
    });
    assert!((white.l - 1.0).abs() < 1e-6, "white has full lightness");
    assert!(white.s.abs() < 1e-6, "white is desaturated");

    let back = Rgba::from(to_hsla(Rgb { r: 255, g: 0, b: 0 }));
    assert!((back.r - 1.0).abs() < 1e-3);
    assert!(back.g.abs() < 1e-3 && back.b.abs() < 1e-3);
}

#[test]
fn resolve_covers_every_color_slot() {
    let mut palette = [Rgb { r: 0, g: 0, b: 0 }; 256];
    for (i, slot) in palette.iter_mut().enumerate() {
        slot.r = i as u8;
    }
    let fg = Rgb {
        r: 200,
        g: 201,
        b: 202,
    };
    let bg = Rgb {
        r: 10,
        g: 11,
        b: 12,
    };

    let spec = Rgb { r: 1, g: 2, b: 3 };
    assert_eq!(
        resolve(AnsiColor::Spec(spec), &palette, fg, bg),
        (spec, false)
    );

    let (rgb, is_def) = resolve(AnsiColor::Indexed(5), &palette, fg, bg);
    assert_eq!(rgb.r, 5);
    assert!(!is_def);

    assert_eq!(
        resolve(AnsiColor::Named(NamedColor::Foreground), &palette, fg, bg),
        (fg, true)
    );
    assert_eq!(
        resolve(AnsiColor::Named(NamedColor::Background), &palette, fg, bg),
        (bg, true)
    );

    let (rgb, is_def) = resolve(AnsiColor::Named(NamedColor::Red), &palette, fg, bg);
    assert_eq!(rgb.r, NamedColor::Red as u8);
    assert!(!is_def);
}

#[test]
fn build_font_sets_weight_and_style() {
    let base = gpui::font("Courier");
    let plain = build_font(&base, false, false);
    assert_eq!(plain.weight, FontWeight::NORMAL);
    assert_eq!(plain.style, FontStyle::Normal);

    let bold_italic = build_font(&base, true, true);
    assert_eq!(bold_italic.weight, FontWeight::BOLD);
    assert_eq!(bold_italic.style, FontStyle::Italic);

    assert_eq!(bold_italic.family, base.family);
}

#[test]
fn cell_rect_maps_grid_to_pixels() {
    let geom = CellGeom {
        origin: point(px(10.), px(20.)),
        cell_width: px(8.),
        line_height: px(16.),
        cols: 80,
        rows: 24,
    };
    let r = geom.cell_rect(2, 3, 1);
    assert_eq!(r.origin.x, px(10. + 8. * 3.));
    assert_eq!(r.origin.y, px(20. + 16. * 2.));
    assert_eq!(r.size.width, px(8.));
    assert_eq!(r.size.height, px(16.));
    assert_eq!(geom.cell_rect(0, 0, 4).size.width, px(32.));
}

#[test]
fn pos_to_cell_clamps_and_detects_halves() {
    let geom = CellGeom {
        origin: point(px(0.), px(0.)),
        cell_width: px(10.),
        line_height: px(20.),
        cols: 5,
        rows: 3,
    };
    let (c, r, left) = geom.pos_to_cell(point(px(2.), px(5.)));
    assert_eq!((c, r), (0, 0));
    assert!(left);
    let (_, _, left) = geom.pos_to_cell(point(px(8.), px(5.)));
    assert!(!left);
    let (c, r, _) = geom.pos_to_cell(point(px(-100.), px(-100.)));
    assert_eq!((c, r), (0, 0));
    let (c, r, _) = geom.pos_to_cell(point(px(9999.), px(9999.)));
    assert_eq!((c, r), (4, 2));
}

#[test]
fn drag_overshoot_signed_by_edge_and_zero_inside() {
    let bounds = Bounds::new(point(px(0.), px(100.)), size(px(200.), px(100.)));
    assert_eq!(drag_overshoot(px(150.), bounds, px(10.)), 0.);
    assert_eq!(drag_overshoot(px(100.), bounds, px(10.)), 0.);
    assert_eq!(drag_overshoot(px(200.), bounds, px(10.)), 0.);
    assert_eq!(drag_overshoot(px(80.), bounds, px(10.)), 2.);
    assert_eq!(drag_overshoot(px(230.), bounds, px(10.)), -3.);
}

#[test]
fn fit_scale_shrinks_what_overflows_and_never_enlarges() {
    let budget = px(15.);
    assert_eq!(fit_scale(px(19.2), budget), 15. / 19.2);
    // Whatever already fits keeps its own size, including a glyph that
    // lands exactly on the edge and a run that measured as empty. Making
    // a small glyph bigger is `icon_fit`'s rule, not this one's.
    assert_eq!(fit_scale(px(15.), budget), 1.);
    assert_eq!(fit_scale(px(12.), budget), 1.);
    assert_eq!(fit_scale(px(0.), budget), 1.);
}

#[test]
fn an_emoji_only_shrinks_where_the_face_is_narrow_and_the_next_cell_is_taken() {
    // Apple Color Emoji inks 1.25em, which is 18.75px at a 15px font size.
    let ink = px(18.75);
    let scale = |advance_em: f32, room: bool| {
        let cell = px(15. * advance_em);
        fit_scale(ink, seg_budget(false, true, 2, room, cell))
    };

    // Menlo and friends: a quarter cell of slack is enough on its own.
    assert_eq!(scale(0.6, false), 1.);
    // Ubuntu Mono is narrow enough that a crowded neighbour forces a
    // shrink, but a blank one lends a whole cell and the emoji stays whole.
    assert!(scale(0.5, false) < 1.);
    assert_eq!(scale(0.5, true), 1.);
}

#[test]
fn a_run_that_keeps_one_glyph_per_cell_is_painted_whole() {
    assert_eq!(run_drift([0, 1, 2, 3]), None);
    assert_eq!(run_drift([0]), None);
    assert_eq!(run_drift([0usize; 0]), None);
}

#[test]
fn a_ligature_cuts_the_run_at_the_column_that_drifted() {
    // "office" through a face that ligates "ffi": the shaper answers with
    // o, ffi, c, e, and `force_width` sits them on columns 0..4 — so 'c'
    // lands two columns early, over the ligature's tail, and the row ends
    // two columns short of where the caret is drawn.
    assert_eq!(run_drift([0, 1, 4, 5]), Some(4));
    // The remainder, restarted at column 4, lines up on its own.
    assert_eq!(run_drift([0, 1]), None);
    // A two-character ligature drifts by one: "afib" -> a, fi, b.
    assert_eq!(run_drift([0, 1, 3]), Some(3));
    // And a run can drift on its very first pair: "fib" -> fi, b.
    assert_eq!(run_drift([0, 2]), Some(2));
}

/// Drives the loop `paint_glyphs` runs a `RowSeg::Run` through, with the
/// shaping stubbed out. `drifts` is what the shaper is pretending to
/// answer for each piece it is handed, in order.
fn run_steps(start: usize, text: &str, drifts: &[Option<usize>]) -> Vec<RunStep> {
    let mut seen = Vec::new();
    let mut answers = drifts.iter().copied();
    paint_run(start, text.len(), |step| {
        let drift = matches!(step, RunStep::Drift { .. })
            .then(|| {
                answers
                    .next()
                    .expect("asked to shape more pieces than scripted")
            })
            .flatten();
        seen.push(step);
        drift
    });
    seen
}

/// What the caller is told to paint, as `(column, text)`.
fn painted<'a>(steps: &[RunStep], text: &'a str) -> Vec<(usize, &'a str)> {
    steps
        .iter()
        .filter_map(|step| match *step {
            RunStep::Paint { col, at, len } => Some((col, &text[at..at + len])),
            RunStep::Drift { .. } => None,
        })
        .collect()
}

#[test]
fn a_run_that_does_not_drift_is_shaped_once_and_painted_whole() {
    let steps = run_steps(7, "hello", &[None]);
    assert_eq!(
        steps,
        [
            RunStep::Drift { at: 0, len: 5 },
            RunStep::Paint {
                col: 7,
                at: 0,
                len: 5
            }
        ],
        "no drift is one shaping and one paint, at the column it started in"
    );
}

#[test]
fn a_drifted_run_paints_the_head_alone_and_restarts_at_its_own_column() {
    // "office" through a face that ligates "ffi": the shaper answers with
    // o, ffi, c, e, so 'c' arrives at byte 4 as the third glyph and the
    // run has to be cut there.
    let steps = run_steps(0, "office", &[Some(4), None]);
    assert_eq!(
        painted(&steps, "office"),
        [(0, "offi"), (4, "ce")],
        "the head is a piece of its own, and the rest starts at column 4"
    );
    // The head must be *shaped* as "offi", not painted as the whole run
    // under a four-cell clip: 'c' and 'e' drifted left, into those very
    // cells, so a clip would leave them on top of the ligature.
    assert_eq!(
        steps[1],
        RunStep::Paint {
            col: 0,
            at: 0,
            len: 4
        }
    );
    // And the remainder is re-shaped on its own before it is painted, so
    // its own drift is measured from its own column.
    assert_eq!(steps[2], RunStep::Drift { at: 4, len: 2 });
}

#[test]
fn a_run_that_drifts_twice_keeps_advancing_and_finishes() {
    // "affib" -> a, ffi, b cuts once; the tail "b" then stands on its own.
    assert_eq!(
        painted(&run_steps(3, "affib", &[Some(4), None]), "affib"),
        [(3, "affi"), (7, "b")]
    );
    // Two cuts in a row: every piece moves the column on by its own width
    // and the walk ends on the piece that does not drift.
    assert_eq!(
        painted(
            &run_steps(0, "offifie", &[Some(4), Some(2), None]),
            "offifie"
        ),
        [(0, "offi"), (4, "fi"), (6, "e")]
    );
}

#[test]
fn a_face_that_adds_glyphs_is_left_alone() {
    // Two glyphs for one character walk ahead of their columns, not
    // behind them. Cutting there would restart the run where it already
    // is and never finish.
    assert_eq!(run_drift([0, 0, 1, 2]), None);
    assert_eq!(run_drift([0, 1, 1, 2]), None);
}

#[test]
fn only_a_plain_blank_cell_counts_as_room() {
    let mut row: Vec<_> = "ab".chars().map(cell).collect();
    row.push(cell(' '));
    assert!(!has_room_after(&row, 0, 1), "a letter is not room");
    assert!(has_room_after(&row, 1, 1), "a blank is");
    assert!(has_room_after(&row, 2, 1), "so is the end of the row");

    // A blank that paints something of its own is not free real estate.
    row[2].draw_bg = true;
    assert!(!has_room_after(&row, 1, 1));
    row[2].draw_bg = false;
    row[2].selected = true;
    assert!(!has_room_after(&row, 1, 1));
    row[2].selected = false;

    // Nor is a blank that carries a rule of its own: it is painted after
    // the segment next to it, so the stroke would land on the glyph.
    row[2].underline = UnderlineKind::Single;
    assert!(!has_room_after(&row, 1, 1), "an underlined blank");
    row[2].underline = UnderlineKind::None;
    row[2].strikeout = true;
    assert!(!has_room_after(&row, 1, 1), "a struck-through blank");
    row[2].strikeout = false;
    row[2].link_hover = true;
    assert!(!has_room_after(&row, 1, 1), "a hovered link's blank");
}

#[test]
fn terminal_cursor_shape_maps_to_painted_cursor_style() {
    use anterm_ui_kit::config::CursorStyle;

    assert_eq!(cursor_style_from_shape(CursorShape::Beam), CursorStyle::Bar);
    assert_eq!(
        cursor_style_from_shape(CursorShape::Underline),
        CursorStyle::Underline
    );
    assert_eq!(
        cursor_style_from_shape(CursorShape::Block),
        CursorStyle::Block
    );
    assert_eq!(
        cursor_style_from_shape(CursorShape::HollowBlock),
        CursorStyle::Block
    );
}

fn cell(c: char) -> RenderCell {
    RenderCell {
        c,
        ..RenderCell::default()
    }
}

fn run(start: usize, cells: usize, text: &str) -> RowSeg {
    RowSeg::Run {
        start,
        cells,
        text: text.to_string(),
    }
}

fn wide(start: usize, cells: usize, text: &str) -> RowSeg {
    RowSeg::Wide {
        start,
        cells,
        text: text.into(),
    }
}

fn wide_run(start: usize, cells: usize, text: &str) -> RowSeg {
    RowSeg::WideRun {
        start,
        cells,
        text: text.to_string(),
    }
}

fn wide_cells(chars: &str) -> Vec<RenderCell> {
    let mut row = Vec::new();
    for c in chars.chars() {
        row.push(cell(c));
        let mut sp = cell(' ');
        sp.spacer = true;
        row.push(sp);
    }
    row
}

#[test]
fn segment_row_batches_uniform_ascii() {
    let row: Vec<_> = "hello".chars().map(cell).collect();
    assert_eq!(segment_row(&row), [run(0, 5, "hello")]);
}

#[test]
fn segment_row_joins_plain_runs_across_gaps_but_trims_edges() {
    let row: Vec<_> = " ab  cd  ".chars().map(cell).collect();
    assert_eq!(segment_row(&row), [run(1, 6, "ab  cd")]);
    let mut row: Vec<_> = "ab cd".chars().map(cell).collect();
    row[2].c = '\0';
    assert_eq!(segment_row(&row), [run(0, 5, "ab cd")]);
}

/// `paint_glyphs` reads a glyph's byte index as the column it came from
/// and cuts a drifted run there, which only holds while a run is ASCII and
/// one byte wide per cell. Pin that here rather than in the paint code.
#[test]
fn a_run_is_as_long_in_bytes_as_it_is_wide_in_cells() {
    let rows: [Vec<RenderCell>; 4] = [
        " ab  cd  ".chars().map(cell).collect(),
        "https://例/a".chars().map(cell).collect(),
        {
            let mut row: Vec<_> = "ab cd".chars().map(cell).collect();
            for c in &mut row {
                c.underline = UnderlineKind::Single;
            }
            row
        },
        "x->y a//b".chars().map(cell).collect(),
    ];
    for row in rows {
        for seg in segment_row(&row) {
            if let RowSeg::Run { cells, text, .. } = seg {
                assert!(text.is_ascii(), "{text:?} is not ASCII");
                assert_eq!(text.len(), cells, "{text:?} does not span {cells} cells");
            }
        }
    }
}

#[test]
fn segment_row_ends_underlined_runs_at_blanks() {
    let mut row: Vec<_> = "ab cd".chars().map(cell).collect();
    for c in &mut row {
        c.underline = UnderlineKind::Single;
    }
    assert_eq!(
        segment_row(&row),
        [run(0, 2, "ab"), run(2, 1, " "), run(3, 2, "cd")]
    );

    let mut row: Vec<_> = "ab cd".chars().map(cell).collect();
    for c in &mut row {
        c.link_hover = true;
    }
    assert_eq!(segment_row(&row), [run(0, 2, "ab"), run(3, 2, "cd")]);
}

#[test]
fn segment_row_keeps_decorated_blank_runs() {
    let mut row: Vec<_> = "   ".chars().map(cell).collect();
    for c in &mut row {
        c.strikeout = true;
    }
    assert_eq!(segment_row(&row), [run(0, 3, "   ")]);

    for c in &mut row {
        c.strikeout = false;
        c.underline = UnderlineKind::Dotted;
    }
    assert_eq!(segment_row(&row), [run(0, 3, "   ")]);
}

#[test]
fn segment_row_splits_on_style_changes() {
    let mut row: Vec<_> = "abcd".chars().map(cell).collect();
    row[2].fg = gpui::red();
    row[3].fg = gpui::red();
    assert_eq!(segment_row(&row), [run(0, 2, "ab"), run(2, 2, "cd")]);

    let mut row: Vec<_> = "abcd".chars().map(cell).collect();
    row[0].bold = true;
    assert_eq!(segment_row(&row), [run(0, 1, "a"), run(1, 3, "bcd")]);
}

#[test]
fn segment_row_isolates_non_ascii() {
    let mut row: Vec<_> = "ok?字 no".chars().map(cell).collect();
    row.insert(4, {
        let mut sp = cell(' ');
        sp.spacer = true;
        sp
    });
    assert_eq!(
        segment_row(&row),
        [run(0, 3, "ok?"), wide(3, 2, "字"), run(6, 2, "no")]
    );

    let row: Vec<_> = "a─b".chars().map(cell).collect();
    assert_eq!(
        segment_row(&row),
        [run(0, 1, "a"), RowSeg::Solo { col: 1 }, run(2, 1, "b")]
    );
}

#[test]
fn powerline_shape_maps_only_the_solid_separators() {
    for (c, shape) in [
        ('\u{e0b0}', PowerlineShape::TriangleRight),
        ('\u{e0b2}', PowerlineShape::TriangleLeft),
        ('\u{e0b4}', PowerlineShape::HalfCircleRight),
        ('\u{e0b6}', PowerlineShape::HalfCircleLeft),
        ('\u{e0b8}', PowerlineShape::SlantLowerLeft),
        ('\u{e0ba}', PowerlineShape::SlantLowerRight),
        ('\u{e0bc}', PowerlineShape::SlantUpperLeft),
        ('\u{e0be}', PowerlineShape::SlantUpperRight),
    ] {
        assert_eq!(PowerlineShape::of(c), Some(shape), "U+{:04X}", c as u32);
    }
    for c in [
        '\u{e0b1}', '\u{e0b3}', '\u{e0b5}', '\u{e0b7}', '\u{e0b9}', '\u{e0bb}', '\u{e0bd}',
        '\u{e0bf}', '\u{e0a0}', '\u{e0c0}', '\u{2500}', '❯', '➜',
    ] {
        assert_eq!(PowerlineShape::of(c), None, "U+{:04X}", c as u32);
    }
}

#[test]
fn powerline_path_fills_exactly_one_cell() {
    let (x0, y0, w, h) = (px(10.), px(20.), px(9.), px(21.));
    let bounds = Bounds::new(point(x0, y0), size(w, h));
    for shape in [
        PowerlineShape::TriangleRight,
        PowerlineShape::TriangleLeft,
        PowerlineShape::HalfCircleRight,
        PowerlineShape::HalfCircleLeft,
        PowerlineShape::SlantLowerLeft,
        PowerlineShape::SlantLowerRight,
        PowerlineShape::SlantUpperLeft,
        PowerlineShape::SlantUpperRight,
    ] {
        let path = powerline_path(bounds, shape);
        assert!(!path.vertices.is_empty(), "{shape:?} produced no geometry");
        let (mut min_x, mut max_x) = (px(f32::MAX), px(f32::MIN));
        for v in &path.vertices {
            let p = v.xy_position;
            assert!(
                p.x >= x0 && p.x <= x0 + w && p.y >= y0 && p.y <= y0 + h,
                "{shape:?} vertex at {p:?} escapes the cell"
            );
            min_x = min_x.min(p.x);
            max_x = max_x.max(p.x);
        }
        assert_eq!(min_x, x0, "{shape:?} does not reach the left cell edge");
        assert_eq!(
            max_x,
            x0 + w,
            "{shape:?} does not reach the right cell edge"
        );
        match shape {
            PowerlineShape::HalfCircleRight => assert!(
                path.vertices[1].xy_position.x <= x0 + w * 0.4
                    && path.vertices[path.vertices.len() - 2].xy_position.x <= x0 + w * 0.4,
                "right caps should not collapse into a diagonal wedge"
            ),
            PowerlineShape::HalfCircleLeft => assert!(
                path.vertices[1].xy_position.x >= x0 + w * 0.6
                    && path.vertices[path.vertices.len() - 2].xy_position.x >= x0 + w * 0.6,
                "left caps should not collapse into a diagonal wedge"
            ),
            _ => {}
        }
    }
}

#[test]
fn powerline_solid_edge_covers_only_the_fractional_device_pixel() {
    let bounds = Bounds::new(point(px(10.2), px(20.)), size(px(9.2), px(21.)));
    let scale = 1.25;

    for shape in [
        PowerlineShape::TriangleRight,
        PowerlineShape::HalfCircleRight,
        PowerlineShape::SlantLowerLeft,
        PowerlineShape::SlantUpperLeft,
    ] {
        assert_eq!(
            powerline_solid_edge(bounds, shape, scale, 1.),
            Some(Bounds::from_corners(
                point(px(12. / scale), bounds.top()),
                point(px(13. / scale), bounds.bottom()),
            )),
            "{shape:?} must cover only the device pixel containing its left edge"
        );
    }

    for shape in [
        PowerlineShape::TriangleLeft,
        PowerlineShape::HalfCircleLeft,
        PowerlineShape::SlantLowerRight,
        PowerlineShape::SlantUpperRight,
    ] {
        assert_eq!(
            powerline_solid_edge(bounds, shape, scale, 1.),
            Some(Bounds::from_corners(
                point(px(24. / scale), bounds.top()),
                point(px(25. / scale), bounds.bottom()),
            )),
            "{shape:?} must cover only the device pixel containing its right edge"
        );
    }
}

#[test]
fn powerline_solid_edge_skips_device_aligned_edges() {
    let bounds = Bounds::new(point(px(10.), px(20.)), size(px(9.), px(21.)));

    for shape in [
        PowerlineShape::TriangleRight,
        PowerlineShape::TriangleLeft,
        PowerlineShape::HalfCircleRight,
        PowerlineShape::HalfCircleLeft,
        PowerlineShape::SlantLowerLeft,
        PowerlineShape::SlantLowerRight,
        PowerlineShape::SlantUpperLeft,
        PowerlineShape::SlantUpperRight,
    ] {
        assert_eq!(
            powerline_solid_edge(bounds, shape, 2., 1.),
            None,
            "{shape:?}"
        );
    }
}

#[test]
fn powerline_solid_edge_sanitizes_invalid_scale() {
    let bounds = Bounds::new(point(px(10.25), px(20.)), size(px(9.), px(21.)));
    let expected = Some(Bounds::from_corners(
        point(px(10.), bounds.top()),
        point(px(11.), bounds.bottom()),
    ));

    for scale in [0., -1., f32::NAN] {
        assert_eq!(
            powerline_solid_edge(bounds, PowerlineShape::TriangleRight, scale, 1.),
            expected
        );
    }
}

#[test]
fn powerline_solid_edge_skips_translucent_separators() {
    let bounds = Bounds::new(point(px(10.2), px(20.)), size(px(9.2), px(21.)));

    for shape in [
        PowerlineShape::TriangleRight,
        PowerlineShape::TriangleLeft,
        PowerlineShape::HalfCircleRight,
        PowerlineShape::HalfCircleLeft,
        PowerlineShape::SlantLowerLeft,
        PowerlineShape::SlantLowerRight,
        PowerlineShape::SlantUpperLeft,
        PowerlineShape::SlantUpperRight,
    ] {
        assert!(
            powerline_solid_edge(bounds, shape, 1.25, 1.).is_some(),
            "{shape:?} still needs the cover quad when opaque"
        );
        for alpha in [DIM_OPACITY, 0., 0.99, f32::NAN] {
            assert_eq!(
                powerline_solid_edge(bounds, shape, 1.25, alpha),
                None,
                "{shape:?} must not stack a cover quad under a translucent path"
            );
        }
    }
}

#[test]
fn seg_budget_frees_solo_symbols_and_lends_a_cell_only_when_one_is_free() {
    let cell = px(10.);
    assert_eq!(
        seg_budget(true, true, 1, true, cell),
        px(20.),
        "a solo glyph leans into a free cell"
    );
    assert_eq!(
        seg_budget(true, true, 1, false, cell),
        px(10.),
        "but keeps to its own once the next cell is taken"
    );
    assert_eq!(
        seg_budget(false, true, 2, false, cell),
        px(22.5),
        "a quarter cell"
    );
    assert_eq!(
        seg_budget(false, true, 2, true, cell),
        px(30.),
        "a whole cell"
    );
    assert_eq!(seg_budget(false, true, 1, false, cell), px(12.5));
}

#[test]
fn a_fallback_icon_is_fitted_to_its_cell_only_when_the_next_one_is_taken() {
    // Measured on Windows: Maple Mono NF CN supplying U+F059 to a
    // 15px Cascadia Mono grid inks 13.85px across an 8.79px cell.
    let cell = px(8.789);
    let ink = px(13.845);

    let leaning = fit_scale(ink, seg_budget(true, true, 1, true, cell));
    assert_eq!(leaning, 1., "a blank neighbour still lends its cell");

    let crowded = fit_scale(ink, seg_budget(true, true, 1, false, cell));
    assert!(crowded < 1., "an occupied neighbour does not");
    assert!(
        ink * crowded <= cell,
        "and the icon has to end inside its own cell"
    );
}

#[test]
fn a_solo_segment_is_held_to_its_cell_only_where_its_ink_was_measured() {
    let cell = px(10.);
    let budget = |ink, text| seg_budget(true, ink_covers_segment(ink, text), 1, false, cell);

    // One character the face answered bounds for: the whole of it was
    // measured, so `fit_scale` can bring it into one cell and the clip
    // may be drawn there.
    assert!(ink_covers_segment(Some(px(15.)), "\u{f059}"));
    assert_eq!(budget(Some(px(15.)), "\u{f059}"), px(10.));

    // A base with a combining mark hanging off it reaches paint_glyphs as
    // a one-cell `Cluster`, which counts as solo. `ink_extent` read the
    // base alone: Devanagari ka plus the aa matra inks to the right of the
    // base, and none of that overhang is in the 9px. Nothing would shrink
    // it, so a one-cell clip would cut the matra clean off.
    assert!(!ink_covers_segment(Some(px(9.)), "\u{915}\u{93E}"));
    assert_eq!(budget(Some(px(9.)), "\u{915}\u{93E}"), px(20.));

    // A face that answers no bounds at all is the same story from the
    // other side: `fit_scale` is never reached, so halving the clip only
    // clips.
    assert!(!ink_covers_segment(None, "\u{f059}"));
    assert_eq!(budget(None, "\u{f059}"), px(20.));

    // A free neighbour lends its cell either way \u2014 the measurement only
    // decides whether the lean can be withdrawn.
    for ink in [Some(px(15.)), None] {
        for text in ["\u{f059}", "\u{915}\u{93E}"] {
            assert_eq!(
                seg_budget(true, ink_covers_segment(ink, text), 1, true, cell),
                px(20.)
            );
        }
    }
}

/// The emitted feature set is the whole contract with the shaper, so pin it
/// tag for tag rather than asking after one tag at a time.
///
/// `calt: 0` alone is not "ligatures off" on any platform: it never asks
/// for `liga`/`clig`, which every shaper defaults on. Shaping "office
/// waffle fluffier" in Calibri through the real DirectWrite shaper gives 15
/// glyphs for 22 characters with `calt: 0`, and 22 once all three are named
/// zero.
#[test]
fn build_font_emits_the_pinned_feature_set_for_each_ligature_setting() {
    fn tags(font: &Font) -> Vec<(&str, u32)> {
        font.features
            .tag_value_list()
            .iter()
            .map(|(tag, value)| (tag.as_str(), *value))
            .collect()
    }

    // Default config, and the settings toggle switched off: both leave
    // `font_features` unset, so the terminal names its own.
    let font = build_font(&gpui::font("Test"), false, false);
    assert_eq!(
        tags(&font),
        vec![("calt", 0), ("liga", 0), ("clig", 0)],
        "an unconfigured face must name every ligature feature off",
    );
    assert_eq!(font.features.is_calt_enabled(), Some(false));

    // Every face the grid paints with gets the same set, not just the plain one.
    for (bold, italic) in [(true, false), (false, true), (true, true)] {
        assert_eq!(
            tags(&build_font(&gpui::font("Test"), bold, italic)),
            vec![("calt", 0), ("liga", 0), ("clig", 0)],
            "bold={bold} italic={italic}",
        );
    }

    // Explicitly on: what Settings → Appearance → Font ligatures writes.
    let mut configured = gpui::font("Test");
    configured.features = anterm_ui_kit::config::gpui_font_features(
        &serde_json::from_str(r#"{"calt":true,"liga":1}"#).unwrap(),
    );
    let font = build_font(&configured, false, false);
    assert_eq!(
        tags(&font),
        vec![("calt", 1), ("liga", 1)],
        "an explicit request for ligatures must survive untouched",
    );
    assert_eq!(font.features.is_calt_enabled(), Some(true));

    // Explicitly off in `config.json`: also passed through unchanged.
    let mut configured = gpui::font("Test");
    configured.features = anterm_ui_kit::config::gpui_font_features(
        &serde_json::from_str(r#"{"calt":0,"liga":0,"clig":0}"#).unwrap(),
    );
    assert_eq!(
        tags(&build_font(&configured, false, false)),
        vec![("calt", 0), ("liga", 0), ("clig", 0)],
    );
}

#[test]
fn natively_drawn_cells_keep_decorations_owned_by_text_shaping() {
    let plain = GlyphStyle::of(&cell('│'));
    assert_eq!(
        native_cell_residue(&plain),
        None,
        "an unstyled box character has nothing left to shape"
    );

    for kind in [UnderlineKind::Single, UnderlineKind::Curly] {
        let mut c = cell('│');
        c.underline = kind;
        assert_eq!(
            native_cell_residue(&GlyphStyle::of(&c)),
            Some(' '),
            "{kind:?} underline dropped on a box-drawing cell"
        );
    }

    for ch in ['│', '─', '╭', '█', '\u{e0b0}'] {
        let mut c = cell(ch);
        c.link_hover = true;
        assert_eq!(
            native_cell_residue(&GlyphStyle::of(&c)),
            Some(' '),
            "hovered-link underline dropped on U+{:04X}",
            ch as u32
        );
    }
}

#[test]
fn segment_row_keeps_powerline_separators_solo() {
    let row = vec![cell('a'), cell('\u{e0b0}'), cell('\u{e0b4}'), cell('b')];
    assert_eq!(
        segment_row(&row),
        [
            run(0, 1, "a"),
            RowSeg::Solo { col: 1 },
            RowSeg::Solo { col: 2 },
            run(3, 1, "b")
        ]
    );
}

#[test]
fn segment_row_batches_uniform_cjk_wide_runs_and_isolates_emoji() {
    let row = wide_cells("你好世界");
    assert_eq!(segment_row(&row), [wide_run(0, 8, "你好世界")]);

    // Emoji and non-CJK wide symbols remain isolated `RowSeg::Wide` so
    // `ink_bounds` / `fit_scale` continues to handle oversised strikes.
    let emoji_row = wide_cells("🚀🔥");
    assert_eq!(
        segment_row(&emoji_row),
        [wide(0, 2, "🚀"), wide(2, 2, "🔥")]
    );
}

#[test]
fn segment_row_isolates_narrow_fullwidth_punctuation() {
    let row = wide_cells("（这样");
    assert_eq!(
        segment_row(&row),
        [wide(0, 2, "（"), wide_run(2, 4, "这样")]
    );
}

#[test]
fn segment_row_tracks_wide_columns_across_styles_and_gaps() {
    let mut row = wide_cells("你好世界");
    row[4].fg = gpui::red();
    row[6].fg = gpui::red();
    assert_eq!(
        segment_row(&row),
        [wide_run(0, 4, "你好"), wide_run(4, 4, "世界")]
    );

    let mut row = wide_cells("你好");
    row.insert(2, cell(' '));
    assert_eq!(segment_row(&row), [wide(0, 2, "你"), wide(3, 2, "好")]);
}

#[test]
fn segment_row_leaves_spacerless_wide_char_solo() {
    let row = vec![cell('a'), cell('字')];
    assert_eq!(segment_row(&row), [run(0, 1, "a"), RowSeg::Solo { col: 1 }]);
}

#[test]
fn segment_row_ignores_blank_rows() {
    let row: Vec<_> = "  \0 ".chars().map(cell).collect();
    assert!(segment_row(&row).is_empty());
}

fn cluster(col: usize, cells: usize, text: &str) -> RowSeg {
    RowSeg::Cluster {
        col,
        cells,
        text: text.to_string(),
        wide_base: false,
    }
}

fn wide_cluster(col: usize, cells: usize, text: &str) -> RowSeg {
    RowSeg::Cluster {
        col,
        cells,
        text: text.to_string(),
        wide_base: true,
    }
}

#[test]
fn segment_row_shapes_combining_marks_with_their_base() {
    let mut row = vec![cell('a'), cell('e'), cell('b')];
    row[1].marks = Some(Box::from(['\u{0301}']));
    assert_eq!(
        segment_row(&row),
        [run(0, 1, "a"), cluster(1, 1, "e\u{0301}"), run(2, 1, "b"),]
    );

    let mut row = wide_cells("\u{2764}");
    row[0].marks = Some(Box::from(['\u{FE0F}']));
    assert_eq!(segment_row(&row), [wide_cluster(0, 2, "\u{2764}\u{FE0F}")]);

    let mut row = vec![cell('\u{0E17}'), cell('a')];
    row[0].marks = Some(Box::from(['\u{0E35}', '\u{0E48}']));
    assert_eq!(
        segment_row(&row),
        [cluster(0, 1, "\u{0E17}\u{0E35}\u{0E48}"), run(1, 1, "a")]
    );
}

#[test]
fn segment_row_absorbs_sara_am_into_its_base() {
    let mut row = vec![cell('\u{0E19}'), cell('\u{0E33}'), cell('a')];
    row[0].marks = Some(Box::from(['\u{0E49}']));
    assert_eq!(
        segment_row(&row),
        [cluster(0, 2, "\u{0E19}\u{0E49}\u{0E33}"), run(2, 1, "a")]
    );

    let row = vec![cell('\u{0E01}'), cell('\u{0E33}')];
    assert_eq!(segment_row(&row), [cluster(0, 2, "\u{0E01}\u{0E33}")]);

    let row = vec![cell('\u{0E81}'), cell('\u{0EB3}')];
    assert_eq!(segment_row(&row), [cluster(0, 2, "\u{0E81}\u{0EB3}")]);

    let mut row = vec![cell('\u{0E01}'), cell('\u{0E33}')];
    row[1].fg = gpui::red();
    assert_eq!(segment_row(&row), [cluster(0, 2, "\u{0E01}\u{0E33}")]);
}

#[test]
fn segment_row_leaves_a_baseless_sara_am_alone() {
    let row = vec![cell('\u{0E33}'), cell('a')];
    assert_eq!(segment_row(&row), [RowSeg::Solo { col: 0 }, run(1, 1, "a")]);

    let row = vec![cell(' '), cell('\u{0E33}')];
    assert_eq!(segment_row(&row), [RowSeg::Solo { col: 1 }]);

    let row = vec![cell('\u{0E33}'), cell('\u{0E33}')];
    assert_eq!(
        segment_row(&row),
        [RowSeg::Solo { col: 0 }, RowSeg::Solo { col: 1 }]
    );
}

#[test]
fn segment_row_joins_a_regional_indicator_pair() {
    let row = vec![cell('\u{1F1E8}'), cell('\u{1F1F3}')];
    assert_eq!(segment_row(&row), [cluster(0, 2, "\u{1F1E8}\u{1F1F3}")]);

    let row = vec![cell('a'), cell('\u{1F1E8}'), cell('\u{1F1F3}'), cell('b')];
    assert_eq!(
        segment_row(&row),
        [
            run(0, 1, "a"),
            cluster(1, 2, "\u{1F1E8}\u{1F1F3}"),
            run(3, 1, "b"),
        ]
    );

    let row = vec![
        cell('\u{1F1E8}'),
        cell('\u{1F1F3}'),
        cell('\u{1F1FA}'),
        cell('\u{1F1F8}'),
    ];
    assert_eq!(
        segment_row(&row),
        [
            cluster(0, 2, "\u{1F1E8}\u{1F1F3}"),
            cluster(2, 2, "\u{1F1FA}\u{1F1F8}"),
        ]
    );

    let mut row = vec![cell('\u{1F1E8}'), cell('\u{1F1F3}')];
    row[1].fg = gpui::red();
    assert_eq!(segment_row(&row), [cluster(0, 2, "\u{1F1E8}\u{1F1F3}")]);

    let mut row = vec![cell('\u{1F1E8}'), cell('\u{1F1F3}')];
    row[0].marks = Some(Box::from(['\u{FE0F}']));
    assert_eq!(
        segment_row(&row),
        [cluster(0, 2, "\u{1F1E8}\u{FE0F}\u{1F1F3}")]
    );

    let mut row = vec![cell('\u{1F1E8}'), cell('\u{1F1F3}')];
    row[1].marks = Some(Box::from(['\u{FE0F}']));
    assert_eq!(
        segment_row(&row),
        [cluster(0, 2, "\u{1F1E8}\u{1F1F3}\u{FE0F}")]
    );
}

#[test]
fn segment_row_leaves_an_unpaired_regional_indicator_alone() {
    let row = vec![cell('\u{1F1E8}')];
    assert_eq!(segment_row(&row), [RowSeg::Solo { col: 0 }]);

    let row = vec![cell('\u{1F1E8}'), cell('\u{1F1F3}'), cell('\u{1F1FA}')];
    assert_eq!(
        segment_row(&row),
        [cluster(0, 2, "\u{1F1E8}\u{1F1F3}"), RowSeg::Solo { col: 2 }]
    );

    let row = vec![cell('\u{1F1E8}'), cell('a')];
    assert_eq!(segment_row(&row), [RowSeg::Solo { col: 0 }, run(1, 1, "a")]);

    let row = vec![cell('\u{1F1E8}'), cell(' '), cell('\u{1F1F3}')];
    assert_eq!(
        segment_row(&row),
        [RowSeg::Solo { col: 0 }, RowSeg::Solo { col: 2 }]
    );
}

#[test]
fn a_regional_indicator_pair_reaches_segment_row_as_two_plain_columns() {
    let mut term = alacritty_terminal::Term::new(
        alacritty_terminal::term::Config::default(),
        &crate::backend::size::TermSize::new(80, 24),
        alacritty_terminal::event::VoidListener,
    );
    let mut parser: alacritty_terminal::vte::ansi::Processor =
        alacritty_terminal::vte::ansi::Processor::new();
    parser.advance(&mut term, "\u{1F1E8}\u{1F1F3}x".as_bytes());

    let palette = [Rgb { r: 0, g: 0, b: 0 }; 256];
    let colors = test_colors();
    let row: Vec<_> = (0..4)
        .map(|col| {
            let point = AlacPoint::new(AlacLine(0), AlacColumn(col));
            snapshot_cell(&term.grid()[point], point, &palette, &colors, None)
        })
        .collect();
    assert_eq!(
        segment_row(&row),
        [cluster(0, 2, "\u{1F1E8}\u{1F1F3}"), run(2, 1, "x")]
    );
}

#[test]
fn segment_row_never_batches_a_marked_cell() {
    let mut row: Vec<_> = "abc".chars().map(cell).collect();
    row[1].marks = Some(Box::from(['\u{0301}']));
    assert_eq!(
        segment_row(&row),
        [run(0, 1, "a"), cluster(1, 1, "b\u{0301}"), run(2, 1, "c"),]
    );

    let mut row = wide_cells("你好世");
    row[2].marks = Some(Box::from(['\u{FE0F}']));
    assert_eq!(
        segment_row(&row),
        [
            wide(0, 2, "你"),
            wide_cluster(2, 2, "好\u{FE0F}"),
            wide(4, 2, "世"),
        ]
    );
}

#[test]
fn segment_row_keeps_a_blank_that_carries_marks() {
    let mut row = vec![cell(' '), cell(' ')];
    row[0].marks = Some(Box::from(['\u{0301}']));
    assert_eq!(segment_row(&row), [cluster(0, 1, " \u{0301}")]);
}

#[test]
fn char_string_memoizes_per_char() {
    let a = char_string('界');
    let b = char_string('界');
    assert_eq!(a, b);
    assert_eq!(a.as_ref(), "界");
}

pub(super) fn test_colors() -> PaintColors {
    let fg = Rgb {
        r: 10,
        g: 10,
        b: 10,
    };
    let bg = Rgb {
        r: 250,
        g: 250,
        b: 245,
    };
    let wash = |a: f32| {
        let mut c = to_hsla(Rgb {
            r: 0x4a,
            g: 0x43,
            b: 0x39,
        });
        c.a = a;
        c
    };
    PaintColors {
        default_fg: to_hsla(fg),
        default_bg: to_hsla(bg),
        caret: to_hsla(fg),
        selection_bg: wash(0.55),
        match_bg: wash(0.32),
        current_match_bg: wash(0.85),
        fg_rgb: fg,
        bg_rgb: bg,
        legible_dim: true,
    }
}

#[test]
fn selected_cells_keep_their_own_colors_for_the_translucent_wash() {
    let mut palette = [Rgb { r: 0, g: 0, b: 0 }; 256];
    palette[1] = Rgb {
        r: 0xcc,
        g: 0x22,
        b: 0x22,
    };
    palette[2] = Rgb {
        r: 0x22,
        g: 0x88,
        b: 0x22,
    };
    let colors = test_colors();
    let point = AlacPoint::new(AlacLine(0), AlacColumn(0));
    let range = SelectionRange::new(point, point, false);

    let swatch = Cell {
        bg: AnsiColor::Indexed(1),
        ..Cell::default()
    };
    let rc = snapshot_cell(&swatch, point, &palette, &colors, Some(&range));
    assert!(rc.selected);
    assert!(rc.draw_bg, "the swatch background still paints");
    assert_eq!(rc.bg, to_hsla(palette[1]), "background not replaced");

    let text = Cell {
        c: 'x',
        fg: AnsiColor::Indexed(2),
        ..Cell::default()
    };
    let rc = snapshot_cell(&text, point, &palette, &colors, Some(&range));
    assert!(rc.selected);
    assert_eq!(rc.fg, to_hsla(palette[2]), "foreground not forced");

    assert!(colors.selection_bg.a < 1.0);
}

#[test]
fn inverse_swaps_colors_and_always_paints_the_background() {
    let mut palette = [Rgb { r: 0, g: 0, b: 0 }; 256];
    palette[2] = Rgb {
        r: 0x22,
        g: 0x88,
        b: 0x22,
    };
    let colors = test_colors();
    let point = AlacPoint::new(AlacLine(0), AlacColumn(0));

    let cell = Cell {
        c: 'x',
        fg: AnsiColor::Indexed(2),
        flags: Flags::INVERSE,
        ..Cell::default()
    };
    let rc = snapshot_cell(&cell, point, &palette, &colors, None);
    assert_eq!(rc.fg, to_hsla(colors.bg_rgb), "fg takes the old background");
    assert_eq!(rc.bg, to_hsla(palette[2]), "bg takes the old foreground");
    assert!(
        rc.draw_bg,
        "inverse cells paint their background even on the default bg"
    );
}

#[test]
fn hidden_paints_the_foreground_as_the_background() {
    let mut palette = [Rgb { r: 0, g: 0, b: 0 }; 256];
    palette[1] = Rgb {
        r: 0xcc,
        g: 0x22,
        b: 0x22,
    };
    let colors = test_colors();
    let point = AlacPoint::new(AlacLine(0), AlacColumn(0));

    let on_default = Cell {
        c: 's',
        fg: AnsiColor::Indexed(1),
        flags: Flags::HIDDEN,
        ..Cell::default()
    };
    let rc = snapshot_cell(&on_default, point, &palette, &colors, None);
    assert_eq!(rc.fg, rc.bg, "concealed text is invisible");
    assert_eq!(rc.fg, to_hsla(colors.bg_rgb));
    assert!(!rc.draw_bg);

    let on_colored = Cell {
        c: 's',
        bg: AnsiColor::Indexed(1),
        flags: Flags::HIDDEN,
        ..Cell::default()
    };
    let rc = snapshot_cell(&on_colored, point, &palette, &colors, None);
    assert_eq!(rc.fg, to_hsla(palette[1]));
    assert_eq!(rc.fg, rc.bg);
}

#[test]
fn dim_flag_reduces_foreground_intensity() {
    let palette = [Rgb { r: 0, g: 0, b: 0 }; 256];
    let colors = test_colors();
    let point = AlacPoint::new(AlacLine(0), AlacColumn(0));
    let cell = Cell {
        c: 'f',
        flags: Flags::DIM,
        ..Cell::default()
    };

    let rc = snapshot_cell(&cell, point, &palette, &colors, None);

    assert_eq!(rc.fg.h, colors.default_fg.h);
    assert_eq!(rc.fg.s, colors.default_fg.s);
    assert_eq!(rc.fg.l, colors.default_fg.l);
    assert!(
        rc.fg.a < colors.default_fg.a,
        "SGR 2 text must paint with reduced intensity"
    );
}

/// What a cell's foreground lands on screen as: its rgb composited over
/// the cell background by its alpha, the way the glyph is actually drawn.
fn on_screen(fg: Hsla, under: Rgb) -> u32 {
    let c = Rgba::from(fg);
    let ch = |v: f32, u: u8| ((v * c.a + (u as f32 / 255.) * (1. - c.a)) * 255.).round() as u32;
    ch(c.r, under.r) << 16 | ch(c.g, under.g) << 8 | ch(c.b, under.b)
}

/// The colours a pane on builtin `t` resolves cells with.
fn builtin_colors(t: &anterm_ui_kit::presets::Theme) -> (PaintColors, [Rgb; 256]) {
    let (fg, bg) = (unpack_rgb(t.foreground), unpack_rgb(t.background_color()));
    let mut colors = test_colors();
    colors.default_fg = to_hsla(fg);
    colors.default_bg = to_hsla(bg);
    colors.fg_rgb = fg;
    colors.bg_rgb = bg;
    let mut palette = crate::render::palette::build();
    palette[..16].copy_from_slice(&t.active_palette(true).ansi16);
    (colors, palette)
}

/// Every colour faint text is commonly written in: the default foreground,
/// the sixteen palette slots, the 256-colour greys apps reach for as
/// "muted", and a truecolour grey.
fn faint_samples() -> Vec<(String, AnsiColor)> {
    let mut v = vec![("fg".to_string(), AnsiColor::Named(NamedColor::Foreground))];
    for i in 0..16u8 {
        v.push((format!("ansi{i}"), AnsiColor::Indexed(i)));
    }
    for i in [240u8, 244, 248, 250] {
        v.push((format!("256:{i}"), AnsiColor::Indexed(i)));
    }
    v.push((
        "#999999".to_string(),
        AnsiColor::Spec(Rgb {
            r: 153,
            g: 153,
            b: 153,
        }),
    ));
    v
}

/// (plain, faint) on-screen colours of `color` as a pane on `t` paints them.
fn plain_and_faint(
    colors: &PaintColors,
    palette: &[Rgb; 256],
    color: AnsiColor,
) -> (RenderCell, RenderCell) {
    let point = AlacPoint::new(AlacLine(0), AlacColumn(0));
    let mut cell = Cell {
        c: 'x',
        fg: color,
        ..Cell::default()
    };
    let plain = snapshot_cell(&cell, point, palette, colors, None);
    cell.flags = Flags::DIM;
    let faint = snapshot_cell(&cell, point, palette, colors, None);
    (plain, faint)
}

#[test]
fn faint_text_keeps_the_text_floor_on_every_light_builtin() {
    // #858: SGR 2 painted the ink at 66% over the cell, which on a light
    // background took Catppuccin Latte's foreground to 3.2:1 and every
    // bright-black the palette rescue had lifted to 4.5:1 back to ~2.5:1.
    // Faint text on a light cell must now clear 4.5:1 — or, for an ink
    // that never cleared it undimmed, stay at the ink's own ratio.
    use anterm_ui_kit::presets::{builtins, contrast};
    let mut failures = Vec::new();
    eprintln!("| theme | colour | plain | faint |");
    for t in builtins().into_iter().filter(|t| !t.dark) {
        let (colors, palette) = builtin_colors(&t);
        let bg = t.background_color();
        for (name, color) in faint_samples() {
            let (plain, faint) = plain_and_faint(&colors, &palette, color);
            let plain = contrast(on_screen(plain.fg, colors.bg_rgb), bg);
            let faint = contrast(on_screen(faint.fg, colors.bg_rgb), bg);
            eprintln!("| {} | {name} | {plain:.2} | {faint:.2} |", t.id);
            if faint < 4.5_f32.min(plain) - 0.05 {
                failures.push(format!(
                    "{}/{name}: faint {faint:.2}:1 (plain {plain:.2}:1)",
                    t.id
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "faint text under the floor:\n{}",
        failures.join("\n")
    );
}

#[test]
fn faint_text_on_dark_builtins_is_the_plain_fade() {
    // The floor is a light-background rescue: every dark builtin must
    // keep painting SGR 2 exactly as it did, as the ink at DIM_OPACITY.
    for t in anterm_ui_kit::presets::builtins()
        .into_iter()
        .filter(|t| t.dark)
    {
        let (colors, palette) = builtin_colors(&t);
        for (name, color) in faint_samples() {
            let (plain, faint) = plain_and_faint(&colors, &palette, color);
            let mut fade = plain.fg;
            fade.a *= DIM_OPACITY;
            assert_eq!(
                faint.fg, fade,
                "{}/{name}: dark theme faint text changed",
                t.id
            );
        }
    }
}

#[test]
fn faint_text_stays_fainter_than_plain_text() {
    // The rescue buys legibility, not a restyle: faint text never gains
    // contrast over its own ink, and an ink with room to spare above the
    // floor still reads visibly fainter.
    use anterm_ui_kit::presets::{builtins, contrast};
    for t in builtins() {
        let (colors, palette) = builtin_colors(&t);
        let bg = t.background_color();
        for (name, color) in faint_samples() {
            let (plain, faint) = plain_and_faint(&colors, &palette, color);
            let plain = contrast(on_screen(plain.fg, colors.bg_rgb), bg);
            let faint = contrast(on_screen(faint.fg, colors.bg_rgb), bg);
            assert!(
                faint <= plain + 0.01,
                "{}/{name}: faint {faint:.2} > plain {plain:.2}",
                t.id
            );
            if plain >= 6.0 {
                assert!(
                    faint <= plain * 0.85,
                    "{}/{name}: faint {faint:.2} no longer reads fainter than {plain:.2}",
                    t.id
                );
            }
        }
    }
}

#[test]
fn faint_text_is_the_plain_fade_with_the_legibility_switch_off() {
    // Settings → Appearance's palette switch off renders colours as
    // authored; that includes faint text.
    for t in anterm_ui_kit::presets::builtins() {
        let (mut colors, palette) = builtin_colors(&t);
        colors.legible_dim = false;
        for (name, color) in faint_samples() {
            let (plain, faint) = plain_and_faint(&colors, &palette, color);
            let mut fade = plain.fg;
            fade.a *= DIM_OPACITY;
            assert_eq!(faint.fg, fade, "{}/{name}: switch off still rescued", t.id);
        }
    }
}

#[test]
fn blend_toward_mixes_in_rgb_space_and_keeps_alpha() {
    let under = Rgba {
        r: 16. / 255.,
        g: 18. / 255.,
        b: 19. / 255.,
        a: 1.,
    };
    let red = to_hsla(Rgb {
        r: 218,
        g: 98,
        b: 125,
    });
    let half = Rgba::from(blend_toward(red, 0.5, under));
    assert!((half.r - 0.5 * (218. / 255.) - 0.5 * (16. / 255.)).abs() < 1e-3);
    assert!((half.g - 0.5 * (98. / 255.) - 0.5 * (18. / 255.)).abs() < 1e-3);
    assert!((half.b - 0.5 * (125. / 255.) - 0.5 * (19. / 255.)).abs() < 1e-3);
    assert_eq!(half.a, 1.0, "an opaque colour stays opaque");

    // A translucent tint keeps its own alpha: only the rgb is blended.
    let tint = Hsla { a: 0.24, ..red };
    let dimmed = Rgba::from(blend_toward(tint, 0.55, under));
    assert!((dimmed.a - 0.24).abs() < 1e-3, "alpha is untouched");
    assert!(
        (dimmed.r - 0.55 * (218. / 255.) - 0.45 * (16. / 255.)).abs() < 1e-3,
        "rgb still blends toward the under"
    );
}

#[test]
fn dimming_a_stacked_fill_stays_continuous_with_its_segment() {
    // The reported bug: a powerline separator (fill) drawn over its
    // segment (bg) dimmed unevenly because the old pane element-opacity
    // alpha-multiplied each layer, so the separator kept the segment's own
    // dim visible through its (1 - dim) and landed with a seam against
    // the segment (brighter on this palette). Blending every colour
    // toward the under first keeps the composite exactly as dimmed as any
    // single layer.
    let under = Rgba {
        r: 16. / 255.,
        g: 18. / 255.,
        b: 19. / 255.,
        a: 1.,
    };
    let segment = to_hsla(Rgb {
        r: 218,
        g: 98,
        b: 125,
    });
    let fill = to_hsla(Rgb {
        r: 154,
        g: 52,
        b: 142,
    });
    let dim = 0.55;

    let over = |top: Rgba, bottom: Rgba| Rgba {
        r: top.r * top.a + bottom.r * (1. - top.a),
        g: top.g * top.a + bottom.g * (1. - top.a),
        b: top.b * top.a + bottom.b * (1. - top.a),
        a: top.a + bottom.a * (1. - top.a),
    };

    // New rendering: both layers are pre-blended and stay opaque, so the
    // fill over the segment composites to exactly the dimmed fill — the
    // same value the segment itself renders as, with no seam between them.
    let dimmed_segment = Rgba::from(blend_toward(segment, dim, under));
    let dimmed_fill = Rgba::from(blend_toward(fill, dim, under));
    let new_composite = over(dimmed_fill, dimmed_segment);
    assert!((new_composite.r - dimmed_fill.r).abs() < 1e-6);
    assert!((new_composite.g - dimmed_fill.g).abs() < 1e-6);
    assert!((new_composite.b - dimmed_fill.b).abs() < 1e-6);

    // Old rendering: the pane opacity turned both layers translucent, so
    // the fill showed the already-dimmed segment underneath and painted
    // with a seam against the segment next to it.
    let old_fill = Rgba {
        a: dim,
        ..Rgba::from(fill)
    };
    let old_segment = Rgba {
        a: dim,
        ..Rgba::from(segment)
    };
    let old_composite = over(old_fill, old_segment);
    assert!(
        old_composite.r > new_composite.r + 0.05,
        "on this palette the old alpha-multiplied separator was visibly brighter than the segment"
    );
}

#[test]
fn dim_cell_blends_truecolor_cells_toward_the_under() {
    // Starship's prompt paints its segments with direct 38;2;/48;2; SGR
    // colours that a palette-level dim would never see; the pane dim must
    // reach those cells too, or the whole prompt would stay at full
    // brightness while indexed content around it dims.
    let under = Rgba {
        r: 16. / 255.,
        g: 18. / 255.,
        b: 19. / 255.,
        a: 1.,
    };
    let mut cell = RenderCell::default();
    cell.c = 'x';
    cell.fg = to_hsla(Rgb {
        r: 218,
        g: 98,
        b: 125,
    });
    cell.bg = to_hsla(Rgb {
        r: 154,
        g: 52,
        b: 142,
    });
    cell.draw_bg = true;

    let dimmed = dim_cell(cell, 0.55, under);
    assert_eq!(dimmed.c, 'x', "the character itself survives");
    assert!(dimmed.draw_bg, "the explicit background flag survives");
    let fg = Rgba::from(dimmed.fg);
    assert!((fg.r - (0.55 * 218. + 0.45 * 16.) / 255.).abs() < 1e-3);
    assert!((fg.g - (0.55 * 98. + 0.45 * 18.) / 255.).abs() < 1e-3);
    assert!((fg.b - (0.55 * 125. + 0.45 * 19.) / 255.).abs() < 1e-3);
    let bg = Rgba::from(dimmed.bg);
    assert!((bg.r - (0.55 * 154. + 0.45 * 16.) / 255.).abs() < 1e-3);
    assert_eq!(fg.a, 1.0, "opaque cell colours stay opaque");

    // A DIM-flagged cell keeps its reduced alpha through the blend.
    let mut dim_flag = RenderCell::default();
    dim_flag.fg = to_hsla(Rgb {
        r: 218,
        g: 98,
        b: 125,
    });
    dim_flag.fg.a = DIM_OPACITY;
    let dimmed_flag = dim_cell(dim_flag, 0.55, under);
    assert!(
        (dimmed_flag.fg.a - DIM_OPACITY).abs() < 1e-3,
        "the SGR dim alpha survives the pane dim"
    );
}

#[test]
fn paint_colors_dimmed_keeps_translucent_tint_alphas() {
    let colors = caret_colors();
    let under = Rgba {
        r: 16. / 255.,
        g: 18. / 255.,
        b: 19. / 255.,
        a: 1.,
    };
    let dimmed = colors.dimmed(0.55, under);
    assert!((dimmed.caret.a - colors.caret.a).abs() < 1e-3);
    assert!(
        (dimmed.selection_bg.a - colors.selection_bg.a).abs() < 1e-3,
        "translucent overlays keep their own alpha so they still tint"
    );
    assert_ne!(dimmed.default_fg, colors.default_fg);
    // The named foreground/background rgb feed `resolve` for default cells,
    // which `dim_cell` blends — pre-blending them here would double-dim.
    assert_eq!(dimmed.fg_rgb, colors.fg_rgb);
    assert_eq!(dimmed.bg_rgb, colors.bg_rgb);
}

#[test]
fn premultiplied_under_scales_rgb_by_the_window_opacity_and_stays_opaque() {
    // A dimmed pane blends toward the colour the workspace actually paints
    // behind it: the fill scaled by the window's own opacity (premultiplied
    // alpha), never toward a colour that includes the OS backdrop.
    let u = premultiplied(0xda_62_7d, 0.82);
    assert_eq!(u.a, 1.0, "the under must stay an opaque paint colour");
    assert!((u.r - 0.82 * (218. / 255.)).abs() < 1e-6);
    assert!((u.g - 0.82 * (98. / 255.)).abs() < 1e-6);
    assert!((u.b - 0.82 * (125. / 255.)).abs() < 1e-6);
    // A fully opaque window keeps the fill untouched.
    let opaque = premultiplied(0xda_62_7d, 1.0);
    assert!((opaque.r - 218. / 255.).abs() < 1e-6);
    assert!((opaque.g - 98. / 255.).abs() < 1e-6);
    assert!((opaque.b - 125. / 255.).abs() < 1e-6);
}

#[test]
fn fill_under_maps_preset_fills_to_their_premultiplied_under() {
    use anterm_ui_kit::presets::Fill;

    // A solid fill is exactly the colour the workspace paints behind the
    // terminal, scaled by the window's own opacity.
    let solid = fill_under(&Fill::Solid(0xda_62_7d), Some(0.82));
    assert!((solid.r - 0.82 * (218. / 255.)).abs() < 1e-6);
    assert!((solid.g - 0.82 * (98. / 255.)).abs() < 1e-6);
    assert!((solid.b - 0.82 * (125. / 255.)).abs() < 1e-6);
    assert_eq!(solid.a, 1.0);

    // A gradient is approximated by its midpoint stop (`mix` rounds each
    // channel, so 0x00…ff lands on 128/255 rather than exactly 0.5).
    let vertical = fill_under(
        &Fill::Vertical {
            top: 0x00_00_00,
            bottom: 0xff_ff_ff,
        },
        None,
    );
    assert!((vertical.r - 128. / 255.).abs() < 1e-6);
    assert_eq!(vertical.r, vertical.g);
    assert_eq!(vertical.r, vertical.b);
    let horizontal = fill_under(
        &Fill::Horizontal {
            left: 0xff_00_00,
            right: 0x00_00_ff,
        },
        None,
    );
    assert!((horizontal.r - 128. / 255.).abs() < 1e-6);
    assert!((horizontal.b - 128. / 255.).abs() < 1e-6);

    // No explicit opacity means the fill is used at full strength.
    let full = fill_under(&Fill::Solid(0xda_62_7d), None);
    assert!((full.r - 218. / 255.).abs() < 1e-6);
}

#[test]
fn theme_under_premultiplies_by_the_theme_background_alpha() {
    // The no-preset fallback mirrors `theme().background`, whose alpha is
    // the window's own opacity.
    let bg = to_hsla(Rgb {
        r: 218,
        g: 98,
        b: 125,
    });
    let mut bg = bg;
    bg.a = 0.82;
    let under = theme_under(bg);
    assert_eq!(under.a, 1.0);
    assert!((under.r - 0.82 * (218. / 255.)).abs() < 1e-6);
    assert!((under.g - 0.82 * (98. / 255.)).abs() < 1e-6);
    assert!((under.b - 0.82 * (125. / 255.)).abs() < 1e-6);
    // An opaque theme token is used untouched.
    let opaque = theme_under(to_hsla(Rgb {
        r: 218,
        g: 98,
        b: 125,
    }));
    assert!((opaque.r - 218. / 255.).abs() < 1e-6);
}

#[test]
fn underline_flag_bits_map_to_their_variants() {
    let palette = [Rgb { r: 0, g: 0, b: 0 }; 256];
    let colors = test_colors();
    let point = AlacPoint::new(AlacLine(0), AlacColumn(0));
    let kind = |flags: Flags| {
        let cell = Cell {
            c: 'u',
            flags,
            ..Cell::default()
        };
        snapshot_cell(&cell, point, &palette, &colors, None).underline
    };

    assert_eq!(kind(Flags::empty()), UnderlineKind::None);
    assert_eq!(kind(Flags::UNDERLINE), UnderlineKind::Single);
    assert_eq!(kind(Flags::DOUBLE_UNDERLINE), UnderlineKind::Double);
    assert_eq!(kind(Flags::UNDERCURL), UnderlineKind::Curly);
    assert_eq!(kind(Flags::DOTTED_UNDERLINE), UnderlineKind::Dotted);
    assert_eq!(kind(Flags::DASHED_UNDERLINE), UnderlineKind::Dashed);
    assert_eq!(
        kind(Flags::UNDERLINE | Flags::UNDERCURL),
        UnderlineKind::Curly
    );
}

#[test]
fn strikeout_flag_maps_to_a_text_decoration() {
    let palette = [Rgb { r: 0, g: 0, b: 0 }; 256];
    let colors = test_colors();
    let point = AlacPoint::new(AlacLine(0), AlacColumn(0));
    let cell = Cell {
        c: 's',
        flags: Flags::STRIKEOUT,
        ..Cell::default()
    };

    let rc = snapshot_cell(&cell, point, &palette, &colors, None);
    let style = GlyphStyle::of(&rc).strikethrough_style().unwrap();
    assert_eq!(style.thickness, px(1.));
    assert_eq!(style.color, Some(rc.fg));
}

#[test]
fn special_underlines_have_distinct_device_pixel_geometry() {
    let bounds = Bounds::new(point(px(0.), px(0.)), size(px(12.), px(20.)));
    let ink = |kind| {
        let mut rects = Vec::new();
        for_each_special_underline(bounds, kind, 2., |rect| rects.push(rect));
        rects
    };

    let double = ink(UnderlineKind::Double);
    assert_eq!(
        double.len(),
        2,
        "double underline uses two separate strokes"
    );
    assert_ne!(double[0].origin.y, double[1].origin.y);

    let dotted = ink(UnderlineKind::Dotted);
    let dashed = ink(UnderlineKind::Dashed);
    assert!(
        dotted.len() > dashed.len(),
        "dots repeat more often than dashes"
    );
    assert!(dotted.len() > 1 && dashed.len() > 1);
    assert!(
        dotted.iter().all(|r| r.size.width == px(1.)),
        "each dot is a logical pixel wide at 2x"
    );
    assert!(
        dashed.iter().all(|r| r.size.width <= px(3.)),
        "each dash is at most three logical pixels wide at 2x"
    );
}

#[test]
fn a_link_is_pointed_out_faintly_until_the_modifier_is_down() {
    let mut plain = cell('l');
    plain.fg = gpui::hsla(0., 0., 0.9, 1.);

    assert_eq!(
        link_underline_color(&plain, true),
        None,
        "armed, the underline is the text's own colour"
    );
    let held = link_underline_color(&plain, false).expect("a colour of its own");
    assert!(
        held.a < plain.fg.a,
        "held back, it is the same colour with less of it"
    );
    assert_eq!(
        (held.h, held.s, held.l),
        (plain.fg.h, plain.fg.s, plain.fg.l)
    );

    let mut own = cell('l');
    own.underline = UnderlineKind::Curly;
    own.underline_color = Some(gpui::hsla(0.1, 1., 0.5, 1.));
    assert_eq!(
        link_underline_color(&own, false),
        own.underline_color,
        "a spelling mistake stays the colour the application painted it"
    );
}

#[test]
fn special_underlines_are_not_overpainted_by_link_hover() {
    for kind in [
        UnderlineKind::Double,
        UnderlineKind::Dotted,
        UnderlineKind::Dashed,
    ] {
        let mut c = cell('l');
        c.underline = kind;
        c.link_hover = true;
        assert_eq!(GlyphStyle::of(&c).underline_style(), None, "{kind:?}");
    }
}

#[test]
fn native_cells_keep_only_decorations_that_need_glyph_shaping() {
    for kind in [
        UnderlineKind::Double,
        UnderlineKind::Dotted,
        UnderlineKind::Dashed,
    ] {
        let mut c = cell('│');
        c.underline = kind;
        assert_eq!(native_cell_residue(&GlyphStyle::of(&c)), None);
    }

    let mut c = cell('│');
    c.strikeout = true;
    assert_eq!(native_cell_residue(&GlyphStyle::of(&c)), Some(' '));
}

#[test]
fn sgr58_underline_color_resolves_through_the_palette() {
    let mut palette = [Rgb { r: 0, g: 0, b: 0 }; 256];
    palette[5] = Rgb {
        r: 0xd0,
        g: 0x30,
        b: 0x30,
    };
    let colors = test_colors();
    let point = AlacPoint::new(AlacLine(0), AlacColumn(0));

    let mut cell = Cell {
        c: 'e',
        flags: Flags::UNDERCURL,
        ..Cell::default()
    };
    cell.set_underline_color(Some(AnsiColor::Indexed(5)));
    let rc = snapshot_cell(&cell, point, &palette, &colors, None);
    assert_eq!(rc.underline_color, Some(to_hsla(palette[5])));

    let plain = Cell {
        c: 'e',
        flags: Flags::UNDERCURL,
        ..Cell::default()
    };
    let rc = snapshot_cell(&plain, point, &palette, &colors, None);
    assert_eq!(
        rc.underline_color, None,
        "no SGR 58 → paint falls back to fg"
    );
}

#[test]
fn bold_italic_flag_sets_both_emphases() {
    let palette = [Rgb { r: 0, g: 0, b: 0 }; 256];
    let colors = test_colors();
    let point = AlacPoint::new(AlacLine(0), AlacColumn(0));
    let emphases = |flags: Flags| {
        let cell = Cell {
            c: 'b',
            flags,
            ..Cell::default()
        };
        let rc = snapshot_cell(&cell, point, &palette, &colors, None);
        (rc.bold, rc.italic)
    };

    assert_eq!(emphases(Flags::BOLD), (true, false));
    assert_eq!(emphases(Flags::ITALIC), (false, true));
    assert_eq!(emphases(Flags::BOLD_ITALIC), (true, true));
}

#[test]
fn wide_char_spacers_defer_to_the_leading_cell() {
    let mut palette = [Rgb { r: 0, g: 0, b: 0 }; 256];
    palette[1] = Rgb {
        r: 0xcc,
        g: 0x22,
        b: 0x22,
    };
    let colors = test_colors();
    let point = AlacPoint::new(AlacLine(0), AlacColumn(0));

    for flags in [Flags::WIDE_CHAR_SPACER, Flags::LEADING_WIDE_CHAR_SPACER] {
        let cell = Cell {
            c: ' ',
            bg: AnsiColor::Indexed(1),
            flags: flags | Flags::UNDERLINE,
            ..Cell::default()
        };
        let rc = snapshot_cell(&cell, point, &palette, &colors, None);
        assert!(rc.spacer);
        assert!(!rc.draw_bg, "spacers never paint");
        assert_eq!(rc.underline, UnderlineKind::None);
    }
}

#[test]
fn only_real_combining_marks_set_marks() {
    let palette = [Rgb { r: 0, g: 0, b: 0 }; 256];
    let colors = test_colors();
    let point = AlacPoint::new(AlacLine(0), AlacColumn(0));

    let mut colored = Cell {
        c: 'e',
        flags: Flags::UNDERCURL,
        ..Cell::default()
    };
    colored.set_underline_color(Some(AnsiColor::Indexed(5)));
    assert_eq!(
        snapshot_cell(&colored, point, &palette, &colors, None).marks,
        None,
        "SGR 58 alone is not a combining mark"
    );

    let mut linked = Cell {
        c: 'e',
        ..Cell::default()
    };
    linked.set_hyperlink(Some(alacritty_terminal::term::cell::Hyperlink::new(
        Some("id"),
        "https://example.com".to_string(),
    )));
    assert_eq!(
        snapshot_cell(&linked, point, &palette, &colors, None).marks,
        None,
        "OSC 8 alone is not a combining mark"
    );

    let mut marked = Cell {
        c: '\u{2764}',
        ..Cell::default()
    };
    marked.push_zerowidth('\u{FE0F}');
    assert_eq!(
        snapshot_cell(&marked, point, &palette, &colors, None)
            .marks
            .as_deref(),
        Some(&['\u{FE0F}'][..]),
    );
}

fn ink(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
}

/// Menlo at 22px: a 13.2px cell in a 30.8px row (line height 1.4).
const CELL: f32 = 13.2;
const EM: f32 = 22.;

fn fit(ink: Bounds<Pixels>, cells: usize, lend: bool) -> (f32, usize) {
    icon_fit(ink, px(CELL), cells, lend, px(EM))
}

#[test]
fn only_private_use_code_points_count_as_icons() {
    for c in [
        '\u{e000}',   // first of the BMP Private Use Area
        '\u{e0a0}',   // Powerline branch
        '\u{e0d8}',   // first past the Powerline separators
        '\u{f179}',   // Font Awesome apple
        '\u{f8ff}',   // last of the BMP Private Use Area
        '\u{f0001}',  // Material Design icon, Supplementary PUA-A
        '\u{10fffd}', // end of Supplementary PUA-B
    ] {
        assert!(is_icon(c), "U+{:04X}", c as u32);
    }
    // Text a fallback face supplies keeps its own metrics: CJK, emoji,
    // Latin, and the drawing characters painted natively anyway.
    for c in [
        '中',
        'あ',
        '한',
        '\u{1F600}',
        '\u{2764}',
        'M',
        '─',
        '█',
        '\u{f900}',
    ] {
        assert!(!is_icon(c), "U+{:04X}", c as u32);
    }
    // Nor do the Powerline separators, which are shapes, not icons.
    for c in ['\u{e0b0}', '\u{e0b1}', '\u{e0b3}', '\u{e0d7}'] {
        assert!(!is_icon(c), "U+{:04X}", c as u32);
    }
}

#[test]
fn a_nerd_font_icon_before_a_space_takes_two_cells_at_full_size() {
    // Symbols Nerd Font Mono's apple, as CoreText measures it at 22px: a
    // full em tall and 1.4 cells wide. Held to one cell it is shrunk to
    // 0.71 of the text's size, which is what #866 saw; with the space
    // after it lent, it keeps its whole em.
    let apple = ink(1.76, -4.43, 18.48, 22.03);
    let (held, cells) = fit(apple, 1, false);
    assert_eq!(cells, 1);
    assert!((apple.size.width.as_f32() * held - CELL).abs() < 1e-3);
    let (lent, cells) = fit(apple, 1, true);
    assert_eq!(cells, 2);
    assert!((apple.size.height.as_f32() * lent - EM).abs() < 1e-3);
    assert!(apple.size.width.as_f32() * lent <= 2. * CELL);
}

#[test]
fn a_narrow_icon_keeps_to_its_own_cell_even_when_the_next_is_free() {
    // The branch symbol is an em tall and 0.4em wide: one cell already
    // holds it at full height, so borrowing would only push it off
    // towards the text after it.
    let branch = ink(6.52, -4.4, 8.96, 22.);
    assert_eq!(fit(branch, 1, true), (1., 1));
}

#[test]
fn a_small_icon_from_a_narrow_face_grows_to_fill_its_cell() {
    // A face with a narrower cell than the primary inks well under it.
    let icon = ink(0.5, -1., 8.8, 9.);
    let (scale, cells) = fit(icon, 1, false);
    assert_eq!(cells, 1);
    assert!(scale > 1.);
    assert!((icon.size.width.as_f32() * scale - CELL).abs() < 1e-3);
    // The same icon two cells wide may use both.
    let (two, cells) = fit(icon, 2, false);
    assert_eq!(cells, 2);
    assert!(two > scale);
}

#[test]
fn a_fitted_icon_is_centred_in_its_room() {
    let icon = ink(0.4, -0.6, 5.3, 6.);
    let room = size(px(CELL), px(30.8));
    let (scale, _) = fit(icon, 1, false);
    let (ascent, descent) = (px(20.4 * scale), px(5.2 * scale));
    let at = icon_offset(icon, scale, room, true, ascent, descent);
    // Spilling into a borrowed cell, it stays where its face put it,
    // unless that would take its ink past either edge of the room.
    assert_eq!(
        icon_offset(icon, scale, room, false, ascent, descent).x,
        px(0.)
    );
    let lent = size(px(2. * CELL), px(30.8));
    let overhang = ink(-1., 0., 20., 20.);
    assert_eq!(
        icon_offset(overhang, 1., lent, false, ascent, descent).x,
        px(1.)
    );
    let pushed = ink(8., 0., 20., 20.);
    let x = icon_offset(pushed, 1., lent, false, ascent, descent).x;
    assert!((x.as_f32() - (2. * CELL - 28.)).abs() < 1e-4, "{x:?}");
    let left = at.x.as_f32() + icon.origin.x.as_f32() * scale;
    let right = left + icon.size.width.as_f32() * scale;
    assert!((left - (CELL - right)).abs() < 1e-3, "{left}..{right}");
    let baseline = at.y + (room.height - ascent - descent) / 2. + ascent;
    let top = (baseline - (icon.origin.y + icon.size.height) * scale).as_f32();
    let bottom = (baseline - icon.origin.y * scale).as_f32();
    assert!((top - (30.8 - bottom)).abs() < 1e-3, "{top}..{bottom}");
}

#[test]
fn icon_fit_leaves_what_is_already_the_right_size_alone() {
    // Within 2% of the cell: not worth shaping the glyph again.
    assert_eq!(fit(ink(0., 0., 13., 13.), 1, false), (1., 1));
    // Nothing measured, nothing to scale by.
    assert_eq!(fit(ink(0., 0., 0., 0.), 1, true), (1., 1));
    // A dot is small by design and grows no more than the cap.
    assert_eq!(fit(ink(1., 2., 2., 2.), 1, false).0, MAX_ICON_GROWTH);
}

fn icon_row(next: RenderCell) -> Vec<RenderCell> {
    let icon = RenderCell {
        c: '\u{f179}',
        ..RenderCell::default()
    };
    vec![icon, next]
}

#[test]
fn a_blank_in_the_same_colour_lends_an_icon_its_cell() {
    let blank = RenderCell::default();
    assert!(lends_to_icon(&icon_row(blank.clone()), 0, 1));
    // Whatever `has_room_after` lends, the end of the row included.
    assert!(lends_to_icon(&icon_row(blank.clone())[..1], 0, 1));
    // A prompt segment: icon and the space after it on one background.
    let bg = Hsla::blue();
    let mut row = icon_row(RenderCell {
        bg,
        draw_bg: true,
        ..RenderCell::default()
    });
    row[0].bg = bg;
    row[0].draw_bg = true;
    assert!(lends_to_icon(&row, 0, 1));
    // The next segment's colour, a glyph or a selection do not.
    row[1].bg = Hsla::red();
    assert!(!lends_to_icon(&row, 0, 1));
    let letter = RenderCell {
        c: 'x',
        ..RenderCell::default()
    };
    assert!(!lends_to_icon(&icon_row(letter), 0, 1));
    let selected = RenderCell {
        selected: true,
        ..RenderCell::default()
    };
    assert!(!lends_to_icon(&icon_row(selected), 0, 1));
    // Nor does a coloured blank after an icon on the default background.
    let coloured = RenderCell {
        bg,
        draw_bg: true,
        ..blank
    };
    assert!(!lends_to_icon(&icon_row(coloured), 0, 1));
}

#[test]
fn icon_borrowing_requires_matching_selection_states() {
    for draw_bg in [false, true] {
        for icon_selected in [false, true] {
            for next_selected in [false, true] {
                let mut row = icon_row(RenderCell {
                    draw_bg,
                    selected: next_selected,
                    ..RenderCell::default()
                });
                row[0].draw_bg = draw_bg;
                row[0].selected = icon_selected;
                assert_eq!(
                    lends_to_icon(&row, 0, 1),
                    icon_selected == next_selected,
                    "draw_bg={draw_bg}, selected={icon_selected}/{next_selected}"
                );
            }
        }
    }
}

#[test]
fn icon_borrowing_requires_matching_backgrounds() {
    for icon_draw_bg in [false, true] {
        for next_draw_bg in [false, true] {
            for same_colour in [false, true] {
                let mut row = icon_row(RenderCell {
                    draw_bg: next_draw_bg,
                    bg: if same_colour {
                        Hsla::blue()
                    } else {
                        Hsla::red()
                    },
                    ..RenderCell::default()
                });
                row[0].draw_bg = icon_draw_bg;
                row[0].bg = Hsla::blue();
                assert_eq!(
                    lends_to_icon(&row, 0, 1),
                    icon_draw_bg == next_draw_bg && (!icon_draw_bg || same_colour),
                    "draw_bg={icon_draw_bg}/{next_draw_bg}, same_colour={same_colour}"
                );
            }
        }
    }
}

#[test]
fn icon_borrowing_requires_matching_search_highlights() {
    let highlights = [(false, false), (true, false), (false, true)];
    for draw_bg in [false, true] {
        for (icon_hit, icon_current) in highlights {
            for (next_hit, next_current) in highlights {
                let mut row = icon_row(RenderCell {
                    draw_bg,
                    match_hit: next_hit,
                    match_current: next_current,
                    ..RenderCell::default()
                });
                row[0].draw_bg = draw_bg;
                row[0].match_hit = icon_hit;
                row[0].match_current = icon_current;
                assert_eq!(
                    lends_to_icon(&row, 0, 1),
                    (icon_hit, icon_current) == (next_hit, next_current),
                    "draw_bg={draw_bg}, highlights={icon_hit}/{icon_current}, {next_hit}/{next_current}"
                );
            }
        }
    }
}

#[test]
fn icon_borrowing_rejects_occupied_or_decorated_cells() {
    let mut neighbours = vec![
        cell('x'),
        RenderCell {
            marks: Some(vec!['\u{301}'].into_boxed_slice()),
            ..RenderCell::default()
        },
        RenderCell {
            strikeout: true,
            ..RenderCell::default()
        },
        RenderCell {
            link_hover: true,
            ..RenderCell::default()
        },
    ];
    for underline in [
        UnderlineKind::Single,
        UnderlineKind::Double,
        UnderlineKind::Curly,
        UnderlineKind::Dotted,
        UnderlineKind::Dashed,
    ] {
        neighbours.push(RenderCell {
            underline,
            ..RenderCell::default()
        });
    }
    for (i, next) in neighbours.into_iter().enumerate() {
        assert!(!lends_to_icon(&icon_row(next), 0, 1), "neighbour {i}");
    }
}

#[test]
fn icon_borrowing_reaches_the_segment_clip_budget() {
    for (draw_bg, selected, match_hit, match_current) in [
        (true, false, false, false),
        (false, true, false, false),
        (true, true, false, false),
        (false, false, true, false),
        (false, false, false, true),
    ] {
        let next = RenderCell {
            bg: Hsla::blue(),
            draw_bg,
            selected,
            match_hit,
            match_current,
            ..RenderCell::default()
        };
        let mut row = icon_row(next.clone());
        row[0] = RenderCell {
            c: '\u{f002}',
            ..next
        };
        let cell_width = px(10.);
        let budget = |row: &[RenderCell]| {
            let room = segment_has_room_after(row, 0, 1, true);
            seg_budget(true, true, 1, room, cell_width)
        };
        assert_eq!(budget(&row), cell_width * 2.);
        row[1].selected = !selected;
        assert_eq!(budget(&row), cell_width);
        row[1].selected = selected;
        if draw_bg {
            row[1].bg = Hsla::red();
            assert_eq!(budget(&row), cell_width);
            row[1].bg = Hsla::blue();
        }
        row[1].c = 'x';
        assert_eq!(budget(&row), cell_width);
    }
}

#[test]
fn non_icon_segments_keep_the_original_room_rule() {
    for next in [
        RenderCell::default(),
        RenderCell {
            draw_bg: true,
            ..RenderCell::default()
        },
        RenderCell {
            selected: true,
            ..RenderCell::default()
        },
        RenderCell {
            underline: UnderlineKind::Single,
            ..RenderCell::default()
        },
        cell('x'),
    ] {
        let row = vec![
            RenderCell {
                c: 'Ω',
                ..next.clone()
            },
            next,
        ];
        let room = segment_has_room_after(&row, 0, 1, false);
        assert_eq!(room, has_room_after(&row, 0, 1));
        assert_eq!(
            seg_budget(true, true, 1, room, px(10.)),
            if room { px(20.) } else { px(10.) }
        );
        assert!(segment_has_room_after(&row[..1], 0, 1, false));
    }
}
