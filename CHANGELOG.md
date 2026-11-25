# Changelog

All notable changes to **anterm** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-11-25

### Added

- **Initial open-source release under the MIT License**: pure-Rust GPU-accelerated terminal workbench built on [GPUI Kit](https://gpui-kit.com) (`gpui-kit 0.10.0`) and `alacritty_terminal`, backed by a persistent headless session daemon (`anterm-server`) and standalone auto-updater (`anterm-updater`).
- **13-crate acyclic Cargo virtual workspace**: headless core & tooling (`anterm-protocol`, `anterm-i18n`, `anterm-editor`, `anterm-core`, `anterm-server`, `anterm-updater`) cleanly separated from desktop GPUI UI crates (`anterm-ui-kit`, `anterm-terminal`, `anterm-remote-ui`, `anterm-files-ui`, `anterm-editor-ui`, `anterm-settings-ui`, `anterm-app`) with zero production `.unwrap()` (`unwrap_used = "deny"`).
- **Persistent sessions, 4-direction splits & user-defined tab tags**: daemon-owned PTYs surviving window closes and restarts, four-direction splitting (`Right` / `Left` / `Down` / `Up`), tab hibernation, and daemon-synced tab tags with semantic color dots, single-select sidebar filter bar, section headers, and `Edit Tab Tags` (`⌘⇧L`).
- **Zed-style Command Palette (`⌘P` / `Ctrl+Shift+P`)**: frecency-ranked pure command palette with dedicated sub-pickers (`Switch Tab…`, `Switch Tag…`, `Change Theme`, `Go to File`, `Go to Symbol`, `SSH Hosts`, `Shells`).
- **Editor-grade terminal input & coding agent awareness**: inline history ghost suggestions, explained tab completion, syntax highlighting, multi-line prompt & docked composer (`⌘I`), optional Helix modal editing, `Ctrl+R` fuzzy history search, zero-install shell integration (zsh, bash, fish, PowerShell, WSL, remote), and native detection/hooks/resume/fork support for 28 CLI coding agents.
- **Native SSH, SFTP & port forwarding**: built-in pure-Rust `russh` stack with separate `servers.json` profiles, OS keychain credential storage, virtualized SFTP file browser, local/remote/dynamic port forwarding, jump hosts, and unprivileged remote `anterm-server` deployment.
- **Built-in code editor & virtualized Git SCM**: docked or full-screen code editor with LSP diagnostics/completion/navigation, `syntect` syntax highlighting, Markdown preview, side-by-side and unified diff overlays, virtualized file tree, and windowed Git commit graph.
- **Localization, packaging & community docs**: full English (`en`) and Simplified Chinese (`zh-CN`) localization, `scripts/package-release.sh` (`.app` / `.zip` / `--dmg` / Linux tarballs + `SHA256SUMS.txt`), `docs/` guides, GitHub templates, and CI workflow.

### Fixed

- **Startup & split initial PTY grid sizing**: estimate exact initial PTY rows/cols and Retina device-pixel cell metrics from window/pane bounds + font shaping before spawning local and native SSH PTYs, and skip redundant Frame 1 `ClientMsg::Resize` / `SIGWINCH` signals to prevent zsh `PROMPT_SP` `%%` artifacts.

[Unreleased]: https://github.com/netkn001/anterm/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/netkn001/anterm/releases/tag/v0.1.0
