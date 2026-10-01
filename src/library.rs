use crate::wake::Sender;
use crate::{
    flac,
    model::{Album, Track},
    playlist,
};
use rusqlite::{Connection, params};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::UNIX_EPOCH,
};
#[derive(Debug)]
pub enum ScanEvent {
    Progress(usize),
    Done(ScanReport),
    Failed(String),
}
#[derive(Debug, Default)]
pub struct ScanReport {
    pub tracks: Vec<Track>,
    /// Albums built from `tracks` in the scan thread, so the interface never builds them.
    pub albums: Vec<Album>,
    pub errors: Vec<String>,
    pub cached: usize,
    pub parsed: usize,
    pub cancelled: bool,
}
/// Hidden folders, folders starting with "_" (tools, backups, work folders) and
/// common build or provenance folders are never indexed.
pub fn excluded(name: &str) -> bool {
    name.starts_with('.')
        || name.starts_with('_')
        || matches!(name, "Provenance" | "node_modules" | "target")
}
fn connection(db: &Path) -> Result<Connection, String> {
    if let Some(p) = db.parent() {
        fs::create_dir_all(p).map_err(|e| e.to_string())?
    }
    let c = Connection::open(db).map_err(|e| e.to_string())?;
    // A second process (CLI scan, validation) waits for the lock instead of failing.
    c.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; CREATE TABLE IF NOT EXISTS tracks(path TEXT PRIMARY KEY, root TEXT NOT NULL, size INTEGER NOT NULL, modified INTEGER NOT NULL, metadata TEXT NOT NULL); CREATE TABLE IF NOT EXISTS roots(configured TEXT PRIMARY KEY, resolved TEXT NOT NULL); PRAGMA user_version=1;").map_err(|e|e.to_string())?;
    Ok(c)
}
pub fn scan(
    roots: &[PathBuf],
    db: &Path,
    cancel: &AtomicBool,
    events: Option<&Sender<ScanEvent>>,
) -> Result<ScanReport, String> {
    let mut c = connection(db)?;
    let mut report = ScanReport::default();
    let mut seen_all = HashSet::new();
    for root in roots {
        let configured = root;
        let root = match root.canonicalize() {
            Ok(p) if p.is_dir() => p,
            _ => {
                // An unmounted drive or a lost share is often temporary: keep what was known.
                // The index is keyed by the resolved folder, which a dangling link no longer
                // reveals; the last resolution is remembered for that.
                report
                    .errors
                    .push(format!("Folder unavailable: {}", configured.display()));
                let resolved = c
                    .query_row(
                        "SELECT resolved FROM roots WHERE configured=?1",
                        [configured.to_string_lossy()],
                        |r| r.get::<_, String>(0),
                    )
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| configured.clone());
                for t in cached_tracks(&c, &resolved)? {
                    // Overlapping roots list a file once.
                    if seen_all.insert(t.path.clone()) {
                        report.tracks.push(t);
                    }
                }
                continue;
            }
        };
        c.execute(
            "INSERT OR REPLACE INTO roots(configured,resolved) VALUES(?1,?2)",
            [configured.to_string_lossy(), root.to_string_lossy()],
        )
        .map_err(|e| e.to_string())?;
        let mut cached = HashMap::new();
        {
            let mut s = c
                .prepare("SELECT path,size,modified,metadata FROM tracks WHERE root=?1")
                .map_err(|e| e.to_string())?;
            let rows = s
                .query_map([root.to_string_lossy()], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, u64>(1)?,
                        r.get::<_, u64>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })
                .map_err(|e| e.to_string())?;
            for row in rows {
                let (p, s, m, j) = row.map_err(|e| e.to_string())?;
                cached.insert(PathBuf::from(p), (s, m, j));
            }
        }
        let tx = c.transaction().map_err(|e| e.to_string())?;
        let mut seen = HashSet::new();
        let mut complete = true;
        for entry in walkdir::WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| e.depth() == 0 || !excluded(&e.file_name().to_string_lossy()))
        {
            if cancel.load(Ordering::Relaxed) {
                report.cancelled = true;
                complete = false;
                break;
            }
            let entry = match entry {
                Ok(v) => v,
                Err(e) => {
                    report.errors.push(e.to_string());
                    complete = false;
                    continue;
                }
            };
            if !entry.file_type().is_file()
                || !entry
                    .path()
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("flac"))
            {
                continue;
            }
            let path = entry.path().to_owned();
            // Settings, playlists and the index store paths as UTF-8; other names are reported
            // per file instead of failing the whole scan or every later save.
            if path.to_str().is_none() {
                report.errors.push(format!(
                    "Name is not valid UTF-8, skipped: {}",
                    path.display()
                ));
                continue;
            }
            seen.insert(path.clone());
            if !seen_all.insert(path.clone()) {
                continue;
            }
            let meta = match path.metadata() {
                Ok(m) => m,
                Err(e) => {
                    report.errors.push(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            let modified = meta
                .modified()
                .unwrap_or(UNIX_EPOCH)
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .min(u64::MAX as u128) as u64;
            if let Some((size, mtime, j)) = cached.get(&path) {
                if *size == meta.len() && *mtime == modified {
                    if let Ok(t) = serde_json::from_str::<Track>(j).and_then(|t| {
                        if t.metadata_version >= crate::flac::METADATA_VERSION {
                            Ok(t)
                        } else {
                            Err(<serde_json::Error as serde::de::Error>::custom("old cache"))
                        }
                    }) {
                        report.tracks.push(t);
                        report.cached += 1;
                        continue;
                    }
                }
            }
            if meta.len() < 42 {
                report
                    .errors
                    .push(format!("Waiting for a complete file: {}", path.display()));
                continue;
            }
            match flac::read(&path, false) {
                Ok(m) => {
                    // The file may be renamed or deleted while it is read.
                    let Ok(after) = path.metadata() else {
                        report
                            .errors
                            .push(format!("File changed during reading: {}", path.display()));
                        continue;
                    };
                    if after.len() != meta.len() || after.modified().ok() != meta.modified().ok() {
                        report
                            .errors
                            .push(format!("File still being written: {}", path.display()));
                        continue;
                    }
                    let t = m.track;
                    let json = match serde_json::to_string(&t) {
                        Ok(j) => j,
                        Err(e) => {
                            report.errors.push(format!("{}: {e}", path.display()));
                            continue;
                        }
                    };
                    tx.prepare_cached("INSERT INTO tracks(path,root,size,modified,metadata) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(path) DO UPDATE SET root=excluded.root,size=excluded.size,modified=excluded.modified,metadata=excluded.metadata")
                        .and_then(|mut st| st.execute(params![path.to_string_lossy(),root.to_string_lossy(),meta.len(),modified,json]))
                        .map_err(|e|e.to_string())?;
                    report.tracks.push(t);
                    report.parsed += 1;
                }
                Err(e) => report.errors.push(format!("{}: {e}", path.display())),
            }
            if report.tracks.len() % 50 == 0 {
                if let Some(s) = events {
                    let _ = s.send(ScanEvent::Progress(report.tracks.len()));
                }
            }
        }
        // A root with no entries at all is what an unmounted drive's mount point looks like;
        // pruning it would erase the whole cache. Any other content means files were removed.
        let root_empty = fs::read_dir(&root).is_ok_and(|mut d| d.next().is_none());
        if complete && root_empty && !cached.is_empty() {
            complete = false;
            report.errors.push(format!(
                "No FLAC files found in {}; is the drive mounted? The previous tracks were kept.",
                root.display()
            ));
            for (_, _, j) in cached.values() {
                if let Ok(t) = serde_json::from_str::<Track>(j) {
                    report.tracks.push(t);
                }
            }
        }
        if complete {
            for path in cached.keys().filter(|p| !seen.contains(*p)) {
                tx.prepare_cached("DELETE FROM tracks WHERE path=?1")
                    .and_then(|mut st| st.execute([path.to_string_lossy()]))
                    .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        if report.cancelled {
            break;
        }
    }
    report.tracks.sort_by(|a, b| {
        a.album_artist
            .to_lowercase()
            .cmp(&b.album_artist.to_lowercase())
            .then(a.date.cmp(&b.date))
            .then(a.album.cmp(&b.album))
            .then(a.disc.cmp(&b.disc))
            .then(a.number.cmp(&b.number))
            .then(a.path.cmp(&b.path))
    });
    Ok(report)
}
/// Tracks remembered for a root, used when the root cannot be read right now.
fn cached_tracks(c: &Connection, root: &Path) -> Result<Vec<Track>, String> {
    let mut s = c
        .prepare_cached("SELECT metadata FROM tracks WHERE root=?1")
        .map_err(|e| e.to_string())?;
    let rows = s
        .query_map([root.to_string_lossy()], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    Ok(rows
        .filter_map(Result::ok)
        .filter_map(|j| serde_json::from_str::<Track>(&j).ok())
        .collect())
}
pub fn start_scan(
    roots: Vec<PathBuf>,
    db: PathBuf,
    sender: Sender<ScanEvent>,
    cancel: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let e = match scan(&roots, &db, &cancel, Some(&sender)) {
            Ok(mut r) => {
                // Grouping reads every album's playlist: done here, off the interface thread.
                if !r.cancelled {
                    r.albums = albums(&r.tracks);
                }
                ScanEvent::Done(r)
            }
            Err(e) => ScanEvent::Failed(e),
        };
        let _ = sender.send(e);
    });
}
pub fn albums(tracks: &[Track]) -> Vec<Album> {
    let mut groups: BTreeMap<String, Vec<Track>> = BTreeMap::new();
    for t in tracks {
        groups.entry(t.album_key()).or_default().push(t.clone());
    }
    let mut albums = Vec::new();
    for (key, mut ts) in groups {
        // Disc folders (CD1, CD2) without DISCNUMBER tags must not interleave: the folder
        // orders them after the disc number and before the track number.
        ts.sort_by(|a, b| {
            (a.disc, a.path.parent(), a.number, &a.path).cmp(&(
                b.disc,
                b.path.parent(),
                b.number,
                &b.path,
            ))
        });
        let dir = ts[0].album_dir();
        let playlist = [dir.join("playlist.m3u8"), dir.join("playlist.m3u")]
            .into_iter()
            .find(|p| p.is_file());
        if let Some(p) = playlist {
            if let Ok(paths) = playlist::read(&p) {
                let mut order = HashMap::new();
                for (i, p) in paths.iter().enumerate() {
                    order.entry(p).or_insert(i);
                }
                ts.sort_by_key(|t| order.get(&t.path).copied().unwrap_or(usize::MAX));
            }
        }
        let first = &ts[0];
        albums.push(Album {
            key,
            title: first.album.clone(),
            artist: first.album_artist.clone(),
            year: first.date.chars().take(4).collect(),
            tracks: ts,
        });
    }
    albums.sort_by(|a, b| {
        a.artist
            .to_lowercase()
            .cmp(&b.artist.to_lowercase())
            .then(a.year.cmp(&b.year))
            .then(a.title.cmp(&b.title))
            .then(a.key.cmp(&b.key))
    });
    albums
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skips_private_and_project_folders() {
        for s in [".git", ".aws", ".codex", ".agents", "_FLACker", "_backups"] {
            assert!(excluded(s));
        }
        assert!(!excluded("Pink Floyd"));
    }
    #[test]
    fn sorts_discs_numerically() {
        let mut a = Track {
            album: "A".into(),
            path: "/missing/01.flac".into(),
            number: 10,
            disc: 1,
            ..Default::default()
        };
        let mut b = a.clone();
        b.number = 2;
        b.path = "/missing/02.flac".into();
        a.disc = 2;
        assert_eq!(albums(&[a, b])[0].tracks[0].number, 2);
    }
}
