#!/usr/bin/env python3
"""Exercise --wait launchers against the real GUI and its test-only tab driver."""
import json
import os
import pathlib
import signal
import shlex
import subprocess
import sys
import tempfile
import time

binary = str(pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/slate-gui').resolve())

for fresh in (True, False):
    with tempfile.TemporaryDirectory(prefix='slate-editor-wait-') as temporary:
        root = pathlib.Path(temporary)
        (root / 'runtime').mkdir(mode=0o700)
        files = [root / name for name in ('one.txt', 'two.txt', 'unrelated.txt')]
        for file in files:
            file.write_text(file.stem + '\n')
        environment = {**os.environ, 'QT_QPA_PLATFORM': 'offscreen', 'QT_QUICK_BACKEND': 'software',
                       'XDG_CONFIG_HOME': str(root / 'config'), 'XDG_STATE_HOME': str(root / 'state'),
                       'XDG_RUNTIME_DIR': str(root / 'runtime'), 'SHELL': '/bin/sh',
                       'SLATE_GUI_SMOKE_DIR': str(root), 'SLATE_GUI_WAIT_SMOKE': '1'}
        sequence = 0
        primary = None
        children = []
        gui_pid = None

        def state():
            try:
                return json.loads((root / 'wait-state.json').read_text())
            except (FileNotFoundError, json.JSONDecodeError):
                return {}

        def until(check, detail, seconds=15):
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                report = state()
                if check(report):
                    return report
                time.sleep(0.03)
            raise AssertionError(f'{detail}: {state()}')

        def command(action, **arguments):
            global sequence
            sequence += 1
            pending = root / 'wait-command.json.tmp'
            pending.write_text(json.dumps({'sequence': sequence, 'command': {'action': action, **arguments}}))
            pending.replace(root / 'wait-command.json')
            return until(lambda s: s.get('sequence') == sequence, f'Command {action} not applied')

        def open_file(file):
            command('open', path=str(file))
            until(lambda s: s.get('document', {}).get('title', '').startswith(file.name), f'{file.name} not active')

        def waiting(process):
            time.sleep(0.15)
            assert process.poll() is None, f'Launcher returned before files closed: {process.communicate()}'

        def launch(*arguments):
            process = subprocess.Popen([binary, *arguments], env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            children.append(process)
            return process

        try:
            if not fresh:
                primary = launch(str(files[2]))
                gui_pid = until(lambda s: 'pid' in s, 'Existing GUI did not start')['pid']
            waiter = launch('--wait', '+1,2', str(files[0]), str(files[1]))
            gui_pid = until(lambda s: 'pid' in s, 'Waiting GUI did not start')['pid']
            open_file(files[1])
            waiting(waiter)
            open_file(files[2])
            # An unrelated tab closing is not completion.
            command('close_document', force=False)
            waiting(waiter)
            open_file(files[0])
            command('paste', text='edited ')
            command('save')
            until(lambda s: s.get('frame', {}).get('status', '').startswith('Saved'), 'Save did not finish')
            assert files[0].read_text() == 'oedited ne\n'
            waiting(waiter)
            command('close_document', force=False)
            waiting(waiter)
            open_file(files[1])
            # Two independent callers can wait on the same already-open file.
            concurrent = launch('--wait', str(files[1]))
            time.sleep(0.5)
            waiting(concurrent)
            command('close_document', force=False)
            for caller in (waiter, concurrent):
                stdout, stderr = caller.communicate(timeout=5)
                assert caller.returncode == 0, (stdout, stderr)
            os.kill(gui_pid, 0)  # The GUI stays available after its launchers return.
            assert state()['pid'] == gui_pid
            # Ordinary forwarding retains its immediate-return behavior.
            ordinary = launch(str(files[2]))
            ordinary.communicate(timeout=5)
            assert ordinary.returncode == 0
            open_file(files[2])
            # Git must not read its commit message until this GUI tab closes.
            def git(*arguments):
                return subprocess.check_output(['git', '-C', str(root), *arguments], env=environment, stderr=subprocess.STDOUT, text=True)
            git('init', '-q')
            git('config', 'user.name', 'Slate wait fixture')
            git('config', 'user.email', 'fixture@example.invalid')
            git('add', 'one.txt')
            git('commit', '-qm', 'Initial fixture')
            files[0].write_text(files[0].read_text() + 'change\n')
            git('add', 'one.txt')
            commit = subprocess.Popen(['git', '-C', str(root), '-c', f'core.editor={shlex.quote(binary)} --wait', 'commit'], env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            children.append(commit)
            until(lambda s: s.get('document', {}).get('title', '').startswith('COMMIT_EDITMSG'), 'Git message did not open')
            waiting(commit)
            command('paste', text='Wait integration commit\n')
            command('save')
            until(lambda s: s.get('frame', {}).get('status', '').startswith('Saved'), 'Git message save did not finish')
            waiting(commit)
            command('close_document', force=False)
            stdout, stderr = commit.communicate(timeout=5)
            assert commit.returncode == 0, (stdout, stderr)
            assert git('log', '-1', '--pretty=%s').strip() == 'Wait integration commit'
            open_file(files[2])
            abandoned = launch('--wait', str(files[2]))
            waiting(abandoned)
            abandoned.terminate()
            abandoned.communicate(timeout=5)
            survivor = launch('--wait', str(files[2]))
            waiting(survivor)
            command('close_document', force=False)
            survivor.communicate(timeout=5)
            assert survivor.returncode == 0
            # A failed open must return a failure rather than hang or report success.
            binary_file = root / 'binary.dat'
            binary_file.write_bytes(b'bad\x00data')
            failed = launch('--wait', str(binary_file))
            failed.communicate(timeout=5)
            assert failed.returncode != 0
            failed_startup = launch('--new-instance', '--wait', str(binary_file))
            failed_startup.communicate(timeout=5)
            assert failed_startup.returncode != 0
            # Abrupt GUI death is a failure for every remaining caller.
            crashed = launch('--wait', str(files[2]))
            waiting(crashed)
            os.kill(gui_pid, signal.SIGKILL)
            gui_pid = None
            crashed.communicate(timeout=5)
            assert crashed.returncode != 0
            # A new-instance launcher also completes on an orderly whole-window quit.
            (root / 'wait-state.json').unlink(missing_ok=True)
            (root / 'wait-command.json').unlink(missing_ok=True)
            sequence = 0
            orderly = launch('--new-instance', '--wait', str(files[0]))
            gui_pid = until(lambda s: 'pid' in s, 'New-instance GUI did not start')['pid']
            waiting(orderly)
            command('quit', force=False)
            orderly.communicate(timeout=5)
            assert orderly.returncode == 0
            gui_pid = None
        finally:
            if gui_pid is not None:
                try:
                    os.kill(gui_pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
            for child in children:
                if child.poll() is None:
                    child.terminate()
                child.communicate(timeout=10)
print('PASS editor wait: new/existing GUI, multiple files, save and unrelated tabs, concurrent/abandoned callers, ordinary forwarding, actual Git commits, open/startup failure, orderly quit and crash')
