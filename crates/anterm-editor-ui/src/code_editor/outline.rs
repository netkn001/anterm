//! A file's symbols — what Go to Symbol lists and the breadcrumbs walk.
//!
//! An [`Outline`] is plain data: language servers populate it through
//! `AntermApp::editor_set_document_symbols` from `textDocument/documentSymbol`
//! ([`Outline::from_nodes`]), and Markdown files get a lightweight offline
//! heading outline directly from their source text without requiring an LSP.

use std::ops::Range;

use gpui_kit::component::Rope;
use gpui_kit::component::input::{Position, RopeExt as _};

/// What a symbol is — the shapes the picker and the breadcrumbs tell apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Module,
    Namespace,
    Class,
    Struct,
    Enum,
    Interface,
    Impl,
    Function,
    Method,
    Constructor,
    Constant,
    Variable,
    Field,
    Type,
    Heading,
}

impl SymbolKind {
    /// LSP's `SymbolKind` number, for a language server's answer. Kinds with
    /// no counterpart here (a file, a package, an event…) fold into the
    /// nearest one that draws the same.
    pub fn from_lsp(kind: u32) -> Self {
        match kind {
            1 | 2 | 4 => Self::Module, // File, Module, Package
            3 => Self::Namespace,
            5 => Self::Class,
            6 => Self::Method,
            7 | 8 => Self::Field, // Property, Field
            9 => Self::Constructor,
            10 => Self::Enum,
            11 => Self::Interface,
            12 => Self::Function,
            13 => Self::Variable,
            14 | 22 => Self::Constant, // Constant, EnumMember
            23 => Self::Struct,
            26 => Self::Type, // TypeParameter
            _ => Self::Variable,
        }
    }

    /// The short tag the picker shows beside a name. Code's own words, the
    /// same in every locale, like the keywords they stand for.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Module => "mod",
            Self::Namespace => "namespace",
            Self::Class => "class",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Interface => "interface",
            Self::Impl => "impl",
            Self::Function => "fn",
            Self::Method => "method",
            Self::Constructor => "ctor",
            Self::Constant => "const",
            Self::Variable => "var",
            Self::Field => "field",
            Self::Type => "type",
            Self::Heading => "#",
        }
    }
}

/// One symbol, flattened into its outline.
#[derive(Clone, Debug, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// The whole item, 0-based, in the editor's own [`Position`]s.
    pub range: Range<Position>,
    /// Where a jump to it lands: its name.
    pub selection: Position,
    /// How many symbols it sits inside.
    pub depth: usize,
    /// The symbol it sits directly inside, as an index into the outline.
    pub parent: Option<usize>,
}

/// A nested symbol, the shape an LSP `DocumentSymbol` has.
#[derive(Clone, Debug)]
pub struct SymbolNode {
    pub name: String,
    pub kind: SymbolKind,
    pub range: Range<Position>,
    pub selection: Position,
    pub children: Vec<SymbolNode>,
}

/// A file's symbols in document order, every container before what it holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outline {
    pub symbols: Vec<Symbol>,
}

fn key(p: Position) -> (u32, u32) {
    (p.line, p.character)
}

impl Outline {
    /// Flattens a nested answer (a language server's `documentSymbol`).
    pub fn from_nodes(nodes: Vec<SymbolNode>) -> Self {
        fn walk(
            nodes: Vec<SymbolNode>,
            parent: Option<usize>,
            depth: usize,
            out: &mut Vec<Symbol>,
        ) {
            let mut nodes = nodes;
            nodes.sort_by_key(|n| key(n.range.start));
            for node in nodes {
                let ix = out.len();
                out.push(Symbol {
                    name: node.name,
                    kind: node.kind,
                    range: node.range,
                    selection: node.selection,
                    depth,
                    parent,
                });
                walk(node.children, Some(ix), depth + 1, out);
            }
        }
        let mut symbols = Vec::new();
        walk(nodes, None, 0, &mut symbols);
        Self { symbols }
    }

    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }

    /// The symbols `pos` is inside, outermost first — the breadcrumbs' trail.
    pub fn chain_at(&self, pos: Position) -> Vec<usize> {
        let at = key(pos);
        // Children follow their parent and siblings never overlap, so the
        // last symbol that holds the position is the innermost one.
        let Some(innermost) = self
            .symbols
            .iter()
            .rposition(|s| key(s.range.start) <= at && at <= key(s.range.end))
        else {
            return Vec::new();
        };
        let mut chain = vec![innermost];
        let mut cur = innermost;
        while let Some(parent) = self.symbols[cur].parent {
            chain.push(parent);
            cur = parent;
        }
        chain.reverse();
        chain
    }

    /// The names of what symbol `ix` sits inside, outermost first.
    pub fn container_path(&self, ix: usize) -> Vec<&str> {
        let mut names = Vec::new();
        let mut cur = self.symbols.get(ix).and_then(|s| s.parent);
        while let Some(p) = cur {
            names.push(self.symbols[p].name.as_str());
            cur = self.symbols[p].parent;
        }
        names.reverse();
        names
    }
}

/// Whether `language` has a built-in offline outline (without an LSP).
pub fn supports(language: &str) -> bool {
    matches!(language, "markdown")
}

/// The longest name a symbol shows.
const NAME_MAX_CHARS: usize = 80;

fn collapse(text: &str) -> String {
    let mut out = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.chars().count() > NAME_MAX_CHARS {
        out = out.chars().take(NAME_MAX_CHARS - 1).collect::<String>() + "…";
    }
    out
}

/// Lightweight offline outline for languages supported without an LSP
/// (currently Markdown headings). Code files receive their outlines from LSP
/// `textDocument/documentSymbol`.
pub fn outline_of_text(language: &str, text: &Rope) -> Outline {
    match language {
        "markdown" => markdown_outline(text),
        _ => Outline::default(),
    }
}

fn parse_fence(trimmed: &str) -> Option<(char, usize)> {
    let ch = trimmed.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let count = trimmed.bytes().take_while(|&b| b == ch as u8).count();
    if count >= 3 {
        Some((ch, count))
    } else {
        None
    }
}

fn parse_atx_heading(line: &str) -> Option<(u8, usize, &str)> {
    let bytes = line.as_bytes();
    let indent = bytes.iter().take_while(|&&b| b == b' ').count();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let hashes = rest.bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let after = &rest[hashes..];
    if !after.is_empty() && !after.starts_with(' ') && !after.starts_with('\t') {
        return None;
    }
    let ws = after
        .bytes()
        .take_while(|&b| b == b' ' || b == b'\t')
        .count();
    let mut title = after[ws..].trim_end();
    // Strip optional closing ATX hashes preceded by whitespace.
    let trimmed_hashes = title.trim_end_matches('#');
    if trimmed_hashes.len() < title.len()
        && (trimmed_hashes.is_empty()
            || trimmed_hashes.ends_with(' ')
            || trimmed_hashes.ends_with('\t'))
    {
        title = trimmed_hashes.trim_end();
    }
    if title.is_empty() {
        return None;
    }
    let title_col = indent + hashes + ws;
    Some((hashes as u8, title_col, title))
}

fn is_setext_underline(line: &str) -> Option<u8> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.bytes().all(|b| b == b'=') {
        Some(1)
    } else if trimmed.len() >= 2 && trimmed.bytes().all(|b| b == b'-') {
        Some(2)
    } else {
        None
    }
}

/// Extracts nested ATX (`#`..`######`) and Setext (`===`/`---`) headings from
/// Markdown while ignoring headings inside fenced code blocks.
fn markdown_outline(text: &Rope) -> Outline {
    struct Heading {
        level: u8,
        name: String,
        start_line: u32,
        selection: Position,
        /// End line for setext-only headings (None means extend through section).
        fixed_end: Option<Position>,
    }

    let source = text.to_string();
    let lines: Vec<&str> = source.lines().collect();
    let last_pos = text.offset_to_position(text.len());
    let mut headings: Vec<Heading> = Vec::new();
    let mut fence: Option<(char, usize)> = None;

    for (idx, raw_line) in lines.iter().enumerate() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let trimmed = line.trim_start();
        if let Some((fch, flen)) = fence {
            if let Some((c, n)) = parse_fence(trimmed)
                && c == fch
                && n >= flen
                && trimmed[n..].trim().is_empty()
            {
                fence = None;
            }
            continue;
        }
        if let Some(opened) = parse_fence(trimmed) {
            // Backtick info strings cannot contain backticks.
            if opened.0 != '`' || !trimmed[opened.1..].contains('`') {
                fence = Some(opened);
                continue;
            }
        }

        if let Some((level, col, title)) = parse_atx_heading(line) {
            let name = collapse(title);
            if !name.is_empty() {
                headings.push(Heading {
                    level,
                    name,
                    start_line: idx as u32,
                    selection: Position::new(idx as u32, col as u32),
                    fixed_end: None,
                });
            }
            continue;
        }

        if idx > 0
            && let Some(level) = is_setext_underline(line)
        {
            let prev = lines[idx - 1].strip_suffix('\r').unwrap_or(lines[idx - 1]);
            let prev_trimmed = prev.trim();
            if !prev_trimmed.is_empty()
                && parse_atx_heading(prev).is_none()
                && parse_fence(prev.trim_start()).is_none()
                && is_setext_underline(prev).is_none()
            {
                let name = collapse(prev_trimmed);
                if !name.is_empty() {
                    let col = prev.len() - prev.trim_start().len();
                    headings.push(Heading {
                        level,
                        name,
                        start_line: (idx - 1) as u32,
                        selection: Position::new((idx - 1) as u32, col as u32),
                        fixed_end: Some(Position::new(idx as u32, line.len() as u32)),
                    });
                }
            }
        }
    }

    let mut symbols: Vec<Symbol> = Vec::with_capacity(headings.len());
    // Stack of (symbol_index, heading_level) for open ATX sections.
    let mut stack: Vec<(usize, u8)> = Vec::new();

    for h in headings {
        while let Some(&(closed_ix, lvl)) = stack.last() {
            if lvl < h.level {
                break;
            }
            stack.pop();
            let end_line = h.start_line.saturating_sub(1);
            let end_col = lines
                .get(end_line as usize)
                .map(|l| l.strip_suffix('\r').unwrap_or(l).len() as u32)
                .unwrap_or(0);
            symbols[closed_ix].range.end = Position::new(end_line, end_col);
        }
        let parent = stack.last().map(|&(ix, _)| ix);
        let depth = stack.len();
        let start = Position::new(h.start_line, 0);
        let end = h.fixed_end.unwrap_or(last_pos);
        let ix = symbols.len();
        symbols.push(Symbol {
            name: h.name,
            kind: SymbolKind::Heading,
            range: start..end,
            selection: h.selection,
            depth,
            parent,
        });
        if h.fixed_end.is_none() {
            stack.push((ix, h.level));
        }
    }

    Outline { symbols }
}

#[cfg(test)]
pub fn outline_of(language: &str, source: &str) -> Outline {
    outline_of_text(language, &Rope::from(source))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `name@depth:kind` for every symbol, in order.
    fn shape(language: &str, source: &str) -> Vec<String> {
        outline_of(language, source)
            .symbols
            .iter()
            .map(|s| format!("{}@{}:{}", s.name, s.depth, s.kind.tag()))
            .collect()
    }

    #[test]
    fn markdown_headings_nest_by_level() {
        let src = "# Title\n\nintro\n\n## Install\n\ntext\n\n### From source\n\n```sh\n# not a heading\n```\n\n## Usage\n";
        assert_eq!(
            shape("markdown", src),
            ["Title@0:#", "Install@1:#", "From source@2:#", "Usage@1:#"]
        );
        assert_eq!(
            shape("markdown", "Setext\n======\n\ntext\n"),
            ["Setext@0:#"]
        );
    }

    #[test]
    fn non_markdown_languages_have_empty_offline_outline() {
        assert!(!supports("json"));
        assert!(!supports("rust"));
        assert!(supports("markdown"));
        assert!(outline_of("json", "{\"a\": 1}").is_empty());
        assert!(outline_of("rust", "fn main() {}").is_empty());
    }

    #[test]
    fn the_chain_at_a_position_runs_outermost_to_innermost() {
        let p = |l, c| Position::new(l, c);
        let outline = Outline::from_nodes(vec![
            SymbolNode {
                name: "net".into(),
                kind: SymbolKind::Module,
                range: p(0, 0)..p(6, 1),
                selection: p(0, 4),
                children: vec![SymbolNode {
                    name: "impl Server".into(),
                    kind: SymbolKind::Impl,
                    range: p(1, 4)..p(5, 5),
                    selection: p(1, 4),
                    children: vec![SymbolNode {
                        name: "serve".into(),
                        kind: SymbolKind::Method,
                        range: p(2, 8)..p(4, 9),
                        selection: p(2, 11),
                        children: vec![],
                    }],
                }],
            },
            SymbolNode {
                name: "main".into(),
                kind: SymbolKind::Function,
                range: p(7, 0)..p(7, 12),
                selection: p(7, 3),
                children: vec![],
            },
        ]);
        let names = |pos: Position| -> Vec<String> {
            outline
                .chain_at(pos)
                .into_iter()
                .map(|i| outline.symbols[i].name.clone())
                .collect()
        };
        assert_eq!(names(p(3, 12)), ["net", "impl Server", "serve"]);
        assert_eq!(names(p(1, 0)), ["net"]);
        assert_eq!(names(p(7, 3)), ["main"]);
        assert_eq!(names(p(6, 0)), ["net"]);
        assert!(names(p(6, 2)).is_empty());
        let serve = outline
            .symbols
            .iter()
            .position(|s| s.name == "serve")
            .unwrap();
        assert_eq!(outline.container_path(serve), ["net", "impl Server"]);
        assert_eq!(outline.symbols[serve].selection, p(2, 11));
    }

    #[test]
    fn a_language_servers_nested_answer_flattens_the_same_way() {
        let p = |l, c| Position::new(l, c);
        let outline = Outline::from_nodes(vec![
            SymbolNode {
                name: "later".into(),
                kind: SymbolKind::from_lsp(12),
                range: p(10, 0)..p(12, 1),
                selection: p(10, 3),
                children: vec![],
            },
            SymbolNode {
                name: "Server".into(),
                kind: SymbolKind::from_lsp(5),
                range: p(0, 0)..p(8, 1),
                selection: p(0, 6),
                children: vec![SymbolNode {
                    name: "serve".into(),
                    kind: SymbolKind::from_lsp(6),
                    range: p(2, 2)..p(4, 3),
                    selection: p(2, 6),
                    children: vec![],
                }],
            },
        ]);
        let got: Vec<_> = outline
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.depth, s.parent))
            .collect();
        assert_eq!(
            got,
            [
                ("Server", 0, None),
                ("serve", 1, Some(0)),
                ("later", 0, None)
            ]
        );
        assert_eq!(outline.chain_at(p(3, 0)), [0, 1]);
    }
}
