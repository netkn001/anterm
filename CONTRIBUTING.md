# Contributing to anterm

Thank you for your interest in contributing to **anterm**! Whether you are reporting a bug, improving documentation, refining UI polish, or implementing a new feature, your help is welcome.

Please read our [Code of Conduct](CODE_OF_CONDUCT.md) before participating.

---

## Development Prerequisites

- **Rust toolchain**: Rust 1.85+ (`edition = "2024"`, workspace uses Cargo `resolver = "3"`). Install via [rustup](https://rustup.rs/).
- **Git**: used by Cargo (`[net] git-fetch-with-cli = true` in `.cargo/config.toml`).

### Platform System Dependencies

- **macOS**: Xcode Command Line Tools (`xcode-select --install`), macOS 12.0+.
- **Linux**:
  - Debian / Ubuntu:
    ```bash
    sudo apt-get update && sudo apt-get install -y \
      build-essential pkg-config cmake \
      libx11-dev libxkbcommon-dev libxkbcommon-x11-dev libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
      libwayland-dev libfontconfig1-dev libfreetype6-dev libvulkan-dev libasound2-dev libssl-dev
    ```
  - Fedora / Arch: install the equivalent `pkg-config`, `cmake`, X11/XCB/Wayland, `libxkbcommon`, `fontconfig`, `freetype`, `vulkan`, and OpenSSL development packages.
- **Windows**: Visual Studio Build Tools with the **Desktop development with C++** workload (MSVC links the CRT statically via `.cargo/config.toml`).

---

## Workspace Architecture

anterm is structured as a **13-crate acyclic Cargo virtual workspace** under `crates/`. Before making architectural changes, please read the [Architecture Guide](docs/ARCHITECTURE.md).

Key invariants to preserve:

1. **Strict Acyclic DAG**: UI feature crates (`anterm-terminal`, `anterm-remote-ui`, `anterm-files-ui`, `anterm-editor-ui`, `anterm-settings-ui`) must never depend upward on `anterm-app`.
2. **Headless Binary Cleanliness**: `anterm-server` and `anterm-updater` must never pull in `gpui`, `rusqlite`, or `keyring`.
3. **Zero Production `.unwrap()`**: `[workspace.lints.clippy]` enforces `unwrap_used = "deny"`, `dbg_macro = "deny"`, and `todo = "deny"` in all non-test code (`clippy.toml` permits `.unwrap()` inside tests).
4. **Render Isolation & Virtualization**: Keep high-frequency PTY updates isolated to `TerminalCanvas`, and use virtualized list views for unbounded collections (`FileTreeView`, `ScmPanelView`, `SftpPanelView`, `TabSidebar`).
5. **Localization**: User-facing strings live in `crates/anterm-i18n` and must provide exhaustive translations for both English (`en`) and Simplified Chinese (`zh-CN`).

---

## Running & Testing Locally

Run the app against an isolated throwaway config directory (`.anterm-dev/`) so development never modifies your real `~/.config/anterm/`:

```bash
cargo dev
```

Fast unit test loop for pure-Rust headless crates (no GPU/windowing needed):

```bash
cargo test -p anterm-protocol -p anterm-i18n -p anterm-editor -p anterm-core --lib
```

Targeted UI crate tests:

```bash
cargo test -p anterm-ui-kit -p anterm-remote-ui -p anterm-settings-ui -p anterm-files-ui -p anterm-editor-ui -p anterm-terminal -p anterm-app --lib
```

Full workspace tests:

```bash
cargo test --workspace --lib
```

---

## Required Pre-PR Checks

Please ensure all of the following pass cleanly (zero warnings, zero errors) before opening a pull request:

```bash
# 1. Compile all crates, binaries, examples, and tests with zero warnings
cargo check --workspace --all-targets --tests

# 2. Enforce zero production .unwrap() / dbg!() / todo!()
cargo clippy --workspace --all-targets -- -A clippy::all -D clippy::unwrap_used

# 3. Verify headless binaries stay free of GUI / SQLite / keychain dependencies
cargo tree -p anterm-server  | grep -E 'gpui|rusqlite|keyring' && exit 1 || echo "OK"
cargo tree -p anterm-updater | grep -E 'gpui|rusqlite|keyring' && exit 1 || echo "OK"

# 4. Run tests
cargo test --workspace --lib
```

---

## Submitting a Pull Request

1. Fork the repository and create a focused branch from `main`.
2. Keep PRs cohesive — separate refactoring from feature changes where practical.
3. Add regression tests for bug fixes and new behavior.
4. Update relevant documentation (`README.md`, `README.zh-CN.md`, `docs/`, `CHANGELOG.md`) when behavior, shortcuts, or configuration keys change.
5. Open a pull request describing the motivation, design trade-offs, and verification steps.

## Licensing

By contributing to anterm, you agree that your contributions will be licensed under the [MIT License](LICENSE).
