//! Per-agent screen-scraping heuristics, `/` and `@` menu sources, and toolbar
//! readings (model, effort, and permission mode labels).

use std::path::Path;

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, Rgb};
use gpui::Pixels;

use super::plan::mention;
use anterm_core::cli_agent::CLIAgent;
use anterm_ui_kit::host_ops::Host;
use anterm_i18n::{L10nKey, t};
use anterm_ui_kit::search::files::{FileIndex, IndexedFile, rank};

/// Rows the `/` and `@` menu shows at once.
const MENU_ROWS: usize = 8;

/// How far up from the bottom of the screen an agent's input area can start.
/// Past this the thing found is transcript, not the input.
const INPUT_AREA_MAX_ROWS: usize = 40;

/// The models `/model` takes by alias, in the order its own list gives the
/// families, with the 1M-context variants it offers. An account without one
/// says so itself.
pub(super) const CLAUDE_MODELS: [&str; 7] = [
    "default",
    "opus",
    "opus[1m]",
    "fable",
    "sonnet",
    "sonnet[1m]",
    "haiku",
];

/// Claude Code's effort levels, lowest first. A model that takes fewer says so
/// itself when `/effort` names one it does not.
pub(super) const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

// ---------------------------------------------------------------------------
// Finding the agent's input area

/// Where an agent's input area starts on the screen, and what its status rows
/// say about the permission mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InputArea {
    /// Screen row of the area's first line.
    pub top: usize,
    /// The permission mode, in the agent's own words (`acceptEdits`, …),
    /// when its status rows name one.
    pub mode: Option<&'static str>,
}

/// Whether the box can be laid over this agent's input.
pub(super) fn covers(agent: CLIAgent) -> bool {
    matches!(
        agent,
        CLIAgent::Claude
            | CLIAgent::Codex
            | CLIAgent::Gemini
            | CLIAgent::Pi
            | CLIAgent::OhMyPi
            | CLIAgent::PrimeAgent
            | CLIAgent::Grok
            | CLIAgent::Crush
            | CLIAgent::Goose
            | CLIAgent::Amp
            | CLIAgent::Copilot
            | CLIAgent::Qwen
            | CLIAgent::CodeBuddy
            | CLIAgent::Kimi
            | CLIAgent::OpenCode
            | CLIAgent::QoderCLI
            | CLIAgent::QoderCLICn
    )
}

/// Whether the grid gives up rows for the box covering `agent`'s input — see
/// [`crate::view::TerminalView::composer_reserved_rows`]. Prime Agent's four-row block
/// falls a row short, which cut its hint line above in half.
pub(super) fn makes_room(agent: CLIAgent) -> bool {
    matches!(
        agent,
        CLIAgent::OhMyPi | CLIAgent::PrimeAgent | CLIAgent::Goose
    )
}

/// Whole rows of `line` it takes to make up what `area` rows of `line`, with
/// `below` under them, fall short of `need` — none when they don't.
pub(crate) fn rows_short(need: Pixels, line: Pixels, area: usize, below: Pixels) -> usize {
    let line = line.as_f32();
    if line <= 0. {
        return 0;
    }
    let short = need.as_f32() - line * area as f32 - below.as_f32();
    match short > 0. {
        true => (short / line).ceil() as usize,
        false => 0,
    }
}

fn is_rule(row: &str, width: usize) -> bool {
    let t = row.trim();
    t.chars().count() >= width * 3 / 5 && t.chars().all(|c| c == '─')
}

/// Find `agent`'s input area in `rows`, the screen's lines top to bottom.
///
/// - **Claude Code** draws its input between two full-width rules with `❯`
///   on the first line, and its status rows under the lower rule.
/// - **Codex** starts its input line with `›`, on a padded block.
/// - **Gemini** shades it as a block whose first line is `> ` — see
///   [`gemini_input_top`] for its other drawings.
/// - **Pi** draws it between two rules, over its status rows — see
///   [`pi_input_top`].
/// - **Oh My Pi** hangs it under its status line — see [`omp_input_top`].
/// - **Prime Agent** shades it as a block over its footer — see
///   [`prime_input_top`].
/// - **Grok Build** frames it in a rounded box — see [`grok_input_top`].
/// - **Crush** draws its editor over its key help, its dialogs framed over
///   it — see [`crush_input_top`].
/// - **Goose** prints its prompt under its context gauge, wherever its output
///   ended — see [`goose_input_top`].
/// - **Amp** frames it at the bottom of the screen — see [`amp_input_top`].
/// - **Copilot**, **Qwen Code** and **CodeBuddy** rule it off top and bottom,
///   over their status rows — see [`ruled_input_top`].
/// - **Kimi Code** frames it over its model line — see [`kimi_input_top`].
/// - **OpenCode** draws it on a bar over its key hints — see
///   [`opencode_input_top`].
/// - **Qoder** rules it off under its mode line, over its model line — see
///   [`qoder_input`].
///
/// `None` is the agent showing something else there — which is exactly when
/// the box must get out of the way.
pub(crate) fn input_area(agent: CLIAgent, rows: &[String], width: usize) -> Option<InputArea> {
    let floor = rows.len().saturating_sub(INPUT_AREA_MAX_ROWS);
    let starts = |i: usize, p: char| rows[i].trim_start().starts_with(p);
    match agent {
        CLIAgent::Claude => {
            let rules: Vec<usize> = (floor..rows.len())
                .filter(|&i| is_rule(&rows[i], width))
                .collect();
            rules.windows(2).rev().find_map(|pair| {
                let (upper, lower) = (pair[0], pair[1]);
                (upper + 1..lower)
                    .any(|i| starts(i, '❯'))
                    .then(|| InputArea {
                        top: upper,
                        mode: Some(claude_mode(&rows[lower + 1..])),
                    })
            })
        }
        CLIAgent::Codex => {
            let line = (floor..rows.len()).rev().find(|&i| starts(i, '›'))?;
            // Codex points at the selected row of its own lists — approvals,
            // `/model`, `/permissions`, the hook review, the update offer —
            // with the same `›`. Such a row is a numbered option with its
            // siblings around it; the input is not.
            let option = |row: &str| numbered_option(row.trim_start().trim_start_matches('›'));
            if option(&rows[line])
                && (line.saturating_sub(CODEX_LIST_REACH)..rows.len())
                    .any(|i| i != line && option(&rows[i]))
            {
                return None;
            }
            let top = match line > 0 && rows[line - 1].trim().is_empty() {
                true => line - 1,
                false => line,
            };
            Some(InputArea { top, mode: None })
        }
        CLIAgent::Gemini => gemini_input_top(rows, floor, width).map(|top| InputArea {
            top: over_mode_line(rows, floor, width, top),
            mode: None,
        }),
        CLIAgent::Pi => pi_input_top(rows, floor, width).map(|top| InputArea { top, mode: None }),
        CLIAgent::OhMyPi => {
            omp_input_top(rows, floor, width).map(|top| InputArea { top, mode: None })
        }
        CLIAgent::PrimeAgent => {
            prime_input_top(rows, floor).map(|top| InputArea { top, mode: None })
        }
        CLIAgent::Grok => {
            grok_input_top(rows, floor, width).map(|top| InputArea { top, mode: None })
        }
        CLIAgent::Crush => crush_input_top(rows, floor).map(|top| InputArea { top, mode: None }),
        // Goose's input is the last thing printed, wherever that is: nothing
        // is drawn under it, so it counts from any height.
        CLIAgent::Goose => goose_input_top(rows).map(|top| InputArea { top, mode: None }),
        CLIAgent::Amp => amp_input_top(rows, floor, width).map(|top| InputArea { top, mode: None }),
        CLIAgent::Copilot => copilot_shaded_input(rows, floor, width)
            .map(|(top, _)| top)
            .or_else(|| ruled_input_top(rows, floor, width, ruled_prompts(agent)))
            .map(|top| InputArea { top, mode: None }),
        CLIAgent::Qwen | CLIAgent::CodeBuddy => {
            ruled_input_top(rows, floor, width, ruled_prompts(agent))
                .map(|top| InputArea { top, mode: None })
        }
        CLIAgent::Kimi => kimi_input_top(rows, floor).map(|top| InputArea { top, mode: None }),
        CLIAgent::OpenCode => {
            opencode_input_top(rows, floor).map(|top| InputArea { top, mode: None })
        }
        CLIAgent::QoderCLI | CLIAgent::QoderCLICn => {
            qoder_input(rows, floor, width).map(|q| InputArea {
                top: q.top,
                mode: None,
            })
        }
        _ => None,
    }
}

/// The prompts an input ruled off top and bottom opens with: Copilot's `❯`;
/// Qwen Code's `>`, `*` in YOLO mode and `!` in shell mode; CodeBuddy's `>`,
/// `!` in bash mode.
fn ruled_prompts(agent: CLIAgent) -> &'static [char] {
    match agent {
        CLIAgent::Copilot => &['❯'],
        CLIAgent::Qwen => &['>', '*', '!'],
        _ => &['>', '!'],
    }
}

/// How many rows Crush draws under its editor at most: a blank row and its
/// key help, which `ctrl+g` opens out to several.
const CRUSH_FOOTER_MAX_ROWS: usize = 8;

/// Where Crush's editor starts: its prompt line, `> ` (`!` in yolo mode),
/// with every further line opening with `:::` — and the first one too while
/// the chat, not the editor, has the keys. A blank row and the key help are
/// under it, at the bottom of the screen.
///
/// Crush draws its dialogs — the command palette, the model and session
/// lists, a permission request — framed over the screen, the editor left
/// standing under them; they take the keys while open, so with a frame on
/// screen the box steps aside.
fn crush_input_top(rows: &[String], floor: usize) -> Option<usize> {
    let frame_top = |r: &String| {
        r.find('╭').is_some_and(|at| {
            let edge = &r[at + '╭'.len_utf8()..];
            let run = edge.chars().take_while(|&c| c == '─').count();
            run >= 2 && edge.chars().nth(run) == Some('╮')
        })
    };
    if rows.iter().any(frame_top) {
        return None;
    }
    let more = |i: usize| rows[i].trim_start().starts_with(":::");
    let last = (floor..rows.len()).rev().find(|&i| more(i))?;
    let mut first = last;
    while first > floor && more(first - 1) {
        first -= 1;
    }
    let prompt = |row: &str| {
        let t = row.trim_start();
        t == ">" || t.starts_with("> ") || t.starts_with('!')
    };
    let top = match first.checked_sub(1) {
        Some(above) if above >= floor && prompt(&rows[above]) => above,
        _ => first,
    };
    let blank_under = rows.get(last + 1).is_some_and(|r| r.trim().is_empty());
    let footer = rows[last + 1..]
        .iter()
        .filter(|r| !r.trim().is_empty())
        .count();
    (blank_under && footer <= CRUSH_FOOTER_MAX_ROWS).then_some(top)
}

/// Where Goose's input starts: the context gauge (`╌╌╌ 1% 5k/1.0M`) right
/// over its `> ` line, which is the last thing on the screen.
///
/// Goose is a line-at-a-time prompt, not a full-screen one: while a turn
/// runs its spinner and the turn's output are printed under the line just
/// sent, and a question it asks — a tool approval — is printed there too,
/// so the box steps aside until the next gauge and `> ` close the turn.
fn goose_input_top(rows: &[String]) -> Option<usize> {
    let last = rows.iter().rposition(|r| !r.trim().is_empty())?;
    let gauge = last.checked_sub(1)?;
    let t = rows[gauge].trim();
    let prompt = rows[last] == ">" || rows[last].starts_with("> ");
    (t.starts_with('╌') && t.contains('%') && prompt).then_some(gauge)
}

/// Where Amp's input starts: the top edge of the frame at the bottom of the
/// screen, its mode named in that edge and the directory in the bottom one.
///
/// Its command palette and pickers open framed over the screen, indented from
/// its left edge; a notice it puts in the input's place — out of credits — is
/// a full-width frame with nothing in its bottom edge. The box steps aside
/// for both.
fn amp_input_top(rows: &[String], floor: usize, width: usize) -> Option<usize> {
    let bottom = (floor..rows.len())
        .rev()
        .find(|&i| !rows[i].trim().is_empty())?;
    let edge = rows[bottom].trim_end();
    let labelled = edge.chars().any(|c| !matches!(c, '╰' | '─' | '╯' | ' '));
    if !(edge.starts_with('╰')
        && edge.ends_with('╯')
        && edge.chars().count() >= width * 3 / 5
        && labelled)
    {
        return None;
    }
    let top = (floor..bottom).rev().find(|&i| !rows[i].starts_with('│'))?;
    let framed = rows[top].starts_with('╭') && rows[top].trim_end().ends_with('╮');
    let overlay = rows[..top]
        .iter()
        .any(|r| !r.starts_with('╭') && r.trim_start().starts_with('╭'));
    (framed && top + 1 < bottom && !overlay).then_some(top)
}

/// How many rows Copilot, Qwen Code and CodeBuddy draw under their input's
/// lower rule at most: their status rows, which wrap in a narrow pane.
const RULED_FOOTER_MAX_ROWS: usize = 5;

/// Where an input ruled off top and bottom starts — Copilot's, Qwen Code's,
/// CodeBuddy's: the upper of the last two full-width rules on the screen,
/// with the prompt (one of `prompts`, at the very start of the row) on the
/// line right under it and no more than the status rows under the lower one.
///
/// What they put in the input's place is no such block: their tool approvals
/// and pickers are drawn between rules of their own — or in a frame — with
/// the question, not the prompt, on the first line, or with the options
/// under the lower rule; and their `/` and `@` lists open under the lower
/// rule (Qwen Code, CodeBuddy) or right over the upper one (Copilot, its rows
/// on a `┃` bar). They take the keys while open, so the box steps aside.
fn ruled_input_top(rows: &[String], floor: usize, width: usize, prompts: &[char]) -> Option<usize> {
    let mut rules = (floor..rows.len())
        .rev()
        .filter(|&i| is_rule(&rows[i], width));
    let lower = rules.next()?;
    let upper = rules.next()?;
    let footer = rows[lower + 1..]
        .iter()
        .filter(|r| !r.trim().is_empty())
        .count();
    let prompt = rows[upper + 1]
        .chars()
        .next()
        .is_some_and(|c| prompts.contains(&c));
    let list_above = upper > 0 && rows[upper - 1].starts_with('┃');
    (upper + 1 < lower && prompt && footer <= RULED_FOOTER_MAX_ROWS && !list_above).then_some(upper)
}

/// Where Copilot's input starts, and where it ends, when it shades it — as
/// it does in a terminal with true colour: a block opened by a `╻▄▄▄` edge
/// and closed by a `╹▀▀▀` one, its lines on a `┃` bar, the status rows under
/// it. Elsewhere it rules the input off — see [`ruled_input_top`].
///
/// Its `/` and `@` lists open on a `┃` bar right over the block, and take the
/// keys while open, so the box steps aside for them.
fn copilot_shaded_input(rows: &[String], floor: usize, width: usize) -> Option<(usize, usize)> {
    let edge = |r: &str, corner: char, run: char| {
        let t = r.trim();
        t.starts_with(corner)
            && t.chars().count() >= width * 3 / 5
            && t.chars().skip(1).all(|c| c == run)
    };
    let bottom = (floor..rows.len())
        .rev()
        .find(|&i| edge(&rows[i], '╹', '▀'))?;
    let footer = rows[bottom + 1..]
        .iter()
        .filter(|r| !r.trim().is_empty())
        .count();
    let top = (floor..bottom).rev().find(|&i| !rows[i].starts_with('┃'))?;
    let list_above = copilot_list_over(rows, top);
    (edge(&rows[top], '╻', '▄')
        && top + 1 < bottom
        && footer <= RULED_FOOTER_MAX_ROWS
        && !list_above)
        .then_some((top, bottom))
}

/// How far over Copilot's shaded input its `/` and `@` lists reach: their
/// rows, and the blank ones a short list leaves under it.
const COPILOT_LIST_REACH: usize = 12;

/// Whether one of Copilot's `/` or `@` lists is open over its input, whose
/// top is `top`: on a `┃` bar right over it, or — drawn shaded — as rows
/// whose selected one opens with `❯ /` or `❯ @`. A message sent before is
/// drawn with a `❯` too, but with the time it was sent at the end.
fn copilot_list_over(rows: &[String], top: usize) -> bool {
    if top > 0 && rows[top - 1].starts_with('┃') {
        return true;
    }
    rows[top.saturating_sub(COPILOT_LIST_REACH)..top]
        .iter()
        .any(|r| {
            let t = r.trim();
            let sent_at = t.len() >= 5
                && t.is_char_boundary(t.len() - 5)
                && t[t.len() - 5..].chars().enumerate().all(|(i, c)| match i {
                    2 => c == ':',
                    _ => c.is_ascii_digit(),
                });
            (t.starts_with("❯ /") || t.starts_with("❯ @")) && !sent_at
        })
}

/// Whether `agent`'s input, as `rows` show it, is in its shell mode — which
/// Qwen Code and CodeBuddy stay in after a `!` command has run, their prompt
/// a `!`, until Esc takes them out.
pub(super) fn in_shell_mode(agent: CLIAgent, rows: &[String], width: usize) -> bool {
    let floor = rows.len().saturating_sub(INPUT_AREA_MAX_ROWS);
    match agent {
        CLIAgent::Qwen | CLIAgent::CodeBuddy => {
            ruled_input_top(rows, floor, width, ruled_prompts(agent))
                .is_some_and(|top| rows[top + 1].starts_with('!'))
        }
        CLIAgent::QoderCLI | CLIAgent::QoderCLICn => qoder_input(rows, floor, width)
            .is_some_and(|q| rows[q.first].trim_start().starts_with('!')),
        _ => false,
    }
}

/// How many rows Qoder draws under its input's lower rule at most: its model
/// line, which wraps in a narrow pane.
const QODER_FOOTER_MAX_ROWS: usize = 3;

/// How far over its input the mode line of Gemini CLI and Qoder reaches,
/// from the rule over it: in a narrow pane Qoder's mode and MCP count go on
/// rows of their own, a blank one between them.
const MODE_LINE_MAX_ROWS: usize = 4;

/// Qoder's input, as [`qoder_input`] finds it.
struct QoderInput {
    /// The first row the box covers.
    top: usize,
    /// The input's first line, the prompt's.
    first: usize,
    /// The rule under its last line.
    lower: usize,
}

/// Where Qoder's input is: the lines between the last two full-width rules,
/// the first opening with its prompt — ` > `, ` * ` in YOLO mode, ` ! ` in
/// shell mode — and only its model line under the lower rule. Over the upper
/// rule its mode line has a rule of its own, and the `? for shortcuts` hint
/// sits over that; the box covers them too, or they would be left standing
/// over it.
///
/// Its `/` and `@` lists open under the lower rule, in the model line's
/// place, the selected row opening with `❯`, and take the keys while open.
/// Its dialogs — the folder trust and sign-in prompts, `/model`, `/help` —
/// open under a rule of their own with no prompt under it, and a sent message
/// is a ` > ` line with no rules around it. The box steps aside for all of
/// them.
fn qoder_input(rows: &[String], floor: usize, width: usize) -> Option<QoderInput> {
    let mut rules = (floor..rows.len())
        .rev()
        .filter(|&i| is_rule(&rows[i], width));
    let lower = rules.next()?;
    let upper = rules.next()?;
    let first = upper + 1;
    let prompt = rows[first].strip_prefix(' ').is_some_and(|t| {
        let mut chars = t.chars();
        matches!(chars.next(), Some('>' | '*' | '!')) && matches!(chars.next(), None | Some(' '))
    });
    let footer: Vec<&str> = rows[lower + 1..]
        .iter()
        .map(|r| r.trim())
        .filter(|t| !t.is_empty())
        .collect();
    let list = footer.iter().any(|t| t.starts_with(['❯', '▲', '▼']));
    if !(first < lower && prompt && footer.len() <= QODER_FOOTER_MAX_ROWS && !list) {
        return None;
    }
    let top = over_mode_line(rows, floor, width, upper);
    Some(QoderInput { top, first, lower })
}

/// The row to cover from for an input starting at `top` that Gemini CLI or
/// a fork of it draws: from the rule over its mode line ("Shift+Tab to
/// accept edits", "shell mode enabled"), and the `? for shortcuts` hint over
/// that, when they are there — left standing, they would show over the box.
fn over_mode_line(rows: &[String], floor: usize, width: usize, top: usize) -> usize {
    let Some(rule) = (top.saturating_sub(MODE_LINE_MAX_ROWS).max(floor)..top)
        .rev()
        .find(|&i| is_rule(&rows[i], width))
    else {
        return top;
    };
    let between = &rows[rule + 1..top];
    let mode_line = between.iter().any(|r| !r.trim().is_empty())
        && between
            .iter()
            .all(|r| !r.trim_start().starts_with(['>', '❯', '│', '╭', '╰']));
    if !mode_line {
        return top;
    }
    match rule > floor && rows[rule - 1].trim() == "? for shortcuts" {
        true => rule - 1,
        false => rule,
    }
}

/// How many rows Kimi Code draws under its input's frame at most: its model
/// and directory line and the context gauge, which wrap in a narrow pane.
const KIMI_FOOTER_MAX_ROWS: usize = 4;

/// Where Kimi Code's input starts: the top edge of the rounded frame whose
/// first line inside opens with `>`, with its model line and context gauge
/// under it.
///
/// Its `/` and `@` lists open under the frame, between it and the model
/// line; a tool approval takes the frame's place between two rules; the
/// welcome banner is a frame of its own, far up with the transcript under
/// it. The box steps aside for the lists and approvals.
fn kimi_input_top(rows: &[String], floor: usize) -> Option<usize> {
    let edge = |r: &str, open: char, close: char| {
        let t = r.trim();
        t.starts_with(open) && t.ends_with(close) && t.chars().count() > 2
    };
    let bottom = (floor..rows.len())
        .rev()
        .find(|&i| edge(&rows[i], '╰', '╯'))?;
    let footer = rows[bottom + 1..]
        .iter()
        .filter(|r| !r.trim().is_empty())
        .count();
    let top = (floor..bottom)
        .rev()
        .find(|&i| !rows[i].trim_start().starts_with('│'))?;
    let prompt = rows.get(top + 1).is_some_and(|r| {
        r.trim_start()
            .trim_start_matches('│')
            .trim_start()
            .starts_with('>')
    });
    (edge(&rows[top], '╭', '╮') && top + 1 < bottom && prompt && footer <= KIMI_FOOTER_MAX_ROWS)
        .then_some(top)
}

/// How many rows OpenCode draws under its input at most: its key hints, and
/// on its home screen the directory at the bottom of the screen.
const OPENCODE_FOOTER_MAX_ROWS: usize = 3;

/// Where OpenCode's input starts: the top of the block it draws on a `┃`
/// bar, the agent and model on its last line, closed by a `╹▀▀▀` edge with
/// the key hints under it.
///
/// Its `/` and `@` lists open on the same bar right over the input, each row
/// closed by a `┃` at its right; a permission request takes the input's
/// place on a bar with no edge under it. The sent messages in the
/// transcript are on a bar too, without the edge. The box steps aside for
/// the lists and requests.
fn opencode_input_top(rows: &[String], floor: usize) -> Option<usize> {
    let bar = |r: &str| r.trim_start().starts_with('┃');
    let edge = (floor..rows.len()).rev().find(|&i| {
        let t = rows[i].trim();
        t.starts_with('╹') && t.chars().skip(1).all(|c| c == '▀') && t.chars().count() > 2
    })?;
    let footer = rows[edge + 1..]
        .iter()
        .filter(|r| !r.trim().is_empty())
        .count();
    let mut top = edge;
    while top > floor && bar(&rows[top - 1]) {
        top -= 1;
    }
    let list = rows[top..edge].iter().any(|r| {
        let t = r.trim();
        t.chars().count() > 1 && t.ends_with('┃')
    });
    (top + 2 <= edge && footer <= OPENCODE_FOOTER_MAX_ROWS && !list).then_some(top)
}

/// How many rows Grok Build draws under its input's frame at most: a blank
/// row and its key hints.
const GROK_FOOTER_MAX_ROWS: usize = 3;

/// Where Grok Build's input starts: the top of its rounded frame, whose first
/// line inside opens with `❯`, with the model named in its bottom edge and
/// the key hints under it.
///
/// Its lists — `/` commands, the `/model` picker — open above the frame
/// between two rules, the lower one right on top of the frame, and take the
/// arrow keys and Enter there, so with one open the box steps aside. Its
/// dialogs — the release notes, the shortcuts — are drawn over the frame,
/// breaking its top edge, and the box steps aside for them too.
fn grok_input_top(rows: &[String], floor: usize, width: usize) -> Option<usize> {
    let bottom = (floor..rows.len()).rev().find(|&i| {
        let t = rows[i].trim();
        t.starts_with('╰') && t.ends_with('╯')
    })?;
    let footer = rows[bottom + 1..]
        .iter()
        .filter(|r| !r.trim().is_empty())
        .count();
    let top = (floor..bottom)
        .rev()
        .find(|&i| rows[i].trim_start().starts_with('╭'))?;
    // An edge with anything on it is a dialog drawn over the frame.
    let edge = rows[top]
        .trim()
        .chars()
        .all(|c| matches!(c, '╭' | '─' | '╮'));
    let prompt = rows.get(top + 1).is_some_and(|r| {
        r.trim_start()
            .trim_start_matches('│')
            .trim_start()
            .starts_with('❯')
    });
    let list_open = top > 0 && is_rule(&rows[top - 1], width);
    (footer <= GROK_FOOTER_MAX_ROWS && top + 1 < bottom && edge && prompt && !list_open)
        .then_some(top)
}

/// Crush's placeholders: what its empty editor says, idle, mid-turn and in
/// yolo mode.
const CRUSH_PLACEHOLDERS: [&str; 11] = [
    "Ready!",
    "Ready?",
    "Ready...",
    "Ready for instructions",
    "Working!",
    "Working...",
    "Brrrrr...",
    "Prrrrrrrr...",
    "Processing...",
    "Thinking...",
    "Go crazy",
];

/// How many lines of text `agent`'s input, as `rows` show it, holds — down
/// to the last one with anything on it.
///
/// Under the box nobody sees it, and the next message would be typed on
/// after it. Grok Build keeps a command whose list was dismissed — `/model`,
/// closed with Esc — in its input; Crush, Goose and Amp keep whatever was
/// typed into them before the box was opened over them.
pub(super) fn held_lines(agent: CLIAgent, rows: &[String], width: usize) -> usize {
    let floor = rows.len().saturating_sub(INPUT_AREA_MAX_ROWS);
    // Lines `first..` hold text; how many, down to the last that does.
    let count = |lines: Vec<&str>| {
        lines
            .iter()
            .rposition(|l| !l.trim().is_empty())
            .map_or(0, |last| last + 1)
    };
    match agent {
        CLIAgent::Grok => grok_input_top(rows, floor, width).map_or(0, |top| {
            let line = rows[top + 1].trim().trim_matches('│').trim();
            usize::from(!line.trim_start_matches('❯').trim().is_empty())
        }),
        CLIAgent::Crush => crush_input_top(rows, floor).map_or(0, |top| {
            let mut lines: Vec<&str> = rows[top..]
                .iter()
                .take_while(|r| !r.trim().is_empty())
                .map(|r| {
                    let t = r.trim_start();
                    let t = t.strip_prefix(":::").unwrap_or(t);
                    let t = t.strip_prefix('>').unwrap_or(t);
                    t.strip_prefix('!').unwrap_or(t)
                })
                .collect();
            if lines
                .first()
                .is_some_and(|l| CRUSH_PLACEHOLDERS.contains(&l.trim()))
            {
                lines[0] = "";
            }
            count(lines)
        }),
        CLIAgent::Goose => goose_input_top(rows).map_or(0, |top| {
            let line = rows[top + 1].trim_start_matches('>').trim();
            usize::from(!line.is_empty() && line != "Enter to send · Ctrl+J newline")
        }),
        CLIAgent::Amp => amp_input_top(rows, floor, width).map_or(0, |top| {
            count(
                rows[top + 1..]
                    .iter()
                    .take_while(|r| r.starts_with('│'))
                    .map(|r| r.trim_matches(|c: char| c == '│' || c == ' '))
                    .collect(),
            )
        }),
        CLIAgent::Copilot if let Some((top, bottom)) = copilot_shaded_input(rows, floor, width) => {
            count(
                rows[top + 1..bottom]
                    .iter()
                    .map(|r| r.trim_start_matches('┃').trim())
                    .collect(),
            )
        }
        CLIAgent::Copilot | CLIAgent::Qwen | CLIAgent::CodeBuddy => {
            ruled_input_top(rows, floor, width, ruled_prompts(agent)).map_or(0, |top| {
                let lines: Vec<&str> = rows[top + 1..]
                    .iter()
                    .take_while(|r| !is_rule(r, width))
                    .enumerate()
                    .map(|(i, r)| {
                        let mut chars = r.chars();
                        if i == 0 {
                            chars.next();
                        }
                        let t = chars
                            .as_str()
                            .trim_matches(|c: char| c.is_whitespace() || c == '\u{200b}');
                        // Placeholders, and a suggestion offered to send as is.
                        match t == "Type your message or @path/to/file" || t.ends_with("↵ send") {
                            true => "",
                            false => t,
                        }
                    })
                    .collect();
                count(lines)
            })
        }
        CLIAgent::Kimi => kimi_input_top(rows, floor).map_or(0, |top| {
            count(
                rows[top + 1..]
                    .iter()
                    .take_while(|r| r.trim_start().starts_with('│'))
                    .enumerate()
                    .map(|(i, r)| {
                        let t = r.trim().trim_matches('│').trim();
                        match i {
                            0 => t.trim_start_matches('>').trim(),
                            _ => t,
                        }
                    })
                    .collect(),
            )
        }),
        CLIAgent::OpenCode => opencode_input_top(rows, floor).map_or(0, |top| {
            let block: Vec<&str> = rows[top..]
                .iter()
                .take_while(|r| r.trim_start().starts_with('┃'))
                .map(|r| r.trim().trim_start_matches('┃').trim())
                .collect();
            // The last line names the agent and model.
            let lines = block[..block.len().saturating_sub(1)]
                .iter()
                .map(|l| match l.starts_with("Ask anything...") {
                    true => "",
                    false => l,
                })
                .collect();
            count(lines)
        }),
        CLIAgent::QoderCLI | CLIAgent::QoderCLICn => {
            qoder_input(rows, floor, width).map_or(0, |q| {
                count(
                    rows[q.first..q.lower]
                        .iter()
                        .enumerate()
                        .map(|(i, r)| {
                            let t = r.trim();
                            let t = match i {
                                0 => t.get(1..).unwrap_or("").trim(),
                                _ => t,
                            };
                            // Its placeholders, idle and in shell mode.
                            match t {
                                "Type your message or @path/to/file"
                                | "Type your shell command" => "",
                                _ => t,
                            }
                        })
                        .collect(),
                )
            })
        }
        _ => 0,
    }
}

/// Where Prime Agent's input starts: the padding row atop its shaded block.
///
/// The block is a blank row, the prompt line ` >  …` with any further lines
/// indented under it, and another blank row, right over the footer — `←
/// manage` and the model — which is the last thing on the screen. Its `/`
/// list opens above the block. The model picker draws its own ` >` search
/// line between two rules, away from the footer, and is not the input.
fn prime_input_top(rows: &[String], floor: usize) -> Option<usize> {
    let footer = (floor..rows.len())
        .rev()
        .find(|&i| !rows[i].trim().is_empty())?;
    // The block's lower padding row, then its lines up to the prompt's.
    let mut line = footer.checked_sub(1)?;
    if line < floor || !rows[line].trim().is_empty() {
        return None;
    }
    loop {
        line = line.checked_sub(1)?;
        if line < floor {
            return None;
        }
        let row = &rows[line];
        if row.trim().is_empty() {
            return None;
        }
        if row.starts_with(' ') && row.trim_start().starts_with('>') {
            break;
        }
        if !row.starts_with("    ") {
            return None;
        }
    }
    let top = line.checked_sub(1)?;
    (top >= floor && rows[top].trim().is_empty()).then_some(top)
}

/// Where Oh My Pi's input starts, in the composer shapes that keep it findable.
///
/// - **The status band**, its default, and **the rounded box**: the status
///   line — model, directory, branch, context gauge — on the row above the
///   input, whose first line opens with `╰─ `. Its own lists open below that
///   line. A dialog's frame closes with `╰──…╯` instead, and a tool approval
///   or a picker takes the input's place, so neither reads as the input.
/// - **The Pi and Claude Code shapes** rule the input off the way Pi does —
///   see [`pi_input_top`].
///
/// Its borderless, field and rail shapes leave no frame to find, and the box
/// stays aside for them.
fn omp_input_top(rows: &[String], floor: usize, width: usize) -> Option<usize> {
    let line = (floor + 1..rows.len()).rev().find(|&i| {
        rows[i]
            .trim_start()
            .strip_prefix("╰─")
            .is_some_and(|rest| !rest.starts_with('─'))
    });
    match line {
        Some(line) => (!rows[line - 1].trim().is_empty()).then_some(line - 1),
        None => pi_input_top(rows, floor, width),
    }
}

/// How many rows Pi draws under its input at most: the directory and usage
/// rows, an extension's status, and its own completion list, which opens
/// there.
const PI_FOOTER_MAX_ROWS: usize = 10;

/// Where Pi's input starts: its upper rule.
///
/// Pi draws its editor between two full-width rules, the upper one carrying
/// the turn's spinner (`── ⠹ Working ──`) while it works, with the
/// directory and usage rows under the lower one. Its selectors — `/model`,
/// `/resume`, `/tree`, `/login` — take the editor's place between the same
/// rules, but open on a blank row and run on for several, and `/settings`
/// opens on its `>` search line; the editor is one row when empty, and opens
/// on its text otherwise. Only the lowest pair of rules counts: anything
/// ruled higher up is the transcript's.
fn pi_input_top(rows: &[String], floor: usize, width: usize) -> Option<usize> {
    let rule = |row: &str| {
        let t = row.trim();
        t.starts_with("──")
            && t.ends_with('─')
            && t.chars().filter(|&c| c == '─').count() >= width * 3 / 5
    };
    let rules: Vec<usize> = (floor..rows.len()).filter(|&i| rule(&rows[i])).collect();
    let (&lower, higher) = rules.split_last()?;
    let &upper = higher.last()?;
    let footer = rows[lower + 1..]
        .iter()
        .filter(|r| !r.trim().is_empty())
        .count();
    let inside = &rows[upper + 1..lower];
    let opens_on_text = |row: &String| {
        let t = row.trim_end();
        !t.trim().is_empty() && t != ">" && !t.starts_with("> ")
    };
    let editor = inside.len() == 1 || inside.first().is_some_and(opens_on_text);
    (footer <= PI_FOOTER_MAX_ROWS && editor).then_some(upper)
}

/// How many rows Gemini's footer — the workspace, sandbox and model, under
/// its labels — takes below the input at most.
const GEMINI_FOOTER_MAX_ROWS: usize = 4;

/// Where Gemini's input starts, in whichever of its drawings is on screen.
///
/// - **A shaded block**, the usual one: a row of `▄` and a row of `▀`, as wide
///   as the screen, round the prompt line. A message already sent is drawn the
///   same way in the transcript, so the block counts only with nothing under
///   it but the footer — a permission prompt or a list in the input's place
///   puts itself below the last message instead.
/// - **A rule over the prompt line**, where the block has no colour to shade
///   with (`NO_COLOR`, or its background colour switched off).
/// - **A rounded frame**, `╭ … ╰` with `> ` inside, in versions before the
///   block.
///
/// The prompt line opens with `>`, or with `!` in shell mode, `*` in YOLO
/// mode and `(r:)` while searching the history.
fn gemini_input_top(rows: &[String], floor: usize, width: usize) -> Option<usize> {
    let prompt = |row: &str| {
        let t = row.trim_start();
        ['>', '!', '*'].iter().any(|p| t.starts_with(*p)) || t.starts_with("(r:)")
    };
    let band = |row: &str, c: char| {
        let t = row.trim();
        t.chars().count() >= width * 3 / 5 && t.chars().all(|x| x == c)
    };

    // Shading is on for the whole screen or off for all of it, so a band
    // anywhere says which drawing to look for.
    if let Some(lower) = (floor..rows.len()).rev().find(|&i| band(&rows[i], '▀')) {
        let footer = rows[lower + 1..]
            .iter()
            .filter(|r| !r.trim().is_empty())
            .count();
        let upper = (floor..lower).rev().find(|&i| band(&rows[i], '▄'))?;
        return (footer <= GEMINI_FOOTER_MAX_ROWS && upper + 1 < lower && prompt(&rows[upper + 1]))
            .then_some(upper);
    }

    if let Some(line) = (floor + 1..rows.len())
        .rev()
        .find(|&i| prompt(&rows[i]) && is_rule(&rows[i - 1], width))
    {
        return Some(line - 1);
    }

    let bottom = (floor..rows.len())
        .rev()
        .find(|&i| rows[i].trim_start().starts_with('╰'))?;
    let top = (floor..bottom)
        .rev()
        .find(|&i| rows[i].trim_start().starts_with('╭'))?;
    (top + 1..bottom)
        .any(|i| {
            rows[i]
                .trim_start()
                .trim_start_matches('│')
                .trim_start()
                .starts_with('>')
        })
        .then_some(top)
}

/// How far above Codex's selected row the rest of its list can start: the
/// options before it, each with a description that may wrap.
const CODEX_LIST_REACH: usize = 16;

/// Whether `text` opens the way a numbered list option does: `2. Skip`.
fn numbered_option(text: &str) -> bool {
    let text = text.trim_start();
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && text[digits..].starts_with(". ")
}

/// Claude Code's permission mode, off the status rows under its input. The
/// default mode is the one it does not name.
fn claude_mode(status_rows: &[String]) -> &'static str {
    let text = status_rows.join(" ").to_lowercase();
    [
        ("bypass permissions on", "bypassPermissions"),
        ("accept edits on", "acceptEdits"),
        ("plan mode on", "plan"),
        ("auto mode on", "auto"),
    ]
    .iter()
    .find(|(said, _)| text.contains(said))
    .map_or("default", |(_, mode)| mode)
}

/// The background the agent paints the screen row `line` in: the colour most
/// of its cells have — an inverse cell's being its foreground — or `None` for
/// the terminal's own background.
pub(super) fn row_backdrop<T: EventListener>(
    term: &Term<T>,
    line: usize,
    palette: &[Rgb; 256],
) -> Option<Rgb> {
    let grid = term.grid();
    if line >= grid.screen_lines() {
        return None;
    }
    let row = &grid[Line(line as i32)];
    let mut tally: Vec<(Option<Rgb>, usize)> = Vec::new();
    for c in 0..grid.columns() {
        let cell = &row[Column(c)];
        let color = match cell.flags.contains(Flags::INVERSE) {
            true => cell.fg,
            false => cell.bg,
        };
        let rgb = match color {
            AnsiColor::Spec(rgb) => Some(rgb),
            AnsiColor::Indexed(i) => Some(palette[i as usize]),
            AnsiColor::Named(named) => palette.get(named as usize).copied(),
        };
        match tally.iter_mut().find(|(seen, _)| *seen == rgb) {
            Some((_, n)) => *n += 1,
            None => tally.push((rgb, 1)),
        }
    }
    tally
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .and_then(|(rgb, _)| rgb)
}

/// The screen as text, one string per line, wide characters' spacer cells
/// left out. Reuses `rows` and each inner `String`'s allocation across scans.
pub(super) fn fill_screen_rows<T: EventListener>(term: &Term<T>, rows: &mut Vec<String>) {
    let grid = term.grid();
    let lines = grid.screen_lines();
    let cols = grid.columns();
    rows.resize_with(lines, String::new);
    rows.truncate(lines);
    for (l, dst) in rows.iter_mut().enumerate() {
        dst.clear();
        if dst.capacity() < cols {
            dst.reserve(cols - dst.len());
        }
        let row = &grid[Line(l as i32)];
        for c in 0..cols {
            let cell = &row[Column(c)];
            if !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                dst.push(cell.c);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Menus

/// A `/` or `@` being typed at the caret: which one, where it starts, and what
/// follows it so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Trigger {
    pub sigil: char,
    /// Byte offset of the sigil.
    pub start: usize,
    pub query: String,
}

/// The word the caret is at the end of, when it opens with `/` or `@`.
///
/// A `/` counts only as the first thing in the message: that is the only
/// place an agent reads a slash command, and anywhere else it is a path or a
/// fraction. An `@` counts at the start of any word.
pub(crate) fn trigger_at(text: &str, cursor: usize) -> Option<Trigger> {
    let before = text.get(..cursor)?;
    let start = before
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(i, c)| i + c.len_utf8());
    let word = &before[start..];
    let sigil = word.chars().next()?;
    let valid = match sigil {
        '/' => before[..start].trim().is_empty(),
        '@' => true,
        _ => false,
    };
    valid.then(|| Trigger {
        sigil,
        start,
        query: word[1..].to_string(),
    })
}

/// One row of the `/` or `@` menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MenuItem {
    pub label: String,
    pub detail: String,
    /// What replaces the trigger word when the row is picked.
    pub insert: String,
    pub is_file: bool,
}

/// The agent's own commands, the ones it ships with. Descriptions stay in the
/// agent's language — they name what its UI will do, in the words it uses.
pub(super) fn builtin_commands(agent: CLIAgent) -> &'static [(&'static str, &'static str)] {
    match agent {
        CLIAgent::Claude => &[
            ("/compact", "Summarize and free up context"),
            ("/clear", "Start a fresh conversation"),
            ("/review", "Review the current changes"),
            ("/model", "Switch model"),
            ("/effort", "Set reasoning effort"),
            ("/init", "Create a CLAUDE.md for this repo"),
            ("/context", "Show context usage"),
            ("/usage", "Show cost and plan usage"),
            ("/resume", "Resume a previous conversation"),
            ("/rewind", "Rewind the conversation or code"),
            ("/memory", "Edit memory files"),
            ("/permissions", "Manage tool permissions"),
            ("/mcp", "Manage MCP servers"),
            ("/config", "Open settings"),
            ("/help", "Show help"),
        ],
        CLIAgent::Codex => &[
            ("/model", "Choose model and reasoning effort"),
            ("/permissions", "Choose what Codex is allowed to do"),
            ("/review", "Review the current changes"),
            ("/new", "Start a new chat"),
            ("/resume", "Resume a saved chat"),
            ("/compact", "Summarize to free up context"),
            ("/init", "Create an AGENTS.md for this repo"),
            ("/diff", "Show the git diff"),
            ("/mention", "Mention a file"),
            ("/status", "Show session configuration and usage"),
            ("/mcp", "List MCP tools"),
        ],
        CLIAgent::Gemini => &[
            ("/compress", "Summarize to free up context"),
            ("/clear", "Clear the screen and start a new session"),
            ("/model", "Choose the model"),
            ("/resume", "Browse and resume saved conversations"),
            ("/init", "Create a GEMINI.md for this repo"),
            ("/memory", "Manage memory"),
            ("/tools", "List available tools"),
            ("/mcp", "List MCP servers"),
            ("/stats", "Show session statistics"),
            ("/help", "Show help"),
        ],
        CLIAgent::OpenCode => &[
            ("/new", "New session"),
            ("/models", "Switch model"),
            ("/agents", "Switch agent"),
            ("/sessions", "Switch session"),
            ("/compact", "Compact session"),
            ("/undo", "Undo previous message"),
            ("/review", "Review changes"),
            ("/init", "Guided AGENTS.md setup"),
            ("/share", "Share session"),
            ("/status", "View status"),
            ("/mcps", "Toggle MCPs"),
            ("/help", "Help"),
        ],
        CLIAgent::Copilot => &[
            ("/model", "Switch the model"),
            ("/clear", "Abandon this session and start fresh"),
            ("/compact", "Summarize the conversation to free up context"),
            ("/context", "Show context window usage"),
            ("/resume", "Switch to another session"),
            ("/review", "Review the current changes"),
            ("/diff", "Review the changes in the current directory"),
            ("/init", "Create Copilot instructions for this repo"),
            ("/usage", "Show session usage metrics"),
            ("/permissions", "Choose how requests are approved"),
            ("/agent", "Browse and select agents"),
            ("/mcp", "Manage MCP servers"),
            ("/help", "Show help for interactive commands"),
        ],
        CLIAgent::Qwen => &[
            ("/compress", "Summarize to free up context"),
            ("/clear", "Clear conversation history and free up context"),
            ("/model", "Switch the model"),
            ("/resume", "Resume a previous session"),
            ("/init", "Create a QWEN.md for this repo"),
            ("/approval-mode", "Change the approval mode for tools"),
            ("/tools", "List available tools"),
            ("/mcp", "Manage MCP servers"),
            ("/stats", "Show usage statistics"),
            ("/help", "Show help"),
        ],
        CLIAgent::CodeBuddy => &[
            ("/compact", "Compress context"),
            ("/clear", "Start a new conversation"),
            ("/model", "Switch or view the main model"),
            ("/resume", "Resume a previous session"),
            ("/rewind", "Rewind the conversation or code"),
            ("/init", "Create a CODEBUDDY.md for this repo"),
            ("/context", "Show context usage"),
            ("/cost", "Show session cost and token usage"),
            ("/memory", "Manage long-term memory"),
            ("/permissions", "Manage tool permissions"),
            ("/mcp", "Manage MCP connections"),
            ("/status", "Show repository and session status"),
            ("/help", "Show help"),
        ],
        CLIAgent::Kimi => &[
            ("/new", "Start a fresh session"),
            ("/compact", "Compact the conversation context"),
            ("/model", "Switch LLM model"),
            ("/effort", "Switch thinking effort"),
            ("/sessions", "Browse and resume sessions"),
            ("/undo", "Withdraw the last prompt"),
            ("/init", "Analyze the codebase and generate AGENTS.md"),
            ("/plan", "Toggle plan mode"),
            ("/permission", "Select permission mode"),
            ("/usage", "Show session tokens and context window"),
            ("/status", "Show session and runtime status"),
            ("/mcp", "Show MCP server status"),
            ("/help", "Show available commands and shortcuts"),
        ],
        CLIAgent::Pi => &[
            ("/model", "Select model"),
            ("/thinking", "Set thinking level"),
            ("/compact", "Compact the session context"),
            ("/new", "Start a new session"),
            ("/resume", "Resume a different session"),
            ("/tree", "Navigate the session tree"),
            ("/fork", "Fork from a previous message"),
            ("/session", "Show session info and stats"),
            ("/copy", "Copy the last agent message"),
            ("/export", "Export the session"),
            ("/settings", "Open settings"),
            ("/reload", "Reload extensions, skills and prompts"),
            ("/hotkeys", "Show keyboard shortcuts"),
            ("/quit", "Quit Pi"),
        ],
        CLIAgent::OhMyPi => &[
            ("/model", "Switch model"),
            ("/plan", "Toggle plan mode"),
            ("/compact", "Compact the session context"),
            ("/new", "Start a new session"),
            ("/resume", "Resume a different session"),
            ("/retry", "Retry the last failed turn"),
            ("/tree", "Navigate the session tree"),
            ("/fork", "Fork from a previous message"),
            ("/context", "Show context usage"),
            ("/usage", "Show provider usage and limits"),
            ("/todo", "View or change the todo list"),
            ("/copy", "Copy text from the conversation"),
            ("/settings", "Open settings"),
            ("/exit", "Exit Oh My Pi"),
        ],
        CLIAgent::PrimeAgent => &[
            ("/model", "Switch model"),
            ("/effort", "Set reasoning effort"),
            ("/compact", "Compact the session context"),
            ("/new", "Start a new session"),
            ("/resume", "Resume a session"),
            ("/tree", "Jump to any point in the session"),
            ("/fork", "Fork from a previous message"),
            ("/usage", "Show token, cost and context usage"),
            ("/btw", "Ask a side question"),
            ("/copy", "Copy the last assistant message"),
            ("/export", "Export the session to HTML"),
            ("/settings", "Open settings"),
            ("/reload", "Reload extensions, skills and prompts"),
            ("/quit", "Quit Prime Agent"),
        ],
        CLIAgent::Grok => &[
            ("/model", "Switch the active model"),
            ("/effort", "Set reasoning effort"),
            ("/plan", "Enter plan mode"),
            ("/compact", "Compact conversation history"),
            ("/new", "Start a new session"),
            ("/resume", "Resume a previous session"),
            ("/context", "View context usage"),
            ("/usage", "View usage"),
            ("/btw", "Ask a side question"),
            ("/recap", "Summarize the session so far"),
            ("/copy", "Copy the last response"),
            ("/mcps", "Show MCP server status"),
            ("/settings", "Open settings"),
            ("/quit", "Quit Grok Build"),
        ],
        CLIAgent::Goose => &[
            ("/model", "Show or switch the model"),
            ("/mode", "Set the mode: auto, approve, smart_approve, chat"),
            ("/compact", "Compact the conversation"),
            ("/clear", "Clear the chat history"),
            ("/new", "Start a fresh session"),
            ("/status", "Show model, provider, mode and token usage"),
            ("/skills", "List or enable skills"),
            ("/prompts", "List available prompts"),
            ("/goal", "Set a goal to satisfy before finishing"),
            ("/doctor", "Check the Goose setup"),
            ("/help", "Show the commands"),
            ("/exit", "Exit the session"),
        ],
        CLIAgent::QoderCLI | CLIAgent::QoderCLICn => &[
            ("/compact", "Replace the context with a summary"),
            ("/clear", "Start a new conversation"),
            ("/model", "Set or manage the model"),
            ("/effort", "Set reasoning effort"),
            ("/resume", "Resume a previous session"),
            ("/rewind", "Rewind the conversation"),
            ("/init", "Create a context file for this repo"),
            ("/context", "Show context usage"),
            ("/plan", "Toggle plan mode"),
            ("/permissions", "Manage permissions"),
            ("/review", "Review code changes"),
            ("/memory", "Manage memory"),
            ("/mcp", "Manage MCP servers"),
            ("/status", "Show account and session status"),
            ("/help", "Show help"),
        ],
        // Amp and Crush have no typed commands: `/` on an empty input opens
        // their command palette, which a message cannot drive.
        _ => &[],
    }
}

/// Custom slash commands Claude Code reads from `.claude/commands` — the
/// project's under `cwd`, the user's under `home`. A file's name is its
/// command; subdirectories only namespace the description.
pub(super) fn custom_commands(
    host: &dyn Host,
    cwd: Option<&Path>,
    home: Option<&Path>,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let commands = |root: &Path| host.join(&host.join(root, ".claude"), "commands");
    let roots = [
        (cwd.map(commands), L10nKey::ComposerCmdProject),
        (home.map(commands), L10nKey::ComposerCmdUser),
    ];
    // Directories are followed through symlinks, as Claude Code follows
    // them; each is read once, so a link back up the tree ends the walk
    // instead of looping it.
    let mut seen = std::collections::HashSet::new();
    for (dir, scope) in roots {
        let Some(dir) = dir else { continue };
        let mut stack = vec![dir];
        while let Some(dir) = stack.pop() {
            if !host.canonicalize(&dir).is_ok_and(|real| seen.insert(real)) {
                continue;
            }
            let Ok(entries) = host.read_dir(&dir, None) else {
                continue;
            };
            for entry in entries {
                if entry.is_dir {
                    stack.push(host.join(&dir, &entry.name));
                } else if let Some(stem) = entry.name.strip_suffix(".md").filter(|s| !s.is_empty())
                {
                    let name = format!("/{stem}");
                    if !out.iter().any(|(n, _)| *n == name) {
                        out.push((name, t(scope).to_string()));
                    }
                }
            }
        }
    }
    out.sort();
    out
}

/// The `/` menu for `query`: names starting with it first, in the order the
/// agent lists them, then names that merely contain it.
pub(crate) fn command_items(commands: &[(String, String)], query: &str) -> Vec<MenuItem> {
    let q = query.to_lowercase();
    let starts = commands
        .iter()
        .filter(|(n, _)| n[1..].to_lowercase().starts_with(&q));
    let contains = commands.iter().filter(|(n, _)| {
        let n = n[1..].to_lowercase();
        !n.starts_with(&q) && n.contains(&q)
    });
    starts
        .chain(contains)
        .take(MENU_ROWS)
        .map(|(name, detail)| MenuItem {
            label: name.clone(),
            detail: detail.clone(),
            insert: name.clone(),
            is_file: false,
        })
        .collect()
}

/// The `@` menu for `query`. The mention is spelled relative to `cwd` — which
/// is what the agent resolves it against — and in full for a file outside it.
pub(super) fn file_items(
    index: &FileIndex,
    query: &str,
    cwd: Option<&Path>,
    agent: Option<CLIAgent>,
    shell: Option<&str>,
) -> Vec<MenuItem> {
    let spell = |path: &Path| -> String {
        cwd.and_then(|c| path.strip_prefix(c).ok())
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned()
    };
    let item = |f: &IndexedFile| MenuItem {
        label: f.name().to_string(),
        detail: f.dir().to_string(),
        insert: mention(agent, &spell(&f.path), shell),
        is_file: true,
    };
    match query.is_empty() {
        // The walk is breadth-first, so its head is the top of the project.
        true => index.files.iter().take(MENU_ROWS).map(item).collect(),
        false => rank(index, query, MENU_ROWS)
            .into_iter()
            .map(|(_, f)| item(f))
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// The toolbar's readings

/// A model id the way a person says it: `claude-opus-5-5` is Opus 5.5.
/// Anything that is not a Claude id is shown as the agent spelled it.
pub(crate) fn model_label(id: &str) -> String {
    let id = id.trim_end_matches("[1m]");
    let Some(rest) = id.strip_prefix("claude-") else {
        return id.to_string();
    };
    let mut parts: Vec<&str> = rest.split('-').collect();
    if parts
        .last()
        .is_some_and(|p| p.len() == 8 && p.bytes().all(|b| b.is_ascii_digit()))
    {
        parts.pop();
    }
    let Some((family, version)) = parts.split_first() else {
        return id.to_string();
    };
    let mut name: String = family
        .chars()
        .enumerate()
        .map(|(i, c)| if i == 0 { c.to_ascii_uppercase() } else { c })
        .collect();
    if !version.is_empty() {
        name.push(' ');
        name.push_str(&version.join("."));
    }
    name
}

pub(super) fn mode_label(mode: &str) -> String {
    match mode {
        "default" => t(L10nKey::ComposerModeDefault).to_string(),
        "acceptEdits" => t(L10nKey::ComposerModeAcceptEdits).to_string(),
        "plan" => t(L10nKey::ComposerModePlan).to_string(),
        "bypassPermissions" => t(L10nKey::ComposerModeBypass).to_string(),
        "auto" => t(L10nKey::ComposerModeAuto).to_string(),
        other => other.to_string(),
    }
}

/// Which of [`CLAUDE_MODELS`] a reported model is: its family, and the 1M
/// variant when its id says so.
pub(super) fn model_alias(model: &str) -> Option<&'static str> {
    let long = model.contains("[1m]");
    let pair = if model.contains("opus") {
        ["opus", "opus[1m]"]
    } else if model.contains("sonnet") {
        ["sonnet", "sonnet[1m]"]
    } else if model.contains("fable") {
        return Some("fable");
    } else if model.contains("haiku") {
        return Some("haiku");
    } else {
        return None;
    };
    Some(pair[long as usize])
}

/// How the toolbar spells an effort level: capitalized, as the model names
/// and modes are.
pub(super) fn effort_label(level: &str) -> String {
    match level {
        "xhigh" => "Extra high".into(),
        _ => capitalized(level),
    }
}

/// `text` with its first character upper-cased. A level is whatever the
/// environment or the settings file said, so that character need not be one
/// byte.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// How the model list spells an alias.
pub(super) fn alias_label(alias: &str) -> String {
    if alias == "default" {
        return t(L10nKey::ComposerModelDefault).to_string();
    }
    let (family, long) = match alias.strip_suffix("[1m]") {
        Some(family) => (family, true),
        None => (alias, false),
    };
    let mut name = capitalized(family);
    if long {
        name.push_str(" · 1M");
    }
    name
}
