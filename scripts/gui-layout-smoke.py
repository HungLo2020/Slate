#!/usr/bin/env python3
"""Check real GUI geometry at small/large sizes, large fonts, and fractional DPI.

Build with --features gui-smoke. Runs 12 window/font combinations per style/scale.
The C++ driver checks controls for collisions and text/PTY viewport clipping.
"""
import os
import pathlib
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parent
binary = sys.argv[1] if len(sys.argv) > 1 else 'target/debug/slate'
for style in ('Basic', 'Fusion'):
    for scale in ('1', '1.5', '2'):
        print(f'Layout checks: style={style}, scale={scale}', flush=True)
        environment = {**os.environ, 'SLATE_GUI_LAYOUT_SMOKE': '1',
                       'QT_QUICK_CONTROLS_STYLE': style, 'QT_SCALE_FACTOR': scale}
        subprocess.run([sys.executable, str(root/'gui-offscreen-smoke.py'), binary],
                       env=environment, check=True)
print('PASS 72 GUI window/font/style/scale combinations')
