#!/usr/bin/env python3
"""Run inside dbus-run-session on an isolated Xvfb display. Never plays sound."""
import json
import os
import re
from pathlib import Path
import subprocess
import sys
import tempfile
import time

binary = Path(sys.argv[1]).resolve()

def dbus(method, *args, expect=True):
    result = subprocess.run([
        'gdbus', 'call', '--session', '--dest', 'org.mpris.MediaPlayer2.flacker',
        '--object-path', '/org/mpris/MediaPlayer2', '--method', method, *args
    ], capture_output=True, text=True, timeout=6)
    if expect and result.returncode:
        raise AssertionError(result.stderr)
    return result

def prop(name):
    return dbus('org.freedesktop.DBus.Properties.Get', 'org.mpris.MediaPlayer2.Player', name).stdout

def player(method, *args):
    return dbus('org.mpris.MediaPlayer2.Player.' + method, *args)

with tempfile.TemporaryDirectory(prefix='flacker-mpris-') as temp:
    temp = Path(temp)
    music = temp / 'music'
    music.mkdir()
    for i in (1, 2):
        subprocess.run(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-f', 'lavfi',
                        '-i', f'sine=frequency={i * 440}:sample_rate=44100:duration=12',
                        '-c:a', 'flac', '-metadata', f'TITLE=Signal {i}',
                        '-metadata', 'ALBUM=Calibration', '-metadata', 'ARTIST=Flacker tests',
                        '-metadata', f'TRACKNUMBER={i}', str(music / f'{i}.flac')], check=True)
    extra = temp / 'extra'
    extra.mkdir()
    long_track = extra / 'long.flac'
    subprocess.run(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-f', 'lavfi',
                    '-i', 'sine=frequency=330:sample_rate=8000:duration=150', '-ac', '1',
                    '-c:a', 'flac', '-metadata', 'TITLE=Long signal', '-metadata', 'ALBUM=Calibration',
                    '-metadata', 'ARTIST=Flacker tests', str(long_track)], check=True)
    sequence = music / 'reverse.m3u8'
    sequence.write_text('#EXTM3U\n2.flac\n1.flac\n')
    data = temp / 'data'
    data.mkdir()
    (data / 'settings.json').write_text(json.dumps({'roots': [str(music)]}))
    env = dict(os.environ, FLACKER_SILENT='1', FLACKER_DATA_DIR=str(data), GTK_A11Y='none')
    env.pop('FLACKER_UI_SMOKE', None)
    with (temp / 'app.log').open('w+') as log:
        app = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=log)
        try:
            for _ in range(60):
                time.sleep(.2)
                if dbus('org.freedesktop.DBus.Properties.Get', 'org.mpris.MediaPlayer2.Player', 'CanControl', expect=False).returncode == 0:
                    break
            else:
                raise AssertionError('MPRIS did not appear')
            player('OpenUri', sequence.as_uri())
            time.sleep(.8)
            assert 'Signal 2' in prop('Metadata')
            assert 'Playing' in prop('PlaybackStatus')
            player('Pause')
            time.sleep(.3)
            assert 'Paused' in prop('PlaybackStatus')
            paused_position = int(re.search(r'int64 ([0-9]+)', prop('Position')).group(1))
            dbus('org.freedesktop.DBus.Properties.Set', 'org.mpris.MediaPlayer2.Player', 'LoopStatus', "<'Playlist'>")
            dbus('org.freedesktop.DBus.Properties.Set', 'org.mpris.MediaPlayer2.Player', 'Shuffle', '<true>')
            time.sleep(.35)
            assert 'Paused' in prop('PlaybackStatus')
            assert abs(int(re.search(r'int64 ([0-9]+)', prop('Position')).group(1)) - paused_position) < 20000
            dbus('org.freedesktop.DBus.Properties.Set', 'org.mpris.MediaPlayer2.Player', 'LoopStatus', "<'None'>")
            dbus('org.freedesktop.DBus.Properties.Set', 'org.mpris.MediaPlayer2.Player', 'Shuffle', '<false>')
            player('Seek', '1000000')
            time.sleep(.3)
            assert int(re.search(r'int64 ([0-9]+)', prop('Position')).group(1)) >= 1000000
            player('Play')
            time.sleep(.3)
            player('Seek', '8000000')
            time.sleep(.3)
            player('Seek', '--', '-8000000')
            time.sleep(.5)
            assert 'Signal 2' in prop('Metadata')
            track_id = re.search(r"objectpath '([^']+)'", prop('Metadata')).group(1)
            def position():
                return int(re.search(r'int64 ([0-9]+)', prop('Position')).group(1))
            previous = position()
            for target in (10000000, 1000000) * 10:
                player('SetPosition', track_id, str(target))
                # While the pipeline is rebuilt the reported position must stay
                # on the old or the new point, never fall back to the start.
                floor = min(previous, target) - 500000
                end = time.monotonic() + .45
                while time.monotonic() < end:
                    sample = position()
                    assert sample >= floor, ('position fell back during seek', target, sample)
                    time.sleep(.03)
                previous = target
                assert 'Signal 2' in prop('Metadata'), (target, prop('Metadata'), prop('Position'))
            player('Next')
            time.sleep(.4)
            assert 'Signal 1' in prop('Metadata')
            dbus('org.freedesktop.DBus.Properties.Set', 'org.mpris.MediaPlayer2.Player', 'Volume', '<0.6>')
            time.sleep(.3)
            assert abs(float(re.search(r'[0-9]+(?:\.[0-9]+)?', prop('Volume')).group()) - .6) < .001
            dbus('org.freedesktop.DBus.Properties.Set', 'org.mpris.MediaPlayer2.Player', 'LoopStatus', "<'Track'>")
            time.sleep(.4)
            assert 'Track' in prop('LoopStatus')
            dbus('org.freedesktop.DBus.Properties.Set', 'org.mpris.MediaPlayer2.Player', 'Shuffle', '<true>')
            time.sleep(.4)
            assert 'true' in prop('Shuffle')
            player('Stop')
            time.sleep(.3)
            assert 'Stopped' in prop('PlaybackStatus')
            remote = dbus('org.mpris.MediaPlayer2.Player.OpenUri', 'https://example.invalid/music.flac', expect=False)
            assert remote.returncode != 0
            subprocess.run([str(binary), str(music / '2.flac')], env=env, check=True, timeout=6)
            time.sleep(.5)
            assert 'Signal 2' in prop('Metadata')
            subprocess.run([str(binary), '--play-pause'], env=env, check=True, timeout=6)
            time.sleep(.3)
            assert 'Paused' in prop('PlaybackStatus')
            dbus('org.mpris.MediaPlayer2.Quit')
            app.wait(timeout=8)
            assert app.returncode == 0
            saved = json.loads((data / 'settings.json').read_text())
            assert saved['queue']['tracks'][0]['title'] == 'Signal 2'
            assert abs(saved['volume'] - .6) < .01
            saved['position'] = 5.0
            saved['queue']['repeat'] = 'Off'
            saved['queue']['shuffle'] = False
            (data / 'settings.json').write_text(json.dumps(saved))
            app = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=log)
            time.sleep(.8)
            assert 'Stopped' in prop('PlaybackStatus')
            player('Play')
            time.sleep(.8)
            assert int(re.search(r'int64 ([0-9]+)', prop('Position')).group(1)) >= 5000000
            # Seeks far from the end stay in the running pipeline: the position lands on the
            # target, playback never flickers to Paused and the track is not reported as new.
            player('OpenUri', long_track.as_uri())
            time.sleep(1.0)
            assert 'Long signal' in prop('Metadata')
            long_id = re.search(r"objectpath '([^']+)'", prop('Metadata')).group(1)
            for target in (60_000_000, 20_000_000, 95_000_000):
                player('SetPosition', long_id, str(target))
                states = []
                for _ in range(6):
                    time.sleep(.05)
                    states.append(prop('PlaybackStatus'))
                assert all('Playing' in st for st in states), states
                position = int(re.search(r'int64 ([0-9]+)', prop('Position')).group(1))
                assert abs(position - target) < 1_500_000, (target, position)
                assert re.search(r"objectpath '([^']+)'", prop('Metadata')).group(1) == long_id
            player('Seek', '10000000')
            time.sleep(.4)
            position = int(re.search(r'int64 ([0-9]+)', prop('Position')).group(1))
            assert abs(position - 105_000_000) < 1_500_000, position
            # Near the end the pipeline is rebuilt; it must still never report Paused.
            player('SetPosition', long_id, '135000000')
            states = []
            for _ in range(10):
                time.sleep(.03)
                states.append(prop('PlaybackStatus'))
            assert all('Playing' in st for st in states), states
            position = int(re.search(r'int64 ([0-9]+)', prop('Position')).group(1))
            assert abs(position - 135_000_000) < 1_500_000, position
            # Pausing right after a seek is reported as paused.
            player('SetPosition', long_id, '40000000')
            player('Pause')
            time.sleep(.5)
            assert 'Paused' in prop('PlaybackStatus')
            player('Play')
            time.sleep(.4)
            assert 'Playing' in prop('PlaybackStatus')
            dbus('org.mpris.MediaPlayer2.Quit')
            app.wait(timeout=8)
            assert app.returncode == 0
            log.seek(0)
            output = log.read()
            assert 'panicked' not in output and 'CRITICAL' not in output, output
            print(json.dumps({'result': 'passed', 'checks': ['playlist order', 'metadata', 'pause/resume', 'seek', 'next', 'volume', 'repeat', 'shuffle', 'stop', 'reject remote URI', 'single instance file open', 'desktop action', 'persist state', 'seek with preloaded next stream', 'twenty alternating seeks preserve current track and never report 0:00', 'restore position without autoplay', 'repeat and shuffle preserve paused position', 'in-place seeks on a long track keep playing without a new track']}))
        finally:
            if app.poll() is None:
                app.terminate()
                app.wait(timeout=5)
            if app.returncode:
                log.seek(0)
                print(log.read(), file=sys.stderr)
