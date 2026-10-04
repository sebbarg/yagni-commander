#!/usr/bin/env bash
# Builds the Linux release tarball: target/dist/yagni-commander-<version>-linux-<arch>.tar.gz
# with the binary, `install.sh` (install-linux.sh), the icons and the
# licenses. Users unpack it and run ./install.sh. Built on the oldest
# distribution to support: the binary needs that glibc or newer.
set -euo pipefail

app=yagni-commander
root=$(cd "$(dirname "$0")/.." && pwd)
cargo=$(command -v cargo || echo "$HOME/.cargo/bin/cargo")
"$cargo" build --release --locked --manifest-path "$root/Cargo.toml" -p $app
version=$("$cargo" pkgid --manifest-path "$root/Cargo.toml" -p $app | sed 's/.*[#@]//')

name=$app-$version-linux-$(uname -m)
stage=$root/target/dist/$name
rm -rf "$stage" "$stage.tar.gz"
mkdir -p "$stage/icons"
install -m755 "$root/target/release/$app" "$stage/$app"
strip "$stage/$app"
install -m755 "$root/scripts/install-linux.sh" "$stage/install.sh"
cp "$root/packaging/icons/$app.svg" "$stage/icons/"
cp -R "$root/packaging/icons/hicolor" "$stage/icons/"
"$root/scripts/copy-licenses.sh" "$stage"
tar -czf "$stage.tar.gz" -C "$(dirname "$stage")" "$name"
rm -rf "$stage"
echo "$stage.tar.gz"
