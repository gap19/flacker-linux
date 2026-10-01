//! Pure collection queries. Filters never rewrite metadata or alter album order.
use crate::model::{Album, Track};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    rc::Rc,
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Filter {
    pub genre: String,
    pub year: String,
    pub original_year: bool,
    pub quality: u32,
    pub favorites: bool,
    pub sort: u32,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub filter: Filter,
    pub cover_size: u32,
    pub lyrics_size: u32,
    pub lyrics_width: u32,
    pub accent: bool,
    pub meter: bool,
    pub history: bool,
    pub output: String,
    pub clock: u32,
    pub gallery_pages: BTreeMap<PathBuf, String>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            filter: Filter::default(),
            cover_size: 1,
            lyrics_size: 19,
            lyrics_width: 64,
            accent: false,
            meter: false,
            history: false,
            output: String::new(),
            clock: 0,
            gallery_pages: BTreeMap::new(),
        }
    }
}
pub fn year(date: &str) -> &str {
    date.get(..4)
        .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
        .unwrap_or("")
}
impl Filter {
    pub fn accepts(&self, track: &Track, favorites: &HashSet<PathBuf>) -> bool {
        let date = if self.original_year {
            &track.original_date
        } else {
            &track.date
        };
        (self.genre.is_empty()
            || track
                .genres
                .iter()
                .any(|g| g.eq_ignore_ascii_case(&self.genre)))
            && (self.year.is_empty() || year(date) == self.year)
            && (self.quality == 0
                || if self.quality == 1 {
                    track.high_res()
                } else {
                    !track.high_res()
                })
            && (!self.favorites || favorites.contains(&track.path))
    }
    pub fn albums(
        &self,
        albums: &[Rc<Album>],
        favorites: &HashSet<PathBuf>,
        query: &str,
    ) -> Vec<Rc<Album>> {
        let query = query.to_lowercase();
        let mut selected: Vec<_> = albums
            .iter()
            .filter(|a| {
                let text = format!("{} {} {}", a.artist, a.title, a.year).to_lowercase();
                query.split_whitespace().all(|term| text.contains(term))
                    && a.tracks.iter().any(|t| self.accepts(t, favorites))
            })
            .cloned()
            .collect();
        selected.sort_by_cached_key(|a| {
            let t = a.tracks.first();
            let date = t
                .map(|t| {
                    if self.sort == 3 {
                        year(&t.original_date)
                    } else {
                        year(&t.date)
                    }
                })
                .unwrap_or("");
            let artist = a.artist.to_lowercase();
            let title = a.title.to_lowercase();
            match self.sort {
                1 => (title, artist, date.to_string(), a.key.clone()),
                2 | 3 => (
                    format!("{}{}", if date.is_empty() { "1" } else { "0" }, date),
                    artist,
                    title,
                    a.key.clone(),
                ),
                _ => (artist, date.to_string(), title, a.key.clone()),
            }
        });
        selected
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dates_are_not_invented_and_filters_keep_album_order() {
        let a = Track {
            date: "2002".into(),
            original_date: "1978-05".into(),
            genres: vec!["Jazz".into()],
            path: "one".into(),
            ..Default::default()
        };
        let b = Track {
            path: "two".into(),
            bits: 24,
            ..a.clone()
        };
        let album = Album {
            key: "a".into(),
            title: "A".into(),
            artist: "B".into(),
            year: "2002".into(),
            tracks: vec![a.clone(), b.clone(), a.clone()],
        };
        let f = Filter {
            year: "1978".into(),
            original_year: true,
            quality: 1,
            genre: "jazz".into(),
            ..Default::default()
        };
        assert!(!f.accepts(&a, &HashSet::new()));
        assert!(f.accepts(&b, &HashSet::new()));
        assert_eq!(
            f.albums(&[Rc::new(album)], &HashSet::new(), "A")[0].tracks,
            [a.clone(), b, a]
        );
        assert_eq!(year("19"), "");
        assert_eq!(year("unknown"), "");
    }
}
