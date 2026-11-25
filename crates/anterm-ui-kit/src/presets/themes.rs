use std::path::PathBuf;

use gpui::App;
use serde::Deserialize;

use super::{DEFAULT_ID, Fill, Image, RejectedThemes, Theme, Themes, contrast, is_dark};

impl Theme {
    fn from_builtin(b: &BuiltinSpec) -> Theme {
        let bg = b.background;
        Theme {
            id: b.id.to_string(),
            name: b.name.to_string(),
            dark: is_dark(bg),
            background: Fill::Solid(bg),
            foreground: b.foreground,
            accent: b.accent,
            caret: b.caret,
            selection: None,
            opacity: None,
            blur: false,
            image: None,
            ansi16: b.ansi16,
            path: None,
        }
    }
}

pub fn load_registry(cx: &mut App) {
    let (themes, rejected) = load_all();
    cx.set_global(Themes(themes));
    cx.set_global(RejectedThemes(rejected));
}

pub fn all(cx: &App) -> Vec<Theme> {
    cx.try_global::<Themes>()
        .map(|t| t.0.clone())
        .unwrap_or_else(builtins)
}

pub fn rejected(cx: &App) -> Vec<(String, String)> {
    cx.try_global::<RejectedThemes>()
        .map(|r| r.0.clone())
        .unwrap_or_default()
}

pub fn by_id(cx: &App, id: &str) -> Theme {
    let themes = all(cx);
    themes
        .iter()
        .find(|t| t.id == id)
        .or_else(|| themes.iter().find(|t| t.id == DEFAULT_ID))
        .cloned()
        .unwrap_or_else(|| themes.into_iter().next().expect("at least the built-ins"))
}

impl Theme {
    pub fn editable(&self) -> bool {
        self.path
            .as_ref()
            .and_then(|p| p.extension())
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml"))
    }
}

pub fn to_yaml(t: &Theme) -> String {
    fn hex(c: u32) -> String {
        format!("\"#{:06x}\"", c & 0xff_ffff)
    }
    fn rgb_hex((r, g, b): (u8, u8, u8)) -> String {
        format!("\"#{r:02x}{g:02x}{b:02x}\"")
    }
    let mut s = String::new();
    s.push_str(&format!("name: {:?}\n", t.name));
    match &t.background {
        Fill::Solid(c) => s.push_str(&format!("background: {}\n", hex(*c))),
        Fill::Vertical { top, bottom } => s.push_str(&format!(
            "background: {{ top: {}, bottom: {} }}\n",
            hex(*top),
            hex(*bottom)
        )),
        Fill::Horizontal { left, right } => s.push_str(&format!(
            "background: {{ left: {}, right: {} }}\n",
            hex(*left),
            hex(*right)
        )),
    }
    s.push_str(&format!("foreground: {}\n", hex(t.foreground)));
    s.push_str(&format!("accent: {}\n", hex(t.accent)));
    if let Some(c) = t.caret {
        s.push_str(&format!("cursor: {}\n", hex(c)));
    }
    if let Some(c) = t.selection {
        s.push_str(&format!("selection: {}\n", hex(c)));
    }
    if let Some(o) = t.opacity {
        s.push_str(&format!("opacity: {o}\n"));
    }
    if t.blur {
        s.push_str("blur: true\n");
    }
    if let Some(img) = &t.image {
        s.push_str(&format!(
            "background_image:\n  path: {:?}\n  opacity: {}\n",
            img.path.display().to_string(),
            img.opacity
        ));
    }
    let row = |range: std::ops::Range<usize>| {
        range
            .map(|i| rgb_hex(t.ansi16[i]))
            .collect::<Vec<_>>()
            .join(", ")
    };
    s.push_str("ansi:\n");
    s.push_str(&format!("  normal: [{}]\n", row(0..8)));
    s.push_str(&format!("  bright: [{}]\n", row(8..16)));
    s
}

pub fn fork_to_file(t: &Theme) -> std::io::Result<String> {
    let dir = themes_dir()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no themes directory"))?;
    std::fs::create_dir_all(&dir)?;
    let base = format!("{}-custom", t.id.trim_end_matches("-custom"));
    let mut stem = base.clone();
    let mut n = 2;
    while dir.join(format!("{stem}.yaml")).exists() {
        stem = format!("{base}-{n}");
        n += 1;
    }
    let mut copy = t.clone();
    // The name lands in the YAML on disk and is matched back with
    // `trim_end_matches(" (custom)")`, so it stays English in every locale —
    // a translated suffix would survive a language switch and stack up.
    copy.name = format!("{} (custom)", t.name.trim_end_matches(" (custom)"));
    crate::config::write_atomic(
        &dir.join(format!("{stem}.yaml")),
        to_yaml(&copy).as_bytes(),
    )?;
    Ok(stem)
}

pub fn write_theme_file(t: &Theme) -> std::io::Result<()> {
    let path = t.path.clone().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "theme is not file-backed")
    })?;
    crate::config::write_atomic(&path, to_yaml(t).as_bytes())
}

fn load_all() -> (Vec<Theme>, Vec<(String, String)>) {
    let mut themes = builtins();
    let (user, rejected) = load_user_themes();
    themes.extend(user);
    dedupe_ids(&mut themes);
    (themes, rejected)
}

fn dedupe_ids(themes: &mut [Theme]) {
    let mut seen = std::collections::HashSet::new();
    for t in themes.iter_mut() {
        if seen.insert(t.id.clone()) {
            continue;
        }
        let base = t.id.clone();
        let mut n = 2;
        let mut candidate = format!("{base}-{n}");
        while !seen.insert(candidate.clone()) {
            n += 1;
            candidate = format!("{base}-{n}");
        }
        t.name = format!("{} ({n})", t.name);
        t.id = candidate;
    }
}

pub fn themes_dir() -> Option<PathBuf> {
    crate::config::config_path("themes")
}

fn load_user_themes() -> (Vec<Theme>, Vec<(String, String)>) {
    let Some(dir) = themes_dir() else {
        return (Vec::new(), Vec::new());
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return (Vec::new(), Vec::new());
    };
    let mut out = Vec::new();
    let mut rejected = Vec::new();
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort_by_key(|p| p.to_string_lossy().to_lowercase());
    for path in paths {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());
        let parsed = match ext.as_deref() {
            Some("yaml") | Some("yml") => load_yaml_theme(&path),
            Some("itermcolors") => load_iterm_theme(&path),
            _ => continue,
        };
        match parsed {
            Ok(theme) => out.push(theme),
            Err(e) => {
                log::warn!("skipping theme {}: {e}", path.display());
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string());
                rejected.push((name, e));
            }
        }
    }
    (out, rejected)
}

pub(super) fn id_and_name(path: &std::path::Path) -> (String, String) {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("theme")
        .to_string();
    let name = stem
        .split(|c| c == '_' || c == '-' || c == ' ')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut cs = w.chars();
            match cs.next() {
                Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    (
        stem,
        if name.is_empty() {
            // Theme names are data — they get written back to the YAML file,
            // so the fallback stays English rather than following the GUI.
            "Theme".into()
        } else {
            name
        },
    )
}

#[derive(Deserialize)]
pub(super) struct ThemeFile {
    name: Option<String>,
    pub(super) background: FillFile,
    foreground: String,
    accent: String,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    selection: Option<String>,
    #[serde(default)]
    pub(super) opacity: Option<f32>,
    #[serde(default)]
    pub(super) blur: bool,
    #[serde(default)]
    pub(super) background_image: Option<ImageFile>,
    pub(super) ansi: AnsiFile,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum FillFile {
    Solid(String),
    Vertical { top: String, bottom: String },
    Horizontal { left: String, right: String },
}

#[derive(Deserialize)]
pub(super) struct AnsiFile {
    pub(super) normal: [String; 8],
    pub(super) bright: [String; 8],
}

#[derive(Deserialize)]
pub(super) struct ImageFile {
    pub(super) path: String,
    #[serde(default = "default_image_opacity")]
    pub(super) opacity: f32,
}

fn default_image_opacity() -> f32 {
    0.3
}

fn load_yaml_theme(path: &std::path::Path) -> Result<Theme, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let file: ThemeFile =
        serde_yaml::from_str(crate::config::strip_bom(&text)).map_err(|e| e.to_string())?;
    let (id, derived_name) = id_and_name(path);

    let background = file.background.into_fill()?;
    let bg = background.color();
    let mut ansi16 = [(0u8, 0u8, 0u8); 16];
    for i in 0..8 {
        ansi16[i] = parse_rgb(&file.ansi.normal[i])?;
        ansi16[i + 8] = parse_rgb(&file.ansi.bright[i])?;
    }

    Ok(Theme {
        id,
        name: file.name.unwrap_or(derived_name),
        dark: is_dark(bg),
        background,
        foreground: parse_hex(&file.foreground)?,
        accent: parse_hex(&file.accent)?,
        caret: file.cursor.as_deref().map(parse_hex).transpose()?,
        selection: file.selection.as_deref().map(parse_hex).transpose()?,
        opacity: file.opacity.map(|o| o.clamp(0.0, 1.0)),
        blur: file.blur,
        image: file.background_image.map(|i| Image {
            path: expand_path(&i.path),
            opacity: i.opacity.clamp(0.0, 1.0),
        }),
        ansi16,
        path: Some(path.to_path_buf()),
    })
}

impl FillFile {
    pub(super) fn into_fill(self) -> Result<Fill, String> {
        Ok(match self {
            FillFile::Solid(s) => Fill::Solid(parse_hex(&s)?),
            FillFile::Vertical { top, bottom } => Fill::Vertical {
                top: parse_hex(&top)?,
                bottom: parse_hex(&bottom)?,
            },
            FillFile::Horizontal { left, right } => Fill::Horizontal {
                left: parse_hex(&left)?,
                right: parse_hex(&right)?,
            },
        })
    }
}

fn expand_path(p: &str) -> PathBuf {
    let p = p.trim();
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    let path = PathBuf::from(p);
    if path.is_absolute() {
        return path;
    }
    themes_dir().map(|d| d.join(&path)).unwrap_or(path)
}

pub(super) fn load_iterm_theme(path: &std::path::Path) -> Result<Theme, String> {
    let value = plist::Value::from_file(path).map_err(|e| e.to_string())?;
    let dict = value
        .as_dictionary()
        .ok_or("not an iTerm color plist (expected a dictionary)")?;

    let color = |key: &str| -> Option<u32> {
        let c = dict.get(key)?.as_dictionary()?;
        let comp = |k: &str| -> Option<u32> {
            // iTerm2 and plistlib write exact 0 and 1 as `<integer>`.
            let v = c.get(k)?;
            let f = v
                .as_real()
                .or_else(|| v.as_signed_integer().map(|i| i as f64))?;
            Some((f.clamp(0.0, 1.0) * 255.0).round() as u32)
        };
        Some(
            (comp("Red Component")? << 16)
                | (comp("Green Component")? << 8)
                | comp("Blue Component")?,
        )
    };

    let mut ansi16 = [(0u8, 0u8, 0u8); 16];
    for i in 0..16 {
        let c = color(&format!("Ansi {i} Color"))
            .ok_or_else(|| format!("missing or malformed 'Ansi {i} Color'"))?;
        ansi16[i] = ((c >> 16) as u8, (c >> 8) as u8, c as u8);
    }

    let background = color("Background Color").ok_or("missing 'Background Color'")?;
    let foreground = color("Foreground Color").ok_or("missing 'Foreground Color'")?;
    let cursor = color("Cursor Color");
    let bright_blue = {
        let (r, g, b) = ansi16[12];
        (r as u32) << 16 | (g as u32) << 8 | b as u32
    };
    let accent = match cursor {
        Some(c) if contrast(background, c) >= 1.5 => c,
        _ => bright_blue,
    };

    let (id, name) = id_and_name(path);
    Ok(Theme {
        id,
        name,
        dark: is_dark(background),
        background: Fill::Solid(background),
        foreground,
        accent,
        caret: cursor,
        selection: None,
        opacity: None,
        blur: false,
        image: None,
        ansi16,
        path: Some(path.to_path_buf()),
    })
}

pub(super) fn parse_hex(s: &str) -> Result<u32, String> {
    let hex = s.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return Err(format!("'{s}' is not a 6-digit hex color"));
    }
    u32::from_str_radix(hex, 16).map_err(|_| format!("'{s}' is not a hex color"))
}

pub(super) fn parse_rgb(s: &str) -> Result<(u8, u8, u8), String> {
    let n = parse_hex(s)?;
    Ok(((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

pub fn builtins() -> Vec<Theme> {
    BUILTINS.iter().map(Theme::from_builtin).collect()
}

struct BuiltinSpec {
    id: &'static str,
    name: &'static str,
    background: u32,
    foreground: u32,
    accent: u32,
    caret: Option<u32>,
    ansi16: [(u8, u8, u8); 16],
}

static BUILTINS: [BuiltinSpec; 13] = [
    BuiltinSpec {
        id: "light",
        name: "Light",
        background: 0xfcfcfb,
        foreground: 0x1c1c1e,
        accent: 0x1f6bf0,
        caret: None,
        ansi16: [
            (0x24, 0x29, 0x2e),
            (0xd1, 0x24, 0x2f),
            (0x1a, 0x7f, 0x37),
            (0x9a, 0x67, 0x00),
            (0x09, 0x69, 0xda),
            (0x82, 0x50, 0xdf),
            (0x1b, 0x7c, 0x83),
            (0x6e, 0x77, 0x81),
            (0x57, 0x60, 0x6a),
            (0xcf, 0x22, 0x2e),
            (0x1f, 0x88, 0x3d),
            (0xbf, 0x87, 0x00),
            (0x21, 0x8b, 0xff),
            (0xa4, 0x75, 0xf9),
            (0x31, 0x92, 0xaa),
            (0x8c, 0x95, 0x9f),
        ],
    },
    BuiltinSpec {
        id: "one_light",
        name: "One Light",
        background: 0xfafafa,
        foreground: 0x383a42,
        accent: 0x4078f2,
        caret: None,
        ansi16: [
            (0x38, 0x3a, 0x42),
            (0xe4, 0x56, 0x49),
            (0x50, 0xa1, 0x4f),
            (0xc1, 0x84, 0x01),
            (0x40, 0x78, 0xf2),
            (0xa6, 0x26, 0xa4),
            (0x01, 0x84, 0xbc),
            (0xa0, 0xa1, 0xa7),
            (0x69, 0x6c, 0x77),
            (0xe4, 0x56, 0x49),
            (0x50, 0xa1, 0x4f),
            (0xc1, 0x84, 0x01),
            (0x40, 0x78, 0xf2),
            (0xa6, 0x26, 0xa4),
            (0x01, 0x84, 0xbc),
            (0xfa, 0xfa, 0xfa),
        ],
    },
    BuiltinSpec {
        id: "catppuccin_latte",
        name: "Catppuccin Latte",
        background: 0xeff1f5,
        foreground: 0x4c4f69,
        accent: 0x1e66f5,
        caret: None,
        ansi16: [
            (0xbc, 0xc0, 0xcc),
            (0xd2, 0x0f, 0x39),
            (0x40, 0xa0, 0x2b),
            (0xdf, 0x8e, 0x1d),
            (0x1e, 0x66, 0xf5),
            (0xea, 0x76, 0xcb),
            (0x17, 0x92, 0x99),
            (0x5c, 0x5f, 0x77),
            (0xac, 0xb0, 0xbe),
            (0xd2, 0x0f, 0x39),
            (0x40, 0xa0, 0x2b),
            (0xdf, 0x8e, 0x1d),
            (0x1e, 0x66, 0xf5),
            (0xea, 0x76, 0xcb),
            (0x17, 0x92, 0x99),
            (0x6c, 0x6f, 0x85),
        ],
    },
    BuiltinSpec {
        id: "rose_pine_dawn",
        name: "Rosé Pine Dawn",
        background: 0xfaf4ed,
        foreground: 0x575279,
        accent: 0x907aa9,
        caret: None,
        ansi16: [
            (0xf2, 0xe9, 0xe1),
            (0xb4, 0x63, 0x7a),
            (0x28, 0x69, 0x83),
            (0xea, 0x9d, 0x34),
            (0x56, 0x94, 0x9f),
            (0x90, 0x7a, 0xa9),
            (0xd7, 0x82, 0x7e),
            (0x57, 0x52, 0x79),
            (0x98, 0x93, 0xa5),
            (0xb4, 0x63, 0x7a),
            (0x28, 0x69, 0x83),
            (0xea, 0x9d, 0x34),
            (0x56, 0x94, 0x9f),
            (0x90, 0x7a, 0xa9),
            (0xd7, 0x82, 0x7e),
            (0x57, 0x52, 0x79),
        ],
    },
    BuiltinSpec {
        id: "dark",
        name: "Dark",
        background: 0x18181a,
        foreground: 0xececed,
        accent: 0x78a8f5,
        caret: None,
        ansi16: [
            (0x61, 0x61, 0x61),
            (0xff, 0x82, 0x72),
            (0xb4, 0xfa, 0x72),
            (0xfe, 0xfd, 0xc2),
            (0xa5, 0xd5, 0xfe),
            (0xff, 0x8f, 0xfd),
            (0xd0, 0xd1, 0xfe),
            (0xf1, 0xf1, 0xf1),
            (0x8e, 0x8e, 0x8e),
            (0xff, 0xc4, 0xbd),
            (0xd6, 0xfc, 0xb9),
            (0xfe, 0xfd, 0xd5),
            (0xc1, 0xe3, 0xfe),
            (0xff, 0xb1, 0xfe),
            (0xe5, 0xe6, 0xfe),
            (0xfe, 0xff, 0xff),
        ],
    },
    BuiltinSpec {
        id: "dracula",
        name: "Dracula",
        background: 0x282a36,
        foreground: 0xf8f8f2,
        accent: 0xff79c6,
        caret: None,
        ansi16: [
            (0x00, 0x00, 0x00),
            (0xff, 0x55, 0x55),
            (0x50, 0xfa, 0x7b),
            (0xf1, 0xfa, 0x8c),
            (0xbd, 0x93, 0xf9),
            (0xff, 0x79, 0xc6),
            (0x8b, 0xe9, 0xfd),
            (0xbb, 0xbb, 0xbb),
            (0x55, 0x55, 0x55),
            (0xff, 0x55, 0x55),
            (0x50, 0xfa, 0x7b),
            (0xf1, 0xfa, 0x8c),
            (0xca, 0xa9, 0xfa),
            (0xff, 0x79, 0xc6),
            (0x8b, 0xe9, 0xfd),
            (0xff, 0xff, 0xff),
        ],
    },
    BuiltinSpec {
        id: "harbor",
        name: "Harbor",
        background: 0x1d2022,
        foreground: 0xe4eef5,
        accent: 0x6c96b4,
        caret: None,
        ansi16: [
            (0x12, 0x12, 0x12),
            (0xc7, 0x61, 0x56),
            (0x57, 0xc7, 0x8a),
            (0xc8, 0xa3, 0x5a),
            (0x57, 0x85, 0xc7),
            (0xc7, 0x56, 0xa9),
            (0x57, 0xc7, 0xc3),
            (0xee, 0xed, 0xeb),
            (0x29, 0x29, 0x29),
            (0xd2, 0x2d, 0x1e),
            (0x1c, 0xa0, 0x5a),
            (0xe5, 0xa0, 0x1a),
            (0x14, 0x58, 0xb8),
            (0xa4, 0x37, 0x87),
            (0x4d, 0x99, 0x89),
            (0xff, 0xff, 0xff),
        ],
    },
    BuiltinSpec {
        id: "one_dark_pro",
        name: "One Dark Pro",
        background: 0x282c34,
        foreground: 0xabb2bf,
        accent: 0x528bff,
        caret: None,
        ansi16: [
            (0x3f, 0x44, 0x51),
            (0xe0, 0x6c, 0x75),
            (0x98, 0xc3, 0x79),
            (0xe5, 0xc0, 0x7b),
            (0x61, 0xaf, 0xef),
            (0xc6, 0x78, 0xdd),
            (0x56, 0xb6, 0xc2),
            (0xab, 0xb2, 0xbf),
            (0x5c, 0x63, 0x70),
            (0xff, 0x61, 0x6e),
            (0xa5, 0xe0, 0x75),
            (0xf0, 0xa4, 0x5d),
            (0x4d, 0xc4, 0xff),
            (0xde, 0x73, 0xff),
            (0x4c, 0xd1, 0xe0),
            (0xe6, 0xe6, 0xe6),
        ],
    },
    BuiltinSpec {
        id: "rose_pine",
        name: "Rosé Pine",
        background: 0x191724,
        foreground: 0xe0def4,
        accent: 0xc4a7e7,
        caret: None,
        ansi16: [
            (0x26, 0x23, 0x3a),
            (0xeb, 0x6f, 0x92),
            (0x31, 0x74, 0x8f),
            (0xf6, 0xc1, 0x77),
            (0x9c, 0xcf, 0xd8),
            (0xc4, 0xa7, 0xe7),
            (0xeb, 0xbc, 0xba),
            (0xe0, 0xde, 0xf4),
            (0x6e, 0x6a, 0x86),
            (0xeb, 0x6f, 0x92),
            (0x31, 0x74, 0x8f),
            (0xf6, 0xc1, 0x77),
            (0x9c, 0xcf, 0xd8),
            (0xc4, 0xa7, 0xe7),
            (0xeb, 0xbc, 0xba),
            (0xe0, 0xde, 0xf4),
        ],
    },
    BuiltinSpec {
        id: "catppuccin_mocha",
        name: "Catppuccin Mocha",
        background: 0x1e1e2e,
        foreground: 0xcdd6f4,
        accent: 0x89b4fa,
        // Rosewater, the cursor colour Catppuccin's own terminal spec names —
        // the accent fallback would paint it blue.
        caret: Some(0xf5e0dc),
        ansi16: [
            (0x45, 0x47, 0x5a),
            (0xf3, 0x8b, 0xa8),
            (0xa6, 0xe3, 0xa1),
            (0xf9, 0xe2, 0xaf),
            (0x89, 0xb4, 0xfa),
            (0xf5, 0xc2, 0xe7),
            (0x94, 0xe2, 0xd5),
            (0xba, 0xc2, 0xde),
            (0x58, 0x5b, 0x70),
            (0xf3, 0x8b, 0xa8),
            (0xa6, 0xe3, 0xa1),
            (0xf9, 0xe2, 0xaf),
            (0x89, 0xb4, 0xfa),
            (0xf5, 0xc2, 0xe7),
            (0x94, 0xe2, 0xd5),
            (0xa6, 0xad, 0xc8),
        ],
    },
    BuiltinSpec {
        id: "gruvbox_dark",
        name: "Gruvbox Dark",
        background: 0x282828,
        foreground: 0xebdbb2,
        accent: 0xfe8019,
        caret: Some(0xebdbb2),
        ansi16: [
            (0x28, 0x28, 0x28),
            (0xcc, 0x24, 0x1d),
            (0x98, 0x97, 0x1a),
            (0xd7, 0x99, 0x21),
            (0x45, 0x85, 0x88),
            (0xb1, 0x62, 0x86),
            (0x68, 0x9d, 0x6a),
            (0xa8, 0x99, 0x84),
            (0x92, 0x83, 0x74),
            (0xfb, 0x49, 0x34),
            (0xb8, 0xbb, 0x26),
            (0xfa, 0xbd, 0x2f),
            (0x83, 0xa5, 0x98),
            (0xd3, 0x86, 0x9b),
            (0x8e, 0xc0, 0x7c),
            (0xeb, 0xdb, 0xb2),
        ],
    },
    BuiltinSpec {
        id: "nord",
        name: "Nord",
        background: 0x2e3440,
        foreground: 0xd8dee9,
        accent: 0x88c0d0,
        caret: Some(0xd8dee9),
        ansi16: [
            (0x3b, 0x42, 0x52),
            (0xbf, 0x61, 0x6a),
            (0xa3, 0xbe, 0x8c),
            (0xeb, 0xcb, 0x8b),
            (0x81, 0xa1, 0xc1),
            (0xb4, 0x8e, 0xad),
            (0x88, 0xc0, 0xd0),
            (0xe5, 0xe9, 0xf0),
            (0x4c, 0x56, 0x6a),
            (0xbf, 0x61, 0x6a),
            (0xa3, 0xbe, 0x8c),
            (0xeb, 0xcb, 0x8b),
            (0x81, 0xa1, 0xc1),
            (0xb4, 0x8e, 0xad),
            (0x8f, 0xbc, 0xbb),
            (0xec, 0xef, 0xf4),
        ],
    },
    BuiltinSpec {
        id: "tokyo_night",
        name: "Tokyo Night",
        background: 0x1a1b26,
        foreground: 0xc0caf5,
        accent: 0x7aa2f7,
        caret: Some(0xc0caf5),
        ansi16: [
            (0x15, 0x16, 0x1e),
            (0xf7, 0x76, 0x8e),
            (0x9e, 0xce, 0x6a),
            (0xe0, 0xaf, 0x68),
            (0x7a, 0xa2, 0xf7),
            (0xbb, 0x9a, 0xf7),
            (0x7d, 0xcf, 0xff),
            (0xa9, 0xb1, 0xd6),
            (0x41, 0x48, 0x68),
            (0xf7, 0x76, 0x8e),
            (0x9e, 0xce, 0x6a),
            (0xe0, 0xaf, 0x68),
            (0x7a, 0xa2, 0xf7),
            (0xbb, 0x9a, 0xf7),
            (0x7d, 0xcf, 0xff),
            (0xc0, 0xca, 0xf5),
        ],
    },
];
