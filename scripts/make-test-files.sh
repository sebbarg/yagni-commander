#!/usr/bin/env bash
# Creates a directory full of small files for testing how the UI copes with
# huge listings. Works on Linux and macOS.
#
# usage: scripts/make-test-files.sh [count] [dir]
#   count  number of files (default 10000); also creates count/100 subdirectories
#   dir    target directory (default /tmp/fm-test-<count>)
#
# Re-running with the same dir replaces it, but only if this script created it.

set -euo pipefail

count=${1:-10000}
dir=${2:-/tmp/fm-test-$count}
marker=.fm-test-files

if ! [[ $count =~ ^[0-9]+$ ]]; then
    echo "count must be a non-negative integer, got: $count" >&2
    exit 1
fi

if [[ -e $dir ]]; then
    if [[ ! -f $dir/$marker ]]; then
        echo "refusing to replace $dir: it was not created by this script" >&2
        exit 1
    fi
    rm -rf -- "$dir"
fi
mkdir -p -- "$dir"
touch -- "$dir/$marker"

# Mixed prefixes, cases and extensions so sorting has something to do.
prefixes=(alpha Beta gamma Delta report IMG_ notes Zeta _underscore .hidden)
exts=(txt log md jpg rs json)

dirs=$((count / 100))
for ((i = 0; i < dirs; i++)); do
    mkdir -- "$dir/${prefixes[i % ${#prefixes[@]}]}-dir-$i"
done

start=$SECONDS
for ((i = 0; i < count; i++)); do
    name="${prefixes[i % ${#prefixes[@]}]}-$i.${exts[i % ${#exts[@]}]}"
    # Content varies in length so the size column varies too.
    printf '%*d\n' $((i % 4000)) "$i" > "$dir/$name"
    if ((i > 0 && i % 50000 == 0)); then
        echo "  $i files..."
    fi
done

echo "created $count files and $dirs directories in $dir ($((SECONDS - start))s)"
