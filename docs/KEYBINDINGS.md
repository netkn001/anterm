# Keybindings Reference

Default keyboard shortcuts shipped with **anterm**. You can view or record shortcuts interactively in **Settings → Keybindings** (or press <kbd>⌘ /</kbd> on macOS), or customize them directly in `config.json`.

> **Notation**: On macOS, `secondary` maps to **⌘ (Cmd)**. On Windows and Linux, `secondary` maps to **Ctrl**.

---

## Tabs, Tags & Windows

| Action | macOS | Windows / Linux |
| --- | --- | --- |
| `NewTab` | <kbd>⌘ T</kbd> | <kbd>Ctrl+Shift+T</kbd> |
| `NewWindow` | <kbd>⌘ N</kbd> | *(unbound)* |
| `CloseActiveTab` | <kbd>⌘ W</kbd> | <kbd>Ctrl+Shift+W</kbd> |
| `CloseWindow` | <kbd>⌘ ⇧ W</kbd> | *(unbound)* |
| `ReopenClosedTab` | <kbd>⌘ ⇧ T</kbd> | <kbd>Alt+Shift+T</kbd> |
| `NextTab` (MRU) | <kbd>⌃ Tab</kbd> | <kbd>Ctrl+Tab</kbd> |
| `PrevTab` (MRU) | <kbd>⌃ ⇧ Tab</kbd> | <kbd>Ctrl+Shift+Tab</kbd> |
| `SelectNextTab` | <kbd>⌘ ⇧ ]</kbd> | <kbd>Ctrl+PageDown</kbd> |
| `SelectPrevTab` | <kbd>⌘ ⇧ [</kbd> | <kbd>Ctrl+PageUp</kbd> |
| `ActivateTab1` … `ActivateTab9` | <kbd>⌘ 1</kbd> … <kbd>⌘ 9</kbd> | <kbd>Alt+1</kbd> … <kbd>Alt+9</kbd> |
| `EditTabTags` | <kbd>⌘ ⇧ L</kbd> | <kbd>Ctrl+Shift+L</kbd> |
| `SwitchTab` / `SwitchTag` | *(via Command Palette or bindable)* | *(via Command Palette or bindable)* |
| `NewAgentTab` | <kbd>⌘ ⇧ A</kbd> | *(unbound)* |
| `FocusNextAgentNeedingAttention` | <kbd>⌘ ⇧ U</kbd> | <kbd>Ctrl+Shift+U</kbd> |

## Splits & Pane Navigation

| Action | macOS | Windows / Linux |
| --- | --- | --- |
| `SplitRight` | <kbd>⌘ D</kbd> | <kbd>Ctrl+Shift+D</kbd> |
| `SplitDown` | <kbd>⌘ ⇧ D</kbd> | <kbd>Ctrl+Alt+Shift+D</kbd> |
| `SplitLeft` / `SplitUp` | *(via Command Palette / menus or bindable)* | *(via Command Palette / menus or bindable)* |
| `FocusNextPane` / `FocusPrevPane` | <kbd>⌘ ]</kbd> / <kbd>⌘ [</kbd> | <kbd>Ctrl+Shift+]</kbd> / <kbd>Ctrl+Shift+[</kbd> |
| `FocusPaneLeft` / `Right` / `Up` / `Down` | <kbd>⌘ ⌥ ←</kbd> / <kbd>→</kbd> / <kbd>↑</kbd> / <kbd>↓</kbd> | <kbd>Alt+←</kbd> / <kbd>→</kbd> / <kbd>↑</kbd> / <kbd>↓</kbd> |
| `ToggleMaximizePane` | <kbd>⌘ ⇧ ↵</kbd> | <kbd>Ctrl+Shift+Enter</kbd> |

## Command Palette, Panels & Window Chrome

| Action | macOS | Windows / Linux |
| --- | --- | --- |
| `TogglePalette` (Command Palette) | <kbd>⌘ P</kbd> | <kbd>Ctrl+Shift+P</kbd> |
| `QuickOpenFile` | <kbd>⌘ O</kbd> | *(unbound)* |
| `ToggleLeftPanel` (Tab Sidebar) | <kbd>⌘ B</kbd> | <kbd>Ctrl+Shift+B</kbd> |
| `ToggleRightPanel` (Files / SCM / Info / SFTP) | <kbd>⌘ J</kbd> | <kbd>Ctrl+Shift+J</kbd> |
| `OpenSettings` | <kbd>⌘ ,</kbd> | <kbd>Ctrl+,</kbd> |
| `ShowKeyboardShortcuts` | <kbd>⌘ /</kbd> | *(unbound)* |
| `ToggleFullscreen` | <kbd>⌘ ↵</kbd> | <kbd>F11</kbd> |
| `IncreaseFontSize` / `DecreaseFontSize` / `ResetFontSize` | <kbd>⌘ =</kbd> / <kbd>⌘ -</kbd> / <kbd>⌘ 0</kbd> | <kbd>Ctrl+=</kbd> / <kbd>Ctrl+-</kbd> / <kbd>Ctrl+0</kbd> |

## Terminal & Composer

| Action | macOS | Windows / Linux |
| --- | --- | --- |
| `FindInTerminal` | <kbd>⌘ F</kbd> | <kbd>Ctrl+Shift+F</kbd> |
| `FindNext` / `FindPrevious` | <kbd>⌘ G</kbd> / <kbd>⌘ ⇧ G</kbd> | <kbd>F3</kbd> / <kbd>Shift+F3</kbd> |
| `ClearScrollback` | <kbd>⌘ K</kbd> | <kbd>Ctrl+Shift+K</kbd> |
| `ToggleComposer` | <kbd>⌘ I</kbd> | <kbd>Ctrl+Shift+I</kbd> |
| `InsertNewline` | <kbd>⇧ ↵</kbd> (or <kbd>⌥ ↵</kbd>) | <kbd>Shift+Enter</kbd> (or <kbd>Alt+Enter</kbd>) |
| `CopyText` / `PasteText` | <kbd>⌘ C</kbd> / <kbd>⌘ V</kbd> | <kbd>Ctrl+Shift+C</kbd> / <kbd>Ctrl+Shift+V</kbd> (plus <kbd>Ctrl+V</kbd> at prompt) |
| `CopyRaw` (skip wrapped-line join) | <kbd>⌘ ⌥ C</kbd> | *(bindable)* |
| Fuzzy History Reverse Search | <kbd>⌃ R</kbd> | <kbd>Ctrl+R</kbd> |

## Built-in Code Editor & LSP

Active while focus is inside the built-in code editor:

| Action | macOS | Windows / Linux |
| --- | --- | --- |
| `EditorSplitRight` | <kbd>⌘ \\</kbd> | <kbd>Ctrl+\\</kbd> |
| `ToggleLineComment` / `ToggleBlockComment` | <kbd>⌘ /</kbd> / <kbd>⌥ ⇧ A</kbd> | <kbd>Ctrl+/</kbd> / <kbd>Alt+Shift+A</kbd> |
| `SelectNextOccurrence` / `SelectAllOccurrences` | <kbd>⌘ D</kbd> / <kbd>⌘ ⇧ L</kbd> | <kbd>Ctrl+D</kbd> / <kbd>Ctrl+Shift+L</kbd> |
| `SkipOccurrence` | <kbd>⌘ K</kbd> <kbd>⌘ D</kbd> | <kbd>Ctrl+K</kbd> <kbd>Ctrl+D</kbd> |
| `AddCursorAbove` / `AddCursorBelow` | <kbd>⌘ ⌥ ↑</kbd> / <kbd>⌘ ⌥ ↓</kbd> | <kbd>Ctrl+Alt+↑</kbd> / <kbd>Ctrl+Alt+↓</kbd> |
| `MoveLineUp` / `MoveLineDown` | <kbd>⌥ ↑</kbd> / <kbd>⌥ ↓</kbd> | <kbd>Alt+↑</kbd> / <kbd>Alt+↓</kbd> |
| `CopyLineUp` / `CopyLineDown` | <kbd>⌥ ⇧ ↑</kbd> / <kbd>⌥ ⇧ ↓</kbd> | <kbd>Alt+Shift+↑</kbd> / <kbd>Alt+Shift+↓</kbd> |
| `DeleteLine` / `SelectLine` | <kbd>⌘ ⇧ K</kbd> / <kbd>⌘ L</kbd> | <kbd>Ctrl+Shift+K</kbd> / <kbd>Ctrl+L</kbd> |
| `EditorGoToDefinition` / `EditorFindReferences` | <kbd>F12</kbd> / <kbd>⇧ F12</kbd> | <kbd>F12</kbd> / <kbd>Shift+F12</kbd> |
| `EditorRenameSymbol` / `EditorFormatDocument` | <kbd>F2</kbd> / <kbd>⌥ ⇧ F</kbd> | <kbd>F2</kbd> / <kbd>Shift+Alt+F</kbd> |
| `EditorNextChange` / `PrevChange` / `PeekChange` | <kbd>⌥ F5</kbd> / <kbd>⌥ ⇧ F5</kbd> / <kbd>⌥ F3</kbd> | <kbd>Alt+F5</kbd> / <kbd>Shift+Alt+F5</kbd> / <kbd>Alt+F3</kbd> |

---

## Customizing Keybindings in `config.json`

`keybindings` maps action names to either a **string** (adds a chord alongside the default, or `""` to unbind) or an **array of strings** (replaces the default chords with the exact list; `[]` unbinds):

```json
{
  "keybinding_preset": "default",
  "keybindings": {
    "SplitLeft": "cmd-alt-shift-left",
    "SplitUp": "cmd-alt-shift-up",
    "SwitchTab": "cmd-shift-p",
    "SelectNextTab": ["cmd-shift-]"],
    "ToggleFullscreen": ""
  }
}
```

You can also switch `keybinding_preset` (including tmux-style prefix bindings with configurable `"prefix": "ctrl-b"`) from **Settings → Keybindings** or directly in `config.json`.
