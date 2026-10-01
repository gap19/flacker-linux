use crate::model::Queue;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub experience: crate::collection::Preferences,
    pub roots: Vec<PathBuf>,
    pub volume: f64,
    pub theme: String,
    pub replaygain: String,
    pub notifications: bool,
    pub prefer_embedded: bool,
    pub queue: Queue,
    pub position: f64,
    pub favorites: Vec<PathBuf>,
    pub width: i32,
    pub height: i32,
    #[serde(default)]
    pub maximized: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            experience: Default::default(),
            roots: vec![],
            volume: 0.8,
            theme: "dark".into(),
            replaygain: "off".into(),
            notifications: false,
            prefer_embedded: true,
            queue: Queue::default(),
            position: 0.,
            favorites: vec![],
            width: 1320,
            height: 880,
            maximized: false,
        }
    }
}
pub fn data_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("FLACKER_DATA_DIR") {
        return p.into();
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        });
    let dir = base.join("flacker");
    migrate_legacy(&base.join("astracks"), &dir);
    dir
}
/// Versions before 1.0 were called Astracks and kept settings, favorites, playlists and
/// the library index under "astracks". Move that folder once, so nothing is lost.
fn migrate_legacy(old: &Path, new: &Path) {
    if new.exists() || !old.is_dir() {
        return;
    }
    if let Err(e) = fs::rename(old, new) {
        eprintln!(
            "FLACker: could not move {} to {}: {e}",
            old.display(),
            new.display()
        );
    }
}
pub fn cache_dir() -> PathBuf {
    if std::env::var_os("FLACKER_DATA_DIR").is_some() {
        return data_dir().join("cache");
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")
        });
    let dir = base.join("flacker");
    migrate_legacy(&base.join("astracks"), &dir);
    dir
}
/// Replace a regenerable cache file (thumbnails, exported covers) by write and rename, without
/// the disk syncs of [`atomic_write`]: losing one after a crash only means drawing it again.
pub fn cache_write(path: &Path, body: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".flacker-{}-{}.tmp",
        std::process::id(),
        rand::random::<u64>()
    ));
    let result = fs::write(&temp, body).and_then(|_| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
pub fn atomic_write(path: &Path, body: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".flacker-{}-{}.tmp",
        std::process::id(),
        rand::random::<u64>()
    ));
    let result = (|| {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        f.write_all(body)?;
        f.sync_all()?;
        fs::rename(&temp, path)?;
        fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
/// The bytes last written (or read) for settings.json, so unchanged settings are not rewritten.
static LAST_SAVED: std::sync::Mutex<Option<Vec<u8>>> = std::sync::Mutex::new(None);
/// Set when settings.json exists but could not be read: saving defaults over it would erase
/// the user's folders and favorites, so writes are refused for this session.
static UNREADABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
impl Settings {
    pub fn load() -> Self {
        let path = data_dir().join("settings.json");
        match fs::read(&path) {
            Ok(b) => match serde_json::from_slice(&b) {
                Ok(s) => {
                    *LAST_SAVED.lock().unwrap() = serde_json::to_vec(&s).ok();
                    s
                }
                Err(e) => {
                    eprintln!("Invalid settings: {e}. A copy was kept.");
                    let _ = fs::copy(&path, path.with_extension("json.invalid"));
                    Self::default()
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                eprintln!(
                    "Could not read {}: {e}. It will not be overwritten.",
                    path.display()
                );
                UNREADABLE.store(true, std::sync::atomic::Ordering::Relaxed);
                Self::default()
            }
        }
    }
    /// Writes only when something changed since the last save (the periodic save is then free).
    pub fn save(&self) -> io::Result<()> {
        if UNREADABLE.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(io::Error::other(
                "settings.json could not be read at start; not overwriting it",
            ));
        }
        let bytes = serde_json::to_vec(self)?;
        let mut last = LAST_SAVED.lock().unwrap();
        if last.as_deref() == Some(bytes.as_slice()) {
            return Ok(());
        }
        atomic_write(&data_dir().join("settings.json"), &bytes)?;
        *last = Some(bytes);
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::migrate_legacy;
    use std::fs;

    #[test]
    fn legacy_data_moves_once() {
        let root = std::env::temp_dir().join(format!("flacker-migrate-{}", std::process::id()));
        let old = root.join("astracks");
        let new = root.join("flacker");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("settings.json"), b"{}").unwrap();
        migrate_legacy(&old, &new);
        assert!(!old.exists());
        assert_eq!(fs::read(new.join("settings.json")).unwrap(), b"{}");
        // An existing new folder is never replaced by an old one.
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("settings.json"), b"old").unwrap();
        migrate_legacy(&old, &new);
        assert_eq!(fs::read(new.join("settings.json")).unwrap(), b"{}");
        fs::remove_dir_all(&root).unwrap();
    }
}
