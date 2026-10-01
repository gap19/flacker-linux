use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};
const MAX_TEXT: u64 = 2 * 1024 * 1024;
#[derive(Clone, Debug, Default)]
pub struct Lyrics {
    pub text: String,
    pub lines: Vec<(f64, String)>,
    pub source: String,
    pub warning: Option<String>,
}
fn instrumental(text: &str) -> bool {
    matches!(
        text.trim().to_lowercase().as_str(),
        "[instrumental]" | "instrumental" | "(instrumental)" | "[instrumental track]"
    )
}
pub fn parse_lrc(text: &str, duration: f64) -> Lyrics {
    let mut out = Lyrics::default();
    let mut plain = Vec::new();
    let mut offset = 0.;
    let mut invalid = false;
    let mut last = -1.;
    for line in text.lines() {
        let mut rest = line.trim().trim_start_matches('\u{feff}');
        let mut stamps = Vec::new();
        while rest.starts_with('[') {
            let Some(end) = rest.find(']') else {
                invalid = true;
                break;
            };
            let token = &rest[1..end];
            let is_time = token
                .split_once(':')
                .is_some_and(|(m, _)| !m.is_empty() && m.chars().all(|c| c.is_ascii_digit()));
            let is_metadata = token.split_once(':').is_some_and(|(key, _)| {
                matches!(
                    key.to_ascii_lowercase().as_str(),
                    "ar" | "al" | "ti" | "au" | "by" | "re" | "ve" | "length" | "offset"
                )
            });
            if !is_time && !is_metadata {
                break;
            }
            rest = rest[end + 1..].trim();
            if let Some(v) = token.strip_prefix("offset:") {
                match v.parse::<f64>() {
                    Ok(v) if v.is_finite() => offset = v / 1000.,
                    _ => invalid = true,
                };
                continue;
            }
            if let Some((m, s)) = token.split_once(':') {
                if let (Ok(m), Ok(s)) = (m.parse::<u32>(), s.parse::<f64>()) {
                    let t = m as f64 * 60. + s;
                    if !s.is_finite() || !(0.0..60.0).contains(&s) {
                        invalid = true
                    } else {
                        stamps.push(t);
                    }
                } else if token.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    invalid = true
                }
            }
        }
        if !rest.is_empty() {
            plain.push(strip_cue_markup(rest));
        } else if line.trim().is_empty() {
            plain.push(String::new());
        }
        for t in stamps {
            if t < last {
                invalid = true;
            }
            last = t;
            out.lines.push((t, rest.into()));
        }
    }
    for (t, _) in &mut out.lines {
        *t = (*t + offset).max(0.);
        if *t > duration + 2. {
            invalid = true
        }
    }
    out.text = plain.join("\n").trim().to_string();
    if out.text.trim().is_empty() {
        out.lines.clear();
    }
    if instrumental(&out.text) {
        out.text.clear();
        out.lines.clear();
        out.source = "Marked instrumental in the local file".into();
    }
    if invalid {
        out.lines.clear();
        out.warning = Some("Inconsistent LRC timestamps; showing the text without timing.".into());
    }
    out
}
// Reading a growing or incorrectly named file remains bounded.
fn read_text(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_TEXT + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_TEXT {
        return None;
    }
    decode_text(&bytes)
}
fn decode_text(bytes: &[u8]) -> Option<String> {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        if bytes.len() % 2 != 0 {
            return None;
        }
        let little = bytes[0] == 0xff;
        let units = bytes[2..].chunks_exact(2).map(|b| {
            if little {
                u16::from_le_bytes([b[0], b[1]])
            } else {
                u16::from_be_bytes([b[0], b[1]])
            }
        });
        return char::decode_utf16(units)
            .collect::<Result<String, _>>()
            .ok();
    }
    if bytes.contains(&0) {
        return None;
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return Some(s.trim_start_matches('\u{feff}').to_string());
    }
    // Local legacy lyric files commonly use Windows-1252 (Latin-1 outside C1).
    const C1: [char; 32] = [
        '€', '�', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '�', 'Ž', '�', '�', '‘',
        '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '�', 'ž', 'Ÿ',
    ];
    Some(
        bytes
            .iter()
            .map(|&b| {
                if (0x80..=0x9f).contains(&b) {
                    C1[(b - 0x80) as usize]
                } else {
                    char::from(b)
                }
            })
            .collect(),
    )
}
fn strip_cue_markup(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('>') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let tag = &rest[start + 1..start + end];
        let name = tag
            .trim_start_matches('/')
            .split([' ', '.'])
            .next()
            .unwrap_or("");
        let timestamp = tag.chars().next().is_some_and(|c| c.is_ascii_digit()) && tag.contains(':');
        if !timestamp && !matches!(name, "b" | "i" | "u" | "c" | "v" | "lang" | "ruby" | "rt") {
            out.push_str(&rest[start..=start + end]);
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}
/// An SRT/VTT timing line such as "00:00:01,000 --> 00:00:03,000"; plain lyrics may use
/// arrows too and must not be read as subtitles.
fn cue_timing(line: &str) -> bool {
    let Some((left, right)) = line.split_once("-->") else {
        return false;
    };
    let stamp = |s: &str| {
        let s = s.split_whitespace().next().unwrap_or("");
        s.contains(':')
            && !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_digit() || matches!(c, ':' | '.' | ','))
    };
    stamp(left.trim()) && stamp(right.trim())
}
fn subtitle_text(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut paragraphs = Vec::new();
    for block in normalized.split("\n\n") {
        let lines: Vec<_> = block.lines().collect();
        let Some(timing) = lines.iter().position(|l| cue_timing(l)) else {
            continue;
        };
        if lines.first().is_some_and(|l| {
            l.starts_with("NOTE") || l.starts_with("STYLE") || l.starts_with("REGION")
        }) {
            continue;
        }
        let body = lines[timing + 1..]
            .iter()
            .map(|l| strip_cue_markup(l))
            .collect::<Vec<_>>()
            .join("\n");
        if !body.trim().is_empty() {
            paragraphs.push(body);
        }
    }
    paragraphs.join("\n\n")
}
fn plain_text(text: &str, duration: f64) -> String {
    let text = text
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    if text.lines().any(cue_timing) {
        subtitle_text(&text)
    } else if text.lines().any(|l| l.trim_start().starts_with('[')) {
        parse_lrc(&text, duration).text
    } else {
        text.trim().to_string()
    }
}
fn candidates(path: &Path) -> Vec<PathBuf> {
    let parent = path.parent().unwrap_or(Path::new("."));
    let stem = path.file_stem().unwrap_or_default();
    let mut dirs = vec![parent.to_path_buf()];
    if let Ok(rd) = fs::read_dir(parent) {
        let mut extra: Vec<_> = rd
            .flatten()
            .filter(|e| {
                e.file_type().is_ok_and(|t| t.is_dir())
                    && matches!(
                        e.file_name().to_string_lossy().to_lowercase().as_str(),
                        "lyrics"
                    )
            })
            .map(|e| e.path())
            .collect();
        extra.sort();
        dirs.extend(extra);
    }
    let mut choices = Vec::new();
    for (folder_rank, dir) in dirs.iter().enumerate() {
        let Ok(rd) = fs::read_dir(dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.file_stem() != Some(stem) || !entry.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            let ext = p
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_lowercase();
            if let Some(rank) = ["txt", "lyrics", "text", "lrc", "srt", "vtt"]
                .iter()
                .position(|v| *v == ext)
            {
                choices.push((rank, folder_rank, p));
            }
        }
    }
    choices.sort();
    choices.into_iter().map(|(_, _, p)| p).collect()
}
pub fn load(path: &Path, duration: f64) -> Lyrics {
    let mut marker = None;
    if let Ok(text) = crate::flac::read_lyrics(path) {
        let text = plain_text(&text, duration);
        if instrumental(&text) {
            marker = Some("Marked instrumental in the local file".to_string());
        } else if !text.trim().is_empty() {
            return Lyrics {
                text,
                source: "Lyrics embedded in the FLAC".into(),
                ..Default::default()
            };
        }
    }
    for p in candidates(path) {
        let Some(text) = read_text(&p) else {
            continue;
        };
        let text = plain_text(&text, duration);
        if instrumental(&text) {
            marker = Some("Marked instrumental in the local file".to_string());
            continue;
        }
        if text.trim().is_empty() {
            continue;
        }
        return Lyrics {
            text,
            source: p.file_name().unwrap_or_default().to_string_lossy().into(),
            ..Default::default()
        };
    }
    Lyrics {
        source: marker.unwrap_or_else(|| "No local lyrics for this track".into()),
        ..Default::default()
    }
}

/// Bounded worker. A slow disk cannot block transport buttons or publish old lyrics.
pub struct Loader {
    requests: std::sync::mpsc::SyncSender<(u64, PathBuf, f64)>,
    results: std::sync::mpsc::Receiver<(u64, Lyrics)>,
    pending: Option<(u64, PathBuf, f64)>,
    generation: u64,
    key: Option<(PathBuf, u64)>,
    /// A request whose result has not arrived yet.
    awaiting: bool,
}
impl Default for Loader {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel::<(u64, PathBuf, f64)>(1);
        let (result, results) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            while let Ok(mut job) = rx.recv() {
                while let Ok(new) = rx.try_recv() {
                    job = new;
                }
                let text = load(&job.1, job.2);
                if result.send((job.0, text)).is_err() {
                    break;
                }
            }
        });
        Self {
            requests: tx,
            results,
            pending: None,
            generation: 0,
            key: None,
            awaiting: false,
        }
    }
}
impl Loader {
    pub fn invalidate(&mut self) {
        self.key = None;
        self.pending = None;
        self.awaiting = false;
        self.generation = self.generation.wrapping_add(1);
    }
    pub fn request(&mut self, track: &crate::model::Track) -> bool {
        let key = (track.path.clone(), track.modified);
        if self.key.as_ref() == Some(&key) {
            return false;
        }
        self.key = Some(key);
        self.generation = self.generation.wrapping_add(1);
        self.pending = Some((self.generation, track.path.clone(), track.duration));
        self.awaiting = true;
        true
    }
    /// Lyrics are being read: the caller keeps polling quickly until they arrive.
    pub fn busy(&self) -> bool {
        self.awaiting
    }
    pub fn poll(&mut self) -> Option<Lyrics> {
        let mut current = None;
        while let Ok((id, lyrics)) = self.results.try_recv() {
            if id == self.generation {
                current = Some(lyrics);
                self.awaiting = false;
            }
        }
        if let Some(job) = self.pending.take() {
            if let Err(std::sync::mpsc::TrySendError::Full(job)) = self.requests.try_send(job) {
                self.pending = Some(job);
            }
        }
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn static_text_keeps_headings_and_removes_word_timing() {
        assert_eq!(
            plain_text(
                "[ar:Artist]\n[Verse 1]\n[00:01.00]<00:01.01>Hello <00:02.00>world",
                10.
            ),
            "[Verse 1]\nHello world"
        );
    }
    #[test]
    fn subtitles_preserve_words_without_identifiers_or_timing() {
        let s = "WEBVTT\n\nNOTE ignore this\n\nverse-1\n00:00:01.000 --> 00:00:03.000 align:start\n<v Singer><b>Hello</b> &amp; you\n\n2\n00:00:04,000 --> 00:00:05,000\nAnother verse";
        assert_eq!(plain_text(s, 10.), "Hello & you\n\nAnother verse");
    }
    #[test]
    fn unicode_and_legacy_local_text() {
        assert_eq!(decode_text(&[0xff, 0xfe, 0xfc, 0]).as_deref(), Some("ü"));
        assert_eq!(decode_text(&[0xfe, 0xff, 0, 0xfc]).as_deref(), Some("ü"));
        assert_eq!(decode_text(b"Se\xf1al \xfc").as_deref(), Some("Señal ü"));
        assert!(decode_text(&[0, 1, 2]).is_none());
    }
    #[test]
    fn parses_fraction_and_offset() {
        let l = parse_lrc("[offset:-100]\n[00:01.23]One\n[00:02.345]Two", 10.);
        assert!((l.lines[0].0 - 1.13).abs() < 0.0001);
        assert_eq!(l.lines[1].1, "Two");
    }
    #[test]
    fn invalid_timing_retains_words() {
        let l = parse_lrc("[00:70]One\n[00:02]Two", 10.);
        assert!(l.lines.is_empty());
        assert_eq!(l.text, "One\nTwo");
        assert!(l.warning.is_some());
    }
    #[test]
    fn excludes_instrumental() {
        assert!(parse_lrc("[00:01]Instrumental", 10.).text.is_empty());
    }
    #[test]
    fn descending_times_are_not_repaired() {
        assert!(parse_lrc("[00:02]Two\n[00:01]One", 10.).lines.is_empty());
    }
}
