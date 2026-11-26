//! Which video modes a phone camera can stream, from the capability report the phone sends.
//!
//! A mode is offered only if the camera outputs that size at that rate, auto-exposure has a frame-rate
//! range ending at that rate, and the phone's hardware encoder handles it. Sizes are limited to the
//! common webcam ones so apps show a short, familiar list.

use lenslink_vcam::modes::Mode;
use serde_json::Value;

use crate::stream::Codec;

const SIZES: [(u32, u32); 7] = [
    (3840, 2160),
    (2560, 1440),
    (1920, 1080),
    (1600, 900),
    (1280, 720),
    (640, 480),
    (640, 360),
];
const RATES: [u32; 2] = [60, 30];

pub fn from_capabilities(caps: &Value, camera: &str, codec: Codec) -> Result<Vec<Mode>, String> {
    let cam = caps["cameras"]
        .as_array()
        .and_then(|cams| cams.iter().find(|c| c["id"].as_str() == Some(camera)))
        .ok_or_else(|| format!("the phone has no camera {camera}"))?;

    let sizes: Vec<((u32, u32), u32)> = cam["videoSizes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| Some((parse_size(v["size"].as_str()?)?, v["maxFps"].as_u64()? as u32)))
        .collect();
    let ae_upper: Vec<u32> = cam["aeTargetFpsRanges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r.as_str()?.split_once('-')?.1.parse().ok())
        .collect();

    let mime = match codec {
        Codec::H264 => "video/avc",
        Codec::H265 => "video/hevc",
    };
    let encoder = caps["encoders"]
        .as_array()
        .and_then(|e| e.iter().find(|e| e["hardware"].as_bool() == Some(true) && e["mime"].as_str() == Some(mime)))
        .ok_or_else(|| format!("the phone has no hardware {} encoder", codec.wire_name()))?;
    let (max_w, max_h) = encoder["maxSize"].as_str().and_then(parse_size).unwrap_or((1920, 1080));

    let mut modes = Vec::new();
    for (w, h) in SIZES {
        let Some(&(_, cam_fps)) = sizes.iter().find(|(s, _)| *s == (w, h)) else { continue };
        for fps in RATES {
            let encoder_fps = encoder["sizes"][bucket(h)]["maxFps"].as_u64().unwrap_or(30) as u32;
            if cam_fps >= fps && ae_upper.contains(&fps) && w <= max_w && h <= max_h && encoder_fps >= fps {
                modes.push(Mode { width: w, height: h, fps });
            }
        }
    }
    if modes.is_empty() {
        return Err(format!("camera {camera} offers none of the usual webcam modes"));
    }
    Ok(modes)
}

/// Bitrate for a mode: about 0.32 bit per pixel (20 Mbit/s at 1080p30), 2–60 Mbit/s.
pub fn bitrate(m: Mode) -> u32 {
    let bps = m.width as f64 * m.height as f64 * m.fps as f64 * 0.32;
    bps.clamp(2e6, 60e6) as u32
}

fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// The encoder report groups rates by standard height.
fn bucket(height: u32) -> &'static str {
    match height {
        0..=720 => "720p",
        721..=1080 => "1080p",
        1081..=1440 => "1440p",
        1441..=2160 => "2160p",
        _ => "4320p",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn m(width: u32, height: u32, fps: u32) -> Mode {
        Mode { width, height, fps }
    }

    /// Trimmed from the Redmi Note 8T report.
    fn note8t() -> Value {
        json!({
            "cameras": [{
                "id": "0",
                "aeTargetFpsRanges": ["15-15", "7-24", "24-24", "7-30", "30-30"],
                "videoSizes": [
                    {"size": "4000x3000", "maxFps": 24}, {"size": "3840x2160", "maxFps": 30},
                    {"size": "1920x1080", "maxFps": 30}, {"size": "1280x720", "maxFps": 30},
                    {"size": "640x480", "maxFps": 30}
                ]
            }],
            "encoders": [
                {"mime": "video/avc", "hardware": false, "maxSize": "2048x2048", "sizes": {}},
                {"mime": "video/avc", "hardware": true, "maxSize": "4096x2160",
                 "sizes": {"720p": {"maxFps": 240}, "1080p": {"maxFps": 120}, "2160p": {"maxFps": 30}}}
            ]
        })
    }

    #[test]
    fn note8t_modes() {
        let modes = from_capabilities(&note8t(), "0", Codec::H264).unwrap();
        assert_eq!(modes, [m(3840, 2160, 30), m(1920, 1080, 30), m(1280, 720, 30), m(640, 480, 30)]);
    }

    #[test]
    fn sixty_needs_camera_ae_and_encoder() {
        let mut caps = note8t();
        caps["cameras"][0]["aeTargetFpsRanges"] = json!(["30-30", "60-60"]);
        caps["cameras"][0]["videoSizes"] = json!([{"size": "3840x2160", "maxFps": 60}, {"size": "1920x1080", "maxFps": 60}]);
        let modes = from_capabilities(&caps, "0", Codec::H264).unwrap();
        // 4K60 is out: the encoder does 30 at 2160p.
        assert_eq!(modes, [m(3840, 2160, 30), m(1920, 1080, 60), m(1920, 1080, 30)]);
    }

    #[test]
    fn errors() {
        assert!(from_capabilities(&note8t(), "7", Codec::H264).is_err());
        assert!(from_capabilities(&note8t(), "0", Codec::H265).is_err());
    }

    #[test]
    fn bitrates() {
        assert_eq!(bitrate(m(1920, 1080, 30)), 19_906_560);
        assert_eq!(bitrate(m(3840, 2160, 30)), 60_000_000);
        assert_eq!(bitrate(m(640, 360, 30)), 2_211_840);
    }
}
