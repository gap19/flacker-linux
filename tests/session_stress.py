#!/usr/bin/env python3
"""Three-minute offline transport/history recovery test. Use headless.py; no real sound."""
import json, os, re, subprocess, sys, tempfile, time
from pathlib import Path
binary=Path(sys.argv[1]).resolve()
def call(interface, method, *args, check=True):
    r=subprocess.run(['gdbus','call','--session','--dest','org.mpris.MediaPlayer2.flacker','--object-path','/org/mpris/MediaPlayer2','--method',interface+'.'+method,*args],capture_output=True,text=True,timeout=5)
    if check: assert r.returncode==0,r.stderr
    return r.stdout
player=lambda name,*args:call('org.mpris.MediaPlayer2.Player',name,*args)
prop=lambda name:call('org.freedesktop.DBus.Properties','Get','org.mpris.MediaPlayer2.Player',name)
def setprop(name,value):call('org.freedesktop.DBus.Properties','Set','org.mpris.MediaPlayer2.Player',name,value)
def action(name):
    subprocess.run(['gdbus','call','--session','--dest','io.github.gap19.FLACker','--object-path','/io/github/gap19/FLACker','--method','org.gtk.Actions.Activate',name,'[]','{}'],check=True,capture_output=True,timeout=5)
with tempfile.TemporaryDirectory(prefix='flacker-session-') as tmp:
    root=Path(tmp);music=root/'music';music.mkdir();data=root/'data';data.mkdir()
    for i in range(2):
        subprocess.run(['ffmpeg','-hide_banner','-loglevel','error','-f','lavfi','-i',f'sine=frequency={440+i*220}:sample_rate=96000:duration=6','-c:a','flac','-sample_fmt','s32','-metadata',f'TITLE=Session {i}','-metadata','ALBUM=Long session',str(music/f'{i}.flac')],check=True)
    playlist=music/'playlist.m3u8';playlist.write_text('0.flac\n1.flac\n')
    (data/'settings.json').write_text(json.dumps({'roots':[str(music)],'experience':{'history':True}}))
    env=dict(os.environ,FLACKER_DATA_DIR=str(data),FLACKER_SILENT='1');env.pop('FLACKER_UI_SMOKE',None)
    with (root/'app.log').open('w+') as log:
        app=subprocess.Popen([str(binary)],env=env,stdout=log,stderr=log)
        try:
            for _ in range(40):
                time.sleep(.25)
                if 'true' in call('org.freedesktop.DBus.Properties','Get','org.mpris.MediaPlayer2.Player','CanControl',check=False):break
            player('OpenUri',playlist.as_uri());setprop('LoopStatus',"<'Playlist'>")
            seen=set();rss=[]
            for i in range(90):
                time.sleep(2)
                assert app.poll() is None
                seen.update(re.findall('Session [01]',prop('Metadata')))
                assert 'Playing' in prop('PlaybackStatus')
                status=Path(f'/proc/{app.pid}/status').read_text();rss.append(int(re.search(r'VmRSS:\s+(\d+)',status).group(1)))
                if i in (15,45,75):
                    player('Pause');time.sleep(.3);pos=prop('Position');time.sleep(1)
                    assert prop('Position')==pos,'Position advanced during pause'
                    setprop('Shuffle','<true>');setprop('Shuffle','<false>');player('Play')
                if i in (30,60):
                    player('Stop');player('OpenUri',playlist.as_uri());setprop('LoopStatus',"<'Playlist'>")
            assert seen=={'Session 0','Session 1'},seen
            player('Pause');time.sleep(.3)
            entries=json.loads((data/'history.json').read_text());assert 2<=len(entries)<=500
            action('clear-history');time.sleep(.4);assert json.loads((data/'history.json').read_text())==[]
            # Missing removable media is simulated by renaming a synthetic file.
            track=music/'0.flac';player('OpenUri',track.as_uri());time.sleep(.3);player('Stop');time.sleep(.3);track.rename(music/'removed.flac');player('Play');time.sleep(.5)
            assert 'Stopped' in prop('PlaybackStatus')
            (music/'removed.flac').rename(track);player('OpenUri',track.as_uri());time.sleep(.4);assert 'Playing' in prop('PlaybackStatus')
            call('org.mpris.MediaPlayer2','Quit');app.wait(timeout=8);log.seek(0);logs=log.read();assert 'CRITICAL' not in logs and 'panicked' not in logs,logs
            print(json.dumps({'result':'passed','duration_seconds':180,'tracks_seen':sorted(seen),'history_entries_before_clear':len(entries),'memory_first_mib':round(rss[5]/1024,2),'memory_last_mib':round(rss[-1]/1024,2),'pause_resume':3,'stop_restart':2,'missing_file_recovery':True,'history_clear':True,'physical_suspend_or_DAC_test':False}))
        finally:
            if app.poll() is None:app.terminate();app.wait(timeout=5)
