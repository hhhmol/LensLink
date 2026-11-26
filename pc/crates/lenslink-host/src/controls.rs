//! Camera controls: which ones the phone supports, and how Windows values become phone settings.
//! Units are described in `lenslink_vcam::controls`.

use lenslink_vcam::controls::{Control, ControlDesc, FLAG_AUTO, FLAG_MANUAL};
use serde_json::{json, Map, Value};

/// Manual focus 0–100 spans 0 (infinity) to this many diopters (10 cm).
const FOCUS_DIOPTERS: f64 = 10.0;
/// Exposure range stops at one frame at 30 fps; the phone shortens it further at 60 fps.
const MAX_EXPOSURE_S: f64 = 1.0 / 30.0;

pub fn from_capabilities(caps: &Value, camera: &str) -> Vec<ControlDesc> {
    let Some(cam) = caps["cameras"].as_array().and_then(|c| c.iter().find(|c| c["id"].as_str() == Some(camera))) else {
        return Vec::new();
    };
    let strings = |key: &str| -> Vec<String> {
        cam[key].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect()
    };
    let pair = |key: &str| -> Option<(f64, f64)> {
        let a = cam[key].as_array()?;
        Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?))
    };
    let manual_sensor = strings("capabilities").iter().any(|c| c == "MANUAL_SENSOR");
    let both = FLAG_AUTO | FLAG_MANUAL;
    let mut out = Vec::new();
    let mut add = |control, min: i32, max: i32, step, default: i32, caps, default_flags| {
        let d = ControlDesc { control, min, max, step, default: default.clamp(min, max), caps, default_flags };
        if d.is_valid() {
            out.push(d);
        }
    };

    let zoom = pair("zoomRatioRange").or_else(|| Some((1.0, cam["maxDigitalZoom"].as_f64()?)));
    if let Some((lo, hi)) = zoom {
        add(Control::Zoom, (lo * 10.0).round() as i32, (hi * 10.0).round() as i32, 1, 10, FLAG_MANUAL, FLAG_MANUAL);
    }

    let af = strings("afModes");
    if af.iter().any(|m| m == "CONTINUOUS_VIDEO") {
        let caps = if af.iter().any(|m| m == "OFF") { both } else { FLAG_AUTO };
        add(Control::Focus, 0, 100, 1, 0, caps, FLAG_AUTO);
    }

    if let (true, Some((lo_ns, hi_ns))) = (manual_sensor, pair("exposureRangeNs")) {
        let lo = (lo_ns / 1e9).log2().ceil() as i32;
        let hi = (hi_ns / 1e9).min(MAX_EXPOSURE_S).log2().floor() as i32;
        add(Control::Exposure, lo, hi, 1, hi, both, FLAG_AUTO);
    }

    if let Some((lo, hi)) = pair("exposureCompensationEv") {
        add(Control::Brightness, (lo * 10.0).round() as i32, (hi * 10.0).round() as i32, 1, 0, FLAG_MANUAL, FLAG_MANUAL);
    }

    if let (true, Some((lo, hi))) = (manual_sensor, pair("isoRange")) {
        add(Control::Gain, lo as i32, hi as i32, 1, 400, both, FLAG_AUTO);
    }

    let presets = ["INCANDESCENT", "FLUORESCENT", "DAYLIGHT", "CLOUDY_DAYLIGHT", "SHADE"];
    if strings("awbModes").iter().any(|m| presets.contains(&m.as_str())) {
        add(Control::WhiteBalance, 2700, 7500, 100, 5500, both, FLAG_AUTO);
    }

    add(Control::PowerlineFrequency, 0, 3, 1, 3, FLAG_MANUAL, FLAG_MANUAL);
    out
}

/// A `set` message for the phone with every control's current value.
pub fn to_phone(values: &[(Control, i32, u32)]) -> Value {
    let mut m = Map::new();
    m.insert("type".into(), json!("set"));
    for &(control, v, flags) in values {
        let auto = flags & FLAG_AUTO != 0;
        let manual = |x: Value| if auto { Value::Null } else { x };
        let (key, value) = match control {
            Control::Zoom => ("zoom", json!(v as f64 / 10.0)),
            Control::Focus => ("focus_diopters", manual(json!(v as f64 / 100.0 * FOCUS_DIOPTERS))),
            Control::Exposure => ("exposure_ns", manual(json!((2f64.powi(v) * 1e9).round() as i64))),
            Control::Brightness => ("ev", json!(v as f64 / 10.0)),
            Control::Gain => ("iso", manual(json!(v))),
            Control::WhiteBalance => ("awb_kelvin", manual(json!(v))),
            Control::PowerlineFrequency => {
                ("antibanding", json!(["off", "50hz", "60hz", "auto"].get(v as usize).copied().unwrap_or("auto")))
            }
        };
        m.insert(key.into(), value);
    }
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note8t() -> Value {
        json!({ "cameras": [{
            "id": "0",
            "capabilities": ["BACKWARD_COMPATIBLE", "MANUAL_SENSOR"],
            "isoRange": [100, 3200],
            "exposureRangeNs": [15056, 32000000000u64],
            "exposureCompensationEv": [-4, 4],
            "afModes": ["AUTO", "OFF", "CONTINUOUS_VIDEO"],
            "awbModes": ["AUTO", "INCANDESCENT", "DAYLIGHT"],
            "zoomRatioRange": [1, 8]
        }]})
    }

    fn find(d: &[ControlDesc], c: Control) -> ControlDesc {
        *d.iter().find(|x| x.control == c).unwrap()
    }

    #[test]
    fn note8t_controls() {
        let d = from_capabilities(&note8t(), "0");
        assert_eq!(d.len(), 7);
        let z = find(&d, Control::Zoom);
        assert_eq!((z.min, z.max, z.default), (10, 80, 10));
        let e = find(&d, Control::Exposure);
        assert_eq!((e.min, e.max, e.default_flags), (-16, -5, FLAG_AUTO));
        let b = find(&d, Control::Brightness);
        assert_eq!((b.min, b.max), (-40, 40));
        assert_eq!(find(&d, Control::Gain).max, 3200);
    }

    #[test]
    fn no_manual_sensor_no_exposure() {
        let mut caps = note8t();
        caps["cameras"][0]["capabilities"] = json!(["BACKWARD_COMPATIBLE"]);
        let d = from_capabilities(&caps, "0");
        assert!(!d.iter().any(|x| x.control == Control::Exposure || x.control == Control::Gain));
    }

    #[test]
    fn phone_message() {
        let m = to_phone(&[
            (Control::Zoom, 6, FLAG_MANUAL),
            (Control::Exposure, -5, FLAG_MANUAL),
            (Control::Gain, 800, FLAG_AUTO),
            (Control::Brightness, -13, FLAG_MANUAL),
            (Control::PowerlineFrequency, 1, FLAG_MANUAL),
        ]);
        assert_eq!(m["zoom"], json!(0.6));
        assert_eq!(m["exposure_ns"], json!(31_250_000));
        assert_eq!(m["iso"], Value::Null);
        assert_eq!(m["ev"], json!(-1.3));
        assert_eq!(m["antibanding"], json!("50hz"));
    }
}
