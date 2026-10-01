use crate::{
    library, lyrics,
    model::Track,
    player::{Event, Player},
};
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};
pub fn cli(args: &[String]) -> Option<Result<(), String>> {
    let mode = args.get(1)?;
    match mode.as_str() {
        "--scan" | "--validate" => Some((|| {
            let path = PathBuf::from(args.get(2).ok_or("Give a folder")?);
            let start = Instant::now();
            let r = library::scan(
                &[path],
                &crate::storage::data_dir().join("library.sqlite3"),
                &AtomicBool::new(false),
                None,
            )?;
            let albums = library::albums(&r.tracks);
            let mut lyric_count = 0;
            if mode == "--validate" {
                for t in &r.tracks {
                    let l = lyrics::load(&t.path, t.duration);
                    if !l.text.is_empty() {
                        lyric_count += 1
                    }
                }
            }
            let report = serde_json::json!({"tracks":r.tracks.len(),"albums":albums.len(),"cached":r.cached,"parsed":r.parsed,"errors":r.errors,"seconds":start.elapsed().as_secs_f64(),"high_resolution_tracks":r.tracks.iter().filter(|t|t.high_res()).count(),"embedded_covers":r.tracks.iter().filter(|t|t.has_cover).count(),"lyrics":lyric_count,"lyrics_display":"static","album_order":albums.iter().map(|a|serde_json::json!({"album":a.title,"artist":a.artist,"paths":a.tracks.iter().map(|t|t.path.clone()).collect::<Vec<_>>()})).collect::<Vec<_>>()});
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
            if r.errors.is_empty() {
                Ok(())
            } else {
                Err("Reading produced warnings; see the report".into())
            }
        })()),
        "--audio-test" => Some(audio_test(&args[2..], false, false)),
        "--audio-mode-test" => Some(audio_test(&args[2..], true, false)),
        "--audio-edit-test" => Some(audio_test(&args[2..], false, true)),
        "--audio-output-test" => Some(
            args.get(2)
                .ok_or_else(|| "Give a test FLAC".to_string())
                .and_then(|p| output_test(p)),
        ),
        _ => None,
    }
}
fn audio_test(paths: &[String], exercise_modes: bool, exercise_edits: bool) -> Result<(), String> {
    if paths.is_empty() {
        return Err("Give FLAC files for the test".into());
    }
    let tracks: Vec<Track> = paths
        .iter()
        .map(|p| {
            crate::flac::read(
                &PathBuf::from(p).canonicalize().map_err(|e| e.to_string())?,
                false,
            )
            .map(|m| m.track)
            .map_err(|e| e.to_string())
        })
        .collect::<Result<_, _>>()?;
    if (exercise_modes || exercise_edits) && tracks.first().is_none_or(|t| t.duration < 12.) {
        return Err("The mode test needs a first track of at least 12 seconds".into());
    }
    let edit_track = tracks.last().unwrap().clone();
    let expected: Vec<_> = tracks.iter().map(|t| t.path.clone()).collect();
    let duration: f64 = tracks.iter().map(|t| t.duration).sum();
    let mut p = Player::new(true)?;
    p.volume = 1.0;
    use gst::prelude::*;
    let captured = std::sync::Arc::new(std::sync::Mutex::new((
        0f64,
        0u64,
        Vec::<String>::new(),
        Sha256::new(),
    )));
    let sample_data = captured.clone();
    if let Some(sink) = p.element.property::<Option<gst::Element>>("audio-sink") {
        sink.set_property("signal-handoffs", true);
        sink.connect("handoff", false, move |args| {
            let buffer = args[1].get::<gst::Buffer>().ok()?;
            let pad = args[2].get::<gst::Pad>().ok()?;
            let caps = pad.current_caps()?;
            let structure = caps.structure(0)?;
            let rate = structure.get::<i32>("rate").ok()?;
            let channels = structure.get::<i32>("channels").ok()?;
            let format = structure.get::<String>("format").ok()?;
            let bytes = match format.as_str() {
                "S16LE" | "S16BE" => 2,
                "S24LE" | "S24BE" => 3,
                "S24_32LE" | "S24_32BE" | "S32LE" | "S32BE" | "F32LE" | "F32BE" => 4,
                "F64LE" | "F64BE" => 8,
                _ => return None,
            };
            let frames = buffer.size() as u64 / (bytes * channels as u64);
            let mut data = sample_data.lock().ok()?;
            data.0 += frames as f64 / rate as f64;
            data.1 += frames;
            if let Ok(bytes) = buffer.map_readable() {
                data.3.update(bytes.as_slice());
            }
            let description = format!("{format} / {rate} Hz / {channels} ch");
            if !data.2.contains(&description) {
                data.2.push(description);
            }
            None
        });
    }
    p.set_queue(tracks, 0)?;
    let start = Instant::now();
    let mut changed = Vec::new();
    let mut finished = false;
    let mut mode_changes = 0;
    let mut edit_rounds = 0;
    let mut measured = false;
    while start.elapsed().as_secs_f64() < duration + 15. {
        for event in p.poll() {
            match event {
                Event::TrackChanged => {
                    if let Some(t) = p.current() {
                        changed.push(t.path);
                    }
                }
                Event::Ended => finished = true,
                Event::Error(e) => return Err(e),
                _ => {}
            }
        }
        if finished {
            break;
        }
        if exercise_edits
            && changed.len() == 1
            && edit_rounds < 20
            && p.position() >= 0.2 + f64::from(edit_rounds) * 0.12
        {
            let position = p.position();
            let base_time = p.element.base_time();
            let count = p.summary().2;
            p.move_item(count - 1, count - 2)?;
            p.move_item(count - 2, count - 1)?;
            p.remove(count - 1)?;
            if !p.undo_remove() {
                return Err("Undo did not restore the track".into());
            }
            p.play_next(vec![edit_track.clone()]);
            p.remove(p.queue().index.unwrap() + 1)?;
            p.meter
                .enabled
                .store(true, std::sync::atomic::Ordering::Relaxed);
            if p.element.current_state() != gst::State::Playing
                || p.element.base_time() != base_time
                || (p.position() - position).abs() > 0.1
            {
                return Err("Queue editing interrupted the transport".into());
            }
            edit_rounds += 1;
        }
        measured |= p.meter.values()[0] > 0.;
        if exercise_modes
            && changed.len() == 1
            && mode_changes < 20
            && p.position() >= 0.2 + f64::from(mode_changes) * 0.12
        {
            let position = p.position();
            let base_time = p.element.base_time();
            let repeat = match mode_changes % 3 {
                0 => crate::model::Repeat::One,
                1 => crate::model::Repeat::All,
                _ => crate::model::Repeat::Off,
            };
            p.set_repeat(repeat)?;
            p.set_shuffle(mode_changes % 2 == 0)?;
            if mode_changes == 19 {
                p.set_repeat(crate::model::Repeat::Off)?;
                p.set_shuffle(false)?;
            }
            if p.element.current_state() != gst::State::Playing
                || p.element.base_time() != base_time
                || !p.loaded
                || !p.playing
                || (p.position() - position).abs() > 0.1
            {
                return Err(
                    "Repeat/shuffle interrupted the transport or moved its position".into(),
                );
            }
            mode_changes += 1;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    p.stop()?;
    if !finished {
        return Err("Playback test timed out".into());
    }
    if exercise_modes && mode_changes != 20 {
        return Err(format!("Incomplete mode changes: {mode_changes}/20"));
    }
    if exercise_edits && (edit_rounds != 20 || !measured) {
        return Err(format!(
            "Incomplete edits/meter: {edit_rounds}; signal={measured}"
        ));
    }
    if changed != expected {
        return Err(format!(
            "Order differs: observed {changed:?}; expected {expected:?}"
        ));
    }
    let captured = captured.lock().map_err(|e| e.to_string())?;
    if (captured.0 - duration).abs() > 0.002 {
        return Err(format!(
            "PCM duration differs: {} instead of {duration}",
            captured.0
        ));
    }
    println!(
        "{}",
        serde_json::json!({"result":"passed","tracks":changed.len(),"mode_change_rounds":mode_changes,"queue_edit_rounds":edit_rounds,"meter_signal":measured,"pcm_sha256":format!("{:x}",captured.3.clone().finalize()),"duration":duration,"decoded_duration":captured.0,"decoded_frames":captured.1,"formats":captured.2,"elapsed":start.elapsed().as_secs_f64(),"output":"synchronized fakesink; no physical output claimed"})
    );
    Ok(())
}

fn output_test(path: &str) -> Result<(), String> {
    if !std::env::var("PULSE_SERVER")
        .unwrap_or_default()
        .starts_with("unix:/tmp/flacker-output-")
    {
        return Err("This test needs the private server created by tests/output_server.py".into());
    }
    use gst::prelude::*;
    let mut p = Player::new(false)?;
    let devices = crate::player::outputs();
    let find = |name: &str| {
        devices
            .iter()
            .find(|(_, label)| label.contains(name))
            .map(|(id, _)| id.clone())
            .ok_or_else(|| format!("Test device missing: {name}; {devices:?}"))
    };
    let a = find("Flacker_Test_A")?;
    let b = find("Flacker_Test_B")?;
    let track = crate::flac::read(std::path::Path::new(path), false)
        .map_err(|e| e.to_string())?
        .track;
    let wait = |p: &mut Player, seconds: f64| -> Result<(), String> {
        let start = Instant::now();
        while start.elapsed().as_secs_f64() < seconds {
            for event in p.poll() {
                if let Event::Error(e) = event {
                    return Err(e);
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    };
    p.select_output(&a)?;
    p.set_queue(vec![track], 0)?;
    wait(&mut p, 2.)?;
    if !p.playing || p.position() < 0.5 {
        return Err("Output A did not advance".into());
    }
    let before = p.position();
    p.select_output(&b)?;
    wait(&mut p, 1.5)?;
    if !p.playing || p.position() < before + 0.5 {
        return Err(format!(
            "Output switch lost the position: {before} -> {}",
            p.position()
        ));
    }
    let sink = p
        .element
        .property::<Option<gst::Element>>("audio-sink")
        .unwrap();
    let selected = sink
        .property::<Option<String>>("current-device")
        .unwrap_or_default();
    if selected != b {
        return Err(format!("Effective output differs: {selected} / {b}"));
    }
    p.pause()?;
    wait(&mut p, 0.2)?;
    let before = p.position();
    p.select_output(&a)?;
    wait(&mut p, 0.5)?;
    if p.playing || (p.position() - before).abs() > 0.03 {
        return Err(format!(
            "Paused switch lost the position: {before} -> {}",
            p.position()
        ));
    }
    p.resume()?;
    wait(&mut p, 0.5)?;
    if !p.playing {
        return Err("Resuming after the switch failed".into());
    }
    println!("READY_DISCONNECT");
    {
        use std::io::Write;
        std::io::stdout().flush().map_err(|e| e.to_string())?;
    }
    let start = Instant::now();
    let mut disconnected = false;
    while start.elapsed().as_secs_f64() < 5. {
        for event in p.poll() {
            if matches!(event, Event::Error(_)) {
                disconnected = true;
            }
        }
        if disconnected {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if !disconnected || p.loaded {
        return Err("Server crash did not stop playback cleanly".into());
    }
    p.stop()?;
    println!(
        "{}",
        serde_json::json!({"result":"passed","devices":devices,"switch_while_playing":"preserved position","switch_while_paused":"preserved position and pause","physical_hardware":false,"server_disconnect":"stopped cleanly"})
    );
    Ok(())
}

#[cfg(test)]
mod integration {
    use super::*;
    use std::{fs, io::Write, path::Path};
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "flacker-test-{}-{}",
                std::process::id(),
                rand::random::<u64>()
            ));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn flac(path: &Path, title: &str, track: u32, disc: u32) {
        flac_extra(path, title, track, disc, &[]);
    }
    fn flac_extra(path: &Path, title: &str, track: u32, disc: u32, extra: &[&str]) {
        let mut file = fs::File::create(path).unwrap();
        file.write_all(b"fLaC").unwrap();
        file.write_all(&[0, 0, 0, 34]).unwrap();
        let mut stream = [0u8; 34];
        let packed = (44100u64 << 44) | (1 << 41) | (15 << 36) | 44100;
        stream[10..18].copy_from_slice(&packed.to_be_bytes());
        file.write_all(&stream).unwrap();
        let mut comments = Vec::new();
        comments.extend(0u32.to_le_bytes());
        let mut tags = vec![
            format!("TITLE={title}"),
            "ALBUM=Test edition".into(),
            "ARTIST=Test artist".into(),
            format!("TRACKNUMBER={track}"),
            format!("DISCNUMBER={disc}"),
            "RELEASE_ID=release-concrete".into(),
            "MBID=not-a-release".into(),
            "SOURCE_RELEASE_ID=historic".into(),
        ];
        tags.extend(extra.iter().map(|s| s.to_string()));
        comments.extend((tags.len() as u32).to_le_bytes());
        for tag in tags {
            comments.extend((tag.len() as u32).to_le_bytes());
            comments.extend(tag.as_bytes());
        }
        let len = comments.len();
        file.write_all(&[
            0x84,
            ((len >> 16) & 255) as u8,
            ((len >> 8) & 255) as u8,
            (len & 255) as u8,
        ])
        .unwrap();
        file.write_all(&comments).unwrap();
        file.write_all(&[0xff, 0xf8, 0, 0]).unwrap();
    }
    /// A minimal FLAC with STREAMINFO and the given PICTURE blocks (type, data).
    fn flac_pictures(path: &Path, pictures: &[(u32, &[u8])]) {
        let mut out = b"fLaC".to_vec();
        out.extend([0, 0, 0, 34]);
        let mut stream = [0u8; 34];
        let packed = (44100u64 << 44) | (1 << 41) | (15 << 36) | 44100;
        stream[10..18].copy_from_slice(&packed.to_be_bytes());
        out.extend(stream);
        for (i, (kind, data)) in pictures.iter().enumerate() {
            let mut block = Vec::new();
            block.extend(kind.to_be_bytes());
            block.extend(9u32.to_be_bytes());
            block.extend(b"image/png");
            block.extend(0u32.to_be_bytes());
            block.extend([0u8; 16]);
            block.extend((data.len() as u32).to_be_bytes());
            block.extend(*data);
            let last = if i + 1 == pictures.len() { 0x80 } else { 0 };
            let len = block.len();
            out.extend([last | 6, (len >> 16) as u8, (len >> 8) as u8, len as u8]);
            out.extend(block);
        }
        out.extend([0xff, 0xf8, 0, 0]);
        fs::write(path, out).unwrap();
    }
    #[test]
    fn id3_prefixed_flac_is_read() {
        let f = Fixture::new();
        let plain = f.0.join("plain.flac");
        flac(&plain, "Tagged twice", 1, 1);
        let mut bytes = b"ID3\x04\x00\x00".to_vec();
        // Syncsafe size 200 (1 << 7 | 72), then the padding the size announces.
        bytes.extend([0, 0, 1, 72]);
        bytes.extend([0u8; 200]);
        bytes.extend(fs::read(&plain).unwrap());
        let prefixed = f.0.join("prefixed.flac");
        fs::write(&prefixed, bytes).unwrap();
        let m = crate::flac::read(&prefixed, false).unwrap();
        assert_eq!(m.track.title, "Tagged twice");
        assert!((m.track.duration - 1.).abs() < 0.001);
    }
    #[test]
    fn front_cover_wins_and_other_pictures_are_a_fallback() {
        let f = Fixture::new();
        let mixed = f.0.join("mixed.flac");
        flac_pictures(
            &mixed,
            &[(4, b"back"), (1, b"icon"), (3, b"front"), (0, b"other")],
        );
        let m = crate::flac::read_options(&mixed, false, true).unwrap();
        assert!(m.track.has_cover);
        assert_eq!(m.picture.as_deref(), Some(&b"front"[..]));
        let untyped = f.0.join("untyped.flac");
        flac_pictures(&untyped, &[(4, b"back"), (0, b"other")]);
        let m = crate::flac::read_options(&untyped, false, true).unwrap();
        assert_eq!(m.picture.as_deref(), Some(&b"other"[..]));
        // Without loading pictures, the index still knows there is artwork.
        assert!(crate::flac::read(&untyped, false).unwrap().track.has_cover);
        let icon = f.0.join("icon.flac");
        flac_pictures(&icon, &[(1, b"icon")]);
        assert!(!crate::flac::read(&icon, false).unwrap().track.has_cover);
    }
    #[test]
    fn embedded_lyrics_win_without_loading_artwork() {
        let f = Fixture::new();
        let path = f.0.join("01.flac");
        flac_extra(
            &path,
            "One",
            1,
            1,
            &["LYRICS=[Verse 1]\n[00:01]Embedded text"],
        );
        let mut bytes = fs::read(&path).unwrap();
        bytes[42] = 4; // Add a deliberately unreadable picture block after the comments.
        bytes.truncate(bytes.len() - 4);
        bytes.extend([0x86, 0, 0, 4, 0, 0, 0, 3]);
        fs::write(&path, &bytes).unwrap();
        fs::write(path.with_extension("txt"), "Other text").unwrap();
        let lyrics = lyrics::load(&path, 10.);
        assert_eq!(lyrics.text, "[Verse 1]\nEmbedded text");
        assert!(lyrics.lines.is_empty());
        assert_eq!(lyrics.source, "Lyrics embedded in the FLAC");
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    #[test]
    fn static_sidecar_formats_and_case_insensitive_extensions() {
        let f = Fixture::new();
        let path = f.0.join("01-02 - Señal ü.flac");
        flac(&path, "Señal ü", 2, 1);
        let folder = f.0.join("lYrIcS");
        fs::create_dir(&folder).unwrap();
        for (ext, body) in [
            ("TXT", "Local text"),
            ("lyrics", "Local text"),
            ("text", "Local text"),
            ("LRC", "[00:99]Local text"),
            ("SRT", "1\n00:00:01,000 --> 00:00:02,000\nLocal text"),
            (
                "vtt",
                "WEBVTT\n\ncue\n00:00:01.000 --> 00:00:02.000\nLocal text",
            ),
        ] {
            let sidecar = folder.join("01-02 - Señal ü").with_extension(ext);
            fs::write(&sidecar, body).unwrap();
            let lyrics = lyrics::load(&path, 1.);
            assert_eq!(lyrics.text, "Local text", "{ext}");
            assert!(lyrics.lines.is_empty());
            fs::remove_file(sidecar).unwrap();
        }
        fs::write(f.0.join("Another song.txt"), "Does not match").unwrap();
        assert!(lyrics::load(&path, 1.).text.is_empty());
    }
    #[test]
    fn empty_and_instrumental_sidecars_do_not_hide_valid_words() {
        let f = Fixture::new();
        let path = f.0.join("01.flac");
        flac(&path, "One", 1, 1);
        fs::write(path.with_extension("txt"), "  ").unwrap();
        fs::write(path.with_extension("lyrics"), "Instrumental").unwrap();
        fs::write(path.with_extension("lrc"), "[00:00]Lyrics kept").unwrap();
        assert_eq!(lyrics::load(&path, 1.).text, "Lyrics kept");
        fs::remove_file(path.with_extension("lrc")).unwrap();
        let utf16: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain("Señal ü".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        fs::write(path.with_extension("txt"), utf16).unwrap();
        assert_eq!(lyrics::load(&path, 1.).text, "Señal ü");
    }
    #[test]
    fn index_is_incremental_read_only_and_separates_editions() {
        let f = Fixture::new();
        let music = f.0.join("music");
        fs::create_dir(&music).unwrap();
        let a = music.join("01.flac");
        flac(&a, "One", 1, 1);
        let before = fs::read(&a).unwrap();
        let db = f.0.join("index.sqlite");
        let one = library::scan(
            std::slice::from_ref(&music),
            &db,
            &AtomicBool::new(false),
            None,
        )
        .unwrap();
        assert_eq!(one.parsed, 1);
        assert_eq!(one.tracks[0].release_id, "release-concrete");
        let two = library::scan(
            std::slice::from_ref(&music),
            &db,
            &AtomicBool::new(false),
            None,
        )
        .unwrap();
        assert_eq!(two.cached, 1);
        assert_eq!(two.parsed, 0);
        assert_eq!(before, fs::read(&a).unwrap());
        fs::write(music.join("notes.txt"), b"kept").unwrap();
        fs::remove_file(a).unwrap();
        let three = library::scan(
            std::slice::from_ref(&music),
            &db,
            &AtomicBool::new(false),
            None,
        )
        .unwrap();
        assert!(three.tracks.is_empty());
    }
    #[test]
    fn empty_mount_point_keeps_the_known_tracks() {
        let f = Fixture::new();
        let music = f.0.join("drive");
        fs::create_dir(&music).unwrap();
        let a = music.join("01.flac");
        flac(&a, "One", 1, 1);
        let db = f.0.join("index.sqlite");
        let scan = || {
            library::scan(
                std::slice::from_ref(&music),
                &db,
                &AtomicBool::new(false),
                None,
            )
            .unwrap()
        };
        assert_eq!(scan().tracks.len(), 1);
        // The drive is unmounted: its mount point is an empty folder.
        fs::remove_file(&a).unwrap();
        let offline = scan();
        assert_eq!(offline.tracks.len(), 1);
        assert!(offline.errors.iter().any(|e| e.contains("mounted")));
    }
    #[test]
    fn dangling_root_link_keeps_the_known_tracks() {
        let f = Fixture::new();
        let drive = f.0.join("drive");
        fs::create_dir(&drive).unwrap();
        flac(&drive.join("01.flac"), "One", 1, 1);
        let link = f.0.join("Music");
        std::os::unix::fs::symlink(&drive, &link).unwrap();
        let db = f.0.join("index.sqlite");
        let scan = || {
            library::scan(
                std::slice::from_ref(&link),
                &db,
                &AtomicBool::new(false),
                None,
            )
            .unwrap()
        };
        assert_eq!(scan().tracks.len(), 1);
        // The drive behind the link is gone: the link dangles.
        fs::rename(&drive, f.0.join("elsewhere")).unwrap();
        let offline = scan();
        assert_eq!(offline.tracks.len(), 1);
        assert!(offline.errors.iter().any(|e| e.contains("unavailable")));
    }
    #[test]
    fn latin1_playlist_is_read() {
        let f = Fixture::new();
        let a = f.0.join("Zoë Dür.flac");
        flac(&a, "One", 1, 1);
        let list = f.0.join("old.m3u");
        fs::write(&list, b"#EXTM3U\r\nZo\xeb D\xfcr.flac\r\n").unwrap();
        let read = crate::playlist::read(&list).unwrap();
        assert_eq!(read, vec![a.canonicalize().unwrap()]);
    }
    #[test]
    fn local_playlist_controls_album_order_and_ignores_remote() {
        let f = Fixture::new();
        let a = f.0.join("Ünïcode 01.flac");
        let b = f.0.join("02.flac");
        flac(&a, "One", 1, 1);
        flac(&b, "Two", 2, 1);
        fs::write(
            f.0.join("playlist.m3u8"),
            "\u{feff}#EXTM3U\n02.flac\nhttps://example.invalid/remote.flac\nÜnïcode 01.flac\n",
        )
        .unwrap();
        let ts = vec![
            crate::flac::read(&a, false).unwrap().track,
            crate::flac::read(&b, false).unwrap().track,
        ];
        let album = library::albums(&ts);
        assert_eq!(album[0].tracks[0].title, "Two");
        assert_eq!(album[0].tracks[1].title, "One");
        assert_eq!(
            crate::playlist::read(&f.0.join("playlist.m3u8"))
                .unwrap()
                .len(),
            2
        );
    }
    #[test]
    fn playlist_roundtrip_keeps_unicode_and_special_characters() {
        let f = Fixture::new();
        let a = f.0.join("#01 ü & %.flac");
        flac(&a, "# One\nnot-a-path", 1, 1);
        let t = crate::flac::read(&a, false).unwrap().track;
        let p = f.0.join("out.m3u8");
        crate::playlist::write(&p, &[t]).unwrap();
        assert_eq!(crate::playlist::read(&p).unwrap(), vec![a]);
    }
    #[test]
    fn invalid_lrc_falls_back_to_plain_text() {
        let f = Fixture::new();
        let a = f.0.join("01.flac");
        flac(&a, "One", 1, 1);
        fs::write(a.with_extension("lrc"), "[01:00]Wrong time").unwrap();
        fs::write(a.with_extension("txt"), "The supplied text").unwrap();
        let l = lyrics::load(&a, 1.);
        assert!(l.lines.is_empty());
        assert_eq!(l.text, "The supplied text");
        assert!(l.warning.is_none());
    }
    #[test]
    fn cache_ignores_administrative_folders_and_symlinks() {
        let f = Fixture::new();
        let music = f.0.join("music");
        fs::create_dir(&music).unwrap();
        flac(&music.join("01.flac"), "One", 1, 1);
        for dir in ["_FLACker", "_backups", ".git", ".aws", ".agents", ".codex"] {
            fs::create_dir(music.join(dir)).unwrap();
            flac(&music.join(dir).join("hidden.flac"), "Hidden", 1, 1);
        }
        std::os::unix::fs::symlink(&music, music.join("loop")).unwrap();
        let r = library::scan(
            &[music],
            &f.0.join("index.sqlite"),
            &AtomicBool::new(false),
            None,
        )
        .unwrap();
        assert_eq!(r.tracks.len(), 1);
        assert!(r.errors.is_empty());
    }
    #[test]
    fn cancelled_scan_keeps_database_reusable() {
        let f = Fixture::new();
        flac(&f.0.join("01.flac"), "One", 1, 1);
        let db = f.0.join("index.sqlite");
        let r = library::scan(
            std::slice::from_ref(&f.0),
            &db,
            &AtomicBool::new(true),
            None,
        )
        .unwrap();
        assert!(r.cancelled);
        let r = library::scan(
            std::slice::from_ref(&f.0),
            &db,
            &AtomicBool::new(false),
            None,
        )
        .unwrap();
        assert_eq!(r.tracks.len(), 1);
    }
    #[test]
    fn different_release_ids_do_not_merge() {
        let a = Track {
            path: "/music/edition/01.flac".into(),
            album: "Same title".into(),
            release_id: "release-a".into(),
            ..Default::default()
        };
        let mut b = a.clone();
        b.path = "/music/edition/02.flac".into();
        b.release_id = "release-b".into();
        assert_eq!(library::albums(&[a, b]).len(), 2);
    }
    #[test]
    fn repeated_album_names_in_different_directories_remain_distinct() {
        let a = Track {
            path: "/music/original/01.flac".into(),
            album: "Same title".into(),
            ..Default::default()
        };
        let mut b = a.clone();
        b.path = "/music/remaster/01.flac".into();
        assert_eq!(library::albums(&[a, b]).len(), 2);
    }
}
