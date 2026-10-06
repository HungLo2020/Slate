#!/usr/bin/env python3
"""Run real Qt input/scene tests without an X server; build --features gui-smoke."""
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile

binary = str(pathlib.Path(sys.argv[1] if len(sys.argv)>1 else 'target/debug/slate').resolve())
with tempfile.TemporaryDirectory(prefix='slate-gui-offscreen-') as temporary:
    root = pathlib.Path(temporary)
    workspace = root/'workspace'
    workspace.mkdir()
    (workspace/'edit.txt').write_text('original\n')
    (workspace/'second.txt').write_text('second original')
    (root/'runtime').mkdir(mode=0o700)
    environment = {**os.environ, 'SLATE_GUI_SMOKE_DIR':temporary,
                   'QT_QPA_PLATFORM':'offscreen', 'QT_QUICK_BACKEND':'software', 'SHELL':'/bin/sh',
                   'XDG_CONFIG_HOME':str(root/'config'), 'XDG_STATE_HOME':str(root/'state'),
                   'XDG_RUNTIME_DIR':str(root/'runtime')}
    # Desktop helpers can outlive Qt and inherit its output handles. Files avoid
    # waiting for those unrelated descendants to close subprocess capture pipes.
    timed_out = False
    with (root/'stdout.log').open('w') as stdout, (root/'stderr.log').open('w') as stderr:
        try:
            result = subprocess.run([binary,'--gui',str(workspace)], env=environment,
                                    stdout=stdout, stderr=stderr, text=True, timeout=45)
        except subprocess.TimeoutExpired:
            timed_out = True
    report = json.loads((root/'report.json').read_text()) if (root/'report.json').exists() else {}
    screenshot = os.environ.get('SLATE_SCREENSHOT')
    if screenshot and (root/'gui.png').exists():
        shutil.copyfile(root/'gui.png',screenshot)
    logs = (root/'stdout.log').read_text()+(root/'stderr.log').read_text()
    assert not timed_out, f'GUI process timeout: {report}\n{logs}'
    assert result.returncode==0 and report.get('pass'), f'{report}\n{logs}'
    print('PASS GUI:',report['detail'])
