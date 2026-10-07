#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
builds_dir="$repo_root/builds"
build_meta="$builds_dir/latest-build.env"

for tool in cargo python3 dpkg dpkg-deb dpkg-shlibdeps install; do
    command -v "$tool" >/dev/null || { echo "[build] Missing required command: $tool" >&2; exit 1; }
done
if [[ "$(uname -s)/$(uname -m)" != Linux/x86_64 || "$(dpkg --print-architecture)" != amd64 ]]; then
    echo "[build] Build the amd64 package on an amd64 Debian/Ubuntu host" >&2
    exit 1
fi

cd "$repo_root"
version="$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"])')"
dpkg --validate-version "$version"
mkdir -p "$builds_dir"
# A failed build must not leave metadata pointing at a previous package.
rm -f "$build_meta"
staging="$(mktemp -d "$builds_dir/.slate-package.XXXXXX")"
trap 'rm -rf "$staging"' EXIT

echo "[build] Building Slate $version (TUI slate and GUI slate-gui)" >&2
cargo build --locked --release --package slate --package slate-gui
target_dir="$(cargo metadata --locked --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
package_root="$staging/pkgroot"
mkdir -p "$package_root/DEBIAN" "$package_root/usr/bin" "$staging/debian"
install -m755 "$target_dir/release/slate" "$package_root/usr/bin/slate"
install -m755 "$target_dir/release/slate-gui" "$package_root/usr/bin/slate-gui"
install -Dm644 packaging/slate.desktop "$package_root/usr/share/applications/slate.desktop"
install -Dm644 resources/slate.svg "$package_root/usr/share/icons/hicolor/scalable/apps/slate.svg"
install -Dm644 README.md "$package_root/usr/share/doc/slate/README.md"

echo "[build] Computing shared-library dependencies" >&2
printf 'Source: slate\n\nPackage: slate\nArchitecture: amd64\n' > "$staging/debian/control"
# `slate` (terminal) must install on servers without Qt: only its own
# libraries are hard dependencies. The GUI's libraries are recommended, so a
# default install gets both and --no-install-recommends gets the TUI alone.
shlib_depends="$(cd "$staging" && dpkg-shlibdeps -O "$package_root/usr/bin/slate" | sed -n 's/^shlibs:Depends=//p')"
gui_depends="$(cd "$staging" && dpkg-shlibdeps -O "$package_root/usr/bin/slate-gui" | sed -n 's/^shlibs:Depends=//p')"
if [[ -z "$shlib_depends" || -z "$gui_depends" || "$gui_depends" == *private-abi* ]]; then
    echo "[build] Missing shared dependencies or forbidden Qt private ABI: $shlib_depends / $gui_depends" >&2
    exit 1
fi
if [[ "$shlib_depends" == *qt6* || "$shlib_depends" == *libqt* ]]; then
    echo "[build] The terminal executable must not depend on Qt: $shlib_depends" >&2
    exit 1
fi
# QML imports/plugins are loaded dynamically and aren't found by shlibdeps.
qml_depends="qml6-module-org-kde-kirigami, qml6-module-qtquick, qml6-module-qtquick-controls, qml6-module-qtquick-layouts, qml6-module-qtquick-templates, qml6-module-qtquick-window, qml6-module-qtqml, qml6-module-qtqml-models, qml6-module-qtqml-workerscript, qt6-svg-plugins"
installed_size="$(du -sk "$package_root/usr" | cut -f1)"
cat > "$package_root/DEBIAN/control" <<EOF
Package: slate
Version: $version
Section: editors
Priority: optional
Architecture: amd64
Maintainer: Slate Maintainers
Installed-Size: $installed_size
Depends: $shlib_depends
Recommends: $gui_depends, $qml_depends, git, plasma-integration, qml6-module-org-kde-desktop, breeze-icon-theme, fonts-dejavu-core
Suggests: aspell | hunspell, wl-clipboard | xclip
Homepage: https://github.com/HungLo2020/Slate
Description: Shared graphical and terminal text editor
 Slate provides a Kirigami graphical editor and a terminal editor using the
 same Rust core, with configurable layouts, recovery, Git and PTY terminals.
 Invoke slate for the terminal interface or slate-gui for the graphical one.
 The terminal interface does not need Qt; install with --no-install-recommends
 on servers to omit the graphical components.
EOF

deb_path="$builds_dir/slate_${version}_amd64.deb"
dpkg-deb --root-owner-group --build "$package_root" "$staging/slate.deb"
mv -f "$staging/slate.deb" "$deb_path"
cat > "$staging/latest-build.env" <<EOF
BUILD_PLATFORM=linux-amd64
BUILD_ARTIFACT_TYPE=deb
BUILD_ARTIFACT_PATH=$deb_path
BUILD_ARTIFACT_NAME=$(basename "$deb_path")
EOF
mv -f "$staging/latest-build.env" "$build_meta"
echo "[build] Built artifact: $deb_path" >&2
echo "[build] Metadata: $build_meta" >&2
