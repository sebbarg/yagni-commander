#!/usr/bin/env bash
# Builds a release binary and installs it for the current user, with a
# launcher entry and the app icon, so the desktop's app menu finds it.
#
#   scripts/install-linux.sh             install into ~/.local
#   scripts/install-linux.sh uninstall   remove what install put there
#
# PREFIX (default ~/.local) moves the whole install; it must be writable
# by you, since the build never runs as root.
set -euo pipefail

app=yagni-commander
root=$(cd "$(dirname "$0")/.." && pwd)
prefix=${PREFIX:-$HOME/.local}
bin=$prefix/bin/$app
desktop=$prefix/share/applications/$app.desktop
icons=$prefix/share/icons/hicolor

refresh() {
    # Optional caches; desktops rescan without them. The icon cache only
    # if one is there already: a new one would hide icons other programs
    # add later without updating it.
    if command -v update-desktop-database >/dev/null; then
        update-desktop-database -q "$prefix/share/applications" || true
    fi
    if [[ -f $icons/icon-theme.cache ]] && command -v gtk-update-icon-cache >/dev/null; then
        gtk-update-icon-cache -q -t "$icons" || true
    fi
}

if [[ ${1:-} == uninstall ]]; then
    rm -f "$bin" "$desktop" "$icons/scalable/apps/$app.svg" "$icons"/*/apps/"$app".png
    refresh
    echo "removed $app from $prefix"
    exit 0
elif [[ $# -gt 0 ]]; then
    echo "usage: $0 [uninstall]" >&2
    exit 2
fi

cargo=$(command -v cargo || echo "$HOME/.cargo/bin/cargo")
"$cargo" build --release --manifest-path "$root/Cargo.toml" -p $app

install -Dm755 "$root/target/release/$app" "$bin"
install -Dm644 "$root/packaging/icons/$app.svg" "$icons/scalable/apps/$app.svg"
for png in "$root"/packaging/icons/hicolor/*/apps/"$app".png; do
    size=$(basename "$(dirname "$(dirname "$png")")")
    install -Dm644 "$png" "$icons/$size/apps/$app.png"
done

# Exec has the full path: ~/.local/bin is not on every session's PATH.
# StartupWMClass is the app id the windows carry (`windows::APP_ID`), so
# the taskbar matches them to this entry and shows the icon.
mkdir -p "$(dirname "$desktop")"
cat >"$desktop" <<EOF
[Desktop Entry]
Type=Application
Name=yagni-commander
GenericName=File Manager
Comment=Dual-pane file manager
Exec="$bin"
Icon=$app
Terminal=false
Categories=System;FileTools;FileManager;
Keywords=files;folders;commander;dual pane;
StartupWMClass=$app
EOF

refresh
echo "installed $bin"
echo "launcher: $desktop"
