#!/usr/bin/env bash
# Releases <version>: sets the workspace version, updates Cargo.lock, runs
# the tests, commits "Release v<version>", tags it and pushes both to
# origin, which starts the release workflow. It asks once, before the slow
# part, so nothing waits for an answer at the end.
#
#   scripts/release.sh 0.2.0
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
cargo=$(command -v cargo || echo "$HOME/.cargo/bin/cargo")

fail() {
    echo "release.sh: $*" >&2
    exit 1
}

[[ $# == 1 && $1 =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "usage: $0 <major.minor.patch>"
version=$1
tag=v$version

[[ $(git branch --show-current) == main ]] || fail "not on main"
[[ -z $(git status --porcelain) ]] || fail "the working tree has changes; commit or stash them first"
git fetch -q origin
[[ $(git rev-parse HEAD) == $(git rev-parse origin/main) ]] || fail "main differs from origin/main; pull or push first"
# Tags here and on origin: fetch only follows tags on main's commits.
tags=$( (git tag -l 'v*'; git ls-remote --tags --refs origin 'v*' | sed 's|.*refs/tags/||') | sort -u)
! grep -qxF "$tag" <<<"$tags" || fail "tag $tag exists already"

# Newer than both Cargo.toml and the highest release tag.
current=$("$cargo" pkgid -p yagni-commander | sed 's/.*[#@]//')
released=$(grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' <<<"$tags" | sed 's/^v//' | sort -V | tail -1 || true)
for older in $current $released; do
    newest=$(printf '%s\n%s\n' "$older" "$version" | sort -V | tail -1)
    [[ $version != "$older" && $newest == "$version" ]] || fail "$version is not newer than $older"
done

read -r -p "Release $current -> $version and push main and $tag to origin? [y/N] " answer || true
[[ ${answer:-} == [yY] ]] || fail "cancelled"

# Only the version line of [workspace.package].
sed -i.bak "/^\[workspace.package\]/,/^\[/ s/^version = \".*\"/version = \"$version\"/" Cargo.toml
rm Cargo.toml.bak
"$cargo" update --workspace -q
[[ $("$cargo" pkgid -p yagni-commander | sed 's/.*[#@]//') == "$version" ]] || fail "could not set the version in Cargo.toml"
"$cargo" test --workspace -q

git commit -q -am "Release $tag"
git tag -a "$tag" -m "$tag"
# Atomic: if origin moved meanwhile, neither main nor the tag is pushed.
git push -q --atomic origin main "$tag" ||
    fail "push failed; the release commit and tag are local. Retry with: git push --atomic origin main $tag"
echo "Pushed $tag; the release workflow builds a draft release on GitHub."
