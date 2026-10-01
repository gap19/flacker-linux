use gtk::gio::{self, prelude::*};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
pub fn read(path: &Path) -> io::Result<Vec<PathBuf>> {
    if path.metadata()?.len() > 8 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Playlist too large",
        ));
    }
    // .m3u8 is UTF-8; older .m3u files are often Latin-1, which maps byte for byte.
    let content = match String::from_utf8(fs::read(path)?) {
        Ok(text) => text,
        Err(e) => e.into_bytes().into_iter().map(char::from).collect(),
    };
    let base = path.parent().unwrap_or(Path::new("."));
    let mut result = Vec::new();
    for line in content.trim_start_matches('\u{feff}').lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let p = if line.starts_with("file:") {
            let Some(p) = gio::File::for_uri(line).path() else {
                continue;
            };
            p
        } else {
            if line.contains("://") {
                continue;
            }
            let p = PathBuf::from(line);
            if p.is_absolute() { p } else { base.join(p) }
        };
        if p.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("flac"))
        {
            result.push(p.canonicalize().unwrap_or(p));
        }
    }
    Ok(result)
}
pub fn write(path: &Path, tracks: &[crate::model::Track]) -> io::Result<()> {
    let mut body = String::from("#EXTM3U\n");
    for t in tracks {
        body.push_str(&format!(
            "#EXTINF:{},{} - {}\n{}\n",
            t.duration as u64,
            t.artist.replace(['\r', '\n'], " "),
            t.title.replace(['\r', '\n'], " "),
            gio::File::for_path(&t.path).uri()
        ));
    }
    crate::storage::atomic_write(path, body.as_bytes())
}
