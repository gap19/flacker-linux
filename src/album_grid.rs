//! Recycled album tiles. The model keeps one cover reference per album, not its tracks.
use crate::wake::Sender;
use crate::{
    artwork::ArtCache,
    commands::{Command, View},
    model::{Album, Track},
};
use gtk::{gio, glib, prelude::*};
use std::{cell::RefCell, rc::Rc};

struct Tile {
    key: String,
    title: String,
    artist: String,
    details: String,
    cover: Option<Track>,
}
fn parts(item: &gtk::ListItem) -> (gtk::Box, gtk::Picture, gtk::Label, gtk::Label, gtk::Label) {
    let row = item.child().and_downcast::<gtk::Box>().unwrap();
    let aspect = row
        .first_child()
        .and_downcast::<crate::scene::Sleeve>()
        .unwrap()
        .child()
        .downcast::<crate::presentation::CoverFrame>()
        .unwrap();
    let picture = aspect.first_child().and_downcast::<gtk::Picture>().unwrap();
    let title = row
        .first_child()
        .unwrap()
        .next_sibling()
        .and_downcast::<gtk::Label>()
        .unwrap();
    let artist = title.next_sibling().and_downcast::<gtk::Label>().unwrap();
    let details = artist.next_sibling().and_downcast::<gtk::Label>().unwrap();
    (row, picture, title, artist, details)
}
pub fn build(
    albums: Vec<Rc<Album>>,
    cache: Rc<RefCell<ArtCache>>,
    embedded: bool,
    sender: Sender<Command>,
    cover_size: u32,
    short: bool,
    motion: Rc<crate::motion::Motion>,
) -> gtk::GridView {
    let model = gio::ListStore::new::<glib::BoxedAnyObject>();
    for a in albums {
        let details = format!(
            "{}{}{}",
            a.year,
            if a.year.is_empty() { "" } else { "  ·  " },
            crate::presentation::count(a.tracks.len(), "track", "tracks")
        );
        model.append(&glib::BoxedAnyObject::new(Tile {
            key: a.key.clone(),
            title: a.title.clone(),
            artist: a.artist.clone(),
            details,
            cover: a.tracks.first().cloned(),
        }));
    }
    let factory = gtk::SignalListItemFactory::new();
    let sizes = if short {
        [96, 104, 136]
    } else {
        [104, 136, 176]
    };
    let edge = sizes[cover_size.min(2) as usize];
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let tile = gtk::Box::new(gtk::Orientation::Vertical, 2);
        tile.add_css_class("album-tile");
        tile.set_width_request(edge + 12);
        tile.set_valign(gtk::Align::Start);
        let picture = gtk::Picture::new();
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.add_css_class("album-cover");
        let aspect = crate::presentation::cover(&picture, edge);
        aspect.set_size_request(edge, edge);
        aspect.set_halign(gtk::Align::Center);
        aspect.set_valign(gtk::Align::Start);
        let sleeve = crate::scene::Sleeve::new(&aspect, 0.);
        sleeve.set_halign(gtk::Align::Center);
        sleeve.set_valign(gtk::Align::Start);
        tile.append(&sleeve);
        for class in ["album-title", "album-artist", "album-year"] {
            let label = gtk::Label::new(None);
            label.set_xalign(0.);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_max_width_chars(26);
            if class == "album-title" {
                label.set_wrap(true);
                label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                label.set_lines(2);
                label.set_valign(gtk::Align::Start);
            }
            label.add_css_class(class);
            tile.append(&label);
        }
        item.set_child(Some(&tile));
    });
    let unbind_cache = cache.clone();
    let activated_motion = motion.clone();
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let object = item.item().and_downcast::<glib::BoxedAnyObject>().unwrap();
        let tile = object.borrow::<Tile>();
        let (row, picture, title, artist, details) = parts(item);
        picture.set_widget_name(&format!("cover:{}", tile.key));
        title.set_text(&tile.title);
        artist.set_text(&tile.artist);
        details.set_text(&tile.details);
        row.set_tooltip_text(Some(&format!("{} — {}", tile.artist, tile.title)));
        if let Some(t) = tile.cover.as_ref() {
            cache
                .borrow_mut()
                .request(&picture, t, embedded, crate::artwork::THUMB);
            motion.arrive(tile.key.clone(), &picture, 10);
        }
    });
    factory.connect_unbind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        unbind_cache.borrow_mut().cancel(&parts(item).1);
        if let Some(sleeve) = item
            .child()
            .and_then(|r| r.first_child())
            .and_downcast::<crate::scene::Sleeve>()
        {
            sleeve.reset();
        }
    });
    let grid = gtk::GridView::new(
        Some(gtk::NoSelection::new(Some(model.clone()))),
        Some(factory),
    );
    grid.set_min_columns(1);
    grid.set_max_columns(12);
    grid.set_single_click_activate(true);
    grid.add_css_class("album-grid");
    grid.connect_activate(move |grid, pos| {
        if let Some(object) = model.item(pos).and_downcast::<glib::BoxedAnyObject>() {
            let tile = object.borrow::<Tile>();
            if let Some(p) = crate::motion::find_cover(grid.upcast_ref(), &tile.key) {
                activated_motion
                    .requested
                    .replace(activated_motion.anchor(tile.key.clone(), &p));
            }
            let _ = sender.send(Command::View(View::Album(tile.key.clone())));
        }
    });
    grid
}
