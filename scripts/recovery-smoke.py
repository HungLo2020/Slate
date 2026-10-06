#!/usr/bin/env python3
"""Kill an actual TUI process, restart, and save its recovered unsaved document."""
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
    def launch(path):
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH',40,160,0,0))
        process = subprocess.Popen([binary, '--tui', str(path)], stdin=slave, stdout=slave,
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
    process, master = launch(file)
    try:
        pump(master,1)
        send(master,b'\x01')
        send(master,b'RECOVERED_UNSAVED\rsecond line')
        send(master,b'\x1bOP')
        send(master,b'split-down\r')
        deadline = time.monotonic()+8
        while True:
            pump(master,.1)
            sessions = list((root/'state').glob('slate/workspaces/*/session.json'))
            if sessions:
                checkpoint = json.loads(sessions[0].read_text())
                if any(d['text']=='RECOVERED_UNSAVED\nsecond line' for d in checkpoint['documents'].values()) and len(checkpoint['views'])==2:
                    break
            assert time.monotonic()<deadline, 'Unsaved checkpoint did not reach disk'
        assert file.read_text()=='original\n'
        process.kill()
        process.wait(timeout=5)
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)
        os.close(master)
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
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)
        os.close(master)
print('PASS recovery: forced process crash, unsaved UTF-8 document, split layout, restarted editor save')
