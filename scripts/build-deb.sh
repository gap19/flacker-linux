#!/usr/bin/env bash
# Build dist/flacker_<version>-1_<arch>.deb on Debian 13+ / Ubuntu 25.04+ (or in packaging/debian/Containerfile).
set -euo pipefail
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(dpkg --print-architecture)
offline=()
if [[ -f .cargo/config.toml ]]; then offline=(--offline); fi  # vendored source release
cargo build --release --locked "${offline[@]}"
root=$(mktemp -d)
work=$(mktemp -d)
trap 'rm -rf "$root" "$work"' EXIT
make install DESTDIR="$root" PREFIX=/usr
install -Dm644 README.md "$root/usr/share/doc/flacker/README.md"
install -Dm644 CHANGELOG.md "$root/usr/share/doc/flacker/changelog"
{
  printf '%s\n' 'Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/' 'Upstream-Name: FLACker' 'Source: https://github.com/gap19/flacker-linux' ''
  printf '%s\n' 'Files: *' 'Copyright: 2026 Gabriel Aleixo' 'License: GPL-3.0-or-later' ' On Debian systems, see /usr/share/common-licenses/GPL-3.' ''
  printf '%s\n' 'Files: assets/fonts/*' 'Copyright: The League of Moveable Type; Adobe' 'License: OFL-1.1' ' The font licenses are installed in /usr/share/licenses/flacker/fonts/.'
} > "$root/usr/share/doc/flacker/copyright"
# Library dependencies are computed from the binary, as debhelper would.
mkdir -p "$work/debian"
printf '%s\n' 'Source: flacker' '' 'Package: flacker' 'Architecture: any' > "$work/debian/control"
shlibs=$(cd "$work" && dpkg-shlibdeps -O "$root/usr/bin/flacker" | sed 's/^shlibs:Depends=//')
mkdir -p "$root/DEBIAN"
cat > "$root/DEBIAN/control" <<CONTROL
Package: flacker
Version: ${version}-1
Architecture: ${arch}
Maintainer: Gabriel Aleixo <gabriel@aleixo.net>
Installed-Size: $(du -sk "$root/usr" | cut -f1)
Depends: ${shlibs}, libgtk-4-1 (>= 4.16), libadwaita-1-0 (>= 1.6), gstreamer1.0-plugins-base, gstreamer1.0-plugins-good
Section: sound
Priority: optional
Homepage: https://github.com/gap19/flacker-linux
Description: offline high-resolution FLAC player for GNOME
 FLACker is a native Rust/GTK4/libadwaita music player with an artwork-led
 interface. It reads local FLAC metadata, covers and lyrics, supports high
 resolution and gapless queues, and integrates with GNOME through MPRIS.
 The music library is read-only. No accounts or online services are used.
CONTROL
mkdir -p dist
dpkg-deb --build --root-owner-group "$root" "dist/flacker_${version}-1_${arch}.deb"
(cd dist && sha256sum "flacker_${version}-1_${arch}.deb" > "flacker_${version}-1_${arch}.deb.sha256")
