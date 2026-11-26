//! Items pulled out of the curtains: each one floats on the video as a glass pill with its name and
//! its control or value, as long as the user likes. A widget is pulled out by its name on a panel,
//! moved by its body, and dropped back onto an open curtain to put it away. It follows the pointer
//! on a spring and stretches a little in the direction it moves. Positions are in DIPs.

use super::panel::{Key, WIDGET_HEIGHT};

/// At most this many widgets: with the caption buttons and the status they fill the shader's slots.
pub const MAX: usize = 9;

pub struct Widget {
    pub key: Key,
    /// Where it is, and where it is going.
    pub x: f32,
    pub y: f32,
    tx: f32,
    ty: f32,
    vx: f32,
    vy: f32,
    /// Half its width at rest; the height is fixed.
    pub half_width: f32,
}

impl Widget {
    pub fn half(&self) -> (f32, f32) {
        (self.half_width, WIDGET_HEIGHT / 2.0)
    }

    /// Its glass, stretched along its motion.
    pub fn stretched(&self) -> (f32, f32) {
        // The prototype's stretch, from its speed per 60 Hz frame.
        let sx = (self.vx.abs() / 60.0 * 0.012).min(0.22);
        let sy = (self.vy.abs() / 60.0 * 0.012).min(0.22);
        let (hw, hh) = self.half();
        (hw * (1.0 + sx - sy * 0.5), hh * (1.0 + sy - sx * 0.5))
    }

    /// Whether (x, y) is on it.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        let (hw, hh) = self.half();
        (x - self.x).abs() <= hw && (y - self.y).abs() <= hh
    }

    /// Its top left corner, where its contents start.
    pub fn origin(&self) -> (f32, f32) {
        let (hw, hh) = self.half();
        (self.x - hw, self.y - hh)
    }
}

/// The spring a widget follows the pointer with: a little overshoot.
const K: f32 = 540.0;
const C: f32 = 23.0;

#[derive(Default)]
pub struct Widgets {
    pub list: Vec<Widget>,
    /// The widget held, and where the pointer holds it from its centre.
    drag: Option<(usize, f32, f32)>,
}

impl Widgets {
    /// Puts `key` at (x, y) at once.
    pub fn add(&mut self, key: Key, x: f32, y: f32, half_width: f32) -> bool {
        if self.list.len() >= MAX {
            return false;
        }
        self.list.push(Widget { key, x, y, tx: x, ty: y, vx: 0.0, vy: 0.0, half_width });
        true
    }

    /// The widget at (x, y), the one on top first.
    pub fn at(&self, x: f32, y: f32) -> Option<usize> {
        self.list.iter().rposition(|w| w.contains(x, y))
    }

    /// Holds widget `i` where the pointer took it.
    pub fn hold(&mut self, i: usize, x: f32, y: f32) {
        let w = &self.list[i];
        self.drag = Some((i, x - w.tx, y - w.ty));
    }

    /// Pulls a new widget for `key` out at the pointer and holds it by its centre.
    pub fn pull(&mut self, key: Key, x: f32, y: f32, half_width: f32) -> bool {
        if !self.add(key, x, y, half_width) {
            return false;
        }
        self.drag = Some((self.list.len() - 1, 0.0, 0.0));
        true
    }

    /// The pointer moved: the held widget heads there, inside the window.
    pub fn drag_to(&mut self, x: f32, y: f32, size: (f32, f32)) {
        if let Some((i, ox, oy)) = self.drag {
            let w = &mut self.list[i];
            let (hw, hh) = w.half();
            w.tx = (x - ox).clamp(hw, (size.0 - hw).max(hw));
            w.ty = (y - oy).clamp(hh, (size.1 - hh).max(hh));
        }
    }

    /// Lets go; returns the widget that was held.
    pub fn release(&mut self) -> Option<usize> {
        self.drag.take().map(|d| d.0)
    }

    pub fn remove(&mut self, i: usize) {
        self.list.remove(i);
    }

    /// The window changed size: widgets keep their place in proportion, inside it.
    pub fn rescale(&mut self, from: (f32, f32), to: (f32, f32)) {
        if from.0 <= 0.0 || from.1 <= 0.0 {
            return;
        }
        for w in &mut self.list {
            let (hw, hh) = w.half();
            let fit = |v: f32, s: f32, t: f32, h: f32| (v * t / s).clamp(h, (t - h).max(h));
            w.x = fit(w.x, from.0, to.0, hw);
            w.tx = fit(w.tx, from.0, to.0, hw);
            w.y = fit(w.y, from.1, to.1, hh);
            w.ty = fit(w.ty, from.1, to.1, hh);
        }
    }

    /// Advances by `dt` seconds; returns whether any widget moves.
    pub fn step(&mut self, dt: f32) -> bool {
        let n = (dt * 240.0).ceil().max(1.0);
        let h = dt / n;
        let mut moving = self.drag.is_some();
        for w in &mut self.list {
            for _ in 0..n as usize {
                w.vx += (K * (w.tx - w.x) - C * w.vx) * h;
                w.vy += (K * (w.ty - w.y) - C * w.vy) * h;
                w.x += w.vx * h;
                w.y += w.vy * h;
            }
            if (w.x - w.tx).abs() < 0.05 && (w.y - w.ty).abs() < 0.05 && w.vx.abs() < 1.0 && w.vy.abs() < 1.0 {
                (w.x, w.y, w.vx, w.vy) = (w.tx, w.ty, 0.0, 0.0);
            } else {
                moving = true;
            }
        }
        moving
    }

    /// Where each widget is, as a fraction of the window, for the settings.
    pub fn save(&self, size: (f32, f32)) -> Vec<(Key, f64, f64)> {
        self.list.iter().map(|w| (w.key, (w.tx / size.0.max(1.0)) as f64, (w.ty / size.1.max(1.0)) as f64)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (f32, f32) = (960.0, 540.0);

    #[test]
    fn a_pulled_widget_follows_the_pointer_and_settles() {
        let mut w = Widgets::default();
        assert!(w.pull(Key::Zoom, 100.0, 100.0, 80.0));
        w.drag_to(400.0, 100.0, SIZE);
        let mut frames = 0;
        while w.step(1.0 / 60.0) && frames < 600 {
            frames += 1;
            if frames == 3 {
                let (rw, rh) = w.list[0].stretched();
                assert!(rw > 80.0 && rh < 20.0, "stretched along its motion: {rw} {rh}");
            }
            if frames == 120 {
                w.release();
            }
        }
        assert!(frames < 600);
        assert_eq!((w.list[0].x, w.list[0].y), (400.0, 100.0));
    }

    #[test]
    fn widgets_stay_inside_the_window() {
        let mut w = Widgets::default();
        w.pull(Key::Fps, 100.0, 100.0, 60.0);
        w.drag_to(2000.0, -50.0, SIZE);
        w.release();
        for _ in 0..300 {
            w.step(1.0 / 60.0);
        }
        assert_eq!((w.list[0].x, w.list[0].y), (900.0, 20.0));
        w.rescale(SIZE, (480.0, 270.0));
        assert_eq!((w.list[0].x, w.list[0].y), (420.0, 20.0));
    }

    #[test]
    fn held_by_where_it_was_taken() {
        let mut w = Widgets::default();
        w.add(Key::Zoom, 200.0, 200.0, 80.0);
        assert_eq!(w.at(250.0, 210.0), Some(0));
        assert_eq!(w.at(290.0, 210.0), None);
        w.hold(0, 250.0, 210.0);
        w.drag_to(350.0, 310.0, SIZE);
        assert_eq!((w.list[0].tx, w.list[0].ty), (300.0, 300.0));
        assert_eq!(w.save(SIZE)[0].0, Key::Zoom);
    }

    #[test]
    fn no_more_than_the_shader_holds() {
        let mut w = Widgets::default();
        for _ in 0..MAX {
            assert!(w.add(Key::Phone, 100.0, 100.0, 50.0));
        }
        assert!(!w.pull(Key::Fps, 0.0, 0.0, 50.0));
    }
}
