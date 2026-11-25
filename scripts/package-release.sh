#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

SKIP_BUILD=0
BUILD_DMG=0
BUILD_APPIMAGE=0
TARGET=""
SIGN_IDENTITY="${SIGN_IDENTITY:--}"
OUT_DIR="$ROOT_DIR/dist"

usage() {
    cat <<'EOF'
Usage: scripts/package-release.sh [OPTIONS]

Build and package anterm release artifacts into dist/.

Options:
  --skip-build          Skip `cargo build --release` and package existing binaries
  --dmg                 (macOS) Also create a drag-to-Applications .dmg image
  --appimage            (Linux) Also create a self-updating .AppImage (requires appimagetool)
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
        --appimage)
            BUILD_APPIMAGE=1
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

write_linux_desktop_entry() {
    local dest="$1"
    cat > "$dest" <<EOF
[Desktop Entry]
Type=Application
Name=anterm
Comment=GPU-accelerated terminal workbench with persistent sessions, SSH/SFTP, editor, and coding agent support
Exec=anterm-app %F
Icon=anterm
Terminal=false
Categories=System;TerminalEmulator;Development;
StartupWMClass=anterm
X-AppImage-Version=${VERSION}
EOF
}

package_linux() {
    local stage="$OUT_DIR/anterm-${VERSION}-linux-${ARCH}"
    local tar="$OUT_DIR/anterm-${VERSION}-linux-${ARCH}.tar.gz"
    local appimage="$OUT_DIR/anterm-${VERSION}-linux-${ARCH}.AppImage"
    local srv="$OUT_DIR/anterm-server-linux-${SERVER_ARCH}"

    echo "==> Packaging $tar ..."
    rm -rf "$stage" "$tar" "$appimage" "$srv" "$OUT_DIR/SHA256SUMS.txt" "$OUT_DIR/checksums.txt"
    mkdir -p \
        "$stage/completions" \
        "$stage/share/applications" \
        "$stage/share/icons/hicolor/256x256/apps" \
        "$stage/share/icons/hicolor/scalable/apps"

    cp "$BIN_DIR/anterm-app" "$BIN_DIR/anterm-updater" "$stage/"
    chmod +x "$stage/anterm-app" "$stage/anterm-updater"
    cp "$ROOT_DIR/LICENSE" "$ROOT_DIR/README.md" "$stage/"
    cp "$ROOT_DIR"/assets/completions/*.json "$stage/completions/"
    cp "$ROOT_DIR/assets/logo@256.png" "$stage/share/icons/hicolor/256x256/apps/anterm.png"
    cp "$ROOT_DIR/assets/app-icon.svg" "$stage/share/icons/hicolor/scalable/apps/anterm.svg"
    write_linux_desktop_entry "$stage/share/applications/anterm.desktop"

    tar -czf "$tar" -C "$OUT_DIR" "$(basename "$stage")"
    rm -rf "$stage"
    checksum_files+=("$(basename "$tar")")

    if [[ "$BUILD_APPIMAGE" -eq 1 ]]; then
        local appimagetool_bin="${APPIMAGETOOL:-appimagetool}"
        if ! command -v "$appimagetool_bin" >/dev/null 2>&1; then
            echo "error: --appimage requires 'appimagetool' on PATH (or set APPIMAGETOOL)" >&2
            exit 1
        fi
        echo "==> Creating $appimage ..."
        local appdir
        appdir="$(mktemp -d)"
        trap 'rm -rf "$appdir"' RETURN
        mkdir -p \
            "$appdir/usr/bin" \
            "$appdir/usr/share/anterm/completions" \
            "$appdir/usr/share/applications" \
            "$appdir/usr/share/icons/hicolor/256x256/apps" \
            "$appdir/usr/share/icons/hicolor/scalable/apps"

        cp "$BIN_DIR/anterm-app" "$BIN_DIR/anterm-updater" "$appdir/usr/bin/"
        chmod +x "$appdir/usr/bin/anterm-app" "$appdir/usr/bin/anterm-updater"
        cp "$ROOT_DIR"/assets/completions/*.json "$appdir/usr/share/anterm/completions/"
        cp "$ROOT_DIR/assets/logo@256.png" "$appdir/usr/share/icons/hicolor/256x256/apps/anterm.png"
        cp "$ROOT_DIR/assets/app-icon.svg" "$appdir/usr/share/icons/hicolor/scalable/apps/anterm.svg"
        # Exact path and X-AppImage-Version key verified by crates/anterm-updater/src/linux.rs.
        write_linux_desktop_entry "$appdir/usr/share/applications/anterm.desktop"
        ln -s "usr/share/applications/anterm.desktop" "$appdir/anterm.desktop"
        ln -s "usr/share/icons/hicolor/256x256/apps/anterm.png" "$appdir/anterm.png"
        ln -s "anterm.png" "$appdir/.DirIcon"

        cat > "$appdir/AppRun" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/anterm-app" "$@"
EOF
        chmod +x "$appdir/AppRun"

        local appimage_arch="$RAW_ARCH"
        case "$ARCH" in
            x86_64) appimage_arch="x86_64" ;;
            arm64)  appimage_arch="aarch64" ;;
        esac
        ARCH="$appimage_arch" "$appimagetool_bin" "$appdir" "$appimage"
        rm -rf "$appdir"
        trap - RETURN
        chmod +x "$appimage"
        checksum_files+=("$(basename "$appimage")")
    fi

    cp "$BIN_DIR/anterm-server" "$srv"
    chmod +x "$srv"
    checksum_files+=("$(basename "$srv")")

    (cd "$OUT_DIR" && sha256sum "${checksum_files[@]}" | tee SHA256SUMS.txt > checksums.txt)

    if [[ "$ARCH" == "$HOST_ARCH" ]]; then
        echo "==> Smoke check:"
        "$srv" --version
        if [[ "$BUILD_APPIMAGE" -eq 1 ]]; then
            local verify_stage
            verify_stage="$(mktemp -d)"
            trap 'rm -rf "$verify_stage"' RETURN
            "$BIN_DIR/anterm-updater" verify "$appimage" "$OUT_DIR/checksums.txt" "$(basename "$appimage")" "$verify_stage" "$VERSION"
            rm -rf "$verify_stage"
            trap - RETURN
            echo "  -> anterm-updater verify: OK"
        fi
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
