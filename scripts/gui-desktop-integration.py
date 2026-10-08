#!/usr/bin/env python3
"""Exercise Qt's real AT-SPI bridge in a private D-Bus session on X11/Wayland.

CI runs under Xvfb; SLATE_GUI_PLATFORM=wayland uses the current compositor.
No desktop preferences, clipboard contents or user documents are changed.
"""
import os
import shutil
from pathlib import Path
import subprocess
import sys
import tempfile
import time

binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/slate").resolve())
if "--child" not in sys.argv:
    raise SystemExit(subprocess.call(["dbus-run-session", "--", sys.executable, __file__, binary, "--child"],
                                    env={**os.environ, "GSETTINGS_BACKEND": "memory"}))

import gi
gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, Gio, GLib

# Enable assistive technology only on this test's private bus.
bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
bus.call_sync("org.a11y.Bus", "/org/a11y/bus", "org.freedesktop.DBus.Properties", "Set",
              GLib.Variant("(ssv)", ("org.a11y.Status", "ScreenReaderEnabled", GLib.Variant("b", True))),
              None, Gio.DBusCallFlags.NONE, 5000, None)
Atspi.init()

def editors(node):
    found = []
    if node.get_role() == Atspi.Role.TEXT and "accessibility-fixture" in node.get_name():
        found.append(node)
    for i in range(node.get_child_count()):
        child = node.get_child_at_index(i)
        if child:
            found.extend(editors(child))
    return found

with tempfile.TemporaryDirectory(prefix="slate-atspi-") as temporary:
    root = Path(temporary)
    content = "q" * 6000 + "猫👩‍💻\n" + "second line\n"
    file = root / "accessibility-fixture.txt"
    file.write_text(content)
    (root / "runtime").mkdir(mode=0o700)
    environment = {**os.environ, "QT_QPA_PLATFORM": os.environ.get("SLATE_GUI_PLATFORM", "xcb"),
                   "QT_LINUX_ACCESSIBILITY_ALWAYS_ON": "1", "SHELL": "/bin/sh",
                   "XDG_RUNTIME_DIR": str(root / "runtime"), "XDG_CONFIG_HOME": str(root / "config"),
                   "XDG_STATE_HOME": str(root / "state"), "XDG_DATA_HOME": str(root / "data")}
    environment.pop("SLATE_GUI_SMOKE_DIR", None)
    if environment["QT_QPA_PLATFORM"].startswith("wayland"):
        display = Path(os.environ.get("WAYLAND_DISPLAY", "wayland-0"))
        environment["WAYLAND_DISPLAY"] = str(display if display.is_absolute() else Path(os.environ["XDG_RUNTIME_DIR"]) / display)
    with (root / "gui.log").open("w") as log:
        process = subprocess.Popen([binary, "--gui", "--fresh", "--editor-only", str(file)], env=environment, stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 15
            while True:
                assert process.poll() is None, (root / "gui.log").read_text()
                matches = editors(Atspi.get_desktop(0))
                if matches:
                    break
                assert time.monotonic() < deadline, "Editor was not exposed through AT-SPI"
                time.sleep(0.05)
            text = matches[0].get_text_iface()
            # AT-SPI indexes Unicode code points; Qt converts its UTF-16 offsets.
            assert text.get_character_count() >= len(content), text.get_character_count()
            assert Atspi.Text.get_text(text, 0, 12) == "q" * 12
            assert text.set_caret_offset(0)
            assert text.get_caret_offset() == 0
            assert text.add_selection(0, 12)
            selection = Atspi.Text.get_selection(text, 0)
            assert selection.start_offset == 0 and selection.end_offset == 12
            assert text.remove_selection(0)
            assert Atspi.Text.get_text(text, 6000, 6001) == "猫"
            orca = os.environ.get("SLATE_ORCA_BINARY", "orca")
            if os.environ.get("SLATE_REQUIRE_ORCA") or shutil.which(orca):
                result = subprocess.run([orca, "--list-apps"], env={**environment, "LC_ALL": "C"},
                                        text=True, capture_output=True, timeout=15)
                assert result.returncode == 0 and "slate" in result.stdout.lower(), result.stdout + result.stderr
                print("PASS Orca discovers Slate through the private accessibility bus")
            print("PASS real AT-SPI: full-document ranges, stable offsets, cursor and selection operations")
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
    assert file.read_text() == content
