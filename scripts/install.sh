#!/bin/sh
set -eu
prefix=${1:-"$HOME/.local"}
cd "$(dirname "$0")/.."
cargo build --release
install -Dm755 target/release/slate "$prefix/bin/slate"
ln -sfn slate "$prefix/bin/slate-gui"
printf 'Installed Slate in %s/bin\n' "$prefix"
