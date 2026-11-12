#!/bin/bash
# Build yamc.app and zip it as dist/yamc-macos.zip (the asset the in-app updater looks for).
#
#   TARGETS        Rust targets to build and merge (default: Apple Silicon only)
#   SIGN_IDENTITY  codesign identity (default "-": ad-hoc, no certificate)
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/yamc/Cargo.toml | head -1)
TARGETS=${TARGETS:-"aarch64-apple-darwin"}
SIGN_IDENTITY=${SIGN_IDENTITY:--}
export MACOSX_DEPLOYMENT_TARGET=11.0

bins=()
for t in $TARGETS; do
  cargo build --release --locked -p yamc --target "$t"
  bins+=("target/$t/release/yamc")
done

APP=dist/yamc.app
rm -rf dist
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/yamc" "${bins[@]}"
cp assets/yamc.icns "$APP/Contents/Resources/yamc.icns"
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>dev.yamc.app</string>
  <key>CFBundleName</key><string>yamc</string>
  <key>CFBundleDisplayName</key><string>yamc</string>
  <key>CFBundleExecutable</key><string>yamc</string>
  <key>CFBundleIconFile</key><string>yamc</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>$MACOSX_DEPLOYMENT_TARGET</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF

codesign --force --sign "$SIGN_IDENTITY" "$APP"
codesign --verify --deep --strict "$APP"
ditto -c -k --keepParent "$APP" dist/yamc-macos.zip
echo "dist/yamc-macos.zip (yamc $VERSION, $(lipo -archs "$APP/Contents/MacOS/yamc"))"
