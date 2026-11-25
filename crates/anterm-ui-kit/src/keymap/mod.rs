use gpui::{App, Global, KeyBinding, Keystroke, Modifiers};

use crate::actions::*;
use crate::config::{Config, KeybindingOverride};
use crate::actions::terminal_actions::{
    AlternatePaste, ClearScrollback, CopyRaw, CopyText, FindInTerminal, FindNext, FindPrevious,
    InsertNewline, InsertNewlineFallback, PasteText, RunReverseSearchSelection, ToggleComposer,
};
use anterm_i18n::{L10nKey, t, t_fmt};
use crate::search::CommandGroup;
use crate::actions::humanize_action;
use crate::theme::set_menus;

/// The bindings that were already in the keymap before anterm put its own
/// there. gpui-component installs a table of its own from `gpui_kit::component::
/// init` — the `Input` context's editing keys, the list and menu navigation,
/// the escape that closes a dialog — and it has no entry point to install
/// them a second time. A rebuild replaces the whole map, so it has to lay
/// these back down first, in the order they were added, or the map that comes
/// out has no backspace in any text field in the app (#548).
struct BaseBindings(Vec<KeyBinding>);
impl Global for BaseBindings {}

pub fn init(cx: &mut App) {
    if cx.try_global::<BaseBindings>().is_none() {
        let base: Vec<KeyBinding> = cx.key_bindings().borrow().bindings().cloned().collect();
        cx.set_global(BaseBindings(base));
    }
    rebuild_keymap(cx);
    cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
    // On the app rather than on a window, for the same reason `Quit` is: this
    // is the one action in the table whose whole point is the state where no
    // window is there to dispatch it. `show_tray_icon` is on by default, so
    // closing the last window retires anterm to the tray instead of quitting —
    // process alive, nothing on screen — and a `NewWindow` that only exists on
    // a window's render root is dead in exactly the state a New Window chord
    // is for. `AntermApp`'s own listener still wins wherever there is a window:
    // gpui runs the window's bubble phase first and returns before the global
    // one, so the two never both fire.
    cx.on_action(|_: &NewWindow, cx: &mut App| crate::app_hooks::open_new_window(cx));
    crate::app_hooks::init_palette(cx);
    set_menus(cx);
}

/// The fixed bindings every keymap carries, whatever the config says. These
/// are the terminal's own keys (Tab/BackTab as input), the font-size step
/// that has no settings row, and the modal-context guards the dispatch path
/// needs — none of them come from `effective_bindings`, so a rebuild that
/// only replayed the config would drop them.
fn fixed_bindings() -> Vec<KeyBinding> {
    let mut bindings = vec![
        kb("secondary-+", IncreaseFontSize, None),
        kb("tab", SendTab, Some("Terminal")),
        kb("shift-tab", SendBackTab, Some("Terminal")),
        // The Ctrl+R menu runs its selection on Cmd+Enter, which on macOS is
        // also `ToggleFullscreen`. Scoped to the open menu, it is deeper in
        // the dispatch path than the window binding and wins only there.
        // `cmd`, not `secondary`: the menu's own key handler runs on the
        // platform key everywhere, and Ctrl+Enter off macOS only accepts.
        kb(
            "cmd-enter",
            RunReverseSearchSelection,
            Some("Terminal && reverse_search"),
        ),
    ];
    // Tab walks the search's tabs. Bound on the search's own context so Root's
    // focus walker does not take it out of the modal onto chrome tiles behind it.
    let search = Some(crate::actions::SEARCH_KEY_CONTEXT);
    bindings.push(kb("tab", SearchNextTab, search));
    bindings.push(kb("shift-tab", SearchPrevTab, search));
    // The code editor's multi-cursor chords, laid down again after anterm's
    // own table. gpui ranks a context-free binding as deep as the focused
    // context, and breaks the tie by whichever was added last — so the copies
    // gpui-component installs lose ⌘D to `SplitRight` and ⌘⌥↑/↓ to
    // `FocusPaneUp`/`Down`, whose handlers sit on the workspace root and take
    // the key before the editor is asked. Re-added here they win inside an
    // `Input`; only the code editor handles them, so in any other text field
    // gpui falls through to the pane binding next in line.
    {
        use crate::actions::CODE_EDITOR_CONTEXT;
        use crate::actions::editor_text_actions::{
            AddCursorAbove, AddCursorBelow, SelectAllOccurrences,
            SelectNextOccurrence, SkipOccurrence,
        };
        let input = Some("Input");
        bindings.push(kb("secondary-d", SelectNextOccurrence, input));
        bindings.push(kb(
            "secondary-shift-l",
            SelectAllOccurrences,
            input,
        ));
        bindings.push(kb("secondary-alt-up", AddCursorAbove, input));
        bindings.push(kb("secondary-alt-down", AddCursorBelow, input));
        // ⌘K ⌘D: a chord whose first key is `ClearScrollback` on macOS. gpui
        // drops a pending chord that ranks below a complete match, so this
        // too has to come after anterm's table. Code editor only: in any other
        // field ⌘K goes straight through, without waiting for a second key.
        bindings.push(kb(
            "secondary-k secondary-d",
            SkipOccurrence,
            Some(CODE_EDITOR_CONTEXT),
        ));
    }
    // The language-server commands, on the keys VS Code taught everyone, and
    // only inside a text field: F2 and F12 in a terminal belong to the
    // program running there. The handlers act only for the code editor and
    // let the key go on anywhere else. ⌘. / Ctrl+. needs nothing here: the
    // editor binds it to the same code-action menu itself.
    let input = Some("Input");
    bindings.push(kb("f12", EditorGoToDefinition, input));
    bindings.push(kb("f2", EditorRenameSymbol, input));
    bindings.push(kb("shift-alt-f", EditorFormatDocument, input));
    bindings.push(kb("shift-f12", EditorFindReferences, input));
    // The gutter's change markers, on VS Code's chords. Only in an `Input`:
    // a function key bound for the whole window would never reach the shell,
    // and the handlers let the key through when the code editor is not the
    // field that has it.
    {
        let input = Some("Input");
        bindings.push(kb("alt-f5", EditorNextChange, input));
        bindings.push(kb("shift-alt-f5", EditorPrevChange, input));
        bindings.push(kb("alt-f3", EditorPeekChange, input));
    }
    // The code editor's line commands, for the same tie: ⌘/ is the shortcut
    // sheet, ⌘↵ fullscreen, ⌘⇧↵ maximize and ⌥↑/⌥↓ pane focus off macOS, all
    // context-free. Bound on `CodeEditor`, which only a multi-line code
    // editor declares, so every other text field and the terminal keep the
    // app's keys.
    {
        use crate::actions::editor_text_actions::{
            CODE_EDITOR_CONTEXT, CopyLineDown, CopyLineUp, DeleteLine, InsertLineAbove,
            InsertLineBelow, MoveLineDown, MoveLineUp, MoveToMatchingBracket, SelectLine,
            ToggleBlockComment, ToggleLineComment,
        };
        let editor = Some(CODE_EDITOR_CONTEXT);
        bindings.extend([
            kb("secondary-/", ToggleLineComment, editor),
            kb("alt-shift-a", ToggleBlockComment, editor),
            kb("alt-up", MoveLineUp, editor),
            kb("alt-down", MoveLineDown, editor),
            kb("alt-shift-up", CopyLineUp, editor),
            kb("alt-shift-down", CopyLineDown, editor),
            kb("secondary-shift-k", DeleteLine, editor),
            kb("secondary-enter", InsertLineBelow, editor),
            kb("secondary-shift-enter", InsertLineAbove, editor),
            kb("secondary-l", SelectLine, editor),
            kb("secondary-shift-\\", MoveToMatchingBracket, editor),
        ]);
        #[cfg(target_os = "macos")]
        bindings.push(kb(
            "ctrl-j",
            crate::actions::editor_text_actions::JoinLines,
            editor,
        ));
    }
    bindings
}

/// Replaces the keymap with the full set: the bindings anterm inherited, then
/// the ones the live config asks for, then the fixed ones.
///
/// gpui's `Keymap::add_bindings` only ever pushes — it never dedups and
/// nothing retires a binding — so rebinding by *appending* (the old `rebind`)
/// leaked one full table per call, and every keystroke walks the whole map.
/// Clearing first makes a rebind O(map) instead of O(map × calls), and makes
/// it safe to drive from the config watcher: a hand edit that only reorders
/// or re-comments the file reloads to an identical triple and never reaches
/// here, while a real change rebuilds once (#548). The order is the order
/// `init` bound them in, and gpui reads the map back to front, so anterm's
/// bindings still win over the inherited ones.
fn rebuild_keymap(cx: &mut App) {
    let effective = effective_bindings(cx);
    let mut bindings: Vec<KeyBinding> = cx
        .try_global::<BaseBindings>()
        .map(|base| base.0.clone())
        .unwrap_or_default();
    bindings.extend(action_bindings(&effective));
    bindings.extend(fixed_bindings());
    cx.clear_key_bindings();
    cx.bind_keys(bindings);
}

pub fn rebind(cx: &mut App) {
    rebuild_keymap(cx);
    set_menus(cx);
}

/// The three config fields `effective_bindings` reads, as one comparable
/// value. The config watcher reloads on every write — including the app's
/// own `save()`, which fires on a sidebar drag — so it compares the triple
/// before and after and only rebinds when one of these actually moved;
/// otherwise each save would rebuild the keymap for nothing (#548).
pub fn keybinding_config(cx: &App) -> (Vec<(String, KeybindingOverride)>, String, String) {
    let cfg = cx.global::<Config>();
    let mut overrides: Vec<(String, KeybindingOverride)> = cfg
        .keybindings
        .iter()
        .map(|(a, k)| (a.clone(), k.clone()))
        .collect();
    // A HashMap's order is not stable across reloads, and a reordered map is
    // not a changed binding set.
    overrides.sort();
    (overrides, cfg.keybinding_preset.clone(), cfg.prefix.clone())
}

const INSERT_NEWLINE_DEFAULT: &str = "shift-enter";

const INSERT_NEWLINE_ALT_DEFAULT: &str = "alt-enter";

const PASTE_ALT_DEFAULT: &str = "shift-insert";

fn paste_text_default() -> &'static str {
    per_platform("", "ctrl-shift-v")
}

/// Windows and Linux paste with Ctrl+V everywhere else in the desktop, so the
/// terminal answers it too — but only off the alternate screen, where the key
/// is a control code a full-screen program is waiting for (#677). It is a
/// binding of its own rather than a second keystroke on `PasteText` so that it
/// can carry that narrower context, and so that a user who wants the Windows
/// Terminal behaviour back can say so: `"AlternatePaste": ""` hands Ctrl+V to
/// the shell at the prompt as well, and `"PasteText": "ctrl-v"` pastes with it
/// on every screen.
fn alternate_paste_default() -> &'static str {
    per_platform("", "ctrl-v")
}

fn extra_defaults() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        (
            "InsertNewline",
            INSERT_NEWLINE_DEFAULT,
            INSERT_NEWLINE_ALT_DEFAULT,
        ),
        ("PasteText", paste_text_default(), PASTE_ALT_DEFAULT),
    ]
}

fn extra_keystrokes(effective: &[(String, String)]) -> Vec<(&'static str, &'static str)> {
    extra_defaults()
        .into_iter()
        .filter(|(action, primary, _)| {
            !primary.is_empty() && effective.iter().any(|(a, k)| a == action && k == primary)
        })
        .map(|(action, _, extra)| (action, extra))
        .collect()
}

fn is_default_insert_newline_binding(action: &str, key: &str) -> bool {
    action == "InsertNewline" && key == INSERT_NEWLINE_DEFAULT
}

pub fn extra_bindings(cx: &App) -> Vec<(String, String)> {
    extra_keystrokes(&effective_bindings(cx))
        .into_iter()
        .map(|(a, k)| (a.to_string(), k.to_string()))
        .collect()
}

/// Installs `key` for `action`, and alongside it the chord the platform will
/// actually deliver when the two are spelled differently — `secondary-shift-]`
/// is pressed as `secondary-}`, and only the second ever reaches the
/// dispatcher. Both go in rather than one replacing the other, so a spec that
/// was already live stays live whatever the backend does with Shift (#750).
///
/// Answers whether the action was known at all, which is the caller's cue to
/// warn about a name it cannot bind.
fn push_binding(bindings: &mut Vec<KeyBinding>, action: &str, key: &str) -> bool {
    let Some(binding) = make_binding(action, key) else {
        return false;
    };
    bindings.push(binding);
    if let Some(folded) = folded_spec(key)
        && let Some(alias) = make_binding(action, &folded)
    {
        bindings.push(alias);
    }
    true
}

fn action_bindings(effective: &[(String, String)]) -> Vec<KeyBinding> {
    let mut bindings = Vec::new();
    for (action, key) in extra_keystrokes(effective) {
        push_binding(&mut bindings, action, key);
    }
    for (action, key) in effective {
        if key.is_empty() {
            continue;
        }
        if !keystroke_is_valid(key) {
            log::warn!("ignoring keybinding for '{action}': invalid keystroke '{key}'");
            continue;
        }
        // Said once, and the binding still installs: a chord the user asked
        // for by name is the user's to spend, the way the tmux preset spends
        // Ctrl+B. The invariant this guards is that no *default* spends one
        // without saying so — `no_default_binding_sits_on_a_terminal_control_code`
        // is the half of it that fails a build. A single chord only, since a
        // prefix like `ctrl-b n` is that choice made deliberately.
        // Over both spellings that reach the keymap, because the fold is what
        // decides which of them the shell actually loses. `steals_a_control_code`
        // only ever recognises a bare Ctrl, so it waves `ctrl-shift-2` through —
        // and then the fold installs `ctrl-@` beside it, which is NUL. The chord
        // was dead before it was folded and stole nothing; now that it is live
        // the warning has to follow it (#750). At most one of the two can trip:
        // a written chord that folds carries Shift, and one that carries Shift
        // is never a control code.
        let folded = folded_spec(key);
        for chord in [Some(key.as_str()), folded.as_deref()]
            .into_iter()
            .flatten()
        {
            if !chord.contains(' ')
                && steals_a_control_code(chord)
                && !control_code_binding_allowed(action, chord)
            {
                log::warn!(
                    "keybinding '{chord}' for '{action}' takes a control code away from the shell"
                );
            }
        }
        if !push_binding(&mut bindings, action, key) {
            log::warn!("ignoring keybinding: unknown action '{action}'");
        } else if is_default_insert_newline_binding(action, key) {
            bindings.push(KeyBinding::new(
                INSERT_NEWLINE_DEFAULT,
                InsertNewlineFallback,
                Some("Terminal"),
            ));
        }
    }
    bindings
}

/// Whether a chord is one the terminal owes the PTY as a control code.
///
/// Ctrl and nothing else, over the keys that carry a C0 byte: the alphabet,
/// `@ [ \ ] ^ _ / ?`, the digits 2..8 and Space — `ctrl_c0` in
/// `terminal::input` is the table this mirrors. A binding sitting on one of
/// these does not merely shadow the shell, it deletes a byte the program on
/// the far end is waiting for — Ctrl+D is EOF, Ctrl+W deletes a word, Ctrl+^
/// is vim's alternate file.
///
/// The backtick is in the set without being in that table — it is held over
/// from when this rule lived inside the defaults test. It errs the safe way:
/// a chord that encodes nothing gets a warning it did not strictly earn,
/// which is cheaper than a default quietly eating one that does.
fn steals_a_control_code(chord: &str) -> bool {
    let Ok(ks) = Keystroke::parse_with_os(chord, crate::platform::current_os()) else {
        return false;
    };
    let m = &ks.modifiers;
    if !m.control || m.alt || m.shift || m.platform || m.function {
        return false;
    }
    ks.key == "space"
        || ks.key.len() == 1
            && ks.key.chars().next().is_some_and(|c| {
                c.is_ascii_alphabetic() || "[]\\`^_/?@".contains(c) || ('2'..='8').contains(&c)
            })
}

/// The bindings allowed to sit on a control code anyway.
///
/// `EditorSave` stays on Ctrl+S — and `EditorGoToLine` on Ctrl+G, as in
/// every other code editor — because its handler in `app.rs` calls
/// `cx.propagate()` whenever the editor does not have focus, so the keystroke
/// reaches the terminal as XOFF instead of dying at the window. Ctrl+V is the
/// paste chord every Windows and Linux desktop trains its users on; anterm
/// answers it the way Windows Terminal does, and keeps it off the alternate
/// screen (see `alternate_paste_default`), so it is allowed under any action —
/// including a `PasteText` a user deliberately moves onto it (#677).
///
/// Anything else added here needs a fall-through of its own; a binding that
/// simply swallows the byte does not belong on this list.
fn control_code_binding_allowed(action: &str, chord: &str) -> bool {
    matches!(action, "EditorSave" | "EditorGoToLine")
        // Bound in the text field's context only, and handled only by the
        // code editor: in the terminal the chord is never matched, and in any
        // other field it falls through. VS Code's Go Forward, ⌃⇧-, folds to
        // Ctrl+_ (US) there.
        || action_context(action) == Some(crate::actions::NAV_KEY_CONTEXT)
        || (cfg!(not(target_os = "macos")) && chord == "ctrl-v")
}

fn per_platform(mac: &'static str, other: &'static str) -> &'static str {
    if cfg!(target_os = "macos") {
        mac
    } else {
        other
    }
}

pub fn default_bindings() -> Vec<(&'static str, &'static str)> {
    let mut bindings = shipped_bindings();
    // One slot per agent, unbound: the palette's `Agent: …` rows are what these
    // run, and each takes a key the same way any other action does. Right
    // after the other agent actions, so the Keybindings page keeps them together.
    let at = bindings
        .iter()
        .position(|(action, _)| *action == "NewAgentTab")
        .map_or(bindings.len(), |i| i + 1);
    bindings.splice(
        at..at,
        crate::actions::launch_action_names()
            .iter()
            .map(|(_, name)| (name.as_str(), "")),
    );
    bindings
}

fn shipped_bindings() -> Vec<(&'static str, &'static str)> {
    vec![
        ("NewTab", per_platform("secondary-t", "secondary-shift-t")),
        ("NewWindow", per_platform("secondary-n", "")),
        // Closing the window is how anterm is put away with every shell still
        // running, and Cmd+Q — the only chord a Mac user reaches for — stops
        // the server instead. Cmd+Shift+W is what "Close Window" means on
        // macOS and is free here. Off macOS that chord is `CloseActiveTab`.
        ("CloseWindow", per_platform("secondary-shift-w", "")),
        (
            "CloseActiveTab",
            per_platform("secondary-w", "secondary-shift-w"),
        ),
        ("RenameTab", ""),
        ("EditTabTags", "secondary-shift-l"),
        ("SwitchTab", ""),
        ("SwitchTag", ""),
        ("CloseOtherTabs", ""),
        ("CloseTabsToTheRight", ""),
        ("CopyWorkingDirectory", ""),
        ("MarkTabUnread", ""),
        ("HibernateTab", ""),
        ("ForkAgentSession", ""),
        ("ForkAgentSessionRight", ""),
        ("ForkAgentSessionLeft", ""),
        ("ForkAgentSessionDown", ""),
        ("ForkAgentSessionUp", ""),
        ("CopyAgentSessionId", ""),
        // ⌘⇧A for "agent" on macOS, where nothing else in the table or the
        // terminal holds it. Off macOS Ctrl+Shift+A is select-all in every
        // Linux terminal and in this prompt editor's Ctrl+A family, so it ships
        // unbound there rather than on a chord nobody would guess.
        ("NewAgentTab", per_platform("secondary-shift-a", "")),
        // U for "up next": the agent that has waited on you longest. Free in
        // the table on both sides, and a letter under Ctrl+Shift off macOS,
        // where window actions live. GNOME's IBus claims Ctrl+Shift+U for
        // Unicode entry while it is the active input method, and wins there:
        // that user rebinds this one, the price every Ctrl+Shift letter pays
        // to some input method or other.
        ("FocusNextAgentNeedingAttention", "secondary-shift-u"),
        (
            "SplitRight",
            per_platform("secondary-d", "secondary-shift-d"),
        ),
        ("SplitLeft", ""),
        (
            "SplitDown",
            per_platform("secondary-shift-d", "secondary-alt-shift-d"),
        ),
        ("SplitUp", ""),
        (
            "FocusNextPane",
            per_platform("secondary-]", "secondary-shift-]"),
        ),
        (
            "FocusPrevPane",
            per_platform("secondary-[", "secondary-shift-["),
        ),
        (
            "FocusPaneLeft",
            per_platform("secondary-alt-left", "alt-left"),
        ),
        (
            "FocusPaneRight",
            per_platform("secondary-alt-right", "alt-right"),
        ),
        ("FocusPaneUp", per_platform("secondary-alt-up", "alt-up")),
        (
            "FocusPaneDown",
            per_platform("secondary-alt-down", "alt-down"),
        ),
        ("ResizePaneLeft", ""),
        ("ResizePaneRight", ""),
        ("ResizePaneUp", ""),
        ("ResizePaneDown", ""),
        ("SwapPaneNext", ""),
        ("SwapPanePrev", ""),
        // Ctrl+Tab opens the most-recently-used switcher (a quick tap lands
        // on the tab you were just in); these step to the neighbouring tab
        // in strip order with no popup (#867). ⌘⇧] / ⌘⇧[ is iTerm2's and
        // Safari's chord. Off macOS Ctrl+Shift+] is FocusNextPane, so they
        // take Ctrl+PgDn / Ctrl+PgUp, the chord GNOME Terminal, xfce4-terminal
        // and every browser use for the same thing.
        ("NextTab", "ctrl-tab"),
        ("PrevTab", "ctrl-shift-tab"),
        (
            "SelectNextTab",
            per_platform("secondary-shift-]", "ctrl-pagedown"),
        ),
        (
            "SelectPrevTab",
            per_platform("secondary-shift-[", "ctrl-pageup"),
        ),
        // Reordering, not switching: bound by the user who wants it (the
        // palette and the Keybindings page both carry these), like the pane
        // swap pair above. No chord ships free enough to spend on both.
        ("MoveTabLeft", ""),
        ("MoveTabRight", ""),
        ("ActivateTab1", per_platform("secondary-1", "alt-1")),
        ("ActivateTab2", per_platform("secondary-2", "alt-2")),
        ("ActivateTab3", per_platform("secondary-3", "alt-3")),
        ("ActivateTab4", per_platform("secondary-4", "alt-4")),
        ("ActivateTab5", per_platform("secondary-5", "alt-5")),
        ("ActivateTab6", per_platform("secondary-6", "alt-6")),
        ("ActivateTab7", per_platform("secondary-7", "alt-7")),
        ("ActivateTab8", per_platform("secondary-8", "alt-8")),
        ("ActivateTab9", per_platform("secondary-9", "alt-9")),
        ("IncreaseFontSize", "secondary-="),
        ("DecreaseFontSize", "secondary--"),
        ("ResetFontSize", "secondary-0"),
        (
            "TogglePalette",
            per_platform("secondary-p", "secondary-shift-p"),
        ),
        // VS Code's ⌘P, which here is already Search Everywhere. ⌘O instead:
        // it is "Open…" in every Mac app, and nothing in the table or the
        // terminal holds it. Off macOS the obvious chords are gone — Ctrl+P
        // and Ctrl+O are bytes the shell is owed, Ctrl+Shift+P is Search
        // Everywhere and Ctrl+Shift+O the workspace switcher — so it ships
        // unbound there, like `NewWindow`, rather than on a chord nobody would
        // guess. The Files tab is a Tab press away inside the search either way.
        ("QuickOpenFile", per_platform("secondary-o", "")),
        (
            "ReopenClosedTab",
            per_platform("secondary-shift-t", "alt-shift-t"),
        ),
        ("ToggleMaximizePane", "secondary-shift-enter"),
        // The one default that sits on a bare function key, and it stays
        // there: F11 is the fullscreen chord Windows Terminal, GNOME Terminal
        // and konsole all train their users on, and no shell binds it — the
        // keys PSReadLine actually wants are F3, F7 and F8. gpui matches this
        // binding before the pane's key handler runs, so the terminal never
        // sees a bare F11; Shift/Ctrl/Alt+F11 are not bound and still reach
        // the PTY as `\E[23;<mods>~`, and the whole chord is one line of
        // config away from being retired. See
        // `f11_is_the_only_default_on_a_bare_function_key`.
        ("ToggleFullscreen", per_platform("secondary-enter", "f11")),
        ("ToggleTabSidebar", ""),
        (
            "ToggleLeftPanel",
            per_platform("secondary-b", "secondary-shift-b"),
        ),
        (
            "ToggleRightPanel",
            per_platform("secondary-j", "secondary-shift-j"),
        ),
        (
            "FindInTerminal",
            if cfg!(target_os = "macos") {
                "secondary-f"
            } else {
                "ctrl-shift-f"
            },
        ),
        (
            "FindNext",
            if cfg!(target_os = "macos") {
                "secondary-g"
            } else {
                "f3"
            },
        ),
        (
            "FindPrevious",
            if cfg!(target_os = "macos") {
                "secondary-shift-g"
            } else {
                "shift-f3"
            },
        ),
        (
            "ClearScrollback",
            per_platform("secondary-k", "secondary-shift-k"),
        ),
        ("InsertNewline", INSERT_NEWLINE_DEFAULT),
        // ⌘I for "input". Nothing in the table holds it, and no shell or agent
        // reads Cmd at all. Off macOS Ctrl+I is Tab, so the chord takes Shift
        // like every other terminal command there.
        (
            "ToggleComposer",
            per_platform("secondary-i", "secondary-shift-i"),
        ),
        ("CopyText", per_platform("", "ctrl-shift-c")),
        ("CopyRaw", "cmd-alt-c"),
        ("PasteText", paste_text_default()),
        ("AlternatePaste", alternate_paste_default()),
        ("OpenSettings", "secondary-,"),
        (
            "ShowKeyboardShortcuts",
            if cfg!(target_os = "macos") {
                "secondary-/"
            } else {
                ""
            },
        ),
        ("About", ""),
        ("CheckForUpdates", ""),
        ("OpenDocumentation", ""),
        ("OpenDiscord", ""),
        ("ReportIssue", ""),
        (
            "HideApp",
            if cfg!(target_os = "macos") {
                "secondary-h"
            } else {
                ""
            },
        ),
        (
            "HideOthers",
            if cfg!(target_os = "macos") {
                "secondary-alt-h"
            } else {
                ""
            },
        ),
        ("ShowAll", ""),
        (
            "MinimizeWindow",
            if cfg!(target_os = "macos") {
                "secondary-m"
            } else {
                ""
            },
        ),
        ("ZoomWindow", ""),
        // `secondary-enter` is `ToggleFullscreen` on macOS. That is not a
        // clash: `ScmCommit` binds inside the `ScmCommit` key context, so it
        // only wins while the caret sits in the commit box.
        ("ScmCommit", "secondary-enter"),
        ("ScmCommitAmend", ""),
        ("ScmStageAll", ""),
        ("ScmUnstageAll", ""),
        ("ScmDiscardAll", ""),
        ("ScmRefresh", ""),
        ("ScmSync", ""),
        ("ScmPush", ""),
        ("ScmPull", ""),
        ("ScmFetch", ""),
        ("ScmCheckoutBranch", ""),
        ("ScmCreateBranch", ""),
        ("ScmToggleGraph", ""),
        ("ToggleDiffViewMode", ""),
        ("ToggleSftp", ""),
        ("ShowSshForwards", ""),
        ("ToggleCodePanel", "secondary-shift-e"),
        // Deliberately unbound. Docking is the default and Esc already gets the
        // terminal back, so a default chord here would only be one more thing
        // competing for a two-key combination nobody asked for.
        ("ToggleDocumentFill", ""),
        ("DocumentWidthThird", ""),
        ("DocumentWidthHalf", ""),
        ("DocumentWidthTwoThirds", ""),
        // Unbound for the same reason, and because the obvious chords belong
        // to the editor that has the focus when you would reach for them.
        ("ToggleDocumentPreview", ""),
        ("ToggleDocumentWrap", ""),
        // Implemented, dispatchable, and until now unbindable: `set_binding`
        // only fills slots that exist here, so `"ShowRightPanelInfo": "ctrl-1"`
        // in config.json was dropped without a word, and the Keybindings page —
        // which reads this list — never showed them at all.
        ("ShowRightPanelInfo", ""),
        ("ShowRightPanelFiles", ""),
        ("ShowRightPanelSearch", ""),
        ("ShowRightPanelChanges", ""),
        ("EditorSave", "secondary-s"),
        ("EditorSaveAs", "secondary-shift-s"),
        ("EditorGoToLine", "ctrl-g"),
        // VS Code's chords. ⌘⇧O is the workspace switcher everywhere but the
        // editor, which takes it back while it has the focus (see
        // `action_context`). Back and Forward are ⌃- and ⌃⇧- on macOS; off
        // it ⌃- is Decrease Font Size, so Alt+←/→ as VS Code has them there.
        ("EditorGoToSymbol", "secondary-shift-o"),
        ("EditorNavigateBack", per_platform("ctrl--", "alt-left")),
        (
            "EditorNavigateForward",
            per_platform("ctrl-shift--", "alt-right"),
        ),
        // VS Code's Split Editor. Off macOS Ctrl+\ is the terminal's SIGQUIT,
        // which it keeps: the binding lives in the editor's context only.
        ("EditorSplitRight", "secondary-\\"),
        // Moving between the two groups, on the chords that move between
        // panes — at the edge of the editor, or with no split, they carry on
        // to the panes. Off macOS those are Alt+←/→, which Back and Forward
        // hold inside the editor.
        (
            "EditorFocusLeftGroup",
            per_platform("secondary-alt-left", "ctrl-alt-left"),
        ),
        (
            "EditorFocusRightGroup",
            per_platform("secondary-alt-right", "ctrl-alt-right"),
        ),
        // Unbound: ⌘N is New Window, and the editor's header has a + for it.
        ("EditorNewFile", ""),
        // The language-server commands. Unbound here — a default on a bare
        // function key would take it from the shell — and bound instead in
        // the editor's own context by `fixed_bindings`.
        ("EditorGoToDefinition", ""),
        ("EditorQuickFix", ""),
        ("EditorRenameSymbol", ""),
        ("EditorFormatDocument", ""),
        // The editor's text commands. Unbound: none of them has a chord in
        // VS Code either, except Join Lines, whose ⌃J is a fixed `CodeEditor`
        // binding on macOS (in the terminal it is a line feed).
        ("EditorTransformUppercase", ""),
        ("EditorTransformLowercase", ""),
        ("EditorTransformTitleCase", ""),
        ("EditorTrimTrailingWhitespace", ""),
        ("EditorJoinLines", ""),
        ("EditorRemoveSurroundingBrackets", ""),
        ("EditorFindReferences", ""),
        // Alt+F5 / Shift+Alt+F5 (VS Code's) are fixed `Input`-context
        // bindings: a default on a function key would hide it from the shell.
        ("EditorNextChange", ""),
        ("EditorPrevChange", ""),
        ("EditorRevertChange", ""),
        ("ToggleEditorGitGutter", ""),
        // Alt+F3 is a fixed `Input`-context binding, like Alt+F5.
        ("EditorPeekChange", ""),
        // VS Code's chord for its Problems panel.
        ("ToggleEditorProblems", "secondary-shift-m"),
        ("OpenSshProfiles", ""),
        ("RestartSshSession", "secondary-shift-r"),
        ("Quit", per_platform("secondary-q", "secondary-shift-q")),
    ]
}

/// Where an action sits on the Keybindings page, and what it is called there.
///
/// The page used to render `humanize_action`'s CamelCase split — English in a
/// three-locale app, and a fourth vocabulary on top of the menu bar, the
/// palette and the docs ("Toggle Maximize Pane" for what everything else calls
/// Zoom Pane). This routes both the name and the section through the strings
/// the rest of the app already uses.
pub fn action_entry(action: &str) -> (CommandGroup, String) {
    authored_entry(action).unwrap_or_else(|| {
        // An action with no entry still renders, under Application and with its
        // name split on capitals — a new action shows up on the page rather
        // than disappearing from it. `every_action_has_an_authored_name` is
        // what keeps that from being how the page looks.
        (CommandGroup::Application, humanize_action(action))
    })
}

fn authored_entry(action: &str) -> Option<(CommandGroup, String)> {
    // The numbered families are one row each in nine copies; a templated label
    // beats nine hand-written strings per locale.
    if let Some(n) = action.strip_prefix("ActivateTab") {
        return Some((
            CommandGroup::TabsPanes,
            t_fmt(L10nKey::KeybindGoToTab, &[("n", n)]),
        ));
    }
    if let Some(agent) = crate::actions::agent_for_launch_action(action) {
        return Some((
            CommandGroup::Agents,
            t_fmt(
                L10nKey::AppCmdAgentLaunchTitle,
                &[("name", agent.display_name())],
            ),
        ));
    }
    Some(match action {
        "NewTab" => (CommandGroup::TabsPanes, t(L10nKey::CmdNewTab).to_string()),
        "CloseActiveTab" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdClosePaneTab).to_string(),
        ),
        "RenameTab" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdRenameTab).to_string(),
        ),
        "EditTabTags" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdEditTabTags).to_string(),
        ),
        "SwitchTab" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdSwitchTab).to_string(),
        ),
        "SwitchTag" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdSwitchTag).to_string(),
        ),
        "CloseOtherTabs" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdCloseOtherTabs).to_string(),
        ),
        "CloseTabsToTheRight" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdCloseTabsToTheRight).to_string(),
        ),
        "CopyWorkingDirectory" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdCopyWorkingDirectory).to_string(),
        ),
        "MarkTabUnread" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdMarkTabAsUnread).to_string(),
        ),
        "HibernateTab" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdHibernateTab).to_string(),
        ),
        "ReopenClosedTab" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdReopenClosedTab).to_string(),
        ),
        "SplitRight" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdSplitRight).to_string(),
        ),
        "SplitLeft" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdSplitLeft).to_string(),
        ),
        "SplitDown" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdSplitDown).to_string(),
        ),
        "SplitUp" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdSplitUp).to_string(),
        ),
        "ToggleMaximizePane" => (CommandGroup::TabsPanes, t(L10nKey::CmdZoomPane).to_string()),
        "FocusNextPane" => (CommandGroup::TabsPanes, t(L10nKey::CmdNextPane).to_string()),
        "FocusPrevPane" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdPreviousPane).to_string(),
        ),
        "FocusPaneLeft" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdFocusPaneLeft).to_string(),
        ),
        "FocusPaneRight" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdFocusPaneRight).to_string(),
        ),
        "FocusPaneUp" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdFocusPaneUp).to_string(),
        ),
        "FocusPaneDown" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdFocusPaneDown).to_string(),
        ),
        "ResizePaneLeft" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdResizePaneLeft).to_string(),
        ),
        "ResizePaneRight" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdResizePaneRight).to_string(),
        ),
        "ResizePaneUp" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdResizePaneUp).to_string(),
        ),
        "ResizePaneDown" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdResizePaneDown).to_string(),
        ),
        "SwapPaneNext" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdSwapPaneNext).to_string(),
        ),
        "SwapPanePrev" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdSwapPanePrevious).to_string(),
        ),
        // `NextTab` / `PrevTab` keep their names — `Config::keybindings` is
        // keyed by them — but what they do is open the MRU switcher, so that
        // is what the page calls them. "Next Tab" belongs to the action that
        // actually goes to the next tab, the same one the palette runs.
        "NextTab" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdRecentTabSwitcher).to_string(),
        ),
        "PrevTab" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdRecentTabSwitcherReverse).to_string(),
        ),
        "SelectNextTab" => (CommandGroup::TabsPanes, t(L10nKey::CmdNextTab).to_string()),
        "SelectPrevTab" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdPreviousTab).to_string(),
        ),
        "MoveTabLeft" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdMoveTabLeft).to_string(),
        ),
        "MoveTabRight" => (
            CommandGroup::TabsPanes,
            t(L10nKey::CmdMoveTabRight).to_string(),
        ),
        "IncreaseFontSize" => (
            CommandGroup::View,
            t(L10nKey::AppMenuIncreaseFontSize).to_string(),
        ),
        "DecreaseFontSize" => (
            CommandGroup::View,
            t(L10nKey::AppMenuDecreaseFontSize).to_string(),
        ),
        "ResetFontSize" => (CommandGroup::View, t(L10nKey::CmdResetFontSize).to_string()),
        "ToggleFullscreen" => (
            CommandGroup::View,
            t(L10nKey::CmdEnterFullScreen).to_string(),
        ),
        "ToggleTabSidebar" => (
            CommandGroup::View,
            t(L10nKey::AppMenuTabBarPosition).to_string(),
        ),
        "ToggleLeftPanel" => (
            CommandGroup::View,
            t(L10nKey::AppMenuLeftSidebar).to_string(),
        ),
        "ToggleRightPanel" => (
            CommandGroup::View,
            t(L10nKey::AppMenuRightPanel).to_string(),
        ),
        "ToggleCodePanel" => (CommandGroup::View, t(L10nKey::AppMenuCodePanel).to_string()),
        "ToggleDocumentFill" => (
            CommandGroup::View,
            t(L10nKey::CmdToggleDocumentFill).to_string(),
        ),
        "DocumentWidthThird" => (
            CommandGroup::View,
            t(L10nKey::CmdDocumentWidthThird).to_string(),
        ),
        "DocumentWidthHalf" => (
            CommandGroup::View,
            t(L10nKey::CmdDocumentWidthHalf).to_string(),
        ),
        "DocumentWidthTwoThirds" => (
            CommandGroup::View,
            t(L10nKey::CmdDocumentWidthTwoThirds).to_string(),
        ),
        "ToggleDocumentPreview" => (
            CommandGroup::View,
            t(L10nKey::CmdToggleDocumentPreview).to_string(),
        ),
        "ToggleDocumentWrap" => (
            CommandGroup::View,
            t(L10nKey::CmdToggleDocumentWrap).to_string(),
        ),
        "ShowRightPanelInfo" => (
            CommandGroup::View,
            t(L10nKey::CmdRightPanelInfo).to_string(),
        ),
        "ShowRightPanelChanges" => (
            CommandGroup::View,
            t(L10nKey::CmdRightPanelChanges).to_string(),
        ),
        "ShowRightPanelFiles" => (
            CommandGroup::View,
            t(L10nKey::CmdRightPanelFiles).to_string(),
        ),
        "ShowRightPanelSearch" => (
            CommandGroup::View,
            t(L10nKey::CmdRightPanelSearch).to_string(),
        ),
        "FindInTerminal" => (
            CommandGroup::Terminal,
            t(L10nKey::CmdFindInTerminal).to_string(),
        ),
        "FindNext" => (CommandGroup::Terminal, t(L10nKey::CmdFindNext).to_string()),
        "ToggleComposer" => (
            CommandGroup::Terminal,
            t(L10nKey::CmdToggleComposer).to_string(),
        ),
        "FindPrevious" => (
            CommandGroup::Terminal,
            t(L10nKey::CmdFindPrevious).to_string(),
        ),
        "ClearScrollback" => (
            CommandGroup::Terminal,
            t(L10nKey::CmdClearScrollback).to_string(),
        ),
        "CopyText" => (CommandGroup::Terminal, t(L10nKey::CmdCopy).to_string()),
        "CopyRaw" => (CommandGroup::Terminal, t(L10nKey::CmdCopyRaw).to_string()),
        "PasteText" => (CommandGroup::Terminal, t(L10nKey::CmdPaste).to_string()),
        "AlternatePaste" => (
            CommandGroup::Terminal,
            t(L10nKey::CmdAlternatePaste).to_string(),
        ),
        "InsertNewline" => (
            CommandGroup::Terminal,
            t(L10nKey::KeybindInsertNewline).to_string(),
        ),
        "EditorSave" => (CommandGroup::Terminal, t(L10nKey::Save).to_string()),
        "EditorSaveAs" => (
            CommandGroup::Terminal,
            t(L10nKey::EditorSaveAsAction).to_string(),
        ),
        "EditorGoToLine" => (
            CommandGroup::Terminal,
            t(L10nKey::EditorGoToLineAction).to_string(),
        ),
        "EditorGoToDefinition" => (
            CommandGroup::Terminal,
            t(L10nKey::LspGoToDefinition).to_string(),
        ),
        "EditorQuickFix" => (CommandGroup::Terminal, t(L10nKey::LspQuickFix).to_string()),
        "EditorRenameSymbol" => (
            CommandGroup::Terminal,
            t(L10nKey::LspRenameSymbolAction).to_string(),
        ),
        "EditorFormatDocument" => (
            CommandGroup::Terminal,
            t(L10nKey::LspFormatDocument).to_string(),
        ),
        "EditorTransformUppercase" => (
            CommandGroup::Terminal,
            t(L10nKey::CmdEditorTransformUppercase).to_string(),
        ),
        "EditorTransformLowercase" => (
            CommandGroup::Terminal,
            t(L10nKey::CmdEditorTransformLowercase).to_string(),
        ),
        "EditorTransformTitleCase" => (
            CommandGroup::Terminal,
            t(L10nKey::CmdEditorTransformTitleCase).to_string(),
        ),
        "EditorTrimTrailingWhitespace" => (
            CommandGroup::Terminal,
            t(L10nKey::CmdEditorTrimTrailingWhitespace).to_string(),
        ),
        "EditorJoinLines" => (
            CommandGroup::Terminal,
            t(L10nKey::CmdEditorJoinLines).to_string(),
        ),
        "EditorRemoveSurroundingBrackets" => (
            CommandGroup::Terminal,
            t(L10nKey::CmdEditorRemoveSurroundingBrackets).to_string(),
        ),
        "EditorFindReferences" => (
            CommandGroup::Terminal,
            t(L10nKey::LspFindReferences).to_string(),
        ),
        "EditorGoToSymbol" => (
            CommandGroup::Terminal,
            t(L10nKey::EditorGoToSymbolAction).to_string(),
        ),
        "EditorNavigateBack" => (
            CommandGroup::Terminal,
            t(L10nKey::EditorNavigateBack).to_string(),
        ),
        "EditorNavigateForward" => (
            CommandGroup::Terminal,
            t(L10nKey::EditorNavigateForward).to_string(),
        ),
        "EditorSplitRight" => (
            CommandGroup::Terminal,
            t(L10nKey::EditorSplitRight).to_string(),
        ),
        "EditorFocusLeftGroup" => (
            CommandGroup::Terminal,
            t(L10nKey::EditorFocusLeftGroup).to_string(),
        ),
        "EditorFocusRightGroup" => (
            CommandGroup::Terminal,
            t(L10nKey::EditorFocusRightGroup).to_string(),
        ),
        "EditorNewFile" => (
            CommandGroup::Terminal,
            t(L10nKey::EditorNewFile).to_string(),
        ),
        "EditorNextChange" => (
            CommandGroup::Terminal,
            t(L10nKey::EditorGitNextChange).to_string(),
        ),
        "EditorPrevChange" => (
            CommandGroup::Terminal,
            t(L10nKey::EditorGitPrevChange).to_string(),
        ),
        "EditorRevertChange" => (
            CommandGroup::Terminal,
            t(L10nKey::EditorGitRevertChange).to_string(),
        ),
        "ToggleEditorGitGutter" => (
            CommandGroup::Terminal,
            t(L10nKey::EditorGitToggleGutter).to_string(),
        ),
        "EditorPeekChange" => (
            CommandGroup::Terminal,
            t(L10nKey::EditorGitPeekChange).to_string(),
        ),
        "ToggleEditorProblems" => (
            CommandGroup::Terminal,
            t(L10nKey::EditorProblemsToggle).to_string(),
        ),
        "OpenSshProfiles" => (
            CommandGroup::Ssh,
            t(L10nKey::CmdSshManageProfiles).to_string(),
        ),
        "RestartSshSession" => (CommandGroup::Ssh, t(L10nKey::CmdSshReconnect).to_string()),
        "ToggleSftp" => (CommandGroup::Ssh, t(L10nKey::CmdSshRemoteFiles).to_string()),
        "ShowSshForwards" => (
            CommandGroup::Ssh,
            t(L10nKey::CmdSshPortForwarding).to_string(),
        ),
        "ForkAgentSession" => (CommandGroup::Agents, t(L10nKey::CmdForkSession).to_string()),
        "ForkAgentSessionRight" => (
            CommandGroup::Agents,
            t(L10nKey::KeybindForkSessionRight).to_string(),
        ),
        "ForkAgentSessionLeft" => (
            CommandGroup::Agents,
            t(L10nKey::KeybindForkSessionLeft).to_string(),
        ),
        "ForkAgentSessionDown" => (
            CommandGroup::Agents,
            t(L10nKey::KeybindForkSessionDown).to_string(),
        ),
        "ForkAgentSessionUp" => (
            CommandGroup::Agents,
            t(L10nKey::KeybindForkSessionUp).to_string(),
        ),
        "CopyAgentSessionId" => (
            CommandGroup::Agents,
            t(L10nKey::CmdCopySessionId).to_string(),
        ),
        "NewAgentTab" => (CommandGroup::Agents, t(L10nKey::CmdNewAgentTab).to_string()),
        "FocusNextAgentNeedingAttention" => (
            CommandGroup::Agents,
            t(L10nKey::CmdNextAgentNeedingAttention).to_string(),
        ),
        "TogglePalette" => (
            CommandGroup::Application,
            t(L10nKey::AppMenuSearchEverywhere).to_string(),
        ),
        "QuickOpenFile" => (CommandGroup::View, t(L10nKey::CmdGoToFile).to_string()),
        "NewWindow" => (
            CommandGroup::Application,
            t(L10nKey::CmdNewWindow).to_string(),
        ),
        "CloseWindow" => (
            CommandGroup::Application,
            t(L10nKey::CmdCloseWindow).to_string(),
        ),
        "OpenSettings" => (
            CommandGroup::Application,
            t(L10nKey::CmdSettings).to_string(),
        ),
        "ShowKeyboardShortcuts" => (
            CommandGroup::Application,
            t(L10nKey::CmdKeyboardShortcuts).to_string(),
        ),
        "About" => (
            CommandGroup::Application,
            t(L10nKey::CmdAboutAnterm).to_string(),
        ),
        "CheckForUpdates" => (
            CommandGroup::Application,
            t(L10nKey::CmdCheckForUpdates).to_string(),
        ),
        "OpenDocumentation" => (
            CommandGroup::Application,
            t(L10nKey::CmdDocumentation).to_string(),
        ),
        "OpenDiscord" => (
            CommandGroup::Application,
            t(L10nKey::CmdJoinDiscord).to_string(),
        ),
        "ReportIssue" => (
            CommandGroup::Application,
            t(L10nKey::CmdReportIssue).to_string(),
        ),
        "HideApp" => (
            CommandGroup::Application,
            t(L10nKey::AppMenuHideApp).to_string(),
        ),
        "HideOthers" => (
            CommandGroup::Application,
            t(L10nKey::AppMenuHideOthers).to_string(),
        ),
        "ShowAll" => (
            CommandGroup::Application,
            t(L10nKey::AppMenuShowAll).to_string(),
        ),
        "MinimizeWindow" => (
            CommandGroup::Application,
            t(L10nKey::AppMenuMinimize).to_string(),
        ),
        "ZoomWindow" => (
            CommandGroup::Application,
            t(L10nKey::AppMenuZoom).to_string(),
        ),
        "Quit" => (
            CommandGroup::Application,
            t(L10nKey::CmdQuitAnterm).to_string(),
        ),
        // The source control verbs wear their palette names, so the
        // Keybindings page and the palette agree on what a chord does.
        "ScmCommit" => (CommandGroup::Git, t(L10nKey::CmdGitCommit).to_string()),
        "ScmCommitAmend" => (
            CommandGroup::Git,
            t(L10nKey::ScmAmendLastCommit).to_string(),
        ),
        "ScmStageAll" => (CommandGroup::Git, t(L10nKey::CmdGitStageAll).to_string()),
        "ScmUnstageAll" => (CommandGroup::Git, t(L10nKey::CmdGitUnstageAll).to_string()),
        "ScmDiscardAll" => (CommandGroup::Git, t(L10nKey::CmdGitDiscardAll).to_string()),
        "ScmRefresh" => (CommandGroup::Git, t(L10nKey::ScmRefresh).to_string()),
        "ScmSync" => (CommandGroup::Git, t(L10nKey::CmdGitSync).to_string()),
        "ScmPush" => (CommandGroup::Git, t(L10nKey::CmdGitPush).to_string()),
        "ScmPull" => (CommandGroup::Git, t(L10nKey::CmdGitPull).to_string()),
        "ScmFetch" => (CommandGroup::Git, t(L10nKey::CmdGitFetch).to_string()),
        "ScmCheckoutBranch" => (CommandGroup::Git, t(L10nKey::CmdGitCheckoutTo).to_string()),
        "ScmCreateBranch" => (
            CommandGroup::Git,
            t(L10nKey::CmdGitCreateBranch).to_string(),
        ),
        "ScmToggleGraph" => (CommandGroup::Git, t(L10nKey::CmdGitToggleGraph).to_string()),
        "ToggleDiffViewMode" => (
            CommandGroup::View,
            t(L10nKey::CmdToggleDiffViewMode).to_string(),
        ),
        _ => return None,
    })
}

/// One action and the chords it answers to, split by where they came from:
/// the ones it ships with — its default, or the preset's in its place — and
/// the ones `config.json` names.
///
/// Kept apart because they install apart. gpui resolves two bindings on one
/// chord to the one added last, and a chord the user asked for by name has to
/// be that one, wherever its action sits in the table.
struct ActionChords {
    action: String,
    shipped: Vec<String>,
    configured: Vec<String>,
}

fn resolve_chords(
    preset: &str,
    prefix: &str,
    overrides: &std::collections::HashMap<String, KeybindingOverride>,
) -> Vec<ActionChords> {
    let mut resolved: Vec<ActionChords> = default_bindings()
        .into_iter()
        .map(|(action, key)| ActionChords {
            action: action.to_string(),
            shipped: [key]
                .into_iter()
                .filter(|k| !k.is_empty())
                .map(str::to_string)
                .collect(),
            configured: Vec::new(),
        })
        .collect();
    // A preset is a scheme rather than an addition: its chord takes the
    // default's place.
    for (action, key) in preset_bindings(preset, prefix) {
        if let Some(slot) = resolved.iter_mut().find(|s| s.action == action) {
            slot.shipped = vec![key];
        }
    }
    for (action, value) in overrides {
        let Some(slot) = resolved.iter_mut().find(|s| s.action == *action) else {
            // A hand-edited config.json with a typo used to vanish into this
            // branch. The Keybindings page lists every name that works.
            log::warn!("keybinding for unknown action {action:?} ignored");
            continue;
        };
        match value {
            // `""` has always been how a config retires a chord — the docs
            // spell `"AlternatePaste": ""`, and Settings wrote it for an action
            // whose chord was taken — so it goes on unbinding the action.
            KeybindingOverride::Add(key) if key.is_empty() => {
                slot.shipped.clear();
            }
            // A chord beside the ones the action has, not in place of them
            // (#868): `"NextTab": "cmd-shift-]"` is a second way to switch tabs,
            // and Ctrl+Tab going dead because of it was the bug.
            KeybindingOverride::Add(key) => {
                if !slot.shipped.iter().any(|k| same_chord(k, key)) {
                    slot.configured.push(key.clone());
                }
            }
            KeybindingOverride::Exact(keys) => {
                slot.shipped.clear();
                for key in keys {
                    if !key.is_empty() && !slot.configured.iter().any(|k| same_chord(k, key)) {
                        slot.configured.push(key.clone());
                    }
                }
            }
        }
    }
    resolved
}

/// Every chord as an `(action, chord)` pair, in the order the keymap is built
/// from: all the shipped chords, then all the configured ones, so that a
/// configured chord landing on another action's default is the binding that
/// wins it. An action with no chord at all has no pair.
fn flatten_chords(resolved: &[ActionChords]) -> Vec<(String, String)> {
    let pairs = |pick: fn(&ActionChords) -> &Vec<String>| {
        resolved
            .iter()
            .flat_map(move |s| pick(s).iter().map(|k| (s.action.clone(), k.clone())))
    };
    pairs(|s| &s.shipped)
        .chain(pairs(|s| &s.configured))
        .collect()
}

pub fn effective_bindings(cx: &App) -> Vec<(String, String)> {
    let cfg = cx.global::<Config>();
    flatten_chords(&resolve_chords(
        &cfg.keybinding_preset,
        &cfg.prefix,
        &cfg.keybindings,
    ))
}

/// Every action in table order with all of its chords, shipped first — an
/// empty list for an action that has none. What a page listing the actions
/// reads; the keymap reads [`effective_bindings`].
pub fn effective_chords(cx: &App) -> Vec<(String, Vec<String>)> {
    let cfg = cx.global::<Config>();
    resolve_chords(&cfg.keybinding_preset, &cfg.prefix, &cfg.keybindings)
        .into_iter()
        .map(|mut s| {
            s.shipped.append(&mut s.configured);
            (s.action, s.shipped)
        })
        .collect()
}

fn preset_bindings(preset: &str, prefix: &str) -> Vec<(String, String)> {
    match preset {
        "tmux" => tmux_preset(prefix),
        _ => Vec::new(),
    }
}

fn tmux_preset(prefix: &str) -> Vec<(String, String)> {
    let p = |key: &str| format!("{prefix} {key}");
    [
        ("NewTab", p("c")),
        ("CloseActiveTab", p("x")),
        ("SplitRight", p("%")),
        ("SplitDown", p("\"")),
        ("FocusPaneLeft", p("left")),
        ("FocusPaneRight", p("right")),
        ("FocusPaneUp", p("up")),
        ("FocusPaneDown", p("down")),
        ("ResizePaneLeft", p("ctrl-left")),
        ("ResizePaneRight", p("ctrl-right")),
        ("ResizePaneUp", p("ctrl-up")),
        ("ResizePaneDown", p("ctrl-down")),
        ("SwapPanePrev", p("{")),
        ("SwapPaneNext", p("}")),
        ("ToggleMaximizePane", p("z")),
        ("FocusNextPane", p("o")),
        ("FocusPrevPane", p(";")),
        // tmux's `next-window` / `previous-window`: straight to the
        // neighbour, no chooser. The MRU switcher it used to open never
        // committed here — the modifier it waits on is not held (#867).
        ("SelectNextTab", p("n")),
        ("SelectPrevTab", p("p")),
        ("ActivateTab1", p("1")),
        ("ActivateTab2", p("2")),
        ("ActivateTab3", p("3")),
        ("ActivateTab4", p("4")),
        ("ActivateTab5", p("5")),
        ("ActivateTab6", p("6")),
        ("ActivateTab7", p("7")),
        ("ActivateTab8", p("8")),
        ("ActivateTab9", p("9")),
    ]
    .into_iter()
    .map(|(a, k)| (a.to_string(), k))
    .collect()
}

pub fn effective_key(action: &str, cx: &App) -> Option<String> {
    effective_bindings(cx)
        .into_iter()
        .find(|(a, _)| a == action)
        .map(|(_, k)| k)
        .filter(|k| !k.is_empty())
}

/// The modifier hold that raises the tab-number badges — the one the tab
/// activation chords are really on.
///
/// It used to be assumed: a bare secondary, ⌘ on macOS and Ctrl everywhere
/// else. That is one platform right and one wrong. Off macOS the nine ship on
/// Alt, so holding Alt raised nothing while Ctrl, which activates no tab there
/// at all, lit up every number; and any rebind — `"ActivateTab1": "alt-1"` on
/// macOS is the same complaint the other way round — moved the chords out from
/// under a hint that stayed where it was. Read from `ActivateTab1`, the chord
/// the first badge is drawn for, and matched against the held modifiers
/// exactly, so a wrong modifier, or Shift added mid-hold, takes them down.
///
/// `None` when there is nothing to advertise: an unbound or retired
/// `ActivateTab1`, a chord with no modifier (a bare `1` is not a hold), a key
/// that is not the digit the badge would draw (`alt-q` does activate tab 1, but
/// holding Alt would then be promising a number pad no key answers), or the
/// tmux preset's two-chord `prefix 1` — a sequence has no hold to watch, and
/// raising the badges on the bare prefix would point at half a chord.
pub fn tab_badge_hold(cx: &App) -> Option<Modifiers> {
    let spec = effective_key("ActivateTab1", cx)?;
    let mut chords = spec.split_whitespace();
    let (chord, None) = (chords.next()?, chords.next()) else {
        return None;
    };
    let ks = Keystroke::parse_with_os(chord, crate::platform::current_os()).ok()?;
    (ks.key == "1" && ks.modifiers.modified()).then_some(ks.modifiers)
}

pub fn spec_from_keystroke(ks: &Keystroke) -> Option<String> {
    if matches!(
        ks.key.as_str(),
        "shift" | "control" | "alt" | "platform" | "function" | "cmd" | "ctrl"
    ) {
        return None;
    }
    let m = &ks.modifiers;
    let mut parts: Vec<&str> = Vec::new();
    #[cfg(target_os = "macos")]
    {
        if m.platform {
            parts.push("secondary");
        }
        if m.control {
            parts.push("ctrl");
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        if m.control {
            parts.push("secondary");
        }
        if m.platform {
            parts.push("cmd");
        }
    }
    if m.alt {
        parts.push("alt");
    }
    if m.shift {
        parts.push("shift");
    }
    if m.function {
        parts.push("fn");
    }
    let mut spec = String::new();
    for part in parts {
        spec.push_str(part);
        spec.push('-');
    }
    spec.push_str(&ks.key);
    Some(spec)
}

/// The glyph a US keyboard prints on the shifted half of every key that is not
/// a letter — `shift-]` types `}`, `shift-2` types `@`.
const SHIFTED_GLYPHS: [(char, char); 21] = [
    ('`', '~'),
    ('1', '!'),
    ('2', '@'),
    ('3', '#'),
    ('4', '$'),
    ('5', '%'),
    ('6', '^'),
    ('7', '&'),
    ('8', '*'),
    ('9', '('),
    ('0', ')'),
    ('-', '_'),
    ('=', '+'),
    ('[', '{'),
    (']', '}'),
    ('\\', '|'),
    (';', ':'),
    ('\'', '"'),
    (',', '<'),
    ('.', '>'),
    ('/', '?'),
];

/// Rewrites one chord into the shape the platform actually delivers, or
/// returns `None` when it is already that shape.
///
/// Every gpui backend carries Shift as a modifier for letters only. Over the
/// digits and the punctuation it spends the modifier on the character instead
/// and clears the flag: that is the `chars_with_shift` branch on macOS,
/// `need_to_convert_to_shifted_key` in the Windows mapper, and Linux's "we
/// only include the shift for upper-case letters by convention". So the chord
/// written `secondary-shift-]` arrives as `secondary-}` and the two never
/// meet — the binding installs and is unreachable, which is why Ctrl+Shift+]
/// stopped cycling panes off macOS and why hand-editing a config to that
/// spelling changed nothing (#750).
///
/// Folding the spec the same way on the lookup side closes the round trip.
/// The recorder already writes `secondary-}`, because that is what
/// `spec_from_keystroke` was handed; a default or a config written the other
/// way now installs the very same binding, so both spellings keep working and
/// nobody's config comes undone. `secondary-shift-}`, which people also reach
/// for, folds onto it too — the flag is simply redundant there.
///
/// The table is US-layout, and a chord it does not know is left alone rather
/// than guessed at. It cannot be layout-aware from here: `?` is Shift+ß on a
/// German keyboard and Shift+, on a French one, and the only mapper that knows
/// which is gpui's, reached through `KeyBinding::load` rather than the
/// `KeyBinding::new` that `make_binding` uses — and it exists on Windows only,
/// where `MacKeyboardMapper` does no shift folding at all. So off a US layout
/// the fold is a widening that may not land: `ctrl-shift-4` picks up `ctrl-$`,
/// which on AZERTY is a key of its own. Harmless, because the fold only ever
/// adds — the spec as written stays bound whatever the layout — but it is why
/// this is not the last word on #750.
///
/// The fold is *added* to a spec's bindings, never substituted for them,
/// because the convention is not universal. Windows leaves Shift intact on
/// the numpad's `/ * + -`: `need_to_convert_to_shifted_key` lists the OEM
/// keys and `VK_0..VK_9` but none of `VK_DIVIDE`/`VK_MULTIPLY`/`VK_ADD`/
/// `VK_SUBTRACT`, so `get_keystroke_key` falls through to `get_key_from_vkey`
/// and Ctrl+Shift+numpad-/ really does arrive as `ctrl-shift-/`. Substituting
/// `ctrl-?` for it would unbind a chord that works today — the same class of
/// bug this is fixing. macOS clears the flag there anyway (the numpad glyph
/// is the same shifted or not, so `chars_with_shift` takes the branch that
/// drops it) and Linux names those keys `divide`/`multiply`/`add`/`subtract`,
/// which are not one character and never reach this.
fn fold_shift_into_glyph(chord: &str) -> Option<String> {
    let mut ks = Keystroke::parse_with_os(chord, crate::platform::current_os()).ok()?;
    if !ks.modifiers.shift {
        return None;
    }
    let mut chars = ks.key.chars();
    let (Some(key), None) = (chars.next(), chars.next()) else {
        return None;
    };
    if let Some((_, shifted)) = SHIFTED_GLYPHS.iter().find(|(plain, _)| *plain == key) {
        ks.key = shifted.to_string();
    } else if !SHIFTED_GLYPHS.iter().any(|(_, shifted)| *shifted == key) {
        return None;
    }
    ks.modifiers.shift = false;
    spec_from_keystroke(&ks)
}

/// `spec` with every chord folded, or `None` when it is already the shape the
/// platform delivers and there is nothing to add.
pub fn folded_spec(spec: &str) -> Option<String> {
    let mut folded = String::with_capacity(spec.len());
    let mut changed = false;
    for chord in spec.split_whitespace() {
        if !folded.is_empty() {
            folded.push(' ');
        }
        match fold_shift_into_glyph(chord) {
            Some(chord) => {
                folded.push_str(&chord);
                changed = true;
            }
            None => folded.push_str(chord),
        }
    }
    changed.then_some(folded)
}

/// Whether two specs claim the same keystroke — spelled the same way, or one
/// the fold of the other. `secondary-shift-]` and `secondary-}` are one chord
/// written twice, and a rebind that cannot see that leaves both installed on
/// it, where which one fires is arbitrary (#750).
pub fn same_chord(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (folded_a, folded_b) = (folded_spec(a), folded_spec(b));
    folded_a.as_deref() == Some(b)
        || folded_b.as_deref() == Some(a)
        || (folded_a.is_some() && folded_a == folded_b)
}

pub fn key_chords(spec: &str) -> Vec<Vec<String>> {
    spec.split_whitespace().map(key_tokens).collect()
}

pub fn key_tokens(spec: &str) -> Vec<String> {
    #[cfg(target_os = "macos")]
    const MODS: [(&str, &str); 6] = [
        ("secondary", "⌘"),
        ("cmd", "⌘"),
        ("ctrl", "⌃"),
        ("alt", "⌥"),
        ("shift", "⇧"),
        ("fn", "fn"),
    ];
    #[cfg(not(target_os = "macos"))]
    const MODS: [(&str, &str); 6] = [
        ("secondary", "Ctrl"),
        ("cmd", "Win"),
        ("ctrl", "Ctrl"),
        ("alt", "Alt"),
        ("shift", "Shift"),
        ("fn", "Fn"),
    ];
    let mut rest = spec;
    let mut tokens = Vec::new();
    'outer: loop {
        for (name, glyph) in MODS {
            let prefix = format!("{name}-");
            if let Some(stripped) = rest.strip_prefix(&prefix) {
                if !stripped.is_empty() {
                    tokens.push(glyph.to_string());
                    rest = stripped;
                    continue 'outer;
                }
            }
        }
        break;
    }
    // macOS names modifiers in one fixed order — ⌃ ⌥ ⇧ ⌘ — whatever order a
    // binding was written in; `secondary-shift-o` read `⌘ ⇧ O` here while the
    // menu bar said `⇧⌘O` for the same key.
    #[cfg(target_os = "macos")]
    tokens.sort_by_key(|glyph| {
        ["fn", "⌃", "⌥", "⇧", "⌘"]
            .iter()
            .position(|g| g == glyph)
            .unwrap_or(usize::MAX)
    });
    tokens.push(key_glyph(rest));
    tokens
}

fn key_glyph(key: &str) -> String {
    match key {
        // The glyph every hint in the app already draws for Return; this one
        // alone said `⏎`, so the same key looked different in a row's
        // shortcut and in the footer under it.
        "enter" | "return" => "↵".into(),
        "tab" => "⇥".into(),
        "space" => "Space".into(),
        "escape" | "esc" => "⎋".into(),
        "backspace" => "⌫".into(),
        "up" => "↑".into(),
        "down" => "↓".into(),
        "left" => "←".into(),
        "right" => "→".into(),
        "-" => "−".into(),
        other => other.to_uppercase(),
    }
}

fn keystroke_is_valid(s: &str) -> bool {
    let mut any = false;
    for token in s.split_whitespace() {
        any = true;
        if gpui::Keystroke::parse_with_os(token, crate::platform::current_os()).is_err() {
            return false;
        }
    }
    any
}

fn action_context(action: &str) -> Option<&'static str> {
    match action {
        "FindInTerminal" | "FindNext" | "FindPrevious" | "ClearScrollback" | "InsertNewline"
        | "CopyText" | "CopyRaw" | "PasteText" | "ToggleComposer" => Some("Terminal"),
        // `alt_screen` is declared by the pane whenever a full-screen program
        // owns the grid, so this binding is simply absent there and Ctrl+V
        // carries on to the PTY as SYN (#677).
        "AlternatePaste" => Some("Terminal && !alt_screen"),
        "ScmCommit" | "ScmCommitAmend" => Some("ScmCommit"),
        "EditorGoToSymbol"
        | "EditorNavigateBack"
        | "EditorNavigateForward"
        | "EditorSplitRight"
        | "EditorFocusLeftGroup"
        | "EditorFocusRightGroup" => Some(crate::actions::NAV_KEY_CONTEXT),
        _ => None,
    }
}

fn resolve_secondary(spec: &str) -> String {
    #[cfg(target_os = "macos")]
    const SEC: &str = "cmd";
    #[cfg(not(target_os = "macos"))]
    const SEC: &str = "ctrl";

    spec.split_whitespace()
        .map(|chord| {
            chord
                .split('-')
                .map(|part| {
                    if part.eq_ignore_ascii_case("secondary") {
                        SEC
                    } else {
                        part
                    }
                })
                .collect::<Vec<_>>()
                .join("-")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn intern_keystroke(s: &str) -> &'static str {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    let normalized = resolve_secondary(s);
    static INTERNED: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut set = INTERNED.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&existing) = set.get(normalized.as_str()) {
        return existing;
    }
    let leaked: &'static str = Box::leak(normalized.into_boxed_str());
    set.insert(leaked);
    leaked
}

fn kb<A: gpui::Action>(keystroke: &str, action: A, context: Option<&str>) -> KeyBinding {
    KeyBinding::new(intern_keystroke(keystroke), action, context)
}

fn make_binding(action: &str, keystroke: &str) -> Option<KeyBinding> {
    let keystroke = intern_keystroke(keystroke);
    if let Some(agent) = crate::actions::agent_for_launch_action(action) {
        return Some(KeyBinding::new(keystroke, LaunchAgent { agent }, None));
    }
    Some(match action {
        "NewTab" => KeyBinding::new(keystroke, NewTab, None),
        "NewWindow" => KeyBinding::new(keystroke, NewWindow, None),
        "CloseWindow" => KeyBinding::new(keystroke, CloseWindow, None),
        "CloseActiveTab" => KeyBinding::new(keystroke, CloseActiveTab, None),
        "RenameTab" => KeyBinding::new(keystroke, RenameTab, None),
        "EditTabTags" => KeyBinding::new(keystroke, EditTabTags, None),
        "SwitchTab" => KeyBinding::new(keystroke, SwitchTab, None),
        "SwitchTag" => KeyBinding::new(keystroke, SwitchTag, None),
        "CloseOtherTabs" => KeyBinding::new(keystroke, CloseOtherTabs, None),
        "CloseTabsToTheRight" => KeyBinding::new(keystroke, CloseTabsToTheRight, None),
        "CopyWorkingDirectory" => KeyBinding::new(keystroke, CopyWorkingDirectory, None),
        "MarkTabUnread" => KeyBinding::new(keystroke, MarkTabUnread, None),
        "HibernateTab" => KeyBinding::new(keystroke, HibernateTab, None),
        "ForkAgentSession" => KeyBinding::new(keystroke, ForkAgentSession, None),
        "ForkAgentSessionRight" => KeyBinding::new(keystroke, ForkAgentSessionRight, None),
        "ForkAgentSessionLeft" => KeyBinding::new(keystroke, ForkAgentSessionLeft, None),
        "ForkAgentSessionDown" => KeyBinding::new(keystroke, ForkAgentSessionDown, None),
        "ForkAgentSessionUp" => KeyBinding::new(keystroke, ForkAgentSessionUp, None),
        "CopyAgentSessionId" => KeyBinding::new(keystroke, CopyAgentSessionId, None),
        "NewAgentTab" => KeyBinding::new(keystroke, NewAgentTab, None),
        "FocusNextAgentNeedingAttention" => {
            KeyBinding::new(keystroke, FocusNextAgentNeedingAttention, None)
        }
        "SplitRight" => KeyBinding::new(keystroke, SplitRight, None),
        "SplitLeft" => KeyBinding::new(keystroke, SplitLeft, None),
        "SplitDown" => KeyBinding::new(keystroke, SplitDown, None),
        "SplitUp" => KeyBinding::new(keystroke, SplitUp, None),
        "FocusNextPane" => KeyBinding::new(keystroke, FocusNextPane, None),
        "FocusPrevPane" => KeyBinding::new(keystroke, FocusPrevPane, None),
        "FocusPaneLeft" => KeyBinding::new(keystroke, FocusPaneLeft, None),
        "FocusPaneRight" => KeyBinding::new(keystroke, FocusPaneRight, None),
        "FocusPaneUp" => KeyBinding::new(keystroke, FocusPaneUp, None),
        "FocusPaneDown" => KeyBinding::new(keystroke, FocusPaneDown, None),
        "ResizePaneLeft" => KeyBinding::new(keystroke, ResizePaneLeft, None),
        "ResizePaneRight" => KeyBinding::new(keystroke, ResizePaneRight, None),
        "ResizePaneUp" => KeyBinding::new(keystroke, ResizePaneUp, None),
        "ResizePaneDown" => KeyBinding::new(keystroke, ResizePaneDown, None),
        "SwapPaneNext" => KeyBinding::new(keystroke, SwapPaneNext, None),
        "SwapPanePrev" => KeyBinding::new(keystroke, SwapPanePrev, None),
        "NextTab" => KeyBinding::new(keystroke, NextTab, None),
        "PrevTab" => KeyBinding::new(keystroke, PrevTab, None),
        "SelectNextTab" => KeyBinding::new(keystroke, SelectNextTab, None),
        "SelectPrevTab" => KeyBinding::new(keystroke, SelectPrevTab, None),
        "MoveTabLeft" => KeyBinding::new(keystroke, MoveTabLeft, None),
        "MoveTabRight" => KeyBinding::new(keystroke, MoveTabRight, None),
        "ActivateTab1" => KeyBinding::new(keystroke, ActivateTab1, None),
        "ActivateTab2" => KeyBinding::new(keystroke, ActivateTab2, None),
        "ActivateTab3" => KeyBinding::new(keystroke, ActivateTab3, None),
        "ActivateTab4" => KeyBinding::new(keystroke, ActivateTab4, None),
        "ActivateTab5" => KeyBinding::new(keystroke, ActivateTab5, None),
        "ActivateTab6" => KeyBinding::new(keystroke, ActivateTab6, None),
        "ActivateTab7" => KeyBinding::new(keystroke, ActivateTab7, None),
        "ActivateTab8" => KeyBinding::new(keystroke, ActivateTab8, None),
        "ActivateTab9" => KeyBinding::new(keystroke, ActivateTab9, None),
        "IncreaseFontSize" => KeyBinding::new(keystroke, IncreaseFontSize, None),
        "DecreaseFontSize" => KeyBinding::new(keystroke, DecreaseFontSize, None),
        "ResetFontSize" => KeyBinding::new(keystroke, ResetFontSize, None),
        "TogglePalette" => KeyBinding::new(keystroke, TogglePalette, None),
        "QuickOpenFile" => KeyBinding::new(keystroke, QuickOpenFile, None),
        "ReopenClosedTab" => KeyBinding::new(keystroke, ReopenClosedTab, None),
        "ToggleMaximizePane" => KeyBinding::new(keystroke, ToggleMaximizePane, None),
        "ToggleFullscreen" => KeyBinding::new(keystroke, ToggleFullscreen, None),
        "ToggleTabSidebar" => KeyBinding::new(keystroke, ToggleTabSidebar, None),
        "ToggleLeftPanel" => KeyBinding::new(keystroke, ToggleLeftPanel, None),
        "ToggleRightPanel" => KeyBinding::new(keystroke, ToggleRightPanel, None),
        "ShowRightPanelInfo" => KeyBinding::new(keystroke, ShowRightPanelInfo, None),
        "ShowRightPanelChanges" => KeyBinding::new(keystroke, ShowRightPanelChanges, None),
        "ShowRightPanelFiles" => KeyBinding::new(keystroke, ShowRightPanelFiles, None),
        "ShowRightPanelSearch" => KeyBinding::new(keystroke, ShowRightPanelSearch, None),
        "ScmCommit" => KeyBinding::new(keystroke, ScmCommit, action_context(action)),
        "ScmCommitAmend" => KeyBinding::new(keystroke, ScmCommitAmend, action_context(action)),
        "ScmStageAll" => KeyBinding::new(keystroke, ScmStageAll, None),
        "ScmUnstageAll" => KeyBinding::new(keystroke, ScmUnstageAll, None),
        "ScmDiscardAll" => KeyBinding::new(keystroke, ScmDiscardAll, None),
        "ScmRefresh" => KeyBinding::new(keystroke, ScmRefresh, None),
        "ScmSync" => KeyBinding::new(keystroke, ScmSync, None),
        "ScmPush" => KeyBinding::new(keystroke, ScmPush, None),
        "ScmPull" => KeyBinding::new(keystroke, ScmPull, None),
        "ScmFetch" => KeyBinding::new(keystroke, ScmFetch, None),
        "ScmCheckoutBranch" => KeyBinding::new(keystroke, ScmCheckoutBranch, None),
        "ScmCreateBranch" => KeyBinding::new(keystroke, ScmCreateBranch, None),
        "ScmToggleGraph" => KeyBinding::new(keystroke, ScmToggleGraph, None),
        "ToggleDiffViewMode" => KeyBinding::new(keystroke, ToggleDiffViewMode, None),
        "FindInTerminal" => KeyBinding::new(keystroke, FindInTerminal, action_context(action)),
        "FindNext" => KeyBinding::new(keystroke, FindNext, action_context(action)),
        "ToggleComposer" => KeyBinding::new(keystroke, ToggleComposer, action_context(action)),
        "FindPrevious" => KeyBinding::new(keystroke, FindPrevious, action_context(action)),
        "ClearScrollback" => KeyBinding::new(keystroke, ClearScrollback, action_context(action)),
        "InsertNewline" => KeyBinding::new(keystroke, InsertNewline, action_context(action)),
        "CopyText" => KeyBinding::new(keystroke, CopyText, action_context(action)),
        "CopyRaw" => KeyBinding::new(keystroke, CopyRaw, action_context(action)),
        "PasteText" => KeyBinding::new(keystroke, PasteText, action_context(action)),
        "AlternatePaste" => KeyBinding::new(keystroke, AlternatePaste, action_context(action)),
        "OpenSettings" => KeyBinding::new(keystroke, OpenSettings, None),
        "ShowKeyboardShortcuts" => KeyBinding::new(keystroke, ShowKeyboardShortcuts, None),
        "About" => KeyBinding::new(keystroke, About, None),
        "CheckForUpdates" => KeyBinding::new(keystroke, CheckForUpdates, None),
        "OpenDocumentation" => KeyBinding::new(keystroke, OpenDocumentation, None),
        "OpenDiscord" => KeyBinding::new(keystroke, OpenDiscord, None),
        "ReportIssue" => KeyBinding::new(keystroke, ReportIssue, None),
        "HideApp" => KeyBinding::new(keystroke, HideApp, None),
        "HideOthers" => KeyBinding::new(keystroke, HideOthers, None),
        "ShowAll" => KeyBinding::new(keystroke, ShowAll, None),
        "MinimizeWindow" => KeyBinding::new(keystroke, MinimizeWindow, None),
        "ZoomWindow" => KeyBinding::new(keystroke, ZoomWindow, None),
        "ToggleSftp" => KeyBinding::new(keystroke, ToggleSftp, None),
        "ShowSshForwards" => KeyBinding::new(keystroke, ShowSshForwards, None),
        "ToggleCodePanel" => KeyBinding::new(keystroke, ToggleCodePanel, None),
        "ToggleDocumentFill" => KeyBinding::new(keystroke, ToggleDocumentFill, None),
        "DocumentWidthThird" => KeyBinding::new(keystroke, DocumentWidthThird, None),
        "DocumentWidthHalf" => KeyBinding::new(keystroke, DocumentWidthHalf, None),
        "DocumentWidthTwoThirds" => KeyBinding::new(keystroke, DocumentWidthTwoThirds, None),
        "ToggleDocumentPreview" => KeyBinding::new(keystroke, ToggleDocumentPreview, None),
        "ToggleDocumentWrap" => KeyBinding::new(keystroke, ToggleDocumentWrap, None),
        "EditorSave" => KeyBinding::new(keystroke, EditorSave, None),
        "EditorSaveAs" => KeyBinding::new(keystroke, EditorSaveAs, None),
        "EditorGoToLine" => KeyBinding::new(keystroke, EditorGoToLine, None),
        "EditorGoToSymbol" => KeyBinding::new(
            keystroke,
            EditorGoToSymbol,
            Some(crate::actions::NAV_KEY_CONTEXT),
        ),
        "EditorNavigateBack" => KeyBinding::new(
            keystroke,
            EditorNavigateBack,
            Some(crate::actions::NAV_KEY_CONTEXT),
        ),
        "EditorNavigateForward" => KeyBinding::new(
            keystroke,
            EditorNavigateForward,
            Some(crate::actions::NAV_KEY_CONTEXT),
        ),
        "EditorSplitRight" => KeyBinding::new(
            keystroke,
            EditorSplitRight,
            Some(crate::actions::NAV_KEY_CONTEXT),
        ),
        "EditorFocusLeftGroup" => KeyBinding::new(
            keystroke,
            EditorFocusLeftGroup,
            Some(crate::actions::NAV_KEY_CONTEXT),
        ),
        "EditorFocusRightGroup" => KeyBinding::new(
            keystroke,
            EditorFocusRightGroup,
            Some(crate::actions::NAV_KEY_CONTEXT),
        ),
        "EditorNewFile" => KeyBinding::new(keystroke, EditorNewFile, None),
        "EditorGoToDefinition" => KeyBinding::new(keystroke, EditorGoToDefinition, None),
        "EditorQuickFix" => KeyBinding::new(keystroke, EditorQuickFix, None),
        "EditorRenameSymbol" => KeyBinding::new(keystroke, EditorRenameSymbol, None),
        "EditorFormatDocument" => KeyBinding::new(keystroke, EditorFormatDocument, None),
        "EditorTransformUppercase" => KeyBinding::new(keystroke, EditorTransformUppercase, None),
        "EditorTransformLowercase" => KeyBinding::new(keystroke, EditorTransformLowercase, None),
        "EditorTransformTitleCase" => KeyBinding::new(keystroke, EditorTransformTitleCase, None),
        "EditorTrimTrailingWhitespace" => {
            KeyBinding::new(keystroke, EditorTrimTrailingWhitespace, None)
        }
        "EditorJoinLines" => KeyBinding::new(keystroke, EditorJoinLines, None),
        "EditorRemoveSurroundingBrackets" => {
            KeyBinding::new(keystroke, EditorRemoveSurroundingBrackets, None)
        }
        "EditorFindReferences" => KeyBinding::new(keystroke, EditorFindReferences, None),
        "EditorNextChange" => KeyBinding::new(keystroke, EditorNextChange, None),
        "EditorPrevChange" => KeyBinding::new(keystroke, EditorPrevChange, None),
        "EditorRevertChange" => KeyBinding::new(keystroke, EditorRevertChange, None),
        "ToggleEditorGitGutter" => KeyBinding::new(keystroke, ToggleEditorGitGutter, None),
        "EditorPeekChange" => KeyBinding::new(keystroke, EditorPeekChange, None),
        "ToggleEditorProblems" => KeyBinding::new(keystroke, ToggleEditorProblems, None),
        "OpenSshProfiles" => KeyBinding::new(keystroke, OpenSshProfiles, None),
        "RestartSshSession" => KeyBinding::new(keystroke, RestartSshSession, None),
        "Quit" => KeyBinding::new(keystroke, Quit, None),
        _ => return None,
    })
}


#[cfg(test)]
mod tests;
