#!/usr/bin/env python3
"""Isolate GUI tests from the user's desktop, session bus and audio output."""
import os
from pathlib import Path
import random
import shutil
import subprocess
import sys
import tempfile
import time

root = Path(__file__).resolve().parents[1]
xvfb = shutil.which('Xvfb') or str(root / '.build/sysroot/usr/bin/Xvfb')
display = f':{random.randint(110, 499)}'
env = dict(os.environ, DISPLAY=display, GDK_BACKEND='x11', GSK_RENDERER=os.environ.get('FLACKER_TEST_RENDERER', 'cairo'),
           GTK_A11Y='none', FLACKER_SILENT='1', GTK_USE_PORTAL='0')
with tempfile.TemporaryFile() as log:
    server = subprocess.Popen([xvfb, display, '-screen', '0', '1440x1100x24', '-nolisten', 'tcp'], stdout=log, stderr=log)
    try:
        time.sleep(.5)
        if server.poll() is not None:
            log.seek(0)
            raise RuntimeError(log.read().decode())
        result = subprocess.run(['dbus-run-session', '--config-file', str(root / 'tests/session.conf'), '--', *sys.argv[1:]], env=env)
        sys.exit(result.returncode)
    finally:
        server.terminate()
        server.wait(timeout=5)
