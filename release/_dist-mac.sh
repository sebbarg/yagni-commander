#!/usr/bin/env bash
# Builds the macOS release zip: one binary for Apple Silicon and Intel Macs,
# bundled by packaging/bundle-mac.sh and zipped as
# target/dist/yagni-commander-<version>-macos.zip. Run by the release
# workflow.
set -euo pipefail

app=yagni-commander
root=$(cd "$(dirname "$0")/.." && pwd)
manifest=$root/Cargo.toml

[[ $(uname) == Darwin ]] || { echo "$0: macOS only" >&2; exit 1; }

targets=(aarch64-apple-darwin x86_64-apple-darwin)
# Build with rustup's toolchain, the one the targets are added to: a cargo
# or rustc found first on PATH (Homebrew's rust) has only its own std.
# RUSTC too, since cargo looks rustc up on PATH.
toolchain=$(rustup show active-toolchain | cut -d' ' -f1)
cargo=$(rustup which --toolchain "$toolchain" cargo)
rustup target add --toolchain "$toolchain" "${targets[@]}"
for target in "${targets[@]}"; do
    RUSTC=$(rustup which --toolchain "$toolchain" rustc) \
        "$cargo" build --release --locked --manifest-path "$manifest" -p $app --target "$target"
done
universal=$root/target/universal-apple-darwin/release/$app
mkdir -p "$(dirname "$universal")"
lipo -create -output "$universal" \
    "$root/target/${targets[0]}/release/$app" "$root/target/${targets[1]}/release/$app"
"$root/packaging/bundle-mac.sh" "$universal"

version=$("$cargo" pkgid --manifest-path "$manifest" -p $app | sed 's/.*[#@]//')
zip=$root/target/dist/$app-$version-macos.zip
mkdir -p "$(dirname "$zip")"
rm -f "$zip"
# ditto keeps the bundle and its signature intact; plain zip may not.
ditto -c -k --keepParent "$root/target/release/$app.app" "$zip"
echo "$zip"
