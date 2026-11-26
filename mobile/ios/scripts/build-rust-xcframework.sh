#!/usr/bin/env bash
set -euo pipefail

# Builds `anterm-mobile-ffi` static libraries for iOS device + Simulator,
# generates Swift bindings via UniFFI, packages `AntermMobileFFI.xcframework`,
# and regenerates `mobile/ios/Anterm.xcodeproj` using XcodeGen.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IOS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$IOS_DIR/../.." && pwd)"

PROFILE="${ANTERM_RUST_PROFILE:-debug}"
if [[ "$PROFILE" == "release" ]]; then
  CARGO_PROFILE_FLAG="--release"
  TARGET_SUBDIR="release"
else
  CARGO_PROFILE_FLAG=""
  TARGET_SUBDIR="debug"
fi

GENERATED_DIR="$IOS_DIR/Anterm/Generated"
HEADERS_DIR="$IOS_DIR/build/headers"
XCFRAMEWORK_OUT="$IOS_DIR/Frameworks/AntermMobileFFI.xcframework"

mkdir -p "$GENERATED_DIR" "$HEADERS_DIR" "$IOS_DIR/Frameworks"

echo "==> Building anterm-mobile-ffi for aarch64-apple-ios ($PROFILE)..."
cargo build -p anterm-mobile-ffi --target aarch64-apple-ios $CARGO_PROFILE_FLAG

echo "==> Building anterm-mobile-ffi for aarch64-apple-ios-sim ($PROFILE)..."
cargo build -p anterm-mobile-ffi --target aarch64-apple-ios-sim $CARGO_PROFILE_FLAG

DEVICE_LIB="$REPO_ROOT/target/aarch64-apple-ios/$TARGET_SUBDIR/libanterm_mobile_ffi.a"
SIM_LIB="$REPO_ROOT/target/aarch64-apple-ios-sim/$TARGET_SUBDIR/libanterm_mobile_ffi.a"

echo "==> Generating Swift UniFFI bindings..."
rm -rf "$GENERATED_DIR"/* "$HEADERS_DIR"/*
cargo run -p anterm-mobile-ffi --features bindgen --bin uniffi-bindgen -- \
  generate \
  --library "$SIM_LIB" \
  --language swift \
  --out-dir "$GENERATED_DIR"

# Move C header & modulemap into XCFramework headers directory so Swift imports
# `anterm_mobile_ffiFFI` cleanly without duplicating headers in the app target.
mv "$GENERATED_DIR"/*.h "$HEADERS_DIR/"
if ls "$GENERATED_DIR"/*.modulemap >/dev/null 2>&1; then
  mv "$GENERATED_DIR"/*.modulemap "$HEADERS_DIR/module.modulemap"
fi

echo "==> Packaging AntermMobileFFI.xcframework..."
rm -rf "$XCFRAMEWORK_OUT"
xcodebuild -create-xcframework \
  -library "$DEVICE_LIB" -headers "$HEADERS_DIR" \
  -library "$SIM_LIB" -headers "$HEADERS_DIR" \
  -output "$XCFRAMEWORK_OUT"

if command -v xcodegen >/dev/null 2>&1; then
  echo "==> Running xcodegen..."
  (cd "$IOS_DIR" && xcodegen generate --spec project.yml)
else
  echo "note: xcodegen not found; skipping .xcodeproj generation"
fi

echo "==> Done! XCFramework ready at $XCFRAMEWORK_OUT"
