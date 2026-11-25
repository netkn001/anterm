<div align="center">

<img src="assets/app-icon.svg" alt="anterm" width="88" height="88" />

# anterm

**A GPU-accelerated terminal workbench: persistent sessions, native SSH/SFTP, built-in code editor, and first-class coding agent support.**

<sub>Pure Rust · GPUI Kit GPU renderer · Alacritty VT core · Headless session daemon</sub>

<br />

[![CI](https://github.com/netkn001/anterm/actions/workflows/ci.yml/badge.svg)](https://github.com/netkn001/anterm/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/netkn001/anterm?label=release&color=3FDD8C)](https://github.com/netkn001/anterm/releases)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%C2%B7%20Windows%20%C2%B7%20Linux-blue)](https://github.com/netkn001/anterm/releases)
[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange)](Cargo.toml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

<sub>English · [简体中文](README.zh-CN.md)</sub>

</div>

---

## Why anterm?

In **anterm**, a lightweight headless background daemon (`anterm-server`) owns your shells and panes — not the GUI window. Everything below follows from that design:

- **Persistent sessions without tmux** — close the window and your shells keep running; restore layouts, scrollback, and supported coding-agent sessions after restarts.
- **GPU-rendered & isolated** — built in pure Rust on [GPUI Kit](https://gpui-kit.com) and [`alacritty_terminal`](https://github.com/alacritty/alacritty); high-frequency PTY redraws stay isolated to `TerminalCanvas` while large file trees, commit graphs, SFTP browsers, and sidebars are virtualized.
- **Agent-aware workbench** — automatic detection for 28 CLI coding agents with brand badges, live status dots, git branch/diff context, input-needed notifications, tray alerts, session resume, and session forking.
- **Editor-grade terminal input** — inline ghost suggestions from history, explained tab completion, syntax highlighting, multi-line editing, optional Helix modal editing, and fuzzy history search (`⌃ R`) with zero shell plugins to install.
- **Native SSH & remote development** — pure-Rust `russh` stack with OS keychain profiles, SFTP file browser, local/remote/dynamic port forwarding, jump hosts, and unprivileged remote `anterm-server` deployment.
- **Built-in code editor & Git SCM** — docked or full-width code editor with LSP diagnostics/completion/go-to-definition, TextMate syntax highlighting (`syntect`), side-by-side and unified diff viewer, and windowed Git commit graph.

## Features at a Glance

| Area | Highlights |
| --- | --- |
| **Command Palette** | <kbd>⌘ P</kbd> / <kbd>Ctrl+Shift+P</kbd> Zed-style pure command palette · frecency-ranked commands · focused sub-pickers for **Switch Tab**, **Switch Tag**, **Change Theme**, **Go to File**, **Go to Symbol**, **SSH Hosts**, and **Shells** |
| **Tabs, Tags & Splits** | 4-direction splits (**Right / Left / Down / Up**) · user-defined **Tags** with semantic colors, sidebar filter bar, section headers, and <kbd>⌘ ⇧ L</kbd> tag editor · hibernate tabs to free memory |
| **Agent Awareness** | Per-pane detection (28 CLIs) · optional one-click hooks for live status dots, notifications, tray alerts, reboot resume, and session forking |
| **Prompt & Input** | Inline history ghost suggestions · explained tab completion · multi-line prompt & docked composer (<kbd>⌘ I</kbd>) · optional **Helix** modal editing · click-to-place caret · <kbd>⌃ R</kbd> fuzzy reverse search |
| **Side Panel & Git** | <kbd>⌘ J</kbd> / <kbd>Ctrl+Shift+J</kbd> · virtualized File Tree · Source Control changes, diffs & Commit Graph · Find in Files · live process tree & listening ports |
| **Built-in Code Editor** | Docked or fill-screen document viewer/editor · split editor groups · LSP integration · Git gutter & unified/split diff overlays · Markdown preview |
| **SSH & SFTP** | Built-in `russh` stack (no external `ssh` required) · profiles stored in `servers.json` with secrets in OS keychain · SFTP panel · port forwarding · jump hosts |
| **Shell Integration** | Auto-injected at pane startup (nothing to install) · prompt marks, working directory, exit codes, and long-command notifications across zsh, bash, fish, PowerShell, WSL, and remote panes |
| **Localization** | English (`en`) and Simplified Chinese (`zh-CN`), plus IME support and live `config.json` hot-reload |

## Install

### Pre-built Releases

Download native binaries for macOS, Windows, and Linux from [**GitHub Releases**](https://github.com/netkn001/anterm/releases):

| Platform | Artifact | Notes |
| --- | --- | --- |
| **macOS** | `anterm-…-macos-arm64.dmg` · `…-macos-x86_64.dmg` | Drag `anterm.app` into Applications (macOS 12.0+) |
| **Windows** | `anterm-…-setup.exe` · portable `….zip` | Static MSVC CRT; bundles Windows ConPTY runtime |
| **Linux** | `anterm-…-x86_64.AppImage` / `.tar.gz` | Standalone desktop app & headless `anterm-server` binary |

### Build from Source

Requires **Rust 1.85+** (Rust 2024 edition) and platform GUI build libraries (see [CONTRIBUTING.md](CONTRIBUTING.md#development-prerequisites) for Linux/macOS/Windows packages).

```bash
# Run the desktop app in dev mode using an isolated .anterm-dev/ config directory
cargo dev

# Build release binaries (anterm-app, anterm-server, anterm-updater) and package into dist/
./scripts/package-release.sh          # macOS .app/.zip or Linux .tar.gz + SHA256SUMS.txt
./scripts/package-release.sh --dmg    # (macOS) also create a drag-to-Applications .dmg
.\scripts\package-windows.ps1         # (Windows PowerShell) portable .zip + anterm-server.exe + checksums.txt
```

To build only the headless session daemon (zero GUI, SQLite, or keychain dependencies):

```bash
cargo build --release -p anterm-server
```

## Configuration & Keybindings

Configuration lives in a single hot-reloaded JSON file (while saved SSH hosts live separately in `servers.json` so you can sync dotfiles safely):

- **macOS / Linux**: `~/.config/anterm/config.json`
- **Windows**: `%APPDATA%\anterm\config.json` (or `<exe-dir>\data\config.json` for portable opt-in)
- **Override**: `--config-dir <path>` or `ANTERM_CONFIG_DIR`

See the dedicated reference guides:

- [**Configuration & CLI Reference**](docs/CONFIGURATION.md) — config paths, `config.json` keys, shell/agent overrides, proxy settings, and `anterm-app` / `anterm-server` CLI flags
- [**Keybindings Reference**](docs/KEYBINDINGS.md) — default shortcuts for macOS, Windows, and Linux, plus custom overrides and presets

## Supported Coding Agents

**Detection** works out of the box (brand icon, git branch + diff, tab title).
**Status & Resume** take one click in **Settings → Integrations** to install that agent's hook, enabling status dots, notifications, tray alerts, and session resume after restarts.
**Fork** is available for agents that support both session-ID hooks and a fork command.

<details>
<summary>Full support matrix (28 coding agents)</summary>

| Agent | Detected | Status & Resume | Fork |
| --- | :---: | :---: | :---: |
| **Claude Code** | ✓ | ✓ | ✓ |
| **Codex** | ✓ | ✓ | ✓ |
| **TraeCode** | ✓ | ✓ | ✓ |
| **Grok** | ✓ | ✓ | ✓ |
| **OpenCode** | ✓ | ✓ | ✓ |
| **Oh My Pi** | ✓ | ✓ | ✓ |
| **Prime Agent** | ✓ | ✓ | ✓ |
| **Droid** | ✓ | ✓ | ✓ |
| **Qwen Code** | ✓ | ✓ | ✓ |
| **Goose** | ✓ | ✓ | ✓ |
| **Qoder CLI** | ✓ | ✓ | ✓ |
| **Qoder CN CLI** | ✓ | ✓ | ✓ |
| **CodeBuddy** | ✓ | ✓ | ✓ |
| **Gemini** | ✓ | ✓ | |
| **Copilot** | ✓ | ✓ | |
| **Kimi Code** | ✓ | ✓ | |
| **Pi** | ✓ | ✓ | |
| **Crush** | ✓ | ✓ | |
| **Antigravity** | ✓ | ✓ | |
| **Cursor** | ✓ | ✓ | |
| **Empryo** | ✓ | ✓ | |
| **Muse Code** | ✓ | ✓ | |
| **jcode** | ✓ | ✓ | |
| Aider | ✓ | | |
| Amp | ✓ | | |
| Auggie | ✓ | | |
| Hermes | ✓ | | |
| Vibe | ✓ | | |

</details>

Agents are never wrapped or proxied — each runs directly inside a normal PTY with its native interface. Custom wrapper scripts can be mapped via `agent_commands`, and default launch flags can be customized via `agent_launch` in `config.json`.

## Architecture Overview

anterm is organized as a **13-crate acyclic Cargo virtual workspace**:

- **Headless Core & Tools (zero GPUI)**: `anterm-protocol`, `anterm-i18n`, `anterm-editor`, `anterm-core`, `anterm-server`, `anterm-updater`
- **Desktop GPUI UI Layer**: `anterm-ui-kit`, `anterm-terminal`, `anterm-remote-ui`, `anterm-files-ui`, `anterm-editor-ui`, `anterm-settings-ui`, `anterm-app`

See the [Architecture Guide](docs/ARCHITECTURE.md) for the complete dependency DAG, crate responsibilities, render-isolation invariants, and verification commands.

## Documentation & Community

- [Architecture Guide](docs/ARCHITECTURE.md) — 13-crate workspace DAG, module boundaries, and quality gates
- [Configuration & CLI Reference](docs/CONFIGURATION.md) — `config.json`, `servers.json`, environment overrides, and CLI options
- [Keybindings Reference](docs/KEYBINDINGS.md) — default shortcuts and customization syntax
- [Contributing Guide](CONTRIBUTING.md) — prerequisites, local dev workflow, and pre-PR checks
- [Changelog](CHANGELOG.md) · [Security Policy](SECURITY.md) · [Privacy Policy](PRIVACY.md) · [Code of Conduct](CODE_OF_CONDUCT.md) · [Third-Party Notices](THIRD_PARTY_NOTICES.md)

## License

Copyright (c) 2026 anterm contributors.

anterm is open-source software licensed under the [MIT License](LICENSE). See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for licenses of bundled fonts, runtimes, and third-party dependencies.
