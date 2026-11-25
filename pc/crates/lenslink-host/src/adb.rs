//! Thin wrapper over the `adb` command-line tool.

use std::path::PathBuf;
use std::process::{Command, Output};

/// `adb` from `ANDROID_HOME` when present, otherwise whatever is on PATH.
pub fn path() -> PathBuf {
    if let Some(home) = std::env::var_os("ANDROID_HOME") {
        let candidate = PathBuf::from(home).join("platform-tools").join("adb.exe");
        if candidate.exists() {
            return candidate;
        }
    }
    PathBuf::from("adb")
}

pub fn run(serial: Option<&str>, args: &[&str]) -> Result<Output, String> {
    let mut cmd = Command::new(path());
    if let Some(s) = serial {
        cmd.args(["-s", s]);
    }
    cmd.args(args)
        .output()
        .map_err(|e| format!("cannot run adb ({e}); install Android platform-tools"))
}

/// Runs an adb command and turns a non-zero exit into an error with adb's own message.
pub fn check(serial: Option<&str>, args: &[&str]) -> Result<Output, String> {
    let out = run(serial, args)?;
    if out.status.success() {
        Ok(out)
    } else {
        let msg = String::from_utf8_lossy(&out.stderr);
        Err(format!("adb {}: {}", args.join(" "), msg.trim()))
    }
}
