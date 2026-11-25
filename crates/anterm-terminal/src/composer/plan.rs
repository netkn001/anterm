//! Submission plans, `@` mention spelling/splitting, and input-clearing key
//! sequences for each coding agent.

use std::time::Duration;

use anterm_core::cli_agent::CLIAgent;
use anterm_core::shell_quote::{Quoting, quoting_for};
use crate::view::{pasted_paths_text, types_cleanly};

/// The pause between two writes that must not arrive as one read.
///
/// An agent's input layer tells typing from pasting by how bytes arrive, and a
/// CR landing in the same read as the text before it is taken as part of that
/// text — a newline in the message rather than the key that sends it. Long
/// enough to put the two in separate reads on a loaded machine, short enough
/// to be inside the time a key press takes to feel instant.
pub(super) const SETTLE: Duration = Duration::from_millis(50);

/// Between keys an agent must read one at a time: apart enough that each
/// arrives as a read of its own.
pub(super) const KEY_GAP: Duration = Duration::from_millis(15);

/// Copilot's input treats a CR that follows a paste too closely as part of it.
/// So does Gemini's: it takes an Enter within 40ms of a paste it has finished
/// reading for a line break — and a long paste takes it a while to read, so
/// [`SETTLE`] after the write is not 40ms after that.
pub(super) const SETTLE_AFTER_PASTE_SLOW: Duration = Duration::from_millis(300);

/// How long OpenCode's `@` list takes to search the project for a query.
pub(super) const OPENCODE_LIST_WAIT: Duration = Duration::from_millis(700);

/// Whether `agent` reads its input the way Gemini CLI does. Qwen Code and
/// Qoder (both builds) are forks of it and kept its input: the burst-typing
/// traps, the shell-mode `!`, the `@` list that takes Enter, the Enter right
/// behind a paste taken for a newline, and `@` mentions escaped the same way.
fn reads_like_gemini(agent: CLIAgent) -> bool {
    matches!(
        agent,
        CLIAgent::Gemini | CLIAgent::Qwen | CLIAgent::QoderCLI | CLIAgent::QoderCLICn
    )
}

/// One write of a submission, and how long to wait before making it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Step {
    pub delay: Duration,
    pub bytes: Vec<u8>,
}

/// The writes that hand `text` to `agent` and press Enter on it.
///
/// - **The text and the Enter are separate writes**, [`SETTLE`] apart. In one
///   write, the CR is part of the text as far as the agent can tell.
/// - **A single plain line is typed**, not pasted: it goes in the way the
///   keyboard would have sent it, so the agent treats it like typing — a `/`
///   command opens its menu, nothing is folded into a "pasted text"
///   placeholder. Anything with a line break, a tab or other control
///   character, or past [`types_cleanly`]'s bound goes as one bracketed paste,
///   so the lines are the message's rather than a series of Enters.
/// - **Codex is always pasted.** It watches for bursts of fast keystrokes to
///   spot pastes from terminals that do not bracket them, and the Enter after
///   a typed burst is swallowed into it.
/// - **So is Gemini**, and Qwen Code and Qoder with it
///   ([`reads_like_gemini`]). It judges each key of a burst against the
///   input as it was before the burst, so every `?` in a typed line reads as
///   the key that
///   opens its shortcuts on an empty input and is lost, and every `!` as the
///   one that switches it into shell mode — "hi! there" would run `there`.
/// - **So are OpenCode and Amp.** OpenCode reads a typed burst the same way
///   Gemini does — the `!` in "PONG! ok" switches it into shell mode and the
///   line runs as a command — and Amp takes a typed leading `?` for the key
///   that opens its shortcuts, and a typed leading `/` for the one that opens
///   its command palette, with the rest of the burst landing in the input
///   under it and Enter running whatever the palette had on top.
/// - **So is Copilot.** A typed leading `?` is the key that opens its help,
///   and the rest of the line goes out without it. Its shell mode is a
///   leading `!` it reads off a paste as well.
/// - **So is Kimi Code.** It takes keys arriving faster than typing for an
///   unbracketed paste, and the Enter right behind a typed line for one of its
///   newlines: the message sat in its input, never sent. A bracketed paste it
///   knows for one, and reads its `!`, `?` and `/` off it as typed.
/// - **So is Prime Agent.** Its `@` list opens as a typed mention is
///   typed, stays open past the space after it, and takes the Enter: "read
///   @a.txt " went out as "read @a.txt .git/". A paste it reads whole, its
///   `!` and `/` included.
/// - **A leading `!` goes to Claude Code, CodeBuddy, Gemini, Qwen Code and
///   OpenCode on its own.** It
///   switches the input into shell mode only when it is typed into an empty
///   box as a key of its own; arriving with the rest of the line it is just a
///   character. (Amp's shell mode is a leading `$`, which it reads off a paste
///   as well.)
/// - **A message with an `@` mention gets a space after it for Gemini, Qwen
///   Code, Copilot, Oh My Pi and Grok Build.**
///   With the caret at the end of an `@` word it lists matching files, and
///   Enter then takes the list's pick instead of sending — so a message whose
///   attachments close it would sit in the input, under the box, unsent.
///
/// Without bracketed paste switched on, line breaks go as LF: to every agent
/// input that is Ctrl+J, a newline in the message, where a CR would send each
/// line as a message of its own.
pub(crate) fn submit_plan(agent: CLIAgent, text: &str, bracketed: bool) -> Vec<Step> {
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
    // Where the last word starts is a matter of Gemini's escaping and quoting,
    // so any `@` will do: a space after the message costs nothing. Not after a
    // command, though, where it would open the list of its arguments.
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
            true => anterm_core::paste::bracket(body.as_bytes()),
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

/// The message that goes out: what was written, then the attachments as the
/// words a drop onto the terminal would have typed — which is how an agent is
/// pointed at a file or shown an image.
///
/// Gemini (and Qwen Code and Qoder with it) is the exception. It turns a drop
/// into `@` mentions itself, but only a paste that is nothing but paths; after the
/// message's text, a path is just words to it and the file never reaches the
/// model. So its attachments go as the mentions it would have made.
///
/// Copilot and CodeBuddy attach a file only from an `@` mention — CodeBuddy
/// reads it, an image included, into the turn, Copilot tags it — and a path
/// is just words to them. Neither reads a mention back past a space, though,
/// so a path with one still goes as words.
pub(crate) fn compose_message(
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
        // Oh My Pi reads a mentioned file — an image included — into the
        // turn itself; a path is just words to it.
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

/// Whether `agent` takes a file only from a paste that is nothing but its
/// path. OpenCode and Amp turn such a paste of an image into an attached
/// image, and leave a path that follows the message's text as words — the
/// image never reaches the model.
fn attaches_pasted_paths(agent: CLIAgent) -> bool {
    matches!(agent, CLIAgent::OpenCode | CLIAgent::Amp)
}

/// The writes that send `text` with `attached` — [`compose_message`] handed
/// to [`submit_plan`], except for an agent that takes a file only from a
/// paste of its own ([`attaches_pasted_paths`]). That one gets the text, then
/// each path as a paste by itself after a typed space, and Enter once the
/// last of them has had time to land.
pub(crate) fn submit_message(
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
            bytes: anterm_core::paste::bracket(path.as_bytes()),
        });
    }
    steps.extend(enter.map(|enter| Step {
        delay: SETTLE_AFTER_PASTE_SLOW,
        ..enter
    }));
    steps
}

/// One stretch of a message cut at its mentions — see [`mention_pieces`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Piece<'a> {
    Text(&'a str),
    /// The path of an `@` mention, without the `@`.
    Mention(&'a str),
}

/// `text` cut at its `@` mentions of paths that `exists` — an `@` at the
/// start of a word, followed by the longest run of up to a few words that
/// names one, a closing `.`, `,` and the like left out. An `@` that names
/// nothing is left in the text.
pub(crate) fn mention_pieces<'a>(
    text: &'a str,
    exists: impl Fn(&str) -> bool,
) -> Vec<Piece<'a>> {
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
        // Where each of the next few words ends.
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
///
/// OpenCode attaches a file to the message only when it was picked from its
/// own `@` list; a mention typed or pasted is words to it, with or without a
/// space in the path, and the model has to go looking for the file. So each
/// mention goes the way a pick does: a typed `@` opens the list, the path —
/// its spaces left out, as the list would end at one — goes in as the query,
/// and Enter, once the list has had time to search, takes its top match.
/// A space after it closes a list that found nothing, which would take the
/// Enter that sends.
///
/// The `attached` paths follow, each a paste of its own as
/// [`submit_message`] hands them over.
///
/// `None` for a message without mentions, a command or a shell line, which
/// go the way [`submit_message`] sends them.
pub(crate) fn opencode_mention_plan(
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
                // A pick ends in a space of its own.
                let text = match i > 0 {
                    true => text.strip_prefix(' ').unwrap_or(text),
                    false => text,
                };
                if !text.is_empty() {
                    step(
                        &mut steps,
                        SETTLE,
                        anterm_core::paste::bracket(text.as_bytes()),
                    );
                }
            }
            Piece::Mention(path) => {
                let query: String = path.chars().filter(|c| !c.is_whitespace()).collect();
                step(&mut steps, SETTLE, b"@".to_vec());
                step(
                    &mut steps,
                    SETTLE,
                    anterm_core::paste::bracket(query.as_bytes()),
                );
                step(&mut steps, OPENCODE_LIST_WAIT, b"\r".to_vec());
                // What follows closes a list that found nothing when it
                // has a space in it; otherwise a space of its own does.
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
        step(
            &mut steps,
            SETTLE,
            anterm_core::paste::bracket(path.as_bytes()),
        );
    }
    step(&mut steps, SETTLE_AFTER_PASTE_SLOW, b"\r".to_vec());
    Some(steps)
}

/// `path` as an `@` mention `agent` reads back as that one file — spelled the
/// way the agent's own `@` list would have put it in.
///
/// - **Claude Code** ends a bare mention at the first whitespace; a path with
///   a space in it goes in double quotes, `@"my notes.md"`. **Kimi Code**'s
///   list spells one the same way, and so do Pi's and its forks', Oh My Pi
///   and Prime Agent.
/// - **Codex** hands its mentions to the model as they are, and its list puts
///   in the path alone — without the `@`, in double quotes when it has a
///   space in it.
/// - **Gemini CLI** ends a mention at the first unescaped space, and reads it
///   back through its own `escapePath`: a backslash before each character a
///   shell would take, or — on Windows — the whole path in double quotes.
///   **Qwen Code** and **Qoder** keep that spelling, and Qwen Code escapes
///   a comma too: it ends a mention at one.
///
/// The others take the path as it is.
pub(crate) fn mention(
    agent: Option<CLIAgent>,
    path: &str,
    shell: Option<&str>,
) -> String {
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

/// The keys that stop `agent`'s turn. Grok Build's Esc leaves a running
/// turn running; its Ctrl+C stops one, and empties the input when idle.
/// Crush takes a first Esc as the question "press again to cancel", asked in
/// the key help under the box where nobody reads it, and stops on a second
/// one; the two must arrive as separate reads, or they are one key.
pub(super) fn interrupt_keys(agent: Option<CLIAgent>) -> Vec<Step> {
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

/// The keys that empty `agent`'s input of `lines` lines of text, where some
/// do so without doing anything else; none for an input with nothing in it.
///
/// Grok Build's Ctrl+U empties its input whole. The others' empty the
/// caret's line, and a Backspace then steps up into the line above — a line
/// at a time, from the end, where the caret is left. CodeBuddy takes keys
/// that arrive together for a paste and does none of them, so it gets them
/// one at a time.
pub(super) fn clear_input(agent: CLIAgent, lines: usize) -> Option<Vec<Step>> {
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
