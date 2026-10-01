#!/usr/bin/env python3
"""Portable native UI regression: synthetic audio/art only, no user's library."""
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib


def png(path, width, height):
    """Deterministic colored test pattern, generated without image dependencies."""
    def chunk(kind, body):
        return struct.pack('>I', len(body)) + kind + body + struct.pack('>I', zlib.crc32(kind + body))
    rows = []
    for y in range(height):
        color = (37, 125, 155) if y < height // 2 else (217, 162, 90)
        rows.append(b'\0' + bytes(color) * width)
    path.write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(b''.join(rows))) + chunk(b'IEND', b''))


binary = Path(sys.argv[1]).resolve()
with tempfile.TemporaryDirectory(prefix='flacker-ui-review-') as temp:
    root = Path(temp)
    source = root / 'source.flac'
    subprocess.run(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-f', 'lavfi',
                    '-i', 'sine=frequency=440:sample_rate=44100:duration=3', '-c:a', 'flac',
                    '-metadata', 'TITLE=Review song', '-metadata', 'ALBUM=Review album',
                    '-metadata', 'ARTIST=Flacker tests', '-metadata', 'GENRE=Jazz', '-metadata', 'DATE=2002', '-metadata', 'ORIGINALDATE=1978', '-metadata', 'LYRICS=First verse\nSecond verse\n\nAnother verse',
                    str(source)], check=True)
    music = root / 'music'
    for i in range(80):
        album = music / f'Album {i:03}'
        album.mkdir(parents=True)
        if i == 79:
            subprocess.run(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-i', str(source), '-c:a', 'copy',
                            '-metadata', 'ALBUM=An exceptionally long album title with multiple editions, accents, 日本語 and complete recording credits',
                            '-metadata', 'TITLE=An exceptionally long track title with accents and 日本語', str(album / '01.flac')], check=True)
        else:
            shutil.copyfile(source, album / '01.flac')
        (album / '01.lrc').write_text('[00:00.00]Local text fallback\n')
        png(album / 'cover.png', 120, 120)
    art = music / 'Album 000' / 'Artwork' / ('edition-' + '1234567890' * 12)
    art.mkdir(parents=True)
    png(art / 'back-99999999999999999999999999999999.png', 2400, 900)
    png(art / 'booklet-2.png', 800, 2400)
    png(art / 'booklet-10.png', 1800, 1200)
    data = root / 'data'
    data.mkdir()
    (data / 'settings.json').write_text(json.dumps({'roots': [str(music)], 'width': 1320, 'height': 880, 'theme': 'dark'}))
    screens = root / 'screens'
    env = dict(os.environ, FLACKER_DATA_DIR=str(data), FLACKER_UI_SMOKE=str(screens),
               FLACKER_REVIEW_SMOKE='1', FLACKER_EXPERIENCE_SMOKE='1', FLACKER_SILENT='1')
    env.pop('FLACKER_SMOKE_ALBUM', None)
    run = subprocess.run([str(binary)], env=env, capture_output=True, text=True, timeout=120)
    if os.environ.get("FLACKER_TEST_REPORT_DIR") and screens.exists():
        shutil.copytree(screens, os.environ["FLACKER_TEST_REPORT_DIR"], dirs_exist_ok=True)
    if run.returncode:
        print(run.stderr, file=sys.stderr)
        raise SystemExit(run.returncode)
    assert not any(s in run.stderr for s in ['CRITICAL', 'panicked', 'Theme parser error']), run.stderr
    if os.environ.get('FLACKER_LAYOUT_SMOKE'):
        report = json.loads((screens / 'layout.json').read_text())
        assert report['result'] == 'passed', report
        print(json.dumps(report))
        sys.exit(0)
    report = json.loads((screens / 'review.json').read_text())
    assert report['model_albums'] == 10000 and report['allocated_cells'] < 1000
    assert report['scroll_position_restored'] and report['lyrics_display'] == 'static'
    # Grid covers are decoded at the device pixel size (sharp on HiDPI displays).
    assert report['grid_cover_pixels'] == 200 * report['scale_factor'], report
    if os.environ.get('FLACKER_TEST_REPORT_DIR'):
        shutil.copytree(screens, os.environ['FLACKER_TEST_REPORT_DIR'], dirs_exist_ok=True)
    print(json.dumps({'result': 'passed', **report}))
