#!/usr/bin/env python3
"""Build release Slate and launch its TUI in a separate terminal window."""

import os
import shutil
import subprocess
import sys

from RunGui import ROOT, build_release, slate_arguments


def terminal_command() -> list[str]:
    """Always request a new window; never fall back to the caller's terminal."""
    candidates = (
        ("konsole", ["--separate", "-e"]),
        ("gnome-terminal", ["--window", "--"]),
        ("xfce4-terminal", ["--disable-server", "--execute"]),
        ("kitty", []),
        ("alacritty", ["-e"]),
        ("xterm", ["-e"]),
        ("foot", ["-e"]),
        ("x-terminal-emulator", ["-e"]),
    )
    for name, options in candidates:
        executable = shutil.which(name)
        if executable:
            return [executable, *options]
    raise RuntimeError("No supported terminal emulator found. Install Konsole, GNOME Terminal, or xterm.")


def main() -> int:
    try:
        if not (os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY")):
            raise RuntimeError("A graphical desktop session is required to open a new terminal window.")
        terminal = terminal_command()
        arguments = slate_arguments(sys.argv[1:])
        binary = build_release()
        subprocess.Popen(
            [*terminal, str(binary), *arguments, "--tui"],
            cwd=ROOT,
            stdin=subprocess.DEVNULL,
            start_new_session=True,
        )
        return 0
    except subprocess.CalledProcessError as error:
        return error.returncode
    except (OSError, RuntimeError) as error:
        print(f"Cannot launch Slate TUI: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
