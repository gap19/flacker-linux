//! Bounded FLAC header reader. Never opens a library file for writing or reads audio frames.
use crate::model::Track;
use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
    time::UNIX_EPOCH,
};
pub struct Metadata {
    pub track: Track,
    pub lyrics: String,
    pub picture: Option<Vec<u8>>,
    pub tags: HashMap<String, Vec<String>>,
}
fn invalid(s: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s)
}
fn le32(b: &[u8], p: &mut usize) -> io::Result<usize> {
    let a = b
        .get(*p..*p + 4)
        .ok_or_else(|| invalid("FLAC: truncated field"))?;
    *p += 4;
    Ok(u32::from_le_bytes(a.try_into().unwrap()) as usize)
}
fn be32(b: &[u8], p: &mut usize) -> io::Result<usize> {
    let a = b
        .get(*p..*p + 4)
        .ok_or_else(|| invalid("FLAC: truncated picture"))?;
    *p += 4;
    Ok(u32::from_be_bytes(a.try_into().unwrap()) as usize)
}
fn field<'a>(b: &'a [u8], p: &mut usize, n: usize) -> io::Result<&'a [u8]> {
    let end = p
        .checked_add(n)
        .ok_or_else(|| invalid("FLAC: invalid size"))?;
    let v = b
        .get(*p..end)
        .ok_or_else(|| invalid("FLAC: truncated block"))?;
    *p = end;
    Ok(v)
}
pub fn read(path: &Path, details: bool) -> io::Result<Metadata> {
    read_options(path, details, details)
}
pub fn read_lyrics(path: &Path) -> io::Result<String> {
    read_options(path, true, false).and_then(|m| {
        if m.lyrics.len() > 2 * 1024 * 1024 {
            Err(invalid("Embedded text exceeds the read limit"))
        } else {
            Ok(m.lyrics)
        }
    })
}
/// Bumped when the index must read files again (3: non-front covers, ID3-prefixed files).
pub const METADATA_VERSION: u8 = 3;
/// Which embedded picture shows the album: the front cover, else an untyped picture, else
/// any other picture except file icons, with the back cover last. `u8::MAX` is never used.
fn cover_rank(kind: u32) -> u8 {
    match kind {
        3 => 0,
        0 => 1,
        1 | 2 => u8::MAX,
        4 => 3,
        _ => 2,
    }
}
pub(crate) fn read_options(path: &Path, details: bool, cover: bool) -> io::Result<Metadata> {
    let stat = path.metadata()?;
    let mut file = File::open(path)?;
    let mut magic = [0; 4];
    file.read_exact(&mut magic)?;
    // Some taggers put an ID3v2 tag before the FLAC stream; GStreamer plays such files, so
    // the index reads past it too.
    let mut start = 0u64;
    if &magic[..3] == b"ID3" {
        let mut rest = [0; 6];
        file.read_exact(&mut rest)?;
        let size = rest[2..6]
            .iter()
            .fold(0u64, |n, b| (n << 7) | (*b & 0x7f) as u64);
        let footer = if rest[1] & 0x10 != 0 { 10 } else { 0 };
        start = 10 + size + footer;
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut magic)?;
    }
    if &magic != b"fLaC" {
        return Err(invalid(
            "FLAC header missing; file incomplete or unsupported",
        ));
    }
    let mut track = Track {
        path: path.to_owned(),
        metadata_version: METADATA_VERSION,
        size: stat.len(),
        modified: stat
            .modified()?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .min(u64::MAX as u128) as u64,
        ..Default::default()
    };
    let mut tags: HashMap<String, Vec<String>> = HashMap::new();
    let mut picture = None;
    let mut picture_rank = u8::MAX;
    let mut total = start + 4;
    let mut first = true;
    loop {
        let mut h = [0; 4];
        file.read_exact(&mut h)?;
        let kind = h[0] & 127;
        let last = h[0] & 128 != 0;
        let len = ((h[1] as usize) << 16) | ((h[2] as usize) << 8) | (h[3] as usize);
        total += 4 + len as u64;
        if total > 64 * 1024 * 1024 || total > stat.len() {
            return Err(invalid("FLAC: metadata limit reached or file incomplete"));
        }
        if first && (kind != 0 || len != 34) {
            return Err(invalid("FLAC: invalid STREAMINFO"));
        }
        first = false;
        if kind == 0 || kind == 4 || (kind == 6 && cover) {
            let mut b = vec![0; len];
            file.read_exact(&mut b)?;
            match kind {
                0 => {
                    if b.len() != 34 {
                        return Err(invalid("FLAC: truncated STREAMINFO"));
                    }
                    let v = u64::from_be_bytes(b[10..18].try_into().unwrap());
                    track.sample_rate = (v >> 44) as u32;
                    track.channels = (((v >> 41) & 7) + 1) as u8;
                    track.bits = (((v >> 36) & 31) + 1) as u8;
                    let samples = v & 0xfffffffff;
                    if track.sample_rate == 0 || samples == 0 {
                        return Err(invalid("FLAC has no finished samples yet"));
                    }
                    track.duration = samples as f64 / track.sample_rate as f64;
                }
                4 => {
                    let mut p = 0;
                    let n = le32(&b, &mut p)?;
                    field(&b, &mut p, n)?;
                    let count = le32(&b, &mut p)?;
                    if count > 100_000 {
                        return Err(invalid("FLAC: too many comments"));
                    }
                    for _ in 0..count {
                        let n = le32(&b, &mut p)?;
                        let value = String::from_utf8_lossy(field(&b, &mut p, n)?);
                        if let Some((k, v)) = value.split_once('=') {
                            let k = k.to_ascii_uppercase();
                            if matches!(
                                k.as_str(),
                                "LYRICS"
                                    | "UNSYNCEDLYRICS"
                                    | "USER_UNSYNCED_LYRICS"
                                    | "UNSYNCED LYRICS"
                                    | "SYNCEDLYRICS"
                                    | "SYNCHRONIZEDLYRICS"
                            ) {
                                if !v.trim().is_empty() {
                                    track.has_lyrics = true;
                                }
                                if !details {
                                    continue;
                                }
                            }
                            tags.entry(k).or_default().push(v.to_string());
                        }
                    }
                }
                6 => {
                    let mut p = 0;
                    let typ = be32(&b, &mut p)?;
                    let n = be32(&b, &mut p)?;
                    let mime = field(&b, &mut p, n)?;
                    let linked = mime == b"-->";
                    let n = be32(&b, &mut p)?;
                    field(&b, &mut p, n)?;
                    let width = be32(&b, &mut p)?;
                    let height = be32(&b, &mut p)?;
                    field(&b, &mut p, 8)?;
                    let n = be32(&b, &mut p)?;
                    let data = field(&b, &mut p, n)?;
                    let rank = cover_rank(typ as u32);
                    if rank < picture_rank && !linked {
                        track.has_cover = true;
                        if n <= 12 * 1024 * 1024 && width <= 12000 && height <= 12000 {
                            picture = Some(data.to_vec());
                            picture_rank = rank;
                        }
                    }
                }
                _ => {}
            }
        } else {
            if kind == 6 && len >= 4 {
                let mut typ = [0; 4];
                file.read_exact(&mut typ)?;
                if cover_rank(u32::from_be_bytes(typ)) < u8::MAX {
                    track.has_cover = true;
                }
                file.seek(SeekFrom::Current(len as i64 - 4))?;
            } else {
                file.seek(SeekFrom::Current(len as i64))?;
            }
        }
        if last {
            break;
        }
    }
    let get = |names: &[&str]| -> String {
        names
            .iter()
            .find_map(|k| {
                tags.get(*k)
                    .and_then(|v| v.iter().find(|s| !s.trim().is_empty()))
                    .cloned()
            })
            .unwrap_or_default()
    };
    let number = |name: &str, default: u32| -> u32 {
        get(&[name])
            .split('/')
            .next()
            .unwrap_or("")
            .trim()
            .parse()
            .unwrap_or(default)
    };
    track.title = get(&["TITLE"]);
    if track.title.is_empty() {
        track.title = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into()
    }
    track.artist = get(&["ARTIST"]);
    track.album_artist = get(&["ALBUMARTIST", "ALBUM ARTIST", "ARTIST"]);
    track.album = get(&["ALBUM"]);
    if track.album.is_empty() {
        track.album = path
            .parent()
            .and_then(|p| p.file_name())
            .unwrap_or_default()
            .to_string_lossy()
            .into()
    }
    track.genres = tags.get("GENRE").cloned().unwrap_or_default();
    track.date = get(&["DATE"]);
    track.original_date = get(&["ORIGINALDATE", "ORIGINALYEAR"]);
    track.release_id = get(&["RELEASE_ID", "MUSICBRAINZ_ALBUMID"]);
    track.disc = number("DISCNUMBER", 1);
    track.number = number("TRACKNUMBER", 0);
    track.disc_total = number("DISCTOTAL", number("TOTALDISCS", 1));
    let float = |k: &str| -> Option<f64> {
        get(&[k])
            .split_whitespace()
            .next()?
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
    };
    track.gain_track = float("REPLAYGAIN_TRACK_GAIN");
    track.gain_album = float("REPLAYGAIN_ALBUM_GAIN");
    track.peak_track = float("REPLAYGAIN_TRACK_PEAK");
    track.peak_album = float("REPLAYGAIN_ALBUM_PEAK");
    Ok(Metadata {
        track,
        lyrics: get(&[
            "UNSYNCEDLYRICS",
            "LYRICS",
            "USER_UNSYNCED_LYRICS",
            "UNSYNCED LYRICS",
            "SYNCEDLYRICS",
            "SYNCHRONIZEDLYRICS",
        ]),
        picture,
        tags,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_truncated_comments() {
        assert!(le32(&[1, 2], &mut 0).is_err());
        assert!(field(&[0], &mut 0, usize::MAX).is_err());
    }
}
