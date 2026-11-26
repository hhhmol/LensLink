//! Opens the "LensLink" camera like any app does (through the Frame Server), reads frames and
//! reports the frame rate. `cargo run -p lenslink-vcam --example grab -- [frames] [out.pgm] [WxH@FPS]`
//! lists the camera's modes, optionally selects one, and saves the luma of the last frame as a
//! greyscale PGM image.

use std::time::Instant;

use windows::core::PWSTR;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};

fn main() -> windows::core::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let frames: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(90);
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)?;

        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.unwrap();
        attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)?;
        let mut list = std::ptr::null_mut();
        let mut count = 0;
        MFEnumDeviceSources(&attrs, &mut list, &mut count)?;
        let mut found = None;
        for i in 0..count as usize {
            let a = (*list.add(i)).take().unwrap();
            let mut name = PWSTR::null();
            let mut len = 0;
            a.GetAllocatedString(&MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, &mut name, &mut len)?;
            let n = name.to_string().unwrap_or_default();
            CoTaskMemFree(Some(name.0 as _));
            println!("camera: {n}");
            // Windows appends a localized "(Windows Virtual Camera)".
            if n.starts_with(lenslink_vcam::FRIENDLY_NAME) {
                found = Some(a);
            }
        }
        CoTaskMemFree(Some(list as _));
        let Some(activate) = found else {
            println!("no LensLink camera: is `lenslink-host stream --vcam` running?");
            return Ok(());
        };

        let source: IMFMediaSource = activate.ActivateObject()?;
        let reader = MFCreateSourceReaderFromMediaSource(&source, None)?;
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        let wanted = args.get(3).map(String::as_str);
        let mut i = 0;
        while let Ok(t) = reader.GetNativeMediaType(stream, i) {
            let size = t.GetUINT64(&MF_MT_FRAME_SIZE)?;
            let rate = t.GetUINT64(&MF_MT_FRAME_RATE)?;
            let mode = format!("{}x{}@{}", size >> 32, size & 0xffff_ffff, (rate >> 32) / (rate & 0xffff_ffff).max(1));
            println!("mode {i}: {mode}");
            if wanted == Some(mode.as_str()) {
                reader.SetCurrentMediaType(stream, None, &t)?;
                println!("selected {mode}");
            }
            i += 1;
        }
        let size = reader.GetCurrentMediaType(stream)?.GetUINT64(&MF_MT_FRAME_SIZE)?;
        let (w, h) = ((size >> 32) as usize, (size & 0xffff_ffff) as usize);
        println!("format {w}x{h}");

        let mut start = None;
        let mut last = Vec::new();
        let mut placeholders = 0;
        for _ in 0..frames {
            let mut flags = 0u32;
            let mut sample = None;
            reader.ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample))?;
            let Some(s) = sample else { continue };
            start.get_or_insert_with(Instant::now);
            let buf = s.ConvertToContiguousBuffer()?;
            let mut p = std::ptr::null_mut();
            let mut n = 0u32;
            buf.Lock(&mut p, None, Some(&mut n))?;
            last = std::slice::from_raw_parts(p, (w * h).min(n as usize)).to_vec();
            // The placeholder is uniform grey 40.
            if last.iter().step_by(4099).all(|&v| v == 40) {
                placeholders += 1;
            }
            buf.Unlock()?;
        }
        let secs = start.map_or(0.0, |s| s.elapsed().as_secs_f64());
        let mean = last.iter().map(|&v| v as f64).sum::<f64>() / last.len().max(1) as f64;
        println!(
            "{frames} frames in {secs:.2} s = {:.1} fps, {placeholders} placeholder frames, last frame mean luma {mean:.0}",
            (frames - 1) as f64 / secs
        );
        if let Some(path) = args.get(2) {
            let mut out = format!("P5\n{w} {h}\n255\n").into_bytes();
            out.extend_from_slice(&last);
            std::fs::write(path, out).unwrap();
            println!("saved {path}");
        }
        source.Shutdown()?;
    }
    Ok(())
}
