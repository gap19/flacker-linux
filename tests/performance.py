#!/usr/bin/env python3
"""Small local measurement, intended to run through headless.py. No music writes."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

binary = Path(sys.argv[1]).resolve()
library = Path(sys.argv[2]).resolve()
with tempfile.TemporaryDirectory(prefix='flacker-perf-') as temp:
    data = Path(temp)
    (data / 'settings.json').write_text(json.dumps({'roots': [str(library)], 'theme': 'dark'}))
    env = dict(os.environ, FLACKER_DATA_DIR=str(data), FLACKER_SILENT='1')
    env.pop('FLACKER_UI_SMOKE', None)
    with (data / 'application.log').open('w+') as log:
        app = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=log)
        try:
            time.sleep(15)
            def usage():
                status = dict(line.split(':', 1) for line in Path(f'/proc/{app.pid}/status').read_text().splitlines() if ':' in line)
                stat = Path(f'/proc/{app.pid}/stat').read_text().split()
                ticks = int(stat[13]) + int(stat[14])
                return int(status['VmRSS'].split()[0]), ticks, int(status['Threads'].strip())
            rss, before, threads = usage()
            start = time.monotonic()
            time.sleep(5)
            _, after, _ = usage()
            elapsed = time.monotonic() - start
            print(json.dumps({'resident_memory_mib': round(rss / 1024, 2), 'idle_cpu_percent_one_core': round((after - before) / os.sysconf('SC_CLK_TCK') / elapsed * 100, 2), 'threads': threads, 'conditions': 'Xvfb, Cairo software renderer, idle library grid after initial indexing; machine load affects this sample'}))
            subprocess.run(['gdbus', 'call', '--session', '--dest', 'org.mpris.MediaPlayer2.flacker', '--object-path', '/org/mpris/MediaPlayer2', '--method', 'org.mpris.MediaPlayer2.Quit'], check=True, capture_output=True)
            app.wait(timeout=8)
        finally:
            if app.poll() is None:
                app.terminate()
                app.wait(timeout=5)
