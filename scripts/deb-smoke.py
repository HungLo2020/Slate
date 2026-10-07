#!/usr/bin/env python3
"""Validate/extract a Debian package and exercise its GUI/TUI without installing it."""
import io
import os
import pathlib
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib

root = pathlib.Path(__file__).resolve().parent.parent
version = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]["version"]
artifact = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else root / f"builds/slate_{version}_amd64.deb").resolve()
fields = subprocess.check_output(["dpkg-deb", "--show", "--showformat=${Package}\n${Version}\n${Architecture}\n${Depends}\n${Recommends}\n", str(artifact)], text=True).splitlines()
assert fields[:3] == ["slate", version, "amd64"], fields
depends, recommends = fields[3], fields[4]
assert "private-abi" not in depends + recommends and " (= " not in depends + recommends, fields
# The terminal interface installs without Qt; the GUI's needs are recommended.
assert "qt6" not in depends and "libqt" not in depends.lower(), depends
for dependency in ("qml6-module-qtquick-controls", "qml6-module-qtqml-workerscript", "qt6-svg-plugins", "git"):
    assert dependency in recommends, dependency
assert "kirigami" not in depends + recommends, "The GUI no longer needs Kirigami"
assert "qt6" in recommends.lower(), recommends
contents = subprocess.check_output(["dpkg-deb", "--fsys-tarfile", str(artifact)])
with tarfile.open(fileobj=io.BytesIO(contents)) as archive:
    for member in archive:
        assert member.uid == member.gid == 0, member.name
        assert not member.name.startswith("/") and ".." not in pathlib.PurePosixPath(member.name).parts, member.name

with tempfile.TemporaryDirectory(prefix="slate-deb-smoke-") as temporary:
    work = pathlib.Path(temporary)
    destination = work / "package"
    subprocess.run(["dpkg-deb", "--extract", str(artifact), str(destination)], check=True)
    binary = destination / "usr/bin/slate"
    alias = destination / "usr/bin/slate-gui"
    assert not alias.is_symlink() and os.access(alias, os.X_OK), "slate-gui is its own executable"
    assert os.access(binary, os.X_OK)
    assert "libQt" not in subprocess.check_output(["ldd", str(binary)], text=True)
    assert subprocess.check_output([str(binary), "--version"], text=True).strip() == f"Slate {version}"
    assert (destination / "usr/share/icons/hicolor/scalable/apps/slate.svg").read_bytes() == (root / "resources/slate.svg").read_bytes()
    desktop = destination / "usr/share/applications/slate.desktop"
    assert "Exec=slate-gui %F" in desktop.read_text()
    assert (destination / "usr/share/metainfo/slate.metainfo.xml").exists()
    assert "Icon=slate" in desktop.read_text()
    subprocess.run([sys.executable, str(root / "scripts/tui-smoke.py"), str(binary)], check=True)
    subprocess.run([sys.executable, str(root / "scripts/startup-smoke.py"), str(binary), "--tui-only"], check=True)
    if "--tui-only" not in sys.argv[2:]:
        (work / "runtime").mkdir(mode=0o700)
        file = work / "edit.txt"; file.write_text("Packaged GUI\n")
        environment = {**os.environ, "QT_QPA_PLATFORM": "offscreen", "QT_QUICK_BACKEND": "software",
                       "XDG_CONFIG_HOME": str(work / "config"), "XDG_STATE_HOME": str(work / "state"),
                       "XDG_RUNTIME_DIR": str(work / "runtime"), "SHELL": "/bin/sh"}
        with (work / "gui.log").open("w+") as log:
            process = subprocess.Popen([str(alias), "--fresh", str(file)], env=environment, stdout=log, stderr=log)
            try:
                time.sleep(2)
                log.seek(0); output = log.read()
                assert process.poll() is None, output
                assert not any(error in output for error in ("failed to load", "is not installed", "ReferenceError:", "TypeError:")), output
            finally:
                if process.poll() is None:
                    process.terminate()
                process.wait(timeout=5)
print("PASS DEB: metadata, dependencies, root ownership, paths/alias, packaged TUI workflows and GUI startup")
