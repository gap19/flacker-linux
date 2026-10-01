#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml","rb"))["package"]["version"])')
./scripts/source-release.sh
top="$PWD/.build/rpmbuild"
mkdir -p "$top"/{SOURCES,SPECS,BUILD,BUILDROOT,RPMS,SRPMS,TMP}
cp "dist/flacker-$version.tar.gz" "$top/SOURCES/"
cp packaging/flacker.spec "$top/SPECS/"
rpmbuild --define "_topdir $top" --define "_tmppath $top/TMP" -ba "$top/SPECS/flacker.spec"
find "$top/RPMS" "$top/SRPMS" -name '*.rpm' -exec cp {} dist/ \;
(cd dist && sha256sum ./*.rpm > SHA256SUMS-rpm)
