use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Track {
    pub metadata_version: u8,
    pub genres: Vec<String>,
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub date: String,
    pub original_date: String,
    pub release_id: String,
    pub disc: u32,
    pub number: u32,
    pub disc_total: u32,
    pub duration: f64,
    pub sample_rate: u32,
    pub bits: u8,
    pub channels: u8,
    pub size: u64,
    pub modified: u64,
    pub gain_track: Option<f64>,
    pub gain_album: Option<f64>,
    pub peak_track: Option<f64>,
    pub peak_album: Option<f64>,
    pub has_cover: bool,
    pub has_lyrics: bool,
}
impl Track {
    pub fn album_dir(&self) -> PathBuf {
        let p = self.path.parent().unwrap_or(Path::new("/"));
        let name = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        let disc_dir = name
            .strip_prefix("cd")
            .or_else(|| name.strip_prefix("disc"));
        if disc_dir.is_some_and(|n| n.trim().parse::<u32>().is_ok()) {
            p.parent().unwrap_or(p).into()
        } else {
            p.into()
        }
    }
    pub fn album_key(&self) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{}",
            self.album_dir().display(),
            self.album,
            self.release_id
        )
    }
    pub fn quality(&self) -> String {
        format!(
            "FLAC  ·  {} bit / {} kHz",
            self.bits,
            self.sample_rate as f64 / 1000.0
        )
    }
    pub fn high_res(&self) -> bool {
        self.bits > 16 || self.sample_rate > 48000
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Album {
    pub key: String,
    pub title: String,
    pub artist: String,
    pub year: String,
    pub tracks: Vec<Track>,
}
impl Album {
    pub fn duration(&self) -> f64 {
        self.tracks.iter().map(|t| t.duration).sum()
    }
}
pub fn time(seconds: f64) -> String {
    let s = seconds.max(0.) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Queue {
    pub tracks: Vec<Track>,
    pub index: Option<usize>,
    pub repeat: Repeat,
    pub shuffle: bool,
    #[serde(default)]
    pub history: Vec<usize>,
    #[serde(default)]
    pub shuffle_remaining: Vec<usize>,
}
impl Queue {
    pub fn current(&self) -> Option<&Track> {
        self.index.and_then(|i| self.tracks.get(i))
    }
    pub fn replace(&mut self, tracks: Vec<Track>, index: usize) {
        self.tracks = tracks;
        self.index =
            (!self.tracks.is_empty()).then_some(index.min(self.tracks.len().saturating_sub(1)));
        self.history.clear();
        self.shuffle_remaining.clear();
    }
    pub fn next_index(&mut self, automatic: bool) -> Option<usize> {
        let current = self.index?;
        if automatic && self.repeat == Repeat::One {
            return Some(current);
        }
        if self.shuffle {
            use rand::seq::SliceRandom;
            if self.shuffle_remaining.is_empty() {
                // A cycle ends when every track has played once. History repeats entries
                // (Repeat One, jumps), so it is compared as a set, never by its length.
                let played: std::collections::HashSet<usize> =
                    self.history.iter().copied().collect();
                let mut pool: Vec<usize> = (0..self.tracks.len())
                    .filter(|i| *i != current && !played.contains(i))
                    .collect();
                if pool.is_empty() {
                    if self.repeat != Repeat::All {
                        return None;
                    }
                    self.history.clear();
                    pool = (0..self.tracks.len()).filter(|i| *i != current).collect();
                }
                pool.shuffle(&mut rand::rng());
                self.shuffle_remaining = pool;
            }
            return self
                .shuffle_remaining
                .pop()
                .or_else(|| (self.repeat == Repeat::All).then_some(current));
        }
        if current + 1 < self.tracks.len() {
            Some(current + 1)
        } else if self.repeat == Repeat::All {
            Some(0)
        } else {
            None
        }
    }
    pub fn advance_to(&mut self, i: usize) {
        if let Some(old) = self.index {
            self.history.push(old);
            if self.history.len() > 100_000 {
                self.history.drain(..50_000);
            }
        }
        self.index = Some(i);
        // However a track was reached (gapless transition, Next, a jump), it is no longer
        // waiting in this shuffle cycle; otherwise Next could pick the track already playing.
        self.shuffle_remaining.retain(|&x| x != i);
    }
    pub fn previous(&mut self) -> Option<usize> {
        self.history
            .pop()
            .or_else(|| self.index.and_then(|i| i.checked_sub(1)))
    }
    #[cfg(test)]
    pub fn move_item(&mut self, from: usize, to: usize) {
        if from >= self.tracks.len() || to >= self.tracks.len() || from == to {
            return;
        }
        let item = self.tracks.remove(from);
        self.tracks.insert(to, item);
        self.index = self.index.map(|current| {
            if current == from {
                to
            } else if from < current && to >= current {
                current - 1
            } else if from > current && to <= current {
                current + 1
            } else {
                current
            }
        });
        self.history.clear();
        self.shuffle_remaining.clear();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn durations_show_hours_when_needed() {
        assert_eq!(time(0.), "0:00");
        assert_eq!(time(342.4), "5:42");
        assert_eq!(time(3599.), "59:59");
        assert_eq!(time(5846.), "1:37:26");
    }
    fn queue() -> Queue {
        let mut q = Queue::default();
        q.replace(vec![Track::default(); 3], 0);
        q
    }
    #[test]
    fn moving_duplicate_paths_keeps_the_playing_occurrence() {
        let mut q = queue();
        q.index = Some(1);
        q.move_item(0, 2);
        assert_eq!(q.index, Some(0));
        q.move_item(0, 2);
        assert_eq!(q.index, Some(2));
        q.move_item(0, 1);
        assert_eq!(q.index, Some(2));
        q.move_item(2, 1);
        assert_eq!(q.index, Some(1));
    }
    #[test]
    fn continuous_order_and_stop() {
        let mut q = queue();
        assert_eq!(q.next_index(true), Some(1));
        q.advance_to(1);
        assert_eq!(q.next_index(true), Some(2));
        q.advance_to(2);
        assert_eq!(q.next_index(true), None);
        q.repeat = Repeat::All;
        assert_eq!(q.next_index(true), Some(0));
    }
    #[test]
    fn repeat_one_is_only_automatic() {
        let mut q = queue();
        q.repeat = Repeat::One;
        assert_eq!(q.next_index(true), Some(0));
        assert_eq!(q.next_index(false), Some(1));
    }
    #[test]
    fn shuffle_visits_once() {
        let mut q = queue();
        q.shuffle = true;
        let mut seen = vec![0];
        while let Some(i) = q.next_index(true) {
            assert!(!seen.contains(&i));
            seen.push(i);
            q.advance_to(i);
        }
        assert_eq!(seen.len(), 3);
        assert!(q.previous().is_some());
    }
}
