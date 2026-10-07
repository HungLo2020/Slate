#!/usr/bin/env python3
"""Install built executables in an isolated prefix and verify GUI/TUI entry points."""
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile

root = pathlib.Path(__file__).resolve().parent.parent
binary = pathlib.Path(sys.argv[1] if len(sys.argv)>1 else root/'target/release/slate').resolve()
tui_only = '--tui-only' in sys.argv[2:]
with tempfile.TemporaryDirectory(prefix='slate-install-') as temporary:
    prefix = pathlib.Path(temporary)/'prefix'
    command = ['sh', str(root/'scripts/install.sh'), str(prefix), '--binary', str(binary)]
    if tui_only:
        command.append('--tui-only')
    subprocess.run(command, check=True)
    installed = prefix/'bin/slate'
    assert os.access(installed, os.X_OK)
    subprocess.run([str(installed), '--version'], check=True)
    # The terminal executable must run on systems without Qt.
    if shutil.which('ldd'):
        libraries = subprocess.check_output(['ldd', str(installed)], text=True)
        assert 'libQt' not in libraries, f'slate links Qt:\n{libraries}'
    if tui_only:
        assert not (prefix/'bin/slate-gui').exists()
        # Without a sibling (or another installed) slate-gui, --gui explains what is missing.
        if not shutil.which('slate-gui'):
            result = subprocess.run([str(installed), '--gui', '--version'], capture_output=True, text=True)
            assert result.returncode != 0 and 'slate-gui' in result.stderr, result.stderr
    else:
        gui=prefix/'bin/slate-gui'
        assert not gui.is_symlink() and os.access(gui, os.X_OK), 'slate-gui must be its own executable'
        assert (prefix/'share/applications/slate.desktop').exists()
        assert (prefix/'share/icons/hicolor/scalable/apps/slate.svg').read_bytes() == (root/'resources/slate.svg').read_bytes()
        assert 'Icon=slate' in (prefix/'share/applications/slate.desktop').read_text()
        subprocess.run([str(gui), '--help'], check=True, stdout=subprocess.DEVNULL)
        # `slate --gui` hands over to the installed slate-gui.
        version = subprocess.check_output([str(installed), '--gui', '--version'], text=True)
        assert version.startswith('Slate '), version
    subprocess.run([sys.executable, str(root/'scripts/tui-smoke.py'), str(installed)], check=True)
print('PASS installation: Qt-free slate, slate-gui executable, desktop resources, installed TUI')
