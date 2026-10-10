#!/usr/bin/env python3
"""Terminate real GUI processes at each startup phase and once running.

A workspace with an unsaved buffer (created through the TUI, then killed) is
opened in the GUI, which is held at one phase until SIGTERM arrives:

* core: the workspace is restored, Qt does not exist yet;
* qapplication: QApplication exists, the window does not;
* loading: the window is loaded, the event loop has not started;
* event-loop: the GUI is idle with the unsaved buffer.

Each process must exit promptly and successfully without opening the window
after the signal, and the unsaved buffer must stay in workspace recovery.

Build with: cargo build -p slate -p slate-gui --features slate-gui/smoke
"""
import fcntl
import json
import os
import pathlib
import pty
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

binary = str(pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/slate').resolve())
UNSAVED = 'TERMINATION_UNSAVED\nsecond line'
with tempfile.TemporaryDirectory(prefix='slate-gui-termination-') as temporary:
    root = pathlib.Path(temporary)
    workspace = root/'workspace'
    workspace.mkdir()
    file = workspace/'keep.txt'
    file.write_text('original\n')
    (root/'runtime').mkdir(mode=0o700)
    environment = {**os.environ, 'TERM': 'xterm-256color', 'SHELL': '/bin/sh',
                   'XDG_CONFIG_HOME': str(root/'config'), 'XDG_STATE_HOME': str(root/'state'),
                   'XDG_RUNTIME_DIR': str(root/'runtime')}

    def sessions():
        return list((root/'state').glob('slate/workspaces/*/session.json'))

    def recovered():
        return any(document.get('text') == UNSAVED
                   for session in sessions()
                   for document in json.loads(session.read_text())['documents'].values())

    # An unsaved buffer reaches the recovery checkpoint, then the TUI dies.
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 160, 0, 0))
    process = subprocess.Popen([binary, '--tui', str(workspace)], stdin=slave, stdout=slave,
                               stderr=slave, env=environment, start_new_session=True)
    os.close(slave)
    def pump(seconds=.25):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if select.select([master], [], [], .02)[0]:
                try:
                    data = os.read(master, 65536)
                except OSError:
                    break
                if b'\x1b[6n' in data:
                    os.write(master, b'\x1b[1;1R')
    try:
        pump(1)
        for data in (b'\x1bOP', b':open keep.txt\r', b'\x01', UNSAVED.replace('\n', '\r').encode()):
            os.write(master, data)
            pump(.4)
        deadline = time.monotonic() + 8
        while not recovered():
            assert time.monotonic() < deadline, 'Unsaved checkpoint did not reach disk'
            pump(.1)
        process.kill()
        process.wait(timeout=5)
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)
        os.close(master)

    for phase in ('core', 'qapplication', 'loading', 'event-loop'):
        smoke = root/f'smoke-{phase}'
        smoke.mkdir()
        log = root/f'{phase}.log'
        gui_environment = {**environment, 'QT_QPA_PLATFORM': 'offscreen', 'QT_QUICK_BACKEND': 'software',
                           'SLATE_GUI_SMOKE_DIR': str(smoke), 'SLATE_GUI_SMOKE_HOLD': phase}
        ready = 'Smoke startup: entering event loop' if phase == 'event-loop' else f'Smoke startup: holding at {phase}'
        with log.open('w') as output:
            process = subprocess.Popen([binary, '--gui', str(workspace)], env=gui_environment,
                                       stdout=output, stderr=output)
        try:
            deadline = time.monotonic() + 20
            while ready not in log.read_text():
                assert process.poll() is None, log.read_text()
                assert time.monotonic() < deadline, f'{phase}: GUI never reached the phase\n{log.read_text()}'
                time.sleep(.02)
            if phase == 'event-loop':
                # A dirty window refuses an ordinary close (it asks first);
                # termination must not.
                assert 'Smoke startup: idle, unsaved=yes' in log.read_text(), log.read_text()
                time.sleep(.5)
            signalled = log.read_text()
            process.send_signal(signal.SIGTERM)
            sent = time.monotonic()
            process.wait(timeout=8)
            elapsed = time.monotonic() - sent
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
        text = log.read_text()
        assert process.returncode == 0, f'{phase}: exit code {process.returncode}\n{text}'
        assert elapsed < 5, f'{phase}: took {elapsed:.1f}s to exit after SIGTERM\n{text}'
        # Startup stops at the held phase; a loaded window quits from its
        # event loop as soon as that starts.
        later = {'core': 'constructing QApplication', 'qapplication': 'loading QML'}.get(phase)
        if later:
            assert later not in text, f'{phase}: startup continued after SIGTERM\n{text}'
        assert signalled in text and recovered(), f'{phase}: unsaved buffer left recovery\n{text}'
        assert file.read_text() == 'original\n', f'{phase}: termination wrote the file'
        print(f'PASS GUI SIGTERM at {phase}: exited in {elapsed:.2f}s, recovery kept', flush=True)
print('PASS termination before and during the GUI event loop is prompt and keeps unsaved work')
