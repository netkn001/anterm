# Configuration & CLI Reference

anterm stores user preferences in a plain JSON file (`config.json`) that hot-reloads automatically whenever saved on disk, alongside a separate `servers.json` file for SSH profiles.

## Configuration Directory Resolution

The configuration directory is resolved in the following priority order (first match wins):

1. **CLI flag**: `--config-dir <path>` (or `--config-dir=<path>`)
2. **Environment variable**: `ANTERM_CONFIG_DIR`
3. **Windows portable mode**: `<exe-dir>\data` when both `.anterm-portable` (file) and `data\` (directory) exist beside `anterm-app.exe`
4. **Platform default**:
   - **macOS / Linux**: `~/.config/anterm`
   - **Windows**: `%APPDATA%\anterm`

### Files Inside the Config Directory

- `config.json` — UI, terminal, editor, keybinding, shell, and agent settings. Safe to sync in dotfiles.
- `servers.json` — Saved SSH host profiles and usage frecency (stored separately from `config.json` so machine-specific server lists do not leak into synced dotfiles; actual passwords/passphrases live in your OS keychain).
- `config.json.corrupt` — If a hand-edited `config.json` fails to parse, anterm quarantines the unreadable copy, starts on safe defaults, and refuses to overwrite your file until you fix the syntax error.

---

## Example `config.json`

Every key is optional and falls back to its built-in default if omitted. Invalid values in individual entries (such as a single malformed keybinding or custom shell) are skipped with a warning rather than discarding the rest of your file.

```json
{
  "gui_language": "en",
  "font_family": "Hack",
  "font_fallbacks": ["Symbols Nerd Font Mono"],
  "font_size": 14.0,
  "line_height": 1.25,
  "ui_font_size": 14.0,
  "theme_follow_system": true,
  "theme_preset_light": "default-light",
  "theme_preset_dark": "default-dark",
  "tab_bar_position": "left",
  "prompt_editor": true,
  "helix_mode": false,
  "editor_lsp": true,
  "editor_git_gutter": true,
  "diff_view": "split",
  "restore_session": true,
  "restore_agent_sessions": true,
  "check_for_updates": true,
  "auto_download_updates": true,
  "agent_commands": {
    "my-claude-wrapper": "claude"
  },
  "agent_launch": {
    "claude": "claude --model sonnet"
  },
  "keybindings": {
    "SplitLeft": "cmd-alt-shift-left",
    "SplitUp": "cmd-alt-shift-up"
  }
}
```

---

## Configuration Key Reference

### Typography & Rendering

| Key | Type | Description |
| --- | --- | --- |
| `font_family` | `string` | Primary monospace font family for the terminal grid (bundled `Hack` is always available). |
| `font_fallbacks` | `string[]` | Ordered fallback font families for glyphs missing from `font_family`. |
| `font_family_bold` / `font_family_italic` | `string \| null` | Optional dedicated bold / italic font families. |
| `font_features` | `object \| null` | OpenType 4-char feature tags mapped to `true`/`false` or integer values (e.g. `{"calt": true, "liga": false}`). |
| `font_thicken` | `bool` | macOS only (`true` by default): enables CoreGraphics font smoothing dilation; takes effect on next launch. |
| `font_size` | `number` | Terminal grid font size in logical pixels. |
| `line_height` | `number` | Terminal line height multiplier (e.g. `1.25`). |
| `ui_font_size` | `number` | Root UI chrome font size in pixels (`rem` scaling across panels, headers, and Settings). |
| `ui_font_family` | `string \| null` | Optional custom UI chrome font (`null` uses the system UI font). |

### Theme, Window & Layout

| Key | Type | Description |
| --- | --- | --- |
| `theme` / `theme_preset` | `string` | Active color scheme / theme preset. |
| `theme_follow_system` | `bool` | Automatically switch between `theme_preset_light` and `theme_preset_dark` with OS appearance. |
| `theme_legible_palette` | `bool` | Automatically improves contrast of low-contrast ANSI colors against the active background. |
| `window_opacity` / `window_blur` / `window_backdrop` | `number \| bool \| string` | Optional window transparency, blur, and native backdrop effect. |
| `dim_inactive_panes` | `bool` | Slightly dims unfocused split panes (`true` by default). |
| `auto_hide_titlebar_buttons` | `bool` | Show title-bar action tiles only while hovering over the title bar. |
| `tab_bar_position` | `"top" \| "left"` | Place tabs in the top strip or the virtualized left sidebar (with tag filters and sections). |
| `sidebar_width` / `sidebar_collapsed` | `number \| bool` | Left sidebar width in pixels and collapsed state. |
| `right_panel_visible` / `right_panel_width` / `right_panel_tab` | `bool \| number \| string` | Right detail panel visibility, width, and active tab (`files`, `scm`, `info`, `sftp`). |
| `startup_mode` | `"normal" \| "maximized" \| "fullscreen"` | Initial window state on startup. |
| `remember_window_size` | `bool` | Remember and restore window dimensions across launches (`true` by default). |
| `new_tab_position` | `string` | Where newly opened tabs are inserted relative to the active tab. |
| `gui_language` | `"en" \| "zh-CN"` | UI language (`en` or `zh-CN`). |
| `show_tray_icon` | `bool` | Keep a system tray / menu-bar icon active (`true` by default). |

### Terminal, Prompt Editor & Input

| Key | Type | Description |
| --- | --- | --- |
| `prompt_editor` | `bool` | Enable anterm's built-in inline prompt editor (`true` by default). Set to `false` to pass all prompt keys directly to zsh/bash/fish while keeping shell integration marks active. |
| `tab_completion` / `history_search` | `bool` | Enable explained tab completion and `Ctrl+R` fuzzy history menus (requires `prompt_editor: true`). |
| `helix_mode` | `bool` | Enable Helix modal editing (`Normal`, `Insert`, `Select`) across the code editor, prompt editor, and composer. |
| `per_pane_history` | `bool` | Isolate command history per pane (seeded from shell history and merged on close). |
| `scrollback_limit` | `integer` | Maximum scrollback lines kept per pane. |
| `cursor_style` | `"block" \| "bar" \| "underline"` | Default terminal cursor shape. |
| `prompt_cursor_style` | `"follow" \| "block" \| "bar" \| "underline"` | Cursor shape while waiting at the shell prompt (`follow` inherits `cursor_style`). |
| `cursor_blink` | `bool` | Enable cursor blinking. |
| `bell` | `string` | Terminal bell behavior. |
| `macos_option_as_alt` | `bool` | Treat Option as Alt/Meta on macOS. |
| `mouse_reporting` / `smooth_scroll` / `mouse_scroll_multiplier` | `bool \| number` | Mouse event forwarding, wheel animation, and scroll speed multiplier. |
| `mouse_zoom_modifier` | `string` | Optional modifier key enabling mouse-wheel font zooming. |
| `copy_on_select` / `clipboard_trim_trailing_spaces` / `copy_join_wrapped` | `bool` | Clipboard selection and copy formatting behaviors. |
| `smart_select` / `word_separators` | `bool \| string` | Double-click smart selection boundaries. |
| `link_url` / `link_file_open` / `link_file_command` | `bool \| string` | Clickable URLs and file-path links (open in built-in editor or custom command template). |

### Built-in Code Editor & Git SCM

| Key | Type | Description |
| --- | --- | --- |
| `editor_lsp` | `bool` | Automatically launch installed language servers (`rust-analyzer`, `typescript-language-server`, etc.) (`true` by default). |
| `editor_git_gutter` | `bool` | Display inline git diff markers in the editor gutter (`true` by default). |
| `editor_soft_wrap` | `bool` | Wrap long lines in the built-in code editor. |
| `editor_markdown_preview` | `bool` | Default Markdown files to rendered preview mode. |
| `diff_view` | `"split" \| "unified"` | Diff presentation mode in diff overlays and SCM views. |
| `document_layout` / `document_ratio` | `string \| number` | Open code/diff documents docked beside the terminal or filling the pane area. |
| `scm_graph_expanded` / `scm_changes_tree` | `bool` | Source control commit graph default state and tree/flat change list view. |

### Shell, Sessions, Agents & SSH

| Key | Type | Description |
| --- | --- | --- |
| `shell` | `object \| null` | Explicit default shell program and arguments (`{"program": "/bin/zsh", "args": ["-l"]}`). |
| `custom_shells` | `array` | Additional named shell profiles available in the New Tab menu and Command Palette. |
| `working_directory` | `{"strategy": "inherit" \| "home" \| "custom", "path": ""}` | Initial directory strategy for new tabs and splits. |
| `env` | `object` | Extra environment variables injected into spawned local shells. |
| `restore_session` / `restore_agent_sessions` | `bool` | Restore open tabs/splits and supported coding-agent sessions across restarts (`true` by default). |
| `confirm_close` | `string` | Confirmation policy when closing tabs/panes with running processes. |
| `notify_on_command_finish` / `notify_threshold_secs` | `string \| integer` | Desktop notification policy and minimum duration for long-running shell commands. |
| `agent_commands` | `object` | Map custom wrapper script names to a known agent slug (e.g. `{"claude-work": "claude"}`). |
| `agent_launch` | `object` | Override the launch command line per agent slug (e.g. `{"claude": "claude --model opus"}`). |
| `verify_host_keys` / `ssh_warn_on_close` / `ssh_loopback_forward` | `bool` | Native SSH host-key verification (`true` by default), close confirmation, and loopback forwarding options. |

### Updates & Proxy

| Key | Type | Description |
| --- | --- | --- |
| `check_for_updates` | `bool` | Periodically check GitHub Releases for newer versions. |
| `update_channel` | `"stable" \| "nightly"` | Release feed followed by the updater. |
| `auto_download_updates` | `bool` | Stage verified update packages in the background (`true` by default; installation always requires your confirmation). |
| `http_proxy` | `string \| null` | Optional HTTP/SOCKS proxy for anterm's own update requests (e.g. `"http://127.0.0.1:7890"`, `"socks5://127.0.0.1:1080"`). Terminal shells keep their own environment variables. |

---

## CLI Reference

### `anterm-app` (Desktop Application)

```bash
anterm-app [OPTIONS]
```

| Flag / Subcommand | Description |
| --- | --- |
| `--config-dir <dir>` | Use `<dir>` instead of the default configuration directory. |
| `--open-path <path>` | Open `<path>` (file or directory) in a running window or new startup window. |
| `--daemon` | Run the persistent session daemon in this process. |
| `--stop-daemon` | Gracefully stop the running local session daemon. |
| `agent-hook <agent> <event>` | Hook entry point invoked by coding-agent integrations. |
| `--register-explorer-menu` / `--unregister-explorer-menu` | (Windows) Register or remove Explorer context-menu entries. |

### `anterm-server` (Headless Session Server)

```bash
anterm-server --daemon [--config-dir <dir>]
anterm-server --stdio [--serve | --bridge] [--control-sock <path>]
anterm-server --stdio --pane [--config-dir <dir>]
anterm-server --stop [--config-dir <dir>]
anterm-server agent-hook <agent> <event>
anterm-server --protocol
anterm-server --version
```

`anterm-server` compiles with zero GUI, SQLite, or keychain dependencies, making it suitable for headless Linux/macOS/Windows remote hosts over SSH.
