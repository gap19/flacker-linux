#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ -x target/release/flacker ]]; then exec target/release/flacker "$@"; fi
if [[ -x dist/flacker ]]; then exec dist/flacker "$@"; fi
if command -v cargo >/dev/null; then cargo build --release --locked; exec target/release/flacker "$@"; fi
printf '%s\n' 'Build the project or install a package first. See README.md.' >&2
exit 1
