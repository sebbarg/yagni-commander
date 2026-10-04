#!/bin/sh
# Installs yagni-commander from a GitHub release, for the current user:
#
#   curl -fsSL https://github.com/sebbarg/yagni-commander/releases/latest/download/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --uninstall
#
# macOS: yagni-commander.app into ~/Applications. Linux (x86_64): the
# binary, a launcher entry and the icons into ~/.local (PREFIX moves it).
# Each download is checked against the release's SHA256SUMS first.
#
# The release workflow fills in VERSION, so a release's script installs
# that release. Fetched with curl, the app carries no quarantine flag, so
# macOS opens it although it is not notarized.
#
# Everything runs inside main, so a download cut off halfway runs nothing.

set -eu

VERSION=@VERSION@
REPO=sebbarg/yagni-commander
APP=yagni-commander

fail() {
    echo "install.sh: $*" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || fail "needs $1"
}

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

# Downloads release file $1 into the work folder and checks it.
fetch() {
    curl -fsSL -o "$work/$1" "$base/$1" || fail "could not download $base/$1"
    expected=$(awk -v f="$1" '$2 == f || $2 == "*" f { print $1 }' "$work/SHA256SUMS")
    [ -n "$expected" ] || fail "$1 is not in SHA256SUMS"
    [ "$(sha256 "$work/$1")" = "$expected" ] || fail "$1 does not match its checksum"
}

main() {
    case $VERSION in
    @*) VERSION=${YAGNI_COMMANDER_VERSION:-} ;;
    esac
    [ -n "$VERSION" ] || fail "no version: run the install.sh of a release (see the README)"
    # The release folder; a local one (file://...) for testing.
    base=${YAGNI_COMMANDER_RELEASES:-https://github.com/$REPO/releases/download}/v$VERSION

    uninstall=0
    case ${1:-} in
    "") ;;
    --uninstall) uninstall=1 ;;
    *) fail "usage: install.sh [--uninstall]" ;;
    esac

    need curl
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT

    case $(uname -s) in
    Darwin)
        apps=$HOME/Applications
        if [ $uninstall = 1 ]; then
            rm -rf "${apps:?}/$APP.app"
            echo "removed $apps/$APP.app"
            return
        fi
        need ditto
        file=$APP-$VERSION-macos.zip
        curl -fsSL -o "$work/SHA256SUMS" "$base/SHA256SUMS" || fail "could not download $base/SHA256SUMS"
        fetch "$file"
        ditto -x -k "$work/$file" "$work"
        mkdir -p "$apps"
        rm -rf "${apps:?}/$APP.app"
        mv "$work/$APP.app" "$apps/"
        echo "installed $apps/$APP.app ($VERSION)"
        ;;
    Linux)
        [ "$(uname -m)" = x86_64 ] || fail "no release for $(uname -m); build it from source (see the README)"
        need tar
        name=$APP-$VERSION-linux-x86_64
        curl -fsSL -o "$work/SHA256SUMS" "$base/SHA256SUMS" || fail "could not download $base/SHA256SUMS"
        fetch "$name.tar.gz"
        tar -xzf "$work/$name.tar.gz" -C "$work"
        if [ $uninstall = 1 ]; then
            "$work/$name/install.sh" uninstall
        else
            "$work/$name/install.sh"
        fi
        ;;
    *)
        fail "no release for $(uname -s); build it from source (see the README)"
        ;;
    esac
}

main "$@"
