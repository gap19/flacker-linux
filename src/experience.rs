//! Optional listening tools, kept separate from the main navigation and audio engine.
use super::*;
use crate::{
    collection::{Filter, year},
    history::History,
};

pub(super) struct Focus {
    pub window: adw::Window,
    pub title: gtk::Label,
    pub detail: gtk::Label,
    pub picture: gtk::Picture,
    pub elapsed: gtk::Label,
    pub total: gtk::Label,
    pub seek: gtk::Scale,
    pub seeking: Rc<Cell<bool>>,
    pub touched: Rc<Cell<Option<Instant>>>,
    pub play: gtk::Button,
    pub favorite: gtk::Button,
    pub serial: Cell<u64>,
    pub words: gtk::TextView,
    pub queue: gtk::Box,
    pub queue_revision: Cell<u64>,
}
pub(super) struct Experience {
    pub history: RefCell<History>,
    pub tick: Cell<Instant>,
    pub listened: Cell<f64>,
    pub serial: Cell<u64>,
    pub recorded: Cell<bool>,
    pub focus: RefCell<Option<Focus>>,
    pub meter: RefCell<Option<gtk::DrawingArea>>,
    pub levels: Rc<Cell<[f32; 4]>>,
    pub undo_toast: RefCell<Option<adw::Toast>>,
}
impl Default for Experience {
    fn default() -> Self {
        Self {
            history: RefCell::new(History::load()),
            tick: Cell::new(Instant::now()),
            listened: Cell::new(0.),
            serial: Cell::new(0),
            recorded: Cell::new(false),
            focus: RefCell::new(None),
            meter: RefCell::new(None),
            levels: Rc::new(Cell::new([0.; 4])),
            undo_toast: RefCell::new(None),
        }
    }
}
impl Ui {
    pub(super) fn filtered_albums(&self) -> Vec<Rc<Album>> {
        let settings = self.settings.borrow();
        let mut albums = settings.experience.filter.albums(
            &self.albums.borrow(),
            &self.favorite_paths.borrow(),
            &self.query.borrow(),
        );
        if let View::Artist(artist) = &*self.view.borrow() {
            albums.retain(|a| &a.artist == artist);
        }
        albums
    }
    pub(super) fn collection_controls(&self, heading: &gtk::Box) {
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        controls.set_valign(gtk::Align::Center);
        let random = icon_button(
            "media-playlist-shuffle-symbolic",
            "Play a random album from this selection, in order",
        );
        connect(&random, &self.sender, Command::RandomAlbum);
        if matches!(*self.view.borrow(), View::Albums | View::Artist(_)) {
            controls.append(&random);
        }
        let button = gtk::MenuButton::builder()
            .label("Filter")
            .tooltip_text("Filter and sort the collection")
            .build();
        button.update_property(&[gtk::accessible::Property::Label(
            "Filter and sort the collection",
        )]);
        let pop = gtk::Popover::new();
        let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
        for side in [
            gtk::PositionType::Left,
            gtk::PositionType::Right,
            gtk::PositionType::Top,
            gtk::PositionType::Bottom,
        ] {
            match side {
                gtk::PositionType::Left => body.set_margin_start(16),
                gtk::PositionType::Right => body.set_margin_end(16),
                gtk::PositionType::Top => body.set_margin_top(16),
                _ => body.set_margin_bottom(16),
            }
        }
        body.set_width_request(300);
        let f = self.settings.borrow().experience.filter.clone();
        let mut genres: Vec<_> = self
            .tracks
            .borrow()
            .iter()
            .flat_map(|t| t.genres.clone())
            .filter(|s| !s.trim().is_empty())
            .collect();
        genres.sort_by_key(|s| s.to_lowercase());
        // "Rock" and "rock" are one genre in the filter.
        genres.dedup_by(|a, b| a.to_lowercase() == b.to_lowercase());
        genres.insert(0, String::new());
        let mut years: Vec<_> = self
            .tracks
            .borrow()
            .iter()
            .flat_map(|t| {
                [
                    year(&t.date).to_string(),
                    year(&t.original_date).to_string(),
                ]
            })
            .filter(|s| !s.is_empty())
            .collect();
        years.sort();
        years.dedup();
        years.insert(0, String::new());
        let make = |title: &str, values: &[String], selected: usize| {
            body.append(&label(title, "subtitle"));
            let list: Vec<_> = values.iter().map(String::as_str).collect();
            let d = gtk::DropDown::from_strings(&list);
            d.set_selected(selected as u32);
            d.set_enable_search(true);
            body.append(&d);
            d
        };
        let genre_names: Vec<_> = genres
            .iter()
            .map(|s| {
                if s.is_empty() {
                    "All genres".into()
                } else {
                    s.clone()
                }
            })
            .collect();
        let genre = make(
            "Genre in tags",
            &genre_names,
            genres.iter().position(|s| *s == f.genre).unwrap_or(0),
        );
        let year_names: Vec<_> = years
            .iter()
            .map(|s| {
                if s.is_empty() {
                    "All years".into()
                } else {
                    s.clone()
                }
            })
            .collect();
        let date = make(
            "Year",
            &year_names,
            years.iter().position(|s| *s == f.year).unwrap_or(0),
        );
        let original = gtk::CheckButton::with_label("Filter by original year");
        original.set_active(f.original_year);
        body.append(&original);
        let quality = make(
            "Resolution",
            &[
                "All".into(),
                "High resolution (>16 bit or >48 kHz)".into(),
                "Up to 16 bit / 48 kHz".into(),
            ],
            f.quality.min(2) as usize,
        );
        let favorite = gtk::CheckButton::with_label("With favorite tracks");
        favorite.set_active(f.favorites);
        body.append(&favorite);
        let sort = make(
            "Sort albums",
            &[
                "Artist and edition".into(),
                "Album title".into(),
                "Edition year".into(),
                "Original year".into(),
            ],
            f.sort.min(3) as usize,
        );
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let clear = gtk::Button::with_label("Clear filters");
        let apply = gtk::Button::with_label("Apply");
        apply.add_css_class("suggested-action");
        actions.append(&clear);
        actions.append(&apply);
        body.append(&actions);
        let s = self.sender.clone();
        let p = pop.downgrade();
        apply.connect_clicked(move |_| {
            let _ = s.send(Command::Collection(Filter {
                genre: genres
                    .get(genre.selected() as usize)
                    .cloned()
                    .unwrap_or_default(),
                year: years
                    .get(date.selected() as usize)
                    .cloned()
                    .unwrap_or_default(),
                original_year: original.is_active(),
                quality: quality.selected(),
                favorites: favorite.is_active(),
                sort: sort.selected(),
            }));
            if let Some(p) = p.upgrade() {
                p.popdown();
            }
        });
        let s = self.sender.clone();
        let p = pop.downgrade();
        clear.connect_clicked(move |_| {
            let _ = s.send(Command::Collection(Filter::default()));
            if let Some(p) = p.upgrade() {
                p.popdown();
            }
        });
        if !f.genre.is_empty() || !f.year.is_empty() || f.quality != 0 || f.favorites {
            button.add_css_class("active");
        }
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(430)
            .child(&body)
            .build();
        pop.set_child(Some(&scroll));
        button.set_popover(Some(&pop));
        controls.append(&button);
        heading.append(&controls);
    }
    pub(super) fn recent_page(&self) {
        let h = self.heading("Recently played", "History stays on this computer");
        h.append(&action(
            "Clear",
            "edit-clear-all-symbolic",
            &self.sender,
            Command::ClearHistory,
        ));
        if !self.settings.borrow().experience.history {
            self.empty(
                "History is off",
                "Turn on local history in Preferences to keep your next listens.",
            );
            return;
        }
        let history = self.experience.history.borrow();
        let shelf = self.albums.borrow();
        let all: std::collections::HashMap<&str, &Rc<Album>> =
            shelf.iter().map(|a| (a.key.as_str(), a)).collect();
        let mut seen = std::collections::HashSet::new();
        let albums = history
            .entries
            .iter()
            .filter(|e| seen.insert(e.album_key.as_str()))
            .filter_map(|e| all.get(e.album_key.as_str()).map(|a| Rc::clone(a)))
            .collect::<Vec<_>>();
        if albums.is_empty() {
            self.empty(
                "Nothing played yet",
                "Albums you listen to for a while appear here.",
            );
            return;
        }
        self.album_grid(albums);
    }
    pub(super) fn lyrics_reader(&self) -> gtk::Box {
        let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let source = label(&self.lyrics.borrow().source, "subtitle");
        source.set_wrap(true);
        body.append(&source);
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        toolbar.add_css_class("lyrics-tools");
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search these lyrics"));
        search.set_hexpand(true);
        search.set_width_chars(14);
        toolbar.append(&search);
        let next = icon_button("go-down-symbolic", "Next match in the lyrics");
        toolbar.append(&next);
        let minus = icon_button("zoom-out-symbolic", "Smaller lyrics text");
        toolbar.append(&minus);
        let plus = icon_button("zoom-in-symbolic", "Larger lyrics text");
        toolbar.append(&plus);
        body.append(&toolbar);
        let words = gtk::TextView::new();
        words.set_editable(false);
        words.set_cursor_visible(false);
        words.set_wrap_mode(gtk::WrapMode::WordChar);
        words.add_css_class("lyrics-text");
        words.set_top_margin(14);
        words.set_bottom_margin(24);
        words.set_left_margin(8);
        words.set_right_margin(8);
        let text = self.lyrics.borrow();
        words.buffer().set_text(if text.text.is_empty() {
            "No local lyrics for this track."
        } else {
            &text.text
        });
        drop(text);
        words.update_property(&[gtk::accessible::Property::Label("Static song lyrics")]);
        let size = Rc::new(Cell::new(
            self.settings.borrow().experience.lyrics_size.clamp(14, 30),
        ));
        let tag = gtk::TextTag::builder()
            .size_points(size.get() as f64 * 0.75)
            .build();
        words.buffer().tag_table().add(&tag);
        let b = words.buffer();
        b.apply_tag(&tag, &b.start_iter(), &b.end_iter());
        for (button, delta) in [(&minus, -1), (&plus, 1)] {
            let tag = tag.clone();
            let size = size.clone();
            let s = self.sender.clone();
            button.connect_clicked(move |_| {
                size.set((size.get() as i32 + delta).clamp(14, 30) as u32);
                tag.set_size_points(size.get() as f64 * 0.75);
                let _ = s.send(Command::Preference(
                    "lyrics_size".into(),
                    size.get().to_string(),
                ));
            });
        }
        let w = words.downgrade();
        let entry = search.clone();
        next.connect_clicked(move |_| {
            if let Some(w) = w.upgrade() {
                find_words(&w, entry.text().as_str(), true);
            }
        });
        let w = words.downgrade();
        search.connect_search_changed(move |e| {
            if let Some(w) = w.upgrade() {
                find_words(&w, e.text().as_str(), false);
            }
        });
        let w = words.downgrade();
        search.connect_activate(move |e| {
            if let Some(w) = w.upgrade() {
                find_words(&w, e.text().as_str(), true);
            }
        });
        self.lyrics_view
            .replace(Some((source.clone(), words.clone(), tag.clone())));
        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(
            (self.settings.borrow().experience.lyrics_width.clamp(40, 80) * 10) as i32,
        );
        clamp.set_tightening_threshold(380);
        clamp.set_child(Some(&words));
        clamp.set_vexpand(true);
        body.append(&scrolled(&clamp));
        body
    }
    pub(super) fn inspector(&self, track: Track) {
        // One edition window at a time: a new one replaces the previous.
        EDITION.with(|e| {
            if let Some(w) = e.borrow_mut().take().and_then(|w| w.upgrade()) {
                w.close();
            }
        });
        let win = adw::Window::builder()
            .transient_for(&self.window)
            .destroy_with_parent(true)
            .title("Edition details — FLACker")
            .default_width(640)
            .default_height(620)
            .build();
        presentation::theme(&win);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.add_css_class("main-surface");
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&label("Edition details", "brand")));
        body.append(&header);
        let text = gtk::TextView::new();
        text.add_css_class("edition-text");
        text.set_editable(false);
        text.set_cursor_visible(false);
        text.set_wrap_mode(gtk::WrapMode::WordChar);
        text.set_left_margin(24);
        text.set_right_margin(24);
        text.set_top_margin(20);
        text.set_bottom_margin(24);
        text.buffer().set_text("Reading local metadata…");
        body.append(&scrolled(&text));
        win.set_content(Some(&body));
        let escape = gtk::ShortcutController::new();
        escape.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("Escape"),
            Some(gtk::NamedAction::new("window.close")),
        ));
        win.add_controller(escape);
        EDITION.with(|e| e.replace(Some(win.downgrade())));
        win.present();
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let result=crate::flac::read(&track.path,false).map(|m| {
                let get=|keys:&[&str]|keys.iter().find_map(|k|m.tags.get(*k)).map(|v|v.join("; ")).filter(|s|!s.trim().is_empty()).unwrap_or_else(||"Not provided".into());
                let mut result=format!("{}\n{}\n\nOriginal year/date: {}\nThis edition's date: {}\nLabel: {}\nCatalog number: {}\nGenre: {}\nEdition ID: {}\n\nCredits\nComposer: {}\nPerformers: {}\nProducer: {}\nEngineer: {}\n\nFile\n{}\n{} · {}\nTrack ReplayGain: {}\nAlbum ReplayGain: {}\n\n",track.album,track.album_artist,get(&["ORIGINALDATE","ORIGINALYEAR"]),get(&["DATE"]),get(&["LABEL","ORGANIZATION","PUBLISHER"]),get(&["CATALOGNUMBER"]),get(&["GENRE"]),get(&["RELEASE_ID","MUSICBRAINZ_ALBUMID"]),get(&["COMPOSER"]),get(&["PERFORMER","ARTIST"]),get(&["PRODUCER"]),get(&["ENGINEER"]),track.path.display(),track.quality(),presentation::count(track.channels as usize, "channel", "channels"),get(&["REPLAYGAIN_TRACK_GAIN"]),get(&["REPLAYGAIN_ALBUM_GAIN"]));
                result.push_str("Other local tags\n");let mut tags:Vec<_>=m.tags.into_iter().collect();tags.sort_by(|a,b|a.0.cmp(&b.0));
                for (key,values) in tags {if result.len()>128*1024 {result.push_str("\nDisplay limited to 128 KiB.");break;}result.push_str(&format!("{key}: {}\n",values.join("; ").chars().take(4096).collect::<String>()));}
                result
            }).unwrap_or_else(|e|format!("Could not read the edition details: {e}"));
            let _ = tx.send(result);
        });
        let w = win.downgrade();
        glib::timeout_add_local(Duration::from_millis(100), move || {
            if w.upgrade().is_none_or(|w| !w.is_visible()) {
                return glib::ControlFlow::Break;
            }
            if let Ok(result) = rx.try_recv() {
                text.buffer().set_text(&result);
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }
    pub(super) fn open_focus(&self) {
        if let Some(f) = self.experience.focus.borrow().as_ref() {
            if f.window.is_visible() {
                f.window.present();
                return;
            }
        }
        if let Some(f) = self.experience.focus.take() {
            f.window.close();
        }
        let win = adw::Window::builder()
            .transient_for(&self.window)
            .destroy_with_parent(true)
            .title("Focus Mode — FLACker")
            .default_width(1000)
            .default_height(760)
            .build();
        presentation::theme(&win);
        win.add_css_class("focus-window");
        win.add_css_class(if adw::StyleManager::default().is_dark() {
            "dark"
        } else {
            "light"
        });
        win.set_size_request(640, 580);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&label("Focus Mode", "brand")));
        body.append(&header);
        let core = gtk::Box::new(gtk::Orientation::Horizontal, 36);
        core.add_css_class("focus-core");
        core.set_vexpand(true);
        let picture = gtk::Picture::new();
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_can_shrink(true);
        let art = presentation::fading_cover(&picture, 300);
        art.set_hexpand(true);
        art.set_vexpand(true);
        core.append(&art);
        let info = gtk::Box::new(gtk::Orientation::Vertical, 10);
        info.set_hexpand(true);
        info.set_valign(gtk::Align::Center);
        let title = presentation::title("", "focus-title");
        title.set_max_width_chars(22);
        let detail = ellipsis("", "subtitle");
        detail.set_max_width_chars(34);
        info.append(&title);
        info.append(&detail);
        let timeline = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        timeline.add_css_class("focus-timeline");
        let elapsed = label("0:00", "position");
        let total = label("0:00", "position");
        let seek = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0., 1., 0.1);
        seek.set_increments(5., 30.);
        seek.set_draw_value(false);
        seek.set_hexpand(true);
        seek.update_property(&[gtk::accessible::Property::Label("Track position")]);
        let seeking = Rc::new(Cell::new(false));
        let touched = Rc::new(Cell::new(None));
        super::seek_on_release(&seek, &self.sender, seeking.clone(), touched.clone());
        timeline.append(&elapsed);
        timeline.append(&seek);
        timeline.append(&total);
        info.append(&timeline);
        let transport = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        transport.add_css_class("transport");
        let prev = icon_button("media-skip-backward-symbolic", "Previous track");
        connect(&prev, &self.sender, Command::Previous);
        let play = icon_button("media-playback-start-symbolic", "Play or pause");
        play.add_css_class("play");
        connect(&play, &self.sender, Command::Toggle);
        let next = icon_button("media-skip-forward-symbolic", "Next track");
        connect(&next, &self.sender, Command::Next);
        let favorite = icon_button("non-starred-symbolic", "Favorite the current track");
        connect(&favorite, &self.sender, Command::Favorite);
        transport.append(&prev);
        transport.append(&play);
        transport.append(&next);
        transport.append(&favorite);
        info.append(&transport);
        core.append(&info);
        body.append(&scrolled(&core));
        let bp = adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 760px").unwrap());
        bp.add_setter(
            &core,
            "orientation",
            Some(&gtk::Orientation::Vertical.to_value()),
        );
        bp.add_setter(&art, "height-request", Some(&160i32.to_value()));
        bp.add_setter(&art, "width-request", Some(&160i32.to_value()));
        win.add_breakpoint(bp);
        let reveal = gtk::Revealer::new();
        reveal.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        reveal.set_transition_duration(220);
        let reader = gtk::TextView::new();
        reader.set_editable(false);
        reader.set_cursor_visible(false);
        reader.set_wrap_mode(gtk::WrapMode::WordChar);
        reader.add_css_class("lyrics-text");
        reader.set_left_margin(24);
        reader.set_right_margin(24);
        let stack = gtk::Stack::new();
        let scroll = scrolled(&reader);
        scroll.set_min_content_height(180);
        stack.add_titled(&scroll, Some("lyrics"), "Lyrics");
        let queue = gtk::Box::new(gtk::Orientation::Vertical, 0);
        queue.set_vexpand(true);
        queue.append(&self.queue_list(self.player.borrow().queue()));
        stack.add_titled(&queue, Some("queue"), "Queue");
        let switch = gtk::StackSwitcher::new();
        switch.set_stack(Some(&stack));
        let panels = gtk::Box::new(gtk::Orientation::Vertical, 12);
        panels.add_css_class("listening-panel");
        panels.append(&switch);
        panels.append(&stack);
        reveal.set_child(Some(&panels));
        let toggle = gtk::ToggleButton::with_label("Lyrics and queue");
        toggle.set_tooltip_text(Some("Show or hide lyrics and queue"));
        let r = reveal.clone();
        toggle.connect_toggled(move |b| r.set_reveal_child(b.is_active()));
        header.pack_start(&toggle);
        info.append(&reveal);
        let full = icon_button("view-fullscreen-symbolic", "Fullscreen (F11)");
        let w = win.downgrade();
        full.connect_clicked(move |_| {
            if let Some(w) = w.upgrade() {
                w.set_fullscreened(!w.is_fullscreen());
            }
        });
        header.pack_end(&full);
        let keys = gtk::EventControllerKey::new();
        let w = win.downgrade();
        let s = self.sender.clone();
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(w) = w.upgrade() else {
                return glib::Propagation::Proceed;
            };
            match key {
                gdk::Key::Escape if w.is_fullscreen() => w.unfullscreen(),
                gdk::Key::Escape => w.close(),
                gdk::Key::F11 => w.set_fullscreened(!w.is_fullscreen()),
                gdk::Key::space if modifiers.is_empty() => {
                    let _ = s.send(Command::Toggle);
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        win.add_controller(keys);
        body.set_margin_bottom(16);
        win.set_content(Some(&crate::scene::Stage::new(&body, &picture)));
        win.present();
        self.experience.focus.replace(Some(Focus {
            window: win,
            title,
            detail,
            picture,
            elapsed,
            total,
            seek,
            seeking,
            touched,
            play,
            favorite,
            serial: Cell::new(u64::MAX),
            words: reader,
            queue,
            queue_revision: Cell::new(u64::MAX),
        }));
    }
    pub(super) fn meter_widget(&self) -> gtk::DrawingArea {
        let area = gtk::DrawingArea::new();
        area.set_content_width(240);
        area.set_content_height(68);
        area.set_hexpand(true);
        area.set_tooltip_text(Some(
            "PCM peak and RMS at the app output, sampled periodically before the audio server",
        ));
        area.update_property(&[gtk::accessible::Property::Label(
            "Stereo peak and RMS meter",
        )]);
        let levels = self.experience.levels.clone();
        area.set_draw_func(move |_, cr, width, _| {
            let values = levels.get();
            for c in 0..2 {
                let y = 10. + c as f64 * 28.;
                cr.set_source_rgb(0.5, 0.65, 0.66);
                cr.move_to(2., y + 12.);
                let _ = cr.show_text(if c == 0 { "L" } else { "R" });
                let span = (width - 40).max(20) as f64;
                for segment in 0..32 {
                    let threshold = -60. + segment as f64 * 60. / 32.;
                    let rms = 20. * (values[c + 2] as f64).max(0.000001).log10();
                    let peak = 20. * (values[c] as f64).max(0.000001).log10();
                    let on = rms >= threshold;
                    let marker = peak >= threshold && peak < threshold + 60. / 32.;
                    if on || marker {
                        if segment > 28 {
                            cr.set_source_rgb(0.87, 0.59, 0.30);
                        } else {
                            cr.set_source_rgb(0.45, 0.77, 0.78);
                        }
                    } else {
                        cr.set_source_rgba(0.5, 0.65, 0.66, 0.15);
                    }
                    cr.rectangle(
                        24. + segment as f64 * span / 32.,
                        y,
                        (span / 32. - 2.).max(1.),
                        14.,
                    );
                    let _ = cr.fill();
                }
            }
        });
        area
    }
    pub(super) fn experience_tick(&self) {
        let now = Instant::now();
        let dt = now
            .duration_since(self.experience.tick.replace(now))
            .as_secs_f64()
            // Ticks are 0.15–1 s apart; a longer gap (a stalled loop) does not count.
            .min(1.5);
        let p = self.player.borrow();
        // Only a different track restarts the listening count: seeking keeps the same play.
        let serial = p.track_serial();
        if self.experience.serial.replace(serial) != serial {
            self.experience.listened.set(0.);
            self.experience.recorded.set(false);
        }
        if self.settings.borrow().experience.history && p.playing && !self.experience.recorded.get()
        {
            self.experience
                .listened
                .set(self.experience.listened.get() + dt);
            if let Some(t) = p.current() {
                if self.experience.listened.get() >= 30f64.min(t.duration / 2.) {
                    self.experience.history.borrow_mut().record(&t);
                    self.experience.recorded.set(true);
                }
            }
        }
        let visible = self
            .experience
            .meter
            .borrow()
            .as_ref()
            .is_some_and(|w| w.is_mapped());
        let meter = self.settings.borrow().experience.meter && visible && p.playing;
        p.meter.enabled.store(meter, Ordering::Relaxed);
        let levels = if meter { p.meter.values() } else { [0.; 4] };
        if levels != self.experience.levels.replace(levels) {
            if let Some(w) = self.experience.meter.borrow().as_ref() {
                w.queue_draw();
            }
        }
        if let Some(m) = self.experience.focus.borrow().as_ref() {
            if m.window.is_visible() {
                // The queue list follows the current track and any change to the queue itself.
                let revision = serial.wrapping_mul(1_000_003) ^ p.queue_len() as u64;
                if m.queue_revision.replace(revision) != revision {
                    while let Some(c) = m.queue.first_child() {
                        m.queue.remove(&c);
                    }
                    m.queue.append(&self.queue_list(p.queue()));
                }
                let t = p.current();
                let changed = serial.wrapping_add(t.as_ref().map_or(0, |t| t.modified));
                if m.serial.replace(changed) != changed {
                    if let Some(t) = t.as_ref() {
                        m.title.set_text(&t.title);
                        m.detail.set_text(&format!("{} · {}", t.artist, t.album));
                        self.art.borrow_mut().request(
                            &m.picture,
                            t,
                            self.settings.borrow().prefer_embedded,
                            680,
                        );
                        let text = self.lyrics.borrow();
                        m.words.buffer().set_text(if text.text.is_empty() {
                            "No local lyrics for this track."
                        } else {
                            &text.text
                        });
                        // Same lyrics size as the reader in the main window.
                        lyrics_size(
                            &m.words,
                            self.settings.borrow().experience.lyrics_size.clamp(14, 30),
                        );
                    } else {
                        m.title.set_text("Nothing playing");
                        m.detail.set_text("Choose an album from the collection");
                        // No stale cover or lyrics from the last track.
                        m.picture.set_paintable(gtk::gdk::Paintable::NONE);
                        m.words.buffer().set_text("");
                    }
                }
                let favorite = t
                    .as_ref()
                    .is_some_and(|t| self.favorite_paths.borrow().contains(&t.path));
                set_icon(
                    &m.favorite,
                    if favorite {
                        "starred-symbolic"
                    } else {
                        "non-starred-symbolic"
                    },
                );
                let (position, duration) = (p.position(), p.duration());
                m.elapsed.set_text(&time(position));
                m.total.set_text(&time(duration));
                m.seeking.set(true);
                if (m.seek.adjustment().upper() - duration.max(1.)).abs() > 0.001 {
                    m.seek.set_range(0., duration.max(1.));
                }
                if super::scale_free(&m.touched) && (m.seek.value() - position).abs() > 0.01 {
                    m.seek.set_value(position);
                }
                m.seeking.set(false);
                set_icon(
                    &m.play,
                    if p.playing {
                        "media-playback-pause-symbolic"
                    } else {
                        "media-playback-start-symbolic"
                    },
                );
            }
        }
        drop(p);
    }
    pub(super) fn extra_preferences(&self, page: &adw::PreferencesPage) {
        let g = adw::PreferencesGroup::builder()
            .title("Your way of listening")
            .build();
        let settings = self.settings.borrow();
        let p = &settings.experience;
        for (key, title, subtitle, on) in [
            (
                "meter",
                "Stereo meter",
                "Periodic peak and RMS; active only while visible",
                p.meter,
            ),
            (
                "history",
                "Keep local history",
                "Up to 500 listens; counted after 30 seconds or half the track",
                p.history,
            ),
        ] {
            let row = adw::SwitchRow::builder()
                .title(title)
                .subtitle(subtitle)
                .active(on)
                .build();
            let sender = self.sender.clone();
            row.connect_active_notify(move |r| {
                let _ = sender.send(Command::Preference(key.into(), r.is_active().to_string()));
            });
            g.add(&row);
        }
        for (key, title, options, selected) in [
            (
                "cover_size",
                "Cover size",
                vec!["Compact", "Medium", "Large"],
                p.cover_size.min(2),
            ),
            (
                "clock",
                "Time shown after the timeline",
                vec![
                    "Track duration",
                    "Track time remaining",
                    "Album time remaining in the queue",
                ],
                p.clock.min(2),
            ),
            (
                "lyrics_width",
                "Lyrics width",
                vec!["Narrow", "Comfortable", "Wide"],
                if p.lyrics_width <= 48 {
                    0
                } else if p.lyrics_width <= 64 {
                    1
                } else {
                    2
                },
            ),
        ] {
            let row = adw::ComboRow::builder()
                .title(title)
                .model(&gtk::StringList::new(&options))
                .selected(selected)
                .build();
            let s = self.sender.clone();
            row.connect_selected_notify(move |r| {
                let _ = s.send(Command::Preference(key.into(), r.selected().to_string()));
            });
            g.add(&row);
        }
        g.add(&action(
            "Clear local history",
            "edit-clear-all-symbolic",
            &self.sender,
            Command::ClearHistory,
        ));
        page.add(&g);
        let audio=adw::PreferencesGroup::builder().title("Audio output").description("Uses the audio server's local outputs. Switching keeps the position and briefly reopens the device.").build();
        let devices = adw::ComboRow::builder()
            .title("Device")
            .model(&gtk::StringList::new(&["Looking for outputs…"]))
            .sensitive(false)
            .build();
        audio.add(&devices);
        page.add(&audio);
        let saved = p.output.clone();
        drop(settings);
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = tx.send(crate::player::outputs());
        });
        let d = devices.downgrade();
        let sender = self.sender.clone();
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let Some(d) = d.upgrade() else {
                return glib::ControlFlow::Break;
            };
            match rx.try_recv() {
                Ok(mut devices) => {
                    if !saved.is_empty() && !devices.iter().any(|(id, _)| *id == saved) {
                        devices.push((saved.clone(), "Saved output unavailable".into()));
                    }
                    let names: Vec<_> = devices.iter().map(|(_, name)| name.as_str()).collect();
                    d.set_model(Some(&gtk::StringList::new(&names)));
                    d.set_selected(
                        devices.iter().position(|(id, _)| *id == saved).unwrap_or(0) as u32
                    );
                    d.set_sensitive(true);
                    let s = sender.clone();
                    d.connect_selected_notify(move |d| {
                        if let Some((id, _)) = devices.get(d.selected() as usize) {
                            let _ = s.send(Command::Preference("output".into(), id.clone()));
                        }
                    });
                    glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    d.set_model(Some(&gtk::StringList::new(&["Could not list the outputs"])));
                    glib::ControlFlow::Break
                }
                Err(_) => glib::ControlFlow::Continue,
            }
        });
    }
}
fn find_words(words: &gtk::TextView, needle: &str, next: bool) {
    if needle.is_empty() {
        return;
    }
    let b = words.buffer();
    let start = if next {
        b.selection_bounds()
            .map(|(_, end)| end)
            .unwrap_or_else(|| b.start_iter())
    } else {
        b.start_iter()
    };
    let flags = gtk::TextSearchFlags::CASE_INSENSITIVE | gtk::TextSearchFlags::TEXT_ONLY;
    if let Some((mut a, z)) = start
        .forward_search(needle, flags, None)
        .or_else(|| b.start_iter().forward_search(needle, flags, None))
    {
        b.select_range(&a, &z);
        words.scroll_to_iter(&mut a, 0.15, false, 0., 0.);
    }
}

thread_local! {
    /// The open edition window, if any.
    static EDITION: RefCell<Option<glib::WeakRef<adw::Window>>> = const { RefCell::new(None) };
}

/// Apply the lyrics text size to a whole text view.
fn lyrics_size(view: &gtk::TextView, size: u32) {
    let buffer = view.buffer();
    let table = buffer.tag_table();
    let tag = table.lookup("lyrics-size").unwrap_or_else(|| {
        let t = gtk::TextTag::new(Some("lyrics-size"));
        table.add(&t);
        t
    });
    tag.set_size_points(size as f64 * 0.75);
    buffer.apply_tag(&tag, &buffer.start_iter(), &buffer.end_iter());
}
/// Set a button's icon only when it differs, so the per-tick refresh does not rebuild it.
fn set_icon(button: &gtk::Button, name: &str) {
    if button.icon_name().as_deref() != Some(name) {
        button.set_icon_name(name);
    }
}
