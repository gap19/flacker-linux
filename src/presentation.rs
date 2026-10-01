//! Presentation resources and shared native components. No player or library state.
use adw::prelude::*;
use gtk::{glib, pango};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, ffi::CString, path::PathBuf, sync::Once};

#[link(name = "fontconfig")]
unsafe extern "C" {
    fn FcConfigGetCurrent() -> *mut std::ffi::c_void;
    fn FcConfigAppFontAddFile(config: *mut std::ffi::c_void, file: *const u8) -> i32;
    fn FcConfigBuildFonts(config: *mut std::ffi::c_void) -> i32;
}

/// Register embedded OFL fonts privately with this process, never system-wide.
pub fn fonts() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let root = std::env::var_os("FLACKER_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| glib::user_cache_dir().join("flacker"))
            .join("fonts");
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            std::fs::create_dir_all(&root)?;
            // Fontconfig owns the configuration pointer for the process lifetime.
            let config = unsafe { FcConfigGetCurrent() };
            if config.is_null() {
                return Err("Fontconfig unavailable".into());
            }
            for (name, bytes) in [
                (
                    "LeagueGothic-Regular",
                    include_bytes!("../assets/fonts/LeagueGothic-Regular.otf").as_slice(),
                ),
                (
                    "SourceSans3-Regular",
                    include_bytes!("../assets/fonts/SourceSans3-Regular.otf").as_slice(),
                ),
                (
                    "SourceSans3-Semibold",
                    include_bytes!("../assets/fonts/SourceSans3-Semibold.otf").as_slice(),
                ),
                (
                    "SourceSans3-Bold",
                    include_bytes!("../assets/fonts/SourceSans3-Bold.otf").as_slice(),
                ),
                (
                    "SourceSans3-It",
                    include_bytes!("../assets/fonts/SourceSans3-It.otf").as_slice(),
                ),
            ] {
                let digest = format!("{:x}", Sha256::digest(bytes));
                let path = root.join(format!("{name}-{}.otf", &digest[..16]));
                if std::fs::read(&path).ok().as_deref() != Some(bytes) {
                    let temp = root.join(format!("{name}-{}.tmp", std::process::id()));
                    std::fs::write(&temp, bytes)?;
                    std::fs::rename(&temp, &path)?;
                }
                let path = CString::new(path.as_os_str().as_encoded_bytes())?;
                // CString remains alive throughout the call; Fontconfig copies the path.
                if unsafe { FcConfigAppFontAddFile(config, path.as_ptr().cast()) } == 0 {
                    return Err("Could not register the font".into());
                }
            }
            unsafe { FcConfigBuildFonts(config) };
            Ok(())
        })();
        if let Err(e) = result {
            eprintln!("FLACker fonts: {e}; using the system font.");
        }
    });
}

pub fn theme(widget: &impl IsA<gtk::Widget>) {
    let widget = widget.as_ref();
    widget.add_css_class("flacker");
    let apply = |w: &gtk::Widget, dark| {
        w.remove_css_class(if dark { "light" } else { "dark" });
        w.add_css_class(if dark { "dark" } else { "light" });
    };
    let manager = adw::StyleManager::default();
    apply(widget, manager.is_dark());
    let weak = widget.downgrade();
    let id = RefCell::new(Some(manager.connect_dark_notify(move |m| {
        if let Some(w) = weak.upgrade() {
            apply(&w, m.is_dark());
        }
    })));
    widget.connect_destroy(move |_| {
        if let Some(id) = id.take() {
            manager.disconnect(id);
        }
    });
}

pub fn title(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.);
    label.set_wrap(true);
    label.set_wrap_mode(pango::WrapMode::WordChar);
    label.add_css_class(class);
    label
}

/// Count with the correct singular or plural noun.
pub fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Add a control to an action row. The wrapper is invisible to focus and
/// hover so only the control itself shows state.
pub fn add_action(row: &gtk::FlowBox, widget: &impl IsA<gtk::Widget>) {
    let child = gtk::FlowBoxChild::new();
    child.set_focusable(false);
    child.set_child(Some(widget));
    row.insert(&child, -1);
}

/// A naturally wrapping row of real buttons; no custom focus handling.
pub fn actions() -> gtk::FlowBox {
    let row = gtk::FlowBox::new();
    row.set_selection_mode(gtk::SelectionMode::None);
    row.set_min_children_per_line(1);
    row.set_max_children_per_line(4);
    row.set_column_spacing(8);
    row.set_row_spacing(8);
    row.set_halign(gtk::Align::Start);
    row.add_css_class("action-flow");
    row
}

/// Keep artwork in its original aspect ratio, independent of its frame.
pub fn cover(picture: &gtk::Picture, edge: i32) -> CoverFrame {
    picture.set_can_shrink(true);
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.add_css_class("album-cover");
    let aspect: CoverFrame = glib::Object::new();
    use gtk::subclass::prelude::*;
    aspect.imp().edge.set(edge);
    aspect.imp().picture.replace(Some(picture.clone()));
    picture.set_parent(&aspect);
    aspect
}

mod frame {
    use super::*;
    use gtk::subclass::prelude::*;
    use std::cell::Cell;
    #[derive(Default)]
    pub struct CoverFrame {
        pub edge: Cell<i32>,
        pub picture: RefCell<Option<gtk::Picture>>,
    }
    #[glib::object_subclass]
    impl ObjectSubclass for CoverFrame {
        const NAME: &'static str = "FlackerCoverFrame";
        type Type = super::CoverFrame;
        type ParentType = gtk::Widget;
    }
    impl ObjectImpl for CoverFrame {
        fn dispose(&self) {
            if let Some(p) = self.picture.take() {
                p.unparent();
            }
        }
    }
    impl WidgetImpl for CoverFrame {
        fn measure(&self, _: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            // Texture resolution must not become the natural layout size.
            (self.edge.get().min(96), self.edge.get(), -1, -1)
        }
        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(p) = self.picture.borrow().as_ref() {
                p.allocate(width, height, baseline, None);
            }
        }
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            if let Some(p) = self.picture.borrow().as_ref() {
                self.obj().snapshot_child(p, snapshot);
            }
        }
    }
}
glib::wrapper! {
    pub struct CoverFrame(ObjectSubclass<frame::CoverFrame>)
        @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

/// Crossfade only when an asynchronously loaded image actually changes.
/// Hidden windows skip the effect; the outgoing texture is released on completion.
pub fn fading_cover(picture: &gtk::Picture, edge: i32) -> gtk::Overlay {
    use std::rc::Rc;
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&cover(picture, edge)));
    let old = gtk::Picture::new();
    old.set_content_fit(gtk::ContentFit::Contain);
    old.set_can_shrink(true);
    old.set_can_target(false);
    overlay.add_overlay(&old);
    let previous = RefCell::new(picture.paintable());
    let animation: Rc<RefCell<Option<adw::TimedAnimation>>> = Rc::default();
    let running = animation.clone();
    let weak = old.downgrade();
    picture.connect_paintable_notify(move |p| {
        let outgoing = previous.replace(p.paintable());
        let Some(old) = weak.upgrade() else { return };
        if let Some(a) = running.take() {
            a.pause();
        }
        old.set_paintable(gtk::gdk::Paintable::NONE);
        if !p.is_mapped()
            || p.paintable().is_none()
            || outgoing == p.paintable()
            || !gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations())
        {
            return;
        }
        let Some(outgoing) = outgoing else { return };
        old.set_paintable(Some(&outgoing));
        old.set_opacity(1.);
        let w = old.downgrade();
        let target = adw::CallbackAnimationTarget::new(move |value| {
            if let Some(old) = w.upgrade() {
                old.set_opacity(1. - value);
            }
        });
        let a = adw::TimedAnimation::new(p, 0., 1., 180, target);
        let w = old.downgrade();
        let done = Rc::downgrade(&running);
        a.connect_done(move |_| {
            if let Some(old) = w.upgrade() {
                old.set_paintable(gtk::gdk::Paintable::NONE);
            }
            if let Some(done) = done.upgrade() {
                done.take();
            }
        });
        running.replace(Some(a.clone()));
        a.play();
    });
    let w = old.downgrade();
    overlay.connect_unmap(move |_| {
        if let Some(a) = animation.take() {
            a.pause();
        }
        if let Some(old) = w.upgrade() {
            old.set_paintable(gtk::gdk::Paintable::NONE);
        }
    });
    overlay
}
