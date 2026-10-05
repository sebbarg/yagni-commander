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

# Only these shared libraries may be linked: a -sys crate that finds a
# library through pkg-config links it, and the app then fails to start
# where it is missing (macOS 1.2.0 and Homebrew's liblzma). A new one is a
# decision: link it statically, or add it here.
allowed=(libc.so.6 libm.so.6 libgcc_s.so.1 'ld-linux-*.so.*' libxcb.so.1 libxkbcommon.so.0 libxkbcommon-x11.so.0)
foreign=()
for lib in $(readelf -d "$stage/$app" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p'); do
    ok=
    for pattern in "${allowed[@]}"; do
        # shellcheck disable=SC2053 # $pattern is a glob on purpose
        [[ $lib == $pattern ]] && ok=1
    done
    [[ $ok ]] || foreign+=("$lib")
done
if (( ${#foreign[@]} )); then
    echo "$0: linked to libraries not on the list:" >&2
    printf '%s\n' "${foreign[@]}" >&2
    exit 1
fi
install -m755 "$root/scripts/install-linux.sh" "$stage/install.sh"
cp "$root/packaging/icons/$app.svg" "$stage/icons/"
cp -R "$root/packaging/icons/hicolor" "$stage/icons/"
"$root/scripts/copy-licenses.sh" "$stage"
tar -czf "$stage.tar.gz" -C "$(dirname "$stage")" "$name"
rm -rf "$stage"
echo "$stage.tar.gz"
