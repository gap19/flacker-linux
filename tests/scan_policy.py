#!/usr/bin/env python3
"""Verify startup/manual-only indexing in a private desktop with synthetic FLACs."""
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time

binary = Path(sys.argv[1]).resolve()

def action(name):
    subprocess.run([
        'gdbus', 'call', '--session', '--dest', 'io.github.gap19.FLACker',
        '--object-path', '/io/github/gap19/FLACker', '--method',
        'org.gtk.Actions.Activate', name, '[]', '{}'
    ], check=True, capture_output=True, text=True, timeout=5)

with tempfile.TemporaryDirectory(prefix='flacker-scan-policy-') as temp:
    root = Path(temp)
    music = root / 'music'
    data = root / 'data'
    music.mkdir()
    data.mkdir()
    subprocess.run(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-f', 'lavfi',
                    '-i', 'sine=frequency=440:duration=0.2', '-c:a', 'flac',
                    str(music / 'one.flac')], check=True)
    (data / 'settings.json').write_text(json.dumps({'roots': [str(music)]}))
    env = dict(os.environ, FLACKER_DATA_DIR=str(data), FLACKER_SILENT='1')
    env.pop('FLACKER_UI_SMOKE', None)
    database = data / 'library.sqlite3'
    def count():
        if not database.exists():
            return 0
        with sqlite3.connect(f'file:{database}?mode=ro', uri=True) as db:
            try:
                return db.execute('SELECT count(*) FROM tracks').fetchone()[0]
            except sqlite3.OperationalError:
                return 0
    def wait_count(expected):
        for _ in range(100):
            if count() == expected:
                return
            assert app.poll() is None, 'App exited during scan'
            time.sleep(.1)
        raise AssertionError(f'Expected {expected} indexed tracks; found {count()}')
    with (root / 'app.log').open('w+') as log:
        app = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=log)
        try:
            wait_count(1)
            shutil.copy2(music / 'one.flac', music / 'two.flac')
            # Root event and the former 90-second timer must both leave the index alone.
            deadline = time.monotonic() + 94
            while time.monotonic() < deadline:
                assert app.poll() is None, 'App exited while idle'
                assert count() == 1, 'Unrequested automatic scan changed the index'
                time.sleep(.5)
            action('scan')
            wait_count(2)
            shutil.copy2(music / 'one.flac', music / 'three.flac')
            time.sleep(.6)
            assert count() == 2, 'Directory event triggered a scan'
            action('quit')
            app.wait(timeout=8)
            assert app.returncode == 0
            app = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=log)
            wait_count(3)
            action('quit')
            app.wait(timeout=8)
            assert app.returncode == 0
            log.seek(0)
            output = log.read()
            assert 'CRITICAL' not in output and 'panicked' not in output, output
            print(json.dumps({'result': 'passed', 'idle_seconds': 94,
                              'root_events_do_not_scan': True,
                              'manual_menu_action': 'passed', 'reopen_scan': 'passed'}))
        finally:
            if app.poll() is None:
                app.terminate()
                app.wait(timeout=5)
            if app.returncode:
                log.seek(0)
                print(log.read(), file=sys.stderr)
