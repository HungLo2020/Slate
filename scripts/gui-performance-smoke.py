#!/usr/bin/env python3
"""Release GUI input latency, payload size and retained-layout regression checks.

Build: cargo build --release -p slate -p slate-gui --features slate-gui/smoke
Default: isolated offscreen software renderer. SLATE_GUI_PLATFORM=wayland tests
on the current desktop. No clipboard contents are changed by this driver.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else "target/release/slate").resolve())
reports = []
for mode, size in (("typing", "1360x820"), ("cursor", "1360x820"),
                   ("selection", "1360x820"), ("scroll", "1360x820"),
                   ("syntax", "1360x820"),
                   ("typing", "1920x1080"), ("typing", "3840x2160"),
                   ("burst", "1360x820"), ("busy", "1360x820")):
    with tempfile.TemporaryDirectory(prefix="slate-gui-performance-") as temporary:
        root = Path(temporary)
        workspace = root / "workspace"
        workspace.mkdir()
        original = "A short text document.\nJust three lines.\nLast line.\n"
        if mode == "scroll":
            original = "".join(f"Line {n}: tabs\t猫 and emoji 👩‍💻\n" for n in range(180))
        if mode == "syntax":
            original = '// A short Rust document.\nfn main() {\n    println!("hello");\n}\n'
        file = workspace / ("edit.rs" if mode == "syntax" else "edit.txt")
        file.write_text(original)
        (root / "runtime").mkdir(mode=0o700)
        platform = os.environ.get("SLATE_GUI_PLATFORM", "offscreen")
        environment = {**os.environ, "SHELL": "/bin/sh", "QT_QPA_PLATFORM": platform,
                       "SLATE_GUI_SMOKE_DIR": temporary, "SLATE_GUI_PERF_SMOKE": "1",
                       "SLATE_GUI_PERF_MODE": mode, "SLATE_GUI_PERF_SIZE": size,
                       "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state"),
                       "XDG_RUNTIME_DIR": str(root / "runtime")}
        if platform.startswith("wayland"):
            display = Path(os.environ.get("WAYLAND_DISPLAY", "wayland-0"))
            if not display.is_absolute():
                display = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}")) / display
            environment["WAYLAND_DISPLAY"] = str(display)
        else:
            environment["QT_QUICK_BACKEND"] = "software"
        with (root / "log").open("w") as log:
            result = subprocess.run([binary, "--gui", "--fresh", "--workspace" if mode == "busy" else "--editor-only", str(file)],
                                    env=environment, stdout=log, stderr=log, timeout=25)
        report = json.loads((root / "report.json").read_text()) if (root / "report.json").exists() else {}
        logs = (root / "log").read_text()
        assert result.returncode == 0 and report.get("pass"), f"{mode} {size}: {report}\n{logs}"
        assert not any(error in logs for error in ("ReferenceError:", "TypeError:", "Cannot anchor to an item", "Unable to assign", "Binding loop")), logs
        assert file.read_text() == ("x" * report["keys"] + original if mode in ("typing", "syntax", "burst", "busy") else original)
        reports.append(report)
        actual_size = f"{report['width']}x{report['height']}"
        print(f"PASS {mode:9} {actual_size:9}: frame {report['median_frame_ms']:.2f} ms, "
              f"dispatch {report['median_dispatch_ms']:.3f} ms, "
              f"payload {report['max_update_bytes']} bytes, "
              f"layouts {report['layout_builds']}, updates {report['updates']}", flush=True)
artifact = os.environ.get("SLATE_GUI_PERF_REPORT")
if artifact:
    Path(artifact).write_text(json.dumps(reports, indent=2))
print("PASS all release GUI performance budgets")
