# Anterm Architecture Guide

Anterm is a GPU-rendered terminal workbench (shells, persistent sessions, native SSH/SFTP, coding agents, code editor, Git/SCM, diff viewer) written in pure Rust on [GPUI Kit](https://gpui-kit.com) (`gpui-kit 0.10.0`) and licensed under the [MIT License](../LICENSE).

> **Related Docs**: [Contributing Guide](../CONTRIBUTING.md) · [Configuration & CLI Reference](CONFIGURATION.md) · [Keybindings Reference](KEYBINDINGS.md) · [Third-Party Notices](../THIRD_PARTY_NOTICES.md)

## Workspace Layout

The repository is a **pure Cargo virtual workspace** containing **13 crates** under `crates/` in a strictly acyclic dependency DAG (no root `[package]`, no root `src/`):

```
.
├── Cargo.toml                  # Virtual workspace manifest (`resolver = "3"`, shared pins/lints/profiles)
├── clippy.toml                 # `allow-unwrap-in-tests = true` (production enforces `unwrap_used = "deny"`)
├── .cargo/config.toml          # `cargo dev` alias (`run -p anterm-app --bin anterm-app -- --config-dir .anterm-dev`)
├── assets/                     # Shared icons, fonts, completions, and Windows ConPTY runtime
├── docs/                       # Architecture documentation
├── scripts/                    # Release build & packaging scripts (`package-release.sh`, `package-windows.ps1`)
└── crates/
    ├── anterm-protocol/        # Leaf wire protocol, framing, tags, updater contracts & pure value types
    ├── anterm-i18n/            # Localization keys, plural rules & en/zh-CN translation tables (zero deps)
    ├── anterm-editor/          # Framework-free editor engines (Helix, text/EditorConfig, diff layout, LSP RPC/servers)
    ├── anterm-core/            # Headless session daemon, PTY, native SSH engine, host abstraction & domain model
    ├── anterm-updater/         # Standalone auto-updater helper binary (`anterm-updater`, zero GPUI)
    ├── anterm-server/          # Headless session server binary (`anterm-server`, 1 dep: `anterm-core`, zero GPUI)
    ├── anterm-ui-kit/          # Shared GPUI foundation: theme, presets, actions, keymap, ConfigStore, dialog/kit primitives
    ├── anterm-terminal/        # GPU-rendered terminal widget, PTY mirror, prompt/history subsystem & agent composer
    ├── anterm-remote-ui/       # Remote SSH UI panels: port forwards, SFTP browser & host adapter, SSH auth modal
    ├── anterm-files-ui/        # File tree, Git SCM panel & commit graph, diff overlay, panel search & right detail panel
    ├── anterm-editor-ui/       # Built-in code editor workspace, split groups, LSP UI bridge & document column
    ├── anterm-settings-ui/     # Autonomous Settings window & configuration pages
    └── anterm-app/             # Thin desktop App Shell (~11% prod LOC): startup, multi-window orchestration, tabs/splits, palette, tray
```

### Dependency DAG

```
Headless Core & Tools (zero GPUI):
  anterm-protocol (leaf) ──┬──> anterm-core ──> anterm-server
                           ├──> anterm-editor
                           └──> anterm-updater (Unix: protocol + libc; Windows: +core)
  anterm-i18n (leaf)

Desktop GPUI UI Layer (strictly acyclic):
  {anterm-core, anterm-editor, anterm-i18n, anterm-protocol}
        │
        v
  anterm-ui-kit
        ├──> anterm-terminal
        ├──> anterm-settings-ui
        ├──> anterm-remote-ui ──> anterm-files-ui ──> anterm-editor-ui
        │                                                   │
        └──────────────────────────┬────────────────────────┘
                                   v
                              anterm-app (thin App Shell)
```

## Crate Responsibilities & Seams

### Headless Core & Tools (6 Crates)

#### 1. `crates/anterm-protocol` (Leaf Protocol & Shared Contracts)
Zero GPUI, zero SSH (`russh`), zero `tokio`, zero `portable-pty`, zero `rusqlite`, zero `notify`:
- **`util`**: Leaf utilities (`fnv1a64`, `unix_now`, `strip_bom`, `trim_boms`, `home_dir`, `home_dir_any`, `process_instance()`, `to_json`/`from_json`, `de_lenient`, `write_atomic`/`write_atomic_private`, serde `b64`).
- **`ids`**: Stable cross-process host identifier (`HostId`).
- **`tag`**: User-defined tab tag definitions (`TagId`, `TagColor`, `TagDef`, `SidebarTags`) shared across wire protocol, daemon, CLI, and GUI.
- **`diff`**: Pure diff data structures (`DiffViewMode`, `Hunk`, `LineKind`, `DiffLine`).
- **`pane/`**: 5-byte framed pane/SSH/SFTP/forward/proc wire protocol (`ClientMsg`, `DaemonMsg`, `ShellSpec`, `NativeSshSpec`, `WinSize`, `PaneInfo`, `PaneProcs`, `DaemonVersion`, `feature::*`, codec).
- **`updater/`**: Shared GUI↔updater contract (`contract`, `outcome`, `checksums`).
- **`cli_agent/`**, **`keychain`** (secret-free naming half), **`shell_quote`**, **`ssh_profile`**.

#### 2. `crates/anterm-i18n` (Zero-Dependency Localization)
Self-contained English (`en`) and Simplified Chinese (`zh-CN`) translation tables with compile-time exhaustive `L10nKey` matching, simplified `PluralCategory` (`Zero`, `One`, `Other`), and cross-locale search alias helpers (`t`, `t_fmt`, `t_plural`, `alias_translations`). Zero external dependencies; edits to copy never recompile `anterm-core` internals and can be shared with CLI/updater.

#### 3. `crates/anterm-editor` (Framework-Free Editor Engines)
Pure Rust engines with **zero GPUI / `gpui-component` dependencies** (sub-second unit tests):
- **`helix/`** (`mod`, `motion`, `ops`, `tests`): Helix modal editing state machine taking framework-agnostic `KeyInput<'a>`, selection-first motions, text objects, surround ops, register store, and `:command` parser.
- **`text`**: File encoding/BOM/line-ending detection & encoding (`UTF-8`, `UTF-16`, `GB18030`, `Windows-1252`), indentation detection, `.editorconfig` resolution, save rules.
- **`diff`**: Side-by-side and unified diff row pairing (`split_hunk`, `unified_rows`, `DiffSelection`).
- **`lsp/`** (`rpc`, `servers`): LSP JSON-RPC `Content-Length` framing and language-server registry + workspace root discovery.

#### 4. `crates/anterm-core` (Headless Core)
Flattened top-level module layout re-exporting leaf protocol modules from `anterm-protocol`:
- **Domain & Persistence**: `config/`, `machine/`, `session`, `tab_view`, `window_state`, `ssh_config`, `rate_meter`, `agent_prompt`, `git/` (`log/`, `diff`, `status`, `ops`, `head`), `worktree/`, `gitignore`, `agent_hooks/`, `osc`, `kitty_graphics/`, `term_modes`, `paste`, `path_spelling`, `proc`, `shells`, `clipboard`, `codename`, `crash`, `logfile`, `threads`, `history/`.
- **`daemon/`**: `control/`, `pane/`, `spawn/`, `shell_integration/` (with external `scripts/*.{zsh,fish,bash,ps1,nu}`), `ssh/`, `install/`, `server`, `router`, `transport`, `handoff`, `singleton`, `scrollback`, `procinfo`, plus `pub use anterm_protocol::pane as protocol`.
- **`host/`**: Unified `Host` filesystem/git/search abstraction (`local`, `remote`, `content_search`, `server/`, and `test-support`-gated `conformance`).
- **`client/`**: High-level `PaneClient` and `ControlClient` ergonomics.

#### 5. `crates/anterm-server` (Headless Server)
Thin binary wrapping `anterm-core` (default features only — zero GPUI, zero `rusqlite`, zero `keyring`, zero `ureq`, zero `libgssapi`) for `--daemon`, `--stdio`, `--stop`, and `agent-hook`.

#### 6. `crates/anterm-updater` (Standalone Auto-Updater)
Dedicated lightweight binary crate with **zero GPUI dependencies**. On Unix (macOS/Linux), depends **only on `anterm-protocol` + `libc`** (never compiles `russh`, `tokio`, `portable-pty`, or `rusqlite`); on Windows, adds `anterm-core` for `update_guard` / `stop_for_update`.

---

### Desktop GPUI UI Layer (7 Crates)

#### 7. `crates/anterm-ui-kit` (Shared GPUI Foundation)
Shared GUI infrastructure consumed by every UI feature crate without ever depending upward on `anterm-app`:
- **State & Events**: `config_store` (reactive `ConfigStore` global with rollback & error surface), `intent` (`WorkspaceIntent`, `ActiveWorkspaceContext`, `SettingsSection`, `ThemeEdit`), `app_hooks` (startup callback bridge registered by `anterm-app`), `daemon_rpc` (stateless daemon RPC routes), `host_registry`, `host_ops`, `git/` (`ScmData`, `GitStatusCache`, watchers).
- **Theme & Design System**: `theme` (single write path through `Theme::update` driving `colors`, `tokens`, `gpui-base`, and `Root`-managed `rem` scaling), `presets/`, `palette_colors`, `panel_tokens`, `rounding`, `window_chrome`, `kit` (settings controls backed by `gpui_kit::component::{Switch, Button}` with `cx.theme()` semantic colors & rem scaling), `dialog` (modal card chrome & `Button` primitives), `prompt` (`TextPrompt` with `FocusTrapElement`), `notice`, `scrollbar`.
- **Input, Search & Platform**: `actions`, `keymap/`, `helix_bridge`, `search/` (scoring, command/file indexing), `assets`, `platform/`, `keychain`, `ssh_helpers`, `update/`, `perf`, `prefill`, `path_display`, `default_terminal`, `explorer_context_menu`, `aumid`, `toast_activator`.

#### 8. `crates/anterm-terminal` (Terminal Engine & Widget)
Self-contained GPU terminal widget with zero coupling to other UI feature crates:
- **`view/`**: Lean `TerminalView` (`15` top-level fields grouped into cohesive state structs in `state.rs`), high-frequency isolated `TerminalCanvas`, and cached overlay entities (`TerminalSearchBar`, `AgentComposerView`) that never re-render on pure PTY canvas output.
- **`backend/`**: `fps`, `hold`, `loopback`, `size`, `typeahead`.
- **`render/`**: `boxdraw`, `color_scheme`, `highlight`, `images`, `palette`, `scrollbar`.
- **`prompt/`**: `cmd_editor`, `command_cursor`, `completion`, `fuzzy`, `generator`, `history`, `parked_cursor`, `prompt_reflow`, `reverse_search`, `signature`, `store`.
- **`links/`**, **`remote/`**, **`composer/`**, **`element/`**, **`search/`**, **`input.rs`**.

#### 9. `crates/anterm-remote-ui` (Remote SSH & SFTP UI)
Independent GPUI views communicating via typed events and `WorkspaceIntent`:
- **`forwards`**: `ForwardsPanel` entity for local/remote/dynamic port forwarding.
- **`sftp/` & `sftp_host`**: `SftpPanelView` entity virtualized with `v_virtual_list` + transfer job progress.
- **`ssh_prompt`**: `SshPromptModal` entity with `FocusTrapElement` for password, passphrase, 2FA, and host-key verification.

#### 10. `crates/anterm-files-ui` (Files, Git SCM & Diff UI)
Independent right-panel ecosystem and diff viewer (`anterm-ui-kit` + `anterm-remote-ui`):
- **`file_tree/`**: `FileTreeView` virtualized with `v_virtual_list` (handles 10k+ entries in constant visible rows).
- **`scm/`**: `ScmPanelView` with virtualized change lists and windowed Git Commit Graph.
- **`diff_overlay/` & `diff_list`**: `DiffOverlayView` (`gpui::list` virtualized unified/split diffs).
- **`panel_search/`**, **`proc_usage`**, **`file_copy`**, **`right_panel/`** (`RightPanelView` hosting `Info` / `Files` / `Scm` / `Sftp`).

#### 11. `crates/anterm-editor-ui` (Built-in Code Editor UI)
Deep-module editor workspace (`anterm-editor` + `anterm-ui-kit` + `anterm-files-ui`):
- **`workspace`**: `CodeEditorWorkspace` entity owning per-`TabId` document/diff state and emitting `CodeEditorEvent`.
- **`code_editor/`**: Modular buffer, command, Helix bridge, menu, outline, problems, and session submodules (`< 700` lines each).
- **`document_column`**, **`editor_session`**, **`lsp/`** (GPUI LSP client & providers).

#### 12. `crates/anterm-settings-ui` (Autonomous Settings Window)
Standalone `SettingsWindow` entity owning its full state and reading/writing `ConfigStore` directly (zero cross-window workspace re-renders), decoupled from `anterm-app` via `SettingsHost` callbacks and `anterm_ui_kit::app_hooks`. Verified across `ui_font_size = 12px / 14px / 18px` zoom tiers.

#### 13. `crates/anterm-app` (Thin Desktop App Shell)
Thin orchestration binary + library shell (~11% of workspace production code):
- **`src/lib.rs` & `src/main.rs`**: Application bootstrap, bundled fonts, `register_app_hooks`, and thin binary entry point.
- **`src/startup/`**: `cli.rs` (early CLI dispatch), `config_watch.rs` (live config & theme hot-reload via `ConfigWatcherGuard`).
- **`src/core/` & `src/daemon.rs`**: Session serialization & thin daemon/core re-exports.
- **`src/ui/workspace/`**: `app/` (top-level layout, cached `TabSidebarHost` & `TabStripHost`, action/intent dispatch), `windows`, `pane/` (4-way split tree), `tab_sidebar/` (virtualized with `v_virtual_list`, tag filter bar & tag sections), `tab_strip/`, `home`, `hints`, `reorder`, `pane_drag`, `pending_pane`, `agent_launch`, `intent`.
- **`src/ui/chrome/`**: `palette`, `search/` (Zed-style Command Palette & focused pickers), `tray/`.
- **`src/ui/remote/`**: `local_link`, `machine_mirror`, `ssh_connect`, `tree_sync/`.

## Architectural Invariants & Quality Gates

- **Acyclic Crate Boundaries**: UI feature crates never depend on `anterm-app`; `anterm-terminal`, `anterm-settings-ui`, and `anterm-remote-ui` are mutually independent.
- **Headless Binary Cleanliness**: `anterm-server` and `anterm-updater` must never pull in `gpui`, `rusqlite`, or `keyring`.
- **Production Panic Discipline**: `[workspace.lints.clippy]` enforces `unwrap_used = "deny"`, `dbg_macro = "deny"`, and `todo = "deny"` across all 13 crates (with `clippy.toml` `allow-unwrap-in-tests = true` for idiomatic tests).
- **Render Isolation & Virtualization**: High-frequency PTY output updates only `TerminalCanvas` (0 re-renders on cached `TabSidebarHost`, `TabStripHost`, `TerminalSearchBar`, `AgentComposerView`, and `SettingsWindow`); all four potentially unbounded list views (`FileTreeView`, `ScmPanelView` / Commit Graph, `SftpPanelView`, `TabSidebar`) are virtualized.

## Build, Verification & Packaging Commands

```bash
# Check every crate, target, and test suite across all 13 crates (0 warnings)
cargo check --workspace --all-targets --tests

# Enforce zero production `.unwrap()` across all 13 crates
cargo clippy --workspace --all-targets -- -A clippy::all -D clippy::unwrap_used

# Verify headless binaries remain free of GUI/SQLite/keychain dependencies
cargo tree -p anterm-server  | grep -E 'gpui|rusqlite|keyring' && exit 1 || echo "OK"
cargo tree -p anterm-updater | grep -E 'gpui|rusqlite|keyring' && exit 1 || echo "OK"

# Run the desktop app in dev mode
cargo dev

# Build and package release artifacts into dist/
./scripts/package-release.sh --dmg    # macOS (.app/.zip/.dmg) or Linux (.tar.gz)
.\scripts\package-windows.ps1         # Windows (portable .zip + anterm-server.exe + checksums.txt)

# Run all library tests across all 13 crates
cargo test --workspace --lib

# Fast pure-Rust headless crate tests (zero GPUI, seconds to run)
cargo test -p anterm-protocol -p anterm-i18n -p anterm-editor -p anterm-core --lib

# Targeted UI feature crate tests
cargo test -p anterm-ui-kit -p anterm-remote-ui -p anterm-settings-ui -p anterm-files-ui -p anterm-editor-ui -p anterm-terminal -p anterm-app --lib
```
