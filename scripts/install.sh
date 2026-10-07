#!/bin/sh
set -eu
prefix=${1:-"$HOME/.local"}
if [ "$#" -gt 0 ]; then shift; fi
mode=gui
binary=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --tui-only) mode=tui ;;
        --binary) shift; binary=${1:?--binary requires a path} ;;
        *) printf 'Unknown option: %s\n' "$1" >&2; exit 2 ;;
    esac
    shift
done
# Resolve a supplied binary before changing directory.
if [ -n "$binary" ]; then
    binary=$(realpath "$binary")
else
    cd "$(dirname "$0")/.."
    if [ "$mode" = tui ]; then
        cargo build --release --no-default-features
    else
        cargo build --release
    fi
    binary=${CARGO_TARGET_DIR:-target}/release/slate
fi
[ -x "$binary" ] || { printf 'Missing executable: %s\n' "$binary" >&2; exit 1; }
# Atomic replacement allows an already-running Slate to finish using its old binary.
mkdir -p "$prefix/bin"
staging=$(mktemp "$prefix/bin/.slate-install.XXXXXX")
trap 'rm -f "$staging"' EXIT HUP INT TERM
install -m755 "$binary" "$staging"
mv -f "$staging" "$prefix/bin/slate"
if [ "$mode" = gui ]; then
    ln -sfn slate "$prefix/bin/slate-gui"
    repo_dir=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
    install -Dm644 "$repo_dir/packaging/slate.desktop" "$prefix/share/applications/slate.desktop"
    install -Dm644 "$repo_dir/resources/slate.svg" "$prefix/share/icons/hicolor/scalable/apps/slate.svg"
    if command -v update-desktop-database >/dev/null 2>&1; then update-desktop-database "$prefix/share/applications"; fi
fi
printf 'Installed Slate in %s/bin\n' "$prefix"
