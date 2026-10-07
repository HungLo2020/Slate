#!/bin/sh
# Install Slate into PREFIX (default ~/.local):
#   bin/slate      terminal interface; does not link Qt
#   bin/slate-gui  graphical interface (and `slate-gui --tui`)
# --tui-only installs only bin/slate. --binary PATH installs a prebuilt
# `slate`; its sibling `slate-gui` is installed too unless --tui-only.
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
        cargo build --release --package slate
    else
        cargo build --release --package slate --package slate-gui
    fi
    binary=$(realpath "${CARGO_TARGET_DIR:-target}/release/slate")
fi
[ -x "$binary" ] || { printf 'Missing executable: %s\n' "$binary" >&2; exit 1; }
gui_binary=$(dirname "$binary")/slate-gui
if [ "$mode" = gui ] && [ ! -x "$gui_binary" ]; then
    printf 'Missing executable: %s (build slate-gui, or pass --tui-only)\n' "$gui_binary" >&2
    exit 1
fi
mkdir -p "$prefix/bin"
# Atomic replacement allows an already-running Slate to finish using its old binary.
install_binary() {
    staging=$(mktemp "$prefix/bin/.slate-install.XXXXXX")
    install -m755 "$1" "$staging"
    mv -f "$staging" "$prefix/bin/$2"
}
trap 'rm -f "$prefix"/bin/.slate-install.*' EXIT HUP INT TERM
install_binary "$binary" slate
if [ "$mode" = gui ]; then
    # Older releases installed slate-gui as a symlink to slate.
    rm -f "$prefix/bin/slate-gui"
    install_binary "$gui_binary" slate-gui
    repo_dir=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
    install -Dm644 "$repo_dir/packaging/slate.desktop" "$prefix/share/applications/slate.desktop"
    install -Dm644 "$repo_dir/packaging/slate.metainfo.xml" "$prefix/share/metainfo/slate.metainfo.xml"
    install -Dm644 "$repo_dir/resources/slate.svg" "$prefix/share/icons/hicolor/scalable/apps/slate.svg"
    if command -v update-desktop-database >/dev/null 2>&1; then update-desktop-database "$prefix/share/applications"; fi
fi
printf 'Installed Slate in %s/bin\n' "$prefix"
