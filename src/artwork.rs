//! One thumbnail worker; bounded jobs, weak subscribers and a texture LRU.
//! Small views share one thumbnail size, so switching between albums, artists,
//! the feature strip and the player reuses the same decoded texture. After each
//! library read the whole collection is warmed in the background, at lower
//! priority than visible requests, which also revalidates changed artwork.
use crate::{flac, model::Track, storage};
use gtk::{gdk, gdk_pixbuf, gio, glib, prelude::*};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::mpsc,
};

#[derive(Clone)]
enum Source {
    Track(Box<Track>, bool),
    File(PathBuf),
}
struct Job {
    key: String,
    source: Source,
    size: i32,
    warm: bool,
}
/// Edge of the shared thumbnail used by grids, cards, the feature strip and the dock.
pub const THUMB: i32 = 200;
/// Memory for decoded textures; warming stops earlier so visible requests keep room.
const BUDGET: usize = 160 * 1024 * 1024;
const WARM_BUDGET: usize = 96 * 1024 * 1024;
struct Pixels {
    bytes: glib::Bytes,
    width: i32,
    height: i32,
    stride: i32,
    alpha: bool,
    path: Option<PathBuf>,
}
struct Cached {
    key: String,
    texture: Option<gdk::Texture>,
    path: Option<PathBuf>,
    generation: u64,
}
pub struct ArtCache {
    textures: VecDeque<Cached>,
    bytes: usize,
    waiting: Vec<(glib::WeakRef<gtk::Picture>, String)>,
    queued: VecDeque<Job>,
    warming: VecDeque<Job>,
    generation: u64,
    inflight: HashSet<String>,
    jobs: mpsc::SyncSender<Job>,
    results: mpsc::Receiver<(String, bool, Option<Pixels>)>,
    export: Option<String>,
    pub revision: u64,
    /// Device pixels per logical pixel; covers are decoded at the size actually drawn.
    scale: i32,
}
impl Default for ArtCache {
    fn default() -> Self {
        let (jobs, rx) = mpsc::sync_channel::<Job>(2);
        let (tx, results) = mpsc::sync_channel(16);
        std::thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                let result = decode_thumb(&job);
                if tx.send((job.key, job.warm, result)).is_err() {
                    break;
                }
            }
        });
        Self {
            textures: VecDeque::new(),
            bytes: 0,
            waiting: Vec::new(),
            queued: VecDeque::new(),
            warming: VecDeque::new(),
            generation: 0,
            inflight: HashSet::new(),
            jobs,
            results,
            export: None,
            revision: 0,
            scale: 1,
        }
    }
}
impl ArtCache {
    fn key(track: &Track, embedded: bool, size: i32) -> String {
        format!(
            "{}:{}:{size}:{embedded}",
            track.path.display(),
            track.modified
        )
    }
    /// Follow the display scale (HiDPI). Returns true when it changed, so callers can redraw.
    pub fn set_scale(&mut self, scale: i32) -> bool {
        let scale = scale.clamp(1, 3);
        if self.scale == scale {
            return false;
        }
        self.scale = scale;
        // Covers decoded for the old scale stay usable until replaced; new requests use the
        // new size, and the warm-up decodes the shared thumbnail again.
        true
    }
    /// The decoded edge for a cover shown `size` logical pixels wide.
    fn pixels(&self, size: i32) -> i32 {
        (size.clamp(32, 720) * self.scale).min(1440)
    }
    pub fn request(&mut self, picture: &gtk::Picture, track: &Track, embedded: bool, size: i32) {
        let size = self.pixels(size);
        self.request_job(
            picture,
            Job {
                key: Self::key(track, embedded, size),
                source: Source::Track(Box::new(track.clone()), embedded),
                size,
                warm: false,
            },
        );
    }
    /// Decode every given cover at the shared thumbnail size in the background.
    /// Entries already refreshed in this generation are skipped; older ones are
    /// decoded again so artwork changed on disk replaces the cached texture.
    pub fn warm(&mut self, tracks: &[&Track], embedded: bool) {
        self.generation = self.generation.wrapping_add(1);
        self.warming.clear();
        let mut seen = HashSet::new();
        let size = self.pixels(THUMB);
        for track in tracks {
            let key = Self::key(track, embedded, size);
            if seen.insert(key.clone()) {
                self.warming.push_back(Job {
                    key,
                    source: Source::Track(Box::new((*track).clone()), embedded),
                    size,
                    warm: true,
                });
            }
        }
    }
    pub fn request_file(&mut self, picture: &gtk::Picture, path: &Path, size: i32) {
        let size = self.pixels(size);
        self.request_job(
            picture,
            Job {
                key: format!("file:{}:{size}", path.display()),
                source: Source::File(path.into()),
                size,
                warm: false,
            },
        );
    }
    pub fn cancel(&mut self, picture: &gtk::Picture) {
        self.waiting
            .retain(|(w, _)| w.upgrade().is_some_and(|p| p != *picture));
        picture.set_paintable(gdk::Paintable::NONE);
    }
    fn request_job(&mut self, picture: &gtk::Picture, job: Job) {
        // Keep the shown image until the new one is ready: no blank flash, and the
        // cover crossfade has an outgoing image to fade from.
        self.waiting
            .retain(|(w, _)| w.upgrade().is_some_and(|p| p != *picture));
        if let Some(i) = self.textures.iter().position(|t| t.key == job.key) {
            let item = self.textures.remove(i).unwrap();
            picture.set_paintable(item.texture.as_ref());
            self.textures.push_front(item);
        } else {
            self.waiting.push((picture.downgrade(), job.key.clone()));
            self.enqueue(job);
        }
    }
    fn enqueue(&mut self, job: Job) {
        self.waiting.retain(|(w, _)| w.upgrade().is_some());
        self.queued.retain(|j| {
            self.export.as_ref() == Some(&j.key) || self.waiting.iter().any(|(_, k)| *k == j.key)
        });
        if !self.inflight.contains(&job.key)
            && !self.queued.iter().any(|j| j.key == job.key)
            && self.queued.len() < 512
        {
            self.queued.push_back(job);
        }
    }
    pub fn export(&mut self, track: &Track, embedded: bool) {
        let key = Self::key(track, embedded, 320);
        if self.export.as_ref() != Some(&key) {
            self.revision = self.revision.wrapping_add(1);
        }
        self.export = Some(key.clone());
        if !self.textures.iter().any(|t| t.key == key) {
            self.enqueue(Job {
                key,
                source: Source::Track(Box::new(track.clone()), embedded),
                size: 320,
                warm: false,
            });
        }
    }
    pub fn exported(&self) -> Option<PathBuf> {
        self.export
            .as_ref()
            .and_then(|k| self.textures.iter().find(|c| &c.key == k))
            .and_then(|c| c.path.clone())
    }
    pub fn poll(&mut self) {
        while let Ok((key, warm, pixels)) = self.results.try_recv() {
            self.inflight.remove(&key);
            let mut cached = Cached {
                key,
                texture: None,
                path: None,
                generation: self.generation,
            };
            if let Some(p) = pixels {
                let pix = gdk_pixbuf::Pixbuf::from_bytes(
                    &p.bytes,
                    gdk_pixbuf::Colorspace::Rgb,
                    p.alpha,
                    8,
                    p.width,
                    p.height,
                    p.stride,
                );
                cached.texture = Some(gdk::Texture::for_pixbuf(&pix));
                cached.path = p.path;
                self.bytes += p.width as usize * p.height as usize * 4;
            }
            self.waiting.retain(|(w, k)| {
                let Some(p) = w.upgrade() else { return false };
                if *k == cached.key {
                    p.set_paintable(cached.texture.as_ref());
                    false
                } else {
                    true
                }
            });
            if self.export.as_ref() == Some(&cached.key) {
                self.revision = self.revision.wrapping_add(1);
            }
            // A revalidated entry keeps its place in the LRU; a new warm entry
            // is the first candidate for eviction, a visible one the last.
            if let Some(i) = self.textures.iter().position(|t| t.key == cached.key) {
                let old = std::mem::replace(&mut self.textures[i], cached);
                if let Some(t) = old.texture {
                    self.bytes -= t.width() as usize * t.height() as usize * 4;
                }
            } else if warm {
                self.textures.push_back(cached);
            } else {
                self.textures.push_front(cached);
            }
            while self.textures.len() > 4096 || self.bytes > BUDGET {
                let Some(old) = self.textures.pop_back() else {
                    break;
                };
                if self.export.as_ref() == Some(&old.key) && self.textures.len() > 1 {
                    self.textures.push_front(old);
                    continue;
                }
                if let Some(t) = old.texture {
                    self.bytes -= t.width() as usize * t.height() as usize * 4;
                }
            }
        }
        self.waiting.retain(|(w, _)| w.upgrade().is_some());
        while let Some(job) = self.queued.pop_front() {
            if self.export.as_ref() != Some(&job.key)
                && !self.waiting.iter().any(|(_, k)| *k == job.key)
            {
                continue;
            }
            if !self.send(job) {
                return;
            }
        }
        // Background warming only uses the worker when nothing visible waits.
        while self.bytes < WARM_BUDGET {
            let Some(job) = self.warming.pop_front() else {
                break;
            };
            let fresh = self
                .textures
                .iter()
                .any(|t| t.key == job.key && t.generation == self.generation);
            if fresh || self.inflight.contains(&job.key) {
                continue;
            }
            if !self.send(job) {
                break;
            }
        }
    }
    /// Work remains for the worker or for the next poll.
    pub fn busy(&self) -> bool {
        !self.inflight.is_empty()
            || !self.queued.is_empty()
            || (!self.warming.is_empty() && self.bytes < WARM_BUDGET)
    }
    /// Hand a job to the worker; false (job kept) when its channel is full.
    fn send(&mut self, job: Job) -> bool {
        let key = job.key.clone();
        let warm = job.warm;
        match self.jobs.try_send(job) {
            Ok(()) => {
                self.inflight.insert(key);
                true
            }
            Err(mpsc::TrySendError::Full(job)) => {
                if warm {
                    self.warming.push_front(job);
                } else {
                    self.queued.push_front(job);
                }
                false
            }
            Err(mpsc::TrySendError::Disconnected(_)) => false,
        }
    }
}
fn decode_thumb(job: &Job) -> Option<Pixels> {
    let path = match &job.source {
        Source::Track(t, embedded) => cached_cover(t, *embedded, job.size)?,
        Source::File(p) => {
            if p.metadata().ok()?.len() > 64 * 1024 * 1024 {
                return None;
            }
            let (_, w, h) = gdk_pixbuf::Pixbuf::file_info(p)?;
            if w <= 0 || h <= 0 || i64::from(w) * i64::from(h) > 64_000_000 {
                return None;
            }
            p.clone()
        }
    };
    let p = gdk_pixbuf::Pixbuf::from_file_at_scale(&path, job.size, job.size, true).ok()?;
    Some(Pixels {
        bytes: p.read_pixel_bytes(),
        width: p.width(),
        height: p.height(),
        stride: p.rowstride(),
        alpha: p.has_alpha(),
        path: Some(path),
    })
}
/// Cover image next to the track (a disc folder) or in the album folder: cover, folder or
/// front, in any letter case, as JPEG, PNG or WebP.
pub fn external(track: &Track) -> Option<PathBuf> {
    const NAMES: [&str; 3] = ["cover", "folder", "front"];
    const EXTENSIONS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];
    let mut dirs: Vec<PathBuf> = track
        .path
        .parent()
        .map(Path::to_path_buf)
        .into_iter()
        .collect();
    let album = track.album_dir();
    if !dirs.contains(&album) {
        dirs.push(album);
    }
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut found: Vec<(usize, PathBuf)> = entries
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                let stem = path.file_stem()?.to_str()?.to_lowercase();
                let ext = path.extension()?.to_str()?.to_lowercase();
                let rank = NAMES.iter().position(|n| *n == stem)?;
                (EXTENSIONS.contains(&ext.as_str()) && path.is_file()).then_some((rank, path))
            })
            .collect();
        found.sort();
        if let Some((_, p)) = found.into_iter().next() {
            return Some(p);
        }
    }
    None
}
pub fn cached_cover(track: &Track, embedded: bool, size: i32) -> Option<PathBuf> {
    let external = external(track);
    let ext_stamp = external
        .as_ref()
        .and_then(|p| p.metadata().ok())
        .and_then(|m| m.modified().ok());
    let digest = format!(
        "{:x}",
        Sha256::digest(format!(
            "{}:{}:{size}:{embedded}:{ext_stamp:?}",
            track.path.display(),
            track.modified
        ))
    );
    let target = storage::cache_dir()
        .join("covers")
        .join(format!("{digest}.png"));
    if target.is_file() {
        return Some(target);
    }
    let embedded_image = || {
        flac::read_options(&track.path, false, true)
            .ok()
            .and_then(|m| m.picture)
            .and_then(|data| {
                let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(data));
                gdk_pixbuf::Pixbuf::from_stream_at_scale(
                    &stream,
                    size,
                    size,
                    true,
                    gio::Cancellable::NONE,
                )
                .ok()
            })
    };
    let local_image = || {
        external.as_ref().and_then(|p| {
            p.metadata()
                .ok()
                .filter(|m| m.len() < 20 * 1024 * 1024)
                .and_then(|_| gdk_pixbuf::Pixbuf::from_file_at_scale(p, size, size, true).ok())
        })
    };
    let pix = if embedded {
        embedded_image().or_else(local_image)
    } else {
        local_image().or_else(embedded_image)
    }?;
    let bytes = pix.save_to_bufferv("png", &[]).ok()?;
    storage::cache_write(&target, &bytes).ok()?;
    Some(target)
}
pub fn prune() {
    let dir = storage::cache_dir().join("covers");
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = rd
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_file()
                .then_some((e.path(), m.len(), m.modified().ok()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort_by_key(|f| f.2);
    for (p, n, _) in files {
        if total <= 256 * 1024 * 1024 {
            break;
        }
        if std::fs::remove_file(p).is_ok() {
            total -= n;
        }
    }
}
pub fn extras(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in walkdir::WalkDir::new(dir)
        .max_depth(6)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            e.depth() == 0
                || !e.file_type().is_dir()
                || (e.depth() == 1
                    && matches!(
                        e.file_name().to_string_lossy().to_lowercase().as_str(),
                        "artwork" | "booklet" | "covers" | "scans" | "images"
                    ))
                || (e.depth() > 1 && !e.file_name().to_string_lossy().starts_with('.'))
        })
        .flatten()
    {
        if e.file_type().is_file()
            && e.path().extension().is_some_and(|e| {
                matches!(
                    e.to_string_lossy().to_lowercase().as_str(),
                    "jpg" | "jpeg" | "png" | "webp" | "tif" | "tiff" | "bmp" | "gif" | "pdf"
                )
            })
        {
            out.push(e.into_path());
        }
    }
    for name in ["album.txt", "album.json"] {
        let p = dir.join(name);
        if p.is_file() {
            out.push(p)
        }
    }
    out.sort();
    out
}
pub fn embedded_original(track: &Track) -> Option<PathBuf> {
    let data = flac::read_options(&track.path, false, true).ok()?.picture?;
    let ext = if data.starts_with(b"\x89PNG") {
        "png"
    } else if data.starts_with(b"RIFF") {
        "webp"
    } else {
        "jpg"
    };
    let path = storage::cache_dir()
        .join("covers")
        .join(format!("original-{:x}.{ext}", Sha256::digest(&data)));
    if !path.is_file() {
        storage::cache_write(&path, &data).ok()?;
    }
    Some(path)
}
