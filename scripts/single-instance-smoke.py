#!/usr/bin/env python3
"""A second `slate-gui FILE` hands its file to the running window and exits.

The first window runs offscreen with single-file recovery enabled, so its
checkpoint shows which documents it holds.
"""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time

binary = str(pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/slate-gui').resolve())
with tempfile.TemporaryDirectory(prefix='slate-instance-') as temporary:
    root = pathlib.Path(temporary)
    (root / 'runtime').mkdir(mode=0o700)
    (root / 'config/slate').mkdir(parents=True)
    (root / 'config/slate/settings.toml').write_text('file_recovery = true\n')
    first, second = root / 'first.txt', root / 'second.txt'
    first.write_text('first\n')
    second.write_text('second\n')
    environment = {**os.environ, 'QT_QPA_PLATFORM': 'offscreen', 'QT_QUICK_BACKEND': 'software',
                   'XDG_CONFIG_HOME': str(root / 'config'), 'XDG_STATE_HOME': str(root / 'state'),
                   'XDG_RUNTIME_DIR': str(root / 'runtime'), 'SHELL': '/bin/sh'}
    environment.pop('SLATE_GUI_SMOKE_DIR', None)
    socket = root / 'runtime' / f'slate-gui-{os.geteuid()}.sock'
    log = (root / 'primary.log').open('w')
    primary = subprocess.Popen([binary, str(first)], env=environment, stdout=log, stderr=log)
    try:
        deadline = time.monotonic() + 15
        while not socket.exists():
            assert primary.poll() is None, (root / 'primary.log').read_text()
            assert time.monotonic() < deadline, 'The first window did not listen for files'
            time.sleep(0.05)
        started = time.monotonic()
        handed = subprocess.run([binary, '+2', str(second)], env=environment, capture_output=True, text=True, timeout=10)
        assert handed.returncode == 0, handed.stderr
        assert time.monotonic() - started < 5, 'Handing over took too long'
        # The running window opened the file: its checkpoint now lists it.
        deadline = time.monotonic() + 10
        while True:
            documents = []
            for session in (root / 'state').glob('slate/workspaces/*/session.json'):
                documents += [d.get('path') for d in json.loads(session.read_text())['documents'].values()]
            if str(second) in documents:
                break
            assert time.monotonic() < deadline, f'Running window did not open the file: {documents}'
            time.sleep(0.1)
        # Editing/layout/recovery options keep their semantics in a new window.
        for flag in ('--view', '--fresh', '--editor-only', '--workspace'):
            special_file = root / (flag[2:] + '.txt')
            special_file.write_text('special launch\n')
            special = subprocess.Popen([binary, flag, str(special_file)], env=environment,
                                       stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 8
                while True:
                    assert special.poll() is None, f'{flag} was forwarded instead of opening a window'
                    documents = []
                    for session in (root / 'state').glob('slate/workspaces/*/session.json'):
                        documents += list(json.loads(session.read_text())['documents'].values())
                    document = next((d for d in documents if d.get('path') == str(special_file)), None)
                    if document is not None:
                        assert document.get('read_only', False) == (flag == '--view'), document
                        break
                    assert time.monotonic() < deadline, f'{flag} did not checkpoint its document'
                    time.sleep(0.1)
            finally:
                if special.poll() is None:
                    special.terminate()
                special.wait(timeout=10)
        # --new-instance starts its own window instead.
        separate = subprocess.Popen([binary, '--new-instance', str(second)], env=environment,
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(2)
        assert separate.poll() is None, '--new-instance exited instead of opening a window'
        separate.terminate()
        separate.wait(timeout=10)
    finally:
        if primary.poll() is None:
            primary.terminate()
            primary.wait(timeout=10)
        log.close()
    assert not socket.exists(), 'The socket outlived the window'
print('PASS single instance: files go to the running window, launch options open separate windows, --new-instance opens another, socket cleaned up')
