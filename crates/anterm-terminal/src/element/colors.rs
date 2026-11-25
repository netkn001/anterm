use alacritty_terminal::index::Point as AlacPoint;
use alacritty_terminal::selection::SelectionRange;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor, Rgb};
use gpui::{Font, FontStyle, FontWeight, Hsla, Rgba};
use gpui_kit::component::ActiveTheme as _;

use super::{RenderCell, UnderlineKind, osc8_underline};
use anterm_ui_kit::config::Config;

pub(super) const DIM_OPACITY: f32 = 0.66;

fn pack_rgb(c: Rgb) -> u32 {
    (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32
}

pub(super) fn unpack_rgb(n: u32) -> Rgb {
    Rgb {
        r: (n >> 16) as u8,
        g: (n >> 8) as u8,
        b: n as u8,
    }
}

pub(super) fn to_hsla(c: Rgb) -> Hsla {
    Rgba {
        r: c.r as f32 / 255.,
        g: c.g as f32 / 255.,
        b: c.b as f32 / 255.,
        a: 1.,
    }
    .into()
}

pub(super) fn resolve(
    color: AnsiColor,
    palette: &[Rgb; 256],
    default_fg: Rgb,
    default_bg: Rgb,
) -> (Rgb, bool) {
    match color {
        AnsiColor::Spec(rgb) => (rgb, false),
        AnsiColor::Indexed(i) => (palette[i as usize], false),
        AnsiColor::Named(named) => match named {
            NamedColor::Foreground => (default_fg, true),
            NamedColor::Background => (default_bg, true),
            other => {
                let idx = other as usize;
                if idx < 256 {
                    (palette[idx], false)
                } else {
                    (default_fg, true)
                }
            }
        },
    }
}

pub(super) fn build_font(base: &Font, bold: bool, italic: bool) -> Font {
    let mut f = base.clone();
    f.weight = if bold {
        FontWeight::BOLD
    } else {
        FontWeight::NORMAL
    };
    f.style = if italic {
        FontStyle::Italic
    } else {
        FontStyle::Normal
    };
    if f.features.tag_value_list().is_empty() {
        f.features = ligatures_off();
    }
    f
}

/// The feature set that actually turns ligatures off in a terminal grid.
///
/// gpui's own `FontFeatures::disable_ligatures()` emits `calt: 0` and nothing
/// else. `calt` is only the contextual-alternate feature a programming face
/// drives `=>` and `!=` from; the `fi`/`ffl`/`ffi` ligatures live in `liga` and
/// `clig`, which that call never mentions — so they were left at the shaper's
/// default, and CoreText, DirectWrite and rustybuzz all default them on. A
/// terminal cannot have them: a ligature is one glyph where the grid budgeted a
/// cell per character, so the row drifts.
///
/// Naming all three is therefore the whole fix, and it is not platform
/// specific. Windows is the sharpest case only because gpui's DirectWrite
/// backend always attaches an `IDWriteTypography` to the run: an empty feature
/// list leaves that object empty, which suppresses DirectWrite's own defaults,
/// while any non-empty list has `liga: 1`/`clig: 1` appended to it
/// (`gpui_windows/src/direct_write.rs`, `apply_font_features`). Asking for
/// `calt: 0` there bought ligatures that asking for nothing would not have.
///
/// `rlig` is deliberately left alone: it carries the required ligatures a
/// script cannot be written without.
fn ligatures_off() -> gpui::FontFeatures {
    static OFF: std::sync::OnceLock<gpui::FontFeatures> = std::sync::OnceLock::new();
    OFF.get_or_init(|| {
        gpui::FontFeatures(std::sync::Arc::new(vec![
            ("calt".to_string(), 0),
            ("liga".to_string(), 0),
            ("clig".to_string(), 0),
        ]))
    })
    .clone()
}

pub(super) fn snapshot_cell(
    cell: &Cell,
    point: AlacPoint,
    palette: &[Rgb; 256],
    colors: &PaintColors,
    selection: Option<&SelectionRange>,
) -> RenderCell {
    let flags = cell.flags;
    if flags.contains(Flags::WIDE_CHAR_SPACER) || flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
        return RenderCell {
            spacer: true,
            ..RenderCell::default()
        };
    }

    let inverse = flags.contains(Flags::INVERSE);
    let (mut fgc, _) = resolve(cell.fg, palette, colors.fg_rgb, colors.bg_rgb);
    let (bgc, bg_default) = resolve(cell.bg, palette, colors.fg_rgb, colors.bg_rgb);
    let (fgc, bgc, draw_bg) = if inverse {
        (bgc, fgc, true)
    } else {
        if flags.contains(Flags::HIDDEN) {
            fgc = bgc;
        }
        (fgc, bgc, !bg_default)
    };

    let mut rc = RenderCell {
        c: cell.c,
        marks: cell
            .zerowidth()
            .filter(|marks| !marks.is_empty())
            .map(Box::from),
        fg: to_hsla(fgc),
        bg: to_hsla(bgc),
        draw_bg,
        bold: flags.contains(Flags::BOLD) || flags.contains(Flags::BOLD_ITALIC),
        italic: flags.contains(Flags::ITALIC) || flags.contains(Flags::BOLD_ITALIC),
        strikeout: flags.contains(Flags::STRIKEOUT),
        underline: if flags.contains(Flags::DOUBLE_UNDERLINE) {
            UnderlineKind::Double
        } else if flags.contains(Flags::UNDERCURL) {
            UnderlineKind::Curly
        } else if flags.contains(Flags::DOTTED_UNDERLINE) {
            UnderlineKind::Dotted
        } else if flags.contains(Flags::DASHED_UNDERLINE) {
            UnderlineKind::Dashed
        } else if flags.contains(Flags::UNDERLINE) {
            UnderlineKind::Single
        } else {
            UnderlineKind::None
        },
        underline_color: cell
            .underline_color()
            .map(|c| to_hsla(resolve(c, palette, colors.fg_rgb, colors.bg_rgb).0)),
        ..RenderCell::default()
    };

    if selection.is_some_and(|s| s.contains(point)) {
        rc.selected = true;
    }
    if flags.contains(Flags::DIM) {
        rc.fg = dim_fg(rc.fg, bgc, colors.legible_dim);
    }
    osc8_underline::mark(&mut rc, cell);
    rc
}

/// SGR 2's colour: the ink at `DIM_OPACITY` over its cell, or — on a light
/// cell where that fade would be illegible — the opaque ink
/// `presets::legible_dim` solved for. `legible` is Settings → Appearance's
/// palette switch; off keeps the plain fade everywhere.
fn dim_fg(fg: Hsla, under: Rgb, legible: bool) -> Hsla {
    let mut faded = fg;
    faded.a *= DIM_OPACITY;
    if !legible {
        return faded;
    }
    // A faint run is one (ink, background) pair repeated across the row, and
    // the rescue is a contrast bisection — remember the last answer so a
    // screen of dim text costs one solve per colour change, not one per cell.
    thread_local! {
        static LAST: std::cell::Cell<Option<(u32, u32, Option<u32>)>> =
            const { std::cell::Cell::new(None) };
    }
    let (ink, bg) = (pack_rgb(crate::render::palette::hsla_to_rgb(fg)), pack_rgb(under));
    let solved = LAST.with(|last| match last.get() {
        Some((i, b, s)) if i == ink && b == bg => s,
        _ => {
            let s = anterm_ui_kit::presets::legible_dim(ink, bg, DIM_OPACITY);
            last.set(Some((ink, bg, s)));
            s
        }
    });
    match solved {
        Some(c) => Hsla {
            a: fg.a,
            ..to_hsla(unpack_rgb(c))
        },
        None => faded,
    }
}

fn active_selection_bg(cx: &gpui::App) -> Rgb {
    match cx.try_global::<crate::render::palette::ActivePalette>() {
        Some(a) => a.sel_bg,
        None => Rgb {
            r: 0x4a,
            g: 0x43,
            b: 0x39,
        },
    }
}

/// Blends a finished cell's colours toward `under` for a dimmed pane. The
/// blend has to happen on the *cell*, not on the palette: truecolour cells
/// (the direct `38;2;…`/`48;2;…` SGR a prompt like starship emits for its
/// segments) carry their own `Spec` colour that a palette-level dim would
/// never see, leaving a truecolour prompt at full brightness while indexed
/// content around it dims.
pub(super) fn dim_cell(mut rc: RenderCell, dim: f32, under: Rgba) -> RenderCell {
    rc.fg = blend_toward(rc.fg, dim, under);
    rc.bg = blend_toward(rc.bg, dim, under);
    if let Some(u) = rc.underline_color.as_mut() {
        *u = blend_toward(*u, dim, under);
    }
    rc
}

/// What a search match is washed with.
///
/// The theme's accent, not the terminal palette's selection colour. A hit and a
/// selection are different things, and washing both from the same tint left the
/// only difference between them a few percent of luminance — on a grid that is
/// already grey on grey, that reads as "slightly dirty background", not as "the
/// thing you searched for". The accent is the one colour the terminal surface
/// has nothing else in, and `legible_accent` has already floored it at 3:1
/// against the background, so the wash always has somewhere to travel.
fn match_tint(cx: &gpui::App) -> u32 {
    match cx.try_global::<anterm_ui_kit::presets::ActiveAccent>() {
        Some(a) => a.0,
        None => pack_rgb(active_selection_bg(cx)),
    }
}

pub(crate) struct PaintColors {
    pub(super) default_fg: Hsla,
    pub(super) default_bg: Hsla,
    pub(super) caret: Hsla,
    pub(super) selection_bg: Hsla,
    pub(super) match_bg: Hsla,
    pub(super) current_match_bg: Hsla,
    pub(super) fg_rgb: Rgb,
    pub(super) bg_rgb: Rgb,
    /// Whether faint text on a light cell is held at the text floor (see
    /// `dim_fg`). Mirrors `theme_legible_palette`.
    pub(super) legible_dim: bool,
}

/// The under-colour a dimmed pane blends its content toward: the window
/// background as it actually sits in the frame, i.e. the active preset fill
/// premultiplied by the window's own opacity. Dimming by blending every colour
/// toward this value (instead of alpha-multiplying each primitive) keeps the
/// composite of stacked layers — a powerline separator path over its segment
/// quad, text over a tint — exactly as dimmed as any single layer, so the
/// decorations of a prompt stay continuous when the pane goes inactive.
///
/// The fill comes from the active preset rather than the flat
/// `theme().background` token because the workspace root paints the preset
/// (see `theme::workspace_background`): with a gradient or wallpaper preset
/// the actual backdrop is not the theme token, and blending toward the wrong
/// colour would leave a tinted slab inside the dimmed pane. Solid fills match
/// exactly; gradients are approximated by their midpoint stop; a wallpaper
/// image rides over the fill at low opacity, so the fill stays the best
/// available target. Cells whose background is not painted (the default
/// terminal background) intentionally keep showing the un-dimmed window
/// background through them, so the backdrop material stays visible.
///
/// Two trade-offs follow from painting opaque, pre-blended colours instead of
/// the old element-opacity style. On a translucent window the desktop no
/// longer shows through a dimmed pane's painted cells — the old style left
/// them at alpha `dim` and let the backdrop contribute; the new style paints
/// them opaque, blended toward `fill × window_opacity`, which ignores the
/// backdrop's own contribution. And only the terminal element's cells are
/// dimmed: the search bar, completion menu and integration notice render
/// outside the grid and stay at full brightness, where the old style faded
/// the whole `TerminalView` — the same "the fading is worn by what it holds"
/// rule the drag grip follows.
pub(crate) fn dim_under(cx: &gpui::App) -> Rgba {
    // Mirror `workspace_background`: the active preset fill when a preset is
    // installed, the theme token otherwise. The alpha is the window opacity
    // in both arms, so the premultiply is uniform.
    match cx.try_global::<anterm_ui_kit::presets::ActiveBackground>() {
        Some(bg) => fill_under(&bg.fill, bg.opacity),
        None => theme_under(cx.theme().background),
    }
}

/// The premultiplied under-colour for an active preset's fill at the window's
/// own opacity. Solid fills are exact; gradients are approximated by their
/// midpoint stop (a wallpaper image rides over the fill at low opacity, so
/// the fill stays the best available target).
pub(super) fn fill_under(fill: &anterm_ui_kit::presets::Fill, opacity: Option<f32>) -> Rgba {
    let packed = match fill {
        anterm_ui_kit::presets::Fill::Solid(c) => *c,
        anterm_ui_kit::presets::Fill::Vertical { top, bottom } => {
            anterm_ui_kit::presets::mix(*top, *bottom, 0.5)
        }
        anterm_ui_kit::presets::Fill::Horizontal { left, right } => {
            anterm_ui_kit::presets::mix(*left, *right, 0.5)
        }
    };
    premultiplied(packed, opacity.unwrap_or(1.))
}

/// The premultiplied under-colour for the theme background token, used when
/// no preset is installed — the `None` arm of `workspace_background`.
pub(super) fn theme_under(bg: Hsla) -> Rgba {
    let bg = Rgba::from(bg);
    Rgba {
        r: bg.r * bg.a,
        g: bg.g * bg.a,
        b: bg.b * bg.a,
        a: 1.,
    }
}

/// The premultiplied under-colour for a packed fill colour at the window's
/// own opacity — exactly the colour a `Solid` workspace fill paints over the
/// OS backdrop, ignoring the backdrop's own contribution.
pub(super) fn premultiplied(packed: u32, opacity: f32) -> Rgba {
    Rgba {
        r: ((packed >> 16) & 0xff) as f32 / 255. * opacity,
        g: ((packed >> 8) & 0xff) as f32 / 255. * opacity,
        b: (packed & 0xff) as f32 / 255. * opacity,
        a: 1.,
    }
}

/// Blends `c` toward `under` in RGB space, keeping `c`'s own alpha. `under`
/// is passed pre-converted to `Rgba` because it is a frame constant: every
/// cell blends fg, bg and an optional underline colour against it, and the
/// HSL↔RGB round trip would otherwise run once per colour per cell. Linear
/// in the composited result: painting `blend_toward(a)` over `blend_toward(b)`
/// equals `blend_toward(a over b)`, which is exactly what a uniform opacity
/// of `dim` over the window background would produce.
pub(crate) fn blend_toward(c: Hsla, dim: f32, under: Rgba) -> Hsla {
    let c = Rgba::from(c);
    Rgba {
        r: dim * c.r + (1. - dim) * under.r,
        g: dim * c.g + (1. - dim) * under.g,
        b: dim * c.b + (1. - dim) * under.b,
        a: c.a,
    }
    .into()
}

/// Opaque background colour matching what this pane sits on: `theme_bg`
/// composited over the active backdrop under-colour and dimmed by `dim`. Used
/// by DOM overlays (such as the input bar) that must cover existing grid
/// glyphs underneath without letting them bleed through.
pub(crate) fn opaque_cell_bg(theme_bg: Hsla, dim: f32, cx: &gpui::App) -> Hsla {
    let bg = Rgba::from(theme_bg);
    let under = dim_under(cx);
    let opaque: Hsla = if bg.a >= 1. {
        theme_bg
    } else {
        let a = bg.a.clamp(0., 1.);
        Rgba {
            r: bg.r * a + under.r * (1. - a),
            g: bg.g * a + under.g * (1. - a),
            b: bg.b * a + under.b * (1. - a),
            a: 1.,
        }
        .into()
    };
    let dim = dim.clamp(0., 1.);
    if dim < 1. {
        blend_toward(opaque, dim, under)
    } else {
        opaque
    }
}

impl PaintColors {
    pub(crate) fn resolve(theme: &gpui_kit::component::Theme, cx: &gpui::App) -> Self {
        let default_fg = theme.foreground;
        let default_bg = theme.background;
        let caret = theme.caret;
        let selection_bg = {
            let mut c = default_fg;
            c.a = 0.24;
            c
        };
        // Opaque, and solved for a contrast against the background rather than
        // a fixed alpha: the selection tint is only 24% away from the
        // background to begin with, so a 0.32 alpha landed 7% off it in every
        // theme — on a white one the matches were all but invisible. How far
        // the wash may travel is the theme's to say (`match_wash_targets`),
        // since the glyph on top of it pays for every step.
        let fg_rgb = crate::render::palette::hsla_to_rgb(default_fg);
        let bg_rgb = crate::render::palette::hsla_to_rgb(default_bg);
        let bg_packed = pack_rgb(bg_rgb);
        let tint = match_tint(cx);
        let (hit_target, current_target) =
            anterm_ui_kit::presets::match_wash_targets(bg_packed, pack_rgb(fg_rgb));
        let match_bg = to_hsla(unpack_rgb(anterm_ui_kit::presets::wash(
            bg_packed, tint, hit_target,
        )));
        let current_match_bg = to_hsla(unpack_rgb(anterm_ui_kit::presets::wash(
            bg_packed,
            tint,
            current_target,
        )));
        Self {
            default_fg,
            default_bg,
            caret,
            selection_bg,
            match_bg,
            current_match_bg,
            fg_rgb,
            bg_rgb,
            legible_dim: cx
                .try_global::<Config>()
                .is_none_or(|c| c.theme_legible_palette),
        }
    }

    /// Blends every colour toward `under` for a dimmed pane. The alpha of each
    /// colour is left alone — the blend happens in RGB, so translucent tints
    /// (selection, matches) composite over the already-blended cell colours
    /// exactly as dimmed as opaque content does.
    ///
    /// `fg_rgb`/`bg_rgb` are deliberately left raw: they only feed `resolve`
    /// for the named foreground/background cells, and those cells get blended
    /// by `dim_cell` like every other cell — blending them here as well would
    /// dim the named colours twice.
    pub(super) fn dimmed(&self, dim: f32, under: Rgba) -> Self {
        Self {
            default_fg: blend_toward(self.default_fg, dim, under),
            default_bg: blend_toward(self.default_bg, dim, under),
            caret: blend_toward(self.caret, dim, under),
            selection_bg: blend_toward(self.selection_bg, dim, under),
            match_bg: blend_toward(self.match_bg, dim, under),
            current_match_bg: blend_toward(self.current_match_bg, dim, under),
            fg_rgb: self.fg_rgb,
            bg_rgb: self.bg_rgb,
            legible_dim: self.legible_dim,
        }
    }
}
