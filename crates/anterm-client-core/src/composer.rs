//! Shared pure-Rust coding-agent composer planning: submission write plans,
//! `@` mention spelling/splitting, input-clearing/interrupt sequences, and
//! path quoting helpers shared identically between desktop and mobile.

use std::time::Duration;

use anterm_core::paste;
use anterm_protocol::cli_agent::CLIAgent;
use anterm_protocol::shell_quote::{Quoting, quote_for_shell, quoting_for};

/// The pause between two writes that must not arrive as one read.
pub const SETTLE: Duration = Duration::from_millis(50);

/// Between keys an agent must read one at a time.
pub const KEY_GAP: Duration = Duration::from_millis(15);

/// Settling delay after a bracketed paste for Copilot / Gemini-family agents.
pub const SETTLE_AFTER_PASTE_SLOW: Duration = Duration::from_millis(300);

/// How long OpenCode's `@` list takes to search the project for a query.
pub const OPENCODE_LIST_WAIT: Duration = Duration::from_millis(700);

/// Claude Code model aliases offered in composer pickers.
pub const CLAUDE_MODELS: [&str; 7] = [
    "default",
    "opus",
    "opus[1m]",
    "fable",
    "sonnet",
    "sonnet[1m]",
    "haiku",
];

/// Claude Code effort levels offered in composer pickers.
pub const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// Whether `line` can be typed directly without bracketed paste framing.
pub fn types_cleanly(line: &str) -> bool {
    line.len() <= 512 && !line.chars().any(char::is_control)
}

/// Paths formatted as one line of shell words with a trailing space.
pub fn pasted_paths_text(paths: &[String], shell: Option<&str>) -> String {
    let words: Vec<String> = paths.iter().map(|p| quote_for_shell(p, shell)).collect();
    format!("{} ", words.join(" "))
}

/// Whether `agent` reads its input the way Gemini CLI does.
fn reads_like_gemini(agent: CLIAgent) -> bool {
    matches!(
        agent,
        CLIAgent::Gemini | CLIAgent::Qwen | CLIAgent::QoderCLI | CLIAgent::QoderCLICn
    )
}

/// One write of a submission, and how long to wait before making it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub delay: Duration,
    pub bytes: Vec<u8>,
}

/// The writes that hand `text` to `agent` and press Enter on it.
pub fn submit_plan(agent: CLIAgent, text: &str, bracketed: bool) -> Vec<Step> {
    let clean: String = text
        .replace("\r\n", "\n")
        .chars()
        .filter(|&c| c != '\x1b')
        .map(|c| if c == '\r' { '\n' } else { c })
        .collect();
    let mut body = clean.trim_end();
    let mut steps = Vec::new();
    let mut delay = Duration::ZERO;

    if (matches!(
        agent,
        CLIAgent::Claude | CLIAgent::CodeBuddy | CLIAgent::OpenCode
    ) || reads_like_gemini(agent))
        && let Some(rest) = body.strip_prefix('!')
        && !rest.is_empty()
    {
        steps.push(Step {
            delay,
            bytes: b"!".to_vec(),
        });
        body = rest;
        delay = SETTLE;
    }

    let spaced;
    if (reads_like_gemini(agent)
        || matches!(agent, CLIAgent::Copilot | CLIAgent::OhMyPi | CLIAgent::Grok))
        && body.contains('@')
        && !body.starts_with('/')
    {
        spaced = format!("{body} ");
        body = &spaced;
    }

    let mut pasted = false;
    if !body.is_empty() {
        pasted = bracketed
            && (matches!(
                agent,
                CLIAgent::Codex
                    | CLIAgent::OpenCode
                    | CLIAgent::Amp
                    | CLIAgent::Copilot
                    | CLIAgent::Kimi
                    | CLIAgent::PrimeAgent
            ) || reads_like_gemini(agent)
                || !types_cleanly(body));
        let bytes = match pasted {
            true => paste::bracket(body.as_bytes()),
            false => body.as_bytes().to_vec(),
        };
        steps.push(Step { delay, bytes });
    }

    let enter_delay = match (steps.is_empty(), agent) {
        (true, _) => Duration::ZERO,
        (false, CLIAgent::Copilot) if pasted => SETTLE_AFTER_PASTE_SLOW,
        (false, agent) if pasted && reads_like_gemini(agent) => SETTLE_AFTER_PASTE_SLOW,
        (false, _) => SETTLE,
    };
    steps.push(Step {
        delay: enter_delay,
        bytes: b"\r".to_vec(),
    });
    steps
}

/// The message that goes out: what was written, then attachments formatted per agent.
pub fn compose_message(
    agent: CLIAgent,
    text: &str,
    attached: &[String],
    shell: Option<&str>,
) -> String {
    let text = text.trim_end();
    if attached.is_empty() {
        return text.to_string();
    }
    let words = match agent {
        _ if reads_like_gemini(agent) => attached
            .iter()
            .map(|p| mention(Some(agent), p, shell))
            .collect::<Vec<_>>()
            .join(" "),
        CLIAgent::Copilot | CLIAgent::CodeBuddy => attached
            .iter()
            .map(|p| match p.chars().any(char::is_whitespace) {
                true => pasted_paths_text(std::slice::from_ref(p), shell),
                false => format!("@{p}"),
            })
            .collect::<Vec<_>>()
            .join(" "),
        CLIAgent::OhMyPi => attached
            .iter()
            .map(|p| mention(Some(agent), p, shell))
            .collect::<Vec<_>>()
            .join(" "),
        _ => pasted_paths_text(attached, shell),
    };
    let words = words.trim_end();
    match text.is_empty() {
        true => words.to_string(),
        false => format!("{text} {words}"),
    }
}

fn attaches_pasted_paths(agent: CLIAgent) -> bool {
    matches!(agent, CLIAgent::OpenCode | CLIAgent::Amp)
}

/// The writes that send `text` with `attached` to `agent`.
pub fn submit_message(
    agent: CLIAgent,
    text: &str,
    attached: &[String],
    shell: Option<&str>,
    bracketed: bool,
) -> Vec<Step> {
    if attached.is_empty() || !bracketed || !attaches_pasted_paths(agent) {
        return submit_plan(
            agent,
            &compose_message(agent, text, attached, shell),
            bracketed,
        );
    }
    let mut steps = submit_plan(agent, text, bracketed);
    let enter = steps.pop();
    for path in attached {
        if !steps.is_empty() {
            steps.push(Step {
                delay: SETTLE,
                bytes: b" ".to_vec(),
            });
        }
        let delay = match steps.is_empty() {
            true => Duration::ZERO,
            false => SETTLE,
        };
        steps.push(Step {
            delay,
            bytes: paste::bracket(path.as_bytes()),
        });
    }
    steps.extend(enter.map(|enter| Step {
        delay: SETTLE_AFTER_PASTE_SLOW,
        ..enter
    }));
    steps
}

/// One stretch of a message cut at its mentions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece<'a> {
    Text(&'a str),
    Mention(&'a str),
}

/// `text` cut at its `@` mentions of paths that `exists`.
pub fn mention_pieces<'a>(text: &'a str, exists: impl Fn(&str) -> bool) -> Vec<Piece<'a>> {
    const MAX_WORDS: usize = 4;
    let mut pieces = Vec::new();
    let mut from = 0;
    let mut at = 0;
    while let Some(found) = text[at..].find('@') {
        let sign = at + found;
        at = sign + 1;
        let starts_word = text[..sign]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace);
        if !starts_word {
            continue;
        }
        let rest = &text[sign + 1..];
        let mut ends = Vec::new();
        let mut in_word = false;
        for (i, c) in rest.char_indices() {
            match (c.is_whitespace(), in_word) {
                (true, true) => {
                    ends.push(i);
                    in_word = false;
                    if ends.len() == MAX_WORDS {
                        break;
                    }
                }
                (false, false) => in_word = true,
                _ => {}
            }
        }
        if in_word && ends.len() < MAX_WORDS {
            ends.push(rest.len());
        }
        let path = ends.iter().rev().find_map(|&end| {
            let path = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '"', '\'']);
            (!path.is_empty() && exists(path)).then_some(path)
        });
        if let Some(path) = path {
            if from < sign {
                pieces.push(Piece::Text(&text[from..sign]));
            }
            pieces.push(Piece::Mention(path));
            from = sign + 1 + path.len();
            at = from;
        }
    }
    if from < text.len() {
        pieces.push(Piece::Text(&text[from..]));
    }
    pieces
}

/// The writes that send OpenCode a message whose `pieces` mention files.
pub fn opencode_mention_plan(
    pieces: &[Piece],
    attached: &[String],
    bracketed: bool,
) -> Option<Vec<Step>> {
    let first = match pieces.first()? {
        Piece::Text(t) => t.trim_start(),
        Piece::Mention(_) => "",
    };
    if !bracketed
        || first.starts_with(['!', '/'])
        || !pieces.iter().any(|p| matches!(p, Piece::Mention(_)))
    {
        return None;
    }
    let mut steps: Vec<Step> = Vec::new();
    let step = |steps: &mut Vec<Step>, delay: Duration, bytes: Vec<u8>| {
        let delay = match steps.is_empty() {
            true => Duration::ZERO,
            false => delay,
        };
        steps.push(Step { delay, bytes });
    };
    for (i, piece) in pieces.iter().enumerate() {
        match piece {
            Piece::Text(text) => {
                let text = match i + 1 == pieces.len() {
                    true => text.trim_end(),
                    false => text,
                };
                let text = match i > 0 {
                    true => text.strip_prefix(' ').unwrap_or(text),
                    false => text,
                };
                if !text.is_empty() {
                    step(&mut steps, SETTLE, paste::bracket(text.as_bytes()));
                }
            }
            Piece::Mention(path) => {
                let query: String = path.chars().filter(|c| !c.is_whitespace()).collect();
                step(&mut steps, SETTLE, b"@".to_vec());
                step(&mut steps, SETTLE, paste::bracket(query.as_bytes()));
                step(&mut steps, OPENCODE_LIST_WAIT, b"\r".to_vec());
                let spaced = match pieces.get(i + 1) {
                    Some(Piece::Text(next)) => next
                        .strip_prefix(' ')
                        .unwrap_or(next)
                        .trim_end()
                        .contains(char::is_whitespace),
                    _ => false,
                };
                if !spaced {
                    step(&mut steps, SETTLE, b" ".to_vec());
                }
            }
        }
    }
    for path in attached {
        step(&mut steps, SETTLE, b" ".to_vec());
        step(&mut steps, SETTLE, paste::bracket(path.as_bytes()));
    }
    step(&mut steps, SETTLE_AFTER_PASTE_SLOW, b"\r".to_vec());
    Some(steps)
}

/// `path` spelled as an `@` mention for `agent`.
pub fn mention(agent: Option<CLIAgent>, path: &str, shell: Option<&str>) -> String {
    let spaced = path.chars().any(char::is_whitespace);
    match agent {
        Some(a) if reads_like_gemini(a) => {}
        Some(
            CLIAgent::Claude
            | CLIAgent::Kimi
            | CLIAgent::Pi
            | CLIAgent::OhMyPi
            | CLIAgent::PrimeAgent,
        ) if spaced => return format!("@\"{path}\""),
        Some(CLIAgent::Codex) if spaced && !path.contains('"') => return format!("\"{path}\""),
        Some(CLIAgent::Codex) => return path.to_string(),
        _ => return format!("@{path}"),
    }
    let spelled = match quoting_for(shell) {
        Quoting::Posix => {
            let mut out = String::with_capacity(path.len());
            for c in path.chars() {
                if " \t()[]{};|*?$`'\"#&<>!~\\".contains(c)
                    || (c == ',' && agent == Some(CLIAgent::Qwen))
                {
                    out.push('\\');
                }
                out.push(c);
            }
            out
        }
        _ if path
            .chars()
            .any(|c| c.is_whitespace() || "&()[]{}^=;!'+,`~%$@#".contains(c)) =>
        {
            format!("\"{path}\"")
        }
        _ => path.to_string(),
    };
    format!("@{spelled}")
}

/// The keys that stop `agent`'s turn.
pub fn interrupt_keys(agent: Option<CLIAgent>) -> Vec<Step> {
    let key = |delay, bytes: &[u8]| Step {
        delay,
        bytes: bytes.to_vec(),
    };
    match agent {
        Some(CLIAgent::Grok) => vec![key(Duration::ZERO, b"\x03")],
        Some(CLIAgent::Crush) => vec![key(Duration::ZERO, b"\x1b"), key(SETTLE, b"\x1b")],
        _ => vec![key(Duration::ZERO, b"\x1b")],
    }
}

/// The keys that empty `agent`'s input of `lines` lines of text.
pub fn clear_input(agent: CLIAgent, lines: usize) -> Option<Vec<Step>> {
    if lines == 0 {
        return None;
    }
    let keys = match agent {
        CLIAgent::Grok => b"\x15".to_vec(),
        CLIAgent::Crush
        | CLIAgent::Goose
        | CLIAgent::Amp
        | CLIAgent::Copilot
        | CLIAgent::Qwen
        | CLIAgent::CodeBuddy
        | CLIAgent::Kimi
        | CLIAgent::OpenCode
        | CLIAgent::QoderCLI
        | CLIAgent::QoderCLICn => b"\x15\x7f".repeat(lines),
        _ => return None,
    };
    Some(match agent {
        CLIAgent::CodeBuddy => keys
            .iter()
            .enumerate()
            .map(|(i, &key)| Step {
                delay: match i {
                    0 => Duration::ZERO,
                    _ => KEY_GAP,
                },
                bytes: vec![key],
            })
            .collect(),
        _ => vec![Step {
            delay: Duration::ZERO,
            bytes: keys,
        }],
    })
}

/// Mobile quick-approval action keys mapped cleanly to PTY byte sequences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickApprovalAction {
    ApproveOnce,
    ApproveAlways,
    Deny,
    Choice(u8),
}

/// Byte sequence to send to an agent pane when the user taps a quick-action button
/// on mobile while the agent is in `AgentStatus::Waiting`.
pub fn quick_approval_bytes(agent: Option<CLIAgent>, action: QuickApprovalAction) -> Vec<u8> {
    match action {
        QuickApprovalAction::ApproveOnce => b"\r".to_vec(),
        QuickApprovalAction::ApproveAlways => match agent {
            Some(CLIAgent::Claude) => b"\x1b[B\r".to_vec(),
            _ => b"2\r".to_vec(),
        },
        QuickApprovalAction::Deny => b"\x1b".to_vec(),
        QuickApprovalAction::Choice(n) => {
            if (1..=9).contains(&n) {
                vec![b'0' + n, b'\r']
            } else {
                b"\r".to_vec()
            }
        }
    }
}
