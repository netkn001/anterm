use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::px;

use super::agents::*;
use super::plan::*;
use anterm_core::cli_agent::CLIAgent;

fn bytes(steps: &[Step]) -> Vec<&[u8]> {
    steps.iter().map(|s| s.bytes.as_slice()).collect()
}

/// The writes [`clear_input`] makes, one per step.
fn clear_keys(agent: CLIAgent, lines: usize) -> Option<Vec<Vec<u8>>> {
    clear_input(agent, lines).map(|steps| steps.into_iter().map(|s| s.bytes).collect())
}

#[test]
fn a_plain_line_is_typed_then_entered_separately() {
    let steps = submit_plan(CLIAgent::Claude, "fix the tests", true);
    assert_eq!(bytes(&steps), [&b"fix the tests"[..], b"\r"]);
    assert_eq!(steps[0].delay, Duration::ZERO);
    assert_eq!(steps[1].delay, SETTLE);
}

#[test]
fn several_lines_go_as_one_paste() {
    let steps = submit_plan(CLIAgent::Claude, "one\r\ntwo\n", true);
    assert_eq!(bytes(&steps), [&b"\x1b[200~one\ntwo\x1b[201~"[..], b"\r"]);
}

#[test]
fn without_bracketed_paste_line_breaks_stay_newlines_not_enters() {
    let steps = submit_plan(CLIAgent::Gemini, "one\ntwo", false);
    assert_eq!(bytes(&steps), [&b"one\ntwo"[..], b"\r"]);
}

#[test]
fn codex_is_always_pasted() {
    let steps = submit_plan(CLIAgent::Codex, "hi", true);
    assert_eq!(bytes(&steps), [&b"\x1b[200~hi\x1b[201~"[..], b"\r"]);
}

#[test]
fn a_leading_bang_reaches_claude_and_gemini_as_a_key_of_its_own() {
    for agent in [CLIAgent::Claude, CLIAgent::CodeBuddy] {
        let steps = submit_plan(agent, "!git status", true);
        assert_eq!(
            bytes(&steps),
            [&b"!"[..], b"git status", b"\r"],
            "{agent:?}"
        );
        assert_eq!(steps[1].delay, SETTLE, "{agent:?}");
    }
    let steps = submit_plan(CLIAgent::Gemini, "!git status", true);
    assert_eq!(
        bytes(&steps),
        [&b"!"[..], b"\x1b[200~git status\x1b[201~", b"\r"]
    );
    // Anyone else gets the line as written.
    let steps = submit_plan(CLIAgent::Pi, "!git status", true);
    assert_eq!(bytes(&steps), [&b"!git status"[..], b"\r"]);
}

#[test]
fn kimi_is_always_pasted() {
    // Typed, the Enter behind the line is one of its newlines.
    for text in ["fix it", "?why", "!ls", "/usage"] {
        let steps = submit_plan(CLIAgent::Kimi, text, true);
        let pasted = format!("\x1b[200~{text}\x1b[201~");
        assert_eq!(bytes(&steps), [pasted.as_bytes(), b"\r"], "{text}");
        assert_eq!(steps[1].delay, SETTLE, "{text}");
    }
}

#[test]
fn copilot_is_always_pasted() {
    // Typed, the leading `?` would open its help and be lost; its shell
    // mode takes the `!` off the paste.
    for text in ["?why", "!git status", "hi"] {
        let steps = submit_plan(CLIAgent::Copilot, text, true);
        let pasted = format!("\x1b[200~{text}\x1b[201~");
        assert_eq!(bytes(&steps), [pasted.as_bytes(), b"\r"], "{text}");
        assert_eq!(steps[1].delay, SETTLE_AFTER_PASTE_SLOW, "{text}");
    }
}

#[test]
fn gemini_is_always_pasted() {
    // Typed, the `!` would switch it into shell mode and the `?` would
    // open its shortcuts: each reads the input as empty.
    let steps = submit_plan(CLIAgent::Gemini, "hi! ok?", true);
    assert_eq!(bytes(&steps), [&b"\x1b[200~hi! ok?\x1b[201~"[..], b"\r"]);
}

#[test]
fn opencode_and_amp_are_always_pasted() {
    // Typed, OpenCode would take the `!` for its shell mode, and Amp the
    // leading `?` for its shortcuts and a leading `/` for its palette.
    for (agent, text) in [
        (CLIAgent::OpenCode, "PONG! ok?"),
        (CLIAgent::Amp, "?why"),
        (CLIAgent::Amp, "/new"),
    ] {
        let steps = submit_plan(agent, text, true);
        let pasted = format!("\x1b[200~{text}\x1b[201~");
        assert_eq!(bytes(&steps), [pasted.as_bytes(), b"\r"], "{agent:?}");
        assert_eq!(steps[1].delay, SETTLE, "{agent:?}");
    }
    // A leading `!` is still OpenCode's shell-mode key; Amp's `$` works
    // pasted.
    let steps = submit_plan(CLIAgent::OpenCode, "!ls", true);
    assert_eq!(bytes(&steps), [&b"!"[..], b"\x1b[200~ls\x1b[201~", b"\r"]);
    let steps = submit_plan(CLIAgent::Amp, "$ls", true);
    assert_eq!(bytes(&steps), [&b"\x1b[200~$ls\x1b[201~"[..], b"\r"]);
}

#[test]
fn escapes_cannot_close_the_paste_early() {
    let steps = submit_plan(CLIAgent::Claude, "a\n\x1b[201~b", true);
    assert_eq!(bytes(&steps)[0], b"\x1b[200~a\n[201~b\x1b[201~");
}

#[test]
fn copilot_and_gemini_wait_longer_after_a_paste() {
    for agent in [CLIAgent::Copilot, CLIAgent::Gemini] {
        let steps = submit_plan(agent, "a\nb", true);
        assert_eq!(steps[1].delay, SETTLE_AFTER_PASTE_SLOW, "{agent:?}");
    }
    let steps = submit_plan(CLIAgent::Copilot, "ab", false);
    assert_eq!(steps[1].delay, SETTLE);
}

#[test]
fn attachments_follow_the_text_as_shell_words() {
    let attached = vec!["/tmp/a b.png".to_string(), "/src/x.rs".to_string()];
    let claude = CLIAgent::Claude;
    assert_eq!(
        compose_message(claude, "look at these\n", &attached, Some("zsh")),
        "look at these '/tmp/a b.png' /src/x.rs"
    );
    assert_eq!(
        compose_message(claude, "", &attached[1..], Some("zsh")),
        "/src/x.rs"
    );
    assert_eq!(
        compose_message(claude, "just text  ", &[], Some("zsh")),
        "just text"
    );
}

#[test]
fn opencode_and_amp_get_each_attachment_as_a_paste_of_its_own() {
    let attached = vec!["/tmp/a b.png".to_string(), "/src/x.rs".to_string()];
    for agent in [CLIAgent::OpenCode, CLIAgent::Amp] {
        let steps = submit_message(agent, "look\n", &attached, Some("zsh"), true);
        assert_eq!(
            bytes(&steps),
            [
                &b"\x1b[200~look\x1b[201~"[..],
                b" ",
                b"\x1b[200~/tmp/a b.png\x1b[201~",
                b" ",
                b"\x1b[200~/src/x.rs\x1b[201~",
                b"\r",
            ],
            "{agent:?}"
        );
        assert!(steps[1..].iter().all(|s| s.delay >= SETTLE));
        assert_eq!(steps.last().unwrap().delay, SETTLE_AFTER_PASTE_SLOW);
        // Attachments alone: nothing to space them from.
        let steps = submit_message(agent, "", &attached[..1], None, true);
        assert_eq!(
            bytes(&steps),
            [&b"\x1b[200~/tmp/a b.png\x1b[201~"[..], b"\r"]
        );
        assert_eq!(steps[0].delay, Duration::ZERO);
    }
    // Everyone else still gets them as words after the text.
    let steps = submit_message(CLIAgent::Claude, "look", &attached, Some("zsh"), true);
    assert_eq!(
        bytes(&steps),
        [&b"look '/tmp/a b.png' /src/x.rs"[..], b"\r"]
    );
}

#[test]
fn a_closing_mention_is_left_behind_before_gemini_gets_enter() {
    let steps = submit_plan(CLIAgent::Gemini, r"read @my\ notes.txt", false);
    assert_eq!(bytes(&steps), [&br"read @my\ notes.txt "[..], b"\r"]);
    let steps = submit_plan(CLIAgent::Gemini, r#"read @"C:\My Files\a.rs""#, false);
    assert_eq!(bytes(&steps)[0], br#"read @"C:\My Files\a.rs" "#);
    // Nothing to close without a mention, and a command's space would
    // open its arguments.
    let steps = submit_plan(CLIAgent::Gemini, "read it", false);
    assert_eq!(bytes(&steps), [&b"read it"[..], b"\r"]);
    let steps = submit_plan(CLIAgent::Gemini, "/memory", false);
    assert_eq!(bytes(&steps), [&b"/memory"[..], b"\r"]);
    let steps = submit_plan(CLIAgent::Claude, "read @a.rs", true);
    assert_eq!(bytes(&steps), [&b"read @a.rs"[..], b"\r"]);
    // Copilot's list takes that Enter as well.
    let steps = submit_plan(CLIAgent::Copilot, "read @a.rs", false);
    assert_eq!(bytes(&steps), [&b"read @a.rs "[..], b"\r"]);
}

#[test]
fn gemini_gets_its_attachments_as_mentions() {
    let attached = vec!["/tmp/a b.png".to_string(), "/src/x(1).rs".to_string()];
    assert_eq!(
        compose_message(CLIAgent::Gemini, "look at these", &attached, Some("zsh")),
        r"look at these @/tmp/a\ b.png @/src/x\(1\).rs"
    );
    assert_eq!(
        mention(Some(CLIAgent::Gemini), r"C:\My Files\a.png", Some("pwsh")),
        r#"@"C:\My Files\a.png""#
    );
    assert_eq!(
        mention(Some(CLIAgent::Gemini), r"C:\src\a.rs", Some("pwsh")),
        r"@C:\src\a.rs"
    );
}

#[test]
fn copilot_and_codebuddy_get_their_attachments_as_mentions_where_they_can() {
    let attached = vec!["/tmp/red.png".to_string(), "/tmp/a b.png".to_string()];
    for agent in [CLIAgent::Copilot, CLIAgent::CodeBuddy] {
        assert_eq!(
            compose_message(agent, "look", &attached, Some("zsh")),
            "look @/tmp/red.png '/tmp/a b.png'",
            "{agent:?}"
        );
    }
    // Copilot's list would take the Enter after a closing mention.
    let message = compose_message(CLIAgent::Copilot, "look", &attached[..1], Some("zsh"));
    let steps = submit_plan(CLIAgent::Copilot, &message, true);
    assert_eq!(bytes(&steps)[0], b"\x1b[200~look @/tmp/red.png \x1b[201~");
}

#[test]
fn mentions_close_before_enter_for_oh_my_pi_and_grok_and_prime_is_pasted() {
    // Their `@` list would take the Enter after a closing mention.
    for agent in [CLIAgent::OhMyPi, CLIAgent::Grok] {
        let steps = submit_plan(agent, "read @plain.txt", true);
        assert_eq!(
            bytes(&steps),
            [&b"read @plain.txt "[..], b"\r"],
            "{agent:?}"
        );
        let steps = submit_plan(agent, "/model", true);
        assert_eq!(bytes(&steps), [&b"/model"[..], b"\r"], "{agent:?}");
    }
    // Typed, Prime Agent's list stays open past the space and takes the
    // Enter; pasted, it reads the line whole.
    for text in ["read @plain.txt", "!ls", "/session", "hi"] {
        let steps = submit_plan(CLIAgent::PrimeAgent, text, true);
        let pasted = format!("\x1b[200~{text}\x1b[201~");
        assert_eq!(bytes(&steps), [pasted.as_bytes(), b"\r"], "{text}");
    }
}

/// Oh My Pi reads a mentioned file, an image too, into the turn; a
/// mention with a space goes in its quotes.
#[test]
fn oh_my_pi_gets_its_attachments_as_mentions() {
    let attached = vec!["/tmp/red.png".to_string(), "/tmp/a b.png".to_string()];
    assert_eq!(
        compose_message(CLIAgent::OhMyPi, "look", &attached, Some("zsh")),
        "look @/tmp/red.png @\"/tmp/a b.png\""
    );
    assert_eq!(
        compose_message(CLIAgent::Pi, "look", &attached, Some("zsh")),
        "look /tmp/red.png '/tmp/a b.png'"
    );
}

#[test]
fn qwen_is_spoken_to_the_way_gemini_is() {
    // Typed, its leading `?` opens the shortcuts and is lost.
    let steps = submit_plan(CLIAgent::Qwen, "?why", true);
    assert_eq!(bytes(&steps), [&b"\x1b[200~?why\x1b[201~"[..], b"\r"]);
    assert_eq!(steps[1].delay, SETTLE_AFTER_PASTE_SLOW);
    let steps = submit_plan(CLIAgent::Qwen, "!ls", true);
    assert_eq!(bytes(&steps), [&b"!"[..], b"\x1b[200~ls\x1b[201~", b"\r"]);
    // Its `@` list takes an Enter that comes right after a mention.
    let steps = submit_plan(CLIAgent::Qwen, "read @a.rs", false);
    assert_eq!(bytes(&steps), [&b"read @a.rs "[..], b"\r"]);
    // Mentions as Gemini spells them, with the comma Qwen ends one at.
    assert_eq!(
        mention(Some(CLIAgent::Qwen), "my notes, v2.md", Some("zsh")),
        r"@my\ notes\,\ v2.md"
    );
    assert_eq!(
        mention(Some(CLIAgent::Gemini), "a,b.md", Some("zsh")),
        "@a,b.md"
    );
    let attached = vec!["/tmp/a b.png".to_string()];
    assert_eq!(
        compose_message(CLIAgent::Qwen, "look", &attached, Some("zsh")),
        r"look @/tmp/a\ b.png"
    );
}

#[test]
fn mentions_are_cut_out_where_they_name_a_path() {
    let exists = |p: &str| ["my notes.md", "plain.md", "src/a b/c.rs"].contains(&p);
    assert_eq!(
        mention_pieces("read @my notes.md and @plain.md.", exists),
        [
            Piece::Text("read "),
            Piece::Mention("my notes.md"),
            Piece::Text(" and "),
            Piece::Mention("plain.md"),
            Piece::Text("."),
        ]
    );
    assert_eq!(
        mention_pieces("@src/a b/c.rs", exists),
        [Piece::Mention("src/a b/c.rs")]
    );
    // An `@` inside a word, or naming nothing, stays words.
    for text in ["mail me@plain.md", "ask @someone about it", "@ plain.md"] {
        assert_eq!(mention_pieces(text, exists), [Piece::Text(text)]);
    }
    assert_eq!(mention_pieces("", exists), []);
}

/// OpenCode attaches only what is picked from its `@` list, so each
/// mention is picked there: `@`, the path as the query, Enter.
#[test]
fn opencode_picks_each_mention_from_its_own_list() {
    let pieces = [
        Piece::Text("read "),
        Piece::Mention("my notes.md"),
        Piece::Text(" now, please"),
    ];
    let steps = opencode_mention_plan(&pieces, &[], true).unwrap();
    assert_eq!(
        bytes(&steps),
        [
            &b"\x1b[200~read \x1b[201~"[..],
            b"@",
            b"\x1b[200~mynotes.md\x1b[201~",
            b"\r",
            b"\x1b[200~now, please\x1b[201~",
            b"\r",
        ]
    );
    assert_eq!(steps[0].delay, Duration::ZERO);
    assert_eq!(steps[3].delay, OPENCODE_LIST_WAIT);
    assert_eq!(steps[5].delay, SETTLE_AFTER_PASTE_SLOW);
    // With no space in what follows, one of its own closes the list.
    let steps =
        opencode_mention_plan(&[Piece::Mention("a.md"), Piece::Text(".")], &[], true).unwrap();
    assert_eq!(
        bytes(&steps),
        [
            &b"@"[..],
            b"\x1b[200~a.md\x1b[201~",
            b"\r",
            b" ",
            b"\x1b[200~.\x1b[201~",
            b"\r",
        ]
    );
    // Ending on one, a space closes a list that found nothing, and
    // attachments still follow as pastes of their own.
    let attached = vec!["/tmp/a.png".to_string()];
    let steps = opencode_mention_plan(&[Piece::Mention("plain.md")], &attached, true).unwrap();
    assert_eq!(
        bytes(&steps),
        [
            &b"@"[..],
            b"\x1b[200~plain.md\x1b[201~",
            b"\r",
            b" ",
            b" ",
            b"\x1b[200~/tmp/a.png\x1b[201~",
            b"\r",
        ]
    );
    // Without a mention, or as a command or shell line, it goes as usual.
    assert_eq!(opencode_mention_plan(&[Piece::Text("hi")], &[], true), None);
    assert_eq!(opencode_mention_plan(&pieces, &[], false), None);
    let command = [Piece::Text("/review "), Piece::Mention("plain.md")];
    assert_eq!(opencode_mention_plan(&command, &[], true), None);
}

#[test]
fn qoder_is_spoken_to_the_way_gemini_is() {
    for agent in [CLIAgent::QoderCLI, CLIAgent::QoderCLICn] {
        // Typed, its leading `?` opens the shortcuts and is lost, and an
        // Enter right behind a paste is a newline.
        let steps = submit_plan(agent, "?why", true);
        assert_eq!(bytes(&steps), [&b"\x1b[200~?why\x1b[201~"[..], b"\r"]);
        assert_eq!(steps[1].delay, SETTLE_AFTER_PASTE_SLOW);
        let steps = submit_plan(agent, "!ls", true);
        assert_eq!(bytes(&steps), [&b"!"[..], b"\x1b[200~ls\x1b[201~", b"\r"]);
        // Its `@` list takes an Enter that comes right after a mention.
        let steps = submit_plan(agent, "read @a.rs", true);
        assert_eq!(
            bytes(&steps),
            [&b"\x1b[200~read @a.rs \x1b[201~"[..], b"\r"]
        );
        // Its list puts a path with a space in as `@my\ notes.md`.
        assert_eq!(
            mention(Some(agent), "my notes.md", Some("zsh")),
            r"@my\ notes.md"
        );
    }
}

#[test]
fn a_mention_with_a_space_is_spelled_the_way_each_agent_reads_it() {
    let zsh = Some("zsh");
    assert_eq!(
        mention(Some(CLIAgent::Claude), "my notes.md", zsh),
        r#"@"my notes.md""#
    );
    assert_eq!(
        mention(Some(CLIAgent::Claude), "src/a.rs", zsh),
        "@src/a.rs"
    );
    assert_eq!(
        mention(Some(CLIAgent::Codex), "my notes.md", zsh),
        r#""my notes.md""#
    );
    assert_eq!(mention(Some(CLIAgent::Codex), "src/a.rs", zsh), "src/a.rs");
    assert_eq!(
        mention(Some(CLIAgent::Gemini), "my notes.md", zsh),
        r"@my\ notes.md"
    );
    assert_eq!(
        mention(Some(CLIAgent::Kimi), "my notes.md", zsh),
        r#"@"my notes.md""#
    );
    assert_eq!(mention(Some(CLIAgent::Kimi), "src/a.rs", zsh), "@src/a.rs");
    for agent in [CLIAgent::Pi, CLIAgent::OhMyPi, CLIAgent::PrimeAgent] {
        assert_eq!(
            mention(Some(agent), "my notes.md", zsh),
            "@\"my notes.md\"",
            "{agent:?}"
        );
        assert_eq!(mention(Some(agent), "src/a.rs", zsh), "@src/a.rs");
    }
    // Grok Build's own list puts the path in as it is.
    assert_eq!(
        mention(Some(CLIAgent::Grok), "my notes.md", zsh),
        "@my notes.md"
    );
    assert_eq!(mention(None, "my notes.md", zsh), "@my notes.md");
}

fn screen(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|l| l.to_string()).collect()
}

const RULE: &str = "────────────────────────────────────────";

#[test]
fn claudes_input_is_the_ruled_block_with_the_prompt_in_it() {
    let rows = screen(&[
        "⏺ Done. The tests pass.",
        "",
        RULE,
        "❯ Try \"fix lint errors\"",
        RULE,
        "  ⚠ Transcript saving is off",
        "  ►► bypass permissions on (shift+tab to cycle)",
    ]);
    assert_eq!(
        input_area(CLIAgent::Claude, &rows, 40),
        Some(InputArea {
            top: 2,
            mode: Some("bypassPermissions")
        })
    );
}

#[test]
fn claudes_default_mode_is_the_one_it_does_not_name() {
    let rows = screen(&[RULE, "❯ ", RULE, "  ? for shortcuts"]);
    assert_eq!(
        input_area(CLIAgent::Claude, &rows, 40).and_then(|a| a.mode),
        Some("default")
    );
    let rows = screen(&[RULE, "❯ ", RULE, "  ⏸ plan mode on (shift+tab to cycle)"]);
    assert_eq!(
        input_area(CLIAgent::Claude, &rows, 40).and_then(|a| a.mode),
        Some("plan")
    );
}

/// A permission prompt or a picker takes the input's place: no prompt
/// line between the rules, so no input area, so the box steps aside.
#[test]
fn claude_asking_something_is_not_an_input_area() {
    let rows = screen(&[
        RULE,
        " Do you want to make this edit to route.py?",
        " ❯ 1. Yes",
        "   2. No",
    ]);
    assert_eq!(input_area(CLIAgent::Claude, &rows, 40), None);
    let rows = screen(&["some output", "", "  ────── a short rule ──"]);
    assert_eq!(input_area(CLIAgent::Claude, &rows, 40), None);
}

#[test]
fn codex_input_starts_on_its_prompt_line_or_the_padding_above_it() {
    let rows = screen(&[
        "• Ran tests",
        "",
        "› Ask Codex to do anything",
        "",
        "  ⏎ send",
    ]);
    assert_eq!(
        input_area(CLIAgent::Codex, &rows, 40),
        Some(InputArea { top: 1, mode: None })
    );
}

#[test]
fn codex_lists_are_not_its_input() {
    let lists = [
        // The hook review at launch.
        &[
            "  Hooks need review",
            "",
            "› 1. Review hooks",
            "  2. Trust all and continue",
            "  3. Continue without trusting (hooks won't run)",
            "",
            "  Press enter to confirm or esc to go back",
        ][..],
        // `/permissions`, its descriptions wrapping, the last one picked.
        &[
            "  Update Model Permissions",
            "  1. Ask for approval (current)  Read and edit workspace files",
            "                                 required for internet access",
            "  2. Approve for me              Only ask for actions",
            "› 3. Full Access                 Use with caution: Codex can",
            "                                 and access the internet",
            "  enter select · esc back",
        ][..],
        // A command waiting for approval.
        &[
            "  Would you like to run the following command?",
            "  $ touch a.txt",
            "› 1. Yes, proceed (y)",
            "  2. Yes, and don't ask again for these files (p)",
            "  3. No, and tell Codex what to do differently (esc)",
        ][..],
    ];
    for rows in lists {
        assert_eq!(input_area(CLIAgent::Codex, &screen(rows), 40), None);
    }
    // A message of the user's own that happens to open like an option.
    let rows = screen(&["• Done.", "", "› 1. fix the build", "", "  ⏎ send"]);
    assert_eq!(
        input_area(CLIAgent::Codex, &rows, 40),
        Some(InputArea { top: 1, mode: None })
    );
}

#[test]
fn geminis_input_is_its_framed_prompt() {
    let rows = screen(&[
        "✦ Here you go.",
        "╭──────────────────────╮",
        "│ >   Type your message │",
        "╰──────────────────────╯",
        "~/code  (main)  gemini-2.5-pro",
    ]);
    assert_eq!(
        input_area(CLIAgent::Gemini, &rows, 40),
        Some(InputArea { top: 1, mode: None })
    );
}

const LOWER_HALVES: &str = "▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄";
const UPPER_HALVES: &str = "▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀";

#[test]
fn geminis_input_is_its_shaded_block_over_the_footer() {
    let rows = screen(&[
        LOWER_HALVES,
        " > say pong",
        UPPER_HALVES,
        "",
        "✦ pong",
        "",
        "? for shortcuts",
        RULE,
        " Shift+Tab to accept edits",
        LOWER_HALVES,
        " >   Type your message or @path/to/file",
        UPPER_HALVES,
        " workspace (/directory)   sandbox   /model",
        " ~/proj                   no sandbox gpt-5",
    ]);
    // The box covers its mode line, the rule over it and the hint too.
    assert_eq!(
        input_area(CLIAgent::Gemini, &rows, 40),
        Some(InputArea { top: 6, mode: None })
    );
    // Shell mode and YOLO mode change only the prompt's mark.
    for mark in [" ! ls", " * fix it"] {
        let mut rows = rows.clone();
        rows[10] = mark.to_string();
        assert_eq!(
            input_area(CLIAgent::Gemini, &rows, 40).map(|a| a.top),
            Some(6)
        );
    }
    // Without a footer, too.
    assert_eq!(
        input_area(CLIAgent::Gemini, &rows[..12], 40).map(|a| a.top),
        Some(6)
    );
    // Without a mode line, from the shaded block's top edge.
    let mut bare = rows.clone();
    bare.drain(6..9);
    assert_eq!(
        input_area(CLIAgent::Gemini, &bare, 40).map(|a| a.top),
        Some(6)
    );
    assert_eq!(bare[6], LOWER_HALVES);
}

#[test]
fn a_sent_gemini_message_is_not_its_input() {
    // The input gives its place to a permission prompt; the last message
    // sent, shaded like the input, is still on screen above it.
    let rows = screen(&[
        LOWER_HALVES,
        " > make hello.txt",
        UPPER_HALVES,
        "",
        "╭──────────────────────────────────────╮",
        "│ ?  WriteFile Writing to hello.txt     │",
        "│ Allow this change?                    │",
        "│ ● 1. Allow once                       │",
        "│   2. Allow for this session           │",
        "│   3. No, suggest changes (esc)        │",
        "╰──────────────────────────────────────╯",
    ]);
    assert_eq!(input_area(CLIAgent::Gemini, &rows, 40), None);
}

#[test]
fn geminis_unshaded_input_is_the_prompt_under_its_rule() {
    let rows = screen(&[
        "✦ pong",
        "",
        "? for shortcuts",
        RULE,
        " Shift+Tab to accept edits",
        RULE,
        " >   Type your message or @path/to/file",
        " workspace (/directory)   sandbox   /model",
    ]);
    // From the hint over its mode line's rule.
    assert_eq!(
        input_area(CLIAgent::Gemini, &rows, 40),
        Some(InputArea { top: 2, mode: None })
    );
}

const PI_FOOTER: [&str; 2] = [
    "/private/tmp/t7b5/proj (main)",
    "↑29k ↓52 1.9%/200k (auto)          claude-sonnet-4-6",
];

fn pi_screen(lines: &[&str]) -> Vec<String> {
    let mut rows = screen(lines);
    rows.extend(PI_FOOTER.iter().map(|r| r.to_string()));
    rows
}

#[test]
fn pis_input_is_its_lowest_ruled_editor() {
    // Empty, idle.
    let rows = pi_screen(&[" 只回复两个字：好的", "", " 好的", "", RULE, "", RULE]);
    assert_eq!(
        input_area(CLIAgent::Pi, &rows, 40),
        Some(InputArea { top: 4, mode: None })
    );
    // Several lines in it, while a turn runs and the spinner rides the
    // upper rule.
    let working = "── ⠼ Working ──────────────────────────────";
    let rows = pi_screen(&["", working, "line one", "line two", RULE]);
    assert_eq!(
        input_area(CLIAgent::Pi, &rows, 40),
        Some(InputArea { top: 1, mode: None })
    );
    // Its completion list opens between the editor and the status rows.
    let rows = pi_screen(&[
        RULE,
        "/mod",
        RULE,
        "→ model          <provider/model> — Select model",
        "  scoped-models  Enable/disable models for Ctrl+P cycling",
    ]);
    assert_eq!(
        input_area(CLIAgent::Pi, &rows, 40),
        Some(InputArea { top: 0, mode: None })
    );
}

/// Pi's selectors take the editor's place between its rules: none of
/// them is the input.
#[test]
fn pis_selectors_are_not_its_input() {
    let model = pi_screen(&[
        " 好的",
        "",
        RULE,
        "",
        "Only showing models from configured providers. Use /login to add providers.",
        "",
        ">",
        "",
        "→ ✓ claude-sonnet-4-6 [relay] · default",
        "    gpt-5.5 [relay]",
        "",
        "  Enter to select · Ctrl+S to set as default · Escape/Ctrl+C to cancel",
        RULE,
    ]);
    assert_eq!(input_area(CLIAgent::Pi, &model, 40), None);
    let settings = pi_screen(&[
        " Operation aborted",
        "",
        RULE,
        ">",
        "",
        "→ Auto-compact                      true",
        "  Auto-resize images                true",
        "  (1/32)",
        "",
        "  Type to search · Enter/Space to change · Esc to cancel",
        RULE,
    ]);
    assert_eq!(input_area(CLIAgent::Pi, &settings, 40), None);
    let resume = pi_screen(&[
        RULE,
        "",
        "Resume Session (Current Folder)     ◉ Current Folder | ○ All",
        "tab scope · re:<pattern> regex · \"phrase\" exact",
        "",
        ">",
        "",
        "› Reply with just: ok1                         18 now",
        RULE,
    ]);
    assert_eq!(input_area(CLIAgent::Pi, &resume, 40), None);
    // A ruled block far above the bottom is the transcript's.
    let mut scrolled = screen(&[RULE, "quoted", RULE]);
    scrolled.extend((0..12).map(|i| format!(" reply line {i}")));
    assert_eq!(input_area(CLIAgent::Pi, &scrolled, 40), None);
}

const OMP_BAR: &str =
    " π > ◒ claude-sonnet-4-6 (relay) > 🗑 t7b5/proj > ⑂ main ?15 ▶─4%─────┃────200K─";

#[test]
fn omps_input_hangs_under_its_status_line() {
    let rows = screen(&[
        " DONE",
        "",
        OMP_BAR,
        "╰─                                  ⇧⇥ to change thinking effort",
    ]);
    assert_eq!(
        input_area(CLIAgent::OhMyPi, &rows, 40),
        Some(InputArea { top: 2, mode: None })
    );
    // Several lines, and its command list open under them.
    let rows = screen(&[
        RULE,
        "",
        OMP_BAR,
        "╰─ /model",
        "   second line",
        "❯ ⬢  model         Model: relay/claude-sonnet-4-6                █",
        "  ⬢  modelpreset   Presets: none saved                           █",
    ]);
    assert_eq!(
        input_area(CLIAgent::OhMyPi, &rows, 40),
        Some(InputArea { top: 2, mode: None })
    );
    // The rounded box shape.
    let rows = screen(&[
        "╭── π > ⬢ Sonnet 4.5 · ◒ high > 🗺 Plan ▶────────┃──◀ ◫ 62.0%/200K ⟲ ──╮",
        "╰─ Ask anything, edit files, run tools                               ─╯",
    ]);
    assert_eq!(
        input_area(CLIAgent::OhMyPi, &rows, 40),
        Some(InputArea { top: 0, mode: None })
    );
    // The Claude Code shape, ruled like Pi's.
    let rows = screen(&[
        "───────────────────────────── gallery · ◫ 62.0%/200K ⟲ ─",
        "❯ Ask anything, edit files, run tools",
        RULE,
        " π · ⬢ Sonnet 4.5 · ◒ high · 🗺 Plan",
    ]);
    assert_eq!(
        input_area(CLIAgent::OhMyPi, &rows, 40),
        Some(InputArea { top: 0, mode: None })
    );
}

/// A tool approval and the model picker take the input's place, framed;
/// a frame's bottom edge is not the input's first line.
#[test]
fn omps_dialogs_are_not_its_input() {
    let approval = screen(&[
        "╭──────────────────────────────────────╮",
        "│ $ touch omp2.txt                     │",
        "╰──────────────────────────────────────╯",
        "  ⎋ Creating omp2.txt",
        "╭─ Allow tool: bash ───────────────────╮",
        "│ Command: touch omp2.txt              │",
        "│  ❯ Approve                           │",
        "│    Deny                              │",
        "│ ↑/↓ navigate  ⏎ select  ⎋ cancel     │",
        "╰──────────────────────────────────────╯",
    ]);
    assert_eq!(input_area(CLIAgent::OhMyPi, &approval, 40), None);
    let picker = screen(&[
        "│   ○ anthropic   │   local/qwen2.5-1.5b          free█ │",
        "│   ○ gmi-cloud   │   ● default ◒ · ○ smol · ○ slow   │",
        "├─────────────────┴──────────────────────────────────┤",
        "│ ⏎/→ models · ↑/↓ providers · type to search · ⎋ close │",
        "╰────────────────────────────────────────────────────╯",
    ]);
    assert_eq!(input_area(CLIAgent::OhMyPi, &picker, 40), None);
}

const PRIME_FOOTER: &str = "← manage                           claude-sonnet-4-6 · 0 (0%)";

#[test]
fn prime_agents_input_is_its_shaded_block_over_the_footer() {
    let rows = screen(&[
        "",
        "                      Details mode (Ctrl+O to expand)",
        "",
        " >",
        "",
        PRIME_FOOTER,
    ]);
    assert_eq!(
        input_area(CLIAgent::PrimeAgent, &rows, 40),
        Some(InputArea { top: 2, mode: None })
    );
    // Several lines, while a turn runs, its `/` list open above.
    let rows = screen(&[
        " ⠙ Writing · 4s · ↓ 99 tokens",
        "    › model          [search]",
        "      scoped-models",
        "",
        " >  /model",
        "    second line",
        "",
        "Press Ctrl+C again to exit       claude-sonnet-4-6 · 0 (0%)",
    ]);
    assert_eq!(
        input_area(CLIAgent::PrimeAgent, &rows, 40),
        Some(InputArea { top: 3, mode: None })
    );
}

#[test]
fn prime_agents_model_picker_is_not_its_input() {
    let rows = screen(&[
        "                      Details mode (Ctrl+O to expand)",
        RULE,
        " >  Search models",
        RULE,
        "› claude-sonnet-4-6 (relay)               current · relay",
        "  gpt-5.5 (relay)                                  relay",
        "  (1/1317)",
        "",
        " Input          Cached input          Output",
        " $0             $0                    $0",
        "",
        " ↑/↓ model · ←/→ effort · Enter select · Esc close",
    ]);
    assert_eq!(input_area(CLIAgent::PrimeAgent, &rows, 40), None);
    // A transcript's last words are not a footer with a block over it.
    let rows = screen(&["  Reply with just the word PONG.", "", " PONG"]);
    assert_eq!(input_area(CLIAgent::PrimeAgent, &rows, 40), None);
}

const GROK_TOP: &str = "  ╭──────────────────────────────────────╮";
const GROK_BOTTOM: &str = "  ╰─────────────────── gpt-5.5 (relay) ─╯";

#[test]
fn grok_builds_input_is_its_rounded_frame() {
    let rows = screen(&[
        "  Update: v1.0.46 available, press ctrl+u to restart",
        "",
        GROK_TOP,
        "  │ ❯                                    │",
        GROK_BOTTOM,
        "",
        "                 Logged in with API key  │  [stable]",
    ]);
    assert_eq!(
        input_area(CLIAgent::Grok, &rows, 40),
        Some(InputArea { top: 2, mode: None })
    );
    // Several lines, while a turn runs.
    let rows = screen(&[
        "    ⠼ Waiting for response… 3.1s        3.1s ⇣2.26k [stop]",
        "",
        GROK_TOP,
        "  │ ❯ line one                           │",
        "  │   line two                           │",
        GROK_BOTTOM,
        "",
        "  Shift+Tab:mode  │  Ctrl+c:cancel  │  Ctrl+x:shortcuts",
    ]);
    assert_eq!(
        input_area(CLIAgent::Grok, &rows, 40),
        Some(InputArea { top: 2, mode: None })
    );
}

/// A list open over the frame takes the keys the box would send; a framed
/// block in the transcript is not the input.
#[test]
fn grok_builds_lists_and_transcript_frames_are_not_its_input() {
    let rows = screen(&[
        "  ───────────────────────────────────────4─",
        "    ❯ Grok 4.6                  SpaceXAI's latest",
        "      gpt-5.5 (relay) (current)",
        "  ────────────────────────────────────────",
        GROK_TOP,
        "  │ ❯ /model <model> [window] [effort]   │",
        GROK_BOTTOM,
        "",
        "  Enter:send  │  Opt+Enter:newline  │  Ctrl+x:shortcuts",
    ]);
    assert_eq!(input_area(CLIAgent::Grok, &rows, 40), None);
    // The release notes, drawn over the frame.
    let rows = screen(&[
        "  Update: │                                   │",
        "  ╭───────│        ↑/↓ scroll  |  Esc back    │──╮",
        "  │ ❯     └───────────────────────────────────┘  │",
        GROK_BOTTOM,
        "",
        "                 Logged in with API key  │  [stable]",
    ]);
    assert_eq!(input_area(CLIAgent::Grok, &rows, 40), None);
    let rows = screen(&[
        GROK_TOP,
        "  │ ❯ earlier                            │",
        GROK_BOTTOM,
        " reply line 1",
        " reply line 2",
        " reply line 3",
        " reply line 4",
    ]);
    assert_eq!(input_area(CLIAgent::Grok, &rows, 40), None);
}

/// A dismissed `/model` stays in Grok Build's input under the box, and is
/// cleared before the next message rather than run into it.
#[test]
fn a_command_left_in_grok_builds_input_is_cleared_first() {
    let frame = |line: &str| {
        screen(&[
            GROK_TOP,
            line,
            GROK_BOTTOM,
            "",
            "  Enter:send  │  Opt+Enter:newline",
        ])
    };
    let left = frame("  │ ❯ /model                             │");
    let empty = frame("  │ ❯                                    │");
    assert_eq!(held_lines(CLIAgent::Grok, &left, 40), 1);
    assert_eq!(held_lines(CLIAgent::Grok, &empty, 40), 0);
    assert_eq!(held_lines(CLIAgent::Pi, &left, 40), 0);
    assert_eq!(clear_keys(CLIAgent::Grok, 1), Some(vec![b"\x15".to_vec()]));
    assert_eq!(clear_keys(CLIAgent::Grok, 0), None);
    assert_eq!(clear_keys(CLIAgent::Claude, 1), None);
    // Its Esc does not stop a turn; Ctrl+C does.
    assert_eq!(bytes(&interrupt_keys(Some(CLIAgent::Grok))), [b"\x03"]);
    assert_eq!(bytes(&interrupt_keys(Some(CLIAgent::Pi))), [b"\x1b"]);
    // Crush asks for its Esc twice, in separate reads.
    let crush = interrupt_keys(Some(CLIAgent::Crush));
    assert_eq!(bytes(&crush), [b"\x1b", b"\x1b"]);
    assert!(crush[1].delay >= SETTLE);
}

const CRUSH_HELP: &str =
    " tab focus chat • shift+tab mode • / or ctrl+p commands • ctrl+m models";

#[test]
fn crushs_input_is_its_editor_over_the_key_help() {
    let rows = screen(&[
        " │ Reply with just PONG",
        "",
        "   PONG",
        "",
        "   ◇ claude-sonnet-4-6 (relay) via Kalowave relay in 3s ──────────",
        "",
        "",
        "   > Ready...",
        " :::",
        " :::",
        "",
        CRUSH_HELP,
        "",
    ]);
    assert_eq!(
        input_area(CLIAgent::Crush, &rows, 80),
        Some(InputArea { top: 7, mode: None })
    );
    // Several lines, yolo mode's prompt, the chat holding the keys.
    for (first, top) in [
        ("   > line one", 4),
        ("  !  line one", 4),
        (" ::: Ready...", 4),
    ] {
        let rows = screen(&[
            "   PONG",
            "",
            "",
            "",
            first,
            " ::: line two",
            " ::: line three",
            " ::: line four",
            "",
            " esc cancel • tab focus chat • shift+tab mode",
            "",
        ]);
        assert_eq!(
            input_area(CLIAgent::Crush, &rows, 80),
            Some(InputArea { top, mode: None }),
            "{first}"
        );
    }
}

/// Crush's dialogs are framed over the screen with its editor left under
/// them, and take the keys while open.
#[test]
fn crushs_dialogs_are_not_its_input() {
    let rows = screen(&[
        "   PONG                                    main",
        "        ╭──────────────────────────────╮  /tmp/t7b6/proj",
        "        │ Commands ╱╱╱╱╱╱╱╱╱╱╱╱╱╱╱╱╱╱╱ │",
        "        │ > Type to filter             │",
        "        │ New Session          ctrl+n  │",
        "        ╰──────────────────────────────╯",
        "",
        "   > Ready...",
        " :::",
        " :::",
        "",
        CRUSH_HELP,
    ]);
    assert_eq!(input_area(CLIAgent::Crush, &rows, 80), None);
    // A permission request runs down over the editor's first line.
    let rows = screen(&[
        "     ╭──────────────────────────────────────╮",
        " │ Us│  Permission Required ╱╱╱╱╱╱╱╱╱╱╱╱╱╱╱  │",
        "     │ Tool write                           │",
        "     │        Allow   Allow for Session  Deny│",
        "   > ╰──────────────────────────────────────╯",
        " :::",
        " :::",
        "",
        CRUSH_HELP,
    ]);
    assert_eq!(input_area(CLIAgent::Crush, &rows, 80), None);
    // Its splash, before the editor is drawn.
    let rows = screen(&[
        "  Would you like to initialize now?",
        "",
        "    Yep!     Nope",
        "",
        " ctrl+c quit • ctrl+g more",
    ]);
    assert_eq!(input_area(CLIAgent::Crush, &rows, 80), None);
}

const GOOSE_GAUGE: &str = "  ╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌ 1% 5k/1.0M";

#[test]
fn gooses_input_is_its_prompt_under_the_gauge_at_any_height() {
    let rows = screen(&[
        "    __( O)>  ● new session · anthropic claude-sonnet-4-6",
        "     L L     goose is ready",
        "  ╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌ 0% 0/1.0M",
        "> Enter to send · Ctrl+J newline",
    ]);
    let mut tall = rows.clone();
    tall.extend(std::iter::repeat_n(String::new(), 60));
    for rows in [rows, tall] {
        assert_eq!(
            input_area(CLIAgent::Goose, &rows, 80),
            Some(InputArea { top: 2, mode: None })
        );
    }
    let rows = screen(&["PONG", "  ⏱ 4.60s", GOOSE_GAUGE, "> ", ""]);
    assert_eq!(
        input_area(CLIAgent::Goose, &rows, 80),
        Some(InputArea { top: 2, mode: None })
    );
}

/// While a turn runs, its spinner and output are printed under the line
/// sent; a tool approval is printed there too.
#[test]
fn a_running_goose_turn_is_not_its_input() {
    let spinner = screen(&[
        GOOSE_GAUGE,
        "> Reply with just PONG",
        "",
        "◓  Webbing connections...  (Ctrl+C to interrupt)",
    ]);
    let output = screen(&[
        GOOSE_GAUGE,
        "> Run the shell command: ls",
        "  ────────────────────────────────────────",
        "  ▸ shell",
        "    command: ls",
    ]);
    let transcript = screen(&[GOOSE_GAUGE, "> earlier", "PONG", "  ⏱ 4.60s"]);
    for rows in [spinner, output, transcript] {
        assert_eq!(input_area(CLIAgent::Goose, &rows, 80), None);
    }
}

const AMP_TOP: &str = "╭─────────────────────────────── medium ─╮";
const AMP_BOTTOM: &str = "╰──────────────── /tmp/t7b6/proj (main) ─╯";
const AMP_EMPTY: &str = "│                                      │";

#[test]
fn amps_input_is_the_frame_at_the_bottom() {
    let rows = screen(&[
        "                    •●●●●••     Welcome to Amp",
        "",
        AMP_TOP,
        AMP_EMPTY,
        AMP_EMPTY,
        AMP_EMPTY,
        AMP_BOTTOM,
        "",
    ]);
    assert_eq!(
        input_area(CLIAgent::Amp, &rows, 40),
        Some(InputArea { top: 2, mode: None })
    );
    // Sending, with the thread above.
    let rows = screen(&[
        " ┃ Reply with just PONG",
        "╭──────────────────────── $···· ─ medium ─╮",
        "│ line one                               │",
        "│ line two                               │",
        "╰ ∼ Sending ───────── /tmp/t7b6/proj (main) ─╯",
    ]);
    assert_eq!(
        input_area(CLIAgent::Amp, &rows, 40),
        Some(InputArea { top: 1, mode: None })
    );
}

/// Amp's palette opens framed over the screen; a notice in the input's
/// place is a frame with nothing in its bottom edge.
#[test]
fn amps_palette_and_notices_are_not_its_input() {
    let rows = screen(&[
        "      ╭─ Command Palette ────────────╮",
        "      │ >                            │",
        "   ●● │    thread  new in orb        │",
        "      ╰──────────────────────────────╯",
        "",
        AMP_TOP,
        AMP_EMPTY,
        AMP_BOTTOM,
    ]);
    assert_eq!(input_area(CLIAgent::Amp, &rows, 40), None);
    let rows = screen(&[
        " ┃ Reply with just PONG",
        "╭──────────────────────────────────────╮",
        "│ Out of Credits                       │",
        "│ ‣ Add Paid Credits                   │",
        "│   Dismiss                            │",
        "╰──────────────────────────────────────╯",
    ]);
    assert_eq!(input_area(CLIAgent::Amp, &rows, 40), None);
}

/// What was typed into Crush's, Goose's or Amp's own input before the box
/// was laid over it is cleared, line by line, before the next message.
#[test]
fn text_left_in_crush_goose_and_amp_inputs_is_cleared_first() {
    let crush = |lines: &[&str]| {
        let mut rows = vec!["   PONG".to_string(), String::new()];
        rows.extend(lines.iter().map(|l| l.to_string()));
        rows.extend(["", CRUSH_HELP].map(String::from));
        rows
    };
    assert_eq!(
        held_lines(
            CLIAgent::Crush,
            &crush(&["   > Ready...", " :::", " :::"]),
            80
        ),
        0
    );
    assert_eq!(
        held_lines(
            CLIAgent::Crush,
            &crush(&["  !  Go crazy", " :::", " :::"]),
            80
        ),
        0
    );
    assert_eq!(
        held_lines(CLIAgent::Crush, &crush(&["   > left", " :::", " :::"]), 80),
        1
    );
    assert_eq!(
        held_lines(
            CLIAgent::Crush,
            &crush(&["   > one", " ::: two", " ::: three", " :::"]),
            80
        ),
        3
    );

    let goose = |line: &str| screen(&[GOOSE_GAUGE, line, ""]);
    assert_eq!(
        held_lines(
            CLIAgent::Goose,
            &goose("> Enter to send · Ctrl+J newline"),
            80
        ),
        0
    );
    assert_eq!(held_lines(CLIAgent::Goose, &goose("> "), 80), 0);
    assert_eq!(
        held_lines(CLIAgent::Goose, &goose("> leftover words"), 80),
        1
    );

    let amp = |inside: &[&str]| {
        let mut rows = vec![AMP_TOP.to_string()];
        rows.extend(inside.iter().map(|l| l.to_string()));
        rows.push(AMP_BOTTOM.to_string());
        rows
    };
    assert_eq!(
        held_lines(CLIAgent::Amp, &amp(&[AMP_EMPTY, AMP_EMPTY]), 40),
        0
    );
    assert_eq!(
        held_lines(
            CLIAgent::Amp,
            &amp(&[
                "│ x @my notes.md                       │",
                "│ line two                             │",
                AMP_EMPTY,
            ]),
            40
        ),
        2
    );

    assert_eq!(
        clear_keys(CLIAgent::Crush, 2),
        Some(vec![b"\x15\x7f\x15\x7f".to_vec()])
    );
    assert_eq!(clear_keys(CLIAgent::Amp, 0), None);
}

#[test]
fn copilots_input_is_ruled_off_over_its_status_rows() {
    let rows = screen(&[
        " ● MCP Servers reloaded: 0 servers connected",
        "",
        "",
        " /private/tmp/t7dev/agents/smoke",
        RULE,
        "❯",
        RULE,
        " ← open sidebar · Interactive · Manual Approval",
        " claude-sonnet-4-6",
    ]);
    assert_eq!(
        input_area(CLIAgent::Copilot, &rows, 40),
        Some(InputArea { top: 4, mode: None })
    );
    // Several lines, with the status rows wrapped in a narrow pane.
    let rows = screen(&[
        " /private/tmp/t7dev/agents/smoke",
        RULE,
        "❯ line one",
        "  line two",
        "  line three",
        RULE,
        " ← open    · Interactive · Manual ·/ commands",
        " sidebar     Approval              tab next",
        " claude-sonnet-4-6",
    ]);
    assert_eq!(
        input_area(CLIAgent::Copilot, &rows, 40),
        Some(InputArea { top: 1, mode: None })
    );
    assert_eq!(held_lines(CLIAgent::Copilot, &rows, 40), 3);
    assert_eq!(
        clear_keys(CLIAgent::Copilot, 3),
        Some(vec![b"\x15\x7f\x15\x7f\x15\x7f".to_vec()])
    );
}

const COPILOT_SHADE_TOP: &str = "╻▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄";
const COPILOT_SHADE_BOTTOM: &str = "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀";

/// With true colour Copilot shades its input instead of ruling it off.
#[test]
fn copilots_shaded_input_is_the_block_over_its_status_rows() {
    let at = |inside: &[&str]| {
        let mut rows = vec![
            String::new(),
            " /private/tmp/t7b7/proj".to_string(),
            COPILOT_SHADE_TOP.to_string(),
        ];
        rows.extend(inside.iter().map(|l| l.to_string()));
        rows.push(COPILOT_SHADE_BOTTOM.to_string());
        rows.push(" ← open sidebar · Interactive · Manual Approval".to_string());
        rows.push(" claude-sonnet-4-6".to_string());
        rows
    };
    for (inside, held) in [(&["┃"][..], 0), (&["┃ line one", "┃ line two"][..], 2)] {
        let rows = at(inside);
        assert_eq!(
            input_area(CLIAgent::Copilot, &rows, 40),
            Some(InputArea { top: 2, mode: None })
        );
        assert_eq!(held_lines(CLIAgent::Copilot, &rows, 40), held);
    }
    // Its `/` list over the block, drawn shaded too, with blank rows
    // under a short one; a command sent before is no list.
    let rows = screen(&[
        "  ❯ /move                Move your uncommitted changes",
        "    /memory              Show memory status",
        "",
        "",
        COPILOT_SHADE_TOP,
        "┃ /move",
        COPILOT_SHADE_BOTTOM,
        " Interactive · Manual Approval · /help show help",
    ]);
    assert_eq!(input_area(CLIAgent::Copilot, &rows, 40), None);
    let rows = screen(&[
        "  ❯ /model                                    13:31",
        "",
        " ● Model set to claude-sonnet-4-6",
        "",
        COPILOT_SHADE_TOP,
        "┃",
        COPILOT_SHADE_BOTTOM,
        " Interactive · Manual Approval · / commands",
    ]);
    assert_eq!(
        input_area(CLIAgent::Copilot, &rows, 40),
        Some(InputArea { top: 4, mode: None })
    );
    // On a bar right over the block.
    let rows = screen(&[
        "┃ ❯ /add-dir           Allow file access to a directory",
        "┃   /agent             Browse and select agents",
        COPILOT_SHADE_TOP,
        "┃ /add-dir",
        COPILOT_SHADE_BOTTOM,
        " Interactive · Manual Approval · /help show help",
    ]);
    assert_eq!(input_area(CLIAgent::Copilot, &rows, 40), None);
}

/// Copilot's lists open on a bar right over its input; its approvals and
/// pickers take the input's place, framed or between rules of their own.
#[test]
fn copilots_lists_and_dialogs_are_not_its_input() {
    let slash = screen(&[
        "┃ ❯ /add-dir           Allow file access to a directory",
        "┃   /agent             Browse and select agents",
        RULE,
        "❯ /add-dir",
        RULE,
        " Interactive · Manual Approval · /help show help",
    ]);
    let mention = screen(&[
        "┃   @cop.txt",
        "┃ ❯ @/private/tmp/t7dev/agents/smoke/",
        RULE,
        "❯ @",
        RULE,
        " Interactive · Manual Approval · @ files · # issues",
    ]);
    let approval = screen(&[
        " $ Shell Create cop.txt file",
        "   touch /tmp/t7dev/agents/smoke/cop.txt",
        "",
        "╭──────────────────────────────────────╮",
        "│ Create cop.txt file                  │",
        "│ ──────────────────────────────────── │",
        "│ Do you want to run this command?     │",
        "│                                      │",
        "│ ❯ 1. Yes                             │",
        "│   2. Yes, and don't ask again        │",
        "│   3. No, and tell Copilot (Esc)      │",
        "│                                      │",
        "│ ↑/↓ to navigate · enter to select    │",
        "╰──────────────────────────────────────╯",
    ]);
    let picker = screen(&[
        RULE,
        "",
        " Subagent Configuration",
        "",
        " ❯ explore   built-in   gpt-5.6-luna",
        "   task      built-in   gpt-5.6-luna",
        "",
        " ↑/↓ to navigate · enter to select",
        "",
        RULE,
    ]);
    for rows in [slash, mention, approval, picker] {
        assert_eq!(input_area(CLIAgent::Copilot, &rows, 40), None, "{rows:#?}");
    }
}

const QWEN_FOOTER: [&str; 2] = [
    "  ➜ smoke · claude-sonnet-4-6",
    "  Auto mode (shift + tab to cycle)",
];

#[test]
fn qwens_input_is_ruled_off_over_its_status_rows() {
    let at = |lines: &[&str]| {
        let mut rows = vec![
            "  Tips: You can run any shell commands from Qwen Code.".to_string(),
            String::new(),
            RULE.to_string(),
        ];
        rows.extend(lines.iter().map(|l| l.to_string()));
        rows.push(RULE.to_string());
        rows.extend(QWEN_FOOTER.map(String::from));
        // Drawn after the transcript, with the rest of the screen empty.
        rows.extend(std::iter::repeat_n(String::new(), 14));
        rows
    };
    for (lines, held) in [
        (&[">   Type your message or @path/to/file"][..], 0),
        (&["*   Type your message or @path/to/file"][..], 0),
        (&["!   Type your message or @path/to/file"][..], 0),
        (&["> line one", "  line two \u{200b}"][..], 2),
        (&["> / \u{200b}"][..], 1),
    ] {
        let rows = at(lines);
        assert_eq!(
            input_area(CLIAgent::Qwen, &rows, 40),
            Some(InputArea { top: 2, mode: None }),
            "{lines:?}"
        );
        assert_eq!(held_lines(CLIAgent::Qwen, &rows, 40), held, "{lines:?}");
    }
}

/// Qwen Code stays in its shell mode after a `!` command, its prompt a
/// `!`; the next message leaves it first, and a `!` one stays in it.
#[test]
fn qwens_shell_mode_is_read_off_its_prompt() {
    let at = |line: &str| {
        screen(&[
            RULE,
            line,
            RULE,
            QWEN_FOOTER[0],
            "  shell mode enabled (esc to disable)",
        ])
    };
    assert!(in_shell_mode(CLIAgent::Qwen, &at("! commit this"), 40));
    assert!(!in_shell_mode(
        CLIAgent::Qwen,
        &at(">   Type your message or @path/to/file"),
        40
    ));
    assert!(!in_shell_mode(CLIAgent::Gemini, &at("! commit this"), 40));
    // CodeBuddy's bash mode, the same way.
    let bash = screen(&[RULE, "!", RULE, "! for bash mode  ·  shift+Tab for local"]);
    assert!(in_shell_mode(CLIAgent::CodeBuddy, &bash, 40));
    assert_eq!(
        input_area(CLIAgent::CodeBuddy, &bash, 40),
        Some(InputArea { top: 0, mode: None })
    );
}

/// Qwen Code's `/` list opens under its input, a tool approval and its
/// dialogs take the input's place.
#[test]
fn qwens_lists_and_dialogs_are_not_its_input() {
    let slash = screen(&[
        RULE,
        "> / \u{200b}",
        RULE,
        "  > model [--fast|--voice]   Switch the model for this session",
        "    cd <path>                Move this session to a new directory",
        "    btw                      Ask a quick side question",
        "    bug <description>        submit a bug report",
        "    ide                      manage IDE integration",
        "    vim                      toggle vim mode on/off",
        "  ▼",
        "  (1/86)",
    ]);
    let approval = screen(&[
        RULE,
        "  > Use the shell to run: touch /tmp/t7dev/agents/smoke/qwen.txt",
        "",
        "  ? Shell touch /tmp/t7dev/agents/smoke/qwen.txt",
        "",
        "   Allow execution of: 'touch'?",
        "",
        "   › 1. Yes, allow once",
        "     2. Always allow run 'touch *' commands in this project",
        "     4. No, suggest changes (esc)",
        "",
        "  ⠏ Waiting for user confirmation...",
    ]);
    let model = screen(&[
        "  ╭──────────────────────────────────────╮",
        "  │ Select Model                         │",
        "  │ › 1. [openai] claude-sonnet-4-6      │",
        "  │ ──────────────────────────────────── │",
        "  │ Enter to select, ↑↓ to navigate      │",
        "  ╰──────────────────────────────────────╯",
    ]);
    for rows in [slash, approval, model] {
        assert_eq!(input_area(CLIAgent::Qwen, &rows, 40), None, "{rows:#?}");
    }
}

#[test]
fn codebuddys_input_is_ruled_off_over_its_key_help() {
    let rows = screen(&[
        "│      ████            ████      │ claude-sonnet-4-6      │",
        "╰────────────────────────────────────────────────────────╯",
        "",
        RULE,
        "> line one",
        "  line two",
        RULE,
        "  ← for agents",
        "",
        "",
        "",
    ]);
    assert_eq!(
        input_area(CLIAgent::CodeBuddy, &rows, 40),
        Some(InputArea { top: 3, mode: None })
    );
    assert_eq!(held_lines(CLIAgent::CodeBuddy, &rows, 40), 2);
    // An empty input, or one offering a suggestion to send as it is.
    for line in [">", "> verify file exists with ls command    ↵ send"] {
        let rows = screen(&[
            "✔ Worked for 22s",
            RULE,
            line,
            RULE,
            "? for shortcuts  ← for agents",
        ]);
        assert_eq!(
            input_area(CLIAgent::CodeBuddy, &rows, 40),
            Some(InputArea { top: 1, mode: None })
        );
        assert_eq!(held_lines(CLIAgent::CodeBuddy, &rows, 40), 0);
    }
    // Keys that arrive together it takes for a paste: one at a time.
    assert_eq!(
        clear_keys(CLIAgent::CodeBuddy, 1),
        Some(vec![b"\x15".to_vec(), b"\x7f".to_vec()])
    );
    assert!(clear_input(CLIAgent::CodeBuddy, 1).unwrap()[1].delay >= KEY_GAP);
}

/// CodeBuddy's `/` list opens under its input; a tool approval takes
/// the input's place between rules of its own.
#[test]
fn codebuddys_lists_and_approvals_are_not_its_input() {
    let slash = screen(&[
        RULE,
        "> /",
        RULE,
        "  /commit              Create a git commit",
        "  /commit-push-pr      Commit, push, and open a PR",
        "  /loop                Run a prompt on a recurring interval",
        "  /security-review     Complete a security review",
        "  /add-dir             Add a new working directory",
        "  /agent-mode          Switch the main agent mode",
        "",
    ]);
    let approval = screen(&[
        "● Bash(touch /tmp/t7dev/agents/smoke/cb.txt)",
        RULE,
        " Bash command",
        "",
        "   touch /tmp/t7dev/agents/smoke/cb.txt",
        "   Create cb.txt file",
        "",
        RULE,
        "",
        " Do you want to proceed?",
        "",
        " > 1. Yes",
        "   2. Yes, and don't ask again for session (shift + tab)",
        "   3. No, and tell CodeBuddy what to do differently (escape)",
    ]);
    for rows in [slash, approval] {
        assert_eq!(
            input_area(CLIAgent::CodeBuddy, &rows, 40),
            None,
            "{rows:#?}"
        );
    }
}

const KIMI_FOOTER: [&str; 2] = [
    " claude-sonnet-4-6 thinking: high  …/t7dev/agents/smoke",
    "                                   context: 0% (0/195k)",
];

#[test]
fn kimis_input_is_the_frame_over_its_model_line() {
    let at = |inside: &[&str]| {
        let mut rows = vec![
            " ╭──────────────────────────────────────╮".to_string(),
            " │  ▐█▛█▛█▌  Welcome to Kimi Code!       │".to_string(),
            " ╰──────────────────────────────────────╯".to_string(),
            String::new(),
            "   No session yet — one will be created.".to_string(),
            String::new(),
            " ╭──────────────────────────────────────╮".to_string(),
        ];
        rows.extend(inside.iter().map(|l| l.to_string()));
        rows.push(" ╰──────────────────────────────────────╯".to_string());
        rows.extend(KIMI_FOOTER.map(String::from));
        rows.extend(std::iter::repeat_n(String::new(), 5));
        rows
    };
    for (inside, held) in [
        (&[" │ >                                    │"][..], 0),
        (
            &[
                " │ > line one                           │",
                " │   line two                           │",
                " │   line three                         │",
            ][..],
            3,
        ),
    ] {
        let rows = at(inside);
        assert_eq!(
            input_area(CLIAgent::Kimi, &rows, 40),
            Some(InputArea { top: 6, mode: None })
        );
        assert_eq!(held_lines(CLIAgent::Kimi, &rows, 40), held);
    }
}

/// Kimi Code's lists open under its input's frame; a tool approval takes
/// the frame's place, with only its welcome banner framed far above.
#[test]
fn kimis_lists_and_approvals_are_not_its_input() {
    let slash = screen(&[
        " ╭──────────────────────────────────────╮",
        " │ > /                                  │",
        " ╰──────────────────────────────────────╯",
        " │   → yolo           Ask When Needed    │",
        " │     model          Switch LLM model   │",
        " │     permission     Select permission  │",
        " │     plan           Toggle plan mode   │",
        " │     (1/51)                            │",
        KIMI_FOOTER[0],
        KIMI_FOOTER[1],
    ]);
    let approval = screen(&[
        " ╰──────────────────────────────────────╯",
        "",
        " ✨ Use the shell to run: touch kimi.txt",
        "",
        " ● Running a command · $ touch kimi.txt",
        " ──────────────────────────────────────",
        "   ▶ Run this command?",
        "",
        "   ▶ 1. Approve once",
        "     2. Approve for this session",
        "     3. Reject",
        "",
        "   ↑/↓ select · 1/2/3/4 choose · ↵ confirm",
        " ──────────────────────────────────────",
        KIMI_FOOTER[0],
        KIMI_FOOTER[1],
    ]);
    for rows in [slash, approval] {
        assert_eq!(input_area(CLIAgent::Kimi, &rows, 40), None, "{rows:#?}");
    }
}

const OPENCODE_EDGE: &str = "  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀";

#[test]
fn opencodes_input_is_the_bar_over_its_key_hints() {
    // Its home screen, the input in the middle over the directory.
    let rows = screen(&[
        "        █▀▀█ █▀▀█ █▀▀█ █▀▀▄ █▀▀▀ █▀▀█",
        "",
        "    ┃",
        "    ┃  Ask anything... \"Fix broken tests\"",
        "    ┃",
        "    ┃  Build · claude-sonnet-4-6 Relay",
        "    ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀",
        "    tab agents  ctrl+p commands",
        "",
        "",
        "  /private/tmp/t7dev/agents/smoke            1.18.4",
    ]);
    assert_eq!(
        input_area(CLIAgent::OpenCode, &rows, 40),
        Some(InputArea { top: 2, mode: None })
    );
    assert_eq!(held_lines(CLIAgent::OpenCode, &rows, 40), 0);
    // A session, a turn running, two lines typed.
    let rows = screen(&[
        "  ┃  Use the shell to run: touch oc.txt",
        "  ┃",
        "",
        "     ▣  Build · claude-sonnet-4-6",
        "",
        "  ┃",
        "  ┃  line one",
        "  ┃  line two",
        "  ┃",
        "  ┃  Build · claude-sonnet-4-6 Relay",
        OPENCODE_EDGE,
        "   ⬝⬝⬝⬝⬝⬝⬝⬝  esc interrupt         ctrl+p commands",
        "",
    ]);
    assert_eq!(
        input_area(CLIAgent::OpenCode, &rows, 40),
        Some(InputArea { top: 5, mode: None })
    );
    assert_eq!(held_lines(CLIAgent::OpenCode, &rows, 40), 3);
}

/// OpenCode's lists open on its input's bar, closed by a bar at the
/// right; a permission request takes the input's place, with no edge.
#[test]
fn opencodes_lists_and_requests_are_not_its_input() {
    let slash = screen(&[
        "  ┃ /agents     Switch agent                ┃",
        "  ┃ /connect    Connect provider            ┃",
        "  ┃",
        "  ┃  /",
        "  ┃",
        "  ┃  Build · claude-sonnet-4-6 Relay",
        OPENCODE_EDGE,
        "   tab agents  ctrl+p commands",
    ]);
    let request = screen(&[
        "     $ touch /tmp/t7dev/agents/smoke/oc.txt",
        "",
        "  ┃",
        "  ┃  △ Permission required",
        "  ┃    ← Access external directory",
        "  ┃",
        "  ┃   Allow once   Allow always   Reject",
        "  ┃",
        "",
    ]);
    let sent = screen(&["  ┃", "  ┃  Reply with just PONG", "  ┃", "", "     PONG"]);
    for rows in [slash, request, sent] {
        assert_eq!(input_area(CLIAgent::OpenCode, &rows, 40), None, "{rows:#?}");
    }
}

const QODER_HINT: &str = "                          ? for shortcuts";
const QODER_MODEL: &str = "  Model · ctx ░░░░░░░░░░ 0% · /private/tmp/t7b8/proj";

/// Qoder's screen from its banner down, `over` between the transcript and
/// the mode line's rule, `lines` between the input's rules.
fn qoder_screen(over: &[&str], mode: &[&str], lines: &[&str]) -> Vec<String> {
    let mut rows = screen(&[
        "   ████  ██  Not Login Please Auth   │ 4. Be specific for the best results │",
        "                                     ╰─────────────────────────────────────╯",
        "",
        " > hello there",
        "",
        " x Qoder authentication is not ready. Please sign in again (e.g. via /login)",
        "",
    ]);
    rows.extend(over.iter().map(|l| l.to_string()));
    rows.push(RULE.to_string());
    rows.extend(mode.iter().map(|l| l.to_string()));
    rows.push(RULE.to_string());
    rows.extend(lines.iter().map(|l| l.to_string()));
    rows.push(RULE.to_string());
    rows.push(QODER_MODEL.to_string());
    rows
}

#[test]
fn qoders_input_is_ruled_off_under_its_mode_line() {
    let mode = [" Shift+Tab to Accept Edits          1 MCP server · 20 skills"];
    // Idle, with the hint over the mode line, the box covers from there.
    for (lines, held) in [
        (&[" >   Type your message or @path/to/file"][..], 0),
        (&[" *   Type your message or @path/to/file"][..], 0),
        (&[" !   Type your shell command"][..], 0),
    ] {
        let rows = qoder_screen(&[QODER_HINT], &mode, lines);
        for agent in [CLIAgent::QoderCLI, CLIAgent::QoderCLICn] {
            assert_eq!(
                input_area(agent, &rows, 40),
                Some(InputArea { top: 7, mode: None }),
                "{lines:?}"
            );
            assert_eq!(held_lines(agent, &rows, 40), held, "{lines:?}");
        }
    }
    // Typing, the hint goes; a newline typed last leaves a blank line.
    for (lines, held) in [
        (&[" > hello there"][..], 1),
        (&[" > hello there", "   second line"][..], 2),
        (&[" > line one", "   line two", ""][..], 2),
        (&[" ! echo pasted"][..], 1),
    ] {
        let rows = qoder_screen(&[""], &mode, lines);
        assert_eq!(
            input_area(CLIAgent::QoderCLI, &rows, 40),
            Some(InputArea { top: 8, mode: None }),
            "{lines:?}"
        );
        assert_eq!(held_lines(CLIAgent::QoderCLI, &rows, 40), held, "{lines:?}");
    }
    // In a narrow pane the mode line breaks in two around a blank row.
    let narrow = [
        " Shift+Tab to Accept Edits",
        "",
        "  1 MCP server · 20 skills",
    ];
    let rows = qoder_screen(&[QODER_HINT], &narrow, &[" > x"]);
    assert_eq!(
        input_area(CLIAgent::QoderCLI, &rows, 40),
        Some(InputArea { top: 7, mode: None })
    );
}

/// Qoder stays in its shell mode after a `!` command, its prompt a `!`.
#[test]
fn qoders_shell_mode_is_read_off_its_prompt() {
    let shell = [" Shell mode enabled (esc to disable)  1 MCP server · 20 skills"];
    let rows = qoder_screen(&[QODER_HINT], &shell, &[" !   Type your shell command"]);
    assert!(in_shell_mode(CLIAgent::QoderCLI, &rows, 40));
    assert!(in_shell_mode(CLIAgent::QoderCLICn, &rows, 40));
    let mode = [" Shift+Tab to Accept Edits          1 MCP server · 20 skills"];
    let rows = qoder_screen(&[QODER_HINT], &mode, &[" > ! not a command"]);
    assert!(!in_shell_mode(CLIAgent::QoderCLI, &rows, 40));
}

/// Qoder's lists open under its input, its dialogs in its place.
#[test]
fn qoders_lists_and_dialogs_are_not_its_input() {
    let mode = [" Shift+Tab to Accept Edits          1 MCP server · 20 skills"];
    let mut slash = qoder_screen(&[""], &mode, &[" > /"]);
    slash.pop();
    slash.extend(screen(&[
        " ❯ about                      Show version info",
        "   add-dir                    Add a directory to the workspace context",
        "   agents                     Manage agents",
        "   batch                      Apply a batch change across the codebase",
        "   branch                     Create a new session branch",
        "   btw                        Ask a quick side question",
        "   claim                      Check available campaigns",
        " ▼ clear                      Clear the screen and start a new conversation",
    ]));
    let mut mention = qoder_screen(&[""], &mode, &[" > read @my"]);
    mention.pop();
    mention.push(" ❯ my notes.md".to_string());
    let trust = screen(&[
        " Do you trust the files in this folder?",
        RULE,
        " Please confirm this is your own project or from a trusted source.",
        "",
        " /private/tmp/t7b8/proj",
        "",
        "  ❯ 1. Trust folder",
        "    2. Don't trust and exit",
        "",
        " ↑/↓ navigate · Enter select · Esc back",
    ]);
    let sign_in = screen(&[
        " Welcome to Qoder CLI",
        RULE,
        " Sign in to get started, or exit the application.",
        "",
        "  ❯ 1. Sign in to continue",
        "    2. Exit the application",
        "",
        " ↑/↓ navigate · Enter select · Esc back",
    ]);
    let model = screen(&[
        " > /model",
        "",
        " Model ·  Default (0)   New (0)   Custom (0)",
        RULE,
        " Overview:",
        "  · Current Model     : Auto",
        "",
        " ──────────────────────────────────────",
        "  Press / to search models",
        " ──────────────────────────────────────",
        "  No models in this group.",
        "",
        "",
        " Tab switch · r refresh · Esc back",
    ]);
    let sent = screen(&[
        " > hello there",
        "   second line",
        "",
        " x Qoder authentication is not ready.",
    ]);
    for rows in [slash, mention, trust, sign_in, model, sent] {
        for agent in [CLIAgent::QoderCLI, CLIAgent::QoderCLICn] {
            assert_eq!(input_area(agent, &rows, 40), None, "{rows:#?}");
        }
    }
}

/// The grid gives up rows only for the part of the box an input does not
/// already hold, and whole rows of it.
#[test]
fn the_grid_makes_room_only_for_what_the_input_does_not_hold() {
    let (line, below) = (px(21.), px(9.));
    // Claude Code's five rows hold the box.
    assert_eq!(rows_short(px(101.), line, 5, below), 0);
    // Oh My Pi's two do not: 101 - 42 - 9 = 50, three rows' worth.
    assert_eq!(rows_short(px(101.), line, 2, below), 3);
    assert_eq!(rows_short(px(101.), line, 4, below), 1);
    assert_eq!(rows_short(px(101.), px(0.), 2, below), 0);
}

#[test]
fn model_ids_read_the_way_people_say_them() {
    assert_eq!(model_label("claude-opus-5-5"), "Opus 5.5");
    assert_eq!(model_label("claude-sonnet-5-5[1m]"), "Sonnet 5.5");
    assert_eq!(model_label("claude-haiku-4-5-20251001"), "Haiku 4.5");
    assert_eq!(model_label("gpt-5-codex"), "gpt-5-codex");
}

#[test]
fn a_reported_model_is_checked_under_its_alias() {
    assert_eq!(model_alias("claude-opus-5-5"), Some("opus"));
    assert_eq!(model_alias("claude-sonnet-5-5[1m]"), Some("sonnet[1m]"));
    assert_eq!(model_alias("claude-haiku-4-5-20251001"), Some("haiku"));
    assert_eq!(model_alias("claude-fable-5-1"), Some("fable"));
    assert_eq!(model_alias("gpt-5-codex"), None);
    assert_eq!(alias_label("sonnet[1m]"), "Sonnet · 1M");
    assert_eq!(alias_label("haiku"), "Haiku");
    assert_eq!(effort_label("medium"), "Medium");
    assert_eq!(effort_label("xhigh"), "Extra high");
}

#[test]
fn each_agents_builtin_commands_are_distinct_slash_words() {
    for agent in [
        CLIAgent::Claude,
        CLIAgent::Codex,
        CLIAgent::Gemini,
        CLIAgent::OpenCode,
        CLIAgent::Copilot,
        CLIAgent::Qwen,
        CLIAgent::CodeBuddy,
        CLIAgent::Kimi,
        CLIAgent::Goose,
        CLIAgent::QoderCLI,
    ] {
        let names: Vec<&str> = builtin_commands(agent).iter().map(|(n, _)| *n).collect();
        for (i, name) in names.iter().enumerate() {
            assert!(name.starts_with('/') && !name.contains(' '), "{name}");
            assert!(!names[..i].contains(name), "{agent:?} lists {name} twice");
        }
    }
    // The toolbar's effort picker types it; the menu should offer it too.
    assert!(
        builtin_commands(CLIAgent::Claude)
            .iter()
            .any(|(n, _)| *n == "/effort")
    );
}

#[test]
fn a_slash_opens_the_menu_only_at_the_start_of_the_message() {
    let t = |text: &str| trigger_at(text, text.len());
    assert_eq!(
        t("/comp"),
        Some(Trigger {
            sigil: '/',
            start: 0,
            query: "comp".into()
        })
    );
    assert_eq!(t("  /").map(|t| t.start), Some(2));
    assert_eq!(t("see src/main.rs"), None);
    assert_eq!(t("fix /tmp"), None);
}

#[test]
fn an_at_opens_the_menu_at_the_start_of_any_word() {
    let text = "look at @src/ma and";
    let cursor = "look at @src/ma".len();
    assert_eq!(
        trigger_at(text, cursor),
        Some(Trigger {
            sigil: '@',
            start: 8,
            query: "src/ma".into()
        })
    );
    assert_eq!(trigger_at("mail me@host", 12), None);
    assert_eq!(
        trigger_at("done @x ", 8),
        None,
        "a finished word is not a query"
    );
}

#[test]
fn commands_that_start_with_the_query_come_before_ones_that_contain_it() {
    let all: Vec<(String, String)> = [("/compact", ""), ("/clear", ""), ("/memory", "")]
        .iter()
        .map(|(n, d)| (n.to_string(), d.to_string()))
        .collect();
    let labels = |q| {
        command_items(&all, q)
            .into_iter()
            .map(|i| i.label)
            .collect::<Vec<_>>()
    };
    assert_eq!(labels("c"), ["/compact", "/clear"]);
    assert_eq!(labels("m"), ["/memory", "/compact"]);
    assert_eq!(labels(""), ["/compact", "/clear", "/memory"]);
}

#[test]
fn a_file_mention_is_spelled_from_the_panes_directory() {
    let index = anterm_ui_kit::search::files::build_index(
        &[PathBuf::from("/repo")],
        vec![
            PathBuf::from("/repo/app/src/main.rs"),
            PathBuf::from("/repo/README.md"),
        ],
        false,
    );
    let items = file_items(&index, "main", Some(Path::new("/repo/app")), None, None);
    assert_eq!(items[0].insert, "@src/main.rs");
    let items = file_items(&index, "readme", Some(Path::new("/repo/app")), None, None);
    assert_eq!(
        items[0].insert, "@/repo/README.md",
        "outside the cwd: in full"
    );
}

#[test]
fn claude_custom_commands_come_from_the_project_and_the_user() {
    let dir = std::env::temp_dir().join(format!("anterm-cmds-{}", std::process::id()));
    let project = dir.join("proj");
    let home = dir.join("home");
    std::fs::create_dir_all(project.join(".claude/commands/team")).unwrap();
    std::fs::create_dir_all(home.join(".claude/commands")).unwrap();
    std::fs::write(project.join(".claude/commands/ship.md"), "").unwrap();
    std::fs::write(project.join(".claude/commands/team/triage.md"), "").unwrap();
    std::fs::write(home.join(".claude/commands/standup.md"), "").unwrap();
    std::fs::write(home.join(".claude/commands/notes.txt"), "").unwrap();
    // A link back up the tree is read once, not walked forever.
    #[cfg(unix)]
    std::os::unix::fs::symlink("..", project.join(".claude/commands/team/up")).unwrap();
    let host = anterm_core::host::local::LocalHost::new();
    let names: Vec<String> = custom_commands(&*host, Some(&project), Some(&home))
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(names, ["/ship", "/standup", "/triage"]);
}
