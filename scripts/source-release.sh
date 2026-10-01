#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml","rb"))["package"]["version"])')
# A build fetches only the host platform's crates; notices and the vendored source cover
# every platform in Cargo.lock (no-op when already cached).
cargo fetch --locked
python3 scripts/third-party-licenses.py
mkdir -p dist .build
stage=$(mktemp -d "$PWD/.build/source.XXXXXX")
trap 'rm -rf "$stage"' EXIT
root="$stage/flacker-$version"
mkdir -p "$root/.cargo"
# Explicit allow-list: no library, personal data, screenshots or build cache.
cp -r Cargo.toml Cargo.lock LICENSE LICENSES README.md CHANGELOG.md SECURITY.md Makefile .gitignore .github src assets packaging scripts tests "$root/"
# Documentation travels with the source; screenshots and local validation notes do not.
if [[ -d docs ]]; then cp -r docs "$root/"; rm -rf "$root/docs/screenshots" "$root/docs/local-validation.json"; fi
cargo vendor --locked --versioned-dirs "$root/vendor" > "$stage/vendor-config.toml"
printf '%s\n' '[source.crates-io]' 'replace-with = "vendored-sources"' '' '[source.vendored-sources]' 'directory = "vendor"' > "$root/.cargo/config.toml"
# Local-only bootstrap paths are not part of the source release.
rm -f "$root/scripts/dev-env.sh"
find "$root" -type d -name __pycache__ -prune -exec rm -rf {} +
epoch=${SOURCE_DATE_EPOCH:-$(python3 -c 'import datetime,xml.etree.ElementTree as E; d=E.parse("packaging/io.github.gap19.FLACker.metainfo.xml").find("releases/release").get("date"); print(int(datetime.datetime.fromisoformat(d).replace(tzinfo=datetime.timezone.utc).timestamp()))')}
tar --sort=name --mtime="@$epoch" --owner=0 --group=0 --numeric-owner -C "$stage" -czf "dist/flacker-$version.tar.gz" "flacker-$version"
sha256sum "dist/flacker-$version.tar.gz" > "dist/flacker-$version.tar.gz.sha256"
printf 'Source: %s\n' "$PWD/dist/flacker-$version.tar.gz"
