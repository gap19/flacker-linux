//! One bounded, asynchronously decoded image at a time in an adaptive art window.
use crate::wake::Sender;
use crate::{artwork, commands::Command, flac, model::Track};
use adw::prelude::*;
use gtk::{gdk, gdk_pixbuf, gio, glib};
use std::{
    cell::{Cell, RefCell},
    cmp::Ordering,
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc,
    time::Duration,
};

#[derive(Clone)]
enum Source {
    File(PathBuf),
    Embedded(Box<Track>),
}
struct Item {
    source: Source,
    title: String,
    detail: String,
}
struct Pixels {
    bytes: glib::Bytes,
    width: i32,
    height: i32,
    stride: i32,
    alpha: bool,
}
const PREVIEW_EDGE: i32 = 2048;

fn decode(source: &Source) -> Result<Pixels, String> {
    let pixbuf = match source {
        Source::File(path) => {
            if path.metadata().map_err(|e| e.to_string())?.len() > 64 * 1024 * 1024 {
                return Err(
                    "File too large to preview. Open the original in the system viewer.".into(),
                );
            }
            let (_, width, height) = gdk_pixbuf::Pixbuf::file_info(path)
                .ok_or("This document can be opened in the system app.")?;
            if width <= 0 || height <= 0 || i64::from(width) * i64::from(height) > 64_000_000 {
                return Err(
                    "Image too large to preview. Open the original in the system viewer.".into(),
                );
            }
            gdk_pixbuf::Pixbuf::from_file_at_scale(path, PREVIEW_EDGE, PREVIEW_EDGE, true)
        }
        Source::Embedded(track) => {
            let data = flac::read_options(&track.path, false, true)
                .map_err(|e| e.to_string())?
                .picture
                .ok_or("Embedded cover unavailable")?;
            let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(data));
            gdk_pixbuf::Pixbuf::from_stream_at_scale(
                &stream,
                PREVIEW_EDGE,
                PREVIEW_EDGE,
                true,
                gio::Cancellable::NONE,
            )
        }
    }
    .map_err(|_| {
        "Could not load this image. Try opening the original in the system viewer.".to_string()
    })?;
    Ok(Pixels {
        bytes: pixbuf.read_pixel_bytes(),
        width: pixbuf.width(),
        height: pixbuf.height(),
        stride: pixbuf.rowstride(),
        alpha: pixbuf.has_alpha(),
    })
}
fn category(path: &Path) -> (u8, &'static str) {
    let name = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    let ext = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    if matches!(ext.as_str(), "pdf" | "txt" | "json") {
        (5, "Document")
    } else if ["front", "cover", "folder"]
        .iter()
        .any(|p| name.starts_with(p))
    {
        (0, "Cover")
    } else if name.starts_with("back") {
        (1, "Back cover")
    } else if name.starts_with("booklet") {
        (2, "Booklet")
    } else if name.starts_with("medium") || name.starts_with("disc") || name.starts_with("cd") {
        (3, "Disc")
    } else {
        (4, "Image")
    }
}
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let a = a.to_lowercase();
    let b = b.to_lowercase();
    let mut left = a.chars().peekable();
    let mut right = b.chars().peekable();
    loop {
        match (left.peek(), right.peek()) {
            (Some(a), Some(b)) if a.is_ascii_digit() && b.is_ascii_digit() => {
                let mut x = String::new();
                let mut y = String::new();
                while left.peek().is_some_and(|c| c.is_ascii_digit()) {
                    x.push(left.next().unwrap());
                }
                while right.peek().is_some_and(|c| c.is_ascii_digit()) {
                    y.push(right.next().unwrap());
                }
                let x = x.trim_start_matches('0');
                let y = y.trim_start_matches('0');
                let order = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(_), Some(_)) => {
                let order = left.next().cmp(&right.next());
                if order != Ordering::Equal {
                    return order;
                }
            }
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
        }
    }
}
fn items(dir: &Path, track: Option<Track>) -> Vec<Item> {
    let mut out = Vec::new();
    if let Some(track) = track.filter(|t| t.has_cover) {
        out.push(Item {
            source: Source::Embedded(Box::new(track)),
            title: "Embedded cover".into(),
            detail: "Original image from the FLAC".into(),
        });
    }
    let mut files = artwork::extras(dir);
    files.sort_by(|a, b| {
        category(a)
            .0
            .cmp(&category(b).0)
            .then_with(|| {
                natural_cmp(
                    &a.file_name().unwrap_or_default().to_string_lossy(),
                    &b.file_name().unwrap_or_default().to_string_lossy(),
                )
            })
            .then(a.cmp(b))
    });
    let mut counts = [0; 6];
    for file in files {
        let (rank, name) = category(&file);
        counts[rank as usize] += 1;
        let title = if rank == 5 {
            file.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else {
            format!("{name} {}", counts[rank as usize])
        };
        let detail = file
            .strip_prefix(dir)
            .unwrap_or(&file)
            .to_string_lossy()
            .into_owned();
        out.push(Item {
            source: Source::File(file),
            title,
            detail,
        });
    }
    out
}
fn button(icon: &str, text: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.set_tooltip_text(Some(text));
    b.update_property(&[gtk::accessible::Property::Label(text)]);
    b
}

pub struct Gallery {
    pub window: adw::Window,
    pub(crate) directory: PathBuf,
    pub picture: gtk::Picture,
    pub stage: gtk::ScrolledWindow,
    stack: gtk::Stack,
    spinner: gtk::Spinner,
    notice: gtk::Label,
    pub(crate) select: gtk::DropDown,
    counter: gtk::Label,
    previous: gtk::Button,
    next: gtk::Button,
    minus: gtk::Button,
    plus: gtk::Button,
    items: Rc<Vec<Item>>,
    thumbs: gtk::SingleSelection,
    thumb_list: gtk::ListView,
    pub selected: Cell<usize>,
    pub zoom: Cell<f64>,
    generation: Cell<u64>,
    dimensions: Cell<(i32, i32)>,
    requests: RefCell<Option<mpsc::Sender<(u64, Source)>>>,
    sender: Sender<Command>,
}
impl Gallery {
    pub fn new(
        parent: &adw::ApplicationWindow,
        dir: PathBuf,
        track: Option<Track>,
        sender: Sender<Command>,
        cache: Rc<RefCell<artwork::ArtCache>>,
        saved: Option<String>,
    ) -> Rc<Self> {
        let items = Rc::new(items(&dir, track));
        let window = adw::Window::builder()
            .transient_for(parent)
            .destroy_with_parent(true)
            .title("Booklet and images — FLACker")
            .default_width(parent.width().clamp(540, 1160))
            .default_height(parent.height().clamp(420, 840))
            .build();
        crate::presentation::theme(&window);
        window.add_css_class("art-viewer");
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("main-surface");
        let header = adw::HeaderBar::new();
        let names: Vec<_> = items.iter().map(|i| i.title.as_str()).collect();
        let select = gtk::DropDown::from_strings(&names);
        select.set_enable_search(true);
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let label = gtk::Label::new(None);
            label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            label.set_max_width_chars(26);
            item.set_child(Some(&label));
        });
        factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            if let (Some(label), Some(text)) = (
                item.child().and_downcast::<gtk::Label>(),
                item.item().and_downcast::<gtk::StringObject>(),
            ) {
                label.set_text(&text.string());
            }
        });
        select.set_factory(Some(&factory));
        select.update_property(&[gtk::accessible::Property::Label("Album image or document")]);
        header.set_title_widget(Some(&select));
        let external = button(
            "document-open-symbolic",
            "Open the original in the system viewer",
        );
        let fullscreen = button("view-fullscreen-symbolic", "Fullscreen (F11)");
        header.pack_start(&external);
        header.pack_end(&fullscreen);
        root.append(&header);
        let picture = gtk::Picture::new();
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_hexpand(true);
        picture.set_vexpand(true);
        picture.set_margin_top(12);
        picture.set_margin_bottom(12);
        picture.set_margin_start(12);
        picture.set_margin_end(12);
        let stage = gtk::ScrolledWindow::builder()
            .hexpand(true)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .child(&picture)
            .build();
        let stack = gtk::Stack::new();
        stack.set_hhomogeneous(false);
        stack.set_vhomogeneous(false);
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stage.add_css_class("art-stage");
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_transition_duration(180);
        stack.add_named(&stage, Some("image"));
        let placeholder = gtk::Box::new(gtk::Orientation::Vertical, 16);
        placeholder.set_halign(gtk::Align::Center);
        placeholder.set_valign(gtk::Align::Center);
        placeholder.set_margin_start(24);
        placeholder.set_margin_end(24);
        let spinner = gtk::Spinner::new();
        placeholder.append(&spinner);
        let notice = gtk::Label::new(None);
        notice.set_wrap(true);
        notice.set_max_width_chars(48);
        notice.set_justify(gtk::Justification::Center);
        placeholder.append(&notice);
        stack.add_named(&placeholder, Some("notice"));
        root.append(&stack);
        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        footer.set_halign(gtk::Align::Center);
        footer.add_css_class("art-toolbar");
        let previous = button("go-previous-symbolic", "Previous image (←)");
        let next = button("go-next-symbolic", "Next image (→)");
        let counter = gtk::Label::new(Some("0 / 0"));
        counter.set_width_chars(8);
        let minus = button("zoom-out-symbolic", "Zoom out (−)");
        let plus = button("zoom-in-symbolic", "Zoom in (+)");
        minus.set_sensitive(false);
        plus.set_sensitive(false);
        let fit = gtk::Button::with_label("Fit");
        fit.set_tooltip_text(Some("Show the whole image (0)"));
        for widget in [
            previous.clone().upcast::<gtk::Widget>(),
            counter.clone().upcast(),
            next.clone().upcast(),
            gtk::Separator::new(gtk::Orientation::Vertical).upcast(),
            minus.clone().upcast(),
            fit.clone().upcast(),
            plus.clone().upcast(),
        ] {
            footer.append(&widget);
        }
        let thumb_names: Vec<_> = items.iter().map(|i| i.title.as_str()).collect();
        let thumbs = gtk::SingleSelection::new(Some(gtk::StringList::new(&thumb_names)));
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, object| {
            let item = object.downcast_ref::<gtk::ListItem>().unwrap();
            let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
            let pic = gtk::Picture::new();
            pic.set_size_request(76, 54);
            pic.set_content_fit(gtk::ContentFit::Contain);
            b.append(&pic);
            let label = gtk::Label::new(None);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_max_width_chars(12);
            b.append(&label);
            b.set_margin_start(5);
            b.set_margin_end(5);
            item.set_child(Some(&b));
        });
        let images = items.clone();
        let art = cache.clone();
        factory.connect_bind(move |_, object| {
            let item = object.downcast_ref::<gtk::ListItem>().unwrap();
            let row = item.child().and_downcast::<gtk::Box>().unwrap();
            let pic = row.first_child().and_downcast::<gtk::Picture>().unwrap();
            if let Some(image) = images.get(item.position() as usize) {
                pic.next_sibling()
                    .and_downcast::<gtk::Label>()
                    .unwrap()
                    .set_text(&image.title);
                row.set_tooltip_text(Some(&image.detail));
                match &image.source {
                    Source::File(p) => art.borrow_mut().request_file(&pic, p, 96),
                    Source::Embedded(t) => art.borrow_mut().request(&pic, t, true, 96),
                }
            }
        });
        factory.connect_unbind(move |_, object| {
            let item = object.downcast_ref::<gtk::ListItem>().unwrap();
            let pic = item
                .child()
                .and_downcast::<gtk::Box>()
                .unwrap()
                .first_child()
                .and_downcast::<gtk::Picture>()
                .unwrap();
            cache.borrow_mut().cancel(&pic);
        });
        let thumb_list = gtk::ListView::new(Some(thumbs.clone()), Some(factory));
        thumb_list.set_orientation(gtk::Orientation::Horizontal);
        thumb_list.set_single_click_activate(true);
        thumb_list.add_css_class("art-thumbnails");
        let strip = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .min_content_height(86)
            .child(&thumb_list)
            .build();
        root.append(&strip);
        root.append(&footer);
        window.set_content(Some(&root));
        let (requests, rx) = mpsc::channel::<(u64, Source)>();
        let (results, incoming) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            while let Ok(mut job) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    job = newer;
                }
                if results.send((job.0, decode(&job.1))).is_err() {
                    break;
                }
            }
        });
        let gallery = Rc::new(Self {
            window,
            directory: dir,
            picture,
            stage,
            stack,
            spinner,
            notice,
            select,
            counter,
            previous,
            next,
            minus,
            plus,
            items,
            thumbs,
            thumb_list,
            selected: Cell::new(usize::MAX),
            zoom: Cell::new(1.),
            generation: Cell::new(0),
            dimensions: Cell::new((0, 0)),
            requests: RefCell::new(Some(requests)),
            sender,
        });
        let weak = Rc::downgrade(&gallery);
        gallery.thumb_list.connect_activate(move |_, i| {
            if let Some(g) = weak.upgrade() {
                g.show(i as usize);
            }
        });
        let weak = Rc::downgrade(&gallery);
        gallery.select.connect_selected_notify(move |s| {
            if let Some(g) = weak.upgrade() {
                g.show(s.selected() as usize);
            }
        });
        for (b, change) in [(&gallery.previous, -1), (&gallery.next, 1)] {
            let weak = Rc::downgrade(&gallery);
            b.connect_clicked(move |_| {
                if let Some(g) = weak.upgrade() {
                    g.step(change);
                }
            });
        }
        for (b, factor) in [(&gallery.minus, 0.75), (&gallery.plus, 1.5)] {
            let weak = Rc::downgrade(&gallery);
            b.connect_clicked(move |_| {
                if let Some(g) = weak.upgrade() {
                    g.set_zoom(g.zoom.get() * factor);
                }
            });
        }
        let weak = Rc::downgrade(&gallery);
        fit.connect_clicked(move |_| {
            if let Some(g) = weak.upgrade() {
                g.set_zoom(1.);
            }
        });
        let weak = Rc::downgrade(&gallery);
        fullscreen.connect_clicked(move |_| {
            if let Some(g) = weak.upgrade() {
                g.toggle_fullscreen();
            }
        });
        let weak = Rc::downgrade(&gallery);
        external.connect_clicked(move |_| {
            if let Some(g) = weak.upgrade() {
                g.open_original();
            }
        });
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(&gallery);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(g) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            match key {
                gdk::Key::Left | gdk::Key::Page_Up => g.step(-1),
                gdk::Key::Right | gdk::Key::Page_Down => g.step(1),
                gdk::Key::plus | gdk::Key::equal | gdk::Key::KP_Add => {
                    g.set_zoom(g.zoom.get() * 1.5)
                }
                gdk::Key::minus | gdk::Key::KP_Subtract => g.set_zoom(g.zoom.get() * 0.75),
                gdk::Key::_0 | gdk::Key::KP_0 => g.set_zoom(1.),
                gdk::Key::F11 => g.toggle_fullscreen(),
                gdk::Key::Escape => {
                    if g.window.is_fullscreen() {
                        g.window.unfullscreen();
                    } else {
                        g.window.close();
                    }
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        gallery.window.add_controller(keys);
        let drag = gtk::GestureDrag::new();
        let origin = Rc::new(Cell::new((0., 0.)));
        let weak = Rc::downgrade(&gallery);
        let start = origin.clone();
        drag.connect_drag_begin(move |_, _, _| {
            if let Some(g) = weak.upgrade() {
                start.set((g.stage.hadjustment().value(), g.stage.vadjustment().value()));
            }
        });
        let weak = Rc::downgrade(&gallery);
        drag.connect_drag_update(move |_, x, y| {
            if let Some(g) = weak.upgrade() {
                if g.zoom.get() > 1. {
                    let (a, b) = origin.get();
                    g.stage.hadjustment().set_value(a - x);
                    g.stage.vadjustment().set_value(b - y);
                }
            }
        });
        gallery.picture.add_controller(drag);
        let weak = Rc::downgrade(&gallery);
        gallery.window.connect_close_request(move |_| {
            if let Some(g) = weak.upgrade() {
                g.picture.set_paintable(gdk::Paintable::NONE);
                g.requests.take();
            }
            glib::Propagation::Proceed
        });
        let weak = Rc::downgrade(&gallery);
        glib::timeout_add_local(Duration::from_millis(50), move || {
            let Some(g) = weak.upgrade().filter(|g| g.window.is_visible()) else {
                return glib::ControlFlow::Break;
            };
            while let Ok((id, result)) = incoming.try_recv() {
                if id != g.generation.get() {
                    continue;
                }
                g.spinner.stop();
                match result {
                    Ok(pixels) => {
                        let pixbuf = gdk_pixbuf::Pixbuf::from_bytes(
                            &pixels.bytes,
                            gdk_pixbuf::Colorspace::Rgb,
                            pixels.alpha,
                            8,
                            pixels.width,
                            pixels.height,
                            pixels.stride,
                        );
                        g.dimensions.set((pixels.width, pixels.height));
                        g.picture
                            .set_paintable(Some(&gdk::Texture::for_pixbuf(&pixbuf)));
                        g.stack.set_visible_child_name("image");
                        g.set_zoom(1.);
                    }
                    Err(e) => {
                        g.notice.set_text(&e);
                        g.stack.set_visible_child_name("notice");
                    }
                }
            }
            glib::ControlFlow::Continue
        });
        external.set_sensitive(!gallery.items.is_empty());
        if gallery.items.is_empty() {
            gallery
                .notice
                .set_text("No local images or documents for this album.");
            gallery.stack.set_visible_child_name("notice");
            gallery.previous.set_sensitive(false);
            gallery.next.set_sensitive(false);
        }
        let index = saved
            .and_then(|key| gallery.items.iter().position(|i| i.detail == key))
            .unwrap_or(0);
        gallery.show(index);
        gallery.window.present();
        gallery
    }
    pub fn show(&self, index: usize) {
        let Some(item) = self.items.get(index) else {
            return;
        };
        if self.selected.replace(index) == index {
            return;
        }
        self.select.set_selected(index as u32);
        self.thumbs.set_selected(index as u32);
        self.thumb_list
            .scroll_to(index as u32, gtk::ListScrollFlags::NONE, None);
        let _ = self.sender.send(Command::GalleryPage(
            self.directory.clone(),
            item.detail.clone(),
        ));
        self.select.set_tooltip_text(Some(&item.detail));
        self.counter
            .set_text(&format!("{} / {}", index + 1, self.items.len()));
        self.previous.set_sensitive(index > 0);
        self.next.set_sensitive(index + 1 < self.items.len());
        self.picture.set_paintable(gdk::Paintable::NONE);
        self.picture.set_size_request(-1, -1);
        self.dimensions.set((0, 0));
        self.minus.set_sensitive(false);
        self.plus.set_sensitive(false);
        self.notice.set_text("Loading image…");
        self.stack.set_visible_child_name("notice");
        self.spinner.start();
        let id = self.generation.get().wrapping_add(1);
        self.generation.set(id);
        if let Some(requests) = self.requests.borrow().as_ref() {
            let _ = requests.send((id, item.source.clone()));
        }
    }
    pub fn step(&self, delta: isize) {
        self.show(self.selected.get().saturating_add_signed(delta));
    }
    pub fn set_zoom(&self, factor: f64) {
        let (w, h) = self.dimensions.get();
        if w == 0 || h == 0 {
            return;
        }
        let factor = factor.clamp(1., 6.);
        self.zoom.set(factor);
        self.minus.set_sensitive(factor > 1.);
        self.plus.set_sensitive(factor < 6.);
        let fit = factor == 1.;
        self.stage.set_policy(
            if fit {
                gtk::PolicyType::Never
            } else {
                gtk::PolicyType::Automatic
            },
            if fit {
                gtk::PolicyType::Never
            } else {
                gtk::PolicyType::Automatic
            },
        );
        if fit {
            self.picture.set_size_request(-1, -1);
        } else {
            let scale = ((self.stage.width() - 24).max(1) as f64 / f64::from(w))
                .min((self.stage.height() - 24).max(1) as f64 / f64::from(h));
            self.picture.set_size_request(
                (f64::from(w) * scale * factor) as i32,
                (f64::from(h) * scale * factor) as i32,
            );
        }
    }
    fn toggle_fullscreen(&self) {
        if self.window.is_fullscreen() {
            self.window.unfullscreen();
        } else {
            self.window.fullscreen();
        }
    }
    fn open_original(&self) {
        let Some(item) = self.items.get(self.selected.get()) else {
            return;
        };
        match &item.source {
            Source::File(p) => {
                let _ = self.sender.send(Command::OpenExternal(p.clone()));
            }
            Source::Embedded(t) => {
                // Extracting a large embedded picture reads and writes megabytes: off the
                // interface thread.
                let track = t.clone();
                let sender = self.sender.clone();
                std::thread::spawn(move || {
                    if let Some(p) = artwork::embedded_original(&track) {
                        let _ = sender.send(Command::OpenExternal(p));
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn page_numbers_sort_numerically() {
        assert_eq!(natural_cmp("booklet2.jpg", "booklet10.jpg"), Ordering::Less);
        assert_eq!(
            natural_cmp("booklet002.jpg", "booklet3.jpg"),
            Ordering::Less
        );
        assert_eq!(
            category(Path::new("back-45920983174.jpg")),
            (1, "Back cover")
        );
    }
}
