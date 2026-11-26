//! The camera's single video stream.
//!
//! Media Foundation pulls frames: every `RequestSample` must be answered with one `MEMediaSample`
//! event. A request is answered as soon as the host has published a frame newer than the last one
//! sent, so a frame is never held back waiting for a timer. Without a live host the stream sends a
//! placeholder at the nominal frame rate, so apps that opened the camera keep running.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::S_OK;
use windows::Win32::Media::KernelStreaming::{IKsControl, IKsControl_Impl, KSIDENTIFIER};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows_core::{implement, Interface, IUnknown, Ref, Result, GUID, HRESULT};

use crate::attrs::{forward_attributes, raw};
use crate::log::vlog;
use crate::modes::Mode;
use crate::shm::SharedFrames;

/// A host frame older than this means the host stopped; switch to the placeholder.
const HOST_TIMEOUT_MS: u64 = 1000;

struct State {
    pending: VecDeque<Option<IUnknown>>,
    running: bool,
    shutdown: bool,
    delivered_seq: u64,
    last_delivery: Instant,
    /// The media type the app selected.
    mode: Mode,
}

impl State {
    fn interval(&self) -> Duration {
        Duration::from_micros(1_000_000 / self.mode.fps.max(1) as u64)
    }
}

struct Shared {
    queue: IMFMediaEventQueue,
    frames: Option<SharedFrames>,
    state: Mutex<State>,
}

// Media Foundation objects used here are free-threaded.
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The host is publishing frames of the selected size.
    fn host_live(&self, mode: Mode) -> bool {
        self.frames.as_ref().is_some_and(|f| {
            f.age_ms().is_some_and(|age| age < HOST_TIMEOUT_MS) && f.latest_size() == Some((mode.width, mode.height))
        })
    }

    /// Answers pending requests that can be answered now.
    fn serve(&self, st: &mut State, placeholder_due: bool) {
        while st.running && !st.pending.is_empty() {
            let live = self.host_live(st.mode);
            let fresh = live && self.frames.as_ref().is_some_and(|f| f.seq() > st.delivered_seq);
            if !(fresh || (!live && placeholder_due)) {
                return;
            }
            if let Err(e) = self.deliver(st, fresh) {
                vlog!("deliver failed: {e}");
                return;
            }
            if !fresh {
                return; // one placeholder per interval
            }
        }
    }

    fn deliver(&self, st: &mut State, from_host: bool) -> Result<()> {
        let token = st.pending.pop_front().flatten();
        let m = st.mode;
        let bytes = m.frame_bytes();
        unsafe {
            let buffer = MFCreateMemoryBuffer(bytes as u32)?;
            let mut p = std::ptr::null_mut();
            buffer.Lock(&mut p, None, None)?;
            let dst = std::slice::from_raw_parts_mut(p, bytes);
            let read = match (&self.frames, from_host) {
                (Some(f), true) => f.read(dst, m.width, m.height),
                _ => None,
            };
            match read {
                Some(seq) => st.delivered_seq = seq,
                None => placeholder(dst, m),
            }
            buffer.Unlock()?;
            buffer.SetCurrentLength(bytes as u32)?;

            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(MFGetSystemTime())?;
            sample.SetSampleDuration(10_000_000 / m.fps.max(1) as i64)?;
            if let Some(t) = &token {
                sample.SetUnknown(&MFSampleExtension_Token, t)?;
            }
            self.queue.QueueEventParamUnk(MEMediaSample.0 as u32, &GUID::zeroed(), S_OK, &sample)?;
        }
        st.last_delivery = Instant::now();
        Ok(())
    }
}

/// Dark grey frame: shows that the camera works but the phone is not streaming.
fn placeholder(dst: &mut [u8], m: Mode) {
    let luma = (m.width * m.height) as usize;
    dst[..luma].fill(40);
    dst[luma..].fill(128);
}

fn worker(shared: Arc<Shared>) {
    loop {
        let interval = shared.lock().interval();
        match &shared.frames {
            Some(f) => {
                f.wait(interval.as_millis() as u32 / 2);
            }
            None => std::thread::sleep(interval / 2),
        }
        let mut st = shared.lock();
        if st.shutdown {
            return;
        }
        let due = st.last_delivery.elapsed() >= st.interval();
        shared.serve(&mut st, due);
    }
}

#[implement(IMFMediaStream2, IKsControl, IMFAttributes)]
pub struct Stream {
    shared: Arc<Shared>,
    attrs: IMFAttributes,
    descriptor: IMFStreamDescriptor,
    source: Mutex<Option<IMFMediaSource>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Stream {
    pub fn new(descriptor: IMFStreamDescriptor, attrs: IMFAttributes, mode: Mode) -> Result<Self> {
        let frames = match SharedFrames::create() {
            Ok(f) => Some(f),
            Err(e) => {
                vlog!("shared memory unavailable, placeholder only: {e}");
                None
            }
        };
        let shared = Arc::new(Shared {
            queue: unsafe { MFCreateEventQueue()? },
            frames,
            state: Mutex::new(State {
                pending: VecDeque::new(),
                running: false,
                shutdown: false,
                delivered_seq: 0,
                last_delivery: Instant::now(),
                mode,
            }),
        });
        let w = shared.clone();
        let worker = std::thread::Builder::new()
            .name("lenslink-vcam".into())
            .spawn(move || worker(w))
            .map_err(|_| windows_core::Error::from(windows::Win32::Foundation::E_OUTOFMEMORY))?;
        Ok(Stream {
            shared,
            attrs,
            descriptor,
            source: Mutex::new(None),
            worker: Mutex::new(Some(worker)),
        })
    }

    pub fn set_source(&self, source: IMFMediaSource) {
        *self.source.lock().unwrap() = Some(source);
    }

    /// The app selected `mode`: deliver that size and ask the host to switch the phone to it.
    pub fn set_mode(&self, mode: Mode) {
        let mut st = self.shared.lock();
        st.mode = mode;
        if let Some(f) = &self.shared.frames {
            f.request(mode);
        }
        vlog!("app selected {mode}");
    }

    pub fn frames(&self) -> Option<&SharedFrames> {
        self.shared.frames.as_ref()
    }

    pub fn attributes(&self) -> IMFAttributes {
        self.attrs.clone()
    }

    pub fn start(&self, position: *const PROPVARIANT) -> Result<()> {
        let mut st = self.shared.lock();
        st.running = true;
        unsafe {
            self.shared
                .queue
                .QueueEventParamVar(MEStreamStarted.0 as u32, &GUID::zeroed(), S_OK, position)
        }?;
        vlog!("stream started");
        drop(st);
        Ok(())
    }

    pub fn stop(&self) -> Result<()> {
        let mut st = self.shared.lock();
        st.running = false;
        st.pending.clear();
        unsafe {
            self.shared
                .queue
                .QueueEventParamVar(MEStreamStopped.0 as u32, &GUID::zeroed(), S_OK, std::ptr::null())
        }?;
        vlog!("stream stopped");
        Ok(())
    }

    pub fn shutdown(&self) {
        {
            let mut st = self.shared.lock();
            st.shutdown = true;
            st.running = false;
            st.pending.clear();
        }
        unsafe {
            let _ = self.shared.queue.Shutdown();
        }
        if let Some(w) = self.worker.lock().unwrap().take() {
            let _ = w.join();
        }
        *self.source.lock().unwrap() = None;
    }
}

forward_attributes!(Stream_Impl, attrs);

macro_rules! forward_events {
    ($ty:ty, $($queue:ident).+) => {
        impl IMFMediaEventGenerator_Impl for $ty {
            fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
                unsafe { self.$($queue).+.GetEvent(flags.0) }
            }
            fn BeginGetEvent(&self, callback: Ref<IMFAsyncCallback>, state: Ref<IUnknown>) -> Result<()> {
                let q = &self.$($queue).+;
                unsafe { (Interface::vtable(q).BeginGetEvent)(q.as_raw(), raw(&callback), raw(&state)).ok() }
            }
            fn EndGetEvent(&self, result: Ref<IMFAsyncResult>) -> Result<IMFMediaEvent> {
                let q = &self.$($queue).+;
                let mut event = std::ptr::null_mut();
                unsafe {
                    (Interface::vtable(q).EndGetEvent)(q.as_raw(), raw(&result), &mut event).ok()?;
                    Ok(IMFMediaEvent::from_raw(event))
                }
            }
            fn QueueEvent(&self, met: u32, ext: *const GUID, hr: HRESULT, value: *const PROPVARIANT) -> Result<()> {
                unsafe { self.$($queue).+.QueueEventParamVar(met, ext, hr, value) }
            }
        }
    };
}
pub(crate) use forward_events;

forward_events!(Stream_Impl, shared.queue);

impl IMFMediaStream_Impl for Stream_Impl {
    fn GetMediaSource(&self) -> Result<IMFMediaSource> {
        self.source.lock().unwrap().clone().ok_or_else(|| MF_E_SHUTDOWN.into())
    }

    fn GetStreamDescriptor(&self) -> Result<IMFStreamDescriptor> {
        Ok(self.descriptor.clone())
    }

    fn RequestSample(&self, token: Ref<IUnknown>) -> Result<()> {
        let mut st = self.shared.lock();
        if st.shutdown {
            return Err(MF_E_SHUTDOWN.into());
        }
        if !st.running {
            return Err(MF_E_MEDIA_SOURCE_WRONGSTATE.into());
        }
        st.pending.push_back(token.as_ref().cloned());
        let due = st.last_delivery.elapsed() >= st.interval();
        self.shared.serve(&mut st, due);
        Ok(())
    }
}

impl IMFMediaStream2_Impl for Stream_Impl {
    fn SetStreamState(&self, value: MF_STREAM_STATE) -> Result<()> {
        let mut st = self.shared.lock();
        st.running = value == MF_STREAM_STATE_RUNNING;
        if !st.running {
            st.pending.clear();
        }
        Ok(())
    }

    fn GetStreamState(&self) -> Result<MF_STREAM_STATE> {
        Ok(if self.shared.lock().running { MF_STREAM_STATE_RUNNING } else { MF_STREAM_STATE_STOPPED })
    }
}

/// No camera controls yet: report every property set as unsupported.
macro_rules! no_ks_properties {
    ($ty:ty) => {
        impl IKsControl_Impl for $ty {
            fn KsProperty(&self, _: *const KSIDENTIFIER, _: u32, _: *mut core::ffi::c_void, _: u32, _: *mut u32) -> Result<()> {
                Err(HRESULT::from_win32(windows::Win32::Foundation::ERROR_SET_NOT_FOUND.0).into())
            }
            fn KsMethod(&self, _: *const KSIDENTIFIER, _: u32, _: *mut core::ffi::c_void, _: u32, _: *mut u32) -> Result<()> {
                Err(HRESULT::from_win32(windows::Win32::Foundation::ERROR_SET_NOT_FOUND.0).into())
            }
            fn KsEvent(&self, _: *const KSIDENTIFIER, _: u32, _: *mut core::ffi::c_void, _: u32, _: *mut u32) -> Result<()> {
                Err(HRESULT::from_win32(windows::Win32::Foundation::ERROR_SET_NOT_FOUND.0).into())
            }
        }
    };
}

no_ks_properties!(Stream_Impl);
