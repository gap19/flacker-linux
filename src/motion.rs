//! A short-lived artwork overlay. It never retains a recycled album cell.
use adw::prelude::*;
use gtk::{gdk, glib, graphene, subclass::prelude::*};
use std::cell::{Cell, RefCell};

#[derive(Clone)]
pub struct Anchor {
    pub key: String,
    pub bounds: graphene::Rect,
    pub image: gdk::Paintable,
}

mod imp {
    use super::*;
    #[derive(Default)]
    pub struct CoverMotion {
        pub frame: RefCell<Option<(gdk::Paintable, graphene::Rect)>>,
    }
    #[glib::object_subclass]
    impl ObjectSubclass for CoverMotion {
        const NAME: &'static str = "FlackerCoverMotion";
        type Type = super::CoverMotion;
        type ParentType = gtk::Widget;
    }
    impl ObjectImpl for CoverMotion {}
    impl WidgetImpl for CoverMotion {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            if let Some((image, bounds)) = self.frame.borrow().as_ref() {
                let rect = super::contain(image, bounds);
                snapshot.save();
                snapshot.translate(&graphene::Point::new(rect.x(), rect.y()));
                image.snapshot(snapshot, rect.width() as f64, rect.height() as f64);
                snapshot.restore();
            }
        }
    }
}
glib::wrapper! {
    pub struct CoverMotion(ObjectSubclass<imp::CoverMotion>)
        @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}
impl Default for CoverMotion {
    fn default() -> Self {
        let w: Self = glib::Object::new();
        w.set_can_target(false);
        w.set_focusable(false);
        w.set_hexpand(true);
        w.set_vexpand(true);
        w
    }
}
fn contain(image: &gdk::Paintable, b: &graphene::Rect) -> graphene::Rect {
    let ratio = image.intrinsic_aspect_ratio() as f32;
    if ratio <= 0. || b.height() <= 0. {
        return *b;
    }
    let w = b.width().min(b.height() * ratio);
    let h = w / ratio;
    graphene::Rect::new(
        b.x() + (b.width() - w) / 2.,
        b.y() + (b.height() - h) / 2.,
        w,
        h,
    )
}

pub fn find_cover(widget: &gtk::Widget, key: &str) -> Option<gtk::Picture> {
    if widget.widget_name() == format!("cover:{key}") {
        if let Some(p) = widget.downcast_ref::<gtk::Picture>() {
            return Some(p.clone());
        }
    }
    let mut child = widget.first_child();
    while let Some(w) = child {
        if let Some(p) = find_cover(&w, key) {
            return Some(p);
        }
        child = w.next_sibling();
    }
    None
}

mod row {
    use super::*;
    #[derive(Default)]
    pub struct SlideRow {
        pub child: RefCell<Option<gtk::Box>>,
        pub offset: Cell<f32>,
        pub generation: Cell<u64>,
        pub animation: RefCell<Option<adw::TimedAnimation>>,
    }
    #[glib::object_subclass]
    impl ObjectSubclass for SlideRow {
        const NAME: &'static str = "FlackerSlideRow";
        type Type = super::SlideRow;
        type ParentType = gtk::Widget;
    }
    impl ObjectImpl for SlideRow {
        fn dispose(&self) {
            if let Some(a) = self.animation.take() {
                a.pause();
            }
            if let Some(child) = self.child.take() {
                child.unparent();
            }
        }
    }
    impl WidgetImpl for SlideRow {
        fn measure(&self, direction: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            self.child
                .borrow()
                .as_ref()
                .map_or((0, 0, -1, -1), |c| c.measure(direction, for_size))
        }
        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.child.borrow().as_ref() {
                child.allocate(width, height, baseline, None);
            }
        }
        fn snapshot(&self, s: &gtk::Snapshot) {
            if let Some(child) = self.child.borrow().as_ref() {
                s.save();
                s.translate(&graphene::Point::new(0., self.offset.get()));
                self.obj().snapshot_child(child, s);
                s.restore();
            }
        }
    }
}
glib::wrapper! {
    pub struct SlideRow(ObjectSubclass<row::SlideRow>)
        @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}
impl SlideRow {
    pub fn new(child: &gtk::Box) -> Self {
        let w: Self = glib::Object::new();
        child.set_parent(&w);
        w.imp().child.replace(Some(child.clone()));
        w
    }
    pub fn child(&self) -> gtk::Box {
        self.imp().child.borrow().as_ref().unwrap().clone()
    }
    pub fn reset(&self) {
        self.imp()
            .generation
            .set(self.imp().generation.get().wrapping_add(1));
        if let Some(a) = self.imp().animation.take() {
            a.pause();
        }
        self.imp().offset.set(0.);
        self.queue_draw();
    }
    pub fn shift(&self, rows: i32) {
        self.reset();
        if rows == 0 || !gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations()) {
            return;
        }
        let generation = self.imp().generation.get();
        self.add_tick_callback(move |w, _| {
            if !w.is_mapped() || w.imp().generation.get() != generation {
                return glib::ControlFlow::Break;
            }
            let distance = rows.clamp(-2, 2) as f32 * w.height() as f32;
            let weak = w.downgrade();
            let target = adw::CallbackAnimationTarget::new(move |v| {
                if let Some(w) = weak.upgrade() {
                    w.imp().offset.set(distance * (1. - v as f32));
                    w.queue_draw();
                }
            });
            let a = adw::TimedAnimation::new(w, 0., 1., 180, target);
            let weak = w.downgrade();
            a.connect_done(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.reset();
                }
            });
            w.imp().animation.replace(Some(a.clone()));
            a.play();
            glib::ControlFlow::Break
        });
    }
}

#[derive(Default)]
pub struct Motion {
    pub layer: CoverMotion,
    pub pending: RefCell<Option<Anchor>>,
    pub requested: RefCell<Option<Anchor>>,
    generation: Cell<u64>,
    animation: RefCell<Option<adw::TimedAnimation>>,
    hidden: RefCell<Option<glib::WeakRef<gtk::Picture>>>,
    pub started: Cell<u32>,
    pub completed: Cell<u32>,
}
impl Motion {
    pub fn cancel(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        if let Some(a) = self.animation.take() {
            a.pause();
        }
        if let Some(w) = self.hidden.take().and_then(|w| w.upgrade()) {
            w.set_opacity(1.);
        }
        self.layer.imp().frame.take();
        self.layer.queue_draw();
        self.pending.take();
        self.requested.take();
    }
    pub fn anchor(&self, key: String, picture: &gtk::Picture) -> Option<Anchor> {
        if !picture.is_mapped() {
            return None;
        }
        let bounds = picture.compute_bounds(&self.layer)?;
        if bounds.width() < 1.
            || bounds.height() < 1.
            || bounds.x() < -bounds.width()
            || bounds.y() < -bounds.height()
            || bounds.y() > self.layer.height() as f32
        {
            return None;
        }
        Some(Anchor {
            key,
            bounds,
            image: picture.paintable()?,
        })
    }
    pub fn animate(self: &std::rc::Rc<Self>, from: Anchor, picture: &gtk::Picture, duration: u32) {
        if !gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations()) {
            return;
        }
        let Some(to) = picture.compute_bounds(&self.layer) else {
            return;
        };
        if to.width() <= 0. || to.height() <= 0. {
            return;
        }
        self.started.set(self.started.get() + 1);
        let size = (self.layer.width(), self.layer.height());
        self.hidden.replace(Some(picture.downgrade()));
        picture.set_opacity(0.);
        let weak = std::rc::Rc::downgrade(self);
        let target = adw::CallbackAnimationTarget::new(move |v| {
            if let Some(m) = weak.upgrade() {
                if (m.layer.width(), m.layer.height()) != size {
                    m.cancel();
                    return;
                }
                let t = v as f32;
                let lerp = |a, b| a + (b - a) * t;
                let rect = graphene::Rect::new(
                    lerp(from.bounds.x(), to.x()),
                    lerp(from.bounds.y(), to.y()),
                    lerp(from.bounds.width(), to.width()),
                    lerp(from.bounds.height(), to.height()),
                );
                m.layer
                    .imp()
                    .frame
                    .replace(Some((from.image.clone(), rect)));
                m.layer.queue_draw();
            }
        });
        let a = adw::TimedAnimation::new(&self.layer, 0., 1., duration, target);
        a.set_easing(adw::Easing::EaseInOutCubic);
        a.set_follow_enable_animations_setting(true);
        let weak = std::rc::Rc::downgrade(self);
        a.connect_done(move |_| {
            if let Some(m) = weak.upgrade() {
                m.completed.set(m.completed.get() + 1);
                m.cancel();
            }
        });
        self.animation.replace(Some(a.clone()));
        a.play();
    }
    /// Await allocation and scroll restoration, bounded to twelve frame cycles.
    pub fn arrive(self: &std::rc::Rc<Self>, key: String, picture: &gtk::Picture, frames: u8) {
        if !self.pending.borrow().as_ref().is_some_and(|a| a.key == key) {
            return;
        }
        let weak = std::rc::Rc::downgrade(self);
        let destination = picture.downgrade();
        let generation = self.generation.get();
        let ticks = Cell::new(0u8);
        self.layer.add_tick_callback(move |_, _| {
            let Some(m) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if m.generation.get() != generation {
                return glib::ControlFlow::Break;
            }
            ticks.set(ticks.get() + 1);
            if ticks.get() >= frames {
                if let Some(p) = destination
                    .upgrade()
                    .filter(|p| p.is_mapped() && p.widget_name() == format!("cover:{key}"))
                {
                    if let Some(anchor) = m.pending.take() {
                        m.animate(anchor, &p, 320);
                    }
                    return glib::ControlFlow::Break;
                }
            }
            if ticks.get() >= 12 {
                m.pending.take();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
        self.layer.queue_draw();
    }

    pub fn is_active(&self) -> bool {
        self.animation.borrow().is_some()
            || self.hidden.borrow().is_some()
            || self.pending.borrow().is_some()
    }
    /// A filtered or off-screen destination may never bind. Release its anchor
    /// even when no cell calls arrive(), without leaving an idle timer running.
    pub fn expire_pending(self: &std::rc::Rc<Self>) {
        if self.pending.borrow().is_none() {
            return;
        }
        let weak = std::rc::Rc::downgrade(self);
        let generation = self.generation.get();
        let frames = Cell::new(0);
        self.layer.add_tick_callback(move |_, _| {
            let Some(m) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if generation != m.generation.get() || m.pending.borrow().is_none() {
                return glib::ControlFlow::Break;
            }
            frames.set(frames.get() + 1);
            if frames.get() >= 16 {
                m.pending.take();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }
}
