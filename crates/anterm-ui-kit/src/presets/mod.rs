mod themes;
#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "test-support"))]
pub use themes::builtins;
pub use themes::{
    all, by_id, fork_to_file, load_registry, rejected, themes_dir, to_yaml, write_theme_file,
};

use std::path::PathBuf;

use alacritty_terminal::vte::ansi::Rgb;
use gpui::{App, Global, Hsla};

use crate::palette_colors::ActivePalette;

#[derive(Debug, Clone, PartialEq)]
pub enum Fill {
    Solid(u32),
    Vertical { top: u32, bottom: u32 },
    Horizontal { left: u32, right: u32 },
}

impl Fill {
    pub fn color(&self) -> u32 {
        match *self {
            Fill::Solid(c) => c,
            Fill::Vertical { top, .. } => top,
            Fill::Horizontal { left, .. } => left,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub path: PathBuf,
    pub opacity: f32,
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub dark: bool,
    pub background: Fill,
    pub foreground: u32,
    pub accent: u32,
    pub caret: Option<u32>,
    pub selection: Option<u32>,
    pub opacity: Option<f32>,
    pub blur: bool,
    pub image: Option<Image>,
    pub ansi16: [(u8, u8, u8); 16],
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Neutrals {
    pub background: u32,
    pub foreground: u32,
    pub border: u32,
    pub divider: u32,
    pub secondary: u32,
    pub muted: u32,
    pub muted_foreground: u32,
    pub popover: u32,
    pub caret: u32,
    pub selection: u32,
    pub sidebar: u32,
    pub sidebar_fg: u32,
    /// The left rail's own fill: a small step off the window toward the
    /// foreground, so the tab column reads as navigation beside the work.
    /// The right panel keeps `sidebar` (the window fill).
    pub rail: u32,
    /// The tab title ink on `rail`, floored the way `sidebar_fg` is.
    pub rail_fg: u32,
    pub accent: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct Semantic {
    pub ink: u32,
    pub fill: u32,
    pub on_fill: u32,
}

#[derive(Debug, Clone)]
pub struct Semantics {
    pub danger: Semantic,
    pub warning: Semantic,
    pub success: Semantic,
    pub info: Semantic,
    pub link: Semantic,
}

/// UI interaction roles. Terminal ANSI colours are deliberately independent.
#[derive(Debug, Clone, Copy)]
pub struct Interactions {
    pub primary: Semantic,
    pub primary_hover: u32,
    pub primary_pressed: u32,
    pub choice: u32,
    pub choice_ink: u32,
    pub input_border: u32,
}

impl Global for Interactions {}

pub mod state {
    pub const HOVER: f32 = 1.18;
    pub const SELECTED: f32 = 1.30;
    pub const PRESSED: f32 = 1.55;
    pub const CURSOR: f32 = 1.70;
    /// The side panels' own, lighter rungs (see `Theme::panel_surface`).
    /// A panel lists many rows at once, and at the window's 1.30 the current
    /// one read as a dark slab; these land on #ededec / #e8e8e7 on the
    /// default light rail (#f5f5f4).
    pub const PANEL_HOVER: f32 = 1.07;
    pub const PANEL_SELECTED: f32 = 1.12;
    pub const TEXT_RESTING: f32 = 4.6;
    pub const TEXT_STEP: f32 = 1.4;
}

#[derive(Debug, Clone, Copy)]
pub struct Surface {
    pub base: u32,
    pub hover: u32,
    pub selected: u32,
    #[cfg_attr(not(test), allow(dead_code))]
    pub pressed: u32,
    /// The top rung. No surface paints it today; the ladder tests hold the
    /// rungs below it apart against it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub cursor: u32,
    pub text_resting: u32,
    pub text_selected: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct Scrim {
    pub ink: u32,
    pub alpha: f32,
}

#[derive(Debug, Clone)]
pub struct Surfaces {
    pub window: Surface,
    pub sidebar: Surface,
    /// The left tab rail's ladder, built on `Neutrals::rail`.
    pub rail: Surface,
    pub popover: Surface,
    pub scrim: Scrim,
}

impl Global for Surfaces {}

pub fn scrim_fill(cx: &App) -> Hsla {
    let s = cx.global::<Surfaces>().scrim;
    Hsla::from(gpui::rgb(s.ink)).opacity(s.alpha)
}

pub struct ActiveAccent(pub u32);

impl Global for ActiveAccent {}

/// How many lanes of the commit graph get a colour of their own.
///
/// Six because that is how many hues of the ANSI set survive being pulled to a
/// contrast floor while staying apart from each other — and because the graph
/// caps its visible lanes at the same number, which is what guarantees no two
/// columns on screen are ever the same colour.
pub const LANE_SLOTS: usize = 6;

#[derive(Debug, Clone, Copy)]
pub struct Lanes {
    pub ink: [u32; LANE_SLOTS],
    /// Everything past the last slot shares one column, so it gets a neutral:
    /// a hue there would claim a branch identity the column does not have.
    pub overflow: u32,
}

pub struct ActiveLanes(pub Lanes);

impl Global for ActiveLanes {}

impl Theme {
    pub fn background_color(&self) -> u32 {
        self.background.color()
    }

    pub fn neutrals(&self) -> Neutrals {
        let bg = self.background_color();
        let fg = legible_foreground(bg, self.foreground);
        // One continuous surface: the rails share the window's fill and are
        // told apart from the pane area by a hairline alone. A tinted rail
        // made the workspace read as three boxes set side by side.
        let sidebar = bg;
        // The left rail is the one tinted surface: 3% toward the ink, which
        // lands on #f5f5f3 / #1e1e20 for the default pair. Everything painted
        // on it is floored against it below, the same as the other fills.
        let rail = mix(bg, fg, RAIL_TINT);
        // Light overlays sit above the grey rail; dark overlays lift toward
        // the foreground. A light menu must not be darker than its backdrop.
        let popover = if self.dark { mix(bg, fg, 0.06) } else { bg };
        // Two weights, one derivation. Which one a line gets is decided by
        // whether it is the *only* thing separating what it sits between:
        //
        // - `border` closes an outline around a surface that floats on top of
        //   other content (menu, tooltip, dialog, card) and rules a header off
        //   from the rows under it. Nothing else says where the edge is, so it
        //   has to be visible.
        // - `divider` runs between two panes that already carry their own
        //   fills — the sidebar against the terminal, the right panel against
        //   the workspace. There the line is the second signal, not the first,
        //   and painting it at full weight is what makes a workspace read as
        //   boxes bolted together rather than one surface.
        //
        // Both are floored on all three neutral fills, not only on the window:
        // the same value has to be worth something wherever it is painted.
        let hairline = |seed: f32, floor: f32| {
            [bg, sidebar, rail, popover]
                .into_iter()
                .fold(mix(bg, fg, seed), |ink, surface| {
                    at_least(ink, fg, surface, floor)
                })
        };
        let border = hairline(0.16, BORDER_FLOOR);
        let divider = hairline(0.16, DIVIDER_FLOOR);
        // This shared caption role is painted on all three opaque surfaces.
        // Calibrating only against the window loses contrast on raised fills.
        let muted_foreground = [bg, sidebar, rail, popover]
            .into_iter()
            .fold(dim(fg, bg, state::TEXT_RESTING), |ink, surface| {
                legible_ink(surface, ink, TEXT_FLOOR)
            });
        let title_floor = TITLE_FLOOR.max(contrast(muted_foreground, sidebar) * state::TEXT_STEP);
        let rail_title_floor = TITLE_FLOOR.max(contrast(muted_foreground, rail) * state::TEXT_STEP);
        // Blended, not bisected, so a palette's own softness carries into the
        // rails — but floored on the fill it is actually painted on. This is
        // the tab title, the top rung of a three-rung column (title / branch /
        // group header), and it is floored at `TITLE_FLOOR` rather than
        // `TEXT_FLOOR` because the rung under it, `muted_foreground`, already
        // sits at `TEXT_RESTING`: a title at 4.5:1 next to a caption at 4.6:1
        // is the same grey twice, and the column reads as one flat wash with
        // nothing to look at first.
        let title_ink = |surface: u32, floor: f32| {
            let title = legible_ink(surface, mix(fg, bg, 0.10), floor);
            // …and capped so the selected label keeps its `TEXT_STEP` above
            // it: on a white-on-black palette a 10% blend lands so close to
            // `fg` that there is nothing brighter left to step to. The floor
            // wins over the cap on a soft palette, where the step is taken
            // past `fg` instead (see `stepped_ink`).
            let headroom = (contrast(fg, surface) / state::TEXT_STEP).max(floor);
            match headroom > floor && contrast(title, surface) > headroom {
                true => dim(title, surface, headroom),
                false => title,
            }
        };
        Neutrals {
            background: bg,
            foreground: fg,
            border,
            divider,
            secondary: mix(bg, fg, 0.09),
            muted: mix(bg, fg, 0.06),
            muted_foreground,
            popover,
            caret: legible_ink(bg, self.caret.unwrap_or(self.accent), ACCENT_FLOOR),
            selection: self.selection.unwrap_or_else(|| mix(bg, fg, 0.20)),
            sidebar,
            // Blended, not bisected, so a palette's own softness carries into
            // the sidebar — but floored on the fill it is actually painted on
            // (`sidebar`, not `background`). This is the tab title, the top
            // rung of a three-rung column (title / branch / group header), and
            // it is floored at `TITLE_FLOOR` rather than `TEXT_FLOOR` because
            // the rung under it, `muted_foreground`, already sits at
            // `TEXT_RESTING`: a title at 4.5:1 next to a caption at 4.6:1 is
            // the same grey twice, and the column reads as one flat wash with
            // nothing to look at first.
            sidebar_fg: title_ink(sidebar, title_floor),
            rail,
            rail_fg: title_ink(rail, rail_title_floor),
            accent: legible_accent(bg, self.accent),
        }
    }

    pub fn surface(&self, base: u32) -> Surface {
        let bg = self.background_color();
        let fg = legible_foreground(bg, self.foreground);
        let selected = raise(base, fg, state::SELECTED);
        let text_resting = dim(fg, base, state::TEXT_RESTING);
        Surface {
            base,
            hover: raise(base, fg, state::HOVER),
            selected,
            pressed: raise(base, fg, state::PRESSED),
            cursor: raise(base, fg, state::CURSOR),
            text_resting,
            text_selected: stepped_ink(selected, base, fg, text_resting),
        }
    }

    /// A side panel's surface — the tab rail on the left, the docked panel on
    /// the right. Both list many rows at once, so they step on the lighter
    /// `PANEL_*` rungs, and toward the panel's own shade rather than `fg`: a
    /// cool foreground (the default light's #1c1c1e) bleaches a warm panel's
    /// tint out of a rung this faint, and the row reads as a grey patch on
    /// cream.
    fn panel_surface(&self, base: u32, text_resting: u32, fg: u32) -> Surface {
        let shade = shade_of(base, fg);
        let selected = raise(base, shade, state::PANEL_SELECTED);
        Surface {
            hover: raise(base, shade, state::PANEL_HOVER),
            selected,
            text_resting,
            text_selected: stepped_ink(selected, base, fg, text_resting),
            ..self.surface(base)
        }
    }

    pub fn interactions(&self) -> Interactions {
        let m = self.neutrals();
        // Menus use a blue wash; the switcher uses the solid companion fill.
        // Keep labels and shortcut hints readable on the shared menu surface.
        let choice = mix(m.popover, 0x2878df, if self.dark { 0.34 } else { 0.20 });
        let preferred_label = if self.dark { 0x121418 } else { 0xffffff };
        let fill = self.clear_ink(
            legible_ink(preferred_label, m.accent, TEXT_FLOOR),
            ACCENT_FLOOR,
        );
        // A filled primary action keeps the same readable label while its
        // hover/pressed background moves away from that label.
        let on_fill = ink_on(fill, preferred_label, TEXT_FLOOR);
        let away = if is_dark(on_fill) { 0xffffff } else { 0x000000 };
        Interactions {
            primary: Semantic {
                ink: self.clear_ink(m.accent, TEXT_FLOOR),
                fill,
                on_fill,
            },
            primary_hover: mix(fill, away, 0.08),
            primary_pressed: mix(fill, away, 0.16),
            choice,
            choice_ink: legible_foreground(choice, m.foreground),
            input_border: m.border,
        }
    }

    fn ansi_seed(&self, index: usize) -> u32 {
        let (r, g, b) = self.ansi16[index];
        (r as u32) << 16 | (g as u32) << 8 | b as u32
    }

    /// Clears a seed colour for use as ink on any of anterm's neutral fills.
    ///
    /// An error line lands on a popover or a sidebar row as often as on the
    /// window, and both of those fills sit a step toward the foreground. Clear
    /// the floor on every surface the ink can be painted on, not just on the
    /// darkest one.
    fn clear_ink(&self, seed: u32, floor: f32) -> u32 {
        let m = self.neutrals();
        [m.background, m.sidebar, m.rail, m.popover]
            .into_iter()
            .fold(seed, |ink, surface| legible_ink(surface, ink, floor))
    }

    /// One entry of the theme's own ANSI ramp, legible as chrome text.
    ///
    /// The ramp is authored for terminal cells, where a colour only has to
    /// stand on the background. Chrome draws the same hues on the sidebar and
    /// popover fills too, so they go through the same floor the semantic inks
    /// do.
    pub fn ansi_ink(&self, index: usize) -> u32 {
        self.clear_ink(self.ansi_seed(index), TEXT_FLOOR)
    }

    pub fn semantics(&self) -> Semantics {
        let bg = self.background_color();
        let fg = legible_foreground(bg, self.foreground);
        let build = |seed: u32| {
            let fill = self.clear_ink(seed, ACCENT_FLOOR);
            Semantic {
                ink: self.clear_ink(seed, TEXT_FLOOR),
                fill,
                on_fill: ink_on(fill, fg, TEXT_FLOOR),
            }
        };
        // Green and amber are the v4 Git inks (added / modified); the floors
        // in `build` still walk them to legibility on every surface.
        let (red, green, amber) = if self.dark {
            (0xf07878, 0x4cc27a, 0xe0a345)
        } else {
            (0xc43c43, 0x2f8a52, 0xb7791f)
        };
        Semantics {
            danger: build(red),
            success: build(green),
            warning: build(amber),
            info: build(self.accent),
            link: build(self.accent),
        }
    }

    /// Lane colours for the commit graph, derived the same way every other
    /// colour in this file is: seeded from the theme's own palette, then walked
    /// to a contrast floor on each surface it can be painted on.
    ///
    /// Not a fixed table of hexes. A hard-coded palette would be the one thing
    /// here that does not follow the theme, and — worse — the contrast tests
    /// below cannot see it, so the four light builtins would ship a graph whose
    /// lanes sit at 2:1 against their own background.
    ///
    /// The seed order is blue, yellow, magenta, green, cyan, red. Three
    /// constraints picked it: no two adjacent slots share a hue family; red and
    /// green are never neighbours, for the readers who cannot tell them apart;
    /// and red is last because a panel three or four lanes wide never reaches
    /// it, so the one colour that also means "danger" everywhere else in the UI
    /// stays out of the common case.
    pub fn lanes(&self) -> Lanes {
        const SEEDS: [usize; LANE_SLOTS] = [4, 3, 5, 2, 6, 1];
        let bg = self.background_color();
        let fg = legible_foreground(bg, self.foreground);
        // Through `clear_ink`, the same three surfaces `semantics` clears on:
        // the graph draws on the window in a floating panel, on the sidebar
        // when the panel is docked, and on a popover in the detail view.
        let mut ink = [0u32; LANE_SLOTS];
        for (slot, seed) in SEEDS.iter().enumerate() {
            ink[slot] = self.clear_ink(self.ansi_seed(*seed), ACCENT_FLOOR);
        }
        Lanes {
            ink,
            overflow: dim(fg, bg, state::TEXT_RESTING),
        }
    }

    pub fn surfaces(&self) -> Surfaces {
        let m = self.neutrals();
        let fg = legible_foreground(self.background_color(), self.foreground);
        // The panels once climbed a rung *above* the window (1.50), when a
        // regular-weight current tab was the faintest mark in its column. The
        // current title is SEMIBOLD now and carries that on its own; a heavy
        // fill on top of it read as a pressed button rather than as a place.
        let sidebar = self.panel_surface(m.sidebar, m.sidebar_fg, fg);
        let rail = self.panel_surface(m.rail, m.rail_fg, fg);
        Surfaces {
            window: self.surface(m.background),
            sidebar,
            rail,
            popover: self.surface(m.popover),
            // The v5 scrim: a dim, not a blackout. Dark lands on near-black at
            // 45%; light on a warm near-black at 14%, just enough to say the
            // window is waiting without turning the page grey.
            scrim: Scrim {
                ink: mix(m.background, 0x000000, 0.92),
                alpha: match self.dark {
                    true => 0.45,
                    false => 0.14,
                },
            },
        }
    }

    pub fn active_palette(&self, legible: bool) -> ActivePalette {
        let bg = self.background_color();
        let fg = legible_foreground(bg, self.foreground);
        let mut ansi16 = [Rgb { r: 0, g: 0, b: 0 }; 16];
        for (i, (r, g, b)) in self.ansi16.iter().enumerate() {
            let ink = (*r as u32) << 16 | (*g as u32) << 8 | *b as u32;
            // The bright half of the palette (SGR 90-97) is the shell's
            // readable-variant family: PSReadLine paints parameters, operators
            // and members with it, so a bright slot that cannot clear the text
            // floor on the theme background is walked toward the foreground
            // until it can — the same rescue `sidebar_fg` uses. Dark themes
            // tend to author it as a muted grey (invisible), light themes as a
            // pale grey (invisible the other way); both are lifted here, while
            // already-legible slots render byte-for-byte as authored. The
            // `legible` flag mirrors Settings → Appearance: off renders the
            // palette exactly as authored.
            ansi16[i] = rgb_bytes(if legible && i >= 8 {
                at_least(ink, fg, bg, TEXT_FLOOR)
            } else {
                ink
            });
        }
        ActivePalette {
            ansi16,
            sel_bg: rgb_bytes(mix(bg, fg, 0.24)),
        }
    }
}

fn bisect_contrast(from: u32, toward: u32, against: u32, target: f32) -> u32 {
    const STEPS: u32 = 12;
    let rising = contrast(toward, against) > contrast(from, against);
    if rising && contrast(toward, against) <= target {
        return toward;
    }
    if !rising && contrast(toward, against) >= target {
        return toward;
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..STEPS {
        let m = 0.5 * (lo + hi);
        let reached = if rising {
            contrast(mix(from, toward, m), against) >= target
        } else {
            contrast(mix(from, toward, m), against) <= target
        };
        if reached { hi = m } else { lo = m }
    }
    mix(from, toward, hi)
}

fn raise(base: u32, toward: u32, target: f32) -> u32 {
    bisect_contrast(base, toward, base, target)
}

/// The darkest (or, on a dark surface, lightest) colour that keeps every
/// channel of `base` the same distance apart, so blending toward it only
/// changes lightness: #f5f5f3 steps to #e8e8e6, never to #e8e8e8.
fn shade_of(base: u32, fg: u32) -> u32 {
    let ch = [base >> 16 & 0xff, base >> 8 & 0xff, base & 0xff];
    let min_ch = ch[0].min(ch[1]).min(ch[2]);
    let max_ch = ch[0].max(ch[1]).max(ch[2]);
    let shift = match relative_luminance(fg) < relative_luminance(base) {
        true => ch.map(|c| c - min_ch),
        false => ch.map(|c| c + (0xff - max_ch)),
    };
    (shift[0] << 16) | (shift[1] << 8) | shift[2]
}

fn dim(ink: u32, surface: u32, target: f32) -> u32 {
    bisect_contrast(ink, surface, surface, target)
}

fn stepped_ink(fill: u32, base: u32, fg: u32, resting: u32) -> u32 {
    let ink = ink_on(fill, fg, state::TEXT_RESTING);
    if contrast(ink, resting) >= state::TEXT_STEP {
        return ink;
    }
    let away = match relative_luminance(base) < relative_luminance(resting) {
        true => 0xffffff,
        false => 0x000000,
    };
    bisect_contrast(ink, away, resting, state::TEXT_STEP)
}

fn ink_on(fill: u32, fg: u32, target: f32) -> u32 {
    if contrast(fg, fill) >= target {
        return fg;
    }
    let near = if relative_luminance(fg) < relative_luminance(fill) {
        0x000000
    } else {
        0xffffff
    };
    let deepened = bisect_contrast(fg, near, fill, target);
    if contrast(deepened, fill) >= target {
        return deepened;
    }
    if contrast(fill, 0xffffff) >= contrast(fill, 0x000000) {
        0xffffff
    } else {
        0x000000
    }
}

/// The colour a glyph is redrawn in once an opaque block caret sits under it.
/// The terminal's own background is the conventional choice and wins in every
/// shipped theme; the foreground is the fallback for a caret close enough to
/// the background to swallow it.
pub fn caret_ink(caret: Hsla, background: Hsla, foreground: Hsla) -> Hsla {
    let pack = |c: Hsla| {
        let rgb = crate::palette_colors::hsla_to_rgb(c);
        (rgb.r as u32) << 16 | (rgb.g as u32) << 8 | rgb.b as u32
    };
    let (caret, bg, fg) = (pack(caret), pack(background), pack(foreground));
    if contrast(caret, bg) >= contrast(caret, fg) {
        background
    } else {
        foreground
    }
}

/// Whether a filled shape needs a hairline to stay a shape. A brand colour is a
/// fixed value; a theme background is not, and pure black on a dark window is
/// no shape at all.
pub fn needs_edge(fill: u32, surface: Hsla) -> bool {
    let rgb = crate::palette_colors::hsla_to_rgb(surface);
    let packed = (rgb.r as u32) << 16 | (rgb.g as u32) << 8 | rgb.b as u32;
    contrast(fill, packed) < 1.25
}

/// Whether a surface is dark enough that a halo cut in its own colour stops
/// reading as a ring and starts reading as a hole.
pub fn surface_is_dark(surface: Hsla) -> bool {
    let rgb = crate::palette_colors::hsla_to_rgb(surface);
    is_dark((rgb.r as u32) << 16 | (rgb.g as u32) << 8 | rgb.b as u32)
}

pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let (ar, ag, ab) = (a >> 16 & 0xff, a >> 8 & 0xff, a & 0xff);
    let (br, bg, bb) = (b >> 16 & 0xff, b >> 8 & 0xff, b & 0xff);
    let ch = |x: u32, y: u32| (x as f32 + (y as f32 - x as f32) * t).round() as u32;
    (ch(ar, br) << 16) | (ch(ag, bg) << 8) | ch(ab, bb)
}

fn rgb_bytes(n: u32) -> Rgb {
    Rgb {
        r: (n >> 16) as u8,
        g: (n >> 8) as u8,
        b: n as u8,
    }
}

fn relative_luminance(c: u32) -> f32 {
    fn chan(v: u32) -> f32 {
        let s = v as f32 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * chan(c >> 16 & 0xff) + 0.7152 * chan(c >> 8 & 0xff) + 0.0722 * chan(c & 0xff)
}

#[cfg(test)]
fn channel_distance(a: u32, b: u32) -> u32 {
    let d = |sh: u32| (a >> sh & 0xff).abs_diff(b >> sh & 0xff);
    d(16).max(d(8)).max(d(0))
}

pub fn contrast(a: u32, b: u32) -> f32 {
    let (l1, l2) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

/// The opaque ink SGR 2 (faint) text is painted in on a light cell, when the
/// plain fade would drop it under the text floor — `None` when the fade is
/// already legible and should stay a fade.
///
/// Faint text is `opacity` of its ink over the cell. That costs a fixed share
/// of the ink's luminance distance, and on a light background the ratio that
/// distance buys collapses fast: Catppuccin Latte's own foreground fades from
/// 7.1:1 to 3.2:1, and every bright-black the palette rescue lifted to 4.5:1
/// fades back to 2.5:1 — the "illegible secondary text" of #858. So the fade
/// is walked back toward the ink until it clears `TEXT_FLOOR` again, capped at
/// the ink's own ratio: text that was never above the floor is left as dim as
/// it would have been undimmed, not darkened past what the app asked for.
///
/// Light backgrounds only. Dark themes read the same faint text at the same
/// ratios without complaint, and keeping them byte-for-byte as they were is
/// worth more than a symmetric rule nobody asked for. The test is the cell's
/// own background, so a light cell inside a dark theme is rescued too.
pub fn legible_dim(ink: u32, bg: u32, opacity: f32) -> Option<u32> {
    if is_dark(bg) {
        return None;
    }
    let faded = mix(bg, ink, opacity);
    let floor = TEXT_FLOOR.min(contrast(ink, bg));
    if contrast(faded, bg) >= floor {
        return None;
    }
    Some(bisect_contrast(faded, ink, bg, floor))
}

pub fn is_dark(bg: u32) -> bool {
    relative_luminance(bg) < 0.5
}

pub fn is_lighter(a: u32, b: u32) -> bool {
    relative_luminance(a) > relative_luminance(b)
}

const ACCENT_FLOOR: f32 = 3.0;

/// What the glyph painted on top of a match wash keeps for itself.
///
/// WCAG's non-text ratio, not its text one. A match is a state you read
/// *through* for a moment, not a surface you set body copy on — and holding the
/// glyph at 4.5:1 is not a choice this can make anyway: three of the builtins
/// only give their own text 6.6:1, which caps the wash at 1.46:1. That is a
/// hairline's worth of shift (a hairline is 1.5:1) spread over a whole cell,
/// and it is what made a match impossible to find by scanning.
const GLYPH_FLOOR: f32 = 3.0;

/// The two ends of what a wash is allowed to be, whatever the theme.
///
/// The floor buys the weakest palettes a match you can actually see, at the
/// price of a glyph that dips under `GLYPH_FLOOR` on one. The ceiling stops the
/// strongest from painting what reads as a solid block rather than a highlight.
const WASH_FLOOR: f32 = 1.9;
const WASH_CEILING: f32 = 3.2;

/// How far a search match's wash stands off the background it sits on, and how
/// much further the one you are looking at stands off the rest. Returned as
/// `(hit, current)` contrast targets.
///
/// Derived from the theme rather than fixed, because the budget being spent is
/// the theme's: a palette with 21:1 between its text and its background can
/// afford a wash you cannot miss, and one with 6.6:1 cannot. A single constant
/// has to be safe for the second, which leaves the first with nothing.
pub fn match_wash_targets(bg: u32, fg: u32) -> (f32, f32) {
    let current = (contrast(fg, bg) / GLYPH_FLOOR).clamp(WASH_FLOOR, WASH_CEILING);
    // The plain hits are the crowd the current one has to read out of, so they
    // get a little over half the distance it travels.
    (1.0 + (current - 1.0) * 0.55, current)
}

/// The opacity at which `tint` over `surface` first reaches `target` contrast,
/// as a blended colour.
///
/// A fixed alpha does not survive a theme swap: the terminal's selection tint
/// is `mix(bg, fg, 0.24)`, so painting it at 0.32 is a 7% shift away from the
/// background in *every* theme — invisible on a white one, and barely there on
/// a dark one. Solving for the ratio instead keeps the same wash weight
/// whatever the two colours happen to be.
pub fn wash(surface: u32, tint: u32, target: f32) -> u32 {
    if contrast(mix(surface, tint, 1.0), surface) < target {
        // The tint cannot get there on its own — a theme whose selection colour
        // is nearly its background. Fall back to the ink that reads on it.
        return legible_ink(surface, tint, target);
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..16 {
        let mid = (lo + hi) / 2.0;
        if contrast(mix(surface, tint, mid), surface) >= target {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    mix(surface, tint, hi)
}

const TEXT_FLOOR: f32 = 4.5;

/// How far the left rail's fill steps off the window toward the ink.
const RAIL_TINT: f32 = 0.03;

/// The floor for the top rung of a text column whose second rung rests at
/// `TEXT_RESTING`. WCAG AAA's 7:1, which also happens to be the smallest
/// ratio that clears `TEXT_STEP` over a 4.6:1 caption with room to spare on
/// the fills a sidebar is actually painted on.
const TITLE_FLOOR: f32 = 7.0;

/// A semantic ink stepped down to sit beside body text instead of over it.
///
/// `success` and `danger` are cleared to `TEXT_FLOOR` at full chroma, which is
/// right for the one line that says a push failed and wrong for a `+94 −26`
/// repeated on every row of a list: twelve saturated numerals become the
/// loudest thing in the column while carrying the least. Blending toward the
/// caption ink they sit next to keeps the hue (green still means added) and
/// takes the shout out, then the blend is walked back toward the full ink
/// only when the surface it lands on cannot carry it at `TEXT_FLOOR`.
pub fn resting_ink(ink: Hsla, beside: Hsla, surface: Hsla) -> Hsla {
    let (ink, beside, surface) = (pack(ink), pack(beside), pack(surface));
    let blend = mix(ink, beside, 0.45);
    gpui::rgb(at_least(blend, ink, surface, TEXT_FLOOR)).into()
}

/// High-contrast hue-preserving text ink for a color-tinted pill on `surface`.
///
/// Approximates the painted pill background by blending `tint` over `surface`
/// at `fill_alpha`, then preserves `tint`'s hue while guaranteeing at least
/// WCAG AA `TEXT_FLOOR` (4.5:1) readability on both light and dark themes.
pub fn tinted_ink(tint: Hsla, surface: Hsla, fill_alpha: f32) -> Hsla {
    let (tint_rgb, surface_rgb) = (pack(tint), pack(surface));
    let pill_bg = mix(surface_rgb, tint_rgb, fill_alpha);
    gpui::rgb(legible_ink(pill_bg, tint_rgb, TEXT_FLOOR)).into()
}

/// High-contrast foreground (checkmark or glyph) drawn on top of a solid color
/// swatch `fill`.
pub fn on_swatch_ink(fill: Hsla) -> Hsla {
    let fill_rgb = pack(fill);
    let preferred = if is_dark(fill_rgb) { 0xffffff } else { 0x121418 };
    gpui::rgb(ink_on(fill_rgb, preferred, TEXT_FLOOR)).into()
}

fn pack(c: Hsla) -> u32 {
    let rgb = crate::palette_colors::hsla_to_rgb(c);
    (rgb.r as u32) << 16 | (rgb.g as u32) << 8 | rgb.b as u32
}

/// Hairlines are separators, not control outlines — the surfaces they divide
/// carry their own fills, so WCAG 1.4.11's 3:1 does not apply and painting them
/// that hard would read as a wireframe. This floor only rescues the palettes
/// where the flat blend disappears entirely, so a divider is worth the same
/// amount in every theme.
const BORDER_FLOOR: f32 = 1.5;

/// The same idea one step down, for a line that is not carrying the separation
/// on its own.
///
/// Worth stating because the seed blend cannot say it: `mix(bg, fg, 0.16)`
/// clears neither floor in any builtin theme, so both values are decided
/// entirely here. Lowering the seed changes nothing; this constant is the knob.
const DIVIDER_FLOOR: f32 = 1.2;

/// Keep an authored blend when it already clears `target` on the surface it is
/// painted on, and walk it back toward `toward` only when it does not.
fn at_least(ink: u32, toward: u32, surface: u32, target: f32) -> u32 {
    if contrast(ink, surface) >= target {
        return ink;
    }
    let lifted = bisect_contrast(ink, toward, surface, target);
    if contrast(lifted, surface) >= target {
        return lifted;
    }
    legible_ink(surface, lifted, target)
}

fn legible_ink(bg: u32, seed: u32, floor: f32) -> u32 {
    if contrast(seed, bg) >= floor {
        return seed;
    }
    let away = if contrast(0xffffff, bg) >= contrast(0x000000, bg) {
        0xffffff
    } else {
        0x000000
    };
    bisect_contrast(seed, away, bg, floor)
}

fn legible_accent(bg: u32, accent: u32) -> u32 {
    legible_ink(bg, accent, ACCENT_FLOOR)
}

fn legible_foreground(bg: u32, fg: u32) -> u32 {
    if contrast(bg, fg) >= 4.5 {
        return fg;
    }
    if contrast(bg, 0xffffff) >= contrast(bg, 0x000000) {
        0xffffff
    } else {
        0x000000
    }
}

pub struct ActiveBackground {
    pub fill: Fill,
    pub opacity: Option<f32>,
    pub image: Option<Image>,
}

impl Global for ActiveBackground {}

pub const DEFAULT_ID: &str = "light";

pub struct Themes(pub Vec<Theme>);

impl Global for Themes {}

/// Files in the themes folder that could not be read, and why. A malformed
/// theme used to log a warning and then simply not be in the list — the folder
/// is one the user opens and drops files into, so "it isn't there" needs a
/// reason attached to it.
#[derive(Default)]
pub struct RejectedThemes(pub Vec<(String, String)>);

impl Global for RejectedThemes {}
