//! Native screens. All actions use the existing command channel.
use super::*;

/// Device and negotiated format from the player's output summary.
/// A track row in any list, with the file it shows now (rows are recycled while scrolling).
/// The parts of the Now playing page that change with the track.
pub(super) struct NowParts {
    stage: glib::WeakRef<gtk::Widget>,
    compact: bool,
    track: Rc<RefCell<Track>>,
    picture: gtk::Picture,
    title: gtk::Label,
    subtitle: Vec<gtk::Label>,
    quality: gtk::Label,
    queue: gtk::Box,
    details: gtk::Box,
}
/// A labelled button whose command is built from the track it currently shows.
fn connect_current(
    button: &gtk::Button,
    sender: &Sender<Command>,
    track: &Rc<RefCell<Track>>,
    command: fn(&Track) -> Command,
) {
    let sender = sender.clone();
    let track = track.clone();
    button.connect_clicked(move |_| {
        let _ = sender.send(command(&track.borrow()));
    });
}
pub(super) type TrackRow = (glib::WeakRef<gtk::Box>, Rc<RefCell<Option<PathBuf>>>);

/// Mark the row of the playing file and show its favorite star; announced to screen readers.
pub(super) fn mark_row(
    row: &gtk::Box,
    path: Option<&std::path::Path>,
    current: Option<&std::path::Path>,
    favorites: &HashSet<PathBuf>,
) {
    let playing = path.is_some() && path == current;
    if playing {
        row.add_css_class("current");
        row.update_property(&[gtk::accessible::Property::Description("Now playing")]);
    } else {
        row.remove_css_class("current");
        row.reset_property(gtk::AccessibleProperty::Description);
    }
    // The star sits after the title block: number, texts, star, time, menu.
    if let Some(star) = row
        .first_child()
        .and_then(|n| n.next_sibling())
        .and_then(|t| t.next_sibling())
    {
        star.set_visible(path.is_some_and(|p| favorites.contains(p)));
    }
}

pub(super) fn output_parts(output: &str) -> (String, String) {
    if output.is_empty() {
        return (
            "Reported when playback starts".into(),
            "Reported when playback starts".into(),
        );
    }
    let mut device = String::new();
    let mut format = String::new();
    for line in output.lines() {
        if let Some(v) = line.strip_prefix("Device: ") {
            device = v.trim().into();
        } else if let Some(v) = line.strip_prefix("App output: ") {
            format = v
                .split(". The audio server")
                .next()
                .unwrap_or(v)
                .replace(" · ", "  ·  ")
                .replace(" 1 channels", " 1 channel")
                .replace(" 1 channel.", " 1 channel")
                .trim()
                .into();
        }
    }
    if device.is_empty() && format.is_empty() {
        (output.trim().into(), String::new())
    } else {
        (device, format)
    }
}

struct ArtistCard {
    artist: String,
    albums: usize,
    cover: Option<Track>,
}
impl Ui {
    pub(super) fn heading(&self, title: &str, subtitle: &str) -> gtk::Box {
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        b.add_css_class("library-heading");
        let labels = gtk::Box::new(
            if self.narrow.get() {
                gtk::Orientation::Vertical
            } else {
                gtk::Orientation::Horizontal
            },
            12,
        );
        labels.set_valign(gtk::Align::Center);
        labels.set_hexpand(true);
        let title = label(title, "page-title");
        title.set_wrap(true);
        title.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        labels.append(&title);
        let subtitle = label(subtitle, "subtitle");
        subtitle.set_wrap(true);
        labels.append(&subtitle);
        b.append(&labels);
        self.content.append(&b);
        b
    }
    pub(super) fn clear(&self) {
        if let Some((view, query, scroll)) = self.album_scroll.take() {
            let mut positions = self.album_positions.borrow_mut();
            positions.retain(|(v, q, _)| *v != view || *q != query);
            positions.push((view, query, scroll.vadjustment().value()));
            if positions.len() > 16 {
                positions.remove(0);
            }
        }
        while let Some(c) = self.content.first_child() {
            self.content.remove(&c);
        }
        self.audio_label.replace(None);
        self.lyrics_view.replace(None);
        self.experience.meter.replace(None);
        self.now_parts.replace(None);
    }
    pub(super) fn matches(&self, text: &str) -> bool {
        let q = self.query.borrow().to_lowercase();
        q.split_whitespace()
            .all(|term| text.to_lowercase().contains(term))
    }
    pub(super) fn render(&self) {
        let view = self.view.borrow().clone();
        let previous = self.rendered_view.replace(view.clone());
        let requested = self.motion.requested.take();
        self.motion.cancel();
        let key = match (&previous, &view) {
            (View::Albums | View::Artist(_) | View::Recent, View::Album(key)) => Some(key.clone()),
            (View::Album(key), View::Albums | View::Artist(_) | View::Recent) => Some(key.clone()),
            _ => None,
        };
        if let Some(key) = key {
            if let Some(source) = requested.filter(|a| a.key == key) {
                self.motion.pending.replace(Some(source));
            } else if let Some(picture) = crate::motion::find_cover(self.content.upcast_ref(), &key)
            {
                self.motion
                    .pending
                    .replace(self.motion.anchor(key, &picture));
            }
        }
        if matches!(view, View::Albums | View::Artist(_) | View::Recent) {
            self.collection_view.replace(view.clone());
        }
        self.clear();
        let top_view = match &view {
            View::Album(_) => View::Albums,
            View::Artist(_) => View::Artists,
            v => v.clone(),
        };
        for (v, b) in &self.nav {
            if *v == top_view {
                b.add_css_class("selected")
            } else {
                b.remove_css_class("selected")
            }
        }
        self.nav_select.set_label(match top_view {
            View::Artists => "Artists",
            View::Tracks => "Tracks",
            View::Favorites => "Favorites",
            View::Now => "Now playing",
            View::Recent => "Recently played",
            _ => "Albums",
        });
        match view {
            View::Albums => {
                let count = self.albums.borrow().len();
                let hours = self.tracks.borrow().iter().map(|t| t.duration).sum::<f64>() / 3600.;
                let filtered = self.filtered_albums();
                // The playing album's strip lives in its own slot, so a new album replaces
                // only the strip, never the grid below it.
                let slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
                self.content.append(&slot);
                self.feature_slot.replace(Some(slot.downgrade()));
                self.fill_feature(&slot);
                let h = self.heading(
                    "Collection",
                    &format!(
                        "{} of {}  ·  {}",
                        filtered.len(),
                        presentation::count(count, "album", "albums"),
                        if hours < 1. {
                            format!("{} min of music", (hours * 60.).round() as u32)
                        } else {
                            format!("{hours:.1} hours of music")
                        }
                    ),
                );
                self.collection_controls(&h);
                self.album_grid(filtered);
            }
            View::Artists => {
                let mut map = BTreeMap::<String, (usize, Option<Track>)>::new();
                for a in self.albums.borrow().iter() {
                    let entry = map
                        .entry(a.artist.clone())
                        .or_insert_with(|| (0, a.tracks.first().cloned()));
                    entry.0 += 1;
                }
                let mut artists: Vec<_> = map
                    .into_iter()
                    .filter(|(artist, _)| self.matches(artist))
                    .map(|(artist, (albums, cover))| ArtistCard {
                        artist,
                        albums,
                        cover,
                    })
                    .collect();
                artists.sort_by_cached_key(|a| {
                    a.artist
                        .trim_start_matches(|c: char| !c.is_alphanumeric())
                        .to_lowercase()
                });
                self.heading(
                    "Artists",
                    &format!(
                        "{} in your collection",
                        presentation::count(artists.len(), "artist", "artists")
                    ),
                );
                if artists.is_empty() {
                    self.empty("No artists found", "Change or clear the search.");
                } else {
                    self.artist_grid(artists);
                }
            }
            View::Artist(artist) => {
                let back = action(
                    "Back to artists",
                    "go-previous-symbolic",
                    &self.sender,
                    Command::View(View::Artists),
                );
                back.add_css_class("flat");
                back.add_css_class("album-context");
                back.set_halign(gtk::Align::Start);
                self.content.append(&back);
                let albums = self.filtered_albums();
                let seconds: f64 = albums
                    .iter()
                    .flat_map(|a| a.tracks.iter())
                    .map(|t| t.duration)
                    .sum();
                let h = self.heading(
                    if artist.is_empty() {
                        "Unknown artist"
                    } else {
                        &artist
                    },
                    &format!(
                        "{}  ·  {}",
                        presentation::count(albums.len(), "album", "albums"),
                        time(seconds)
                    ),
                );
                self.collection_controls(&h);
                self.album_grid(albums);
            }
            View::Tracks | View::Favorites => {
                let favorites = view == View::Favorites;
                let favorite_set = self.favorite_paths.borrow();
                let filter = self.settings.borrow().experience.filter.clone();
                let ts = self
                    .tracks
                    .borrow()
                    .iter()
                    .filter(|t| {
                        (!favorites || favorite_set.contains(&t.path))
                            && self.matches(&format!("{} {} {}", t.title, t.artist, t.album))
                            && filter.accepts(t, &favorite_set)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let h = self.heading(
                    if favorites { "Favorites" } else { "All tracks" },
                    &if ts.is_empty() {
                        presentation::count(0, "track", "tracks")
                    } else {
                        format!(
                            "{}  ·  double-click to play",
                            presentation::count(ts.len(), "track", "tracks")
                        )
                    },
                );
                self.collection_controls(&h);
                if ts.is_empty() {
                    if favorites && self.favorite_paths.borrow().is_empty() {
                        self.empty_with_icon(
                            "No favorites yet",
                            "Use the star in the player or a track's ⋮ menu to keep it here.",
                            "starred-symbolic",
                        );
                    } else {
                        self.empty("No tracks found", "Change the search or the filters.");
                    }
                } else {
                    h.append(&action(
                        "Play",
                        "media-playback-start-symbolic",
                        &self.sender,
                        Command::Play(ts.clone(), 0),
                    ));
                    self.content.append(&self.track_list(ts, true));
                }
            }
            View::Album(key) => {
                let a = self.albums.borrow().iter().find(|a| a.key == key).cloned();
                if let Some(a) = a {
                    self.album_page(&a)
                } else {
                    // Retagged or removed in the last refresh, or played from outside the library.
                    let h = self.heading("Album unavailable", "");
                    h.append(&action(
                        "Back to the collection",
                        "go-previous-symbolic",
                        &self.sender,
                        Command::View(View::Albums),
                    ));
                    self.empty(
                        "This album is not in the library",
                        "It may have changed or left the library in the last refresh.",
                    );
                }
            }
            View::Now => self.now_page(),
            View::Recent => self.recent_page(),
        }
        self.motion.expire_pending();
    }
    pub(super) fn empty(&self, title: &str, body: &str) {
        self.empty_with_icon(title, body, "media-optical-symbolic");
    }
    fn empty_with_icon(&self, title: &str, body: &str, icon: &str) {
        let p = adw::StatusPage::builder()
            .title(title)
            .description(body)
            .icon_name(icon)
            .vexpand(true)
            .build();
        self.content.append(&p);
    }
    fn artist_grid(&self, artists: Vec<ArtistCard>) {
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        for a in artists {
            model.append(&glib::BoxedAnyObject::new(a));
        }
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let card = gtk::Box::new(gtk::Orientation::Horizontal, 14);
            card.add_css_class("artist-card");
            card.set_width_request(250);
            let picture = gtk::Picture::new();
            let cover = presentation::cover(&picture, 64);
            cover.set_size_request(64, 64);
            cover.set_valign(gtk::Align::Center);
            card.append(&cover);
            let texts = gtk::Box::new(gtk::Orientation::Vertical, 2);
            texts.set_valign(gtk::Align::Center);
            texts.set_hexpand(true);
            let name = label("", "artist-name");
            name.set_wrap(true);
            name.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            name.set_lines(2);
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            texts.append(&name);
            texts.append(&label("", "artist-count"));
            card.append(&texts);
            item.set_child(Some(&card));
        });
        let parts = |item: &gtk::ListItem| {
            let card = item.child().unwrap();
            let picture = card
                .first_child()
                .and_then(|c| c.first_child())
                .and_downcast::<gtk::Picture>()
                .unwrap();
            let texts = card.last_child().unwrap();
            let name = texts.first_child().and_downcast::<gtk::Label>().unwrap();
            let count = texts.last_child().and_downcast::<gtk::Label>().unwrap();
            (card, picture, name, count)
        };
        let cache = self.art.clone();
        let embedded = self.settings.borrow().prefer_embedded;
        factory.connect_bind(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let object = item.item().and_downcast::<glib::BoxedAnyObject>().unwrap();
            let a = object.borrow::<ArtistCard>();
            let (card, picture, name, count) = parts(item);
            let shown = if a.artist.is_empty() {
                "Unknown artist"
            } else {
                &a.artist
            };
            name.set_text(shown);
            count.set_text(&presentation::count(a.albums, "album", "albums"));
            card.set_tooltip_text(Some(shown));
            if let Some(t) = a.cover.as_ref() {
                cache
                    .borrow_mut()
                    .request(&picture, t, embedded, artwork::THUMB);
            }
        });
        let cache = self.art.clone();
        factory.connect_unbind(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let picture = parts(item).1;
            cache.borrow_mut().cancel(&picture);
            picture.set_paintable(gtk::gdk::Paintable::NONE);
        });
        let grid = gtk::GridView::new(
            Some(gtk::NoSelection::new(Some(model.clone()))),
            Some(factory),
        );
        grid.set_min_columns(1);
        grid.set_max_columns(8);
        grid.set_single_click_activate(true);
        grid.add_css_class("artist-grid");
        let sender = self.sender.clone();
        grid.connect_activate(move |_, pos| {
            if let Some(object) = model.item(pos).and_downcast::<glib::BoxedAnyObject>() {
                let artist = object.borrow::<ArtistCard>().artist.clone();
                let _ = sender.send(Command::View(View::Artist(artist)));
            }
        });
        let scroll = scrolled(&grid);
        self.remember_scroll(&scroll);
        self.content.append(&scroll);
    }
    /// Restore the last position of this view/query and save it on the next clear.
    fn remember_scroll(&self, scroll: &gtk::ScrolledWindow) {
        let view = self.view.borrow().clone();
        let query = self.query.borrow().clone();
        if let Some((_, _, position)) = self
            .album_positions
            .borrow()
            .iter()
            .find(|(v, q, _)| *v == view && *q == query)
        {
            let position = *position;
            // GTK may revise its virtualized height after the first allocation.
            // Restore after the grid settles, then release the frame callback.
            let frames = Cell::new(0u8);
            scroll.add_tick_callback(move |scroll, _| {
                frames.set(frames.get() + 1);
                let a = scroll.vadjustment();
                if frames.get() >= 2 && a.page_size() > 0. && a.upper() > a.page_size() {
                    a.set_value(position.min((a.upper() - a.page_size()).max(0.)));
                }
                if frames.get() >= 8 {
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            });
        }
        self.album_scroll
            .replace(Some((view, query, scroll.clone())));
    }
    pub(super) fn album_grid(&self, albums: Vec<Rc<Album>>) {
        if albums.is_empty() {
            self.empty(
                if self.scanning.get() {
                    "Setting up the shelf"
                } else {
                    "No albums found"
                },
                if self.scanning.get() {
                    "Reading your folders…"
                } else {
                    "Add a folder of FLAC files or change your search."
                },
            );
            return;
        }
        let grid = crate::album_grid::build(
            albums,
            self.art.clone(),
            self.settings.borrow().prefer_embedded,
            self.sender.clone(),
            self.settings.borrow().experience.cover_size,
            self.short_grid.get(),
            self.motion.clone(),
        );
        let scroll = scrolled(&grid);
        self.remember_scroll(&scroll);
        self.content.append(&scroll);
    }
    pub(super) fn track_list(&self, tracks: Vec<Track>, show_album: bool) -> gtk::Box {
        let tracks = Rc::new(tracks);
        let strings: Vec<_> = (0..tracks.len()).map(|i| i.to_string()).collect();
        let refs: Vec<_> = strings.iter().map(String::as_str).collect();
        let model = gtk::StringList::new(&refs);
        // No selection: a clicked row must not stay tinted after the album moves on; the
        // playing track is shown by its own mark.
        let selection = gtk::NoSelection::new(Some(model));
        let factory = gtk::SignalListItemFactory::new();
        let ts = tracks.clone();
        let sender = self.sender.clone();
        let favorites = self.favorite_paths.clone();
        let rows = self.track_rows.clone();
        factory.connect_setup(move |_, object| {
            let item = object.downcast_ref::<gtk::ListItem>().unwrap();
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 14);
            row.add_css_class("track-row");
            {
                let mut rows = rows.borrow_mut();
                // Lists are rebuilt as pages change; drop rows that no longer exist.
                if rows.len() > 512 {
                    rows.retain(|(w, _)| w.upgrade().is_some());
                }
                rows.push((row.downgrade(), Rc::new(RefCell::new(None))));
            }
            let number = label("", "track-number");
            number.set_xalign(1.);
            number.set_valign(gtk::Align::Center);
            row.append(&number);
            let texts = gtk::Box::new(gtk::Orientation::Vertical, 4);
            texts.set_hexpand(true);
            texts.append(&ellipsis("", "track-title"));
            texts.append(&ellipsis("", "track-meta"));
            row.append(&texts);
            let star = gtk::Image::from_icon_name("starred-symbolic");
            star.add_css_class("track-favorite");
            star.set_tooltip_text(Some("Favorite"));
            row.append(&star);
            row.append(&label("", "track-time"));
            let menu = gtk::MenuButton::builder()
                .icon_name("view-more-symbolic")
                .tooltip_text("More track actions")
                .valign(gtk::Align::Center)
                .build();
            menu.add_css_class("flat");
            menu.add_css_class("track-menu");
            menu.update_property(&[gtk::accessible::Property::Label("More track actions")]);
            let pop = gtk::Popover::new();
            let actions = gtk::Box::new(gtk::Orientation::Vertical, 2);
            actions.add_css_class("menu-actions");
            let current = {
                let weak = item.downgrade();
                let ts = ts.clone();
                move || {
                    weak.upgrade()
                        .and_then(|i| ts.get(i.position() as usize).cloned())
                }
            };
            for (text, kind) in [("Play next", 0), ("Add to queue", 1), ("Favorite", 2)] {
                let b = gtk::Button::with_label(text);
                b.add_css_class("flat");
                if let Some(l) = b.child().and_downcast::<gtk::Label>() {
                    l.set_xalign(0.);
                }
                let current = current.clone();
                let sender = sender.clone();
                let p = pop.downgrade();
                let star = star.downgrade();
                b.connect_clicked(move |_| {
                    if let Some(t) = current() {
                        let _ = sender.send(match kind {
                            0 => Command::PlayNext(vec![t]),
                            1 => Command::Append(vec![t]),
                            _ => {
                                if let Some(star) = star.upgrade() {
                                    star.set_visible(!star.is_visible());
                                }
                                Command::ToggleFavorite(t.path)
                            }
                        });
                    }
                    if let Some(p) = p.upgrade() {
                        p.popdown();
                    }
                });
                actions.append(&b);
            }
            let favorite_button = actions.last_child().and_downcast::<gtk::Button>().unwrap();
            let favorites = favorites.clone();
            pop.connect_show(move |_| {
                let on = current().is_some_and(|t| favorites.borrow().contains(&t.path));
                favorite_button.set_label(if on {
                    "Remove from favorites"
                } else {
                    "Favorite"
                });
                if let Some(l) = favorite_button.child().and_downcast::<gtk::Label>() {
                    l.set_xalign(0.);
                }
            });
            pop.set_child(Some(&actions));
            menu.set_popover(Some(&pop));
            row.append(&menu);
            let context = gtk::GestureClick::builder().button(3).build();
            let m = menu.downgrade();
            context.connect_pressed(move |g, _, _, _| {
                g.set_state(gtk::EventSequenceState::Claimed);
                if let Some(m) = m.upgrade() {
                    m.popup();
                }
            });
            row.add_controller(context);
            item.set_child(Some(&row));
        });
        let ts = tracks.clone();
        let favorites = self.favorite_paths.clone();
        let playing = self.last_track.clone();
        let rows = self.track_rows.clone();
        factory.connect_bind(move |_, object| {
            let item = object.downcast_ref::<gtk::ListItem>().unwrap();
            let i = item
                .item()
                .and_downcast::<gtk::StringObject>()
                .unwrap()
                .string()
                .parse::<usize>()
                .unwrap();
            let Some(t) = ts.get(i) else { return };
            let row = item.child().and_downcast::<gtk::Box>().unwrap();
            let number = row.first_child().and_downcast::<gtk::Label>().unwrap();
            number.set_text(&if t.disc_total > 1 {
                format!("{:02}–{:02}", t.disc, t.number)
            } else {
                format!("{:02}", t.number)
            });
            let texts = number.next_sibling().and_downcast::<gtk::Box>().unwrap();
            let title = texts.first_child().and_downcast::<gtk::Label>().unwrap();
            title.set_text(&t.title);
            title
                .next_sibling()
                .and_downcast::<gtk::Label>()
                .unwrap()
                .set_text(&if show_album {
                    format!("{}  ·  {}  ·  {}", t.artist, t.album, t.quality())
                } else {
                    if t.artist == t.album_artist || t.artist.is_empty() {
                        t.quality()
                    } else {
                        format!("{}  ·  {}", t.artist, t.quality())
                    }
                });
            let star = texts.next_sibling().unwrap();
            star.next_sibling()
                .and_downcast::<gtk::Label>()
                .unwrap()
                .set_text(&time(t.duration));
            row.set_tooltip_text(Some(&t.path.display().to_string()));
            if let Some((_, path)) = rows
                .borrow()
                .iter()
                .find(|(w, _)| w.upgrade().as_ref() == Some(&row))
            {
                path.replace(Some(t.path.clone()));
            }
            mark_row(
                &row,
                Some(&t.path),
                playing.borrow().as_deref(),
                &favorites.borrow(),
            );
        });
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.set_vexpand(true);
        let list = gtk::ListView::new(Some(selection), Some(factory));
        list.add_css_class("track-list");
        list.set_single_click_activate(false);
        let s = self.sender.clone();
        list.connect_activate(move |_, i| {
            let _ = s.send(Command::Play(tracks.as_ref().clone(), i as usize));
        });
        body.append(&scrolled(&list));
        body
    }
    /// Show the playing album's strip in `slot` (or nothing when no album is playing).
    pub(super) fn fill_feature(&self, slot: &gtk::Box) {
        while let Some(c) = slot.first_child() {
            slot.remove(&c);
        }
        let current = self.player.borrow().current().map(|t| t.album_key());
        self.feature_album.replace(current.clone());
        if let Some(album) =
            current.and_then(|key| self.albums.borrow().iter().find(|a| a.key == key).cloned())
        {
            self.collection_feature(&album, slot);
        }
    }
    fn collection_feature(&self, album: &Album, slot: &gtk::Box) {
        let Some(track) = album.tracks.first() else {
            return;
        };
        let body = gtk::Box::new(gtk::Orientation::Horizontal, 20);
        body.add_css_class("collection-feature");
        let picture = gtk::Picture::new();
        picture.set_widget_name(&format!("cover:{}", album.key));
        self.art.borrow_mut().request(
            &picture,
            track,
            self.settings.borrow().prefer_embedded,
            artwork::THUMB,
        );
        let cover = presentation::cover(&picture, 80);
        cover.set_size_request(80, 80);
        let sleeve = crate::scene::Sleeve::new(&cover, -3.);
        sleeve.set_valign(gtk::Align::Center);
        let button = gtk::Button::new();
        button.add_css_class("cover-button");
        button.set_child(Some(&sleeve));
        button.set_tooltip_text(Some("Explore the album"));
        button.update_property(&[gtk::accessible::Property::Label("Explore the album")]);
        connect(
            &button,
            &self.sender,
            Command::View(View::Album(album.key.clone())),
        );
        body.append(&button);
        let info = gtk::Box::new(gtk::Orientation::Vertical, 2);
        info.set_hexpand(true);
        info.set_valign(gtk::Align::Center);
        let artist = ellipsis(&album.artist, "feature-artist");
        artist.set_tooltip_text(Some(&album.artist));
        info.append(&artist);
        let title = ellipsis(&album.title, "feature-title");
        title.set_tooltip_text(Some(&album.title));
        info.append(&title);
        body.append(&info);
        let explore = action(
            "Explore the album",
            "view-reveal-symbolic",
            &self.sender,
            Command::View(View::Album(album.key.clone())),
        );
        explore.add_css_class("feature-explore");
        explore.set_valign(gtk::Align::Center);
        body.append(&explore);
        let stage = crate::scene::Stage::new(&body, &picture);
        stage.add_css_class("collection-stage");
        slot.append(&stage);
        self.motion.arrive(album.key.clone(), &picture, 2);
    }
    pub(super) fn album_page(&self, a: &Album) {
        let Some(t) = a.tracks.first() else { return };
        // The album composition depends on width only: a wide but short laptop screen keeps
        // the cover beside the track list instead of stacking two scrolling areas.
        let compact = self.window.width() < 950;
        let back = action(
            "Back to the collection",
            "go-previous-symbolic",
            &self.sender,
            Command::View(self.collection_view.borrow().clone()),
        );
        back.add_css_class("flat");
        back.add_css_class("album-context");
        back.set_halign(gtk::Align::Start);
        self.content.append(&back);
        let composition = gtk::Box::new(
            if compact {
                gtk::Orientation::Vertical
            } else {
                gtk::Orientation::Horizontal
            },
            if compact { 18 } else { 48 },
        );
        composition.add_css_class("album-composition");
        composition.set_vexpand(true);
        let left = gtk::Box::new(
            if compact {
                gtk::Orientation::Horizontal
            } else {
                gtk::Orientation::Vertical
            },
            if compact { 20 } else { 22 },
        );
        left.add_css_class("album-object");
        if !compact {
            left.set_width_request(350);
        }
        let cover = gtk::Picture::new();
        cover.set_widget_name(&format!("cover:{}", a.key));
        self.art
            .borrow_mut()
            .request(&cover, t, self.settings.borrow().prefer_embedded, 720);
        let sleeve = crate::scene::Sleeve::new(
            &presentation::cover(&cover, if compact { 112 } else { 320 }),
            0.,
        );
        let cover_button = gtk::Button::new();
        cover_button.add_css_class("cover-button");
        cover_button.set_child(Some(&sleeve));
        cover_button.set_valign(gtk::Align::Start);
        cover_button.set_halign(gtk::Align::Start);
        cover_button.set_tooltip_text(Some("Enlarge cover and view booklet"));
        cover_button.update_property(&[gtk::accessible::Property::Label(
            "Enlarge cover and view booklet",
        )]);
        connect(&cover_button, &self.sender, Command::Extras(t.album_dir()));
        left.append(&cover_button);
        let info = gtk::Box::new(gtk::Orientation::Vertical, 10);
        info.set_hexpand(true);
        info.append(&presentation::title(&a.artist, "album-artist-name"));
        let title = presentation::title(&a.title, "album-headline");
        title.set_max_width_chars(if compact { 26 } else { 18 });
        info.append(&title);
        info.append(&presentation::title(
            &format!(
                "{}{}{}  /  {}",
                a.year,
                if a.year.is_empty() { "" } else { "  /  " },
                presentation::count(a.tracks.len(), "track", "tracks"),
                time(a.duration())
            ),
            "subtitle",
        ));
        let buttons = presentation::actions();
        let play = action(
            "Play album",
            "media-playback-start-symbolic",
            &self.sender,
            Command::Play(a.tracks.clone(), 0),
        );
        play.add_css_class("suggested-action");
        presentation::add_action(&buttons, &play);
        presentation::add_action(
            &buttons,
            &action(
                "Add to queue",
                "list-add-symbolic",
                &self.sender,
                Command::Append(a.tracks.clone()),
            ),
        );
        info.append(&buttons);
        let extras = presentation::actions();
        presentation::add_action(
            &extras,
            &action(
                "Booklet",
                "image-x-generic-symbolic",
                &self.sender,
                Command::Extras(t.album_dir()),
            ),
        );
        presentation::add_action(
            &extras,
            &action(
                "Details",
                "dialog-information-symbolic",
                &self.sender,
                Command::Inspector(Box::new(t.clone())),
            ),
        );
        let folder = icon_button("folder-open-symbolic", "Open album folder");
        connect(&folder, &self.sender, Command::OpenExternal(t.album_dir()));
        presentation::add_action(&extras, &folder);
        info.append(&extras);
        left.append(&info);
        if compact {
            composition.append(&left);
        } else {
            let left_scroll = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .propagate_natural_width(true)
                .propagate_natural_height(true)
                .child(&left)
                .build();
            left_scroll.set_max_content_width(430);
            left_scroll.set_hexpand(false);
            composition.append(&left_scroll);
        }
        let repertoire = gtk::Box::new(gtk::Orientation::Vertical, 14);
        repertoire.add_css_class("repertoire");
        repertoire.set_hexpand(true);
        repertoire.set_vexpand(true);
        repertoire.append(&label("On this record", "section-title"));
        repertoire.append(&self.track_list(a.tracks.clone(), false));
        if compact {
            repertoire.set_height_request(340);
        }
        composition.append(&repertoire);
        let stage = if compact {
            crate::scene::Stage::new(&scrolled(&composition), &cover)
        } else {
            crate::scene::Stage::new(&composition, &cover)
        };
        stage.add_css_class("album-stage");
        stage.set_vexpand(true);
        self.content.append(&stage);
        self.motion.arrive(a.key.clone(), &cover, 2);
        crate::scene::reveal(&repertoire);
        // Keyboard users start at Back, unless they are typing a search.
        let typing = gtk::prelude::GtkWindowExt::focus(&self.window)
            .is_some_and(|w| w.is::<gtk::Text>() || w.is::<gtk::SearchEntry>());
        if !typing {
            back.grab_focus();
        }
    }
    pub(super) fn now_page(&self) {
        let Some(track) = self.player.borrow().current() else {
            self.heading("The next record is yours", "Choose an album to start");
            self.empty(
                "Press play",
                "Open an album or a playlist from your collection.",
            );
            return;
        };
        let compact = self.window.width() < 950 || self.window.height() < 720;
        let outer = gtk::Box::new(
            if compact {
                gtk::Orientation::Vertical
            } else {
                gtk::Orientation::Horizontal
            },
            if compact { 14 } else { 28 },
        );
        outer.add_css_class("listening");
        if compact {
            outer.add_css_class("compact-listening");
        }
        outer.set_vexpand(true);
        let left = gtk::Box::new(
            if compact {
                gtk::Orientation::Horizontal
            } else {
                gtk::Orientation::Vertical
            },
            14,
        );
        if !compact {
            left.set_width_request(380);
        }
        let info = gtk::Box::new(gtk::Orientation::Vertical, if compact { 6 } else { 12 });
        info.set_hexpand(true);
        let picture = gtk::Picture::new();
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_size_request(
            if compact { 104 } else { 280 },
            if compact { 104 } else { 280 },
        );
        picture.set_can_shrink(true);
        picture.add_css_class("album-cover");
        self.art.borrow_mut().request(
            &picture,
            &track,
            self.settings.borrow().prefer_embedded,
            if compact { artwork::THUMB } else { 680 },
        );
        let aspect = crate::scene::Sleeve::new(
            &presentation::cover(&picture, if compact { 104 } else { 360 }),
            0.,
        );
        aspect.set_valign(gtk::Align::Start);
        let cover_button = gtk::Button::new();
        cover_button.add_css_class("cover-button");
        cover_button.set_child(Some(&aspect));
        cover_button.set_valign(gtk::Align::Start);
        cover_button.set_tooltip_text(Some("Enlarge cover and view booklet"));
        cover_button.update_property(&[gtk::accessible::Property::Label(
            "Enlarge cover and view booklet",
        )]);
        // The page is updated in place on track changes; its buttons act on the track shown.
        let current = Rc::new(RefCell::new(track.clone()));
        connect_current(&cover_button, &self.sender, &current, |t| {
            Command::Extras(t.album_dir())
        });
        left.append(&cover_button);
        let title = label(&track.title, "listening-title");
        title.set_wrap(true);
        if compact {
            title.set_ellipsize(gtk::pango::EllipsizeMode::End);
            title.set_lines(1);
        }
        info.append(&title);
        let subtitle: Vec<gtk::Label> = if compact {
            vec![ellipsis(
                &format!("{} · {}", track.artist, track.album),
                "subtitle",
            )]
        } else {
            vec![
                ellipsis(&track.artist, "subtitle"),
                ellipsis(&track.album, "subtitle"),
            ]
        };
        for line in &subtitle {
            info.append(line);
        }
        let quality = label(&track.quality(), "pill");
        quality.set_halign(gtk::Align::Start);
        quality.set_tooltip_text(Some(&self.player.borrow().output));
        info.append(&quality);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let view_album = action_button("View album", "media-optical-symbolic");
        connect_current(&view_album, &self.sender, &current, |t| {
            Command::View(View::Album(t.album_key()))
        });
        buttons.append(&view_album);
        let booklet = action_button("Booklet", "image-x-generic-symbolic");
        connect_current(&booklet, &self.sender, &current, |t| {
            Command::Extras(t.album_dir())
        });
        buttons.append(&booklet);
        info.append(&buttons);
        left.append(&info);
        outer.append(&left);
        let right = gtk::Box::new(gtk::Orientation::Vertical, 16);
        right.add_css_class("listening-panel");
        right.set_hexpand(true);
        right.set_vexpand(true);
        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_transition_duration(160);
        stack.set_vexpand(true);
        let switcher = gtk::StackSwitcher::new();
        switcher.set_stack(Some(&stack));
        switcher.set_halign(gtk::Align::Start);
        right.append(&switcher);
        let lyrics_box = self.lyrics_reader();
        stack.add_titled(&lyrics_box, Some("lyrics"), "Lyrics");
        let queue_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        self.fill_now_queue(&queue_box);
        stack.add_titled(&queue_box, Some("queue"), "Queue");
        let details = gtk::Box::new(gtk::Orientation::Vertical, 18);
        self.fill_now_details(&details, &track);
        stack.add_titled(&scrolled(&details), Some("details"), "Audio");
        stack.set_visible_child_name(&self.now_tab.borrow());
        let tab = self.now_tab.clone();
        stack.connect_visible_child_name_notify(move |s| {
            if let Some(name) = s.visible_child_name() {
                tab.replace(name.to_string());
            }
        });
        right.append(&stack);
        outer.append(&right);
        let stage = crate::scene::Stage::new(&scrolled(&outer), &picture);
        stage.add_css_class("listening-stage");
        stage.set_vexpand(true);
        self.content.append(&stage);
        self.now_parts.replace(Some(NowParts {
            stage: stage.upcast_ref::<gtk::Widget>().downgrade(),
            compact,
            track: current,
            picture,
            title,
            subtitle,
            quality,
            queue: queue_box,
            details,
        }));
    }
    /// Show another track on the open Now playing page without rebuilding it, so the visible
    /// tab, the lyrics search and keyboard focus stay put. False when the page must be rebuilt.
    pub(super) fn update_now(&self) -> bool {
        let Some(track) = self.player.borrow().current() else {
            return false;
        };
        let parts = self.now_parts.borrow();
        let Some(parts) = parts.as_ref() else {
            return false;
        };
        let compact = self.window.width() < 950 || self.window.height() < 720;
        if parts.compact != compact || parts.stage.upgrade().and_then(|w| w.parent()).is_none() {
            return false;
        }
        self.art.borrow_mut().request(
            &parts.picture,
            &track,
            self.settings.borrow().prefer_embedded,
            if compact { artwork::THUMB } else { 680 },
        );
        parts.title.set_text(&track.title);
        match parts.subtitle.as_slice() {
            [line] => line.set_text(&format!("{} · {}", track.artist, track.album)),
            [artist, album] => {
                artist.set_text(&track.artist);
                album.set_text(&track.album);
            }
            _ => {}
        }
        parts.quality.set_text(&track.quality());
        parts
            .quality
            .set_tooltip_text(Some(&self.player.borrow().output));
        self.fill_now_queue(&parts.queue);
        self.fill_now_details(&parts.details, &track);
        parts.track.replace(track);
        true
    }
    fn fill_now_queue(&self, queue_box: &gtk::Box) {
        while let Some(c) = queue_box.first_child() {
            queue_box.remove(&c);
        }
        let q = self.player.borrow().queue();
        let queue_head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let count = label(
            &format!(
                "{} in the queue",
                presentation::count(q.tracks.len(), "track", "tracks")
            ),
            "subtitle",
        );
        count.set_hexpand(true);
        queue_head.append(&count);
        queue_head.append(&action(
            "Save playlist",
            "document-save-symbolic",
            &self.sender,
            Command::SavePlaylist,
        ));
        queue_box.append(&queue_head);
        queue_box.append(&self.queue_list(q));
    }
    fn fill_now_details(&self, details: &gtk::Box, track: &Track) {
        while let Some(c) = details.first_child() {
            details.remove(&c);
        }
        self.audio_label.replace(None);
        details.append(&self.audio_details(track));
        let tools = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        tools.append(&action(
            "Edition details",
            "dialog-information-symbolic",
            &self.sender,
            Command::Inspector(Box::new(track.clone())),
        ));
        tools.append(&action(
            "Open folder",
            "folder-open-symbolic",
            &self.sender,
            Command::OpenExternal(track.album_dir()),
        ));
        details.append(&tools);
        if self.settings.borrow().experience.meter {
            let meter = self.meter_widget();
            details.append(&meter);
            self.experience.meter.replace(Some(meter));
        } else {
            self.experience.meter.replace(None);
        }
    }
    /// Read-only facts about the file and the negotiated output, as label/value rows.
    fn audio_details(&self, track: &Track) -> gtk::Grid {
        let grid = gtk::Grid::new();
        grid.add_css_class("audio-facts");
        grid.set_column_spacing(20);
        grid.set_row_spacing(12);
        let (device, format) = output_parts(&self.player.borrow().output);
        let replaygain = match self.settings.borrow().replaygain.as_str() {
            "track" => "By track",
            "album" => "By album",
            _ => "Off",
        };
        let file = track
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let rows = [
            ("File", file, Some(track.path.display().to_string())),
            (
                "Original",
                format!(
                    "{}  ·  {}",
                    track.quality(),
                    presentation::count(track.channels as usize, "channel", "channels")
                ),
                None,
            ),
            ("Device", device, None),
            (
                "App output",
                format,
                Some("The audio server may convert the signal before the device.".into()),
            ),
            ("ReplayGain", replaygain.into(), None),
            (
                "Edition",
                if track.release_id.is_empty() {
                    "No edition ID".into()
                } else {
                    track.release_id.clone()
                },
                None,
            ),
        ];
        let mut output = Vec::new();
        for (row, (name, value, tip)) in rows.into_iter().enumerate() {
            let key = label(name, "fact-name");
            key.set_valign(gtk::Align::Start);
            grid.attach(&key, 0, row as i32, 1, 1);
            let value = label(&value, "fact-value");
            value.set_hexpand(true);
            value.set_wrap(true);
            value.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            value.set_tooltip_text(tip.as_deref());
            if name == "Device" || name == "App output" {
                output.push(value.clone());
            }
            grid.attach(&value, 1, row as i32, 1, 1);
        }
        self.audio_label
            .replace(Some((output[0].clone(), output[1].clone())));
        grid
    }
    pub(super) fn queue_list(&self, q: crate::model::Queue) -> gtk::ScrolledWindow {
        let current = q.index;
        let movement = self.queue_motion.get();
        let tracks = Rc::new(q.tracks);
        let strings: Vec<_> = (0..tracks.len()).map(|i| i.to_string()).collect();
        let refs: Vec<_> = strings.iter().map(String::as_str).collect();
        let model = gtk::StringList::new(&refs);
        let selection = gtk::SingleSelection::new(Some(model));
        if let Some(i) = current {
            selection.set_selected(i as u32);
        }
        let factory = gtk::SignalListItemFactory::new();
        let sender = self.sender.clone();
        let count = tracks.len();
        factory.connect_setup(move |_, object| {
            let item = object.downcast_ref::<gtk::ListItem>().unwrap();
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            row.add_css_class("queue-row");
            let number = label("", "track-number");
            number.set_xalign(1.);
            number.set_valign(gtk::Align::Center);
            row.append(&number);
            let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
            text.set_hexpand(true);
            text.append(&ellipsis("", "track-title"));
            text.append(&ellipsis("", "track-meta"));
            row.append(&text);
            row.append(&label("", "track-time"));
            for (icon, tip, kind) in [
                ("go-up-symbolic", "Move up", 0),
                ("go-down-symbolic", "Move down", 1),
                ("list-remove-symbolic", "Remove from queue", 2),
            ] {
                let button = icon_button(icon, tip);
                button.add_css_class("flat");
                let weak = item.downgrade();
                let s = sender.clone();
                button.connect_clicked(move |_| {
                    if let Some(item) = weak.upgrade() {
                        let i = item.position() as usize;
                        let command = match kind {
                            0 if i > 0 => Some(Command::QueueMove(i, i - 1)),
                            1 if i + 1 < count => Some(Command::QueueMove(i, i + 1)),
                            2 => Some(Command::QueueRemove(i)),
                            _ => None,
                        };
                        if let Some(c) = command {
                            let _ = s.send(c);
                        }
                    }
                });
                row.append(&button);
            }
            item.set_child(Some(&crate::motion::SlideRow::new(&row)));
        });
        factory.connect_bind(move |_, object| {
            let item = object.downcast_ref::<gtk::ListItem>().unwrap();
            let i = item.position() as usize;
            let Some(t) = tracks.get(i) else { return };
            let animated = item
                .child()
                .and_downcast::<crate::motion::SlideRow>()
                .unwrap();
            let row = animated.child();
            animated.reset();
            if let Some((from, to)) = movement {
                let shift = if i == to {
                    from as i32 - to as i32
                } else if from < to && (from..to).contains(&i) {
                    1
                } else if from > to && (to + 1..=from).contains(&i) {
                    -1
                } else {
                    0
                };
                animated.shift(shift);
            }
            if Some(i) == current {
                row.add_css_class("current")
            } else {
                row.remove_css_class("current")
            }
            let number = row.first_child().and_downcast::<gtk::Label>().unwrap();
            number.set_text(&format!("{:02}", i + 1));
            let text = number.next_sibling().and_downcast::<gtk::Box>().unwrap();
            let title = text.first_child().and_downcast::<gtk::Label>().unwrap();
            title.set_text(&t.title);
            title
                .next_sibling()
                .and_downcast::<gtk::Label>()
                .unwrap()
                .set_text(&format!("{}  ·  {}", t.artist, t.album));
            text.next_sibling()
                .and_downcast::<gtk::Label>()
                .unwrap()
                .set_text(&time(t.duration));
        });
        factory.connect_unbind(|_, object| {
            let item = object.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(row) = item.child().and_downcast::<crate::motion::SlideRow>() {
                row.reset();
            }
        });
        let list = gtk::ListView::new(Some(selection), Some(factory));
        list.add_css_class("track-list");
        let sender = self.sender.clone();
        list.connect_activate(move |_, i| {
            let _ = sender.send(Command::Jump(i as usize));
        });
        let window = scrolled(&list);
        // Rebuilt after a queue edit (move, remove) for the same track, the list keeps its scroll
        // position; it scrolls to the playing track only when that track is new.
        let serial = self.player.borrow().track_serial();
        let (seen, value) = self.queue_scroll.get();
        let adjustment = window.vadjustment();
        if seen == serial && value > 0. {
            let restored = Rc::new(Cell::new(false));
            adjustment.connect_changed(move |a| {
                if !restored.get() && a.upper() - a.page_size() >= value {
                    restored.set(true);
                    a.set_value(value);
                }
            });
        } else {
            self.queue_scroll.set((serial, 0.));
            if let Some(i) = current {
                list.scroll_to(i as u32, gtk::ListScrollFlags::FOCUS, None);
            }
        }
        let store = self.queue_scroll.clone();
        adjustment.connect_value_changed(move |a| store.set((serial, a.value())));
        window
    }
}

#[cfg(test)]
mod tests {
    use super::output_parts;
    use crate::presentation::count;

    #[test]
    fn counts_use_singular_and_plural() {
        assert_eq!(count(1, "album", "albums"), "1 album");
        assert_eq!(count(0, "track", "tracks"), "0 tracks");
        assert_eq!(count(2, "channel", "channels"), "2 channels");
    }

    #[test]
    fn output_summary_is_split_for_the_audio_tab() {
        let (device, format) = output_parts(
            "Device: Test DAC\nApp output: S16LE · 44.1 kHz · 1 channels. The audio server may convert the signal before the device.",
        );
        assert_eq!(device, "Test DAC");
        assert_eq!(format, "S16LE  ·  44.1 kHz  ·  1 channel");
        assert!(output_parts("").0.contains("when playback starts"));
    }
}
