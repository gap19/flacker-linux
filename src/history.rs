//! Optional, bounded local listening history, separate from the music collection.
use crate::{model::Track, storage};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub path: PathBuf,
    pub album: String,
    pub artist: String,
    pub album_key: String,
    pub at: u64,
}
#[derive(Default)]
pub struct History {
    pub entries: Vec<Entry>,
    dirty: bool,
}
impl History {
    pub fn load() -> Self {
        let path = storage::data_dir().join("history.json");
        let mut entries: Vec<Entry> = path
            .metadata()
            .ok()
            .filter(|m| m.len() <= 4 * 1024 * 1024)
            .and_then(|_| std::fs::read(path).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        entries.truncate(500);
        Self {
            entries,
            dirty: false,
        }
    }
    pub fn record(&mut self, t: &Track) {
        self.entries.insert(
            0,
            Entry {
                path: t.path.clone(),
                album: t.album.clone(),
                artist: t.artist.clone(),
                album_key: t.album_key(),
                at: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            },
        );
        self.entries.truncate(500);
        self.dirty = true;
    }
    pub fn save(&mut self) -> std::io::Result<()> {
        if self.dirty {
            storage::atomic_write(
                &storage::data_dir().join("history.json"),
                &serde_json::to_vec(&self.entries)?,
            )?;
            self.dirty = false;
        }
        Ok(())
    }
    pub fn clear(&mut self) -> std::io::Result<()> {
        self.entries.clear();
        self.dirty = true;
        self.save()
    }
}
