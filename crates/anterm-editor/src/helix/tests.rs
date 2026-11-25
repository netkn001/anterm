use super::{
    CommandBarAction, HelixHostCommand, HelixMode, HelixOutcome, HelixState, KeyInput, SurfaceKind,
    ViewAlign, block_cursor_byte_range_str, find_next_match, parse_command_bar,
    split_selection_on_pattern,
};

struct OwnedKeystroke {
    key: String,
    key_char: Option<String>,
    control: bool,
    alt: bool,
    shift: bool,
}

impl OwnedKeystroke {
    fn as_input(&self) -> KeyInput<'_> {
        KeyInput {
            key: &self.key,
            key_char: self.key_char.as_deref(),
            control: self.control,
            alt: self.alt,
            shift: self.shift,
            platform: false,
        }
    }
}

/// Helper to construct a key event in unit tests.
fn make_keystroke(key: &str, shift: bool, control: bool, alt: bool) -> OwnedKeystroke {
    OwnedKeystroke {
        control,
        alt,
        shift,
        key: key.to_string(),
        key_char: if !control && !alt && key.len() == 1 {
            let c = key.chars().next().unwrap();
            Some((if shift { c.to_ascii_uppercase() } else { c }).to_string())
        } else {
            None
        },
    }
}

fn press(hx: &mut HelixState, text: &mut String, seq: &str) -> Option<HelixHostCommand> {
    let mut last_host = None;
    for ch in seq.chars() {
        let (key_str, shift) = if ch == ' ' {
            ("space".to_string(), false)
        } else if ch.is_ascii_uppercase() {
            (ch.to_ascii_lowercase().to_string(), true)
        } else {
            (ch.to_string(), false)
        };
        let ks = make_keystroke(&key_str, shift, false, false);
        match hx.handle_keystroke(ks.as_input(), text, SurfaceKind::CodeEditor, None) {
            HelixOutcome::Edit {
                range,
                new_text,
                new_anchor,
                new_head,
                ..
            } => {
                text.replace_range(range, &new_text);
                hx.anchor = new_anchor;
                hx.head = new_head;
            }
            HelixOutcome::Host(cmd) => {
                last_host = Some(cmd);
            }
            _ => {}
        }
    }
    last_host
}

#[test]
fn word_motions_select_traversed_span_and_delete_works() {
    let mut hx = HelixState::new(HelixMode::Normal);
    let mut text = "hello world_two three".to_string();
    press(&mut hx, &mut text, "w");
    assert_eq!(&text[hx.selection_range(&text)], "hello ");
    press(&mut hx, &mut text, "d");
    assert_eq!(text, "world_two three");
    assert_eq!(hx.registers.read(None, None), "hello ");
}

#[test]
fn line_selection_x_selects_and_extends_full_lines() {
    let mut hx = HelixState::new(HelixMode::Normal);
    let mut text = "alpha\nbeta\ngamma\n".to_string();
    press(&mut hx, &mut text, "x");
    assert_eq!(&text[hx.selection_range(&text)], "alpha\n");
    press(&mut hx, &mut text, "x");
    assert_eq!(&text[hx.selection_range(&text)], "alpha\nbeta\n");
    press(&mut hx, &mut text, "d");
    assert_eq!(text, "gamma\n");
}

#[test]
fn match_inside_and_around_text_objects_and_surround() {
    let mut hx = HelixState::new(HelixMode::Normal);
    let mut text = "fn call(foo_bar, baz) {}".to_string();
    hx.head = 9; // inside `foo_bar`
    hx.anchor = 9;
    press(&mut hx, &mut text, "mi(");
    assert_eq!(&text[hx.selection_range(&text)], "foo_bar, baz");

    // Surround add `[` around `foo_bar`
    hx.head = 9;
    hx.anchor = 9;
    press(&mut hx, &mut text, "miwms[");
    assert_eq!(text, "fn call([foo_bar], baz) {}");

    // Surround replace `[` -> `{`
    hx.head = 10;
    hx.anchor = 10;
    press(&mut hx, &mut text, "mr[{");
    assert_eq!(text, "fn call({foo_bar}, baz) {}");

    // Surround delete `{`
    hx.head = 10;
    hx.anchor = 10;
    press(&mut hx, &mut text, "md{");
    assert_eq!(text, "fn call(foo_bar, baz) {}");
}

#[test]
fn named_and_blackhole_registers_preserve_default_register() {
    let mut hx = HelixState::new(HelixMode::Normal);
    let mut text = "first second".to_string();
    press(&mut hx, &mut text, "w\"ay");
    assert_eq!(hx.registers.read(Some('a'), None), "first ");
    assert_eq!(hx.registers.read(None, None), "");

    // Blackhole register `_` does not overwrite register `a` or default
    press(&mut hx, &mut text, "\"_d");
    assert_eq!(hx.registers.read(None, None), "");
    assert_eq!(hx.registers.read(Some('a'), None), "first ");
}

#[test]
fn command_bar_parser_handles_standard_helix_commands() {
    assert_eq!(parse_command_bar(":w"), CommandBarAction::Write(None));
    assert_eq!(
        parse_command_bar(":w src/main.rs"),
        CommandBarAction::Write(Some("src/main.rs".to_string()))
    );
    assert_eq!(
        parse_command_bar(":q!"),
        CommandBarAction::Quit { force: true }
    );
    assert_eq!(
        parse_command_bar(":wq"),
        CommandBarAction::WriteQuit {
            force: false,
            path: None
        }
    );
    assert_eq!(parse_command_bar(":wqa"), CommandBarAction::WriteQuitAll);
    assert_eq!(parse_command_bar(":42"), CommandBarAction::GoToLine(42));
    assert_eq!(parse_command_bar(":vs"), CommandBarAction::VSplit(None));
}

#[test]
fn goto_and_space_minor_modes_emit_host_commands() {
    let mut hx = HelixState::new(HelixMode::Normal);
    let mut text = "one\ntwo\nthree".to_string();
    assert_eq!(
        press(&mut hx, &mut text, "gd"),
        Some(HelixHostCommand::GoToDefinition)
    );
    assert_eq!(
        press(&mut hx, &mut text, " f"),
        Some(HelixHostCommand::QuickOpenFile)
    );
    assert_eq!(
        press(&mut hx, &mut text, "zz"),
        Some(HelixHostCommand::AlignView(ViewAlign::Center))
    );
}

#[test]
fn split_selection_and_search_smartcase() {
    let text = "foo = 1, FOO = 2, bar = 3";
    let r1 = find_next_match(text, "foo", 0, true).unwrap();
    assert_eq!(&text[r1], "FOO");
    let r2 = find_next_match(text, "FOO", 0, true).unwrap();
    assert_eq!(&text[r2], "FOO");

    let splits = split_selection_on_pattern(text, 0..text.len(), r"\d+");
    assert_eq!(splits.len(), 3);
    assert_eq!(&text[splits[0].clone()], "1");
    assert_eq!(&text[splits[1].clone()], "2");
    assert_eq!(&text[splits[2].clone()], "3");
}

#[test]
fn block_cursor_byte_range_handles_multibyte_utf8_and_mid_char_offsets() {
    let md = "1234567中\r\nCodex、WorkBuddy 和 🚀\n";

    // `head + 8` lands inside '中' (bytes 7..10): must still measure '1' (0..1).
    assert_eq!(block_cursor_byte_range_str(md, 0), 0..1);
    // Directly on 3-byte CJK character '中' (bytes 7..10).
    assert_eq!(block_cursor_byte_range_str(md, 7), 7..10);
    // Mid-character byte offsets inside '中' clamp safely to its start boundary.
    assert_eq!(block_cursor_byte_range_str(md, 8), 7..10);
    assert_eq!(block_cursor_byte_range_str(md, 9), 7..10);
    // Line endings ('\r' and '\n') collapse to zero-width caret range.
    assert_eq!(block_cursor_byte_range_str(md, 10), 10..10);
    assert_eq!(block_cursor_byte_range_str(md, 11), 11..11);

    // Every byte offset (including non-char boundaries and past-EOF) produces valid boundaries.
    for raw in 0..=(md.len() + 16) {
        let range = block_cursor_byte_range_str(md, raw);
        assert!(md.is_char_boundary(range.start), "start at raw={raw}");
        assert!(md.is_char_boundary(range.end), "end at raw={raw}");
        assert!(range.start <= range.end);
    }
}
