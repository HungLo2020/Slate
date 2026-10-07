#!/usr/bin/env python3
"""Run Qt input/scene tests; default offscreen, SLATE_GUI_PLATFORM=wayland for a desktop."""
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
    git_smoke = bool(os.environ.get('SLATE_GUI_GIT_SMOKE'))
    command_smoke = bool(os.environ.get('SLATE_GUI_COMMAND_SMOKE'))
    if os.environ.get('SLATE_GUI_FEATURE_SMOKE') or git_smoke or command_smoke:
        def git(*args): subprocess.run(['git', '-C', str(workspace), *args], check=True, capture_output=True)
        git('init', '-q'); git('config', 'user.name', 'Slate test'); git('config', 'user.email', 'test@example.invalid')
        if git_smoke:
            (workspace/'src').mkdir()
            for index in range(24):
                (workspace/'src'/f'tracked_{index:02}.rs').write_text('base\n')
        git('add', '.'); git('commit', '-qm', 'Initial fixture')
        (workspace/'second.txt').write_text('staged change\n'); git('add', 'second.txt')
        (workspace/'second.txt').write_text('working change\n')
        (workspace/'new file.txt').write_text('untracked preview\n')
        if git_smoke:
            for index in range(22):
                (workspace/'src'/f'tracked_{index:02}.rs').write_text('working change\n')
                if index < 6:
                    git('add', f'src/tracked_{index:02}.rs')
            git('mv', 'src/tracked_22.rs', 'src/a very long renamed file with spaces and unicode 猫.rs')
            (workspace/'src/tracked_23.rs').unlink()
            (workspace/'new').mkdir()
            for index in range(12):
                (workspace/'new'/f'untracked_{index:02}.txt').write_text('new content\n')
            hook = workspace/'.git/hooks/pre-commit'
            hook.write_text('#!/bin/sh\necho "Fixture hook rejected commit" >&2\nexit 1\n')
            hook.chmod(0o700)
    if command_smoke:
        (root/'config/slate').mkdir(parents=True)
        (root/'config/slate/settings.toml').write_text(
            '[global_keys]\n"f5" = "quit"\n"f4" = "discard-document"\n"f2" = "settings"\n"f3" = "layout-save"\n')
    (root/'runtime').mkdir(mode=0o700)
    platform = os.environ.get('SLATE_GUI_PLATFORM', 'offscreen')
    environment = {**os.environ, 'SLATE_GUI_SMOKE_DIR':temporary,
                   'QT_QPA_PLATFORM':platform, 'QT_QUICK_BACKEND':'software', 'SHELL':'/bin/sh',
                   'XDG_CONFIG_HOME':str(root/'config'), 'XDG_STATE_HOME':str(root/'state'),
                   'XDG_RUNTIME_DIR':str(root/'runtime')}
    if platform.startswith('wayland'):
        display = pathlib.Path(os.environ.get('WAYLAND_DISPLAY', 'wayland-0'))
        if not display.is_absolute():
            display = pathlib.Path(os.environ.get('XDG_RUNTIME_DIR', f'/run/user/{os.getuid()}'))/display
        environment['WAYLAND_DISPLAY'] = str(display)
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
    artifacts = os.environ.get('SLATE_GUI_ARTIFACT_DIR')
    if artifacts:
        destination = pathlib.Path(artifacts)
        destination.mkdir(parents=True, exist_ok=True)
        for artifact in [*root.glob('*.png'), root/'report.json']:
            if artifact.exists():
                shutil.copyfile(artifact, destination/artifact.name)
    logs = (root/'stdout.log').read_text()+(root/'stderr.log').read_text()
    assert not timed_out, f'GUI process timeout: {report}\n{logs}'
    assert result.returncode==0 and report.get('pass'), f'{report}\n{logs}'
    assert not any(error in logs for error in ('ReferenceError:', 'TypeError:', 'Binding loop detected', 'Unable to assign')), logs
    if os.environ.get('SLATE_GUI_FEATURE_SMOKE'):
        subject = subprocess.check_output(['git', '-C', str(workspace), 'log', '-1', '--format=%s'], text=True).strip()
        assert subject == 'GUI smoke commit', subject
    if git_smoke:
        subject = subprocess.check_output(['git', '-C', str(workspace), 'log', '-1', '--format=%s'], text=True).strip()
        assert subject == 'GUI bulk commit', subject
        assert not subprocess.check_output(['git', '-C', str(workspace), 'status', '--porcelain'])
    print('PASS GUI:',report['detail'])
