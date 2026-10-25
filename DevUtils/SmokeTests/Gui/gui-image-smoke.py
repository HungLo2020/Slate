#!/usr/bin/env python3
"""Exercise image startup, real Qt decoding/controls and file lifecycle."""
import json
import os
import pathlib
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib

binary = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/slate-gui').resolve()
with tempfile.TemporaryDirectory(prefix='slate-images-') as temporary:
    root = pathlib.Path(temporary)
    workspace = root / 'workspace'
    workspace.mkdir()
    (root / 'runtime').mkdir(mode=0o700)
    def chunk(kind, data):
        return struct.pack('!I', len(data)) + kind + data + struct.pack('!I', zlib.crc32(kind + data))
    pixels = b''.join(b'\0' + b'\xff\0\0\x80' * 32 for _ in range(24))
    png = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('!2I5B', 32, 24, 8, 6, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(pixels)) + chunk(b'IEND', b'')
    picture = workspace / 'space #% 猫.PNG'
    picture.write_bytes(png)
    (workspace / 'no-extension').write_bytes(png)
    (workspace / 'vector.svg').write_text('<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="900"><rect width="1200" height="900" fill="blue"/></svg>')
    (workspace / 'notes.txt').write_text('text still works\n')
    (workspace / 'broken.png').write_bytes(b'not an image')
    (workspace / 'huge.svg').write_text('<svg xmlns="http://www.w3.org/2000/svg" width="1000000" height="1000000"/>')
    environment = {**os.environ, 'QT_QPA_PLATFORM': 'offscreen', 'QT_QUICK_BACKEND': 'software',
                   'XDG_CONFIG_HOME': str(root / 'config'), 'XDG_STATE_HOME': str(root / 'state'),
                   'XDG_RUNTIME_DIR': str(root / 'runtime'), 'SHELL': '/bin/sh',
                   'SLATE_GUI_SMOKE_DIR': temporary, 'SLATE_GUI_IMAGE_SMOKE': '1'}
    command = [str(binary)] + (['--gui'] if binary.name == 'slate' else [])
    result = subprocess.run([*command, '--fresh', str(picture), str(workspace / 'vector.svg'), str(workspace / 'notes.txt')],
                            env=environment, capture_output=True, text=True, timeout=45)
    report = json.loads((root / 'report.json').read_text()) if (root / 'report.json').exists() else {}
    if os.environ.get('SLATE_GUI_ARTIFACT_DIR'):
        destination = pathlib.Path(os.environ['SLATE_GUI_ARTIFACT_DIR'])
        destination.mkdir(parents=True, exist_ok=True)
        for artifact in [*root.glob('*.png'), root / 'report.json']:
            if artifact.exists():
                shutil.copyfile(artifact, destination / artifact.name)
    assert result.returncode == 0 and report.get('pass'), f'{report}\n{result.stdout}\n{result.stderr}'
    errors = [line for line in result.stderr.splitlines() if 'ReferenceError:' in line or 'TypeError:' in line or 'Binding loop' in line or 'Cannot assign' in line]
    assert not errors, '\n'.join(errors)
    print('PASS GUI images:', report['detail'])
