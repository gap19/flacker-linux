# Changelog

All notable changes to FLACker for Linux are documented here. Versions follow [Semantic Versioning](https://semver.org/).

## 1.0.0 — 2026-10-01

First public release.

- Native GNOME player for local FLAC libraries: albums, artists, tracks and favorites with search, filters, sorting and whole-album shuffle.
- Gapless playback up to 24-bit/192 kHz with repeat, shuffle and ReplayGain (track or album, peak-protected).
- Album order from `playlist.m3u8`, separate editions, multi-disc folders.
- Artwork and booklet viewer, static lyrics from tags and sidecar files, edition sheet and Focus Mode.
- Queue editing without audio interruptions, local M3U8 playlists, MPRIS media controls, optional meter and history.
- The playing track is highlighted in every track list (album pages, Tracks, Favorites, search results) and follows track changes live.
- Missing, unreadable or damaged files are skipped with a notice instead of stopping playback; unmounted drives keep their known albums.
- Seeks within a track stay in the running pipeline (no audible restart or Playing/Paused flicker), Now playing updates in place on track changes, and commands are handled immediately while the timer slows down when nothing moves on screen.
- Sharp artwork on HiDPI displays; compact navigation menu on narrow windows; symlinked library folders, Latin-1 `.m3u` playlists, ID3-prefixed FLAC files and covers stored as a non-front embedded picture.
- Read-only library access, no network services.
- Fedora RPM and Debian/Ubuntu DEB packages.
- Data from pre-1.0 "Astracks" builds is migrated automatically; the RPM replaces the old `astracks` package.
