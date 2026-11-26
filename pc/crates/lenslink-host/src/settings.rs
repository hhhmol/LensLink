//! The app's settings in `%APPDATA%\LensLink\settings.json`.
//!
//! Kept as plain JSON so that later versions can add keys and older ones ignore them. A missing or
//! broken file means defaults.

use std::path::PathBuf;

use serde_json::{json, Value};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Settings {
    /// Window position and size in screen pixels when it is neither minimised nor maximised.
    pub window: Option<[i32; 4]>,
    pub maximized: bool,
}

pub fn dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("LensLink"))
}

fn path() -> Option<PathBuf> {
    dir().map(|d| d.join("settings.json"))
}

impl Settings {
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .map(|v| Self::from_json(&v))
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        // Keys this version does not know survive a save.
        let mut v = std::fs::read_to_string(&p)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        self.write_json(&mut v);
        if let Err(e) = std::fs::write(&p, serde_json::to_string_pretty(&v).unwrap_or_default()) {
            eprintln!("cannot save {}: {e}", p.display());
        }
    }

    fn from_json(v: &Value) -> Self {
        let window = v["window"].as_array().and_then(|a| {
            let n: Vec<i32> = a.iter().filter_map(|x| x.as_i64().map(|x| x as i32)).collect();
            <[i32; 4]>::try_from(n).ok().filter(|r| r[2] > 0 && r[3] > 0)
        });
        Settings { window, maximized: v["maximized"].as_bool().unwrap_or(false) }
    }

    fn write_json(&self, v: &mut Value) {
        v["window"] = match self.window {
            Some(r) => json!(r),
            None => Value::Null,
        };
        v["maximized"] = json!(self.maximized);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_keeps_unknown_keys() {
        let s = Settings { window: Some([10, 20, 960, 540]), maximized: true };
        let mut v = json!({ "future": 1 });
        s.write_json(&mut v);
        assert_eq!(v["future"], 1);
        assert_eq!(Settings::from_json(&v), s);
    }

    #[test]
    fn broken_values_mean_defaults() {
        let v = json!({ "window": [1, 2, 0, 5], "maximized": "yes" });
        assert_eq!(Settings::from_json(&v), Settings::default());
    }
}
