# Anterm Mobile (iOS)

Native **SwiftUI + Metal (120Hz)** iOS companion client backed by Anterm's shared Rust core (`anterm-mobile-ffi`, `anterm-gateway`, `anterm-client-core`, `anterm-core`, `anterm-protocol`).

## Architecture Highlights

- **Zero-Trust E2EE QUIC (`iroh`)**: Mutual Ed25519 QR pairing with automatic LAN direct upgrade, Wi-Fi ↔ 5G QUIC Connection Migration, and self-hostable `iroh-relay` HTTPS/WSS fallback — no VPN/TUN required.
- **Zero-PC-Interference 3-Layer Pane Rendering**:
  1. **Layer 1 (`Agent` / `TranscriptStream`)**: Streams structured JSONL transcripts for native SwiftUI Markdown, collapsible tool cards, and inline diffs (**0% impact on PC PTY**).
  2. **Layer 2 (`Reflow`)**: Pure `ObserveSince` delta replay + client-side `Flags::WRAPLINE` logical-line reflow (**0% impact on PC PTY**).
  3. **Layer 3 (`Terminal` / Metal)**: 120Hz `MTKView` consuming Rust's 16-byte/cell packed frame buffer in Peek mode by default, with an explicit opt-in **Fit Phone** (`SizeLease`) toggle for fullscreen TUIs (`vim`, `htop`) that auto-releases when you leave or type on PC.
- **Localhost Web Preview Tunnel**: Opens an iOS loopback TCP proxy tunneling `WKWebView` directly into whitelisted desktop dev-server ports over E2EE QUIC.

## Prerequisites

```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
brew install xcodegen
```

## Build Rust XCFramework + Xcode Project

```bash
bash mobile/ios/scripts/build-rust-xcframework.sh
```

Then open `mobile/ios/Anterm.xcodeproj` in Xcode, or build for Simulator from the command line:

```bash
xcodebuild \
  -project mobile/ios/Anterm.xcodeproj \
  -scheme Anterm \
  -destination 'generic/platform=iOS Simulator' \
  CODE_SIGNING_ALLOWED=NO \
  build
```

## Pairing with Desktop / Server

- **Desktop**: Open **Settings → Mobile**, enable Mobile Companion, and click **Generate pairing QR**.
- **Headless server**: Build with `--features gateway` and run:
  ```bash
  cargo run -p anterm-server --features gateway -- pair
  ```
