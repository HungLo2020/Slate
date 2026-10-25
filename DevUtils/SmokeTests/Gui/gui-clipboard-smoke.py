#!/usr/bin/env python3
"""Check terminal copying from another process on an isolated KWin Wayland seat.

Requires kwin_wayland, dbus-run-session and Python PyQt6. Build the binary with
`cargo build -p slate-gui --features smoke`. Uses temporary config/state and
an isolated compositor; never reads or changes the user's desktop clipboard.
Set SLATE_GUI_TEST_SHELL to exercise a shell other than /bin/sh.
"""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time


if "--read" in sys.argv:
    # A separate Qt client must receive the compositor's clipboard offer. A
    # same-process QClipboard::text() can succeed without publishing anything.
    from PyQt6.QtCore import QTimer
    from PyQt6.QtWidgets import QApplication, QWidget

    application = QApplication(sys.argv)
    window = QWidget()
    window.setWindowTitle("Slate clipboard test receiver")
    window.show()
    window.activateWindow()

    def read():
        sys.stdout.write(application.clipboard().text())
        sys.stdout.flush()
        application.quit()

    QTimer.singleShot(500, read)
    raise SystemExit(application.exec())


def main():
    script = Path(__file__).resolve()
    binary = str(Path(next((arg for arg in sys.argv[1:] if arg != "--child"),
                           "target/debug/slate-gui")).resolve())
    for command in ("kwin_wayland", "dbus-run-session"):
        if not shutil.which(command):
            raise SystemExit(f"Required test dependency missing: {command}")
    import PyQt6.QtWidgets  # Fail before starting the compositor if unavailable.

    if "--child" not in sys.argv:
        # Portal helpers can outlive the private bus and inherit its handles.
        # Files let the test finish without waiting for those descendants.
        with tempfile.TemporaryFile(mode="w+") as stdout, tempfile.TemporaryFile(mode="w+") as stderr:
            result = subprocess.run(["dbus-run-session", "--", sys.executable,
                                     str(script), binary, "--child"], stdout=stdout, stderr=stderr,
                                    timeout=90)
            stdout.seek(0)
            print(stdout.read(), end="")
            if result.returncode:
                stderr.seek(0)
                print(stderr.read(), end="", file=sys.stderr)
        return result.returncode

    with tempfile.TemporaryDirectory(prefix="slate-clipboard-") as temporary:
        root = Path(temporary)
        root.chmod(0o700)
        environment = {**os.environ, "HOME": str(root), "XDG_RUNTIME_DIR": str(root),
                       "XDG_CONFIG_HOME": str(root / "config"),
                       "XDG_STATE_HOME": str(root / "state"),
                       "XDG_DATA_HOME": str(root / "data"),
                       "QT_QPA_PLATFORM": "offscreen", "KWIN_COMPOSE": "Q"}
        with (root / "kwin.log").open("w") as log:
            compositor = subprocess.Popen(
                ["kwin_wayland", "--virtual", "--socket", "slate-test",
                 "--width", "1600", "--height", "1000", "--no-lockscreen"],
                env=environment, stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 10
                while not (root / "slate-test").exists():
                    assert compositor.poll() is None, (root / "kwin.log").read_text()
                    assert time.monotonic() < deadline, "Test compositor did not start"
                    time.sleep(0.05)
                environment.update(
                    WAYLAND_DISPLAY=str(root / "slate-test"),
                    SLATE_GUI_PLATFORM="wayland",
                    SLATE_GUI_CLIPBOARD_PROBE=str(script))
                subprocess.run([sys.executable, str(script.with_name("gui-offscreen-smoke.py")),
                                binary], env=environment, check=True, timeout=60)
            finally:
                compositor.terminate()
                compositor.wait(timeout=10)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
