//! `Stage`: fills the layer surface and places the panel next to the bar,
//! horizontally centered on the click and clamped to the screen. A click on
//! the empty area around the panel closes the board.

use gtk::glib;
use gtk::graphene::Point;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use std::cell::{Cell, RefCell};
use yard_core::model::{Anchor, Edge};

const SIDE_MARGIN: f64 = 10.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

/// Where the panel goes, given the surface size and the anchor. Returns the
/// panel rectangle and the scale origin in panel coordinates.
pub fn place(anchor: Option<&Anchor>, sw: f64, sh: f64, pw: f64, ph: f64, gap: f64) -> (Rect, (f64, f64)) {
    let pw = pw.min(sw - 2.0 * SIDE_MARGIN).max(1.0);
    let ph = ph.min(sh - 2.0 * gap).max(1.0);
    let clamp_x = |x: f64| x.clamp(SIDE_MARGIN, (sw - pw - SIDE_MARGIN).max(SIDE_MARGIN));
    let clamp_y = |y: f64| y.clamp(gap, (sh - ph - gap).max(gap));
    let a = match anchor {
        Some(a) if !a.center => a,
        _ => {
            let x = (sw - pw) / 2.0;
            let y = ((sh - ph) / 2.0 * 0.85).max(gap);
            return (Rect { x, y, w: pw, h: ph }, (pw / 2.0, ph / 2.0));
        }
    };
    // The surface starts after the space other surfaces reserve.
    let lx = a.x - a.reserved[0];
    let ly = a.y - a.reserved[1];
    match a.edge {
        Edge::Bottom => {
            let x = clamp_x(lx - pw / 2.0);
            let y = sh - gap - ph;
            (Rect { x, y, w: pw, h: ph }, ((lx - x).clamp(0.0, pw), ph + gap))
        }
        Edge::Top => {
            let x = clamp_x(lx - pw / 2.0);
            let y = gap;
            (Rect { x, y, w: pw, h: ph }, ((lx - x).clamp(0.0, pw), -gap))
        }
        Edge::Left => {
            let y = clamp_y(ly - ph / 2.0);
            let x = gap;
            (Rect { x, y, w: pw, h: ph }, (-gap, (ly - y).clamp(0.0, ph)))
        }
        Edge::Right => {
            let y = clamp_y(ly - ph / 2.0);
            let x = sw - gap - pw;
            (Rect { x, y, w: pw, h: ph }, (pw + gap, (ly - y).clamp(0.0, ph)))
        }
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Stage {
        pub anchor: RefCell<Option<Anchor>>,
        pub panel_width: Cell<f64>,
        pub max_height: Cell<f64>,
        pub gap: Cell<f64>,
        pub rect: Cell<Rect>,
        pub origin: Cell<(f64, f64)>,
        pub on_outside: RefCell<Option<Box<dyn Fn()>>>,
        pub on_placed: RefCell<Option<Box<dyn Fn((f64, f64))>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Stage {
        const NAME: &'static str = "YardStage";
        type Type = super::Stage;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Stage {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            let click = gtk::GestureClick::new();
            click.set_button(0);
            let weak = obj.downgrade();
            click.connect_pressed(move |g, _, x, y| {
                let Some(stage) = weak.upgrade() else { return };
                if !stage.imp().rect.get().contains(x, y) {
                    g.set_state(gtk::EventSequenceState::Claimed);
                    if let Some(f) = stage.imp().on_outside.borrow().as_ref() {
                        f();
                    }
                }
            });
            obj.add_controller(click);
        }

        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Stage {
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            (0, 0, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let obj = self.obj();
            let Some(child) = obj.first_child() else { return };
            let (sw, sh) = (width as f64, height as f64);
            let gap = self.gap.get();
            let want_w = self.panel_width.get().min(sw - 2.0 * SIDE_MARGIN);
            let (min_w, _, _, _) = child.measure(gtk::Orientation::Horizontal, -1);
            let pw = want_w.max(min_w as f64);
            let (min_h, nat_h, _, _) = child.measure(gtk::Orientation::Vertical, pw as i32);
            let max_h = (sh * self.max_height.get()).max(min_h as f64);
            let ph = (nat_h as f64).min(max_h).max(min_h as f64);
            let anchor = self.anchor.borrow().clone();
            let (rect, origin) = place(anchor.as_ref(), sw, sh, pw, ph, gap);
            self.rect.set(rect);
            self.origin.set(origin);
            let t = gtk::gsk::Transform::new().translate(&Point::new(rect.x.round() as f32, rect.y.round() as f32));
            child.allocate(rect.w.round() as i32, rect.h.round() as i32, -1, Some(t));
            if let Some(f) = self.on_placed.borrow().as_ref() {
                f(origin);
            }
        }
    }
}

glib::wrapper! {
    pub struct Stage(ObjectSubclass<imp::Stage>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Stage {
    pub fn new(child: &impl IsA<gtk::Widget>, width: f64, max_height: f64, gap: f64) -> Stage {
        let s: Stage = glib::Object::new();
        child.set_parent(&s);
        s.imp().panel_width.set(width);
        s.imp().max_height.set(max_height);
        s.imp().gap.set(gap);
        s
    }

    pub fn set_anchor(&self, a: Option<Anchor>) {
        *self.imp().anchor.borrow_mut() = a;
        self.queue_allocate();
    }

    pub fn set_geometry(&self, width: f64, max_height: f64, gap: f64) {
        self.imp().panel_width.set(width);
        self.imp().max_height.set(max_height);
        self.imp().gap.set(gap);
        self.queue_resize();
    }

    pub fn connect_outside(&self, f: impl Fn() + 'static) {
        *self.imp().on_outside.borrow_mut() = Some(Box::new(f));
    }

    pub fn connect_placed(&self, f: impl Fn((f64, f64)) + 'static) {
        *self.imp().on_placed.borrow_mut() = Some(Box::new(f));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(edge: Edge, x: f64, y: f64) -> Anchor {
        Anchor { edge, x, y, reserved: [0.0, 0.0, 0.0, 24.0], width: 2560.0, height: 1440.0, ..Default::default() }
    }

    #[test]
    fn bottom_right_click_clamps() {
        let (r, o) = place(Some(&anchor(Edge::Bottom, 2500.0, 1428.0)), 2560.0, 1416.0, 640.0, 400.0, 8.0);
        assert_eq!(r.x, 2560.0 - 640.0 - SIDE_MARGIN);
        assert_eq!(r.y, 1416.0 - 8.0 - 400.0);
        assert_eq!(o.1, 408.0);
        assert!(o.0 > 500.0 && o.0 <= 640.0);
    }

    #[test]
    fn bottom_middle_centers() {
        let (r, o) = place(Some(&anchor(Edge::Bottom, 1280.0, 1428.0)), 2560.0, 1416.0, 640.0, 400.0, 8.0);
        assert_eq!(r.x, 960.0);
        assert_eq!(o.0, 320.0);
    }

    #[test]
    fn top_and_center() {
        let mut a = anchor(Edge::Top, 100.0, 10.0);
        a.reserved = [0.0, 30.0, 0.0, 0.0];
        let (r, o) = place(Some(&a), 2560.0, 1410.0, 640.0, 400.0, 8.0);
        assert_eq!(r.y, 8.0);
        assert_eq!(r.x, SIDE_MARGIN);
        assert_eq!(o.1, -8.0);
        let (r, _) = place(None, 1000.0, 800.0, 600.0, 400.0, 8.0);
        assert_eq!(r.x, 200.0);
    }

    #[test]
    fn tiny_screen_shrinks() {
        let (r, _) = place(None, 400.0, 300.0, 640.0, 500.0, 8.0);
        assert_eq!(r.w, 380.0);
        assert_eq!(r.h, 284.0);
    }
}
