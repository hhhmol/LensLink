//! Thin wrapper over the `adb` command-line tool.

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// adb is a console program: without this it would flash a console window when the tray app (which
/// has no console) runs it.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `adb.exe` next to LensLink (as the installer puts it), from `ANDROID_HOME`, from Android Studio's
/// default SDK folder, otherwise whatever is on PATH.
pub fn path() -> PathBuf {
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)) {
        candidates.push(dir.join("adb.exe"));
        candidates.push(dir.join("platform-tools").join("adb.exe"));
    }
    if let Some(home) = std::env::var_os("ANDROID_HOME") {
        candidates.push(PathBuf::from(home).join("platform-tools").join("adb.exe"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("Android").join("Sdk").join("platform-tools").join("adb.exe"));
    }
    candidates.into_iter().find(|p| p.exists()).unwrap_or_else(|| PathBuf::from("adb"))
}

fn command() -> Command {
    let mut cmd = Command::new(path());
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

pub fn run(serial: Option<&str>, args: &[&str]) -> Result<Output, String> {
    let mut cmd = command();
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

/// `adb track-devices`: adb pushes the device list whenever it changes, so waiting for a phone costs
/// nothing instead of starting `adb devices` every second.
pub struct DeviceWatch {
    child: Child,
    rx: Receiver<Vec<(String, String)>>,
    latest: Option<Vec<(String, String)>>,
}

impl DeviceWatch {
    pub fn start() -> Result<Self, String> {
        let mut child = command()
            .arg("track-devices")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot run adb ({e}); install Android platform-tools"))?;
        let mut out = child.stdout.take().expect("stdout is piped");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            // Each message: four hex digits of length, then `serial\tstate` lines.
            let mut len = [0u8; 4];
            while out.read_exact(&mut len).is_ok() {
                let Some(n) = std::str::from_utf8(&len).ok().and_then(|s| usize::from_str_radix(s, 16).ok()) else {
                    return;
                };
                let mut body = vec![0; n];
                if out.read_exact(&mut body).is_err() || tx.send(parse_devices(&String::from_utf8_lossy(&body))).is_err() {
                    return;
                }
            }
        });
        Ok(DeviceWatch { child, rx, latest: None })
    }

    /// The newest device list; `None` until adb sent the first one. Err once adb quit (server killed,
    /// adb updated): start a new watch.
    #[allow(clippy::result_unit_err)] // the only error is "adb quit"
    pub fn devices(&mut self) -> Result<Option<&[(String, String)]>, ()> {
        loop {
            match self.rx.try_recv() {
                Ok(list) => self.latest = Some(list),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Err(()),
            }
        }
        Ok(self.latest.as_deref())
    }
}

impl Drop for DeviceWatch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
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
