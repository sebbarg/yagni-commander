#!/usr/bin/env bash
# Assembles target/release/yagni-commander.app around a built binary (the
# release build unless one is given), with its icon and licenses. Called by
# ./build.sh on macOS and by release/_dist-mac.sh with a universal binary;
# install/install-mac.sh installs the bundle.
#
#   packaging/bundle-mac.sh [binary]
#
# The bundle is signed ad hoc (`codesign -s -`), which is enough on the Mac
# that built it: a local build carries no quarantine flag, so Gatekeeper
# never asks. A downloaded copy is quarantined and, not being notarized,
# blocked until the user clears the flag (see the README).
set -euo pipefail

app=yagni-commander
# macOS keys preferences and permissions by the bundle id, so keep it
# stable once installed.
bundle_id=dk.yagni.yagni-commander
root=$(cd "$(dirname "$0")/.." && pwd)
binary=${1:-$root/target/release/$app}

[[ $(uname) == Darwin ]] || { echo "$0: macOS only" >&2; exit 1; }
[[ -x $binary ]] || { echo "$0: no $binary; run ./build.sh first" >&2; exit 1; }

cargo=$(command -v cargo || echo "$HOME/.cargo/bin/cargo")
version=$("$cargo" pkgid --manifest-path "$root/Cargo.toml" -p $app | sed 's/.*[#@]//')
bundle=$root/target/release/$app.app
rm -rf "$bundle"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
cp "$binary" "$bundle/Contents/MacOS/$app"
strip "$bundle/Contents/MacOS/$app"

# Only the system's libraries may be linked: a -sys crate that finds a
# Homebrew library through pkg-config links its path, and the app then dies
# at launch on Macs without it (1.2.0 and Homebrew's liblzma).
foreign=$(otool -L -arch all "$bundle/Contents/MacOS/$app" |
    awk '/^\t/ && $1 !~ /^\/(usr\/lib|System)\// { print $1 }' | sort -u)
if [[ -n $foreign ]]; then
    echo "$0: linked to libraries outside the system:" >&2
    echo "$foreign" >&2
    exit 1
fi
iconutil -c icns "$root/app-icon/$app.iconset" -o "$bundle/Contents/Resources/$app.icns"
"$root/packaging/copy-licenses.sh" "$bundle/Contents/Resources"

# Never add LSFileQuarantineEnabled: macOS would quarantine what the app
# and its children write, so an update from inside the app would be blocked
# at its next start (see core `update::install`).
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

