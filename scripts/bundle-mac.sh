#!/usr/bin/env bash
# Builds yagni-commander.app from a release build and installs it, so it
# starts from Finder, Launchpad and Spotlight with its icon.
#
#   scripts/bundle-mac.sh                build and install into ~/Applications
#   scripts/bundle-mac.sh --no-install   only build target/release/yagni-commander.app
#   scripts/bundle-mac.sh --dist         a universal (arm64 + x86_64) bundle, zipped as
#                                        target/dist/yagni-commander-<version>-macos.zip
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
dest=${APP_DIR:-$HOME/Applications}

case ${1:-} in
    "") mode=install ;;
    --no-install) mode=build ;;
    --dist) mode=dist ;;
    *) echo "usage: $0 [--no-install | --dist]" >&2; exit 2 ;;
esac

[[ $(uname) == Darwin ]] || { echo "$0: macOS only" >&2; exit 1; }

cargo=$(command -v cargo || echo "$HOME/.cargo/bin/cargo")
manifest=$root/Cargo.toml
version=$("$cargo" pkgid --manifest-path "$manifest" -p $app | sed 's/.*[#@]//')
bundle=$root/target/release/$app.app
rm -rf "$bundle"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"

if [[ $mode == dist ]]; then
    # One binary for Apple Silicon and Intel Macs.
    targets=(aarch64-apple-darwin x86_64-apple-darwin)
    # Build with rustup's toolchain, the one the targets are added to: a
    # cargo or rustc found first on PATH (Homebrew's rust) has only its own
    # std. RUSTC too, since cargo looks rustc up on PATH.
    toolchain=$(rustup show active-toolchain | cut -d' ' -f1)
    rustup target add --toolchain "$toolchain" "${targets[@]}"
    for target in "${targets[@]}"; do
        RUSTC=$(rustup which --toolchain "$toolchain" rustc) \
            "$(rustup which --toolchain "$toolchain" cargo)" build --release --locked \
            --manifest-path "$manifest" -p $app --target "$target"
    done
    lipo -create -output "$bundle/Contents/MacOS/$app" \
        "$root/target/${targets[0]}/release/$app" "$root/target/${targets[1]}/release/$app"
else
    "$cargo" build --release --manifest-path "$manifest" -p $app
    cp "$root/target/release/$app" "$bundle/Contents/MacOS/$app"
fi
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
iconutil -c icns "$root/packaging/icons/$app.iconset" -o "$bundle/Contents/Resources/$app.icns"
"$root/scripts/copy-licenses.sh" "$bundle/Contents/Resources"

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

if [[ $mode == dist ]]; then
    zip=$root/target/dist/$app-$version-macos.zip
    mkdir -p "$(dirname "$zip")"
    rm -f "$zip"
    # ditto keeps the bundle and its signature intact; plain zip may not.
    ditto -c -k --keepParent "$bundle" "$zip"
    echo "$zip"
elif [[ $mode == install ]]; then
    mkdir -p "$dest"
    # Only ever our own bundle at this path; a running copy keeps working
    # until it quits.
    rm -rf "${dest:?}/$app.app"
    cp -R "$bundle" "$dest/"
    echo "installed $dest/$app.app"
fi
