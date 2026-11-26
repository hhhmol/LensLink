//! What the curtains hold: the settings, their values and where each one sits on its panel, and
//! the same items as floating widgets on the video.
//!
//! Left: the camera. Right: information about the phone and the stream (the widgets). Top: the video
//! and the look of the interface. Bottom: the curtains' own motion and glass. Every panel is laid
//! out at rest, in DIPs from its own corner; the glass shader bends it with the curtain's edge.

use lenslink_vcam::modes::Mode;
use serde_json::{json, Value};

use super::curtain::{Motion, Side, DEPTH_END, DEPTH_SIDE};
use crate::settings::Settings;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Zoom,
    Exposure,
    Focus,
    Iso,
    WhiteBalance,
    Torch,
    Resolution,
    Rate,
    Quality,
    Phone,
    Mode,
    Time,
    Fps,
    Bitrate,
    Keyframes,
    Gap,
    Temperature,
    Battery,
    Lens,
    Dispersion,
    Theme,
    Stiffness,
    Viscosity,
    Wave,
    Amplitude,
    Tint,
    Frost,
}

const KEYS: usize = 27;

pub enum Kind {
    Range { min: f32, max: f32, step: f32, format: fn(f32) -> String },
    Choice(&'static [&'static str]),
    /// A choice among what the phone can stream: its value is a picture height or a frame rate.
    Modes,
    Info,
}

pub struct Row {
    pub key: Key,
    /// Name in the saved settings and for saved widgets.
    pub name: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    pub default: f32,
}

fn two(v: f32) -> String {
    format!("{v:.2}")
}

#[allow(clippy::too_many_arguments)] // one row of the table
const fn range(key: Key, name: &'static str, label: &'static str, min: f32, max: f32, step: f32, default: f32, format: fn(f32) -> String) -> Row {
    Row { key, name, label, kind: Kind::Range { min, max, step, format }, default }
}

const fn choice(key: Key, name: &'static str, label: &'static str, options: &'static [&'static str], default: f32) -> Row {
    Row { key, name, label, kind: Kind::Choice(options), default }
}

const fn modes(key: Key, name: &'static str, label: &'static str, default: f32) -> Row {
    Row { key, name, label, kind: Kind::Modes, default }
}

const fn info(key: Key, name: &'static str, label: &'static str) -> Row {
    Row { key, name, label, kind: Kind::Info, default: 0.0 }
}

/// In the order of [`Key`].
pub const ROWS: [Row; KEYS] = [
    range(Key::Zoom, "zoom", "Зум", 1.0, 8.0, 0.1, 1.0, |v| format!("{v:.1}×")),
    range(Key::Exposure, "exposure", "Экспозиция", -2.0, 2.0, 0.1, 0.0, |v| format!("{}{v:.1}", if v > 0.0 { "+" } else { "" })),
    choice(Key::Focus, "focus", "Фокус", &["авто", "ручной"], 0.0),
    range(Key::Iso, "iso", "ISO", 100.0, 3200.0, 100.0, 400.0, |v| format!("{v:.0}")),
    range(Key::WhiteBalance, "white_balance", "Баланс белого", 2500.0, 7500.0, 100.0, 5200.0, |v| format!("{v:.0} K")),
    choice(Key::Torch, "torch", "Фонарик", &["выкл", "вкл"], 0.0),
    modes(Key::Resolution, "resolution", "Разрешение", 1080.0),
    modes(Key::Rate, "rate", "Частота", 30.0),
    range(Key::Quality, "quality", "Качество", 2.0, 100.0, 1.0, 20.0, |v| format!("{v:.0} Мбит/с")),
    info(Key::Phone, "phone", "Телефон"),
    info(Key::Mode, "mode", "Режим"),
    info(Key::Time, "time", "Время"),
    info(Key::Fps, "fps", "Кадры/с"),
    info(Key::Bitrate, "bitrate", "Поток"),
    info(Key::Keyframes, "keyframes", "Ключевые кадры"),
    info(Key::Gap, "gap", "Пауза кадров"),
    info(Key::Temperature, "temperature", "Нагрев"),
    info(Key::Battery, "battery", "Заряд"),
    range(Key::Lens, "lens", "Сила линзы", 0.0, 2.0, 0.05, 1.3, two),
    range(Key::Dispersion, "dispersion", "Дисперсия", 0.0, 2.0, 0.05, 1.2, two),
    choice(Key::Theme, "theme", "Тема", &["графит", "иней"], 0.0),
    range(Key::Stiffness, "stiffness", "Упругость", 0.3, 2.0, 0.05, 1.0, two),
    range(Key::Viscosity, "viscosity", "Вязкость", 0.3, 2.0, 0.05, 1.0, two),
    range(Key::Wave, "wave", "Ширина волны", 0.3, 2.0, 0.05, 0.7, two),
    range(Key::Amplitude, "amplitude", "Амплитуда", 0.0, 2.0, 0.05, 1.3, two),
    range(Key::Tint, "tint", "Затемнение", 0.0, 1.0, 0.05, 0.55, two),
    range(Key::Frost, "frost", "Матовость", 0.0, 1.0, 0.05, 0.35, two),
];

pub fn row(key: Key) -> &'static Row {
    &ROWS[key as usize]
}

pub fn key_by_name(name: &str) -> Option<Key> {
    ROWS.iter().find(|r| r.name == name).map(|r| r.key)
}

/// Settings the camera picks by itself until the user moves them; their slider's left end (or a
/// double click) gives them back to the camera.
pub const AUTOMATIC: [Key; 3] = [Key::Exposure, Key::Iso, Key::WhiteBalance];
/// The part of an automatic setting's slider, at its left end, that means "automatic".
pub const AUTO_NOTCH: f32 = 0.1;

/// Before the phone tells its modes.
const USUAL_MODES: [Mode; 3] =
    [Mode { width: 1280, height: 720, fps: 30 }, Mode { width: 1920, height: 1080, fps: 30 }, Mode { width: 3840, height: 2160, fps: 30 }];

fn height_name(h: u32) -> String {
    if h == 2160 {
        "4K".into()
    } else {
        format!("{h}p")
    }
}

/// Settings that go to the phone at once; the video settings restart the stream.
pub const CAMERA: [Key; 6] = [Key::Zoom, Key::Exposure, Key::Focus, Key::Iso, Key::WhiteBalance, Key::Torch];
pub const VIDEO: [Key; 3] = [Key::Resolution, Key::Rate, Key::Quality];

type Column = (&'static str, &'static [Key]);

/// Columns of each panel, by side.
const LAYOUT: [&[Column]; 4] = [
    &[("Камера", &[Key::Zoom, Key::Exposure, Key::Focus, Key::Iso, Key::WhiteBalance, Key::Torch])],
    &[(
        "Виджеты",
        &[Key::Phone, Key::Mode, Key::Time, Key::Fps, Key::Bitrate, Key::Keyframes, Key::Gap, Key::Temperature, Key::Battery],
    )],
    &[("Видео", &[Key::Resolution, Key::Rate, Key::Quality]), ("Интерфейс", &[Key::Lens, Key::Dispersion, Key::Theme])],
    &[("Шторка · движение", &[Key::Stiffness, Key::Viscosity, Key::Wave, Key::Amplitude]), ("Шторка · стекло", &[Key::Tint, Key::Frost])],
];

/// The settings' current values: a number for a range, an option's index for a choice; text for
/// information.
#[derive(Clone)]
pub struct Values {
    v: [f32; KEYS],
    info: [String; KEYS],
    /// Left to the camera (see [`AUTOMATIC`]).
    auto: [bool; KEYS],
    /// The phone's own range for a setting, when it differs from the table's.
    limits: [Option<(f32, f32)>; KEYS],
    /// What the phone can stream.
    modes: Vec<Mode>,
}

impl Default for Values {
    fn default() -> Self {
        Values {
            v: std::array::from_fn(|i| ROWS[i].default),
            info: Default::default(),
            auto: std::array::from_fn(|i| AUTOMATIC.contains(&ROWS[i].key)),
            limits: [None; KEYS],
            modes: Vec::new(),
        }
    }
}

impl Values {
    pub fn get(&self, key: Key) -> f32 {
        self.v[key as usize]
    }

    /// A range's bounds and step: the phone's, if it told them.
    pub fn range(&self, key: Key) -> (f32, f32, f32) {
        match row(key).kind {
            Kind::Range { min, max, step, .. } => match self.limits[key as usize] {
                Some((lo, hi)) if hi > lo => (lo, hi, step),
                _ => (min, max, step),
            },
            _ => (0.0, 0.0, 1.0),
        }
    }

    /// The phone's range for `key`; the value moves into it.
    pub fn set_limits(&mut self, key: Key, limits: Option<(f32, f32)>) -> bool {
        if self.limits[key as usize] == limits {
            return false;
        }
        self.limits[key as usize] = limits;
        let v = self.get(key);
        self.set(key, v);
        true
    }

    /// Sets a value, kept in range and on the step, or to the nearest option. Returns whether it
    /// changed.
    pub fn set(&mut self, key: Key, value: f32) -> bool {
        let v = match row(key).kind {
            Kind::Range { .. } => {
                let (min, max, step) = self.range(key);
                ((value.clamp(min, max) - min) / step).round() * step + min
            }
            Kind::Choice(options) => value.round().clamp(0.0, options.len() as f32 - 1.0),
            Kind::Modes => {
                let options = self.options(key);
                let nearest = options.iter().map(|o| o.1).min_by(|a, b| (a - value).abs().total_cmp(&(b - value).abs()));
                nearest.unwrap_or(value)
            }
            Kind::Info => return false,
        };
        let v = (v * 1e4).round() / 1e4;
        let old = std::mem::replace(&mut self.v[key as usize], v);
        // Another size may stream at other rates.
        if key == Key::Resolution {
            let rate = self.get(Key::Rate);
            return self.set(Key::Rate, rate) | (old != v);
        }
        old != v
    }

    /// The modes the phone can stream (or the usual ones before it tells).
    fn modes(&self) -> &[Mode] {
        if self.modes.is_empty() {
            &USUAL_MODES
        } else {
            &self.modes
        }
    }

    /// What the phone can stream; the choices move to what it offers.
    pub fn set_modes(&mut self, modes: &[Mode]) -> bool {
        if self.modes == modes {
            return false;
        }
        self.modes = modes.to_vec();
        let (h, fps) = (self.get(Key::Resolution), self.get(Key::Rate));
        self.set(Key::Resolution, h);
        self.set(Key::Rate, fps);
        true
    }

    /// A choice's options: name and value.
    pub fn options(&self, key: Key) -> Vec<(String, f32)> {
        match row(key).kind {
            Kind::Choice(names) => names.iter().enumerate().map(|(i, n)| (n.to_string(), i as f32)).collect(),
            Kind::Modes if key == Key::Resolution => {
                // Webcam sizes: 720p and up when the phone has them.
                let mut heights: Vec<u32> = self.modes().iter().map(|m| m.height).collect();
                heights.sort_unstable();
                heights.dedup();
                if heights.iter().any(|&h| h >= 720) {
                    heights.retain(|&h| h >= 720);
                }
                heights.into_iter().map(|h| (height_name(h), h as f32)).collect()
            }
            Kind::Modes => {
                let h = self.get(Key::Resolution) as u32;
                let mut rates: Vec<u32> = self.modes().iter().filter(|m| m.height == h).map(|m| m.fps).collect();
                rates.sort_unstable();
                rates.dedup();
                rates.into_iter().map(|f| (f.to_string(), f as f32)).collect()
            }
            _ => Vec::new(),
        }
    }

    /// The option chosen now.
    pub fn chosen(&self, key: Key) -> Option<usize> {
        self.options(key).iter().position(|o| o.1 == self.get(key))
    }

    pub fn auto(&self, key: Key) -> bool {
        self.auto[key as usize]
    }

    pub fn set_auto(&mut self, key: Key, auto: bool) -> bool {
        let auto = auto && AUTOMATIC.contains(&key);
        std::mem::replace(&mut self.auto[key as usize], auto) != auto
    }

    pub fn info(&self, key: Key) -> &str {
        &self.info[key as usize]
    }

    pub fn set_info(&mut self, key: Key, text: &str) -> bool {
        let slot = &mut self.info[key as usize];
        if slot == text {
            return false;
        }
        *slot = text.to_string();
        true
    }

    /// What a row shows: a range's number, a choice's option, information's text.
    pub fn text(&self, key: Key) -> String {
        match row(key).kind {
            _ if self.auto(key) => "авто".into(),
            Kind::Range { format, .. } => format(self.get(key)),
            Kind::Choice(options) => options[self.get(key) as usize].to_string(),
            Kind::Modes => self.chosen(key).map(|i| self.options(key)[i].0.clone()).unwrap_or_default(),
            Kind::Info => self.info(key).to_string(),
        }
    }

    pub fn frost_theme(&self) -> bool {
        self.get(Key::Theme) >= 0.5
    }

    pub fn motion(&self) -> Motion {
        Motion {
            stiffness: self.get(Key::Stiffness),
            viscosity: self.get(Key::Viscosity),
            wave: self.get(Key::Wave),
            amplitude: self.get(Key::Amplitude),
        }
    }

    /// The camera settings as a `set` message for the phone (every key: a reconnected phone starts
    /// from automatic).
    pub fn phone_controls(&self) -> Value {
        // Values sit on their steps; written as short decimals.
        let num = |k: Key| json!((self.get(k) as f64 * 1000.0).round() / 1000.0);
        let manual = |k: Key| if self.auto(k) { Value::Null } else { num(k) };
        json!({
            "type": "set",
            "zoom": num(Key::Zoom),
            "ev": if self.auto(Key::Exposure) { json!(0.0) } else { num(Key::Exposure) },
            "focus_diopters": Value::Null,
            "focus_hold": self.get(Key::Focus) >= 0.5,
            "iso": manual(Key::Iso),
            "awb_kelvin": manual(Key::WhiteBalance),
            "torch": self.get(Key::Torch) >= 0.5,
        })
    }

    /// The video asked for: width, height, frames per second and bits per second.
    pub fn video(&self) -> (u32, u32, u32, u32) {
        let (h, fps) = (self.get(Key::Resolution) as u32, self.get(Key::Rate) as u32);
        let w = self.modes().iter().find(|m| m.height == h).map_or(h * 16 / 9, |m| m.width);
        (w, h, fps, (self.get(Key::Quality) * 1e6) as u32)
    }

    pub fn load(s: &Settings) -> Self {
        let mut v = Values::default();
        for r in ROWS.iter().filter(|r| !matches!(r.kind, Kind::Info)) {
            if let Some(&x) = s.values.get(r.name) {
                // Earlier versions kept the option's position.
                let x = match r.key {
                    Key::Resolution if x < 100.0 => [720.0, 1080.0, 2160.0].get(x as usize).copied().unwrap_or(1080.0),
                    Key::Rate if x < 10.0 => if x >= 0.5 { 60.0 } else { 30.0 },
                    _ => x,
                };
                v.set(r.key, x as f32);
            }
            if let Some(&a) = s.values.get(&format!("{}_auto", r.name)) {
                v.set_auto(r.key, a != 0.0);
            }
        }
        v
    }

    pub fn store(&self, s: &mut Settings) {
        for r in ROWS.iter().filter(|r| !matches!(r.kind, Kind::Info)) {
            s.values.insert(r.name.to_string(), (self.get(r.key) as f64 * 1e4).round() / 1e4);
            if AUTOMATIC.contains(&r.key) {
                s.values.insert(format!("{}_auto", r.name), if self.auto(r.key) { 1.0 } else { 0.0 });
            }
        }
    }
}

/// A rectangle in DIPs: left, top, width, height.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn centre(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// How far (x, y) is outside, horizontally and vertically (0 inside).
    pub fn distance(&self, x: f32, y: f32) -> (f32, f32) {
        ((self.x - x).max(x - self.right()).max(0.0), (self.y - y).max(y - self.bottom()).max(0.0))
    }
}

/// Fonts of a panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    /// Section title: small capitals.
    Section,
    Text,
    /// Information values: semibold.
    Strong,
}

pub enum Item {
    Section { text: String, rect: Rect },
    Label { key: Key, rect: Rect },
    Slider { key: Key, rect: Rect },
    /// A range's number, right-aligned.
    Output { key: Key, rect: Rect },
    /// The option buttons of a choice and their common background.
    Choice { key: Key, rect: Rect, options: Vec<Rect> },
    /// Information, right-aligned.
    Info { key: Key, rect: Rect },
}

/// A control the pointer can work.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Control {
    Slider { key: Key, rect: Rect },
    Option { key: Key, index: usize, rect: Rect },
}

impl Control {
    pub fn rect(&self) -> Rect {
        match self {
            Control::Slider { rect, .. } | Control::Option { rect, .. } => *rect,
        }
    }
}

const PAD: f32 = 16.0;
const COLUMN_GAP: f32 = 28.0;
const COLUMN_MAX: f32 = 320.0;
const SECTION: f32 = 14.0;
const SECTION_GAP: f32 = 4.0;
const ROW: f32 = 32.0;
const LABEL: f32 = 100.0;
const GAP: f32 = 8.0;
const OUTPUT_MIN: f32 = 50.0;
const SLIDER: f32 = 16.0;
/// A choice: option padding and the gap and padding around options.
const OPTION_PAD: (f32, f32) = (8.0, 3.0);
const OPTION_GAP: f32 = 2.0;
/// Invisible margin around a control that still hits it: a near miss works.
pub const HALO: (f32, f32) = (8.0, 6.0);
/// A floating widget: its height, side padding, gap between its parts and slider width.
pub const WIDGET_HEIGHT: f32 = 40.0;
const WIDGET_PAD: f32 = 16.0;
const WIDGET_GAP: f32 = 10.0;
const WIDGET_SLIDER: f32 = 110.0;
/// Room for the green light before the phone's name.
const DOT: f32 = 14.0;

/// A panel's size at the window's size.
pub fn panel_size(side: Side, window: (f32, f32)) -> (f32, f32) {
    match side {
        Side::Left | Side::Right => (DEPTH_SIDE, window.1),
        Side::Top | Side::Bottom => (window.0, DEPTH_END),
    }
}

/// A choice's options in a row from `x0`, centred on `cy`.
fn choice_item(key: Key, options: &[String], x0: f32, cy: f32, measure: &dyn Fn(&str, Font) -> (f32, f32)) -> Item {
    let mut ox = x0 + OPTION_GAP;
    let mut rects = Vec::new();
    let mut oh: f32 = 0.0;
    for o in options {
        let (tw, th) = measure(o, Font::Text);
        let r = Rect { x: ox, y: 0.0, w: (tw + 2.0 * OPTION_PAD.0).ceil(), h: th + 2.0 * OPTION_PAD.1 };
        oh = oh.max(r.h);
        ox = r.right() + OPTION_GAP;
        rects.push(r);
    }
    for r in &mut rects {
        r.h = oh;
        r.y = cy - oh / 2.0;
    }
    let rect = Rect { x: x0, y: cy - oh / 2.0 - OPTION_GAP, w: ox - x0, h: oh + 2.0 * OPTION_GAP };
    Item::Choice { key, rect, options: rects }
}

/// Lays out `side`'s panel. `measure` gives a text's width and line height in a font.
pub fn layout(side: Side, window: (f32, f32), values: &Values, measure: &dyn Fn(&str, Font) -> (f32, f32)) -> Vec<Item> {
    let (pw, _) = panel_size(side, window);
    let columns = LAYOUT[side.index()];
    let n = columns.len() as f32;
    let width = ((pw - 2.0 * PAD - COLUMN_GAP * (n - 1.0)) / n).min(COLUMN_MAX);
    let mut out = Vec::new();
    for (c, (title, keys)) in columns.iter().enumerate() {
        let x = PAD + c as f32 * (width + COLUMN_GAP);
        let mut y = PAD;
        out.push(Item::Section { text: title.to_uppercase(), rect: Rect { x, y, w: width, h: SECTION } });
        y += SECTION + SECTION_GAP;
        for &key in *keys {
            let row_rect = Rect { x, y, w: width, h: ROW };
            let cy = y + ROW / 2.0;
            out.push(Item::Label { key, rect: Rect { w: LABEL, ..row_rect } });
            let x0 = x + LABEL + GAP;
            match row(key).kind {
                Kind::Range { min, max, format, .. } => {
                    let widest = measure(&format(min), Font::Text).0.max(measure(&format(max), Font::Text).0);
                    let ow = widest.max(OUTPUT_MIN).ceil();
                    let x1 = row_rect.right() - ow - GAP;
                    out.push(Item::Slider { key, rect: Rect { x: x0, y: cy - SLIDER / 2.0, w: (x1 - x0).max(20.0), h: SLIDER } });
                    out.push(Item::Output { key, rect: Rect { x: row_rect.right() - ow, y, w: ow, h: ROW } });
                }
                Kind::Choice(_) | Kind::Modes => {
                    let names: Vec<String> = values.options(key).into_iter().map(|o| o.0).collect();
                    out.push(choice_item(key, &names, x0, cy, measure));
                }
                Kind::Info => out.push(Item::Info { key, rect: Rect { x: x0, y, w: row_rect.right() - x0, h: ROW } }),
            }
            y += ROW;
        }
    }
    out
}

/// Lays out `key` as a floating widget: its name, then its control or value, in one glass pill.
/// Positions are from the widget's top left; returns the items and the widget's width.
pub fn widget(key: Key, values: &Values, measure: &dyn Fn(&str, Font) -> (f32, f32)) -> (Vec<Item>, f32) {
    let mut out = Vec::new();
    let cy = WIDGET_HEIGHT / 2.0;
    let mut x = WIDGET_PAD;
    // The phone's widget is just its name.
    if key != Key::Phone {
        let w = measure(row(key).label, Font::Text).0.ceil();
        out.push(Item::Label { key, rect: Rect { x, y: 0.0, w, h: WIDGET_HEIGHT } });
        x += w + WIDGET_GAP;
    }
    match row(key).kind {
        Kind::Range { .. } => {
            out.push(Item::Slider { key, rect: Rect { x, y: cy - SLIDER / 2.0, w: WIDGET_SLIDER, h: SLIDER } });
            x += WIDGET_SLIDER + WIDGET_GAP;
            let ow = measure(&values.text(key), Font::Text).0.max(OUTPUT_MIN).ceil();
            out.push(Item::Output { key, rect: Rect { x, y: 0.0, w: ow, h: WIDGET_HEIGHT } });
            x += ow;
        }
        Kind::Choice(_) | Kind::Modes => {
            let names: Vec<String> = values.options(key).into_iter().map(|o| o.0).collect();
            let item = choice_item(key, &names, x, cy, measure);
            if let Item::Choice { rect, .. } = &item {
                x = rect.right();
            }
            out.push(item);
        }
        Kind::Info => {
            let text = values.info(key);
            let mut w = measure(if text.is_empty() { "—" } else { text }, Font::Strong).0.ceil();
            if key == Key::Phone && !text.is_empty() {
                w += DOT;
            }
            out.push(Item::Info { key, rect: Rect { x, y: 0.0, w, h: WIDGET_HEIGHT } });
            x += w;
        }
    }
    (out, x + WIDGET_PAD)
}

/// The controls among `items`.
pub fn controls(items: &[Item]) -> Vec<Control> {
    let mut out = Vec::new();
    for item in items {
        match item {
            Item::Slider { key, rect } => out.push(Control::Slider { key: *key, rect: *rect }),
            Item::Choice { key, options, .. } => {
                out.extend(options.iter().enumerate().map(|(index, r)| Control::Option { key: *key, index, rect: *r }));
            }
            _ => {}
        }
    }
    out
}

/// The control at (x, y) in the items' coordinates, or the nearest one whose halo it is in, with
/// how far off it is.
pub fn control_at(items: &[Item], x: f32, y: f32) -> Option<(Control, f32)> {
    let mut best = None;
    for c in controls(items) {
        let (dx, dy) = c.rect().distance(x, y);
        if dx > HALO.0 || dy > HALO.1 {
            continue;
        }
        if best.is_none_or(|(_, d)| dx + dy < d) {
            best = Some((c, dx + dy));
        }
    }
    best
}

/// The label at (x, y): where an item is grabbed to pull it onto the video.
pub fn label_at(items: &[Item], x: f32, y: f32) -> Option<Key> {
    items.iter().find_map(|i| match i {
        Item::Label { key, rect } if rect.distance(x, y) == (0.0, 0.0) => Some(*key),
        _ => None,
    })
}

/// The pointer at `x` moves a slider's thumb (its centre follows the pointer). On an automatic
/// setting the left end gives the setting back to the camera. Returns whether anything changed.
pub fn slide(values: &mut Values, key: Key, rect: Rect, x: f32) -> bool {
    let (min, max, _) = values.range(key);
    let f = ((x - rect.x - 7.0) / (rect.w - 14.0).max(1.0)).clamp(0.0, 1.0);
    if !AUTOMATIC.contains(&key) {
        return values.set(key, min + f * (max - min));
    }
    if f < AUTO_NOTCH / 2.0 {
        return values.set_auto(key, true);
    }
    let g = ((f - AUTO_NOTCH) / (1.0 - AUTO_NOTCH)).clamp(0.0, 1.0);
    let manual = values.set_auto(key, false);
    values.set(key, min + g * (max - min)) | manual
}

/// Where a slider's thumb is: 0 at the left, 1 at the right.
pub fn slider_fraction(key: Key, values: &Values) -> f32 {
    let (min, max, _) = values.range(key);
    let f = ((values.get(key) - min) / (max - min).max(1e-6)).clamp(0.0, 1.0);
    match AUTOMATIC.contains(&key) {
        true if values.auto(key) => 0.0,
        true => AUTO_NOTCH + f * (1.0 - AUTO_NOTCH),
        false => f,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measure(t: &str, _: Font) -> (f32, f32) {
        (t.chars().count() as f32 * 7.0, 16.5)
    }

    #[test]
    fn values_keep_to_range_and_step() {
        let mut v = Values::default();
        assert!(v.set(Key::Iso, 1234.0));
        assert_eq!(v.get(Key::Iso), 1200.0);
        v.set(Key::Lens, 5.0);
        assert_eq!(v.get(Key::Lens), 2.0);
        v.set(Key::Exposure, 0.33);
        v.set_auto(Key::Exposure, false);
        assert_eq!(v.text(Key::Exposure), "+0.3");
        v.set(Key::Resolution, 5000.0);
        assert_eq!(v.text(Key::Resolution), "4K");
        assert!(!v.set(Key::Resolution, 2160.0));
    }

    #[test]
    fn the_phone_sets_the_range() {
        let mut v = Values::default();
        v.set(Key::Zoom, 8.0);
        assert!(v.set_limits(Key::Zoom, Some((1.0, 5.0))));
        assert_eq!(v.get(Key::Zoom), 5.0);
        v.set(Key::Zoom, 9.0);
        assert_eq!(v.get(Key::Zoom), 5.0);
    }

    #[test]
    fn iso_and_white_balance_start_automatic() {
        let mut v = Values::default();
        assert_eq!(v.text(Key::Iso), "авто");
        assert_eq!(v.phone_controls()["iso"], Value::Null);
        v.set(Key::Iso, 800.0);
        v.set_auto(Key::Iso, false);
        assert_eq!(v.text(Key::Iso), "800");
        assert_eq!(v.phone_controls()["iso"], 800.0);
        assert!(!v.set_auto(Key::Zoom, true), "zoom has no automatic");
    }

    #[test]
    fn values_survive_a_save() {
        let mut v = Values::default();
        v.set(Key::Wave, 1.5);
        v.set(Key::Theme, 1.0);
        v.set_auto(Key::WhiteBalance, false);
        let mut s = Settings::default();
        v.store(&mut s);
        let back = Values::load(&s);
        assert_eq!(back.get(Key::Wave), 1.5);
        assert!(back.frost_theme());
        assert!(!back.auto(Key::WhiteBalance));
        assert!(back.auto(Key::Iso));
        assert_eq!(back.get(Key::Lens), 1.3);
    }

    #[test]
    fn the_video_choice_is_what_the_phone_can_stream() {
        let mode = |width, height, fps| Mode { width, height, fps };
        let mut v = Values::default();
        assert_eq!(v.video(), (1920, 1080, 30, 20_000_000));
        v.set_modes(&[mode(1280, 720, 30), mode(1280, 720, 60), mode(1920, 1080, 30), mode(3840, 2160, 30), mode(640, 480, 30)]);
        let names = |v: &Values, k| v.options(k).into_iter().map(|o| o.0).collect::<Vec<_>>();
        assert_eq!(names(&v, Key::Resolution), ["720p", "1080p", "4K"]);
        assert_eq!(names(&v, Key::Rate), ["30"]);
        v.set(Key::Resolution, 720.0);
        assert_eq!(names(&v, Key::Rate), ["30", "60"]);
        v.set(Key::Rate, 60.0);
        v.set(Key::Quality, 8.0);
        assert_eq!(v.video(), (1280, 720, 60, 8_000_000));
        // 4K has no 60: the rate follows.
        v.set(Key::Resolution, 2160.0);
        assert_eq!(v.video(), (3840, 2160, 30, 8_000_000));
    }

    #[test]
    fn saved_positions_of_earlier_versions() {
        let mut s = Settings::default();
        s.values.insert("resolution".into(), 2.0);
        s.values.insert("rate".into(), 0.0);
        let v = Values::load(&s);
        assert_eq!((v.get(Key::Resolution), v.get(Key::Rate)), (2160.0, 30.0));
    }

    #[test]
    fn the_left_end_is_automatic() {
        let mut v = Values::default();
        let rect = Rect { x: 0.0, y: 0.0, w: 114.0, h: 16.0 };
        assert!(slide(&mut v, Key::Iso, rect, 60.0));
        assert!(!v.auto(Key::Iso));
        assert!(slider_fraction(Key::Iso, &v) >= AUTO_NOTCH);
        assert!(slide(&mut v, Key::Iso, rect, 0.0));
        assert!(v.auto(Key::Iso));
        assert_eq!(v.text(Key::Iso), "авто");
        // Just past the notch: the lowest manual value.
        slide(&mut v, Key::Iso, rect, 7.0 + 100.0 * AUTO_NOTCH);
        assert_eq!((v.auto(Key::Iso), v.get(Key::Iso)), (false, 100.0));
        // Exposure is automatic too, and sends no compensation then.
        assert_eq!(v.phone_controls()["ev"], 0.0);
        assert_eq!(v.text(Key::Exposure), "авто");
    }

    #[test]
    fn the_camera_panel_lists_its_rows_in_one_column() {
        let items = layout(Side::Left, (960.0, 540.0), &Values::default(), &measure);
        let labels: Vec<f32> = items.iter().filter_map(|i| if let Item::Label { rect, .. } = i { Some(rect.y) } else { None }).collect();
        assert_eq!(labels, vec![34.0, 66.0, 98.0, 130.0, 162.0, 194.0]);
        // Four sliders and the options of two choices.
        assert_eq!(controls(&items).len(), 4 + 2 + 2);
        assert_eq!(label_at(&items, 40.0, 80.0), Some(Key::Exposure));
        assert_eq!(label_at(&items, 200.0, 80.0), None);
    }

    #[test]
    fn a_near_miss_still_hits_and_a_far_one_does_not() {
        let v = Values::default();
        let items = layout(Side::Bottom, (960.0, 540.0), &v, &measure);
        let Some(Control::Slider { rect, .. }) = controls(&items).into_iter().find(|c| matches!(c, Control::Slider { key: Key::Wave, .. })) else {
            panic!("no wave slider")
        };
        let (cx, _) = rect.centre();
        assert!(matches!(control_at(&items, cx, rect.y - 5.0), Some((Control::Slider { key: Key::Wave, .. }, _))));
        assert_eq!(control_at(&items, cx, rect.y - 7.5), None);
        // The thumb follows the pointer, and the ends are reachable.
        let mut v = v;
        slide(&mut v, Key::Wave, rect, rect.x);
        assert_eq!(v.get(Key::Wave), 0.3);
        slide(&mut v, Key::Wave, rect, rect.right());
        assert_eq!(v.get(Key::Wave), 2.0);
    }

    #[test]
    fn two_columns_side_by_side_on_the_top_panel() {
        let items = layout(Side::Top, (960.0, 540.0), &Values::default(), &measure);
        let xs: Vec<f32> = items.iter().filter_map(|i| if let Item::Section { rect, .. } = i { Some(rect.x) } else { None }).collect();
        assert_eq!(xs, vec![16.0, 16.0 + 320.0 + 28.0]);
    }

    #[test]
    fn a_widget_is_its_name_and_its_control() {
        let v = Values::default();
        let (items, width) = widget(Key::Zoom, &v, &measure);
        // Name (3 letters), slider, number ("1.0×" is narrower than the 50 minimum).
        assert_eq!(width, 16.0 + 21.0 + 10.0 + 110.0 + 10.0 + 50.0 + 16.0);
        assert!(matches!(items[1], Item::Slider { .. }));
        let (items, _) = widget(Key::Phone, &v, &measure);
        assert!(items.iter().all(|i| !matches!(i, Item::Label { .. })), "the phone's widget has no name");
    }
}
