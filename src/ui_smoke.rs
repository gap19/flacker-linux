//! Opt-in native screenshots and layout regression checks; inactive in normal use.
use super::*;
impl Ui {
    pub fn screenshot(&self, path: &std::path::Path) -> Result<(), String> {
        screenshot_window(&self.window, path)
    }
    pub(super) fn smoke_tick(&self) {
        let Some(dir) = std::env::var_os("FLACKER_UI_SMOKE") else {
            return;
        };
        let dir = PathBuf::from(dir);
        if std::env::var_os("FLACKER_MOTION_DEMO").is_some() {
            if !self.scanning.get()
                && self.started.elapsed().as_secs() >= 4
                && self.smoke_phase.get() == 0
            {
                self.smoke_phase.set(1);
                self.record_motion(&dir);
            }
            return;
        }
        let phase = self.smoke_phase.get();
        if self.scanning.get() || self.started.elapsed().as_secs() < 3 + phase as u64 * 2 {
            return;
        }
        // A cold grid may bind its first cells during allocation. Wait for two
        // real frame-clock cycles before inspecting the native widget tree.
        if !self.smoke_pending.replace(true) {
            self.smoke_ready.set(false);
            let ready = self.smoke_ready.clone();
            let frames = Cell::new(0);
            self.window.add_tick_callback(move |_, _| {
                frames.set(frames.get() + 1);
                if frames.get() >= 2 {
                    ready.set(true);
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            });
            self.window.queue_draw();
            return;
        }
        if !self.smoke_ready.get() {
            return;
        }
        self.smoke_pending.set(false);
        let _ = std::fs::create_dir_all(&dir);
        let result = self.screenshot(&dir.join(format!("screen-{phase}.png")));
        if let Err(e) = result {
            eprintln!("Screenshot: {e}")
        }
        if std::env::var_os("FLACKER_LAYOUT_SMOKE").is_some() {
            self.layout_review(&dir, phase);
            self.smoke_phase.set(phase + 1);
            return;
        }
        if std::env::var_os("FLACKER_TOUR_SMOKE").is_some() {
            self.tour(&dir, phase);
            self.smoke_phase.set(phase + 1);
            return;
        }
        match phase {
            0 => {
                let families = self.window.pango_context().list_families();
                for expected in ["League Gothic", "Source Sans 3"] {
                    assert!(
                        families.iter().any(|f| f.name() == expected),
                        "Bundled font unavailable: {expected}"
                    );
                }
                if let Some(a) = self.albums.borrow().iter().find(|a| {
                    a.tracks[0].has_lyrics && a.tracks[0].path.with_extension("lrc").is_file()
                }) {
                    let _ = self.sender.send(Command::View(View::Album(a.key.clone())));
                }
            }
            1 => {
                assert!(
                    !self.motion.is_active(),
                    "Cover transition retained after completion"
                );
                if let Some(a) = self.albums.borrow().iter().find(|a| {
                    a.tracks[0].has_lyrics && a.tracks[0].path.with_extension("lrc").is_file()
                }) {
                    let _ = self.sender.send(Command::Play(a.tracks.clone(), 0));
                    let _ = self.sender.send(Command::View(View::Now));
                }
            }
            2 => {
                let _ = self.sender.send(Command::View(View::Tracks));
            }
            3 => {
                self.smoke_released
                    .replace(ephemeral_widgets(self.content.upcast_ref()));
                assert!(!self.smoke_released.borrow().is_empty());
                let _ = self.sender.send(Command::Theme("light".into()));
                let _ = self.sender.send(Command::View(View::Albums));
            }
            4 if std::env::var_os("FLACKER_REVIEW_SMOKE").is_some() => {
                assert!(
                    self.smoke_released
                        .borrow()
                        .iter()
                        .all(|w| w.upgrade().is_none()),
                    "Track tools retained after closing their view"
                );
                assert!(
                    !widget_labels(self.window.upcast_ref())
                        .iter()
                        .any(|s| s == "Follow along")
                );
                let _ = self.sender.send(Command::Theme("dark".into()));
                let _ = self.sender.send(Command::View(View::Albums));
            }
            5 => {
                // The grid was shown before: covers must come from memory in
                // the first frames, not from another decode.
                let covers: Vec<gtk::Picture> =
                    descendants(self.album_scroll.borrow().as_ref().unwrap().2.upcast_ref())
                        .into_iter()
                        .filter(|w| w.has_css_class("album-cover") && w.is_mapped())
                        .filter_map(|w| w.downcast::<gtk::Picture>().ok())
                        .collect();
                assert!(!covers.is_empty());
                assert!(
                    covers.iter().all(|p| p.paintable().is_some()),
                    "Revisited grid reloaded its covers"
                );
                let scroll = self.album_scroll.borrow();
                let grid = scroll
                    .as_ref()
                    .unwrap()
                    .2
                    .child()
                    .and_downcast::<gtk::GridView>()
                    .unwrap();
                let count = grid.model().unwrap().n_items();
                grid.scroll_to(count - 1, gtk::ListScrollFlags::NONE, None);
            }
            6 => {
                let position = self
                    .album_scroll
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .2
                    .vadjustment()
                    .value();
                assert!(
                    position > 0.,
                    "The whole collection must be reachable without pagination"
                );
                self.smoke_scroll.set(position);
                self.smoke_released
                    .replace(ephemeral_widgets(self.content.upcast_ref()));
                assert!(!self.smoke_released.borrow().is_empty());
                self.render();
            }
            7 => {
                assert!(
                    self.smoke_released
                        .borrow()
                        .iter()
                        .all(|w| w.upgrade().is_none()),
                    "Filter popover retained after closing its view"
                );
                let position = self
                    .album_scroll
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .2
                    .vadjustment()
                    .value();
                assert!(
                    (position - self.smoke_scroll.get()).abs() < 2.,
                    "Scroll position was lost: {position}"
                );
                let requested = std::env::var("FLACKER_SMOKE_ALBUM").unwrap_or_default();
                let albums = self.albums.borrow();
                let album = albums
                    .iter()
                    .find(|a| a.title == requested)
                    .or_else(|| {
                        albums
                            .iter()
                            .find(|a| artwork::extras(&a.tracks[0].album_dir()).len() > 1)
                    })
                    .unwrap();
                self.extras(album.tracks[0].album_dir());
            }
            8 => {
                let gallery = self.gallery.borrow();
                let g = gallery.as_ref().unwrap();
                assert_fits(g);
                screenshot_window(&g.window, &dir.join("gallery-fit.png")).unwrap();
                g.window.set_default_size(640, 580);
            }
            9 => {
                let gallery = self.gallery.borrow();
                let g = gallery.as_ref().unwrap();
                assert_fits(g);
                screenshot_window(&g.window, &dir.join("gallery-narrow.png")).unwrap();
                let model = g.select.model().unwrap();
                let next = (0..model.n_items())
                    .find(|i| {
                        model
                            .item(*i)
                            .and_downcast::<gtk::StringObject>()
                            .is_some_and(|s| s.string().starts_with("Back cover"))
                    })
                    .unwrap_or(1);
                g.show(next as usize);
            }
            10 => {
                let gallery = self.gallery.borrow();
                let g = gallery.as_ref().unwrap();
                assert_fits(g);
                assert!(g.selected.get() > 0);
                screenshot_window(&g.window, &dir.join("gallery-next.png")).unwrap();
                g.set_zoom(2.);
            }
            11 => {
                let gallery = self.gallery.borrow();
                let g = gallery.as_ref().unwrap();
                assert_eq!(g.zoom.get(), 2.);
                assert!(
                    g.stage.hadjustment().upper() > g.stage.hadjustment().page_size()
                        || g.stage.vadjustment().upper() > g.stage.vadjustment().page_size()
                );
                screenshot_window(&g.window, &dir.join("gallery-zoom.png")).unwrap();
                g.set_zoom(1.);
                g.window.close();
                self.clear();
                let sample = Album::clone(&self.albums.borrow()[0]);
                let mut large = Vec::new();
                for i in 0..10_000 {
                    let mut album = sample.clone();
                    album.key = format!("fixture-{i}");
                    album.title = format!("Album {i}");
                    album.tracks.truncate(1);
                    large.push(Rc::new(album));
                }
                self.heading(
                    "Large shelf test",
                    "10,000 synthetic entries, only in this test session",
                );
                self.album_grid(large);
            }
            12 => {
                let scroll = self.album_scroll.borrow();
                let grid = scroll
                    .as_ref()
                    .unwrap()
                    .2
                    .child()
                    .and_downcast::<gtk::GridView>()
                    .unwrap();
                let allocated = child_count(grid.upcast_ref());
                assert!(allocated < 1000, "Grid did not recycle cells: {allocated}");
                assert_eq!(grid.model().unwrap().n_items(), 10_000);
                // Covers are decoded for the display scale (HiDPI): report the largest grid texture.
                let cover_pixels = descendants(grid.upcast_ref())
                    .into_iter()
                    .filter_map(|w| w.downcast::<gtk::Picture>().ok())
                    .filter_map(|p| p.paintable())
                    .map(|p| p.intrinsic_width())
                    .max()
                    .unwrap_or(0);
                let metrics = serde_json::json!({"model_albums":10_000,"allocated_cells":allocated,"scroll_position_restored":true,"gallery_fit_and_zoom":"passed","lyrics_display":"static","scale_factor":self.window.scale_factor(),"grid_cover_pixels":cover_pixels});
                std::fs::write(
                    dir.join("review.json"),
                    serde_json::to_vec_pretty(&metrics).unwrap(),
                )
                .unwrap();
                if std::env::var_os("FLACKER_EXPERIENCE_SMOKE").is_some() {
                    let f = crate::collection::Filter {
                        year: "1978".into(),
                        original_year: true,
                        genre: "Jazz".into(),
                        ..Default::default()
                    };
                    assert_eq!(
                        f.albums(&self.albums.borrow(), &HashSet::new(), "").len(),
                        80
                    );
                    let _ = self.sender.send(Command::Collection(f));
                    let _ = self.sender.send(Command::View(View::Albums));
                } else {
                    self.save();
                    self.app.quit();
                }
            }
            13 => {
                let _ = self.sender.send(Command::RandomAlbum);
            }
            14 => {
                let q = self.player.borrow().queue();
                assert!(!q.shuffle);
                assert_eq!(q.repeat, Repeat::Off);
                assert!(self.albums.borrow().iter().any(|a| a.tracks == q.tracks));
                let _ = self.sender.send(Command::Presentation);
            }
            15 => {
                let f = self.experience.focus.borrow();
                let f = f.as_ref().unwrap();
                assert!(!f.title.text().is_empty());
                assert_eq!(f.window.title().as_deref(), Some("Focus Mode — FLACker"));
                let labels = widget_labels(f.window.upcast_ref());
                assert!(labels.iter().any(|s| s == "Focus Mode"));
                assert!(!labels.iter().any(|s| s == "Listening"));
                assert!(f.seek.adjustment().upper() >= 1.);
                screenshot_window(&f.window, &dir.join("focus.png")).unwrap();
            }
            16 => {
                if let Some(f) = self.experience.focus.take() {
                    f.window.close();
                }
                let albums = self.albums.borrow();
                let a = albums
                    .iter()
                    .find(|a| artwork::extras(&a.tracks[0].album_dir()).len() > 1)
                    .unwrap();
                self.extras(a.tracks[0].album_dir());
            }
            17 => {
                let g = self.gallery.borrow();
                let g = g.as_ref().unwrap();
                assert!(g.selected.get() > 0, "Gallery did not remember its page");
                g.window.close();
                let _ = self
                    .sender
                    .send(Command::Preference("meter".into(), "true".into()));
                self.now_tab.replace("details".into());
                let _ = self.sender.send(Command::View(View::Now));
            }
            18 => {
                assert!(self.experience.meter.borrow().is_some());
                self.window.set_default_size(640, 580);
                let a = self.albums.borrow()[0].clone();
                let _ = self.sender.send(Command::View(View::Album(a.key.clone())));
            }
            19 => {
                assert!(
                    self.window.width() <= 660,
                    "Album controls force a wide window: {}",
                    self.window.width()
                );
                let _ = self.sender.send(Command::View(View::Now));
            }
            20 => {
                assert!(
                    self.window.width() <= 660,
                    "Listening controls force a wide window"
                );
                self.now_tab.replace("lyrics".into());
                self.render();
            }
            21 => {
                let labels = widget_labels(self.window.upcast_ref());
                assert!(!labels.iter().any(|t| t == "Follow along"));
                self.preferences();
            }
            22 => {
                if let Some(d) = self.window.visible_dialog() {
                    self.smoke_released
                        .replace(vec![d.clone().upcast::<gtk::Widget>().downgrade()]);
                    d.close();
                }
            }
            23 => {
                assert!(
                    self.smoke_released
                        .borrow()
                        .iter()
                        .all(|w| w.upgrade().is_none()),
                    "Preferences retained after closing"
                );
                let path = dir.join("review.json");
                let mut result: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                result["released_widgets"] = true.into();
                result["experience"]="filters, random album, focus mode, remembered gallery, narrow layouts, preferences".into();
                std::fs::write(&path, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
                result["cover_transitions_started"] = self.motion.started.get().into();
                result["cover_transitions_completed"] = self.motion.completed.get().into();
                assert!(self.motion.started.get() > 0, "Cover transition never ran");
                assert!(!self.motion.is_active());
                std::fs::write(&path, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
                gtk::Settings::default()
                    .unwrap()
                    .set_gtk_enable_animations(false);
                let _ = self.sender.send(Command::View(View::Albums));
            }
            24 => {
                let a = self.albums.borrow()[0].clone();
                let _ = self.sender.send(Command::View(View::Album(a.key.clone())));
            }
            25 => {
                assert!(
                    !self.motion.is_active(),
                    "Reduced-motion setting was ignored"
                );
                let path = dir.join("review.json");
                let mut result: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                result["reduced_motion"] = "passed".into();
                result["bundled_fonts"] = "League Gothic, Source Sans 3".into();
                std::fs::write(path, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
                gtk::Settings::default()
                    .unwrap()
                    .set_gtk_enable_animations(true);
                let tracks = self.tracks.borrow().iter().take(4).cloned().collect();
                self.now_tab.replace("queue".into());
                let _ = self.sender.send(Command::Play(tracks, 0));
                let _ = self.sender.send(Command::View(View::Now));
            }
            26 => {
                let buttons = queue_down_buttons(self.content.upcast_ref());
                assert!(buttons.len() >= 3);
                buttons[1].emit_clicked();
            }
            27 => {
                let queue = self.player.borrow().queue();
                assert_eq!(queue.tracks[1].path, self.tracks.borrow()[2].path);
                assert_eq!(queue.tracks[2].path, self.tracks.borrow()[1].path);
                let _ = self.sender.send(Command::Pause);
                if let Some(a) = self.albums.borrow().iter().find(|a| a.title.len() > 70) {
                    let _ = self.sender.send(Command::View(View::Album(a.key.clone())));
                }
                gtk::Settings::default()
                    .unwrap()
                    .set_gtk_font_name(Some("Sans 22"));
            }
            28 => {
                assert!(
                    self.window.width() <= 660,
                    "Large text forced a wider window: {}",
                    self.window.width()
                );
                assert!(self.content.height() > 120, "Large text hid the content");
                for widget in [
                    self.favorite_button.upcast_ref::<gtk::Widget>(),
                    self.total.upcast_ref(),
                ] {
                    let bounds = widget.compute_bounds(&self.window).unwrap();
                    assert!(
                        bounds.x() + bounds.width() <= self.window.width() as f32 + 1.,
                        "Large text clipped a transport control"
                    );
                }
                let path = dir.join("review.json");
                let mut result: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                result["queue_button_reorder"] = "passed".into();
                result["large_text_narrow_album"] = "passed".into();
                std::fs::write(path, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
                let filter = crate::collection::Filter {
                    genre: "Missing destination fixture".into(),
                    ..Default::default()
                };
                let _ = self.sender.send(Command::Collection(filter));
                let _ = self.sender.send(Command::View(View::Albums));
            }
            29 => {
                assert!(
                    !self.motion.is_active(),
                    "Artwork retained without a destination"
                );
                let path = dir.join("review.json");
                let mut result: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                result["missing_cover_destination_released"] = true.into();
                std::fs::write(path, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
                self.save();
                self.app.quit();
            }
            4 => {
                self.save();
                self.app.quit();
            }
            _ => {}
        }
        self.smoke_phase.set(phase + 1);
    }
}
impl Ui {
    /// Visit every screen once for visual review; screenshots are named per phase.
    fn tour(&self, dir: &std::path::Path, phase: u32) {
        let covers: Vec<gtk::Picture> = descendants(self.content.upcast_ref())
            .into_iter()
            .filter(|w| w.has_css_class("album-cover") && w.is_mapped())
            .filter_map(|w| w.downcast::<gtk::Picture>().ok())
            .collect();
        let loaded = covers.iter().filter(|p| p.paintable().is_some()).count();
        let path = dir.join("tour.json");
        let mut report: serde_json::Value = std::fs::read(&path)
            .ok()
            .and_then(|v| serde_json::from_slice(&v).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        report[format!("screen_{phase}")] =
            serde_json::json!({"covers_visible": covers.len(), "covers_loaded": loaded});
        let _ = std::fs::write(&path, serde_json::to_vec_pretty(&report).unwrap());
        let send = |c| {
            let _ = self.sender.send(c);
        };
        match phase {
            0 => send(Command::View(View::Artists)),
            1 => {
                let artist = self
                    .albums
                    .borrow()
                    .iter()
                    .max_by_key(|a| {
                        self.albums
                            .borrow()
                            .iter()
                            .filter(|b| b.artist == a.artist)
                            .count()
                    })
                    .map(|a| a.artist.clone())
                    .unwrap_or_default();
                send(Command::View(View::Artist(artist)));
            }
            2 => {
                let requested = std::env::var("FLACKER_SMOKE_ALBUM").unwrap_or_default();
                let album = self
                    .albums
                    .borrow()
                    .iter()
                    .find(|a| a.title == requested)
                    .cloned()
                    .unwrap_or_else(|| self.albums.borrow()[0].clone());
                send(Command::Play(album.tracks.clone(), 0));
                send(Command::Pause);
                send(Command::View(View::Album(album.key.clone())));
            }
            3 => {
                // The album page marks the playing track, and only that one.
                let marked: Vec<String> = descendants(self.content.upcast_ref())
                    .into_iter()
                    .filter(|w| {
                        w.has_css_class("track-row") && w.has_css_class("current") && w.is_mapped()
                    })
                    .filter_map(|w| {
                        descendants(&w)
                            .into_iter()
                            .find(|l| l.has_css_class("track-title"))
                            .and_downcast::<gtk::Label>()
                            .map(|l| l.text().to_string())
                    })
                    .collect();
                let playing = self
                    .player
                    .borrow()
                    .current()
                    .map(|t| t.title)
                    .unwrap_or_default();
                assert_eq!(
                    marked,
                    vec![playing],
                    "Album page must mark exactly the playing track"
                );
                send(Command::View(View::Tracks))
            }
            4 => {
                if let Some(menu) = descendants(self.content.upcast_ref())
                    .into_iter()
                    .find(|w| w.has_css_class("track-menu"))
                    .and_downcast::<gtk::MenuButton>()
                {
                    menu.popup();
                }
            }
            5 => {
                if let Some(menu) = descendants(self.content.upcast_ref())
                    .into_iter()
                    .find(|w| w.has_css_class("track-menu"))
                    .and_downcast::<gtk::MenuButton>()
                {
                    menu.popdown();
                }
                send(Command::View(View::Favorites));
            }
            6 => {
                self.now_tab.replace("lyrics".into());
                send(Command::View(View::Now));
            }
            7 => {
                self.now_tab.replace("queue".into());
                self.render();
            }
            8 => {
                self.now_tab.replace("details".into());
                self.render();
            }
            9 => {
                // Remember the Now playing page, then change track: it must update in place.
                let stage = find_class(self.content.upcast_ref(), "listening-stage").unwrap();
                NOW_STAGE.with(|s| s.replace(Some(stage.downgrade())));
                send(Command::Next);
                send(Command::Presentation);
            }
            10 => {
                let stage = find_class(self.content.upcast_ref(), "listening-stage").unwrap();
                let kept = NOW_STAGE
                    .with(|s| s.borrow().as_ref().and_then(|w| w.upgrade()))
                    .is_some_and(|old| old == stage);
                assert!(kept, "Now playing was rebuilt on a track change");
                let title = find_class(&stage, "listening-title")
                    .and_downcast::<gtk::Label>()
                    .unwrap()
                    .text();
                let playing = self.player.borrow().current().unwrap().title;
                assert_eq!(title.as_str(), playing, "Now playing shows a stale title");
                report["now_updates_in_place"] = true.into();
                let _ = std::fs::write(&path, serde_json::to_vec_pretty(&report).unwrap());
                if let Some(f) = self.experience.focus.borrow().as_ref() {
                    screenshot_window(&f.window, &dir.join("focus.png")).unwrap();
                }
                if let Some(f) = self.experience.focus.take() {
                    f.window.close();
                }
                send(Command::View(View::Albums));
            }
            11 => {
                self.window.set_default_size(700, 620);
                send(Command::View(View::Artists));
            }
            12 => {
                let key = self.player.borrow().current().unwrap().album_key();
                send(Command::View(View::Album(key)));
            }
            13 => send(Command::View(View::Now)),
            14 => {
                self.window.set_default_size(1366, 768);
                send(Command::Theme("light".into()));
                send(Command::View(View::Artists));
            }
            15 => send(Command::View(View::Albums)),
            16 => {
                let key = self.player.borrow().current().unwrap().album_key();
                send(Command::View(View::Album(key)));
            }
            17 => self.preferences(),
            _ => {
                if let Some(d) = self.window.visible_dialog() {
                    d.close();
                }
                self.app.quit();
            }
        }
    }
    fn layout_review(&self, dir: &std::path::Path, phase: u32) {
        let report = dir.join("layout.json");
        let mut result: serde_json::Value = std::fs::read(&report)
            .ok()
            .and_then(|v| serde_json::from_slice(&v).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        match phase {
            0 => {
                assert!(
                    find_class(self.content.upcast_ref(), "collection-stage").is_none(),
                    "Empty queue has a featured album"
                );
                let requested = std::env::var("FLACKER_SMOKE_ALBUM").unwrap_or_default();
                let a = self
                    .albums
                    .borrow()
                    .iter()
                    .find(|a| a.title == requested)
                    .cloned()
                    .unwrap_or_else(|| self.albums.borrow()[0].clone());
                let _ = self.sender.send(Command::Play(a.tracks.clone(), 0));
                let _ = self.sender.send(Command::Pause);
                let _ = self.sender.send(Command::View(View::Albums));
                result["no_arbitrary_feature"] = true.into();
            }
            1..=3 => {
                let feature = find_class(self.content.upcast_ref(), "collection-stage").unwrap();
                assert!(
                    feature.height() <= 120,
                    "Featured album too tall: {}",
                    feature.height()
                );
                let labels = widget_labels(&feature);
                assert!(labels.iter().any(|s| s == "Explore the album"));
                assert!(!labels.iter().any(|s| s == "Play album"));
                let scroll = self.album_scroll.borrow().as_ref().unwrap().2.clone();
                let rows = full_album_rows(&scroll);
                let minimum = if phase == 3 { 3 } else { 2 };
                assert!(
                    rows >= minimum,
                    "Only {rows} complete rows at {}x{}",
                    self.window.width(),
                    self.window.height()
                );
                let count = assert_text_ink(self.window.upcast_ref());
                result[format!("layout_{phase}")] = serde_json::json!({"width":self.window.width(),"height":self.window.height(),"complete_rows":rows,"feature_height":feature.height(),"labels_checked":count});
                if phase == 1 {
                    self.window.set_default_size(1366, 768);
                } else if phase == 2 {
                    self.window.set_default_size(1400, 1080);
                    let _ = self
                        .sender
                        .send(Command::Preference("cover_size".into(), "0".into()));
                } else {
                    // Keep the current album visible even when a search excludes it.
                    let _ = self
                        .sender
                        .send(Command::Search("no matching album fixture".into()));
                }
            }
            4 => {
                assert!(find_class(self.content.upcast_ref(), "collection-stage").is_some());
                result["feature_survives_search"] = true.into();
                let _ = self.sender.send(Command::Search(String::new()));
                let key = self.player.borrow().current().unwrap().album_key();
                let _ = self.sender.send(Command::View(View::Album(key)));
            }
            5 => {
                result["album_labels_checked"] = assert_text_ink(self.window.upcast_ref()).into();
                // Stress number widths without changing the underlying track data.
                for widget in descendants(self.content.upcast_ref()) {
                    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
                        if label.has_css_class("track-number") {
                            label.set_text("12–103");
                        }
                        if label.has_css_class("track-time") {
                            label.set_text("123:45");
                        }
                    }
                }
            }
            6 => {
                result["long_numbers_checked"] = assert_text_ink(self.window.upcast_ref()).into();
                self.window.set_default_size(640, 580);
                gtk::Settings::default()
                    .unwrap()
                    .set_gtk_font_name(Some("Sans 22"));
            }
            7 => {
                assert!(
                    self.window.width() <= 660,
                    "Text scaling enlarged window to {}",
                    self.window.width()
                );
                result["large_text_labels_checked"] =
                    assert_text_ink(self.window.upcast_ref()).into();
                let key = self.player.borrow().current().unwrap().album_key();
                let _ = self.sender.send(Command::View(View::Album(key)));
            }
            8 => {
                // Compact navigation from a subpage: picking the current section goes back.
                assert!(self.nav_select.is_visible(), "Compact navigation not shown");
                assert_eq!(self.nav_select.label().as_deref(), Some("Albums"));
                assert!(matches!(*self.view.borrow(), View::Album(_)));
                let menu = self.nav_select.popover().unwrap();
                let entry = descendants(menu.upcast_ref())
                    .into_iter()
                    .filter_map(|w| w.downcast::<gtk::Button>().ok())
                    .find(|b| b.label().as_deref() == Some("Albums"))
                    .expect("Albums entry");
                entry.emit_clicked();
            }
            9 => {
                assert!(
                    matches!(*self.view.borrow(), View::Albums),
                    "Compact navigation did not return to Albums"
                );
                result["compact_navigation_reselects_section"] = true.into();
                result["result"] = "passed".into();
                self.app.quit();
            }
            _ => {}
        }
        std::fs::write(report, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    }
}
fn descendants(widget: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut result = vec![widget.clone()];
    let mut child = widget.first_child();
    while let Some(c) = child {
        result.extend(descendants(&c));
        child = c.next_sibling();
    }
    result
}
fn find_class(widget: &gtk::Widget, class: &str) -> Option<gtk::Widget> {
    descendants(widget)
        .into_iter()
        .find(|w| w.has_css_class(class))
}
fn full_album_rows(scroll: &gtk::ScrolledWindow) -> usize {
    let mut rows = Vec::<i32>::new();
    for tile in descendants(scroll.upcast_ref())
        .into_iter()
        .filter(|w| w.has_css_class("album-tile"))
    {
        let Some(bounds) = tile.compute_bounds(scroll) else {
            continue;
        };
        if bounds.y() >= 0. && bounds.y() + bounds.height() <= scroll.height() as f32 + 1. {
            let y = bounds.y().round() as i32;
            if !rows.iter().any(|v| (v - y).abs() < 3) {
                rows.push(y);
            }
        }
    }
    rows.len()
}
fn assert_text_ink(widget: &gtk::Widget) -> usize {
    let mut count = 0;
    for widget in descendants(widget) {
        let Some(label) = widget.downcast_ref::<gtk::Label>() else {
            continue;
        };
        if !label.is_mapped() || label.width() <= 0 || label.height() <= 0 {
            continue;
        }
        if ![
            "brand",
            "feature-title",
            "feature-artist",
            "album-title",
            "album-artist",
            "album-year",
            "track-number",
            "track-time",
            "track-title",
            "track-meta",
            "position",
            "display-time",
        ]
        .iter()
        .any(|class| label.has_css_class(class))
        {
            continue;
        }
        let (ink, _) = label.layout().pixel_extents();
        let (x, y) = label.layout_offsets();
        assert!(
            x + ink.x() >= -1
                && y + ink.y() >= -1
                && x + ink.x() + ink.width() <= label.width() + 1
                && y + ink.y() + ink.height() <= label.height() + 1,
            "Glyph clipping {:?} {:?}: ink {:?}, offset {:?}, allocation {}x{}",
            label.text(),
            label.css_classes(),
            ink,
            (x, y),
            label.width(),
            label.height()
        );
        count += 1;
    }
    assert!(count > 5, "No labels inspected");
    count
}

fn ephemeral_widgets(widget: &gtk::Widget) -> Vec<glib::WeakRef<gtk::Widget>> {
    let mut result = Vec::new();
    if let Some(b) = widget.downcast_ref::<gtk::Button>() {
        if b.label().as_deref() == Some("Play next") {
            result.push(widget.downgrade());
        }
    }
    if let Some(b) = widget.downcast_ref::<gtk::MenuButton>() {
        if let Some(p) = b.popover() {
            result.push(p.upcast::<gtk::Widget>().downgrade());
        }
    }
    let mut child = widget.first_child();
    while let Some(w) = child {
        result.extend(ephemeral_widgets(&w));
        child = w.next_sibling();
    }
    result
}
fn child_count(widget: &gtk::Widget) -> usize {
    let mut count = 0;
    let mut child = widget.first_child();
    while let Some(w) = child {
        count += 1;
        child = w.next_sibling();
    }
    count
}
fn widget_labels(widget: &gtk::Widget) -> Vec<String> {
    let mut labels = Vec::new();
    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
        labels.push(label.text().to_string());
    }
    let mut child = widget.first_child();
    while let Some(w) = child {
        labels.extend(widget_labels(&w));
        child = w.next_sibling();
    }
    labels
}
fn assert_fits(gallery: &crate::gallery::Gallery) {
    assert!(
        gallery.picture.paintable().is_some(),
        "Artwork did not load"
    );
    let bounds = gallery.picture.compute_bounds(&gallery.window).unwrap();
    assert!(bounds.x() >= 0. && bounds.y() >= 0.);
    assert!(bounds.x() + bounds.width() <= gallery.window.width() as f32 + 1.);
    assert!(bounds.y() + bounds.height() <= gallery.window.height() as f32 + 1.);
}
pub fn screenshot_window(
    window: &impl IsA<gtk::Window>,
    path: &std::path::Path,
) -> Result<(), String> {
    let window = window.as_ref();
    let child = window.child().ok_or("Content unavailable")?;
    let snapshot = gtk::Snapshot::new();
    snapshot.append_color(
        &gdk::RGBA::parse(if adw::StyleManager::default().is_dark() {
            "#111617"
        } else {
            "#e7ecee"
        })
        .unwrap(),
        &gtk::graphene::Rect::new(0., 0., window.width() as f32, window.height() as f32),
    );
    window.snapshot_child(&child, &snapshot);
    let node = snapshot.to_node().ok_or("Nothing rendered")?;
    let renderer = window.renderer().ok_or("Renderer unavailable")?;
    renderer
        .render_texture(&node, None)
        .save_to_png(path)
        .map_err(|e| e.to_string())
}

fn queue_down_buttons(widget: &gtk::Widget) -> Vec<gtk::Button> {
    let mut buttons = Vec::new();
    if widget.tooltip_text().as_deref() == Some("Move down") {
        if let Some(b) = widget.downcast_ref::<gtk::Button>() {
            buttons.push(b.clone());
        }
    }
    let mut child = widget.first_child();
    while let Some(w) = child {
        buttons.extend(queue_down_buttons(&w));
        child = w.next_sibling();
    }
    buttons
}

fn first_sleeve(widget: &gtk::Widget) -> Option<crate::scene::Sleeve> {
    if let Some(w) = widget.downcast_ref::<crate::scene::Sleeve>() {
        return Some(w.clone());
    }
    let mut child = widget.first_child();
    while let Some(w) = child {
        if let Some(s) = first_sleeve(&w) {
            return Some(s);
        }
        child = w.next_sibling();
    }
    None
}
impl Ui {
    // Opt-in recording of actual GTK frames, never enabled during ordinary use.
    fn record_motion(&self, dir: &std::path::Path) {
        let requested = std::env::var("FLACKER_SMOKE_ALBUM").unwrap_or_default();
        let albums = self.albums.borrow();
        let Some(album) = albums
            .iter()
            .find(|a| a.title == requested)
            .or_else(|| albums.iter().find(|a| a.tracks[0].has_lyrics))
            .or_else(|| albums.first())
            .cloned()
        else {
            return;
        };
        drop(albums);
        std::fs::create_dir_all(dir).unwrap();
        let dir = dir.to_owned();
        let root = self.content.downgrade();
        let sender = self.sender.clone();
        let now_tab = self.now_tab.clone();
        let app = self.app.downgrade();
        let start = Instant::now();
        let step = Cell::new(0);
        let frames = Rc::new(RefCell::new(Vec::<f64>::new()));
        let clock = self.window.frame_clock().unwrap();
        let captured = frames.clone();
        let destination = dir.clone();
        let window = self.window.downgrade();
        let previous = Cell::new(Instant::now() - Duration::from_secs(1));
        let handler = RefCell::new(Some(clock.connect_after_paint(move |_| {
            if previous.get().elapsed() < Duration::from_millis(50) {
                return;
            }
            if let Some(win) = window.upgrade() {
                let n = captured.borrow().len();
                screenshot_window(&win, &destination.join(format!("frame-{n:04}.png"))).unwrap();
                captured.borrow_mut().push(start.elapsed().as_secs_f64());
                previous.set(Instant::now());
            }
        })));
        self.window.add_tick_callback(move |win,_| {
            let Some(root)=root.upgrade() else{return glib::ControlFlow::Break;};
            let t=start.elapsed().as_secs_f64();
            if step.get()==0 && t>=0.25 {
                if let Some(s)=first_sleeve(root.upcast_ref()){s.pose(-0.9,-0.7,1.);}
                step.set(1);
            }
            if step.get()==1 && t>=0.85 {
                if let Some(s)=first_sleeve(root.upcast_ref()){s.pose(0.9,0.6,1.);}
                step.set(2);
            }
            if step.get()==2 && t>=1.45 {
                if let Some(s)=first_sleeve(root.upcast_ref()){s.pose(0.,0.,0.);}
                step.set(3);
            }
            if step.get()==3 && t>=1.9 {
                let _=sender.send(Command::View(View::Album(album.key.clone())));
                step.set(4);
            }
            if step.get()==4 && t>=2.8 {
                if let Some(s)=first_sleeve(root.upcast_ref()){s.pose(0.8,-0.7,1.);}
                step.set(5);
            }
            if step.get()==5 && t>=3.45 {
                if let Some(s)=first_sleeve(root.upcast_ref()){s.pose(0.,0.,0.);}
                step.set(6);
            }
            if step.get()==6 && t>=3.9 {
                let _=sender.send(Command::Play(album.tracks.clone(),0));
                let _=sender.send(Command::View(View::Now));
                step.set(7);
            }
            if step.get()==7 && t>=4.75 {
                now_tab.replace("queue".into());
                let _=sender.send(Command::View(View::Now));
                step.set(8);
            }
            if step.get()==8 && t>=5.5 {
                let _=sender.send(Command::View(View::Albums));
                step.set(9);
            }
            if t>=6.4 {
                if let Some(id)=handler.take(){clock.disconnect(id);}
                let times=frames.borrow();
                let mut concat=String::new();
                for i in 0..times.len() {
                    concat.push_str(&format!("file 'frame-{i:04}.png'\nduration {:.6}\n",times.get(i+1).map_or(0.06,|v|v-times[i])));
                }
                std::fs::write(dir.join("frames.ffconcat"),concat).unwrap();
                let report=serde_json::json!({"frames":times.len(),"seconds":t,"renderer":win.renderer().map(|r|r.type_().name().to_owned()),"note":"Native interaction recording, not a GPU performance benchmark"});
                std::fs::write(dir.join("capture.json"),serde_json::to_vec_pretty(&report).unwrap()).unwrap();
                if let Some(app)=app.upgrade(){app.quit();}
                return glib::ControlFlow::Break;
            }
            win.queue_draw();
            glib::ControlFlow::Continue
        });
    }
}
thread_local! {
    /// The Now playing page seen before a track change in the tour.
    static NOW_STAGE: std::cell::RefCell<Option<glib::WeakRef<gtk::Widget>>> =
        const { std::cell::RefCell::new(None) };
}
