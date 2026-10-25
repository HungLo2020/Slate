#!/usr/bin/env python3
"""Run Slate against an isolated headless Wayland compositor in CI."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

binary = sys.argv[1] if len(sys.argv) > 1 else "target/debug/slate"
with tempfile.TemporaryDirectory(prefix="slate-wayland-") as temporary:
    root = Path(temporary)
    root.chmod(0o700)
    environment = {**os.environ, "XDG_RUNTIME_DIR": temporary, "WAYLAND_DISPLAY": "slate-test",
                   "SLATE_GUI_PLATFORM": "wayland", "QT_QUICK_CONTROLS_STYLE": "org.kde.desktop",
                   "QT_QPA_PLATFORMTHEME": "kde", "SLATE_GUI_REQUIRE_KDE_DIALOGS": "1"}
    with (root / "weston.log").open("w") as log:
        compositor = subprocess.Popen(["weston", "--backend=headless", "--use-pixman", "--no-config",
                                       "--width=1920", "--height=1080", "--socket=slate-test", "--idle-time=0"],
                                      env=environment, stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 10
            while not (root / "slate-test").exists():
                assert compositor.poll() is None, (root / "weston.log").read_text()
                assert time.monotonic() < deadline, "Wayland compositor did not start"
                time.sleep(0.05)
            script = Path(__file__).with_name("gui-offscreen-smoke.py")
            subprocess.run([sys.executable, str(script), binary], env={**environment, "SLATE_GUI_DESKTOP_SMOKE": "1"}, check=True)
            subprocess.run([sys.executable, str(script), binary], env={**environment, "SLATE_GUI_FILE_DIALOG_SMOKE": "1"}, check=True)
            subprocess.run([sys.executable, str(Path(__file__).with_name("gui-desktop-integration.py")), binary], env=environment, check=True)
        finally:
            compositor.terminate()
            compositor.wait(timeout=5)
print("PASS Wayland desktop, native dialogs and accessibility")
