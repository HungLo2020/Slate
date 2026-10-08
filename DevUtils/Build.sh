#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ "$(uname -s)/$(uname -m)" != Linux/x86_64 ]]; then
    echo "[build] Supported package platform: Linux/x86_64 (amd64)" >&2
    exit 1
fi

cd "$repo_root"
python3 scripts/dependency-policy.py
cargo clippy --workspace --all-targets --all-features -- -D warnings
bash DevUtils/BuildScripts/build-linux-amd64.sh
