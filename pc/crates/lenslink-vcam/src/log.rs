//! Minimal file log. The DLL runs inside a Windows service with no console, so this is the only way
//! to see what it does. The installer creates the folder and lets LOCAL SERVICE write there.

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

const PATH: &str = r"C:\ProgramData\LensLink\logs\vcam.log";
const MAX_BYTES: u64 = 1 << 20;

pub fn log(msg: &str) {
    if std::fs::metadata(PATH).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::rename(PATH, format!("{PATH}.old"));
    }
    let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(PATH) else {
        return;
    };
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let _ = writeln!(f, "{}.{:03} [{}] {msg}", t.as_secs(), t.subsec_millis(), std::process::id());
}

macro_rules! vlog {
    ($($arg:tt)*) => { $crate::log::log(&format!($($arg)*)) };
}
pub(crate) use vlog;
