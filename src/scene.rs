//! Native artwork surfaces and interaction-driven depth. No audio or library state.
use adw::prelude::*;
use gtk::{gdk, glib, graphene, gsk, subclass::prelude::*};
use std::cell::{Cell, RefCell};

fn rect(w: f32, h: f32) -> graphene::Rect {
    graphene::Rect::new(0., 0., w, h)
}
fn color(r: f32, g: f32, b: f32, a: f32) -> gdk::RGBA {
    gdk::RGBA::new(r, g, b, a)
}
fn artwork_bounds(widget: &gtk::Widget, width: f32, height: f32) -> graphene::Rect {
    if let Some(p) = widget.downcast_ref::<gtk::Picture>() {
        let ratio = p
            .paintable()
            .map_or(1., |p| p.intrinsic_aspect_ratio() as f32)
            .max(0.01);
        let w = width.min(height * ratio);
        let h = w / ratio;
        return graphene::Rect::new((width - w) / 2., (height - h) / 2., w, h);
    }
    if let Some(child) = widget.first_child() {
        return artwork_bounds(&child, width, height);
    }
    rect(width, height)
}

mod stage {
    use super::*;
    #[derive(Default)]
    pub struct Stage {
        pub child: RefCell<Option<gtk::Widget>>,
        pub image: RefCell<Option<gdk::Paintable>>,
        pub background: RefCell<Option<gsk::RenderNode>>,
        pub dimensions: Cell<(i32, i32)>,
    }
    #[glib::object_subclass]
    impl ObjectSubclass for Stage {
        const NAME: &'static str = "FlackerArtworkStage";
        type Type = super::Stage;
        type ParentType = gtk::Widget;
    }
    impl ObjectImpl for Stage {
        fn dispose(&self) {
            if let Some(child) = self.child.take() {
                child.unparent();
            }
            self.image.take();
            self.background.take();
        }
    }
    impl WidgetImpl for Stage {
        fn measure(&self, o: gtk::Orientation, s: i32) -> (i32, i32, i32, i32) {
            let (minimum, natural, minimum_baseline, natural_baseline) = self
                .child
                .borrow()
                .as_ref()
                .map_or((0, 0, -1, -1), |c| c.measure(o, s));
            // The page switches composition at its breakpoint. Do not let the
            // previous wide composition prevent the window reaching that width.
            (
                minimum.min(if o == gtk::Orientation::Horizontal {
                    300
                } else {
                    160
                }),
                natural,
                minimum_baseline,
                natural_baseline,
            )
        }
        fn size_allocate(&self, w: i32, h: i32, b: i32) {
            if self.dimensions.replace((w, h)) != (w, h) {
                self.background.take();
            }
            if let Some(c) = self.child.borrow().as_ref() {
                c.allocate(w, h, b, None);
            }
        }
        fn snapshot(&self, s: &gtk::Snapshot) {
            let obj = self.obj();
            let w = obj.width() as f32;
            let h = obj.height() as f32;
            if w <= 0. || h <= 0. {
                return;
            }
            let bounds = rect(w, h);
            s.push_rounded_clip(&gsk::RoundedRect::from_rect(bounds, 24.));
            if self.background.borrow().is_none() {
                let bg = gtk::Snapshot::new();
                bg.append_color(&color(0.055, 0.068, 0.07, 1.), &bounds);
                if let Some(image) = self.image.borrow().as_ref() {
                    let ratio = (image.intrinsic_aspect_ratio() as f32).max(0.1);
                    let iw = (w * 1.12).max(h * ratio * 1.12);
                    let ih = iw / ratio;
                    bg.save();
                    bg.push_blur(28.);
                    bg.translate(&graphene::Point::new((w - iw) * 0.5, (h - ih) * 0.45));
                    image.snapshot(&bg, iw as f64, ih as f64);
                    bg.pop();
                    bg.restore();
                    bg.append_linear_gradient(
                        &bounds,
                        &graphene::Point::new(0., 0.),
                        &graphene::Point::new(w, h * 0.35),
                        &[
                            gsk::ColorStop::new(0., color(0.02, 0.03, 0.035, 0.91)),
                            gsk::ColorStop::new(0.55, color(0.02, 0.03, 0.035, 0.64)),
                            gsk::ColorStop::new(1., color(0.02, 0.03, 0.035, 0.23)),
                        ],
                    );
                    bg.append_linear_gradient(
                        &bounds,
                        &graphene::Point::new(0., h * 0.3),
                        &graphene::Point::new(0., h),
                        &[
                            gsk::ColorStop::new(0., color(0.02, 0.025, 0.03, 0.)),
                            gsk::ColorStop::new(1., color(0.02, 0.025, 0.03, 0.55)),
                        ],
                    );
                }
                self.background.replace(bg.to_node());
            }
            if let Some(bg) = self.background.borrow().as_ref() {
                s.append_node(bg);
            }
            if let Some(c) = self.child.borrow().as_ref() {
                obj.snapshot_child(c, s);
            }
            s.pop();
        }
    }
}
glib::wrapper! {
    pub struct Stage(ObjectSubclass<stage::Stage>) @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}
impl Stage {
    pub fn new(child: &impl IsA<gtk::Widget>, picture: &gtk::Picture) -> Self {
        let w: Self = glib::Object::new();
        w.add_css_class("artwork-stage");
        child.add_css_class("scene-content");
        child.set_parent(&w);
        w.imp().child.replace(Some(child.as_ref().clone()));
        w.imp().image.replace(picture.paintable());
        let weak = w.downgrade();
        picture.connect_paintable_notify(move |p| {
            if let Some(w) = weak.upgrade() {
                w.imp().image.replace(p.paintable());
                w.imp().background.take();
                w.queue_draw();
            }
        });
        w
    }
}

mod sleeve {
    use super::*;
    #[derive(Default)]
    pub struct Sleeve {
        pub child: RefCell<Option<gtk::Widget>>,
        pub pose: Cell<(f32, f32, f32)>,
        pub animation: RefCell<Option<adw::TimedAnimation>>,
        pub rest: Cell<f32>,
    }
    #[glib::object_subclass]
    impl ObjectSubclass for Sleeve {
        const NAME: &'static str = "FlackerSleeve";
        type Type = super::Sleeve;
        type ParentType = gtk::Widget;
    }
    impl ObjectImpl for Sleeve {
        fn dispose(&self) {
            if let Some(a) = self.animation.take() {
                a.pause();
            }
            if let Some(c) = self.child.take() {
                c.unparent();
            }
        }
    }
    impl WidgetImpl for Sleeve {
        fn measure(&self, o: gtk::Orientation, s: i32) -> (i32, i32, i32, i32) {
            self.child
                .borrow()
                .as_ref()
                .map_or((0, 0, -1, -1), |c| c.measure(o, s))
        }
        fn size_allocate(&self, w: i32, h: i32, b: i32) {
            if let Some(c) = self.child.borrow().as_ref() {
                c.allocate(w, h, b, None);
            }
        }
        fn snapshot(&self, s: &gtk::Snapshot) {
            let obj = self.obj();
            let w = obj.width() as f32;
            let h = obj.height() as f32;
            if w <= 0. || h <= 0. {
                return;
            }
            let (x, y, lift) = self.pose.get();
            s.save();
            s.translate(&graphene::Point::new(w / 2., h / 2. - lift * 12.));
            let software = obj
                .native()
                .and_then(|n| n.renderer())
                .is_some_and(|r| r.is::<gsk::CairoRenderer>());
            if software {
                // Cairo cannot render projective nodes. Keep the interaction
                // legible using an affine lift/tilt instead of a pink fallback.
                s.rotate(x * 2. - y);
            } else if lift > 0.001 {
                s.perspective(900.);
                s.rotate_3d(-y * 10., &graphene::Vec3::new(1., 0., 0.));
                s.rotate_3d(x * 12., &graphene::Vec3::new(0., 1., 0.));
            }
            s.rotate(self.rest.get());
            s.scale(1. + lift * 0.035, 1. + lift * 0.035);
            s.translate(&graphene::Point::new(-w / 2., -h / 2.));
            let bounds = self
                .child
                .borrow()
                .as_ref()
                .map_or(rect(w, h), |c| artwork_bounds(c, w, h));
            s.append_outset_shadow(
                &gsk::RoundedRect::from_rect(bounds, 2.),
                &color(0., 0., 0., 0.23 + lift * 0.17),
                0.,
                7. + lift * 9.,
                0.,
                18. + lift * 10.,
            );
            if let Some(c) = self.child.borrow().as_ref() {
                obj.snapshot_child(c, s);
            }
            if lift > 0.001 {
                s.push_clip(&bounds);
                let center = (0.5 + x * 0.18) * w;
                s.append_linear_gradient(
                    &bounds,
                    &graphene::Point::new(center - w * 0.6, 0.),
                    &graphene::Point::new(center + w * 0.6, h),
                    &[
                        gsk::ColorStop::new(0., color(1., 1., 1., 0.)),
                        gsk::ColorStop::new(0.48, color(1., 1., 1., lift * 0.18)),
                        gsk::ColorStop::new(0.58, color(1., 1., 1., lift * 0.02)),
                        gsk::ColorStop::new(1., color(1., 1., 1., 0.)),
                    ],
                );
                s.pop();
            }
            s.restore();
        }
    }
}
glib::wrapper! {
    pub struct Sleeve(ObjectSubclass<sleeve::Sleeve>) @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}
impl Sleeve {
    pub fn new(child: &impl IsA<gtk::Widget>, rest: f32) -> Self {
        let w: Self = glib::Object::new();
        child.set_parent(&w);
        w.imp().child.replace(Some(child.as_ref().clone()));
        w.imp().rest.set(rest);
        w.set_overflow(gtk::Overflow::Visible);
        let motion = gtk::EventControllerMotion::new();
        let weak = w.downgrade();
        motion.connect_motion(move |_, x, y| {
            if let Some(w) = weak.upgrade() {
                w.pose(
                    ((x as f32 / w.width().max(1) as f32) * 2. - 1.).clamp(-1., 1.),
                    ((y as f32 / w.height().max(1) as f32) * 2. - 1.).clamp(-1., 1.),
                    1.,
                );
            }
        });
        let weak = w.downgrade();
        motion.connect_leave(move |_| {
            if let Some(w) = weak.upgrade() {
                w.pose(0., 0., 0.);
            }
        });
        w.add_controller(motion);
        w.connect_unmap(|w| w.reset());
        w
    }
    pub fn child(&self) -> gtk::Widget {
        self.imp().child.borrow().as_ref().unwrap().clone()
    }
    pub fn reset(&self) {
        if let Some(a) = self.imp().animation.take() {
            a.pause();
        }
        self.imp().pose.set((0., 0., 0.));
        self.queue_draw();
    }
    pub fn pose(&self, x: f32, y: f32, lift: f32) {
        if !self.is_mapped()
            || !gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations())
        {
            self.reset();
            return;
        }
        if let Some(a) = self.imp().animation.take() {
            a.pause();
        }
        let from = self.imp().pose.get();
        let weak = self.downgrade();
        let target = adw::CallbackAnimationTarget::new(move |v| {
            if let Some(w) = weak.upgrade() {
                let t = v as f32;
                w.imp().pose.set((
                    from.0 + (x - from.0) * t,
                    from.1 + (y - from.1) * t,
                    from.2 + (lift - from.2) * t,
                ));
                w.queue_draw();
            }
        });
        let a = adw::TimedAnimation::new(self, 0., 1., if lift == 0. { 380 } else { 180 }, target);
        a.set_easing(adw::Easing::EaseOutCubic);
        let weak = self.downgrade();
        a.connect_done(move |_| {
            if let Some(w) = weak.upgrade() {
                w.imp().animation.take();
            }
        });
        self.imp().animation.replace(Some(a.clone()));
        a.play();
    }
}

/// One reveal per navigation, with no idle animation and no retained source page.
pub fn reveal(widget: &impl IsA<gtk::Widget>) {
    let weak = widget.as_ref().downgrade();
    widget.add_tick_callback(move |_, _| {
        if let Some(w) = weak.upgrade() {
            if !gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations()) {
                return glib::ControlFlow::Break;
            }
            let weak = w.downgrade();
            let target = adw::CallbackAnimationTarget::new(move |v| {
                if let Some(w) = weak.upgrade() {
                    w.set_opacity(v);
                }
            });
            let a = adw::TimedAnimation::new(&w, 0., 1., 260, target);
            a.set_easing(adw::Easing::EaseOutCubic);
            a.play();
            // Adwaita retains a playing animation until completion.
        }
        glib::ControlFlow::Break
    });
}
