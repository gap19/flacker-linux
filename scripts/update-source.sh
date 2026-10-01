#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ ! -d .git ]]; then printf '%s\n' 'This folder is not a Git clone. Clone the published repository first.' >&2; exit 1; fi
if [[ -n "$(git status --porcelain)" ]]; then printf '%s\n' 'There are local changes. Commit or stash them before updating.' >&2; exit 1; fi
git pull --ff-only
cargo test --locked
./scripts/install-user.sh
