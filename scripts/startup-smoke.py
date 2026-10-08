#!/usr/bin/env python3
"""Exercise startup defaults, overrides and settings in real GUI/TUI processes.

Build with: cargo build -p slate -p slate-gui --features slate-gui/smoke
Pass --tui-only to test only the terminal interface.
"""
import fcntl
import json
import os
import pathlib
import pty
import re
import select
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import time

binary = str(pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/slate").resolve())
tui_only = "--tui-only" in sys.argv[2:]
cases = [
    ("file", [], None, True),
    ("directory", [], None, False),
    ("no-path", [], None, False),
    ("file", ["--workspace"], None, False),
    ("directory", ["--editor-only"], None, True),
    ("file", [], 'file_startup = "workspace"\n', False),
    ("directory", [], 'directory_startup = "editor-only"\n', True),
    ("file", ["--editor-only"], 'file_startup = "workspace"\n', True),
    ("directory", ["--workspace"], 'directory_startup = "editor-only"\n', False),
    ("file", [], '[global_keys]\nf6 = "next-pane"\n', True),
]

def tui(root, workspace, args, environment, expected, exercise_settings):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 160, 0, 0))
    process = subprocess.Popen([binary, "--tui", "--fresh", *args], cwd=workspace,
                               stdin=slave, stdout=slave, stderr=slave,
                               env=environment, start_new_session=True)
    os.close(slave)
    output = bytearray()
    def pump(seconds=.2):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if select.select([master], [], [], .02)[0]:
                try:
                    data = os.read(master, 65536)
                except OSError:
                    break
                output.extend(data)
                if b"\x1b[6n" in data:
                    os.write(master, b"\x1b[1;1R")
    def send(data):
        os.write(master, data)
        pump()
    def command(text):
        send(b"\x1bOP")
        send((":" + text).encode())
        send(b"\r")
    try:
        pump(.8)
        assert process.poll() is None, output.decode(errors="replace")
        text = re.sub(rb'\x1b\[[0-?]*[ -/]*[@-~]', b'', output)
        labels = re.sub(rb'\s+', b'', text)
        assert b"editor#2" in labels, output.decode(errors="replace")
        assert (b"files#1" not in labels) == expected, output.decode(errors="replace")
        assert (b"terminal#3" not in labels) == expected
        assert (not (root / "shell-starts").exists()) == expected
        send(b"\x1b[21~")  # F10
        send(b"\x1b[21~")
        assert (root / "shell-starts").read_text().splitlines() == ["started"]
        if exercise_settings:
            send(b"\x01")
            send(b"startup modes preserve edits")
            send(b"\x13")
            deadline = time.monotonic() + 5
            while (workspace / "edit.txt").read_text() != "startup modes preserve edits":
                assert process.poll() is None, output.decode(errors="replace")[-4000:]
                assert time.monotonic() < deadline, output.decode(errors="replace")[-4000:]
                pump(.05)
            command("settings")
            assert b"Opening a file" in output and b"automatically" in output, output.decode(errors="replace")[-7000:]
            send(b"\r")  # File: workspace.
            send(b"\x1b[B")
            send(b"\r")  # Directory: editor-only.
            settings = (root / "config/slate/settings.toml").read_text()
            assert 'file_startup = "workspace"' in settings, settings
            assert 'directory_startup = "editor-only"' in settings, settings
            send(b"\x1b")
        command("quit")
        process.wait(timeout=5)
        assert process.returncode == 0, output.decode(errors="replace")
        return (root / "config/slate/settings.toml").read_text() if exercise_settings else None
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=5)
        os.close(master)

def gui(root, workspace, args, environment, expected):
    environment = {**environment, "QT_QPA_PLATFORM": "offscreen", "QT_QUICK_BACKEND": "software",
                   "SLATE_GUI_SMOKE_DIR": str(root), "SLATE_GUI_STARTUP_SMOKE": "1",
                   "SLATE_GUI_EXPECT_EDITOR_ONLY": "1" if expected else "0"}
    result = subprocess.run([binary, "--gui", "--fresh", *args], cwd=workspace,
                            env=environment, capture_output=True, text=True, timeout=20)
    report = json.loads((root / "report.json").read_text()) if (root / "report.json").exists() else {}
    logs = result.stdout + result.stderr
    assert result.returncode == 0 and report.get("pass"), f"{report}\n{logs}"
    assert not any(error in logs for error in ("ReferenceError:", "TypeError:", "Cannot anchor to an item", "Binding loop detected", "Unable to assign")), logs
    artifacts = os.environ.get("SLATE_STARTUP_ARTIFACT_DIR")
    if artifacts:
        destination = pathlib.Path(artifacts) / ("editor-only" if expected else "workspace")
        destination.mkdir(parents=True, exist_ok=True)
        for artifact in root.glob("*.png"):
            shutil.copyfile(artifact, destination / artifact.name)

frontends = ["tui"] if tui_only else ["tui", "gui"]
for frontend in frontends:
    for index, (target, flags, config, expected) in enumerate(cases):
        with tempfile.TemporaryDirectory(prefix="slate-startup-") as temporary:
            root = pathlib.Path(temporary)
            workspace = root / "workspace"
            workspace.mkdir()
            file = workspace / "edit.txt"
            file.write_text("original\n")
            (root / "runtime").mkdir(mode=0o700)
            shell = root / "shell"
            shell.write_text('#!/bin/sh\nprintf "started\\n" >> "' + str(root / "shell-starts") + '"\nexec /bin/sh "$@"\n')
            shell.chmod(0o700)
            if config:
                (root / "config/slate").mkdir(parents=True)
                (root / "config/slate/settings.toml").write_text(config)
            environment = {**os.environ, "TERM": "xterm-256color", "SHELL": str(shell),
                           "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state"),
                           "XDG_RUNTIME_DIR": str(root / "runtime")}
            args = [*flags, *([] if target == "no-path" else [str(file if target == "file" else workspace)])]
            if frontend == "gui":
                gui(root, workspace, args, environment, expected)
            else:
                saved = tui(root, workspace, args, environment, expected, index == 0)
                if saved:
                    (root / "shell-starts").unlink()
                    tui(root, workspace, [str(file)], environment, False, False)
            print(f"PASS {frontend}: {target}, {flags or 'configured default'}", flush=True)
print(f"PASS {len(frontends) * len(cases)} startup cases, settings persistence and command-line precedence")
