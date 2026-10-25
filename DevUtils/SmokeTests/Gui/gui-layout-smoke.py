#!/usr/bin/env python3
"""Check real GUI geometry at small/large sizes, large fonts, and fractional DPI.

Build with: cargo build -p slate -p slate-gui --features slate-gui/smoke. Runs 12 window/font combinations per style/scale.
The C++ driver checks controls for collisions and text/PTY viewport clipping.
Also check KDE desktop button captions when that style is installed.
"""
import os
import pathlib
import shutil
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parent
binary = sys.argv[1] if len(sys.argv) > 1 else 'target/debug/slate'
styles = ['Basic', 'Fusion']
qml_paths = [pathlib.Path(path) for path in os.environ.get('QML_IMPORT_PATH', '').split(os.pathsep) if path]
qtpaths = shutil.which('qtpaths6') or shutil.which('qtpaths')
if qtpaths:
    result = subprocess.run([qtpaths, '--query', 'QT_INSTALL_QML'], capture_output=True, text=True)
    if result.returncode == 0 and result.stdout.strip():
        qml_paths.append(pathlib.Path(result.stdout.strip()))
if any((path/'org/kde/desktop/qmldir').is_file() for path in qml_paths):
    styles.append('org.kde.desktop')
    print('Caption regression check: KDE desktop style', flush=True)
    subprocess.run([sys.executable, str(root/'gui-offscreen-smoke.py'), binary],
                   env={**os.environ, 'SLATE_GUI_CAPTION_SMOKE': '1',
                        'QT_QUICK_CONTROLS_STYLE': 'org.kde.desktop'}, check=True)
else:
    assert not os.environ.get("SLATE_REQUIRE_KDE_STYLE"), "KDE desktop style is required in this test job"
    print('SKIP KDE desktop style: QML module not found')
for style in styles:
    for scale in ('1', '1.5', '2'):
        print(f'Layout checks: style={style}, scale={scale}', flush=True)
        environment = {**os.environ, 'SLATE_GUI_LAYOUT_SMOKE': '1',
                       'QT_QUICK_CONTROLS_STYLE': style, 'QT_SCALE_FACTOR': scale}
        subprocess.run([sys.executable, str(root/'gui-offscreen-smoke.py'), binary],
                       env=environment, check=True)
print(f'PASS {len(styles) * 36} GUI window/font/style/scale combinations')
