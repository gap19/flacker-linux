Name:           flacker
Version:        1.0.0
Release:        1%{?dist}
Summary:        Offline high-resolution FLAC player for GNOME
License:        GPL-3.0-or-later AND OFL-1.1
URL:            https://github.com/gap19/flacker-linux
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  rust >= 1.85
BuildRequires:  cargo
BuildRequires:  gcc
BuildRequires:  make
BuildRequires:  pkgconfig(gtk4) >= 4.16
BuildRequires:  pkgconfig(libadwaita-1) >= 1.6
BuildRequires:  pkgconfig(gstreamer-1.0) >= 1.22
BuildRequires:  gstreamer1-plugins-base
BuildRequires:  pkgconfig(sqlite3)
BuildRequires:  pkgconfig(fontconfig)
Requires:       gtk4 >= 4.16
Requires:       libadwaita >= 1.6
Requires:       gstreamer1 >= 1.22
Requires:       gstreamer1-plugins-base
Requires:       gstreamer1-plugins-good
# Versions before 1.0 were packaged as "astracks"; this package replaces them.
Obsoletes:      astracks < 1.0.0
Provides:       astracks = %{version}-%{release}

# Source includes Cargo.lock and vendored crate sources for a network-free build.
%global debug_package %{nil}

%description
FLACker is a native Rust/GTK4/libadwaita music player with an artwork-led
interface. It reads local FLAC metadata, covers and lyrics, supports high
resolution and continuous queues, and integrates with GNOME through MPRIS.
The music library is read-only. No accounts or online services are required.

%prep
%setup -q

%build
export CARGO_HOME="$PWD/.cargo-home"
export CARGO_BUILD_JOBS=2
# Archives have normalized timestamps. Invalidate this package when a caller
# reuses CARGO_TARGET_DIR so Cargo cannot mistake changed source for old input.
cargo clean -p flacker --offline
cargo clean --release -p flacker --offline
cargo build --release --locked --offline

%check
export CARGO_HOME="$PWD/.cargo-home"
cargo test --locked --offline

%install
make install DESTDIR=%{buildroot} PREFIX=%{_prefix}

%files
%license LICENSE LICENSES/
%license %{_datadir}/licenses/flacker/fonts/
%doc README.md CHANGELOG.md SECURITY.md docs/
%{_bindir}/flacker
%{_datadir}/applications/io.github.gap19.FLACker.desktop
%{_datadir}/icons/hicolor/scalable/apps/io.github.gap19.FLACker.svg
%{_datadir}/metainfo/io.github.gap19.FLACker.metainfo.xml

%changelog
* Thu Oct 01 2026 Gabriel Aleixo <gabriel@aleixo.net> - 1.0.0-1
- First public release.
