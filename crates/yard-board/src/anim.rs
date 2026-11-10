//! `Spring`: a single-child container that draws its child scaled, offset and
//! faded around an origin, driven by a damped spring on the frame clock.

use gtk::glib;
use gtk::graphene::Point;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use std::cell::{Cell, RefCell};

#[derive(Clone, Copy, Debug)]
pub struct Motion {
    /// Scale at progress 0.
    pub from_scale: f64,
    /// Vertical offset at progress 0, in pixels.
    pub from_dy: f64,
    pub stiffness: f64,
    pub damping: f64,
}

impl Motion {
    pub const PANEL: Motion = Motion { from_scale: 0.45, from_dy: 26.0, stiffness: 340.0, damping: 24.0 };
    pub const TILE: Motion = Motion { from_scale: 0.9, from_dy: 12.0, stiffness: 420.0, damping: 26.0 };
}

mod imp {
    use super::*;

    pub struct Spring {
        pub progress: Cell<f64>,
        pub velocity: Cell<f64>,
        pub target: Cell<f64>,
        pub motion: Cell<Motion>,
        /// Origin of the scale, in the child's coordinates.
        pub origin: Cell<(f64, f64)>,
        pub last_frame: Cell<i64>,
        /// Frame time (us) before which the animation holds still.
        pub start_at: Cell<i64>,
        pub delay_us: Cell<i64>,
        pub ticking: Cell<bool>,
        /// Closing uses a quick ease instead of the spring.
        pub closing: Cell<Option<(i64, f64)>>,
        pub on_done: RefCell<Option<Box<dyn Fn()>>>,
    }

    impl Default for Spring {
        fn default() -> Self {
            Spring {
                progress: Cell::new(1.0),
                velocity: Cell::new(0.0),
                target: Cell::new(1.0),
                motion: Cell::new(Motion::PANEL),
                origin: Cell::new((0.0, 0.0)),
                last_frame: Cell::new(0),
                start_at: Cell::new(0),
                delay_us: Cell::new(0),
                ticking: Cell::new(false),
                closing: Cell::new(None),
                on_done: RefCell::new(None),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Spring {
        const NAME: &'static str = "YardSpring";
        type Type = super::Spring;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_layout_manager_type::<gtk::BinLayout>();
        }
    }

    impl ObjectImpl for Spring {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Spring {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let p = self.progress.get();
            if (p - 1.0).abs() < 1e-4 {
                self.parent_snapshot(snapshot);
                return;
            }
            let m = self.motion.get();
            let scale = (m.from_scale + (1.0 - m.from_scale) * p).max(0.01);
            let opacity = (p * 1.7).clamp(0.0, 1.0);
            if opacity <= 0.001 {
                return;
            }
            let dy = (1.0 - p) * m.from_dy;
            let (ox, oy) = self.origin.get();
            snapshot.save();
            snapshot.translate(&Point::new(ox as f32, (oy + dy) as f32));
            snapshot.scale(scale as f32, scale as f32);
            snapshot.translate(&Point::new(-ox as f32, -oy as f32));
            snapshot.push_opacity(opacity);
            self.parent_snapshot(snapshot);
            snapshot.pop();
            snapshot.restore();
        }
    }
}

glib::wrapper! {
    pub struct Spring(ObjectSubclass<imp::Spring>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for Spring {
    fn default() -> Self {
        glib::Object::new()
    }
}

pub fn animations_enabled() -> bool {
    gtk::Settings::default()
        .map(|s| s.is_gtk_enable_animations())
        .unwrap_or(true)
}

impl Spring {
    pub fn new(child: &impl IsA<gtk::Widget>, motion: Motion) -> Spring {
        let s = Spring::default();
        child.set_parent(&s);
        s.imp().motion.set(motion);
        s
    }

    pub fn set_origin(&self, x: f64, y: f64) {
        self.imp().origin.set((x, y));
    }

    pub fn set_motion(&self, m: Motion) {
        self.imp().motion.set(m);
    }

    /// Runs the open motion from nothing, optionally after a delay.
    pub fn play_in(&self, delay_ms: u32) {
        let imp = self.imp();
        if !animations_enabled() {
            imp.progress.set(1.0);
            self.queue_draw();
            return;
        }
        imp.closing.set(None);
        imp.progress.set(0.0);
        imp.velocity.set(0.0);
        imp.target.set(1.0);
        imp.last_frame.set(0);
        imp.start_at.set(0);
        imp.delay_us.set(delay_ms as i64 * 1000);
        self.queue_draw();
        self.ensure_ticking();
    }

    /// Runs a quick ease out, then calls `done`.
    pub fn play_out(&self, done: impl Fn() + 'static) {
        let imp = self.imp();
        if !animations_enabled() {
            imp.progress.set(1.0);
            done();
            return;
        }
        *imp.on_done.borrow_mut() = Some(Box::new(done));
        imp.closing.set(Some((0, imp.progress.get())));
        self.ensure_ticking();
    }

    pub fn settle(&self) {
        let imp = self.imp();
        imp.progress.set(1.0);
        imp.velocity.set(0.0);
        imp.closing.set(None);
        self.queue_draw();
    }

    fn ensure_ticking(&self) {
        let imp = self.imp();
        if imp.ticking.get() {
            return;
        }
        imp.ticking.set(true);
        self.add_tick_callback(|w, clock| {
            let imp = w.imp();
            let t = clock.frame_time();
            if imp.start_at.get() == 0 {
                imp.start_at.set(t + imp.delay_us.get());
            }
            if let Some((start, from)) = imp.closing.get() {
                let start = if start == 0 {
                    imp.closing.set(Some((t, from)));
                    t
                } else {
                    start
                };
                let k = ((t - start) as f64 / 170_000.0).clamp(0.0, 1.0);
                let eased = k * k;
                imp.progress.set(from * (1.0 - eased));
                w.queue_draw();
                if k >= 1.0 {
                    imp.closing.set(None);
                    imp.ticking.set(false);
                    if let Some(done) = imp.on_done.borrow_mut().take() {
                        done();
                    }
                    return glib::ControlFlow::Break;
                }
                return glib::ControlFlow::Continue;
            }
            if t < imp.start_at.get() {
                imp.last_frame.set(t);
                return glib::ControlFlow::Continue;
            }
            let last = imp.last_frame.get();
            let dt = if last == 0 { 1.0 / 60.0 } else { ((t - last) as f64 / 1e6).clamp(0.0, 0.05) };
            imp.last_frame.set(t);
            let m = imp.motion.get();
            let (mut x, mut v) = (imp.progress.get(), imp.velocity.get());
            // Integrate in small steps so a dropped frame cannot blow up.
            let steps = ((dt / 0.004).ceil() as usize).max(1);
            let h = dt / steps as f64;
            for _ in 0..steps {
                let a = -m.stiffness * (x - imp.target.get()) - m.damping * v;
                v += a * h;
                x += v * h;
            }
            imp.progress.set(x);
            imp.velocity.set(v);
            w.queue_draw();
            if (x - imp.target.get()).abs() < 0.0015 && v.abs() < 0.01 {
                imp.progress.set(imp.target.get());
                imp.ticking.set(false);
                w.queue_draw();
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }
}
