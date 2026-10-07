#!/usr/bin/env python3
"""Build release Slate and launch its GUI, optionally opening a file/directory."""

import json
from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]


def build_release() -> Path:
    """Honor Cargo's configured target directory as well as CARGO_TARGET_DIR."""
    # Both executables: `slate` (terminal) execs the sibling `slate-gui` for --gui.
    subprocess.run(["cargo", "build", "--release", "--package", "slate", "--package", "slate-gui"],
                   cwd=ROOT, check=True)
    metadata = subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=ROOT, text=True
    )
    return Path(json.loads(metadata)["target_directory"]) / "release" / "slate"


def slate_arguments(arguments: list[str]) -> list[str]:
    """Resolve paths from the caller's directory; default to this repository."""
    result = [
        argument if argument.startswith("-") else str(Path(argument).expanduser().resolve())
        for argument in arguments
    ]
    if not any(not argument.startswith("-") for argument in arguments):
        result.append(str(ROOT))
    return result


def main() -> int:
    try:
        binary = build_release()
        arguments = slate_arguments(sys.argv[1:])
        return subprocess.run([str(binary), *arguments, "--gui"]).returncode
    except subprocess.CalledProcessError as error:
        return error.returncode
    except OSError as error:
        print(f"Cannot launch Slate GUI: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
