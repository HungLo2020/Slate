#!/usr/bin/env python3
"""Drive the actual TUI under a PTY; check file saves and shell execution."""
import fcntl, os, pathlib, pty, select, signal, struct, subprocess, sys, tempfile, termios, time
binary = str(pathlib.Path(sys.argv[1] if len(sys.argv)>1 else 'target/debug/slate').resolve())
with tempfile.TemporaryDirectory(prefix='slate-tui-') as tmp:
    root=pathlib.Path(tmp); file=root/'edit.txt'; file.write_text('original\n')
    master,slave=pty.openpty()
    fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',40,160,0,0))
    env={**os.environ,'TERM':'xterm-256color','SHELL':'/bin/sh','XDG_CONFIG_HOME':str(root/'config')}
    process=subprocess.Popen([binary,'--tui',str(file)],stdin=slave,stdout=slave,stderr=slave,env=env,start_new_session=True)
    os.close(slave);output=bytearray()
    def pump(seconds=.25):
        deadline=time.monotonic()+seconds
        while time.monotonic()<deadline:
            if select.select([master],[],[],.02)[0]:
                try: data=os.read(master,65536)
                except OSError: break
                output.extend(data)
                if b'\x1b[6n' in data: os.write(master,b'\x1b[1;1R')
    def send(data): os.write(master,data);pump()
    def command(text): send(b'\x1bOP');send(text.encode());send(b'\r')
    try:
        pump(1)
        assert process.poll() is None,output.decode(errors='replace')
        assert b'files #1' in output and b'editor #2' in output and b'terminal #3' in output,'Default panes missing'
        send(b'\x01');send(b'TUI edited\rsecond line');send(b'\x13')
        assert file.read_text()=='TUI edited\nsecond line',file.read_text()
        command('split-down');command('terminal')
        send(b"printf 'PTY_OK' > terminal.txt\r");pump(.4)
        assert (root/'terminal.txt').read_text()=='PTY_OK'
        command('layout-save smoke');command('preset minimal');command('layout-load smoke')
        assert (root/'config/slate/layouts.toml').exists()
        command('quit');process.wait(timeout=5);assert process.returncode==0
        print('PASS TUI: three panes, edit/save, split, terminal command, layout persistence, clean exit')
    finally:
        if process.poll() is None: process.terminate();process.wait(timeout=5)
        os.close(master)
