//! Video modes the camera offers to apps.
//!
//! The host learns them from the phone and writes them to [`MODES_PATH`] before it creates the camera;
//! the camera source (in the Frame Server) reads the file when it starts. The file is user-writable,
//! so parsing is strict and bounded.

use std::fmt;

pub const MODES_PATH: &str = r"C:\ProgramData\LensLink\config\modes.txt";
const MAX_MODES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl Mode {
    pub const DEFAULT: Mode = Mode { width: 1920, height: 1080, fps: 30 };

    pub fn frame_bytes(self) -> usize {
        (self.width * self.height * 3 / 2) as usize
    }

    /// Fits the shared-memory slots and is a valid NV12 size.
    pub fn is_valid(self) -> bool {
        (16..=crate::shm::MAX_WIDTH).contains(&self.width)
            && (16..=crate::shm::MAX_HEIGHT).contains(&self.height)
            && self.width.is_multiple_of(2)
            && self.height.is_multiple_of(2)
            && (1..=240).contains(&self.fps)
    }

    /// `w << 40 | h << 16 | fps`, 0 = none. Used for atomics in shared memory.
    pub fn pack(self) -> u64 {
        ((self.width as u64) << 40) | ((self.height as u64) << 16) | self.fps as u64
    }

    pub fn unpack(v: u64) -> Option<Mode> {
        let m = Mode {
            width: ((v >> 40) & 0xff_ffff) as u32,
            height: ((v >> 16) & 0xff_ffff) as u32,
            fps: (v & 0xffff) as u32,
        };
        (v != 0 && m.is_valid()).then_some(m)
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}@{}", self.width, self.height, self.fps)
    }
}

/// One `WIDTHxHEIGHT@FPS` per line; the first line is the default.
pub fn format(modes: &[Mode]) -> String {
    modes.iter().map(|m| format!("{m}\n")).collect()
}

pub fn parse(text: &str) -> Vec<Mode> {
    let mut out = Vec::new();
    for line in text.lines().take(MAX_MODES) {
        let parsed = line.trim().split_once('x').and_then(|(w, rest)| {
            let (h, fps) = rest.split_once('@')?;
            Some(Mode { width: w.parse().ok()?, height: h.parse().ok()?, fps: fps.parse().ok()? })
        });
        if let Some(m) = parsed.filter(|m| m.is_valid() && !out.contains(m)) {
            out.push(m);
        }
    }
    out
}

pub fn write(modes: &[Mode]) -> std::io::Result<()> {
    std::fs::write(MODES_PATH, format(modes))
}

/// The modes file, or 1080p30 when it is missing or empty.
pub fn read() -> Vec<Mode> {
    let modes = std::fs::read_to_string(MODES_PATH).map(|t| parse(&t)).unwrap_or_default();
    if modes.is_empty() {
        vec![Mode::DEFAULT]
    } else {
        modes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let modes = [Mode { width: 3840, height: 2160, fps: 30 }, Mode::DEFAULT];
        assert_eq!(parse(&format(&modes)), modes);
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse("1920x1080@30\nfoo\n99999x1080@30\n1921x1080@30\n1920x1080@30\n640x360@0\n"), [Mode::DEFAULT]);
    }

    #[test]
    fn pack_unpack() {
        let m = Mode { width: 4096, height: 2160, fps: 60 };
        assert_eq!(Mode::unpack(m.pack()), Some(m));
        assert_eq!(Mode::unpack(0), None);
    }
}
