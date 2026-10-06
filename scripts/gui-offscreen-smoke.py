#!/usr/bin/env python3
"""Run real Qt input/scene tests without an X server; build --features gui-smoke."""
import json, os, pathlib, subprocess, sys, tempfile
binary=str(pathlib.Path(sys.argv[1] if len(sys.argv)>1 else 'target/debug/slate').resolve())
with tempfile.TemporaryDirectory(prefix='slate-gui-offscreen-') as tmp:
    root=pathlib.Path(tmp);workspace=root/'workspace';workspace.mkdir()
    (workspace/'edit.txt').write_text('original\n');(workspace/'second.txt').write_text('second original')
    (root/'runtime').mkdir(mode=0o700)
    env={**os.environ,'SLATE_GUI_SMOKE_DIR':tmp,'QT_QPA_PLATFORM':'offscreen','QT_QUICK_BACKEND':'software','SHELL':'/bin/sh','XDG_CONFIG_HOME':str(root/'config'),'XDG_STATE_HOME':str(root/'state'),'XDG_RUNTIME_DIR':str(root/'runtime')}
    result=subprocess.run([binary,'--gui',str(workspace)],env=env,capture_output=True,text=True,timeout=25)
    report=json.loads((root/'report.json').read_text()) if (root/'report.json').exists() else {}
    screenshot=os.environ.get('SLATE_SCREENSHOT')
    if screenshot:
        import shutil
        shutil.copyfile(root/'gui.png',screenshot)
    assert result.returncode==0 and report.get('pass'),f'{report}\n{result.stdout}\n{result.stderr}'
    print('PASS GUI:',report['detail'])
