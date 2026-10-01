use crate::model::{Queue, Track};
use gst::prelude::*;
use gtk::gio::{self, prelude::*};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
#[derive(Debug)]
pub enum Event {
    TrackChanged,
    StateChanged,
    Ended,
    Error(String),
    /// A queued file could not be played and playback moved past it.
    Skipped(std::path::PathBuf, String),
}
#[derive(Default)]
struct Transport {
    queue: Queue,
    scheduled: Queue,
    pending: VecDeque<usize>,
    priority: VecDeque<usize>,
    serial: u64,
    first_stream: bool,
    next_requested: bool,
    unavailable: Option<std::path::PathBuf>,
    /// Files passed over while preparing the next track, reported once.
    skipped: Vec<std::path::PathBuf>,
    /// Counts only real track changes; a seek or output switch rebuilds the pipeline
    /// (and bumps `serial`) but keeps the same track.
    track_serial: u64,
    last_started: Option<(usize, std::path::PathBuf)>,
    group: Option<gst::GroupId>,
    volume: f64,
    replaygain: String,
}
impl Transport {
    fn remap(&mut self, map: impl Fn(usize) -> Option<usize>) {
        for q in [&mut self.queue, &mut self.scheduled] {
            q.index = q.index.and_then(&map);
            q.history = q.history.iter().filter_map(|i| map(*i)).collect();
            q.shuffle_remaining = q.shuffle_remaining.iter().filter_map(|i| map(*i)).collect();
        }
        self.pending = self.pending.iter().filter_map(|i| map(*i)).collect();
        self.priority = self.priority.iter().filter_map(|i| map(*i)).collect();
    }
    fn prepare_next(&mut self, element: &gst::Element) {
        if !self.next_requested {
            return;
        }
        // Missing files are passed over, so a gone track never stops the queue; the bound
        // keeps a queue of only missing files from looping.
        let mut tries = self.scheduled.tracks.len();
        let (next, track) = loop {
            let Some(next) = self
                .priority
                .pop_front()
                .or_else(|| self.scheduled.next_index(true))
            else {
                return;
            };
            let Some(track) = self.scheduled.tracks.get(next).cloned() else {
                return;
            };
            if track.path.is_file() {
                break (next, track);
            }
            self.unavailable = Some(track.path.clone());
            self.skipped.push(track.path.clone());
            self.scheduled.advance_to(next);
            if tries == 0 || self.scheduled.repeat == crate::model::Repeat::One {
                return;
            }
            tries -= 1;
        };
        self.unavailable = None;
        let uri = gio::File::for_path(&track.path).uri();
        self.scheduled.advance_to(next);
        self.pending.push_back(next);
        self.next_requested = false;
        element.set_property("uri", uri.as_str());
    }
    fn stream_started(&mut self, group: Option<gst::GroupId>) {
        // A flushing seek may post STREAM_START again for the same stream.
        // Only a new stream group consumes a prepared queue entry.
        if !self.first_stream && group.is_some() && self.group == group {
            return;
        }
        let gapless = if self.first_stream {
            self.first_stream = false;
            false
        } else if let Some(i) = self.pending.pop_front() {
            self.queue.advance_to(i);
            true
        } else {
            false
        };
        self.group = group;
        self.serial = self.serial.wrapping_add(1);
        let now = self
            .queue
            .index
            .zip(self.queue.current().map(|t| t.path.clone()));
        // A rebuilt pipeline restarting the same entry (seek, output switch, pause from a
        // stopped state) is not a new track; a gapless successor always is (even Repeat One).
        if gapless || now != self.last_started {
            self.track_serial = self.track_serial.wrapping_add(1);
        }
        self.last_started = now;
    }
}
pub struct Player {
    pub element: gst::Element,
    transport: Arc<Mutex<Transport>>,
    pub playing: bool,
    pub loaded: bool,
    pub volume: f64,
    pub replaygain: String,
    bus: gst::Bus,
    pub output: String,
    restore_seek: Option<f64>,
    /// Position requested by the last seek, reported until the rebuilt
    /// pipeline reaches it, so displays never fall back to 0:00 meanwhile.
    seek_target: std::cell::Cell<Option<(f64, Option<usize>, std::time::Instant)>>,
    desired_playing: bool,
    /// Set from a seek (or a restored position) until GStreamer finishes it; while playback is
    /// meant to continue, the transient Ready/Paused states of a seek are not reported.
    seeking: Option<std::time::Instant>,
    undo: Option<(usize, Track)>,
    silent: bool,
    pub meter: Arc<crate::meter::Meter>,
    pub output_device: String,
    /// Track serial last reported as `Event::TrackChanged`.
    reported_track: u64,
    /// Files skipped one after another after errors; reset once a track plays.
    error_skips: usize,
}
impl Player {
    pub fn new(silent: bool) -> Result<Self, String> {
        gst::init().map_err(|e| e.to_string())?;
        let element = gst::ElementFactory::make("playbin3")
            .build()
            .map_err(|e| e.to_string())?;
        // Audio only. No visualization or video path is ever enabled.
        element.set_property_from_str("flags", "audio+soft-volume");
        let sink = if silent {
            Some(
                gst::ElementFactory::make("fakesink")
                    .property("sync", true)
                    .build()
                    .map_err(|e| e.to_string())?,
            )
        } else {
            gst::ElementFactory::make("pulsesink").build().ok()
        };
        let meter = Arc::new(crate::meter::Meter::default());
        if let Some(s) = sink {
            crate::meter::attach(&s, meter.clone());
            if s.find_property("client-name").is_some() {
                s.set_property("client-name", "FLACker");
            }
            if s.find_property("stream-properties").is_some() {
                s.set_property(
                    "stream-properties",
                    gst::Structure::builder("props")
                        .field("media.role", "music")
                        .field("application.name", "FLACker")
                        .build(),
                );
            }
            element.set_property("audio-sink", &s);
        }
        let transport = Arc::new(Mutex::new(Transport {
            first_stream: true,
            ..Default::default()
        }));
        let t = transport.clone();
        element.connect("about-to-finish", false, move |args| {
            let element = args[0].get::<gst::Element>().ok()?;
            let mut s = t.lock().ok()?;
            s.next_requested = true;
            s.prepare_next(&element);
            None
        });
        let bus = element.bus().ok_or("GStreamer has no bus")?;
        let state = transport.clone();
        let pipeline = element.downgrade();
        // Commit the audible stream and its gain on the streaming boundary, before
        // buffers reach the sink. GTK only observes this state from its own thread.
        bus.set_sync_handler(move |_, message| {
            if let gst::MessageView::StreamStart(stream) = message.view() {
                if let (Ok(mut state), Some(pipeline)) = (state.lock(), pipeline.upgrade()) {
                    state.stream_started(stream.group_id());
                    let volume =
                        effective_volume(state.queue.current(), state.volume, &state.replaygain);
                    drop(state);
                    pipeline.set_property("volume", volume);
                }
            }
            gst::BusSyncReply::Pass
        });
        Ok(Self {
            element,
            transport,
            playing: false,
            loaded: false,
            volume: 0.8,
            replaygain: "off".into(),
            bus,
            output: String::new(),
            restore_seek: None,
            seek_target: std::cell::Cell::new(None),
            desired_playing: false,
            seeking: None,
            undo: None,
            silent,
            meter,
            output_device: String::new(),
            reported_track: 0,
            error_skips: 0,
        })
    }
    pub fn summary(
        &self,
    ) -> (
        Option<Track>,
        Option<usize>,
        usize,
        crate::model::Repeat,
        bool,
    ) {
        let s = self.transport.lock().unwrap();
        (
            s.queue.current().cloned(),
            s.queue.index,
            s.queue.tracks.len(),
            s.queue.repeat.clone(),
            s.queue.shuffle,
        )
    }
    /// Whether playback is meant to run (it may still be prerolling or seeking).
    pub fn intends_playing(&self) -> bool {
        self.desired_playing
    }
    /// Number of queued tracks, without copying the queue.
    pub fn queue_len(&self) -> usize {
        self.transport.lock().unwrap().queue.tracks.len()
    }
    pub fn queue(&self) -> Queue {
        self.transport.lock().unwrap().queue.clone()
    }
    pub fn restore(&mut self, queue: Queue, position: f64) {
        self.transport.lock().unwrap().priority.clear();
        {
            let mut state = self.transport.lock().unwrap();
            state.scheduled = queue.clone();
            state.queue = queue;
        }
        self.restore_seek = Some(position);
    }
    pub fn current(&self) -> Option<Track> {
        self.transport.lock().unwrap().queue.current().cloned()
    }
    pub fn set_queue(&mut self, tracks: Vec<Track>, index: usize) -> Result<(), String> {
        self.reset()?;
        self.undo = None;
        self.transport.lock().unwrap().priority.clear();
        self.transport.lock().unwrap().queue.replace(tracks, index);
        self.restore_seek = None;
        self.start()
    }
    fn reset(&mut self) -> Result<(), String> {
        self.element
            .set_state(gst::State::Null)
            .map_err(|e| e.to_string())?;
        while self.bus.pop().is_some() {}
        let mut s = self.transport.lock().unwrap();
        s.pending.clear();
        s.next_requested = false;
        s.unavailable = None;
        s.first_stream = true;
        s.group = None;
        self.loaded = false;
        self.playing = false;
        self.desired_playing = false;
        self.restore_seek = None;
        self.seek_target.set(None);
        self.seeking = None;
        Ok(())
    }
    pub fn start(&mut self) -> Result<(), String> {
        {
            let mut s = self.transport.lock().unwrap();
            s.scheduled = s.queue.clone();
        }
        let Some(t) = self.current() else {
            return Ok(());
        };
        if !t.path.is_file() {
            return Err(format!("File unavailable: {}", t.path.display()));
        }
        self.element
            .set_property("uri", gio::File::for_path(t.path).uri().as_str());
        self.apply_volume();
        self.desired_playing = true;
        self.element
            .set_state(if self.restore_seek.is_some() {
                gst::State::Paused
            } else {
                gst::State::Playing
            })
            .map_err(|e| e.to_string())?;
        self.loaded = true;
        // Prerolling to a restored position: playback is about to start, so show it.
        if self.restore_seek.is_some() {
            self.seeking = Some(std::time::Instant::now());
        }
        self.playing = true;
        Ok(())
    }
    pub fn toggle(&mut self) -> Result<(), String> {
        if !self.loaded {
            return self.start();
        }
        if self.desired_playing {
            self.pause()
        } else {
            self.resume()
        }
    }
    pub fn pause(&mut self) -> Result<(), String> {
        self.desired_playing = false;
        if !self.loaded {
            self.playing = false;
            return Ok(());
        }
        self.element
            .set_state(gst::State::Paused)
            .map_err(|e| e.to_string())?;
        self.playing = false;
        Ok(())
    }
    pub fn resume(&mut self) -> Result<(), String> {
        self.desired_playing = true;
        if !self.loaded {
            return self.start();
        }
        self.element
            .set_state(gst::State::Playing)
            .map_err(|e| e.to_string())?;
        self.playing = true;
        Ok(())
    }
    pub fn stop(&mut self) -> Result<(), String> {
        self.reset()
    }
    /// Start the current entry; a missing file is reported and the queue moves on to the
    /// next playable one instead of stopping there.
    fn start_playable(&mut self) -> Result<(), String> {
        let mut tries = self.queue_len();
        loop {
            match self.current() {
                Some(t) if !t.path.is_file() && tries > 0 => {
                    tries -= 1;
                    self.transport.lock().unwrap().skipped.push(t.path.clone());
                    let next = {
                        let mut s = self.transport.lock().unwrap();
                        s.priority.pop_front().or_else(|| s.queue.next_index(false))
                    };
                    let Some(i) = next else {
                        return Err(format!("File unavailable: {}", t.path.display()));
                    };
                    self.transport.lock().unwrap().queue.advance_to(i);
                }
                _ => return self.start(),
            }
        }
    }
    pub fn next(&mut self) -> Result<(), String> {
        // Decide first: on the last track (no repeat) Next does nothing rather than stopping.
        let next = {
            let mut s = self.transport.lock().unwrap();
            s.priority.pop_front().or_else(|| s.queue.next_index(false))
        };
        let Some(i) = next else { return Ok(()) };
        self.reset()?;
        self.transport.lock().unwrap().queue.advance_to(i);
        self.start_playable()
    }
    pub fn previous(&mut self) -> Result<(), String> {
        if self.position() > 3. {
            return self.seek(0.);
        }
        // On the first track Previous restarts it instead of stopping playback.
        let Some(i) = self.transport.lock().unwrap().queue.previous() else {
            return self.seek(0.);
        };
        self.reset()?;
        self.transport.lock().unwrap().queue.index = Some(i);
        self.start_playable()
    }
    pub fn jump(&mut self, i: usize) -> Result<(), String> {
        if i >= self.queue_len() {
            return Ok(());
        }
        self.reset()?;
        self.transport.lock().unwrap().queue.advance_to(i);
        self.start_playable()
    }
    pub fn seek(&mut self, seconds: f64) -> Result<(), String> {
        if !seconds.is_finite() {
            return Ok(());
        }
        let end = self.current().map_or(seconds, |t| t.duration);
        let pos = seconds.clamp(0., end.max(0.));
        if !self.loaded {
            self.restore_seek = Some(pos);
            return Ok(());
        }
        if self.can_seek_in_place(pos) {
            // A plain flushing seek: no pipeline rebuild, no device reopen, no state flicker.
            self.raw_seek(pos)?;
            self.seeking = Some(std::time::Instant::now());
            let index = self.transport.lock().unwrap().queue.index;
            self.seek_target
                .set(Some((pos, index, std::time::Instant::now())));
            return Ok(());
        }
        // Scheduling runs concurrently. Checking whether a next URI exists and
        // then seeking leaves a race in which that URI can appear between them.
        // Rebuild every user seek from the captured audible queue instead.
        let queue = self.queue();
        let playing = self.desired_playing;
        self.reset()?;
        self.transport.lock().unwrap().queue = queue;
        self.restore_seek = Some(pos);
        self.start()?;
        if !playing {
            self.pause()?;
        }
        let index = self.transport.lock().unwrap().queue.index;
        self.seek_target
            .set(Some((pos, index, std::time::Instant::now())));
        // The rebuilt pipeline prerolls paused before resuming; report the intended state.
        self.playing = playing;
        Ok(())
    }
    /// A seek can stay in the running pipeline while no successor is prepared or requested and
    /// both the current and the target positions are far from the end: about-to-finish (which
    /// prepares the next track) cannot run during the seek then. Near the end, the pipeline is
    /// rebuilt from the audible queue instead.
    fn can_seek_in_place(&self, target: f64) -> bool {
        const MARGIN: f64 = 30.;
        {
            let s = self.transport.lock().unwrap();
            if !s.pending.is_empty() || s.next_requested || self.restore_seek.is_some() {
                return false;
            }
        }
        let Some(duration) = self
            .element
            .query_duration::<gst::ClockTime>()
            .map(|t| t.nseconds() as f64 / 1e9)
        else {
            return false;
        };
        let Some(current) = self
            .element
            .query_position::<gst::ClockTime>()
            .map(|t| t.nseconds() as f64 / 1e9)
        else {
            return false;
        };
        duration - current.max(target) > MARGIN
    }
    fn raw_seek(&self, pos: f64) -> Result<(), String> {
        self.element
            .seek_simple(
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::ClockTime::from_nseconds((pos * 1e9) as u64),
            )
            .map_err(|e| e.to_string())
    }
    pub fn position(&self) -> f64 {
        if !self.loaded {
            return self.restore_seek.unwrap_or(0.);
        }
        let actual = self
            .element
            .query_position::<gst::ClockTime>()
            .map_or(0., |t| t.nseconds() as f64 / 1e9);
        if let Some((target, index, since)) = self.seek_target.get() {
            let reached = self.restore_seek.is_none() && (actual - target).abs() < 1.;
            let moved_on = self.transport.lock().unwrap().queue.index != index;
            if reached || moved_on || since.elapsed() > std::time::Duration::from_secs(3) {
                self.seek_target.set(None);
            } else {
                return target;
            }
        }
        actual
    }
    pub fn duration(&self) -> f64 {
        self.element.query_duration::<gst::ClockTime>().map_or_else(
            || self.current().map_or(0., |t| t.duration),
            |t| t.nseconds() as f64 / 1e9,
        )
    }
    pub fn apply_volume(&self) {
        let mut state = self.transport.lock().unwrap();
        state.volume = self.volume;
        state.replaygain = self.replaygain.clone();
        let volume = effective_volume(state.queue.current(), self.volume, &self.replaygain);
        drop(state);
        self.element.set_property("volume", volume);
    }
    pub fn set_repeat(&mut self, repeat: crate::model::Repeat) -> Result<(), String> {
        // Policy changes never stop, seek or preroll the audible stream. Already
        // committed gapless buffers remain valid; future choices use the new mode.
        let mut state = self.transport.lock().unwrap();
        state.queue.repeat = repeat.clone();
        state.scheduled.repeat = repeat;
        // about-to-finish may already have declined a successor at the end of
        // the queue. Enabling repeat now must still allow another track.
        state.prepare_next(&self.element);
        Ok(())
    }
    pub fn set_shuffle(&mut self, on: bool) -> Result<(), String> {
        let mut state = self.transport.lock().unwrap();
        if state.queue.shuffle == on {
            return Ok(());
        }
        state.queue.shuffle = on;
        state.queue.history.clear();
        state.queue.shuffle_remaining.clear();
        state.scheduled.shuffle = on;
        state.scheduled.shuffle_remaining.clear();
        // Keep committed successors out of the new random pool. The audible
        // index, pending indices and pipeline are untouched.
        state.scheduled.history = state
            .queue
            .index
            .into_iter()
            .chain(state.pending.iter().copied())
            .take(state.pending.len())
            .collect();
        state.prepare_next(&self.element);
        Ok(())
    }
    /// Changes only when a different track starts (not on seeks or output switches).
    pub fn track_serial(&self) -> u64 {
        self.transport.lock().unwrap().track_serial
    }
    pub fn album_remaining(&self) -> Option<f64> {
        let state = self.transport.lock().unwrap();
        let q = &state.queue;
        if q.shuffle {
            return None;
        }
        let i = q.index?;
        let key = q.current()?.album_key();
        let album: f64 = q.tracks[i..]
            .iter()
            .take_while(|t| t.album_key() == key)
            .map(|t| t.duration)
            .sum();
        // position() takes the same lock while a seek is pending; release it first.
        drop(state);
        Some(album - self.position())
    }
    pub fn append(&mut self, tracks: Vec<Track>) {
        let mut s = self.transport.lock().unwrap();
        s.scheduled.tracks.extend(tracks.clone());
        s.queue.tracks.extend(tracks);
        s.prepare_next(&self.element);
    }
    pub fn insert(&mut self, index: usize, tracks: Vec<Track>, priority: bool) -> usize {
        let mut s = self.transport.lock().unwrap();
        let index = index.min(s.queue.tracks.len());
        let count = tracks.len();
        let was_empty = s.queue.tracks.is_empty();
        s.remap(|i| Some(if i >= index { i + count } else { i }));
        s.queue.tracks.splice(index..index, tracks.clone());
        s.scheduled.tracks.splice(index..index, tracks);
        if s.queue.index.is_none() && !s.queue.tracks.is_empty() {
            s.queue.index = Some(0);
            s.scheduled = s.queue.clone();
        }
        if priority && !was_empty {
            for i in (index..index + count).rev() {
                s.priority.push_front(i);
            }
        }
        s.prepare_next(&self.element);
        index
    }
    pub fn play_next(&mut self, tracks: Vec<Track>) -> bool {
        let (index, committed) = {
            let s = self.transport.lock().unwrap();
            (
                s.pending
                    .back()
                    .copied()
                    .or(s.queue.index)
                    .map_or(0, |i| i + 1),
                !s.pending.is_empty(),
            )
        };
        self.insert(index, tracks, true);
        committed
    }
    pub fn move_item(&mut self, from: usize, to: usize) -> Result<(), String> {
        let mut s = self.transport.lock().unwrap();
        let count = s.queue.tracks.len();
        if from >= count || to >= count || from == to {
            return Ok(());
        }
        s.remap(|i| {
            Some(if i == from {
                to
            } else if from < i && to >= i {
                i - 1
            } else if from > i && to <= i {
                i + 1
            } else {
                i
            })
        });
        let t = s.queue.tracks.remove(from);
        s.queue.tracks.insert(to, t);
        let t = s.scheduled.tracks.remove(from);
        s.scheduled.tracks.insert(to, t);
        s.prepare_next(&self.element);
        Ok(())
    }
    pub fn remove(&mut self, index: usize) -> Result<(), String> {
        let mut s = self.transport.lock().unwrap();
        let Some(track) = s.queue.tracks.get(index).cloned() else {
            return Ok(());
        };
        if s.pending.contains(&index) && s.queue.index != Some(index) {
            return Err(
                "This track is about to start and can't be removed yet. Try again once it plays."
                    .into(),
            );
        }
        if s.queue.index != Some(index) {
            s.remap(|i| {
                if i == index {
                    None
                } else {
                    Some(if i > index { i - 1 } else { i })
                }
            });
            s.queue.tracks.remove(index);
            s.scheduled.tracks.remove(index);
            s.prepare_next(&self.element);
            self.undo = Some((index, track));
            return Ok(());
        }
        drop(s);
        let playing = self.desired_playing;
        let loaded = self.loaded;
        self.reset()?;
        {
            let mut s = self.transport.lock().unwrap();
            s.remap(|i| {
                if i == index {
                    None
                } else {
                    Some(if i > index { i - 1 } else { i })
                }
            });
            s.queue.tracks.remove(index);
            // The track after the removed one takes its place; removing the last one stops.
            let was_last = index >= s.queue.tracks.len();
            s.queue.index = (!s.queue.tracks.is_empty())
                .then_some(index.min(s.queue.tracks.len().saturating_sub(1)));
            s.scheduled = s.queue.clone();
            if was_last {
                drop(s);
                self.undo = Some((index, track));
                return Ok(());
            }
        }
        self.undo = Some((index, track));
        if loaded && self.current().is_some() {
            self.start()?;
            if !playing {
                self.pause()?;
            }
        }
        Ok(())
    }
    pub fn undo_remove(&mut self) -> bool {
        let Some((i, t)) = self.undo.take() else {
            return false;
        };
        self.insert(i, vec![t], false);
        true
    }
    pub fn select_output(&mut self, device: &str) -> Result<(), String> {
        if self.output_device == device {
            return Ok(());
        }
        if self.silent {
            self.output_device = device.into();
            return Ok(());
        }
        let sink = self
            .element
            .property::<Option<gst::Element>>("audio-sink")
            .ok_or("No selectable output")?;
        if sink.find_property("device").is_none() {
            return Err("This audio server does not allow choosing the output here.".into());
        }
        let position = self.position();
        let loaded = self.loaded;
        let playing = self.desired_playing;
        self.reset()?;
        sink.set_property(
            "device",
            if device.is_empty() {
                None
            } else {
                Some(device)
            },
        );
        self.output_device = device.into();
        self.output.clear();
        self.restore_seek = Some(position);
        if loaded {
            self.start()?;
            if !playing {
                self.pause()?;
            }
        }
        Ok(())
    }
    pub fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        while let Some(msg) = self.bus.pop() {
            use gst::MessageView;
            match msg.view() {
                MessageView::Error(e) => {
                    let reason = e.error().to_string();
                    // A file that cannot be read or decoded is skipped; a device error stops.
                    let file_problem = e.error().kind::<gst::StreamError>().is_some()
                        || matches!(
                            e.error().kind::<gst::ResourceError>(),
                            Some(
                                gst::ResourceError::NotFound
                                    | gst::ResourceError::OpenRead
                                    | gst::ResourceError::Read
                                    | gst::ResourceError::Seek
                                    | gst::ResourceError::NotAuthorized
                            )
                        );
                    let failed = {
                        let s = self.transport.lock().unwrap();
                        // While a successor is prepared, the error belongs to the new file.
                        s.pending.front().copied().or(s.queue.index)
                    };
                    let len = self.queue_len();
                    let _ = self.reset();
                    let next = failed.and_then(|f| {
                        let mut s = self.transport.lock().unwrap();
                        s.queue.index = Some(f);
                        s.priority.pop_front().or_else(|| s.queue.next_index(false))
                    });
                    match (file_problem, failed, next) {
                        (true, Some(f), Some(i)) if self.error_skips < len => {
                            self.error_skips += 1;
                            let path = self.queue().tracks.get(f).map(|t| t.path.clone());
                            self.transport.lock().unwrap().queue.advance_to(i);
                            if let Some(p) = path {
                                out.push(Event::Skipped(p, reason));
                            }
                            if let Err(e) = self.start_playable() {
                                out.push(Event::Error(e));
                            }
                        }
                        _ => out.push(Event::Error(reason)),
                    }
                }
                MessageView::StreamStart(_) => {
                    self.update_output();
                    let track = self.transport.lock().unwrap().track_serial;
                    if track != self.reported_track {
                        self.reported_track = track;
                        out.push(Event::TrackChanged);
                    }
                }
                MessageView::AsyncDone(_) => {
                    // The restored position's preroll is followed by its own seek; any other
                    // completion ends the seek in progress.
                    if self.restore_seek.is_none() {
                        self.seeking = None;
                    }
                    if let Some(position) = self.restore_seek.take() {
                        if let Err(error) = self.raw_seek(position) {
                            out.push(Event::Error(error));
                        }
                        if self.desired_playing {
                            let _ = self.element.set_state(gst::State::Playing);
                        }
                    }
                    self.update_output();
                }
                MessageView::Eos(_) => {
                    let pending = self.transport.lock().unwrap().pending.pop_front();
                    if let Some(i) = pending {
                        let _ = self.reset();
                        self.transport.lock().unwrap().queue.advance_to(i);
                        if let Err(e) = self.start_playable() {
                            out.push(Event::Error(e));
                        }
                    } else {
                        let _ = self.reset();
                        out.push(Event::Ended);
                    }
                }
                MessageView::StateChanged(s)
                    if msg.src().is_some_and(|src| src == &self.element) =>
                {
                    // A seek passes through Paused inside GStreamer; while playback is meant to
                    // continue, that transient state is not shown (no Playing/Paused flicker).
                    let seeking = self
                        .seeking
                        .is_some_and(|since| since.elapsed().as_secs() < 5);
                    self.playing = s.current() == gst::State::Playing
                        || (seeking && self.desired_playing && s.current() >= gst::State::Ready);
                    if self.playing {
                        self.error_skips = 0;
                    }
                    out.push(Event::StateChanged);
                }
                _ => {}
            }
        }
        let skipped = std::mem::take(&mut self.transport.lock().unwrap().skipped);
        for p in skipped {
            out.push(Event::Skipped(p, "File unavailable".into()));
        }
        out
    }
    fn update_output(&mut self) {
        if let Some(sink) = self.element.property::<Option<gst::Element>>("audio-sink") {
            if let Some(caps) = sink.static_pad("sink").and_then(|p| p.current_caps()) {
                if let Some(s) = caps.structure(0) {
                    let rate = s.get::<i32>("rate").unwrap_or(0);
                    let format = s.get::<String>("format").unwrap_or_default();
                    let channels = s.get::<i32>("channels").unwrap_or(0);
                    let name = if sink.find_property("device-name").is_some() {
                        sink.property::<Option<String>>("device-name")
                            .unwrap_or_else(|| "System default".into())
                    } else {
                        "System output".into()
                    };
                    self.output = format!(
                        "Device: {name}\nApp output: {format} · {} kHz · {channels} {}. The audio server may convert the signal before the device.",
                        rate as f64 / 1000.,
                        if channels == 1 { "channel" } else { "channels" }
                    );
                }
            }
        }
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.element.set_state(gst::State::Null);
    }
}

fn effective_volume(track: Option<&Track>, volume: f64, mode: &str) -> f64 {
    let mut gain = 1.;
    if let Some(track) = track {
        let (db, peak) = match mode {
            "track" => (track.gain_track, track.peak_track),
            "album" => (
                track.gain_album.or(track.gain_track),
                track.peak_album.or(track.peak_track),
            ),
            _ => (None, None),
        };
        if let Some(db) = db {
            gain = 10f64.powf(db.clamp(-60., 30.) / 20.);
            if let Some(peak) = peak.filter(|p| *p > 0.) {
                gain = gain.min(1. / peak);
            } else {
                // Without a known peak a positive gain could clip: never amplify then.
                gain = gain.min(1.);
            }
        }
    }
    (volume * gain).clamp(0., 10.)
}
/// Enumerated locally. Only Pulse-compatible devices are offered to pulsesink.
pub fn outputs() -> Vec<(String, String)> {
    let mut result = vec![(String::new(), "System default".into())];
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(Some("Audio/Sink"), None);
    if monitor.start().is_ok() {
        for device in monitor.devices() {
            if let Ok(element) = device.create_element(None) {
                if element.factory().is_some_and(|f| f.name() == "pulsesink") {
                    if let Some(id) = element.property::<Option<String>>("device") {
                        if !id.is_empty() && !result.iter().any(|(key, _)| key == &id) {
                            result.push((id, device.display_name().to_string()));
                        }
                    }
                }
            }
        }
        monitor.stop();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editing_stopped_queue_preserves_position_and_duplicate_identity() {
        let mut player = Player::new(true).unwrap();
        let mut queue = Queue::default();
        queue.replace(vec![Track::default(); 3], 1);
        player.restore(queue, 5.);
        player.move_item(0, 2).unwrap();
        assert_eq!(player.summary().1, Some(0));
        assert_eq!(player.position(), 5.);
        player.remove(2).unwrap();
        assert_eq!(player.position(), 5.);
        assert!(!player.loaded);
    }
    #[test]
    fn mode_changes_preserve_stopped_position_and_committed_successors() {
        let mut player = Player::new(true).unwrap();
        let mut queue = Queue::default();
        queue.replace(vec![Track::default(); 4], 0);
        player.restore(queue.clone(), 5.);
        {
            let mut state = player.transport.lock().unwrap();
            state.scheduled = queue;
            state.scheduled.advance_to(1);
            state.scheduled.advance_to(2);
            state.pending.extend([1, 2]);
        }
        player.set_repeat(crate::model::Repeat::One).unwrap();
        player.set_shuffle(true).unwrap();
        assert_eq!(player.position(), 5.);
        assert!(!player.loaded);
        let mut state = player.transport.lock().unwrap();
        assert_eq!(state.queue.index, Some(0));
        assert_eq!(state.scheduled.index, Some(2));
        assert_eq!(state.scheduled.repeat, crate::model::Repeat::One);
        assert_eq!(state.pending.iter().copied().collect::<Vec<_>>(), [1, 2]);
        state.scheduled.repeat = crate::model::Repeat::Off;
        assert_eq!(state.scheduled.next_index(true), Some(3));
    }
    #[test]
    fn repeated_shuffle_setting_keeps_the_random_cycle() {
        let mut player = Player::new(true).unwrap();
        player.set_shuffle(true).unwrap();
        {
            let mut state = player.transport.lock().unwrap();
            state.queue.history = vec![0];
            state.scheduled.shuffle_remaining = vec![3, 2];
        }
        player.set_shuffle(true).unwrap();
        let state = player.transport.lock().unwrap();
        assert_eq!(state.queue.history, [0]);
        assert_eq!(state.scheduled.shuffle_remaining, [3, 2]);
    }
    #[test]
    fn repeated_stream_start_does_not_consume_next_track() {
        let mut state = Transport {
            first_stream: true,
            ..Default::default()
        };
        state
            .queue
            .replace(vec![Track::default(), Track::default()], 0);
        let first = gst::GroupId::next();
        state.stream_started(Some(first));
        state.pending.push_back(1);
        state.stream_started(Some(first));
        assert_eq!(state.queue.index, Some(0));
        assert_eq!(state.pending.len(), 1);
        state.stream_started(Some(gst::GroupId::next()));
        assert_eq!(state.queue.index, Some(1));
        assert!(state.pending.is_empty());
    }
    #[test]
    fn future_edits_preserve_prepared_stream_identity_and_undo() {
        let mut p = Player::new(true).unwrap();
        let mut q = Queue::default();
        let tracks: Vec<_> = (0..5)
            .map(|i| Track {
                title: i.to_string(),
                ..Default::default()
            })
            .collect();
        q.replace(tracks, 1);
        p.restore(q, 12.);
        {
            let mut s = p.transport.lock().unwrap();
            s.scheduled.advance_to(2);
            s.pending.push_back(2);
        }
        p.move_item(4, 0).unwrap();
        assert_eq!(p.current().unwrap().title, "1");
        {
            let s = p.transport.lock().unwrap();
            assert_eq!(s.pending[0], 3);
            assert_eq!(s.scheduled.current().unwrap().title, "2");
        }
        assert!(p.remove(3).is_err());
        p.remove(0).unwrap();
        assert_eq!(p.position(), 12.);
        assert!(p.undo_remove());
        assert!(!p.undo_remove());
        assert_eq!(p.queue().tracks[0].title, "4");
        assert!(p.play_next(vec![Track {
            title: "next".into(),
            ..Default::default()
        }]));
        let mut s = p.transport.lock().unwrap();
        assert_eq!(s.pending[0], 3);
        assert_eq!(s.priority[0], 4);
        s.first_stream = false;
        s.stream_started(Some(gst::GroupId::next()));
        assert_eq!(s.queue.current().unwrap().title, "2");
    }
    #[test]
    fn next_into_empty_queue_does_not_repeat_the_first_track() {
        let mut p = Player::new(true).unwrap();
        p.play_next(vec![Track::default()]);
        assert_eq!(p.summary().1, Some(0));
        assert!(p.transport.lock().unwrap().priority.is_empty());
    }
    #[test]
    fn replaygain_is_optional_and_peak_limited() {
        let t = Track {
            gain_track: Some(-6.020599913),
            gain_album: Some(12.),
            peak_track: Some(0.5),
            peak_album: Some(1.),
            ..Default::default()
        };
        assert_eq!(effective_volume(Some(&t), 0.8, "off"), 0.8);
        assert!((effective_volume(Some(&t), 1., "track") - 0.5).abs() < 0.00001);
        assert_eq!(effective_volume(Some(&t), 1., "album"), 1.);
    }
}
