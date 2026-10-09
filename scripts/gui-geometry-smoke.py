#!/usr/bin/env python3
"""Verify real native geometry persistence across separate GUI sessions."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
binary = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix='slate-geometry-') as directory:
    root = Path(directory)
    workspace = root/'workspace'
    workspace.mkdir()
    environment = {**os.environ, 'QT_QPA_PLATFORM': 'offscreen', 'QT_QUICK_BACKEND': 'software',
                   'XDG_CONFIG_HOME': str(root/'config'), 'XDG_STATE_HOME': str(root/'state'),
                   'SLATE_GUI_SMOKE_DIR': str(root), 'SLATE_GUI_GEOMETRY_SMOKE': '1'}
    for phase in ('save', 'restore'):
        result = subprocess.run([binary,'--gui',str(workspace)], env={**environment,'SLATE_GUI_GEOMETRY_PHASE':phase}, capture_output=True, text=True, timeout=20)
        report = json.loads((root/'report.json').read_text())
        assert result.returncode == 0 and report['pass'], (report,result.stderr)
    assert (root/'config/slate/window.ini').is_file()
print('PASS native geometry persisted and restored across GUI sessions')
