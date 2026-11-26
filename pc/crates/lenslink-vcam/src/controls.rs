//! Camera controls offered to Windows as standard kernel-streaming properties.
//!
//! Apps and the Windows camera settings page change camera settings through the `VideoProcAmp` and
//! `CameraControl` property sets. The host describes which controls the phone supports (ranges,
//! defaults) in [`CONTROLS_PATH`]; the camera source answers property requests from that table and
//! passes new values to the host through shared memory, and the host sends them to the phone.
//!
//! Values use the units Windows defines for each property:
//! zoom ×10 (10 = 1×), focus 0–100 (0 = infinity), exposure log2(seconds) (−5 ≈ 1/32 s),
//! brightness = exposure compensation in tenths of EV, gain = ISO, white balance in kelvin,
//! power-line frequency 0 off / 1 = 50 Hz / 2 = 60 Hz / 3 = auto.

use windows_core::GUID;

pub const CONTROLS_PATH: &str = r"C:\ProgramData\LensLink\config\controls.txt";
/// Slots in shared memory; more than there are controls, for later additions.
pub const MAX_CONTROLS: usize = 16;

pub const FLAG_AUTO: u32 = 1;
pub const FLAG_MANUAL: u32 = 2;

pub const PROPSETID_VIDCAP_VIDEOPROCAMP: GUID = GUID::from_u128(0xc6e13360_30ac_11d0_a18c_00a0c9118956);
pub const PROPSETID_VIDCAP_CAMERACONTROL: GUID = GUID::from_u128(0xc6e13370_30ac_11d0_a18c_00a0c9118956);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Zoom,
    Focus,
    Exposure,
    Brightness,
    Gain,
    WhiteBalance,
    PowerlineFrequency,
}

impl Control {
    pub const ALL: [Control; 7] = [
        Control::Zoom,
        Control::Focus,
        Control::Exposure,
        Control::Brightness,
        Control::Gain,
        Control::WhiteBalance,
        Control::PowerlineFrequency,
    ];

    /// Shared-memory slot.
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            Control::Zoom => "zoom",
            Control::Focus => "focus",
            Control::Exposure => "exposure",
            Control::Brightness => "brightness",
            Control::Gain => "gain",
            Control::WhiteBalance => "white_balance",
            Control::PowerlineFrequency => "powerline_frequency",
        }
    }

    pub fn from_name(s: &str) -> Option<Control> {
        Control::ALL.into_iter().find(|c| c.name() == s)
    }

    /// Kernel-streaming property set and id (`KSPROPERTY_CAMERACONTROL_*` / `KSPROPERTY_VIDEOPROCAMP_*`).
    pub fn ks(self) -> (GUID, u32) {
        match self {
            Control::Zoom => (PROPSETID_VIDCAP_CAMERACONTROL, 3),
            Control::Exposure => (PROPSETID_VIDCAP_CAMERACONTROL, 4),
            Control::Focus => (PROPSETID_VIDCAP_CAMERACONTROL, 6),
            Control::Brightness => (PROPSETID_VIDCAP_VIDEOPROCAMP, 0),
            Control::WhiteBalance => (PROPSETID_VIDCAP_VIDEOPROCAMP, 7),
            Control::Gain => (PROPSETID_VIDCAP_VIDEOPROCAMP, 9),
            Control::PowerlineFrequency => (PROPSETID_VIDCAP_VIDEOPROCAMP, 13),
        }
    }
}

/// What the phone supports for one control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlDesc {
    pub control: Control,
    pub min: i32,
    pub max: i32,
    pub step: i32,
    pub default: i32,
    /// `FLAG_AUTO` and/or `FLAG_MANUAL`.
    pub caps: u32,
    /// Flag in effect by default.
    pub default_flags: u32,
}

impl ControlDesc {
    pub fn is_valid(&self) -> bool {
        self.min <= self.max
            && self.step > 0
            && (self.min..=self.max).contains(&self.default)
            && self.caps != 0
            && self.caps & !(FLAG_AUTO | FLAG_MANUAL) == 0
            && (self.default_flags == FLAG_AUTO || self.default_flags == FLAG_MANUAL)
            && self.caps & self.default_flags != 0
    }

    /// A value and flags an app asked for, made valid: clamped, snapped to the step.
    pub fn sanitize(&self, value: i32, flags: u32) -> (i32, u32) {
        let v = value.clamp(self.min, self.max);
        let v = self.min + (v - self.min) / self.step * self.step;
        let f = if flags & FLAG_AUTO != 0 && self.caps & FLAG_AUTO != 0 { FLAG_AUTO } else if self.caps & FLAG_MANUAL != 0 { FLAG_MANUAL } else { FLAG_AUTO };
        (v, f)
    }
}

/// One `name min max step default caps default_flags` per line.
pub fn format(descs: &[ControlDesc]) -> String {
    descs
        .iter()
        .map(|d| {
            format!(
                "{} {} {} {} {} {} {}\n",
                d.control.name(),
                d.min,
                d.max,
                d.step,
                d.default,
                d.caps,
                d.default_flags
            )
        })
        .collect()
}

pub fn parse(text: &str) -> Vec<ControlDesc> {
    let mut out: Vec<ControlDesc> = Vec::new();
    for line in text.lines().take(MAX_CONTROLS) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let parsed = (|| {
            let [name, min, max, step, default, caps, default_flags] = f[..] else { return None };
            Some(ControlDesc {
                control: Control::from_name(name)?,
                min: min.parse().ok()?,
                max: max.parse().ok()?,
                step: step.parse().ok()?,
                default: default.parse().ok()?,
                caps: caps.parse().ok()?,
                default_flags: default_flags.parse().ok()?,
            })
        })();
        if let Some(d) = parsed.filter(|d| d.is_valid() && !out.iter().any(|o| o.control == d.control)) {
            out.push(d);
        }
    }
    out
}

pub fn write(descs: &[ControlDesc]) -> std::io::Result<()> {
    std::fs::write(CONTROLS_PATH, format(descs))
}

/// The controls file, or none when it is missing.
pub fn read() -> Vec<ControlDesc> {
    std::fs::read_to_string(CONTROLS_PATH).map(|t| parse(&t)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zoom() -> ControlDesc {
        ControlDesc { control: Control::Zoom, min: 6, max: 100, step: 1, default: 10, caps: FLAG_MANUAL, default_flags: FLAG_MANUAL }
    }

    #[test]
    fn round_trip() {
        let wb = ControlDesc {
            control: Control::WhiteBalance,
            min: 2700,
            max: 7500,
            step: 100,
            default: 5500,
            caps: FLAG_AUTO | FLAG_MANUAL,
            default_flags: FLAG_AUTO,
        };
        assert_eq!(parse(&format(&[zoom(), wb])), [zoom(), wb]);
    }

    #[test]
    fn rejects_garbage() {
        let text = "zoom 6 100 1 10 2 2\nzoom 1 2 1 1 2 2\nfocus 0 100 0 0 3 1\nbogus 1 2 1 1 2 2\ngain 100 50 1 100 3 1\n";
        assert_eq!(parse(text), [zoom()]);
    }

    #[test]
    fn sanitize() {
        let wb = ControlDesc {
            control: Control::WhiteBalance,
            min: 2700,
            max: 7500,
            step: 100,
            default: 5500,
            caps: FLAG_AUTO | FLAG_MANUAL,
            default_flags: FLAG_AUTO,
        };
        assert_eq!(wb.sanitize(4321, FLAG_MANUAL), (4300, FLAG_MANUAL));
        assert_eq!(wb.sanitize(99999, FLAG_AUTO), (7500, FLAG_AUTO));
        assert_eq!(zoom().sanitize(20, FLAG_AUTO), (20, FLAG_MANUAL));
    }
}
