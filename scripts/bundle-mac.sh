#!/usr/bin/env bash
# Builds yagni-commander.app from a release build and installs it, so it
# starts from Finder, Launchpad and Spotlight with its icon.
#
#   scripts/bundle-mac.sh            build and install into ~/Applications
#   scripts/bundle-mac.sh --no-install   only build target/release/yagni-commander.app
#
# The bundle is signed ad hoc (`codesign -s -`), which is enough on the Mac
# that built it: a local build carries no quarantine flag, so Gatekeeper
# never asks. A copy downloaded elsewhere would need notarization.
set -euo pipefail

app=yagni-commander
# macOS keys preferences and permissions by the bundle id, so keep it
# stable once installed.
bundle_id=dk.yagni.yagni-commander
root=$(cd "$(dirname "$0")/.." && pwd)
dest=${APP_DIR:-$HOME/Applications}

case ${1:-} in
    "") install=1 ;;
    --no-install) install=0 ;;
    *) echo "usage: $0 [--no-install]" >&2; exit 2 ;;
esac

[[ $(uname) == Darwin ]] || { echo "$0: macOS only" >&2; exit 1; }

cargo=$(command -v cargo || echo "$HOME/.cargo/bin/cargo")
"$cargo" build --release --manifest-path "$root/Cargo.toml" -p $app
version=$("$cargo" pkgid --manifest-path "$root/Cargo.toml" -p $app | sed 's/.*[#@]//')

bundle=$root/target/release/$app.app
rm -rf "$bundle"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
cp "$root/target/release/$app" "$bundle/Contents/MacOS/$app"
iconutil -c icns "$root/packaging/icons/$app.iconset" -o "$bundle/Contents/Resources/$app.icns"

cat >"$bundle/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>en</string>
    <key>CFBundleDisplayName</key><string>$app</string>
    <key>CFBundleExecutable</key><string>$app</string>
    <key>CFBundleIconFile</key><string>$app</string>
    <key>CFBundleIdentifier</key><string>$bundle_id</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>CFBundleName</key><string>$app</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF
plutil -lint "$bundle/Contents/Info.plist" >/dev/null

codesign --force --sign - "$bundle"
echo "built $bundle ($version)"

if [[ $install == 1 ]]; then
    mkdir -p "$dest"
    # Only ever our own bundle at this path; a running copy keeps working
    # until it quits.
    rm -rf "${dest:?}/$app.app"
    cp -R "$bundle" "$dest/"
    echo "installed $dest/$app.app"
fi
