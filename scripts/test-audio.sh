#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
bin=${FLACKER_BIN:-target/debug/flacker}
if [[ ! -x "$bin" ]]; then cargo build --locked; fi
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
for item in '01:44100:s16:440' '02:44100:s16:660' '03:96000:s32:880' '04:192000:s32:990'; do
  IFS=: read -r name rate format frequency <<< "$item"
  ffmpeg -hide_banner -loglevel error -f lavfi -i "sine=frequency=$frequency:sample_rate=$rate:duration=0.8" -af "aformat=sample_fmts=$format" -c:a flac -sample_fmt "$format" -metadata "TITLE=Test signal $name" -metadata 'ARTIST=Flacker test signals' -metadata 'ALBUM=Generated calibration' -metadata "TRACKNUMBER=$name" "$fixture/$name.flac"
done
ffmpeg -hide_banner -loglevel error -f lavfi -i 'sine=frequency=330:sample_rate=96000:duration=12' -c:a flac -sample_fmt s32 "$fixture/modes.flac"
before=$(sha256sum "$fixture"/*.flac)
"$bin" --audio-test "$fixture/01.flac" "$fixture/02.flac" "$fixture/03.flac" "$fixture/04.flac"
"$bin" --audio-test "$fixture/02.flac" "$fixture/01.flac" "$fixture/02.flac"
"$bin" --audio-mode-test "$fixture/modes.flac" "$fixture/01.flac" "$fixture/02.flac" > "$fixture/modes.json"
"$bin" --audio-edit-test "$fixture/modes.flac" "$fixture/01.flac" "$fixture/02.flac" > "$fixture/edits.json"
python3 - "$fixture/modes.json" "$fixture/edits.json" <<'PYCOMPARE'
import json,sys
m,e=(json.load(open(p)) for p in sys.argv[1:])
assert m['pcm_sha256']==e['pcm_sha256'], 'Meter/queue edits changed PCM bytes'
print(json.dumps(m)); print(json.dumps(e))
PYCOMPARE
after=$(sha256sum "$fixture"/*.flac)
[[ "$before" = "$after" ]]
printf '%s\n' 'Playback ordering, duplicate entries, format changes and read-only invariants passed.'
