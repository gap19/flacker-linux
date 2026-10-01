# Testing

## Quick checks

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

## Decoded audio

`./scripts/test-audio.sh` generates 16- and 24-bit signals at 44.1, 96 and 192 kHz with FFmpeg, plays them through a synchronized silent sink and compares the decoded PCM hash, queue order, duplicate entries and format changes. It never claims anything about a physical DAC.

## Desktop integration

These suites need Xvfb, D-Bus (`dbus-daemon`), `gdbus`, Python 3 and FFmpeg. `tests/headless.py` starts a temporary display and a private session bus, then runs the given command:

```sh
cargo build --locked
python3 tests/headless.py python3 tests/mpris_smoke.py   target/debug/flacker
python3 tests/headless.py python3 tests/ui_review.py     target/debug/flacker
python3 tests/headless.py python3 tests/scan_policy.py   target/debug/flacker
python3 tests/headless.py python3 tests/output_server.py target/debug/flacker
python3 tests/headless.py python3 tests/session_stress.py target/debug/flacker
```

- `mpris_smoke.py`: playlist order, metadata, pause/resume, seek (including in-place seeks on a long track that never flicker to Paused), next, volume, repeat, shuffle, stop, remote-URI rejection, single instance, desktop actions and state restore.
- `ui_review.py`: screens, filters, Focus Mode, gallery zoom, narrow windows and 200% text (`FLACKER_LAYOUT_SMOKE=1` adds layout checks and the compact navigation menu, `FLACKER_UI_SMOKE=DIR` saves screenshots). Run it with `GDK_SCALE=2` to check that covers are decoded at the device pixel size.
- `scan_policy.py`, `output_server.py`, `session_stress.py`: indexing rules, audio-server restarts and long sessions.

All suites use generated music and temporary data directories (`FLACKER_DATA_DIR`).

## Metadata

```sh
desktop-file-validate packaging/io.github.gap19.FLACker.desktop
appstreamcli validate --no-net packaging/io.github.gap19.FLACker.metainfo.xml
```
