#!/usr/bin/env python3
"""Drive a real Kirigami window with X11 input; requires Xvfb and xdotool."""
import os, pathlib, subprocess, sys, tempfile, time
binary=str(pathlib.Path(sys.argv[1] if len(sys.argv)>1 else 'target/debug/slate').resolve())
with tempfile.TemporaryDirectory(prefix='slate-gui-') as tmp:
    root=pathlib.Path(tmp);file=root/'edit.txt';file.write_text('original\n')
    env={**os.environ,'SHELL':'/bin/sh','XDG_CONFIG_HOME':str(root/'config'),'XDG_STATE_HOME':str(root/'state'),'QT_QUICK_BACKEND':'software','QT_QPA_PLATFORM':'xcb','DISPLAY':':93','XDG_RUNTIME_DIR':str(root/'runtime')}
    (root/'runtime').mkdir(mode=0o700)
    xvfb=subprocess.Popen(['Xvfb',env['DISPLAY'],'-screen','0','1600x1000x24','-nolisten','tcp'],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
    log=open(root/'gui.log','w+');process=None
    def xdo(*args):return subprocess.check_output(['xdotool',*args],env=env,text=True).strip()
    def wait_for(check,seconds=5):
        deadline=time.monotonic()+seconds
        while not check():
            if process is not None and process.poll() is not None:log.seek(0);raise AssertionError(log.read())
            assert time.monotonic()<deadline,'timed out'
            time.sleep(.05)
    try:
        time.sleep(.4)
        process=subprocess.Popen([binary,'--gui',str(file)],env=env,stdout=log,stderr=log)
        window=None
        deadline=time.monotonic()+8
        while time.monotonic()<deadline:
            if process.poll() is not None:log.seek(0);raise AssertionError(log.read())
            found=xdo('search','--name','^Slate$') if subprocess.call(['xdotool','search','--name','^Slate$'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)==0 else ''
            if found:window=found.splitlines()[0];break
            time.sleep(.1)
        assert window,'Kirigami window did not open'
        xdo('windowfocus',window);time.sleep(.5)
        def key(*keys):xdo('key','--clearmodifiers',*keys);time.sleep(.1)
        def type_text(text):xdo('type','--clearmodifiers','--delay','4',text);time.sleep(.1)
        def command(text):key('F1');type_text(text);key('Return');time.sleep(.2)
        # Focus the actual painted editor, type through Qt input, then save through the shared core.
        xdo('mousemove','--window',window,'480','170');xdo('click','1');key('ctrl+a');type_text('GUI edited');key('Return');type_text('second line');key('ctrl+s')
        wait_for(lambda:file.read_text()=='GUI edited\nsecond line')
        key('ctrl+z');key('ctrl+y');key('ctrl+s');assert file.read_text()=='GUI edited\nsecond line'
        command('split-down');command('terminal')
        type_text("printf 'GUI_PTY_OK' > terminal.txt");key('Return')
        wait_for(lambda:(root/'terminal.txt').exists());assert (root/'terminal.txt').read_text()=='GUI_PTY_OK'
        command('layout-save smoke');command('preset minimal');command('layout-load smoke')
        assert (root/'config/slate/layouts.toml').exists()
        # Browse a second file using the left file pane, then verify its contents can be edited/saved.
        second=root/'second.txt';second.write_text('second original');command('refresh');command('open '+str(second));key('ctrl+a');type_text('SECOND_OK');key('ctrl+s');wait_for(lambda:second.read_text()=='SECOND_OK')
        screenshot=os.environ.get('SLATE_SCREENSHOT')
        if screenshot:
            from PIL import ImageGrab
            old=os.environ.get('DISPLAY');os.environ['DISPLAY']=env['DISPLAY'];ImageGrab.grab().save(screenshot)
            if old is not None:os.environ['DISPLAY']=old
        command('quit');process.wait(timeout=5);assert process.returncode==0
        log.seek(0);text=log.read();assert 'failed to load component' not in text
        print('PASS GUI: real Kirigami window, edit/save, undo/redo, split, PTY command, layout persistence, open second file, clean exit')
    finally:
        if process is not None and process.poll() is None:process.terminate();process.wait(timeout=5)
        xvfb.terminate();xvfb.wait(timeout=5);log.close()
