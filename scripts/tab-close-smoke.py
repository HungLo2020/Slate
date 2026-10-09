#!/usr/bin/env python3
"""Exercise actual TUI tab close mouse input under a PTY, including Unicode."""
import fcntl
import os
import pathlib
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

binary = str(pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/slate").resolve())
with tempfile.TemporaryDirectory(prefix="slate-tab-close-") as temporary:
    root = pathlib.Path(temporary)
    first = root / "猫.txt"
    second = root / "second.txt"
    first.write_text("first")
    second.write_text("second")
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
    environment = {**os.environ, "TERM": "xterm-256color", "SHELL": "/bin/sh",
                   "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    process = subprocess.Popen([binary, "--tui", "--editor-only", str(first)],
                               stdin=slave, stdout=slave, stderr=slave, env=environment, start_new_session=True)
    os.close(slave)
    output = bytearray()

    def pump(seconds=0.25):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.02)[0]:
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

    def click(column):
        # SGR mouse coordinates are one-based. The tab header is row 2.
        send(f"\x1b[<0;{column};2M\x1b[<0;{column};2m".encode())

    def wait_file(path, text):
        deadline = time.monotonic() + 5
        while not path.exists() or path.read_text() != text:
            assert process.poll() is None, output.decode(errors="replace")[-3000:]
            assert time.monotonic() < deadline, output.decode(errors="replace")[-3000:]
            pump(0.05)

    def shell_pid(path):
        deadline = time.monotonic() + 5
        while not path.exists() or not path.read_text().isdigit():
            assert time.monotonic() < deadline, output.decode(errors="replace")[-3000:]
            pump(0.05)
        return int(path.read_text())

    def assert_stopped(pid):
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return
        raise AssertionError(f"Closed terminal shell {pid} is still running")

    try:
        pump(0.7)
        assert b"[x]" in output, "TUI editor tab has no visible close button"
        command("open " + str(second))
        click(11)  # Inactive 猫.txt: title occupies six terminal columns.
        send(b"EDIT")
        send(b"\x13")
        wait_file(second, "EDITsecond")
        assert first.read_text() == "first", "Closing an inactive tab changed it or stole focus"
        click(15)  # The remaining clean second.txt tab.
        send(b"KEEP")
        click(15)  # Untitled *: closing must ask before discarding.
        # Ratatui can position the second word with a cursor escape instead
        # of emitting a literal space; assert the rendered words across CSI.
        assert re.search(rb"Unsaved(?:\x1b\[[0-?]*[ -/]*[@-~]|\s)*changes", output), \
            "Dirty tab close did not ask for confirmation"
        send(b"\x1b")
        scratch = root / "scratch.txt"
        command("save-as " + str(scratch))
        wait_file(scratch, "KEEP")
        send(b"\x17")  # Ctrl+W closes the saved tab.
        send(b"DISCARD")
        click(15)
        send(b"d")
        send(b"AFTER")
        after = root / "after.txt"
        command("save-as " + str(after))
        wait_file(after, "AFTER")
        send(b"\x17")
        long_file = root / ("long-" + "猫" * 55 + ".txt")
        long_file.write_text("long original")
        command("open " + str(long_file))
        click(98)  # Clipped active title still leaves [x] inside the border.
        send(b"NARROW")
        narrow = root / "narrow.txt"
        command("save-as " + str(narrow))
        wait_file(narrow, "NARROW")
        assert long_file.read_text() == "long original"
        # Compact to the focused pane so every tab's coordinates are fixed.
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 60, 0, 0))
        os.kill(process.pid, signal.SIGWINCH)
        pump()
        command("terminal")
        send(b"printf '%s' \"$$\" > first-terminal.pid\r")
        first_pid = shell_pid(root / "first-terminal.pid")
        command("terminal")
        send(b"printf '%s' \"$$\" > second-terminal.pid\r")
        second_pid = shell_pid(root / "second-terminal.pid")
        click(31)  # Inactive terminal after the 15-column narrow.txt tab.
        assert_stopped(first_pid)
        os.kill(second_pid, 0)
        send(b"printf ACTIVE > terminal-focus.txt\r")
        wait_file(root / "terminal-focus.txt", "ACTIVE")
        click(31)  # Active terminal; the editor becomes active again.
        assert_stopped(second_pid)
        send(b"TERMINALCLOSED")
        send(b"\x13")
        wait_file(narrow, "NARROWTERMINALCLOSED")
        send(b"\x11")
        process.wait(timeout=5)
        assert process.returncode == 0
        print("PASS TUI tab close: file/terminal [x], shell cleanup, inactive-tab focus, Unicode coordinates, cancellation, discard, Ctrl+W and clipped long filenames")
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=5)
        os.close(master)
