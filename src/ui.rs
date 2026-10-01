#[path = "experience.rs"]
mod experience;
#[path = "screens.rs"]
mod screens;
#[path = "ui_smoke.rs"]
mod smoke;

use crate::wake::Sender;
use crate::{
    artwork::{self, ArtCache},
    commands::{Command, View},
    library::{self, ScanEvent},
    lyrics::{self, Lyrics},
    model::{Album, Repeat, Track, time},
    motion::Motion,
    mpris,
    player::{Event, Player},
    playlist, presentation,
    storage::{self, Settings},
};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};

fn label(text: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.);
    if !class.is_empty() {
        l.add_css_class(class)
    }
    l
}
fn ellipsis(text: &str, class: &str) -> gtk::Label {
    let l = label(text, class);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l
}
fn icon_button(icon: &str, tip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.set_tooltip_text(Some(tip));
    b.update_property(&[gtk::accessible::Property::Label(tip)]);
    b
}
fn connect(b: &gtk::Button, s: &Sender<Command>, c: Command) {
    let s = s.clone();
    b.connect_clicked(move |_| {
        let _ = s.send(c.clone());
    });
}
fn action_button(text: &str, icon: &str) -> gtk::Button {
    let b = gtk::Button::new();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.append(&gtk::Image::from_icon_name(icon));
    row.append(&label(text, ""));
    b.set_child(Some(&row));
    b
}
fn action(text: &str, icon: &str, s: &Sender<Command>, c: Command) -> gtk::Button {
    let b = action_button(text, icon);
    connect(&b, s, c);
    b
}
/// Seek once a click or drag settles. A drag emits many values and each seek
/// rebuilds the pipeline, so the scale follows the pointer and only the last
/// value is sent. `touched` lets the clock leave the scale alone while the
/// gesture is active.
fn seek_on_release(
    scale: &gtk::Scale,
    sender: &Sender<Command>,
    updating: Rc<Cell<bool>>,
    touched: Rc<Cell<Option<Instant>>>,
) {
    let generation = Rc::new(Cell::new(0u64));
    let s = sender.clone();
    let t = touched;
    scale.connect_change_value(move |scale, _, value| {
        if updating.get() {
            return glib::Propagation::Stop;
        }
        updating.set(true);
        scale.set_value(value);
        updating.set(false);
        t.set(Some(Instant::now()));
        let id = generation.get().wrapping_add(1);
        generation.set(id);
        let generation = generation.clone();
        let s = s.clone();
        glib::timeout_add_local_once(Duration::from_millis(150), move || {
            if generation.get() == id {
                let _ = s.send(Command::Seek(value));
            }
        });
        glib::Propagation::Stop
    });
}
/// The clock may move the scale unless the user touched it in the last moment.
fn scale_free(touched: &Cell<Option<Instant>>) -> bool {
    touched
        .get()
        .is_none_or(|t| t.elapsed() > Duration::from_millis(450))
}
fn scrolled(w: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .hexpand(true)
        .child(w)
        .build()
}

pub struct Ui {
    experience: experience::Experience,
    pub app: adw::Application,
    pub window: adw::ApplicationWindow,
    pub sender: Sender<Command>,
    receiver: Receiver<Command>,
    scan_sender: Sender<ScanEvent>,
    scan_receiver: Receiver<ScanEvent>,
    pub settings: RefCell<Settings>,
    pub player: RefCell<Player>,
    /// Shared with worker threads (opening files reuses what the library read).
    tracks: RefCell<Arc<Vec<Track>>>,
    /// Bumped by every new playback choice; a late `Opened` result is then dropped.
    open_generation: Cell<u64>,
    albums: RefCell<Vec<Rc<Album>>>,
    view: RefCell<View>,
    query: RefCell<String>,
    album_scroll: RefCell<Option<(View, String, gtk::ScrolledWindow)>>,
    album_positions: RefCell<Vec<(View, String, f64)>>,
    gallery: RefCell<Option<Rc<crate::gallery::Gallery>>>,
    content: gtk::Box,
    motion: Rc<Motion>,
    rendered_view: RefCell<View>,
    collection_view: RefCell<View>,
    queue_motion: Cell<Option<(usize, usize)>>,
    search: gtk::SearchEntry,
    toast: adw::ToastOverlay,
    status: gtk::Label,
    spinner: gtk::Spinner,
    scan_cancel: Arc<AtomicBool>,
    scanning: Cell<bool>,
    rescan_queued: Cell<bool>,
    pub(crate) art: Rc<RefCell<ArtCache>>,
    scan_cancel_button: gtk::Button,
    deck_cover: gtk::Picture,
    display_title: gtk::Label,
    display_artist: gtk::Label,
    display_quality: gtk::Label,
    elapsed: gtk::Label,
    total: gtk::Label,
    seek: gtk::Scale,
    seek_updating: Rc<Cell<bool>>,
    seek_touched: Rc<Cell<Option<Instant>>>,
    volume_control: gtk::Scale,
    audio_label: RefCell<Option<(gtk::Label, gtk::Label)>>,
    last_output: RefCell<String>,
    play_button: gtk::Button,
    shuffle_button: gtk::Button,
    repeat_button: gtk::Button,
    favorite_button: gtk::Button,
    lyrics: RefCell<Lyrics>,
    favorite_paths: Rc<RefCell<HashSet<PathBuf>>>,
    lyrics_view: RefCell<Option<(gtk::Label, gtk::TextView, gtk::TextTag)>>,
    now_parts: RefCell<Option<screens::NowParts>>,
    art_pump: Cell<bool>,
    lyrics_loader: RefCell<lyrics::Loader>,
    now_tab: Rc<RefCell<String>>,
    /// Path of the current track, shared with every track list so its row is marked.
    last_track: Rc<RefCell<Option<PathBuf>>>,
    /// Track rows alive in any list; their "current" mark follows track changes.
    track_rows: Rc<RefCell<Vec<screens::TrackRow>>>,
    last_save: Cell<Instant>,
    nav: Vec<(View, gtk::Button)>,
    nav_root: gtk::Box,
    navigation: gtk::Box,
    nav_slot: gtk::ScrolledWindow,
    header: adw::HeaderBar,
    main: gtk::Box,
    nav_stacked: Cell<bool>,
    nav_select: gtk::MenuButton,
    dock: adw::Clamp,
    feature_album: RefCell<Option<String>>,
    /// Container of the playing album strip on the Albums page.
    feature_slot: RefCell<Option<glib::WeakRef<gtk::Box>>>,
    /// Track serial and scroll offset of the queue list, kept across queue edits.
    queue_scroll: Rc<Cell<(u64, f64)>>,
    short_grid: Cell<bool>,
    mpris: RefCell<Option<mpris::Service>>,
    narrow: Cell<bool>,
    tick_pace: Cell<Duration>,
    layout_sensor: gtk::DrawingArea,
    layout_ticks: Cell<u32>,
    layout_queued: Cell<bool>,
    tick_generation: Cell<u64>,
    smoke_phase: Cell<u32>,
    smoke_scroll: Cell<f64>,
    smoke_released: RefCell<Vec<glib::WeakRef<gtk::Widget>>>,
    smoke_pending: Cell<bool>,
    smoke_ready: Rc<Cell<bool>>,
    started: Instant,
}
impl Ui {
    pub fn build(app: &adw::Application, initial: Vec<PathBuf>) -> Result<Rc<Self>, String> {
        presentation::fonts();
        let mut settings = Settings::load();
        if settings.roots.is_empty() {
            if let Some(music) = glib::user_special_dir(glib::UserDirectory::Music) {
                if music.is_dir() {
                    settings.roots.push(music)
                }
            }
            if settings.roots.is_empty() {
                let p = glib::home_dir().join("Music");
                if p.is_dir() {
                    settings.roots.push(p)
                }
            }
        }
        let silent = std::env::var_os("FLACKER_SILENT").is_some();
        let mut player = Player::new(silent)?;
        player.volume = settings.volume.clamp(0., 1.);
        player.replaygain = settings.replaygain.clone();
        player.restore(settings.queue.clone(), settings.position);
        // A saved output that cannot be opened now (DAC unplugged, other audio server)
        // falls back to the default output instead of keeping the app from starting.
        if let Err(e) = player.select_output(&settings.experience.output) {
            eprintln!("Saved audio output unavailable ({e}); using the default output.");
        }
        let (tx, rx) = crate::wake::channel();
        let (stx, srx) = crate::wake::channel();
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("FLACker")
            .default_width(settings.width.max(640))
            .default_height(settings.height.max(580))
            .build();
        presentation::theme(&window);
        window.set_size_request(640, 580);
        if settings.maximized {
            window.maximize();
        }
        window.set_icon_name(Some(crate::APP_ID));
        let css = gtk::CssProvider::new();
        css.load_from_string(include_str!("../assets/style.css"));
        gtk::style_context_add_provider_for_display(
            &gdk::Display::default().ok_or("No display available")?,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
        main.add_css_class("main-surface");
        let header = adw::HeaderBar::new();
        let brand = label("FLACker", "brand");
        let title = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        title.append(&brand);
        header.add_css_class("app-header");
        header.pack_start(&title);
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search your collection"));
        // Natural width comes from max_width_chars; a small minimum lets the
        // window reach the compact breakpoint even with enlarged text.
        search.set_width_chars(6);
        search.set_hexpand(false);
        search.set_max_width_chars(22);
        header.pack_end(&search);
        header.set_title_widget(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
        let s = tx.clone();
        search.connect_search_changed(move |e| {
            let _ = s.send(Command::Search(e.text().to_string()));
        });
        let menu = gio::Menu::new();
        menu.append(Some("Add folder…"), Some("app.add-folder"));
        menu.append(Some("Open FLAC or playlist…"), Some("app.open"));
        menu.append(Some("Refresh library"), Some("app.scan"));
        menu.append(Some("Focus Mode"), Some("app.presentation"));
        menu.append(Some("Recently played"), Some("app.recent"));
        menu.append(Some("Preferences"), Some("app.preferences"));
        menu.append(Some("About FLACker"), Some("app.about"));
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .build();
        header.pack_end(&menu_button);
        main.append(&header);
        let navigation = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        navigation.add_css_class("navigation-rail");
        let sidebar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        sidebar.add_css_class("global-navigation");
        let mut nav = Vec::new();
        for (v, text, _icon) in [
            (View::Albums, "Albums", "media-optical-symbolic"),
            (View::Artists, "Artists", "avatar-default-symbolic"),
            (View::Tracks, "Tracks", "view-list-symbolic"),
            (View::Favorites, "Favorites", "starred-symbolic"),
            (View::Now, "Now playing", "media-playback-start-symbolic"),
        ] {
            let b = gtk::Button::with_label(text);
            connect(&b, &tx, Command::View(v.clone()));
            b.add_css_class("flat");
            sidebar.append(&b);
            nav.push((v, b));
        }
        navigation.append(&sidebar);
        // Compact navigation: a menu whose entries always navigate, so picking the current
        // section again from one of its subpages (an album, an artist) goes back to it.
        let nav_select = gtk::MenuButton::new();
        nav_select.set_label("Albums");
        nav_select.add_css_class("navigation-select");
        nav_select.update_property(&[gtk::accessible::Property::Label("Navigation")]);
        nav_select.set_visible(false);
        let nav_menu = gtk::Popover::new();
        nav_menu.add_css_class("menu");
        let entries = gtk::Box::new(gtk::Orientation::Vertical, 0);
        for (label, view) in [
            ("Albums", View::Albums),
            ("Artists", View::Artists),
            ("Tracks", View::Tracks),
            ("Favorites", View::Favorites),
            ("Now playing", View::Now),
            ("Recently played", View::Recent),
        ] {
            let entry = gtk::Button::with_label(label);
            entry.add_css_class("flat");
            if let Some(child) = entry.child().and_downcast::<gtk::Label>() {
                child.set_xalign(0.);
            }
            let sender = tx.clone();
            let menu = nav_menu.downgrade();
            entry.connect_clicked(move |_| {
                if let Some(menu) = menu.upgrade() {
                    menu.popdown();
                }
                let _ = sender.send(Command::View(view.clone()));
            });
            entries.append(&entry);
        }
        nav_menu.set_child(Some(&entries));
        nav_select.set_popover(Some(&nav_menu));
        navigation.append(&nav_select);
        let scan_state = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let spinner = gtk::Spinner::new();
        scan_state.append(&spinner);
        let status = ellipsis("Reading your collection…", "side-note");
        status.set_max_width_chars(12);
        status.set_visible(false);
        scan_state.append(&status);
        let cancel = action(
            "Cancel reading",
            "process-stop-symbolic",
            &tx,
            Command::CancelScan,
        );
        cancel.add_css_class("flat");
        scan_state.append(&cancel);
        navigation.append(&scan_state);
        // The header keeps the rail's natural width without making it a
        // window minimum; narrower windows switch to the selector instead.
        let nav_slot = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_width(true)
            .propagate_natural_height(true)
            .child(&navigation)
            .build();
        header.set_title_widget(Some(&nav_slot));
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.set_vexpand(true);
        let motion = Rc::new(Motion::default());
        let stage = gtk::Overlay::new();
        stage.set_vexpand(true);
        content.set_margin_bottom(96);
        stage.set_child(Some(&content));
        stage.add_overlay(&motion.layer);
        stage.set_clip_overlay(&motion.layer, true);
        main.append(&stage);
        let deck = gtk::Box::new(gtk::Orientation::Vertical, 0);
        deck.add_css_class("deck");
        let deck_top = gtk::CenterBox::new();
        deck_top.add_css_class("deck-top");
        let display = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        display.add_css_class("display");
        display.set_hexpand(true);
        let deck_cover = gtk::Picture::new();
        deck_cover.set_size_request(44, 44);
        deck_cover.set_can_shrink(true);
        deck_cover.set_content_fit(gtk::ContentFit::Contain);
        display.append(&presentation::fading_cover(&deck_cover, 44));
        let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
        text.set_hexpand(true);
        let display_title = ellipsis("Choose a record", "display-title");
        let display_artist = ellipsis("Your collection, from start to finish", "display-artist");
        let display_quality = ellipsis("FLAC · local playback", "display-quality");
        text.append(&display_title);
        text.append(&display_artist);
        text.append(&display_quality);
        display.append(&text);

        let display_button = gtk::Button::new();
        display_button.add_css_class("flat");
        display_button.add_css_class("now-link");
        display_button.set_tooltip_text(Some("Open the player"));
        display_button.update_property(&[gtk::accessible::Property::Label("Open the player")]);
        display_button.set_hexpand(true);
        display_button.set_child(Some(&display));
        connect(&display_button, &tx, Command::View(View::Now));
        deck_top.set_start_widget(Some(&display_button));
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        controls.add_css_class("transport");
        controls.set_valign(gtk::Align::Center);
        let previous = icon_button("media-skip-backward-symbolic", "Previous track (Alt+←)");
        connect(&previous, &tx, Command::Previous);
        controls.append(&previous);
        let play_button = icon_button("media-playback-start-symbolic", "Play or pause (Space)");
        play_button.add_css_class("play");
        connect(&play_button, &tx, Command::Toggle);
        controls.append(&play_button);
        let next = icon_button("media-skip-forward-symbolic", "Next track (Alt+→)");
        connect(&next, &tx, Command::Next);
        controls.append(&next);
        deck_top.set_center_widget(Some(&controls));
        let modes = gtk::Box::new(gtk::Orientation::Vertical, 4);
        modes.set_valign(gtk::Align::Center);
        modes.set_halign(gtk::Align::End);
        modes.set_hexpand(true);
        let mode_row = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        let shuffle_button = icon_button("media-playlist-shuffle-symbolic", "Shuffle");
        shuffle_button.add_css_class("flat");
        let s = tx.clone();
        shuffle_button.connect_clicked(move |b| {
            let _ = s.send(Command::Shuffle(!b.has_css_class("active")));
        });
        mode_row.append(&shuffle_button);
        let repeat_button = icon_button("media-playlist-repeat-symbolic", "Repeat: off");
        repeat_button.add_css_class("flat");
        mode_row.append(&repeat_button);
        let favorite_button = icon_button("non-starred-symbolic", "Favorite the current track");
        favorite_button.add_css_class("flat");
        connect(&favorite_button, &tx, Command::Favorite);
        mode_row.append(&favorite_button);
        modes.append(&mode_row);
        let volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0., 1., 0.01);
        volume.set_value(settings.volume);
        volume.set_draw_value(false);
        volume.set_width_request(94);
        volume.set_tooltip_text(Some("Playback volume"));
        volume.update_property(&[gtk::accessible::Property::Label("Volume")]);
        let s = tx.clone();
        volume.connect_value_changed(move |v| {
            let _ = s.send(Command::Volume(v.value()));
        });
        modes.append(&volume);
        deck_top.set_end_widget(Some(&modes));
        deck.append(&deck_top);
        let timeline = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let elapsed = label("0:00", "position");
        let total = label("0:00", "position");
        // The right-hand time cycles between duration and remaining times.
        let total_button = gtk::Button::new();
        total_button.add_css_class("flat");
        total_button.add_css_class("time-toggle");
        total_button.set_child(Some(&total));
        let s = tx.clone();
        total_button.connect_clicked(move |_| {
            let _ = s.send(Command::Preference("clock".into(), "next".into()));
        });
        let seek = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0., 1., 0.1);
        // Arrow keys move 5 s and Page Up/Down 30 s (the range is the track in seconds).
        seek.set_increments(5., 30.);
        seek.set_draw_value(false);
        seek.set_hexpand(true);
        seek.update_property(&[gtk::accessible::Property::Label("Track position")]);
        timeline.append(&elapsed);
        timeline.append(&seek);
        timeline.append(&total_button);
        deck.append(&timeline);
        let dock = adw::Clamp::new();
        dock.set_maximum_size(1120);
        dock.set_tightening_threshold(900);
        dock.set_child(Some(&deck));
        dock.set_valign(gtk::Align::End);
        dock.set_margin_start(22);
        dock.set_margin_end(22);
        dock.set_margin_bottom(10);
        stage.add_overlay(&dock);
        let toast = adw::ToastOverlay::new();
        // An invisible full-window sensor reports size changes, so the layout is recomputed
        // only when the window is resized rather than measured on every tick.
        let frame = gtk::Overlay::new();
        frame.set_child(Some(&main));
        let sensor = gtk::DrawingArea::new();
        sensor.set_can_target(false);
        sensor.set_can_focus(false);
        sensor.set_hexpand(true);
        sensor.set_vexpand(true);
        frame.add_overlay(&sensor);
        frame.set_measure_overlay(&sensor, false);
        toast.set_child(Some(&frame));
        window.set_content(Some(&toast));
        let ui = Rc::new(Self {
            experience: experience::Experience::default(),
            app: app.clone(),
            window,
            sender: tx,
            receiver: rx,
            scan_sender: stx,
            scan_receiver: srx,
            settings: RefCell::new(settings),
            player: RefCell::new(player),
            tracks: RefCell::new(Arc::new(Vec::new())),
            open_generation: Cell::new(0),
            albums: RefCell::new(Vec::new()),
            view: RefCell::new(View::Albums),
            query: RefCell::new(String::new()),
            album_scroll: RefCell::new(None),
            album_positions: RefCell::new(Vec::new()),
            gallery: RefCell::new(None),
            content,
            motion,
            rendered_view: RefCell::new(View::Albums),
            collection_view: RefCell::new(View::Albums),
            queue_motion: Cell::new(None),
            search,
            toast,
            status,
            spinner,
            scan_cancel: Arc::new(AtomicBool::new(false)),
            scanning: Cell::new(false),
            rescan_queued: Cell::new(false),
            art: Rc::new(RefCell::new(ArtCache::default())),
            scan_cancel_button: cancel,
            deck_cover,
            display_title,
            display_artist,
            display_quality,
            elapsed,
            total,
            seek,
            seek_updating: Rc::new(Cell::new(false)),
            seek_touched: Rc::new(Cell::new(None)),
            volume_control: volume,
            audio_label: RefCell::new(None),
            last_output: RefCell::new(String::new()),
            play_button,
            shuffle_button,
            repeat_button,
            favorite_button,
            lyrics: RefCell::new(Lyrics::default()),
            favorite_paths: Rc::new(RefCell::new(HashSet::new())),
            lyrics_view: RefCell::new(None),
            now_parts: RefCell::new(None),
            art_pump: Cell::new(false),
            lyrics_loader: RefCell::new(lyrics::Loader::default()),
            now_tab: Rc::new(RefCell::new("lyrics".into())),
            last_track: Rc::new(RefCell::new(None)),
            track_rows: Rc::new(RefCell::new(Vec::new())),
            last_save: Cell::new(Instant::now()),
            nav,
            nav_root: sidebar,
            navigation,
            nav_slot,
            header,
            main,
            nav_stacked: Cell::new(false),
            nav_select,
            dock,
            feature_album: RefCell::new(None),
            feature_slot: RefCell::new(None),
            queue_scroll: Rc::new(Cell::new((u64::MAX, 0.))),
            short_grid: Cell::new(false),
            mpris: RefCell::new(None),
            narrow: Cell::new(false),
            tick_pace: Cell::new(Duration::from_millis(150)),
            layout_sensor: sensor,
            layout_ticks: Cell::new(0),
            layout_queued: Cell::new(false),
            tick_generation: Cell::new(0),
            smoke_phase: Cell::new(0),
            smoke_scroll: Cell::new(0.),
            smoke_released: RefCell::new(Vec::new()),
            smoke_pending: Cell::new(false),
            smoke_ready: Rc::new(Cell::new(false)),
            started: Instant::now(),
        });
        seek_on_release(
            &ui.seek,
            &ui.sender,
            ui.seek_updating.clone(),
            ui.seek_touched.clone(),
        );
        let weak = Rc::downgrade(&ui);
        ui.repeat_button.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                let repeat = match ui.player.borrow().queue().repeat {
                    Repeat::Off => Repeat::All,
                    Repeat::All => Repeat::One,
                    Repeat::One => Repeat::Off,
                };
                let _ = ui.sender.send(Command::Repeat(repeat));
            }
        });
        for (name, c) in [
            ("add-folder", Command::AddFolder),
            ("open", Command::OpenFiles),
            ("scan", Command::Scan),
            ("preferences", Command::Preferences),
            ("about", Command::About),
            ("play-pause", Command::Toggle),
            ("next", Command::Next),
            ("previous", Command::Previous),
            ("quit", Command::Quit),
            ("now", Command::View(View::Now)),
            ("presentation", Command::Presentation),
            ("recent", Command::View(View::Recent)),
            ("clear-history", Command::ClearHistory),
        ] {
            let a = gio::SimpleAction::new(name, None);
            let s = ui.sender.clone();
            a.connect_activate(move |_, _| {
                let _ = s.send(c.clone());
            });
            app.add_action(&a);
        }
        let open_paths =
            gio::SimpleAction::new("open-paths", Some(&Vec::<String>::static_variant_type()));
        let sender = ui.sender.clone();
        open_paths.connect_activate(move |_, arg| {
            if let Some(paths) = arg.and_then(|v| v.get::<Vec<String>>()) {
                let _ = sender.send(Command::OpenPaths(
                    paths.into_iter().map(PathBuf::from).collect(),
                ));
            }
        });
        app.add_action(&open_paths);
        let focus = gio::SimpleAction::new("search", None);
        let se = ui.search.clone();
        focus.connect_activate(move |_, _| {
            se.grab_focus();
        });
        app.add_action(&focus);
        app.set_accels_for_action("app.search", &["<Control>f"]);
        app.set_accels_for_action("app.open", &["<Control>o"]);
        app.set_accels_for_action("app.next", &["<Alt>Right"]);
        app.set_accels_for_action("app.previous", &["<Alt>Left"]);
        app.set_accels_for_action("app.quit", &["<Control>q"]);
        app.set_accels_for_action("app.now", &["<Control>j"]);
        app.set_accels_for_action("app.presentation", &["<Control><Shift>f"]);
        let key = gtk::EventControllerKey::new();
        // Capture phase: Space plays or pauses before a focused button (Back, a track row)
        // could take it, everywhere except text fields.
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&ui);
        key.connect_key_pressed(move |_, key, _, modifiers| {
            if key == gdk::Key::space && modifiers.is_empty() {
                if let Some(ui) = weak.upgrade() {
                    let editing = gtk::prelude::GtkWindowExt::focus(&ui.window).is_some_and(|w| {
                        w.is::<gtk::Text>()
                            || w.is::<gtk::Entry>()
                            || w.is::<gtk::SearchEntry>()
                            || w.is::<gtk::TextView>()
                    });
                    if !editing {
                        let _ = ui.sender.send(Command::Toggle);
                        return glib::Propagation::Stop;
                    }
                }
            }
            glib::Propagation::Proceed
        });
        ui.window.add_controller(key);
        let weak = Rc::downgrade(&ui);
        ui.window.connect_close_request(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.motion.cancel();
                ui.scan_cancel.store(true, Ordering::Relaxed);
                ui.save();
                let _ = ui.player.borrow_mut().stop();
            }
            glib::Propagation::Proceed
        });
        let weak = Rc::downgrade(&ui);
        adw::StyleManager::default().connect_dark_notify(move |_| {
            if let Some(ui) = weak.upgrade() {
                if ui.settings.borrow().theme == "system" {
                    let _ = ui.sender.send(Command::Theme("system".into()));
                }
            }
        });
        ui.favorite_paths
            .replace(ui.settings.borrow().favorites.iter().cloned().collect());
        let compact = ui.settings.borrow().width < 950 || ui.settings.borrow().height < 720;
        ui.narrow.set(compact);
        ui.short_grid.set(ui.settings.borrow().height < 820);
        if compact {
            ui.window.add_css_class("compact");
        }
        ui.display_quality.set_visible(!compact);
        ui.apply_theme();
        // The scale known before mapping (X11, or the display's default) for the first page.
        ui.art.borrow_mut().set_scale(ui.window.scale_factor());
        ui.render();
        ui.window.present();
        ui.mpris.replace(mpris::Service::new(&ui).ok());
        ui.begin_scan();
        if !initial.is_empty() {
            let _ = ui.sender.send(Command::OpenPaths(initial));
        }
        CURRENT.with(|c| c.replace(Rc::downgrade(&ui)));
        crate::wake::set_hook(dispatch);
        schedule_tick(&ui);
        let weak = Rc::downgrade(&ui);
        ui.layout_sensor.connect_resize(move |_, _, _| {
            // Resize arrives during allocation; rebuild the layout right after it instead.
            let Some(ui) = weak.upgrade() else { return };
            if ui.layout_queued.replace(true) {
                return;
            }
            let weak = Rc::downgrade(&ui);
            glib::idle_add_local_once(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.layout_queued.set(false);
                    ui.update_layout();
                }
            });
        });
        if let Some(settings) = gtk::Settings::default() {
            let weak = Rc::downgrade(&ui);
            settings.connect_gtk_xft_dpi_notify(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.update_layout();
                }
            });
        }
        let weak = Rc::downgrade(&ui);
        glib::idle_add_local_once(move || {
            if let Some(ui) = weak.upgrade() {
                ui.update_layout();
            }
        });
        // Covers follow the display scale: sharp on HiDPI, and redrawn when the window moves
        // to a monitor with another scale.
        if ui.art.borrow_mut().set_scale(ui.window.scale_factor()) {
            ui.warm_art();
            ui.render();
        }
        let weak = Rc::downgrade(&ui);
        ui.window.connect_scale_factor_notify(move |w| {
            let Some(ui) = weak.upgrade() else { return };
            if ui.art.borrow_mut().set_scale(w.scale_factor()) {
                ui.warm_art();
                ui.render();
            }
        });
        std::thread::spawn(artwork::prune);
        Ok(ui)
    }
    /// How soon the next tick is needed: fast only while something moves on screen.
    fn tick_interval(&self) -> Duration {
        let shown = self.window.is_mapped() && !self.window.is_suspended();
        let focus = self
            .experience
            .focus
            .borrow()
            .as_ref()
            .is_some_and(|f| f.window.is_visible());
        let playing = {
            let p = self.player.borrow();
            p.playing || p.intends_playing()
        };
        let busy = self.scanning.get()
            || self.art.borrow().busy()
            || self.lyrics_loader.borrow().busy()
            || std::env::var_os("FLACKER_UI_SMOKE").is_some();
        Duration::from_millis(if busy || (playing && (shown || focus)) {
            150
        } else if playing {
            // Minimized: no timeline to draw, only track ends and errors to notice.
            500
        } else {
            1000
        })
    }
    fn notify(&self, text: &str) {
        self.toast.add_toast(adw::Toast::new(text));
    }
    fn save(&self) {
        let mut s = self.settings.borrow_mut();
        let p = self.player.borrow();
        s.queue = p.queue();
        s.position = p.position();
        s.volume = p.volume;
        // A maximized window is remembered as such; its normal size is kept for later.
        s.maximized = self.window.is_maximized();
        if !s.maximized {
            s.width = self.window.width();
            s.height = self.window.height();
        }
        if let Err(e) = self.experience.history.borrow_mut().save() {
            eprintln!("Could not save history: {e}");
        }
        if let Err(e) = s.save() {
            eprintln!("Could not save preferences: {e}");
        }
    }
    fn apply_theme(&self) {
        let theme = self.settings.borrow().theme.clone();
        let manager = adw::StyleManager::default();
        manager.set_color_scheme(match theme.as_str() {
            "light" => adw::ColorScheme::ForceLight,
            "system" => adw::ColorScheme::Default,
            _ => adw::ColorScheme::ForceDark,
        });
        self.window.remove_css_class("dark");
        self.window.remove_css_class("light");
        self.window
            .add_css_class(if manager.is_dark() { "dark" } else { "light" });
        if let Some(m) = self.experience.focus.borrow().as_ref() {
            m.window.remove_css_class("dark");
            m.window.remove_css_class("light");
            m.window
                .add_css_class(if manager.is_dark() { "dark" } else { "light" });
        }
    }
    fn begin_scan(&self) {
        if self.scanning.replace(true) {
            self.rescan_queued.set(true);
            return;
        }
        self.scan_cancel.store(false, Ordering::Relaxed);
        self.spinner.start();
        self.scan_cancel_button.set_visible(true);
        self.status.set_text("Refreshing the collection…");
        library::start_scan(
            self.settings.borrow().roots.clone(),
            storage::data_dir().join("library.sqlite3"),
            self.scan_sender.clone(),
            self.scan_cancel.clone(),
        );
    }
    /// Queue every album cover for background decoding after a library read.
    fn warm_art(&self) {
        let albums = self.albums.borrow();
        let covers: Vec<&Track> = albums.iter().filter_map(|a| a.tracks.first()).collect();
        self.art
            .borrow_mut()
            .warm(&covers, self.settings.borrow().prefer_embedded);
    }
    /// Poll the artwork worker quickly only while covers are pending; the
    /// regular 150 ms tick covers the idle case.
    fn pump_art(self: &Rc<Self>) {
        if self.art_pump.get() || !self.art.borrow().busy() {
            return;
        }
        self.art_pump.set(true);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(15), move || {
            let Some(ui) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            ui.art.borrow_mut().poll();
            if ui.art.borrow().busy() {
                glib::ControlFlow::Continue
            } else {
                ui.art_pump.set(false);
                glib::ControlFlow::Break
            }
        });
    }
    /// Handle pending commands and scan events. Senders wake this at once (see `wake`); the
    /// timer tick calls it too, as a fallback.
    fn drain(self: &Rc<Self>) {
        // A bounded batch keeps the interface responsive; the rest runs right after.
        let mut handled = 0;
        while handled < 100 {
            let Ok(c) = self.receiver.try_recv() else {
                break;
            };
            handled += 1;
            if let Err(e) = self.handle(c) {
                self.notify(&e);
                eprintln!("{e}");
            }
        }
        if handled == 100 {
            crate::wake::rearm();
        }
        while let Ok(event) = self.scan_receiver.try_recv() {
            match event {
                ScanEvent::Progress(n) => self.status.set_text(&format!(
                    "{} found…",
                    presentation::count(n, "track", "tracks")
                )),
                ScanEvent::Done(r) => {
                    self.scanning.set(false);
                    self.spinner.stop();
                    self.scan_cancel_button.set_visible(false);
                    let errors = r.errors.len();
                    let mut changed = false;
                    if !r.cancelled {
                        let albums: Vec<Rc<Album>> = r.albums.into_iter().map(Rc::new).collect();
                        changed =
                            **self.tracks.borrow() != r.tracks || *self.albums.borrow() != albums;
                        if changed {
                            self.albums.replace(albums);
                            self.tracks.replace(Arc::new(r.tracks));
                        }
                    }
                    self.status.set_text(&format!(
                        "{}\n{}{}",
                        presentation::count(self.albums.borrow().len(), "album", "albums"),
                        presentation::count(self.tracks.borrow().len(), "track", "tracks"),
                        if errors > 0 {
                            format!(
                                "\n{}",
                                presentation::count(errors, "read warning", "read warnings")
                            )
                        } else {
                            String::new()
                        }
                    ));
                    let report = storage::data_dir().join("scan-report.txt");
                    let text = r.errors.join("\n");
                    // Tell about problems once, not again at every start for the same files.
                    let known = std::fs::read_to_string(&report).unwrap_or_default();
                    if text != known {
                        let _ = storage::atomic_write(&report, text.as_bytes());
                        if errors > 0 {
                            self.notify(&if errors == 1 {
                                "1 file or folder could not be read. See the report in Preferences."
                                    .to_string()
                            } else {
                                format!("{errors} files or folders could not be read. See the report in Preferences.")
                            });
                        }
                    }
                    if r.cancelled {
                        self.notify("Reading canceled. The previous collection was kept.");
                    }
                    if !r.cancelled {
                        self.warm_art();
                        self.lyrics_loader.borrow_mut().invalidate();
                        self.refresh_track(false);
                    }
                    // An unchanged library keeps the page, its scroll and focus as they are.
                    if changed {
                        self.render();
                    }
                    if self.rescan_queued.replace(false) {
                        self.begin_scan();
                    }
                }
                ScanEvent::Failed(e) => {
                    self.scanning.set(false);
                    self.spinner.stop();
                    self.scan_cancel_button.set_visible(false);
                    self.status.set_text("Reading failed");
                    self.notify(&e);
                }
            }
        }
    }
    fn tick(self: &Rc<Self>) {
        self.smoke_tick();
        self.art.borrow_mut().poll();
        self.pump_art();
        let lyrics = self.lyrics_loader.borrow_mut().poll();
        if let Some(lyrics) = lyrics {
            self.lyrics.replace(lyrics);
            if let Some(m) = self.experience.focus.borrow().as_ref() {
                m.serial.set(u64::MAX);
            }
            self.show_lyrics();
        }
        self.experience_tick();
        self.drain();
        let events = self.player.borrow_mut().poll();
        for event in events {
            match event {
                Event::TrackChanged => {
                    self.refresh_track(true);
                }
                Event::Error(e) => self.notify(&format!("Playback stopped: {e}")),
                Event::Skipped(path, reason) => self.notify(&format!(
                    "Skipped “{}” ({reason}).",
                    path.file_name().unwrap_or_default().to_string_lossy()
                )),
                Event::Ended => {
                    self.refresh_track(false);
                }
                Event::StateChanged => {}
            }
        }
        self.refresh_transport();
        if self.last_save.get().elapsed() > Duration::from_secs(15) {
            self.save();
            self.last_save.set(Instant::now());
        }
        self.status.set_visible(self.scanning.get());
        self.status.set_tooltip_text(Some(&self.status.text()));
        // Safety net for layout changes no signal reports (dock height after a font change).
        let count = self.layout_ticks.get().wrapping_add(1);
        self.layout_ticks.set(count);
        if count % 12 == 0 {
            self.update_layout();
        }
        if let Some(service) = self.mpris.borrow().as_ref() {
            service.update(self);
        }
    }
    /// Compact mode, navigation placement and dock reserve for the current window size.
    fn update_layout(&self) {
        let narrow = self.window.width() < 950 || self.window.height() < 720;
        let navigation_width: i32 = self
            .nav
            .iter()
            .map(|(_, b)| b.measure(gtk::Orientation::Horizontal, -1).1 + 8)
            .sum();
        let select_navigation = self.window.width() < navigation_width + 530;
        self.nav_root.set_visible(!select_navigation);
        self.nav_select.set_visible(select_navigation);
        if self.nav_stacked.replace(select_navigation) != select_navigation {
            if select_navigation {
                self.nav_slot.set_child(gtk::Widget::NONE);
                self.main
                    .insert_child_after(&self.navigation, Some(&self.header));
            } else {
                self.main.remove(&self.navigation);
                self.nav_slot.set_child(Some(&self.navigation));
            }
        }
        let reserve = (self.dock.height() + self.dock.margin_bottom() + 6).max(84);
        if self.content.margin_bottom() != reserve {
            self.content.set_margin_bottom(reserve);
        }
        let short_grid = self.window.height() < 820;
        let grid_changed = self.short_grid.replace(short_grid) != short_grid;
        if self.narrow.replace(narrow) != narrow {
            self.search
                .set_max_width_chars(if narrow { 12 } else { 22 });
            if narrow {
                self.window.add_css_class("compact");
            } else {
                self.window.remove_css_class("compact");
            }
            self.display_quality.set_visible(!narrow);
            self.render();
            // Compact styles change the dock height on the next allocation, which no resize
            // reports: measure the reserve again once that frame is laid out.
            // Tick callbacks run before the frame's layout, so the second one sees it.
            let frames = Cell::new(0);
            self.window.add_tick_callback(move |w, _| {
                frames.set(frames.get() + 1);
                if frames.get() < 2 {
                    return glib::ControlFlow::Continue;
                }
                if let Some(ui) = CURRENT.with(|c| c.borrow().upgrade()) {
                    if ui.window == *w {
                        let reserve = (ui.dock.height() + ui.dock.margin_bottom() + 6).max(84);
                        if ui.content.margin_bottom() != reserve {
                            ui.content.set_margin_bottom(reserve);
                        }
                    }
                }
                glib::ControlFlow::Break
            });
        } else if grid_changed
            && matches!(
                *self.view.borrow(),
                View::Albums | View::Artist(_) | View::Recent
            )
        {
            self.render();
        }
    }
    fn refresh_transport(&self) {
        let p = self.player.borrow();
        let position = p.position();
        let duration = p.duration();
        if (self.volume_control.value() - p.volume).abs() > 0.00001 {
            self.volume_control.set_value(p.volume);
        }
        if *self.last_output.borrow() != p.output {
            self.last_output.replace(p.output.clone());
            self.display_quality.set_tooltip_text(Some(&p.output));
            if let Some((device, format)) = self.audio_label.borrow().as_ref() {
                let (d, f) = screens::output_parts(&p.output);
                device.set_text(&d);
                format.set_text(&f);
            }
        }
        let mode = self.settings.borrow().experience.clock;
        self.elapsed
            .set_text(&time(if scale_free(&self.seek_touched) {
                position
            } else {
                self.seek.value()
            }));
        self.total.set_text(&match mode {
            1 => format!("−{}", time((duration - position).max(0.))),
            2 => p
                .album_remaining()
                .map(|v| format!("−{}", time(v.max(0.))))
                .unwrap_or_else(|| "—:—".into()),
            _ => time(duration),
        });
        let tip = match mode {
            1 => "Track time remaining. Click to switch",
            2 => "Remaining in this album in the queue (not available in shuffle). Click to switch",
            _ => "Track duration. Click to switch",
        };
        if self.total.tooltip_text().as_deref() != Some(tip) {
            if let Some(button) = self.total.parent() {
                button.set_tooltip_text(Some(tip));
                button.update_property(&[gtk::accessible::Property::Label(tip)]);
            }
            self.total.set_tooltip_text(Some(tip));
        }
        self.seek_updating.set(true);
        if (self.seek.adjustment().upper() - duration.max(1.)).abs() > 0.001 {
            self.seek.set_range(0., duration.max(1.));
        }
        if scale_free(&self.seek_touched) && (self.seek.value() - position).abs() > 0.01 {
            self.seek.set_value(position);
        }
        self.seek_updating.set(false);
        let play_icon = if p.playing {
            "media-playback-pause-symbolic"
        } else {
            "media-playback-start-symbolic"
        };
        if self.play_button.icon_name().as_deref() != Some(play_icon) {
            self.play_button.set_icon_name(play_icon);
        }
        let (current, index, count, repeat, shuffle) = p.summary();
        if let Some(t) = current.as_ref() {
            self.display_quality.set_text(&format!(
                "{}  ·  {:02}/{:02}{}",
                t.quality(),
                index.map_or(0, |i| i + 1),
                count,
                if t.disc_total > 1 {
                    format!("  ·  Disc {}/{}", t.disc, t.disc_total)
                } else {
                    String::new()
                }
            ));
        }
        let shuffle_tip = if shuffle {
            "Shuffle: on"
        } else {
            "Shuffle: off"
        };
        self.shuffle_button.set_tooltip_text(Some(shuffle_tip));
        self.shuffle_button
            .update_property(&[gtk::accessible::Property::Label(shuffle_tip)]);
        if shuffle {
            self.shuffle_button.add_css_class("active")
        } else {
            self.shuffle_button.remove_css_class("active")
        };
        if repeat != Repeat::Off {
            self.repeat_button.add_css_class("active")
        } else {
            self.repeat_button.remove_css_class("active")
        };
        let repeat_icon = if repeat == Repeat::One {
            "media-playlist-repeat-song-symbolic"
        } else {
            "media-playlist-repeat-symbolic"
        };
        if self.repeat_button.icon_name().as_deref() != Some(repeat_icon) {
            self.repeat_button.set_icon_name(repeat_icon);
        }
        let repeat_tip = match repeat {
            Repeat::Off => "Repeat: off",
            Repeat::All => "Repeat: whole queue",
            Repeat::One => "Repeat: this track",
        };
        if self.repeat_button.tooltip_text().as_deref() != Some(repeat_tip) {
            self.repeat_button.set_tooltip_text(Some(repeat_tip));
            self.repeat_button
                .update_property(&[gtk::accessible::Property::Label(repeat_tip)]);
        }
        let now = current.map(|t| t.path);
        drop(p);
        if now != *self.last_track.borrow() {
            self.refresh_track(false)
        }
    }
    /// Mark the playing track in every visible track list (album pages, artist pages, Tracks,
    /// Favorites, search results) without rebuilding them; rows that no longer exist are dropped.
    /// Tell desktop media controls where playback jumped (the clamped, applied position).
    fn announce_seek(&self) {
        let position = self.player.borrow().position();
        if let Some(m) = self.mpris.borrow().as_ref() {
            m.seeked(position);
        }
    }
    fn mark_track_rows(&self) {
        let current = self.last_track.borrow().clone();
        let favorites = self.favorite_paths.borrow();
        self.track_rows.borrow_mut().retain(|(weak, path)| {
            let Some(row) = weak.upgrade() else {
                return false;
            };
            screens::mark_row(
                &row,
                path.borrow().as_deref(),
                current.as_deref(),
                &favorites,
            );
            true
        });
    }
    /// Update the open lyrics reader in place; rebuilding the page would lose the visible
    /// tab's scroll position and keyboard focus.
    fn show_lyrics(&self) {
        if let Some((source, words, tag)) = self.lyrics_view.borrow().as_ref() {
            let lyrics = self.lyrics.borrow();
            source.set_text(&lyrics.source);
            let b = words.buffer();
            b.set_text(if lyrics.text.is_empty() {
                "No local lyrics for this track."
            } else {
                &lyrics.text
            });
            b.apply_tag(tag, &b.start_iter(), &b.end_iter());
            words.scroll_to_iter(&mut b.start_iter(), 0., false, 0., 0.);
        }
    }
    fn refresh_track(&self, notify: bool) {
        let t = self.player.borrow().current();
        self.last_track.replace(t.as_ref().map(|t| t.path.clone()));
        self.mark_track_rows();
        if let Some(t) = t {
            self.display_title.set_text(&t.title);
            self.display_artist
                .set_text(&format!("{} · {}", t.artist, t.album));
            self.display_quality.set_text(&t.quality());
            self.display_quality
                .set_tooltip_text(Some(&self.player.borrow().output));
            self.art.borrow_mut().request(
                &self.deck_cover,
                &t,
                self.settings.borrow().prefer_embedded,
                artwork::THUMB,
            );
            self.art
                .borrow_mut()
                .export(&t, self.settings.borrow().prefer_embedded);
            let favorite = self.settings.borrow().favorites.contains(&t.path);
            self.favorite_button.set_icon_name(if favorite {
                "starred-symbolic"
            } else {
                "non-starred-symbolic"
            });
            if self.lyrics_loader.borrow_mut().request(&t) {
                self.lyrics.replace(Lyrics {
                    source: "Reading local lyrics…".into(),
                    ..Default::default()
                });
            }
            if notify && self.settings.borrow().notifications {
                let n = gio::Notification::new(&t.title);
                n.set_body(Some(&format!("{} — {}", t.artist, t.album)));
                self.app.send_notification(Some("now-playing"), &n);
            }
        } else {
            self.display_title.set_text("Choose a record");
            self.display_artist
                .set_text("Your collection, from start to finish");
            self.display_quality.set_text("FLAC · local playback");
            self.deck_cover.set_paintable(gtk::gdk::Paintable::NONE);
            self.lyrics_loader.borrow_mut().invalidate();
            self.lyrics.replace(Lyrics::default());
            self.favorite_button.set_icon_name("non-starred-symbolic");
        }
        let view = self.view.borrow().clone();
        let feature_changed = view == View::Albums
            && self.player.borrow().current().map(|t| t.album_key())
                != *self.feature_album.borrow();
        if view == View::Now {
            if self.update_now() {
                self.show_lyrics();
            } else {
                self.render();
            }
        } else if feature_changed {
            // Only the playing album's strip changes; the grid keeps its scroll and focus.
            let slot = self
                .feature_slot
                .borrow()
                .as_ref()
                .and_then(|w| w.upgrade());
            match slot {
                Some(slot) => self.fill_feature(&slot),
                None => self.render(),
            }
        }
    }
    fn handle(self: &Rc<Self>, command: Command) -> Result<(), String> {
        if matches!(
            command,
            Command::Play(..) | Command::Jump(_) | Command::RandomAlbum
        ) {
            // A newer choice than any files still being opened in the background.
            self.open_generation
                .set(self.open_generation.get().wrapping_add(1));
        }
        if matches!(
            command,
            Command::Play(..)
                | Command::PlayNext(_)
                | Command::Append(_)
                | Command::UndoRemove
                | Command::QueueMove(..)
                | Command::QueueRemove(_)
                | Command::Jump(_)
                | Command::Next
                | Command::Previous
        ) {
            if let Some(m) = self.experience.focus.borrow().as_ref() {
                m.queue_revision.set(u64::MAX);
            }
        }
        match command {
            Command::RandomAlbum => {
                use rand::seq::IndexedRandom;
                if let Some(a) = self.filtered_albums().choose(&mut rand::rng()).cloned() {
                    self.player.borrow_mut().set_shuffle(false)?;
                    self.player.borrow_mut().set_repeat(Repeat::Off)?;
                    self.player.borrow_mut().set_queue(a.tracks.clone(), 0)?;
                    self.refresh_track(false);
                    self.view.replace(View::Album(a.key.clone()));
                    self.render();
                } else {
                    self.notify("No album matches the filters.");
                }
            }
            Command::Collection(f) => {
                self.settings.borrow_mut().experience.filter = f;
                self.album_positions.borrow_mut().clear();
                self.album_scroll.take();
                self.render();
                self.save();
            }
            Command::PlayNext(ts) => {
                if !ts.is_empty() {
                    let committed = self.player.borrow_mut().play_next(ts);
                    self.notify(if committed {
                        "Added after the next track, which is already lined up for a seamless transition."
                    } else {
                        "Added to play next."
                    });
                    if *self.view.borrow() == View::Now {
                        self.render();
                    }
                }
            }
            Command::UndoRemove => {
                if self.player.borrow_mut().undo_remove() {
                    self.render();
                    self.notify("Track restored to the queue.");
                }
            }
            Command::Preference(key, value) => {
                if key == "output" {
                    self.player.borrow_mut().select_output(&value)?;
                }
                {
                    let mut s = self.settings.borrow_mut();
                    let p = &mut s.experience;
                    match key.as_str() {
                        "meter" => p.meter = value == "true",
                        "history" => p.history = value == "true",
                        "output" => p.output = value,
                        "clock" if value == "next" => p.clock = (p.clock.min(2) + 1) % 3,
                        "clock" => p.clock = value.parse::<u32>().unwrap_or(0).min(2),
                        "cover_size" => p.cover_size = value.parse::<u32>().unwrap_or(1).min(2),
                        "lyrics_size" => {
                            p.lyrics_size = value.parse::<u32>().unwrap_or(19).clamp(14, 30)
                        }
                        "lyrics_width" => {
                            p.lyrics_width =
                                [48, 64, 80][value.parse::<usize>().unwrap_or(1).min(2)]
                        }
                        _ => {}
                    }
                }
                if !matches!(key.as_str(), "lyrics_size" | "clock") {
                    self.render();
                }
                self.save();
            }
            Command::Inspector(t) => self.inspector(*t),
            Command::Presentation => self.open_focus(),
            Command::GalleryPage(dir, page) => {
                let mut s = self.settings.borrow_mut();
                let pages = &mut s.experience.gallery_pages;
                if pages.len() >= 256 && !pages.contains_key(&dir) {
                    if let Some(k) = pages.keys().next().cloned() {
                        pages.remove(&k);
                    }
                }
                pages.insert(dir, page);
            }
            Command::ClearHistory => {
                self.experience
                    .history
                    .borrow_mut()
                    .clear()
                    .map_err(|e| e.to_string())?;
                self.experience.recorded.set(true);
                self.notify("Local history cleared.");
                if *self.view.borrow() == View::Recent {
                    self.render();
                }
            }

            Command::View(v) => {
                self.view.replace(v);
                self.render();
            }
            Command::Search(q) => {
                // Pages that show no searchable list (an album, Now playing, history) give
                // way to matching albums as soon as something is typed.
                let page = matches!(
                    *self.view.borrow(),
                    View::Album(_) | View::Now | View::Recent
                );
                if page && !q.is_empty() {
                    self.view.replace(View::Albums);
                }
                self.query.replace(q);
                self.render();
            }
            Command::Play(ts, index) => {
                if ts.is_empty() {
                    self.notify("No tracks available in this selection.");
                } else {
                    self.player.borrow_mut().set_queue(ts, index)?;
                    self.refresh_track(false);
                }
            }
            Command::Append(ts) => {
                let n = ts.len();
                let empty = self.player.borrow().queue().tracks.is_empty();
                if empty {
                    let mut q = self.player.borrow().queue();
                    q.replace(ts, 0);
                    self.player.borrow_mut().restore(q, 0.);
                } else {
                    self.player.borrow_mut().append(ts);
                }
                self.notify(&if n == 1 {
                    "1 track added to the queue".to_string()
                } else {
                    format!("{n} tracks added to the queue")
                });
                if *self.view.borrow() == View::Now {
                    self.render();
                }
            }
            Command::Toggle => {
                self.player.borrow_mut().toggle()?;
            }
            Command::Pause => {
                self.player.borrow_mut().pause()?;
            }
            Command::Resume => {
                self.player.borrow_mut().resume()?;
            }
            Command::Stop => {
                self.player.borrow_mut().stop()?;
            }
            Command::Next => {
                self.player.borrow_mut().next()?;
                self.refresh_track(false);
            }
            Command::Previous => {
                let track = self.player.borrow().track_serial();
                self.player.borrow_mut().previous()?;
                // Previous after a few seconds restarts the same track: that is a seek.
                if self.player.borrow().track_serial() == track {
                    self.announce_seek();
                }
                self.refresh_track(false);
            }
            Command::Seek(s) => {
                self.player.borrow_mut().seek(s)?;
                self.announce_seek();
            }
            Command::SeekRelative(delta) => {
                let position = self.player.borrow().position() + delta;
                self.player.borrow_mut().seek(position)?;
                self.announce_seek();
            }
            Command::Jump(i) => {
                self.player.borrow_mut().jump(i)?;
                self.refresh_track(false);
            }
            Command::Volume(v) => {
                let mut p = self.player.borrow_mut();
                p.volume = v.clamp(0., 1.);
                p.apply_volume();
            }
            Command::Shuffle(s) => {
                self.player.borrow_mut().set_shuffle(s)?;
            }
            Command::Repeat(r) => {
                self.player.borrow_mut().set_repeat(r)?;
            }
            Command::Favorite => {
                let current = self.player.borrow().current();
                if let Some(t) = current {
                    self.handle(Command::ToggleFavorite(t.path))?;
                }
            }
            Command::ToggleFavorite(path) => {
                let mut settings = self.settings.borrow_mut();
                let added = !settings.favorites.contains(&path);
                if added {
                    settings.favorites.push(path.clone());
                } else {
                    settings.favorites.retain(|p| *p != path);
                }
                self.favorite_paths
                    .replace(settings.favorites.iter().cloned().collect());
                drop(settings);
                let current = self.player.borrow().current().map(|t| t.path);
                if current.as_ref() == Some(&path) {
                    self.favorite_button.set_icon_name(if added {
                        "starred-symbolic"
                    } else {
                        "non-starred-symbolic"
                    });
                }
                self.save();
                if *self.view.borrow() == View::Favorites {
                    self.render();
                } else {
                    // Stars in the lists on screen follow a favorite set from the player.
                    self.mark_track_rows();
                }
            }
            Command::Scan => self.begin_scan(),
            Command::CancelScan => {
                self.scan_cancel.store(true, Ordering::Relaxed);
                self.rescan_queued.set(false);
            }
            Command::AddFolder => {
                let dialog = gtk::FileDialog::builder()
                    .title("Add a music folder")
                    .build();
                let s = self.sender.clone();
                dialog.select_folder(Some(&self.window), gio::Cancellable::NONE, move |r| {
                    if let Ok(f) = r {
                        if let Some(p) = f.path() {
                            let _ = s.send(Command::Root(p));
                        }
                    }
                });
            }
            Command::Root(p) => {
                let p = p.canonicalize().map_err(|e| e.to_string())?;
                let mut s = self.settings.borrow_mut();
                if !s.roots.contains(&p) {
                    s.roots.push(p);
                }
                drop(s);
                self.save();
                // A new folder is read right away, as on first setup.
                self.notify("Folder added. Reading it now…");
                self.begin_scan();
            }
            Command::RemoveRoot(p) => {
                self.settings.borrow_mut().roots.retain(|root| *root != p);
                self.save();
                self.notify("Folder removed from the settings. Use Refresh library to apply.");
            }
            Command::OpenFiles => {
                let dialog = gtk::FileDialog::builder()
                    .title("Open a local FLAC or playlist")
                    .build();
                let filter = gtk::FileFilter::new();
                filter.set_name(Some("FLAC and M3U playlists"));
                for suffix in ["flac", "FLAC", "m3u", "m3u8"] {
                    filter.add_suffix(suffix);
                }
                dialog.set_default_filter(Some(&filter));
                let s = self.sender.clone();
                dialog.open_multiple(Some(&self.window), gio::Cancellable::NONE, move |r| {
                    if let Ok(files) = r {
                        let paths = (0..files.n_items())
                            .filter_map(|i| {
                                files
                                    .item(i)
                                    .and_downcast::<gio::File>()
                                    .and_then(|f| f.path())
                            })
                            .collect();
                        let _ = s.send(Command::OpenPaths(paths));
                    }
                });
            }
            Command::OpenPaths(paths) => {
                // Walking folders, reading playlists and FLAC headers can take a while on a
                // slow drive: resolved in the background, then played via `Opened`.
                let known = self.tracks.borrow().clone();
                let generation = self.open_generation.get().wrapping_add(1);
                self.open_generation.set(generation);
                let sender = self.sender.clone();
                std::thread::spawn(move || {
                    let known: HashMap<&std::path::Path, &Track> =
                        known.iter().map(|t| (t.path.as_path(), t)).collect();
                    let (tracks, problems) = open_paths(paths, &known);
                    let _ = sender.send(Command::Opened(generation, tracks, problems));
                });
            }
            Command::Opened(generation, tracks, problems) => {
                // Something else was chosen to play meanwhile: that choice stands.
                if generation != self.open_generation.get() {
                    return Ok(());
                }
                match problems.as_slice() {
                    [] => {}
                    [one] => self.notify(one),
                    [first, rest @ ..] => self.notify(&format!(
                        "{first} (and {} more)",
                        presentation::count(rest.len(), "problem", "problems")
                    )),
                }
                if !tracks.is_empty() {
                    self.handle(Command::Play(tracks, 0))?;
                    self.view.replace(View::Now);
                    self.render();
                }
            }
            Command::SavePlaylist => {
                let dialog = adw::AlertDialog::builder()
                    .heading("Save the queue as a playlist")
                    .body("The playlist is kept in FLACker's data, apart from your music.")
                    .build();
                dialog.add_responses(&[("cancel", "Cancel"), ("save", "Save")]);
                dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
                dialog.set_default_response(Some("save"));
                dialog.set_close_response("cancel");
                let entry = gtk::Entry::builder()
                    .placeholder_text("Playlist name")
                    .text("My selection")
                    .build();
                dialog.set_extra_child(Some(&entry));
                let s = self.sender.clone();
                dialog.connect_response(None, move |_, r| {
                    if r == "save" {
                        let _ = s.send(Command::SaveNamedPlaylist(entry.text().to_string()));
                    }
                });
                dialog.present(Some(&self.window));
            }
            Command::SaveNamedPlaylist(name) => {
                let clean: String = name
                    .chars()
                    .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
                    .take(100)
                    .collect();
                if clean.trim().is_empty() {
                    return Err("Choose a name for the playlist.".into());
                }
                let dir = storage::data_dir().join("playlists");
                let mut p = dir.join(format!("{}.m3u8", clean.trim()));
                let mut n = 2;
                while p.exists() {
                    p = dir.join(format!("{} ({n}).m3u8", clean.trim()));
                    n += 1;
                }
                playlist::write(&p, &self.player.borrow().queue().tracks)
                    .map_err(|e| e.to_string())?;
                self.notify("Playlist saved in FLACker's data.");
            }
            Command::QueueMove(a, b) => {
                self.player.borrow_mut().move_item(a, b)?;
                self.queue_motion.set(Some((a, b)));
                self.render();
                self.queue_motion.set(None);
            }
            Command::QueueRemove(i) => {
                self.player.borrow_mut().remove(i)?;
                if let Some(old) = self.experience.undo_toast.take() {
                    old.dismiss();
                }
                let toast = adw::Toast::builder()
                    .title("Track removed from the queue")
                    .button_label("Undo")
                    .timeout(8)
                    .build();
                let sender = self.sender.clone();
                toast.connect_button_clicked(move |_| {
                    let _ = sender.send(Command::UndoRemove);
                });
                self.experience.undo_toast.replace(Some(toast.clone()));
                self.toast.add_toast(toast);
                // refresh_track redraws Now playing, the only page that shows the queue.
                self.refresh_track(false);
            }
            Command::Preferences => self.preferences(),
            Command::Theme(t) => {
                self.settings.borrow_mut().theme = t;
                self.apply_theme();
                self.save();
            }
            Command::ReplayGain(mode) => {
                self.settings.borrow_mut().replaygain = mode.clone();
                let mut p = self.player.borrow_mut();
                p.replaygain = mode;
                p.apply_volume();
                drop(p);
                self.save();
                if *self.view.borrow() == View::Now {
                    self.render();
                }
            }
            Command::Notifications(on) => {
                self.settings.borrow_mut().notifications = on;
                self.save();
            }
            Command::Embedded(on) => {
                self.settings.borrow_mut().prefer_embedded = on;
                self.art.replace(ArtCache::default());
                self.warm_art();
                self.refresh_track(false);
                self.render();
                self.save();
            }
            Command::OpenExternal(path) => {
                gio::AppInfo::launch_default_for_uri(
                    &gio::File::for_path(path).uri(),
                    gio::AppLaunchContext::NONE,
                )
                .map_err(|e| e.to_string())?;
            }
            Command::Extras(path) => self.extras(path),
            Command::About => {
                let d = adw::AboutDialog::builder()
                    .application_name("FLACker")
                    .application_icon(crate::APP_ID)
                    .version(env!("CARGO_PKG_VERSION"))
                    .developer_name("Gabriel Aleixo")
                    .license_type(gtk::License::Gpl30)
                    .comments(
                        "A home for your collection.\nFLAC, covers and lyrics. Fully offline.",
                    )
                    .build();
                d.present(Some(&self.window));
            }
            Command::Quit => {
                self.save();
                self.app.quit();
            }
            Command::Raise => self.window.present(),
        }
        Ok(())
    }
    fn preferences(&self) {
        let win = adw::PreferencesDialog::builder()
            .title("FLACker Preferences")
            .content_width(660)
            .content_height(650)
            .build();
        let page = adw::PreferencesPage::new();
        let look = adw::PreferencesGroup::builder()
            .title("Appearance and playback")
            .build();
        let settings = self.settings.borrow();
        let theme = adw::ComboRow::builder()
            .title("Appearance")
            .model(&gtk::StringList::new(&["Dark", "Light", "Follow GNOME"]))
            .selected(match settings.theme.as_str() {
                "light" => 1,
                "system" => 2,
                _ => 0,
            })
            .build();
        let s = self.sender.clone();
        theme.connect_selected_notify(move |r| {
            let _ = s.send(Command::Theme(
                ["dark", "light", "system"][r.selected().min(2) as usize].into(),
            ));
        });
        look.add(&theme);
        let gain = adw::ComboRow::builder()
            .title("ReplayGain")
            .subtitle("Uses the values in the tags, with peak protection")
            .model(&gtk::StringList::new(&["Off", "By track", "By album"]))
            .selected(match settings.replaygain.as_str() {
                "track" => 1,
                "album" => 2,
                _ => 0,
            })
            .build();
        let s = self.sender.clone();
        gain.connect_selected_notify(move |r| {
            let _ = s.send(Command::ReplayGain(
                ["off", "track", "album"][r.selected().min(2) as usize].into(),
            ));
        });
        look.add(&gain);
        let notifications = adw::SwitchRow::builder()
            .title("Notify on track change")
            .active(settings.notifications)
            .build();
        let s = self.sender.clone();
        notifications.connect_active_notify(move |r| {
            let _ = s.send(Command::Notifications(r.is_active()));
        });
        look.add(&notifications);
        let cover = adw::SwitchRow::builder()
            .title("Prefer embedded cover")
            .subtitle("Off: cover.jpg or Folder.jpg in the folder comes first")
            .active(settings.prefer_embedded)
            .build();
        let s = self.sender.clone();
        cover.connect_active_notify(move |r| {
            let _ = s.send(Command::Embedded(r.is_active()));
        });
        look.add(&cover);
        page.add(&look);
        self.extra_preferences(&page);
        let roots = adw::PreferencesGroup::builder()
            .title("Library folders")
            .description(
                "Read-only. Refreshed when the app opens or from Refresh library in the menu.",
            )
            .build();
        for p in &settings.roots {
            let row = adw::ActionRow::builder()
                .title(p.to_string_lossy().as_ref())
                .build();
            let remove = icon_button(
                "list-remove-symbolic",
                "Remove from the library without deleting files",
            );
            let path = p.clone();
            let s = self.sender.clone();
            let w = win.downgrade();
            remove.connect_clicked(move |_| {
                let _ = s.send(Command::RemoveRoot(path.clone()));
                if let Some(w) = w.upgrade() {
                    w.close();
                }
            });
            row.add_suffix(&remove);
            roots.add(&row);
        }
        let add = action(
            "Add folder",
            "list-add-symbolic",
            &self.sender,
            Command::AddFolder,
        );
        roots.add(&add);
        page.add(&roots);
        let data=adw::PreferencesGroup::builder().title("Data and diagnostics").description("Preferences, playlists and the index are kept apart from your music. The cover cache is limited to 256 MiB.").build();
        data.add(&action(
            "Open FLACker data",
            "folder-open-symbolic",
            &self.sender,
            Command::OpenExternal(storage::data_dir()),
        ));
        let report = storage::data_dir().join("scan-report.txt");
        if report.is_file() {
            data.add(&action(
                "Read report",
                "text-x-generic-symbolic",
                &self.sender,
                Command::OpenExternal(report),
            ));
        }
        page.add(&data);
        win.add(&page);
        win.present(Some(&self.window));
    }
    fn extras(&self, dir: PathBuf) {
        if let Some(gallery) = self.gallery.borrow().as_ref() {
            if gallery.directory == dir && gallery.window.is_visible() {
                gallery.window.present();
                return;
            }
        }
        if let Some(old) = self.gallery.take() {
            old.window.close();
        }
        let track = self
            .player
            .borrow()
            .current()
            .filter(|t| t.album_dir() == dir)
            .or_else(|| {
                if let View::Album(key) = &*self.view.borrow() {
                    self.albums
                        .borrow()
                        .iter()
                        .find(|a| &a.key == key)
                        .and_then(|a| a.tracks.first())
                        .filter(|t| t.album_dir() == dir)
                        .cloned()
                } else {
                    None
                }
            })
            .or_else(|| {
                self.tracks
                    .borrow()
                    .iter()
                    .find(|t| t.album_dir() == dir)
                    .cloned()
            });
        self.gallery.replace(Some(crate::gallery::Gallery::new(
            &self.window,
            dir.clone(),
            track,
            self.sender.clone(),
            self.art.clone(),
            self.settings
                .borrow()
                .experience
                .gallery_pages
                .get(&dir)
                .cloned(),
        )));
    }
}

thread_local! {
    /// The window's controller, for the wake-up hook of the command channels.
    static CURRENT: RefCell<std::rc::Weak<Ui>> = const { RefCell::new(std::rc::Weak::new()) };
}

/// Wake-up hook: runs on the main loop right after a command or scan event is sent.
fn dispatch() {
    crate::wake::drained();
    if let Some(ui) = CURRENT.with(|c| c.borrow().upgrade()) {
        ui.drain();
        ui.refresh_transport();
        // Play pressed while the tick idles slowly: restart it at the fast pace now.
        if ui.tick_interval() < ui.tick_pace.get() {
            ui.tick_generation
                .set(ui.tick_generation.get().wrapping_add(1));
            schedule_tick(&ui);
        }
    }
}

/// The periodic tick, rescheduled each time at the pace `tick_interval` asks for. Each chain
/// carries a generation; starting a new chain (to speed up) quietly ends the old one.
fn schedule_tick(ui: &Rc<Ui>) {
    let interval = ui.tick_interval();
    ui.tick_pace.set(interval);
    let generation = ui.tick_generation.get();
    let keep = ui.clone();
    glib::timeout_add_local_once(interval, move || {
        if !keep.window.is_visible() || keep.tick_generation.get() != generation {
            return;
        }
        keep.tick();
        schedule_tick(&keep);
    });
}

/// Resolve files, folders and playlists given to FLACker into tracks, reusing what the
/// library already read. Runs off the interface thread.
fn open_paths(
    paths: Vec<PathBuf>,
    known: &HashMap<&std::path::Path, &Track>,
) -> (Vec<Track>, Vec<String>) {
    let mut problems = Vec::new();
    let mut files = Vec::new();
    // A single folder plays its FLAC files; library folders are added in Preferences.
    let single_folder = paths.len() == 1 && paths[0].is_dir();
    for path in paths {
        if single_folder {
            let mut found: Vec<PathBuf> = walkdir::WalkDir::new(&path)
                .follow_links(false)
                .into_iter()
                .flatten()
                .filter(|e| {
                    e.file_type().is_file()
                        && e.path()
                            .extension()
                            .is_some_and(|x| x.eq_ignore_ascii_case("flac"))
                })
                .map(|e| e.into_path())
                .collect();
            found.sort();
            files.extend(found);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("m3u") || e.eq_ignore_ascii_case("m3u8"))
        {
            match playlist::read(&path) {
                Ok(v) => files.extend(v),
                Err(e) => problems.push(format!("Could not read {}: {e}", path.display())),
            }
        } else {
            files.push(path);
        }
    }
    let mut tracks = Vec::new();
    for p in files {
        let p = p.canonicalize().unwrap_or(p);
        if let Some(t) = known.get(p.as_path()) {
            tracks.push((*t).clone());
            continue;
        }
        match crate::flac::read(&p, false) {
            Ok(m) => tracks.push(m.track),
            Err(e) => problems.push(format!("{}: {e}", p.display())),
        }
    }
    (tracks, problems)
}
