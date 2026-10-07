#!/usr/bin/env bash
set -euo pipefail
list="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/apt-dependencies.txt"
mapfile -t packages < <(sed 's/#.*//; /^[[:space:]]*$/d' "$list")
as_root=()
if [[ "$(id -u)" != 0 ]]; then as_root=(sudo); fi
"${as_root[@]}" apt-get update
"${as_root[@]}" apt-get install -y --no-install-recommends "${packages[@]}"
