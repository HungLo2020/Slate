#!/usr/bin/env python3
"""Kill actual TUI processes and check each recovery path:

* a directory workspace restores its unsaved buffers and layout after SIGKILL;
* a single file keeps no checkpoint by default (no copies of the file's text
  in the state directory) and writes nano-style NAME.save files on SIGTERM;
* with file_recovery enabled, a single file restores only its own buffer.
"""
import fcntl
import json
import os
import pathlib
import pty
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time

binary = str(pathlib.Path(sys.argv[1] if len(sys.argv)>1 else 'target/debug/slate').resolve())
with tempfile.TemporaryDirectory(prefix='slate-recovery-') as temporary:
    root = pathlib.Path(temporary)
    workspace = root/'workspace'
    workspace.mkdir()
    file = workspace/'recover.rs'
    file.write_text('original\n')
    environment = {**os.environ, 'TERM':'xterm-256color', 'SHELL':'/bin/sh',
                   'XDG_CONFIG_HOME':str(root/'config'), 'XDG_STATE_HOME':str(root/'state')}
    def launch(*args):
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH',40,160,0,0))
        process = subprocess.Popen([binary, '--tui', *map(str, args)], stdin=slave, stdout=slave,
                                   stderr=slave, env=environment, start_new_session=True)
        os.close(slave)
        return process, master
    def pump(master, seconds=.25):
        deadline = time.monotonic()+seconds
        while time.monotonic()<deadline:
            if select.select([master],[],[],.02)[0]:
                try:
                    data = os.read(master,65536)
                except OSError:
                    break
                if b'\x1b[6n' in data:
                    os.write(master,b'\x1b[1;1R')
    def send(master, data):
        os.write(master,data)
        pump(master)
    def close(process, master):
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)
        os.close(master)
    def sessions():
        return list((root/'state').glob('slate/workspaces/*/session.json'))
    def wait_checkpoint(predicate):
        deadline = time.monotonic()+8
        while True:
            for session in sessions():
                if predicate(json.loads(session.read_text())):
                    return
            assert time.monotonic()<deadline, 'Unsaved checkpoint did not reach disk'
            time.sleep(.1)

    # 1. Directory workspace: crash and restore.
    process, master = launch(workspace)
    try:
        pump(master,1)
        send(master,b'\x1bOP')
        send(master,b':open recover.rs\r')
        pump(master,.5)
        send(master,b'\x01')
        send(master,b'RECOVERED_UNSAVED\rsecond line')
        send(master,b'\x1bOP')
        send(master,b'split-down\r')
        def recorded(checkpoint):
            documents = checkpoint['documents'].values()
            return (any(d.get('text')=='RECOVERED_UNSAVED\nsecond line' for d in documents)
                    and len(checkpoint['views'])>=2)
        wait_checkpoint(recorded)
        assert file.read_text()=='original\n'
        process.kill()
        process.wait(timeout=5)
    finally:
        close(process, master)
    process, master = launch(workspace)
    try:
        pump(master,1)
        send(master,b'\x13')
        assert file.read_text()=='RECOVERED_UNSAVED\nsecond line', file.read_text()
        send(master,b'\x1bOP')
        send(master,b'quit\r')
        process.wait(timeout=5)
        assert process.returncode==0
    finally:
        close(process, master)
    # Clean files are recorded as references only, never with their contents.
    for session in sessions():
        for document in json.loads(session.read_text())['documents'].values():
            assert 'RECOVERED_UNSAVED' not in document.get('text', ''), 'Clean file text was checkpointed'
    workspace_sessions = set(sessions())

    # 2. A single file: no checkpoint; SIGTERM keeps NAME.save.
    single = root/'single.txt'
    single.write_text('one\n')
    process, master = launch(single)
    try:
        pump(master,1)
        send(master,b'UNSAVED ')
        pump(master,1.5)
        assert set(sessions())==workspace_sessions, 'Single-file editing wrote a recovery checkpoint'
        process.terminate()
        process.wait(timeout=5)
    finally:
        close(process, master)
    rescue = root/'single.txt.save'
    assert rescue.read_text()=='UNSAVED one\n', 'SIGTERM did not write NAME.save'
    assert single.read_text()=='one\n'

    # 3. Opt-in single-file recovery restores only that file's buffer.
    (root/'config/slate').mkdir(parents=True, exist_ok=True)
    (root/'config/slate/settings.toml').write_text('file_recovery = true\n')
    process, master = launch(single)
    try:
        pump(master,1)
        send(master,b'OPTIN ')
        wait_checkpoint(lambda c: any(d.get('text')=='OPTIN one\n' for d in c['documents'].values()))
        process.kill()
        process.wait(timeout=5)
    finally:
        close(process, master)
    process, master = launch(single)
    try:
        pump(master,1)
        send(master,b'\x13')
        assert single.read_text()=='OPTIN one\n', single.read_text()
        send(master,b'\x11')
        process.wait(timeout=5)
        assert process.returncode==0
    finally:
        close(process, master)
print('PASS recovery: workspace crash restore, no single-file checkpoint by default, NAME.save on SIGTERM, opt-in single-file recovery')
