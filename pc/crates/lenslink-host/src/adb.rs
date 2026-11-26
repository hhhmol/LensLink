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

/// `(serial, state)` of every device adb sees; state is `device` when usable, `unauthorized` while the
/// phone waits for the user to allow USB debugging, `offline` while it is coming up.
pub fn devices() -> Result<Vec<(String, String)>, String> {
    let out = check(None, &["devices"])?;
    Ok(parse_devices(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_devices(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter(|l| !l.starts_with('*') && !l.starts_with("List of devices"))
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            Some((parts.next()?.to_string(), parts.next()?.to_string()))
        })
        .collect()
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

#[cfg(test)]
mod tests {
    #[test]
    fn devices() {
        let text = "* daemon started successfully\nList of devices attached\nabc123\tdevice\nxyz\tunauthorized\n\n";
        assert_eq!(
            super::parse_devices(text),
            [("abc123".into(), "device".into()), ("xyz".into(), "unauthorized".into())]
        );
    }
}
