//! The four curtains: each window edge is a string of springs. At rest the string sits just outside
//! the window; pulled, it follows the finger with a soft spring and sends waves along itself; let
//! go, it snaps open or shut, carrying the flick on as a travelling wave; under the pointer it
//! bulges out a little to show it can be grabbed. Lengths in DIPs, time in seconds.

/// Points on each edge.
pub const POINTS: usize = 48;
/// How far outside the window a closed curtain rests: further than the drops near the edge reach.
pub const REST: f32 = -24.0;
/// How far a curtain opens: the left and right panels' width, the top and bottom panels' height.
pub const DEPTH_SIDE: f32 = 300.0;
pub const DEPTH_END: f32 = 196.0;
/// The band along an edge, past the edge's current position, where a press grabs the curtain.
pub const GRAB: f32 = 26.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

pub const SIDES: [Side; 4] = [Side::Left, Side::Right, Side::Top, Side::Bottom];

impl Side {
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn depth(self) -> f32 {
        if self.index() < 2 {
            DEPTH_SIDE
        } else {
            DEPTH_END
        }
    }

    /// Position along the edge (u) and depth into the window from it (v).
    pub fn uv(self, x: f32, y: f32, size: (f32, f32)) -> (f32, f32) {
        match self {
            Side::Left => (y, x),
            Side::Right => (y, size.0 - x),
            Side::Top => (x, y),
            Side::Bottom => (x, size.1 - y),
        }
    }

    /// The edge's length.
    fn len(self, size: (f32, f32)) -> f32 {
        if self.index() < 2 {
            size.1
        } else {
            size.0
        }
    }

    /// How far into the window the edge may go.
    fn deep(self, size: (f32, f32)) -> f32 {
        (if self.index() < 2 { size.0 } else { size.1 }) * 0.9
    }
}

/// How the curtains move: the bottom curtain's sliders.
#[derive(Clone, Copy, Debug)]
pub struct Motion {
    pub stiffness: f32,
    pub viscosity: f32,
    /// Width of the waves.
    pub wave: f32,
    pub amplitude: f32,
}

impl Default for Motion {
    fn default() -> Self {
        Motion { stiffness: 1.0, viscosity: 1.0, wave: 0.7, amplitude: 1.3 }
    }
}

#[derive(Clone)]
pub struct Edge {
    pub x: [f32; POINTS],
    v: [f32; POINTS],
    /// Where the edge rests: shut or open.
    target: f32,
    pub open: bool,
}

/// A curtain held by the pointer.
#[derive(Clone, Copy, Debug)]
struct Drag {
    side: Side,
    /// Pointer along the edge and into the window.
    u: f32,
    v: f32,
    /// Pointer depth minus the edge's mean depth when grabbed: the curtain moves by its grip.
    grab: f32,
    /// Smoothed speed into the window, for the flick on release.
    speed: f32,
}

pub struct Curtains {
    pub edges: [Edge; 4],
    drag: Option<Drag>,
}

impl Default for Curtains {
    fn default() -> Self {
        let edge = Edge { x: [REST; POINTS], v: [0.0; POINTS], target: REST, open: false };
        Curtains { edges: std::array::from_fn(|_| edge.clone()), drag: None }
    }
}

fn mean(a: &[f32]) -> f32 {
    a.iter().sum::<f32>() / a.len() as f32
}

impl Edge {
    /// Depth of the edge at `u` along it.
    pub fn at(&self, u: f32, len: f32) -> f32 {
        let f = (u / len.max(1.0)).clamp(0.0, 1.0) * (POINTS - 1) as f32;
        let i = (f.floor() as usize).min(POINTS - 2);
        let t = f - i as f32;
        self.x[i] * (1.0 - t) + self.x[i + 1] * t
    }

    pub fn mean(&self) -> f32 {
        mean(&self.x)
    }

    pub fn max(&self) -> f32 {
        self.x.iter().copied().fold(f32::MIN, f32::max)
    }

    /// Visible at all.
    pub fn shows(&self) -> bool {
        self.max() > 0.0
    }
}

impl Curtains {
    pub fn edge(&self, side: Side) -> &Edge {
        &self.edges[side.index()]
    }

    /// Depth of `side`'s edge at the point (x, y)'s position along it.
    pub fn edge_at(&self, side: Side, x: f32, y: f32, size: (f32, f32)) -> f32 {
        let (u, _) = side.uv(x, y, size);
        self.edge(side).at(u, side.len(size))
    }

    /// Whether (x, y) is inside the curtain on `side` (behind its edge).
    pub fn inside(&self, side: Side, x: f32, y: f32, size: (f32, f32)) -> bool {
        let (_, v) = side.uv(x, y, size);
        v < self.edge_at(side, x, y, size)
    }

    /// The curtain a press at (x, y) would grab: the open curtain it is in, else the nearest edge
    /// within reach.
    pub fn pick(&self, x: f32, y: f32, size: (f32, f32)) -> Option<Side> {
        if let Some(side) = SIDES.into_iter().find(|&s| self.edge(s).open && self.inside(s, x, y, size)) {
            return Some(side);
        }
        let mut best = None;
        let mut best_d = f32::MAX;
        for side in SIDES {
            let (_, v) = side.uv(x, y, size);
            // A closed curtain is grabbed by the window's edge.
            let e = self.edge_at(side, x, y, size).max(0.0);
            if v < e + GRAB && (v - e).abs() < best_d {
                best_d = (v - e).abs();
                best = Some(side);
            }
        }
        best
    }

    pub fn press(&mut self, side: Side, x: f32, y: f32, size: (f32, f32)) {
        let (u, v) = side.uv(x, y, size);
        let grab = v - self.edge(side).mean().max(0.0);
        self.drag = Some(Drag { side, u, v, grab, speed: 0.0 });
    }

    /// The pointer moved by `dt` seconds since the last event.
    pub fn drag_to(&mut self, x: f32, y: f32, dt: f32, size: (f32, f32)) {
        if let Some(d) = self.drag.as_mut() {
            let (u, v) = d.side.uv(x, y, size);
            let speed = ((v - d.v) / dt.max(0.001)).clamp(-4000.0, 4000.0);
            d.speed = 0.6 * d.speed + 0.4 * speed;
            d.u = u;
            d.v = v;
            }
    }

    /// Let go: the curtain snaps open or shut, and the flick travels along the edge as a wave.
    pub fn release(&mut self, m: &Motion, size: (f32, f32)) {
        let Some(d) = self.drag.take() else { return };
        let len = d.side.len(size);
        let sigma = 70.0 * m.wave;
        let e = &mut self.edges[d.side.index()];
        for (i, v) in e.v.iter_mut().enumerate() {
            let q = (i as f32 / (POINTS - 1) as f32 * len - d.u) / sigma;
            *v += d.speed * 0.35 * m.amplitude * (-q * q).exp();
        }
        let depth = d.side.depth();
        e.open = e.mean() + mean(&e.v) * 0.15 > depth * 0.5;
        e.target = if e.open { depth } else { REST };
    }

    /// Advances by `dt` seconds. `hover` is the pointer when nothing is held and it is free to make
    /// an edge bulge. Returns whether anything still moves.
    pub fn step(&mut self, dt: f32, m: &Motion, hover: Option<(f32, f32)>, size: (f32, f32)) -> bool {
        let s2 = m.stiffness * m.stiffness;
        let tension = 20000.0 * m.wave * m.wave;
        let sigma = 70.0 * m.wave;
        let mut moving = false;
        for side in SIDES {
            let len = side.len(size);
            let deep = side.deep(size);
            let drag = self.drag.filter(|d| d.side == side);
            let e = &mut self.edges[side.index()];
            let (k, c) = if drag.is_some() {
                (60.0 * s2 / (0.4 + 0.6 * m.amplitude), 16.0 * m.viscosity)
            } else {
                (170.0 * s2, 12.0 * m.viscosity)
            };
            let base = match drag {
                Some(d) => (d.v - d.grab).clamp(-20.0, deep),
                None => e.target,
            };
            // The bulge under the pointer is measured from the window's edge, even for a curtain
            // resting outside it.
            let shown = e.target.max(0.0);
            let hover = match (drag, self.drag, hover) {
                (None, None, Some((x, y))) => {
                    let (hu, hv) = side.uv(x, y, size);
                    (hv > shown - 10.0 && hv < shown + 90.0).then_some((hu, hv))
                }
                _ => None,
            };
            // Explicit integration is stable only while dt·ω stays small: stiffer strings take
            // more steps.
            let n = (dt * (4.0 * tension + k + 1400.0).sqrt() / 1.2).ceil().max(1.0);
            let h = dt / n;
            let mut peak_a: f32 = 0.0;
            for _ in 0..n as usize {
                peak_a = 0.0;
                for i in 0..POINTS {
                    let u = i as f32 / (POINTS - 1) as f32 * len;
                    let left = if i > 0 { e.x[i - 1] } else { e.x[i] };
                    let right = if i < POINTS - 1 { e.x[i + 1] } else { e.x[i] };
                    let lap = left + right - 2.0 * e.x[i];
                    let f = if let Some(d) = drag {
                        let q = (u - d.u) / sigma;
                        1300.0 * (-q * q).exp() * (d.v - e.x[i])
                    } else if let Some((hu, hv)) = hover {
                        let q = (u - hu) / (sigma * 1.15);
                        let g = (-q * q).exp();
                        // Near the pointer the edge rests at the window's edge, as if it were there.
                        900.0 * g * (hv.min(shown + 30.0 + 20.0 * m.amplitude) - e.x[i]) + k * g * (shown - base)
                    } else {
                        0.0
                    };
                    let a = k * (base - e.x[i]) + tension * lap + f - c * e.v[i];
                    peak_a = peak_a.max(a.abs());
                    e.v[i] += a * h;
                }
                for i in 0..POINTS {
                    e.x[i] += e.v[i] * h;
                    if e.x[i] < -40.0 {
                        e.x[i] = -40.0;
                        e.v[i] = e.v[i].max(0.0);
                    }
                    if e.x[i] > deep {
                        e.x[i] = deep;
                        e.v[i] = e.v[i].min(0.0);
                    }
                }
            }
            if !e.x.iter().all(|x| x.is_finite()) {
                e.x = [e.target; POINTS];
                e.v = [0.0; POINTS];
            }
            let speed = e.v.iter().fold(0.0f32, |a, v| a.max(v.abs()));
            if drag.is_some() || speed > 0.5 || peak_a > 5.0 {
                moving = true;
            } else {
                e.v = [0.0; POINTS];
            }
        }
        moving
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (f32, f32) = (960.0, 540.0);

    fn settle(c: &mut Curtains, m: &Motion, hover: Option<(f32, f32)>) -> usize {
        let mut frames = 0;
        while c.step(1.0 / 60.0, m, hover, SIZE) {
            frames += 1;
            assert!(frames < 2000, "never settles");
        }
        frames
    }

    #[test]
    fn closed_curtains_stay_still() {
        let mut c = Curtains::default();
        assert!(!c.step(1.0 / 60.0, &Motion::default(), None, SIZE));
        assert!(c.edges.iter().all(|e| !e.shows()));
    }

    #[test]
    fn a_pull_past_half_opens_and_the_curtain_settles_open() {
        let m = Motion::default();
        let mut c = Curtains::default();
        assert_eq!(c.pick(5.0, 270.0, SIZE), Some(Side::Left));
        c.press(Side::Left, 5.0, 270.0, SIZE);
        for i in 1..=30 {
            c.drag_to(5.0 + i as f32 * 8.0, 270.0, 1.0 / 60.0, SIZE);
            c.step(1.0 / 60.0, &m, None, SIZE);
        }
        assert!(c.edge(Side::Left).x[POINTS / 2] > 150.0, "the edge follows the finger");
        c.release(&m, SIZE);
        assert!(c.edge(Side::Left).open);
        settle(&mut c, &m, None);
        let e = c.edge(Side::Left);
        assert!(e.x.iter().all(|x| (x - DEPTH_SIDE).abs() < 1.0), "rests open: {:?}", e.x);
    }

    #[test]
    fn a_press_inside_an_open_curtain_takes_that_curtain() {
        let m = Motion::default();
        let mut c = Curtains::default();
        c.edges[Side::Left.index()].open = true;
        c.edges[Side::Left.index()].x = [DEPTH_SIDE; POINTS];
        c.edges[Side::Left.index()].target = DEPTH_SIDE;
        // The bottom edge bulges towards the pointer near it.
        settle(&mut c, &m, Some((150.0, 500.0)));
        assert_eq!(c.pick(150.0, 500.0, SIZE), Some(Side::Left));
    }

    #[test]
    fn a_short_pull_falls_back_shut() {
        let m = Motion::default();
        let mut c = Curtains::default();
        c.press(Side::Bottom, 480.0, 535.0, SIZE);
        c.drag_to(480.0, 480.0, 0.2, SIZE);
        c.step(1.0 / 60.0, &m, None, SIZE);
        c.release(&m, SIZE);
        assert!(!c.edge(Side::Bottom).open);
        settle(&mut c, &m, None);
        assert!(!c.edge(Side::Bottom).shows());
    }

    #[test]
    fn the_edge_bulges_under_the_pointer_and_settles() {
        let m = Motion::default();
        let mut c = Curtains::default();
        settle(&mut c, &m, Some((480.0, 50.0)));
        let top = c.edge(Side::Top);
        let mid = top.at(480.0, SIZE.0);
        assert!(mid > 20.0 && mid < 56.0, "bulge {mid}");
        assert!(top.at(50.0, SIZE.0) < 0.0, "only near the pointer");
        assert!(!c.edge(Side::Left).shows());
    }

    #[test]
    fn stiff_wide_waves_stay_finite() {
        let m = Motion { stiffness: 2.0, viscosity: 0.3, wave: 2.0, amplitude: 2.0 };
        let mut c = Curtains::default();
        c.press(Side::Right, 955.0, 100.0, SIZE);
        for i in 1..=20 {
            c.drag_to(955.0 - i as f32 * 30.0, 100.0 + i as f32 * 10.0, 1.0 / 240.0, SIZE);
            c.step(1.0 / 30.0, &m, None, SIZE);
        }
        c.release(&m, SIZE);
        for _ in 0..200 {
            c.step(1.0 / 30.0, &m, None, SIZE);
        }
        assert!(c.edges.iter().all(|e| e.x.iter().all(|x| x.is_finite())));
    }
}
