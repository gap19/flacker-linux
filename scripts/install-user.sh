#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --locked
make install PREFIX="${FLACKER_PREFIX:-$HOME/.local}"
if command -v update-desktop-database >/dev/null; then update-desktop-database "${FLACKER_PREFIX:-$HOME/.local}/share/applications"; fi
printf '%s\n' 'FLACker installed. Open it from the GNOME application menu.'
