#!/usr/bin/env python3
"""Linux idle CPU regression check for both frontends; use a release build.

Each frontend gets an isolated workspace/config/state and an idle /bin/sh.
The TUI runs under a real PTY. GUI uses offscreen Qt software rendering.
Set SLATE_GUI_PLATFORM=wayland to measure the running desktop instead.
"""
import fcntl
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

binary = str(pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else 'target/release/slate').resolve())
limit = float(os.environ.get('SLATE_IDLE_CPU_LIMIT', '10'))
def cpu_ticks(pid):
    fields = pathlib.Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
    return int(fields[11]) + int(fields[12])
with tempfile.TemporaryDirectory(prefix='slate-idle-') as temporary:
    root = pathlib.Path(temporary)
    file = root/'idle.rs'
    file.write_text('fn main() {}\n')
    for frontend in ('gui', 'tui'):
        runtime = root/frontend/'runtime'; runtime.mkdir(parents=True, mode=0o700)
        environment = {**os.environ, 'SHELL':'/bin/sh', 'TERM':'xterm-256color',
                       'XDG_CONFIG_HOME':str(root/frontend/'config'), 'XDG_STATE_HOME':str(root/frontend/'state'),
                       'XDG_RUNTIME_DIR':str(runtime), 'QT_QPA_PLATFORM':'offscreen', 'QT_QUICK_BACKEND':'software'}
        if frontend == 'gui':
            environment['QT_QPA_PLATFORM'] = os.environ.get('SLATE_GUI_PLATFORM', 'offscreen')
            if environment['QT_QPA_PLATFORM'].startswith('wayland'):
                display = pathlib.Path(os.environ.get('WAYLAND_DISPLAY', 'wayland-0'))
                if not display.is_absolute():
                    display = pathlib.Path(os.environ.get('XDG_RUNTIME_DIR', f'/run/user/{os.getuid()}'))/display
                environment['WAYLAND_DISPLAY'] = str(display)
        master = slave = None
        if frontend == 'tui':
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 45, 160, 0, 0))
        with (root/f'{frontend}.log').open('w') as log:
            process = subprocess.Popen([binary, f'--{frontend}', str(file)], env=environment,
                                       stdin=slave if slave is not None else subprocess.DEVNULL,
                                       stdout=slave if slave is not None else log,
                                       stderr=slave if slave is not None else log, start_new_session=True)
            if slave is not None: os.close(slave)
            def wait(seconds):
                deadline = time.monotonic() + seconds
                while time.monotonic() < deadline:
                    assert process.poll() is None, (root/f'{frontend}.log').read_text()
                    if master is None: time.sleep(.05)
                    elif select.select([master], [], [], .05)[0]:
                        data = os.read(master, 65536)
                        if b'\x1b[6n' in data: os.write(master, b'\x1b[1;1R')
            try:
                wait(3)
                samples = []
                for _ in range(2):
                    started = time.monotonic(); before = cpu_ticks(process.pid); wait(2)
                    percent = (cpu_ticks(process.pid)-before)/os.sysconf('SC_CLK_TCK')/(time.monotonic()-started)*100
                    samples.append(percent)
                print(f'{frontend.upper()} idle CPU: ' + ', '.join(f'{n:.2f}% of one core' for n in samples), flush=True)
                assert max(samples) < limit, f'{frontend}: expected idle CPU below {limit}%, got {samples}'
            finally:
                if process.poll() is None: process.terminate(); process.wait(timeout=5)
                if master is not None: os.close(master)
print('PASS both frontends remain idle without repeated rendering')
