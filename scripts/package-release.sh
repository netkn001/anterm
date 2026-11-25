#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

SKIP_BUILD=0
BUILD_DMG=0
TARGET=""
SIGN_IDENTITY="${SIGN_IDENTITY:--}"
OUT_DIR="$ROOT_DIR/dist"

usage() {
    cat <<'EOF'
Usage: scripts/package-release.sh [OPTIONS]

Build and package anterm release artifacts into dist/.

Options:
  --skip-build          Skip `cargo build --release --workspace` and package existing binaries
  --dmg                 (macOS) Also create a drag-to-Applications .dmg image
  --sign <IDENTITY>     (macOS) codesign identity (default: "-", ad-hoc; or env SIGN_IDENTITY)
  --target <TRIPLE>     Optional Cargo target triple (e.g. aarch64-apple-darwin)
  --out-dir <DIR>       Output directory (default: dist)
  -h, --help            Show this help message
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        --dmg)
            BUILD_DMG=1
            shift
            ;;
        --sign)
            [[ $# -ge 2 ]] || { echo "error: --sign requires an argument" >&2; exit 1; }
            SIGN_IDENTITY="$2"
            shift 2
            ;;
        --target)
            [[ $# -ge 2 ]] || { echo "error: --target requires an argument" >&2; exit 1; }
            TARGET="$2"
            shift 2
            ;;
        --out-dir)
            [[ $# -ge 2 ]] || { echo "error: --out-dir requires an argument" >&2; exit 1; }
            OUT_DIR="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "error: unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

VERSION="$(awk -F'"' '/^version = / { print $2; exit }' "$ROOT_DIR/Cargo.toml")"
if [[ -z "$VERSION" ]]; then
    echo "error: failed to read workspace version from Cargo.toml" >&2
    exit 1
fi

OS="$(uname -s)"
RAW_ARCH="$(uname -m)"
case "$RAW_ARCH" in
    aarch64|arm64) HOST_ARCH="arm64" ;;
    x86_64|amd64)  HOST_ARCH="x86_64" ;;
    *)             HOST_ARCH="$RAW_ARCH" ;;
esac
if [[ -n "$TARGET" ]]; then
    case "$TARGET" in
        aarch64-*) ARCH="arm64";  SERVER_ARCH="aarch64" ;;
        x86_64-*)  ARCH="x86_64"; SERVER_ARCH="x86_64" ;;
        *)         ARCH="$RAW_ARCH"; SERVER_ARCH="$RAW_ARCH" ;;
    esac
    BIN_DIR="$ROOT_DIR/target/$TARGET/release"
else
    case "$RAW_ARCH" in
        aarch64|arm64) ARCH="arm64";  SERVER_ARCH="aarch64" ;;
        x86_64|amd64)  ARCH="x86_64"; SERVER_ARCH="x86_64" ;;
        *)             ARCH="$RAW_ARCH"; SERVER_ARCH="$RAW_ARCH" ;;
    esac
    BIN_DIR="$ROOT_DIR/target/release"
fi

if [[ "$SKIP_BUILD" -eq 0 ]]; then
    echo "==> Building release binaries separately (version $VERSION, target: ${TARGET:-host})..."
    # Build each binary crate individually instead of `--workspace` so Cargo's
    # workspace feature unification does not leak `anterm-app`'s `gssapi` and
    # `remote-install` (`ureq`/`rustls`/`ring`) features into `anterm-server`.
    for pkg in anterm-app anterm-server anterm-updater; do
        echo "  -> cargo build --release -p $pkg"
        if [[ -n "$TARGET" ]]; then
            cargo build --release -p "$pkg" --target "$TARGET"
        else
            cargo build --release -p "$pkg"
        fi
    done
else
    echo "==> Skipping cargo build (--skip-build); using binaries from $BIN_DIR"
fi

for bin in anterm-app anterm-server anterm-updater; do
    if [[ ! -f "$BIN_DIR/$bin" ]]; then
        echo "error: missing release binary: $BIN_DIR/$bin" >&2
        exit 1
    fi
done

mkdir -p "$OUT_DIR"

checksum_files=()

package_macos() {
    local app="$OUT_DIR/anterm.app"
    local zip="$OUT_DIR/anterm-${VERSION}-macos-${ARCH}.zip"
    local dmg="$OUT_DIR/anterm-${VERSION}-macos-${ARCH}.dmg"
    local srv="$OUT_DIR/anterm-server-macos-${SERVER_ARCH}"

    echo "==> Assembling $app ..."
    rm -rf "$app" "$zip" "$dmg" "$srv" "$OUT_DIR/SHA256SUMS.txt" "$OUT_DIR/checksums.txt"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

    cp "$BIN_DIR/anterm-app"     "$app/Contents/MacOS/anterm-app"
    cp "$BIN_DIR/anterm-updater" "$app/Contents/MacOS/anterm-updater"
    cp "$ROOT_DIR/assets/anterm.icns" "$app/Contents/Resources/anterm.icns"
    chmod +x \
        "$app/Contents/MacOS/anterm-app" \
        "$app/Contents/MacOS/anterm-updater"

    cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleDisplayName</key>
    <string>anterm</string>
    <key>CFBundleExecutable</key>
    <string>anterm-app</string>
    <key>CFBundleIconFile</key>
    <string>anterm.icns</string>
    <key>CFBundleIdentifier</key>
    <string>com.anterm.app</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>anterm</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>LSMinimumSystemVersion</key>
    <string>12.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

    plutil -lint "$app/Contents/Info.plist"

    echo "==> Signing $app (identity: $SIGN_IDENTITY) ..."
    codesign --force --deep --sign "$SIGN_IDENTITY" "$app"
    codesign --verify --deep --strict "$app"

    echo "==> Creating $zip ..."
    /usr/bin/ditto -c -k --sequesterRsrc --keepParent "$app" "$zip"
    checksum_files+=("$(basename "$zip")")

    if [[ "$BUILD_DMG" -eq 1 ]]; then
        echo "==> Creating $dmg ..."
        local staging
        staging="$(mktemp -d)"
        trap 'rm -rf "$staging"' RETURN
        cp -R "$app" "$staging/anterm.app"
        ln -s /Applications "$staging/Applications"
        hdiutil create \
            -volname "anterm ${VERSION}" \
            -srcfolder "$staging" \
            -ov -format UDZO \
            "$dmg"
        checksum_files+=("$(basename "$dmg")")
    fi

    cp "$BIN_DIR/anterm-server" "$srv"
    chmod +x "$srv"
    checksum_files+=("$(basename "$srv")")

    (cd "$OUT_DIR" && shasum -a 256 "${checksum_files[@]}" | tee SHA256SUMS.txt > checksums.txt)

    if [[ "$ARCH" == "$HOST_ARCH" ]]; then
        echo "==> Smoke check:"
        "$srv" --version
    else
        echo "==> Skipping smoke check (cross-built $ARCH on $HOST_ARCH host)"
    fi
}

package_linux() {
    local stage="$OUT_DIR/anterm-${VERSION}-linux-${ARCH}"
    local tar="$OUT_DIR/anterm-${VERSION}-linux-${ARCH}.tar.gz"
    local srv="$OUT_DIR/anterm-server-linux-${SERVER_ARCH}"

    echo "==> Packaging $tar ..."
    rm -rf "$stage" "$tar" "$srv" "$OUT_DIR/SHA256SUMS.txt" "$OUT_DIR/checksums.txt"
    mkdir -p "$stage"
    cp "$BIN_DIR/anterm-app" "$BIN_DIR/anterm-updater" "$stage/"
    chmod +x "$stage/anterm-app" "$stage/anterm-updater"

    tar -czf "$tar" -C "$OUT_DIR" "$(basename "$stage")"
    rm -rf "$stage"
    checksum_files+=("$(basename "$tar")")

    cp "$BIN_DIR/anterm-server" "$srv"
    chmod +x "$srv"
    checksum_files+=("$(basename "$srv")")

    (cd "$OUT_DIR" && sha256sum "${checksum_files[@]}" | tee SHA256SUMS.txt > checksums.txt)

    if [[ "$ARCH" == "$HOST_ARCH" ]]; then
        echo "==> Smoke check:"
        "$srv" --version
    else
        echo "==> Skipping smoke check (cross-built $ARCH on $HOST_ARCH host)"
    fi
}

case "$OS" in
    Darwin) package_macos ;;
    Linux)  package_linux ;;
    *)
        echo "error: unsupported host OS: $OS" >&2
        exit 1
        ;;
esac

echo
echo "==> Release artifacts in $OUT_DIR:"
(cd "$OUT_DIR" && ls -lh "${checksum_files[@]}" SHA256SUMS.txt)
echo
echo "==> SHA256SUMS.txt:"
cat "$OUT_DIR/SHA256SUMS.txt"
