#!/usr/bin/env python3
"""Install a built executable in an isolated prefix and verify GUI/TUI entry points."""
import os
import pathlib
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
    if not tui_only:
        alias=prefix/'bin/slate-gui'
        assert alias.is_symlink() and alias.resolve()==installed
        assert (prefix/'share/applications/slate.desktop').exists()
        assert (prefix/'share/icons/hicolor/scalable/apps/slate.svg').read_bytes() == (root/'resources/slate.svg').read_bytes()
        assert 'Icon=slate' in (prefix/'share/applications/slate.desktop').read_text()
        subprocess.run([str(alias), '--help'], check=True, stdout=subprocess.DEVNULL)
    subprocess.run([sys.executable, str(root/'scripts/tui-smoke.py'), str(installed)], check=True)
print('PASS installation: executable, aliases/desktop resources, actual installed TUI')
