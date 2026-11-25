# Third-Party Notices

**anterm** is licensed under the [MIT License](LICENSE).

This repository bundles or builds on the following third-party software and assets, each governed by its respective license.

## Bundled Assets & Vendored Code

### Hack Typeface
- **Location**: `assets/fonts/hack/`
- **Copyright**: Copyright (c) 2018 Source Foundry Authors; Bitstream Vera Sans Mono Copyright (c) 2003 Bitstream, Inc.
- **License**: MIT License and Bitstream Vera License (full text in [`assets/fonts/hack/LICENSE.md`](assets/fonts/hack/LICENSE.md))

### Microsoft Windows ConPTY Runtime
- **Location**: `assets/windows/conpty/` (`conpty.dll`, `OpenConsole.exe`)
- **Source**: <https://github.com/microsoft/terminal>
- **Copyright**: Copyright (c) Microsoft Corporation
- **License**: MIT License (full text in [`assets/windows/conpty/LICENSE.txt`](assets/windows/conpty/LICENSE.txt))

### bash-preexec
- **Location**: Vendored inside `crates/anterm-core/src/daemon/shell_integration/scripts/bash.bash`
- **Source**: <https://github.com/rcaloras/bash-preexec>
- **License**: MIT License

## Core Open-Source Dependencies

In addition to all crates declared in `Cargo.toml` and `Cargo.lock`, anterm relies heavily on:

- **[gpui-kit](https://gpui-kit.com)** & **[GPUI](https://github.com/zed-industries/zed)** — GPU-accelerated Rust UI framework and desktop component library
- **[alacritty_terminal](https://github.com/alacritty/alacritty)** — VT100/xterm terminal emulation core (Apache-2.0)
- **[russh](https://github.com/Eugeny/russh)** — Pure-Rust SSH2 client, SFTP, and port-forwarding stack (Apache-2.0)
- **[syntect](https://github.com/trishume/syntect)** — TextMate syntax highlighting engine (MIT)

Run `cargo metadata --format-version 1` to inspect the full dependency graph and exact license metadata of every Cargo dependency.
