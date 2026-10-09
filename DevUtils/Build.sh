#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ "$(uname -s)/$(uname -m)" != Linux/x86_64 ]]; then
    echo "[build] Supported package platform: Linux/x86_64 (amd64)" >&2
    exit 1
fi

cd "$repo_root"
python3 scripts/dependency-policy.py
python3 -m unittest discover -s tests -v
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
bash DevUtils/BuildScripts/build-linux-amd64.sh
python3 scripts/deb-smoke.py
