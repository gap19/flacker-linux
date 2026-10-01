<p align="center"><img src="assets/io.github.gap19.FLACker.svg" width="112" alt="FLACker icon"></p>
<h1 align="center">FLACker</h1>
<p align="center">Your record collection, offline.</p>

FLACker is a native GNOME music player for personal FLAC libraries, written in Rust with GTK 4, libadwaita and GStreamer. It treats your collection as a shelf of records: covers come first, albums play in their intended order, and lyrics, booklets and edition details sit next to the music.

**Fully offline. No accounts, telemetry, ads or network lookups. Your library is opened read-only — FLACker never renames, retags or rewrites a file.**

There is also a native [FLACker for Android](https://github.com/gap19/flacker-android).

## Features

- **Library:** albums, artists (cover grid with album counts), tracks and favorites, with search, filters (genre, original or edition year, resolution, favorites), sorting and "shuffle a whole album".
- **Playback:** FLAC up to 24-bit/192 kHz, gapless queues, repeat and shuffle that never restart the current track, ReplayGain (track or album) with peak protection.
- **Album order:** follows `playlist.m3u8` / `playlist.m3u` in the album folder; otherwise disc, track number and path. Different editions of an album stay separate (`RELEASE_ID` / `MUSICBRAINZ_ALBUMID`), and `CD1`/`Disc 2` subfolders join their album.
- **Artwork and booklets:** adaptive viewer with zoom, pan and fullscreen for embedded covers and images in `Artwork`, `Booklet`, `Covers`, `Scans` or `Images` folders; PDFs open in the system viewer.
- **Lyrics:** static, selectable text from FLAC tags or matching `.txt`, `.lrc`, `.srt`, `.vtt`, `.lyrics` and `.text` files (UTF-8, UTF-16 and legacy Latin-1). Timestamps are hidden, never invented.
- **Focus Mode:** large artwork, timeline and transport in a window of its own, with lyrics on demand and fullscreen.
- **Edition sheet:** dates, label, catalog number, credits and every tag of the file, without guessing missing fields.
- **Queue:** play next, add to queue, reorder and remove upcoming tracks without interrupting audio; local M3U8 playlists.
- **Desktop integration:** GNOME media controls (MPRIS), media keys, light/dark/system themes, optional stereo meter, listening history and output device selection.

FLAC is the only audio format. CUE sheets, logs and data tracks are left alone.

## Install

Prebuilt x86_64 packages are in [`dist/`](dist/). Check them against `dist/SHA256SUMS`.

**Fedora 44 or newer**

```sh
sudo dnf install ./dist/flacker-1.0.0-1.fc44.x86_64.rpm
```

**Debian 13 (trixie) or newer, Ubuntu 25.04 or newer**

```sh
sudo apt install ./dist/flacker_1.0.0-1_amd64.deb
```

Then open **FLACker** from the application menu. On first launch it reads the system Music folder (falling back to `~/Music`); more folders can be added in Preferences. The library is indexed at startup or when you choose **Refresh library**; there is no background folder watching.

Upgrading from the pre-1.0 "Astracks" builds replaces the old package and moves your settings, favorites and playlists to the new data folder automatically.

## Build from source

Requirements: Rust 1.85+, GTK 4.16+, libadwaita 1.6+, GStreamer 1.22+ (with plugins-base and plugins-good) and SQLite.

```sh
# Fedora
sudo dnf install rust cargo gcc make pkgconf-pkg-config gtk4-devel libadwaita-devel \
  gstreamer1-devel gstreamer1-plugins-base gstreamer1-plugins-good sqlite-devel fontconfig-devel
# Debian / Ubuntu
sudo apt install cargo rustc gcc make pkg-config libgtk-4-dev libadwaita-1-dev \
  libgstreamer1.0-dev gstreamer1.0-plugins-base gstreamer1.0-plugins-good libsqlite3-dev libfontconfig-dev

cargo build --release --locked
./scripts/install-user.sh          # installs into ~/.local
```

Packages:

```sh
./scripts/build-rpm.sh             # on Fedora, writes dist/*.rpm
./scripts/build-deb.sh             # on Debian/Ubuntu, writes dist/*.deb
```

`./scripts/source-release.sh` creates a source archive with vendored crates for network-free builds.

## Keyboard shortcuts

| Shortcut | Action |
|---|---|
| Space | Play / pause (outside text fields) |
| Ctrl+F | Search |
| Ctrl+O | Open a FLAC file or M3U playlist |
| Ctrl+J | Now playing |
| Ctrl+Shift+F | Focus Mode (F11 fullscreen, Esc leaves) |
| Alt+← / Alt+→ | Previous / next track |
| Ctrl+Q | Quit |

## Your data

Settings, favorites, playlists, history and the library index live in `$XDG_DATA_HOME/flacker` (usually `~/.local/share/flacker`); reduced covers in `$XDG_CACHE_HOME/flacker` (capped at 256 MiB). Listening history is off by default. Hidden folders, folders whose name starts with `_` (handy for tools, backups and work folders), `Provenance`, `node_modules` and `target` are skipped, and symbolic links are not followed. Set `FLACKER_DATA_DIR` to run with an isolated profile.

## Development

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
./scripts/test-audio.sh            # decodes generated signals and compares PCM
```

Desktop integration suites run under Xvfb and a private D-Bus session; see [docs/TESTING.md](docs/TESTING.md).

## Disclaimer

FLACker is shared in the hope that it will be useful to others who enjoy listening to their own music collections. It is provided "as is", without warranty of any kind, express or implied, including any warranty of merchantability or fitness for a particular purpose. The author is not liable for any damage or loss arising from its use, including to files, devices or data. Using, building, installing or modifying this software is at your own risk and under your own responsibility. Keep backups of anything that matters to you. See sections 15 and 16 of the [license](LICENSE).

## License

FLACker is free software under the [GNU General Public License v3.0 or later](LICENSE). The bundled League Gothic and Source Sans 3 fonts are under the SIL Open Font License 1.1 (`assets/fonts/`). Third-party crate notices are in [`LICENSES/`](LICENSES/). The license grants no rights over the music, lyrics or images in your library.
