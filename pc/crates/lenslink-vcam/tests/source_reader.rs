//! Reads frames from the media source in-process, the way an app would through the Frame Server.
//! Without a service account the shared memory cannot be created, so this sees placeholder frames.

use std::time::Instant;

use lenslink_vcam::shm::{FRAME_BYTES, HEIGHT, WIDTH};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

#[test]
fn delivers_frames() {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).unwrap();
        let activate = lenslink_vcam::activator().unwrap();
        let source: IMFMediaSource = activate.ActivateObject().unwrap();
        let reader = MFCreateSourceReaderFromMediaSource(&source, None).unwrap();

        let t = reader.GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32).unwrap();
        assert_eq!(t.GetGUID(&MF_MT_SUBTYPE).unwrap(), MFVideoFormat_NV12);
        assert_eq!(t.GetUINT64(&MF_MT_FRAME_SIZE).unwrap(), ((WIDTH as u64) << 32) | HEIGHT as u64);

        let start = Instant::now();
        let mut got = 0;
        while got < 10 {
            let (mut index, mut flags, mut ts) = (0u32, 0u32, 0i64);
            let mut sample = None;
            reader
                .ReadSample(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    0,
                    Some(&mut index),
                    Some(&mut flags),
                    Some(&mut ts),
                    Some(&mut sample),
                )
                .unwrap();
            if let Some(s) = sample {
                assert_eq!(s.GetTotalLength().unwrap() as usize, FRAME_BYTES);
                got += 1;
            }
        }
        let secs = start.elapsed().as_secs_f64();
        // Placeholder frames come at the nominal 30 fps: 10 frames take about a third of a second.
        assert!(secs > 0.2 && secs < 2.0, "10 frames took {secs:.2} s");
        source.Shutdown().unwrap();
    }
}
