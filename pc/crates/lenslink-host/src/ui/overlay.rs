//! The window's floating glass: the caption buttons and the status. They come and go like drops:
//! each grows from its centre on a spring, overshoots a little and settles; buttons swell under the
//! pointer and give when pressed. Positions are in DIPs.

use super::render::{Element, Label};

/// Caption buttons, top right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Minimize,
    Close,
}

const BUTTON: f32 = 30.0;
const BUTTON_GAP: f32 = 8.0;
const MARGIN: f32 = 12.0;
/// Room around a label inside its pill.
const PAD: (f32, f32) = (28.0, 14.0);

fn button_centre(b: Button, width: f32) -> (f32, f32) {
    let right = width - MARGIN - if b == Button::Minimize { BUTTON + BUTTON_GAP } else { 0.0 };
    (right - BUTTON / 2.0, MARGIN + BUTTON / 2.0)
}

pub fn button_at(x: f32, y: f32, width: f32) -> Option<Button> {
    [Button::Minimize, Button::Close].into_iter().find(|&b| {
        let (cx, cy) = button_centre(b, width);
        // A little slack around each button.
        (x - cx).abs() <= BUTTON / 2.0 + 3.0 && (y - cy).abs() <= BUTTON / 2.0 + 3.0
    })
}

/// A damped spring towards a target; returns whether it still moves.
#[derive(Default, Clone, Copy)]
struct Spring {
    x: f32,
    v: f32,
}

impl Spring {
    fn step(&mut self, target: f32, dt: f32, k: f32, c: f32) -> bool {
        // Small steps keep the explicit integration stable whatever the frame time.
        let n = (dt * k.sqrt() / 0.25).ceil().max(1.0);
        let h = dt / n;
        for _ in 0..n as usize {
            self.v += (k * (target - self.x) - c * self.v) * h;
            self.x += self.v * h;
        }
        if (self.x - target).abs() < 1e-3 && self.v.abs() < 1e-2 {
            self.x = target;
            self.v = 0.0;
            return false;
        }
        true
    }

    fn snap(&mut self, x: f32) {
        self.x = x;
        self.v = 0.0;
    }
}

/// Coming and going: a little overshoot, like a drop.
const SHOW: (f32, f32) = (240.0, 19.0);
/// Hover and press: quick, barely any overshoot.
const HOT: (f32, f32) = (420.0, 34.0);
/// A pill following its text's size.
const SIZE: (f32, f32) = (300.0, 30.0);

#[derive(Default)]
struct Pill {
    shown: Spring,
    w: Spring,
    h: Spring,
    text: String,
    /// The width the text wraps at.
    wrap: f32,
}

/// Size changes smaller than this follow at once: a statistics line that changes every second
/// should not keep the window animating.
const SIZE_SNAP: f32 = 16.0;

impl Pill {
    /// `measure` gives the text's size wrapped at `wrap`; the text stays while the pill leaves.
    fn step(&mut self, show: bool, text: &str, wrap: f32, measure: impl Fn(&str, f32) -> (f32, f32), dt: f32) -> bool {
        let show = show && !text.is_empty();
        if show && self.text != text {
            self.text = text.to_string();
        }
        self.wrap = wrap;
        let (tw, th) = measure(&self.text, wrap);
        let (w, h) = (tw + PAD.0, th + PAD.1);
        if self.shown.x <= 0.0 || (self.w.v == 0.0 && (w - self.w.x).abs() < SIZE_SNAP) {
            self.w.snap(w);
        }
        if self.shown.x <= 0.0 || (self.h.v == 0.0 && (h - self.h.x).abs() < SIZE_SNAP) {
            self.h.snap(h);
        }
        let mut moving = self.shown.step(if show { 1.0 } else { 0.0 }, dt, SHOW.0, SHOW.1);
        moving |= self.w.step(w, dt, SIZE.0, SIZE.1);
        moving |= self.h.step(h, dt, SIZE.0, SIZE.1);
        moving
    }
}

/// What the overlay shows now.
pub struct Input<'a> {
    pub size: (f32, f32),
    /// The pointer is over the window.
    pub hover: bool,
    pub hot: Option<Button>,
    pub pressed: Option<Button>,
    /// Frames are arriving: the status is a statistics line, shown only on hover.
    pub live: bool,
    pub status: &'a str,
}

#[derive(Default)]
pub struct Overlay {
    caption: Spring,
    hot: [Spring; 2],
    press: [Spring; 2],
    /// The status in the middle when there is nothing to watch.
    big: Pill,
    /// The statistics line, bottom left, on hover.
    small: Pill,
}

/// Labels fade in once their drop is big enough to hold them.
fn label_alpha(shown: f32) -> f32 {
    ((shown - 0.55) / 0.4).clamp(0.0, 1.0)
}

fn index(b: Button) -> usize {
    b as usize
}

impl Overlay {
    /// Advances the animation by `dt` seconds. `measure` gives a text's size in DIPs (large or small
    /// font, wrapped at a width). Returns whether anything still moves.
    pub fn step(&mut self, dt: f32, input: &Input, measure: impl Fn(&str, bool, f32) -> (f32, f32)) -> bool {
        let (w, _) = input.size;
        let mut moving = self.caption.step(if input.hover { 1.0 } else { 0.0 }, dt, SHOW.0, SHOW.1);
        for b in [Button::Minimize, Button::Close] {
            let i = index(b);
            moving |= self.hot[i].step(if input.hot == Some(b) { 1.0 } else { 0.0 }, dt, HOT.0, HOT.1);
            moving |= self.press[i].step(if input.pressed == Some(b) { 1.0 } else { 0.0 }, dt, HOT.0, HOT.1);
        }
        let wrap = |max: f32| (max - PAD.0).max(40.0);
        moving |= self.big.step(!input.live, input.status, wrap(w - 64.0), |t, wr| measure(t, true, wr), dt);
        moving |= self.small.step(input.live && input.hover, input.status, wrap(w - 2.0 * MARGIN), |t, wr| measure(t, false, wr), dt);
        moving
    }

    pub fn elements(&self, size: (f32, f32)) -> Vec<Element<'_>> {
        let (w, h) = size;
        let mut out = Vec::new();
        fn pill(p: &Pill, centre: (f32, f32), large: bool) -> Element<'_> {
            Element {
                centre,
                half: (p.w.x / 2.0 * p.shown.x, p.h.x / 2.0 * p.shown.x),
                tint: [0.0; 4],
                label: Label::Text { text: &p.text, large, wrap: p.wrap },
                label_alpha: label_alpha(p.shown.x),
            }
        }
        if self.big.shown.x > 0.04 {
            out.push(pill(&self.big, (w / 2.0, h / 2.0), true));
        }
        if self.small.shown.x > 0.04 {
            let s = &self.small;
            out.push(pill(s, (MARGIN + s.w.x / 2.0, h - MARGIN - s.h.x / 2.0), false));
        }

        if self.caption.x > 0.04 {
            for (b, glyph) in [(Button::Minimize, "\u{E921}"), (Button::Close, "\u{E8BB}")] {
                let i = index(b);
                let (hot, press) = (self.hot[i].x, self.press[i].x);
                let r = BUTTON / 2.0 * self.caption.x * (1.0 + 0.1 * hot - 0.12 * press);
                let tint = match b {
                    Button::Close => [0.92, 0.2, 0.18, 0.7 * hot.clamp(0.0, 1.0)],
                    Button::Minimize => [1.0, 1.0, 1.0, (0.14 * hot + 0.1 * press).clamp(0.0, 1.0)],
                };
                out.push(Element {
                    centre: button_centre(b, w),
                    half: (r, r),
                    tint,
                    label: Label::Glyph(glyph),
                    label_alpha: label_alpha(self.caption.x),
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spring_settles_on_its_target_with_any_frame_time() {
        for dt in [1.0 / 240.0, 1.0 / 60.0, 1.0 / 30.0] {
            let mut s = Spring::default();
            let mut peak: f32 = 0.0;
            let mut frames = 0;
            while s.step(1.0, dt, SHOW.0, SHOW.1) {
                peak = peak.max(s.x);
                frames += 1;
                assert!(frames < 2000, "never settles at dt {dt}");
            }
            assert_eq!(s.x, 1.0);
            // A drop overshoots a little, but not much.
            assert!(peak > 1.02 && peak < 1.2, "peak {peak} at dt {dt}");
        }
    }

    #[test]
    fn buttons_are_found_with_a_little_slack() {
        let w = 800.0;
        let (cx, cy) = button_centre(Button::Close, w);
        assert_eq!(button_at(cx, cy, w), Some(Button::Close));
        assert_eq!(button_at(cx - 17.0, cy, w), Some(Button::Close));
        let (mx, _) = button_centre(Button::Minimize, w);
        assert_eq!(button_at(mx, cy + 17.0, w), Some(Button::Minimize));
        assert_eq!(button_at(w / 2.0, cy, w), None);
    }
}
