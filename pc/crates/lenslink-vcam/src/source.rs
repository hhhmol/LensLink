//! The media source: one live NV12 video stream offering every mode the phone supports.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::S_OK;
use windows::Win32::Media::KernelStreaming::{IKsControl, IKsControl_Impl, KSIDENTIFIER, PINNAME_VIDEO_CAPTURE};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows_core::{implement, ComObject, Interface, IUnknown, Ref, Result, BOOL, GUID, HRESULT};

use crate::attrs::{forward_attributes, raw};
use crate::log::vlog;
use crate::modes::{self, Mode};
use crate::{controls, ks};
use crate::stream::{forward_events, Stream};

#[implement(IMFMediaSourceEx, IMFGetService, IKsControl, IMFAttributes)]
pub struct Source {
    attrs: IMFAttributes,
    queue: IMFMediaEventQueue,
    descriptor: IMFPresentationDescriptor,
    stream: Mutex<Option<ComObject<Stream>>>,
    announced: AtomicBool,
    controls: OnceLock<ks::Controls>,
}

impl Source {
    pub fn create() -> Result<IMFMediaSourceEx> {
        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)?;
            let modes = modes::read();
            vlog!("modes: {}", modes.iter().map(Mode::to_string).collect::<Vec<_>>().join(" "));
            let types = modes.iter().map(|&m| media_type(m).map(Some)).collect::<Result<Vec<_>>>()?;
            let sd = MFCreateStreamDescriptor(0, &types)?;
            // The first mode in the file is the default.
            sd.GetMediaTypeHandler()?.SetCurrentMediaType(types[0].as_ref().unwrap())?;
            sd.SetGUID(&MF_DEVICESTREAM_STREAM_CATEGORY, &PINNAME_VIDEO_CAPTURE)?;
            sd.SetUINT32(&MF_DEVICESTREAM_STREAM_ID, 0)?;
            sd.SetUINT32(&MF_DEVICESTREAM_FRAMESERVER_SHARED, 1)?;
            sd.SetUINT32(&MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES, MFFrameSourceTypes_Color.0 as u32)?;
            let stream_attrs = new_attributes()?;
            sd.CopyAllItems(&stream_attrs)?;

            let pd = MFCreatePresentationDescriptor(Some(&[Some(sd.clone())]))?;
            pd.SelectStream(0)?;

            let source = ComObject::new(Source {
                attrs: new_attributes()?,
                queue: MFCreateEventQueue()?,
                descriptor: pd,
                stream: Mutex::new(None),
                announced: AtomicBool::new(false),
                controls: OnceLock::new(),
            });
            let stream = ComObject::new(Stream::new(sd, stream_attrs, modes[0])?);
            let ex: IMFMediaSourceEx = source.to_interface();
            stream.set_source(ex.cast()?);
            let descs = controls::read();
            vlog!("controls: {}", descs.iter().map(|d| d.control.name()).collect::<Vec<_>>().join(" "));
            let _ = source.controls.set(ks::Controls::new(descs, stream.frames()));
            *source.stream.lock().unwrap() = Some(stream);
            vlog!("source created");
            Ok(source.to_interface())
        }
    }

    fn stream(&self) -> Result<ComObject<Stream>> {
        self.stream.lock().unwrap().clone().ok_or_else(|| MF_E_SHUTDOWN.into())
    }
}

fn new_attributes() -> Result<IMFAttributes> {
    let mut a = None;
    unsafe { MFCreateAttributes(&mut a, 8)? };
    Ok(a.expect("MFCreateAttributes returned no store"))
}

fn media_type(m: Mode) -> Result<IMFMediaType> {
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, ((m.width as u64) << 32) | m.height as u64)?;
        t.SetUINT64(&MF_MT_FRAME_RATE, ((m.fps as u64) << 32) | 1)?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1)?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        t.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
        t.SetUINT32(&MF_MT_DEFAULT_STRIDE, m.width)?;
        t.SetUINT32(&MF_MT_SAMPLE_SIZE, m.frame_bytes() as u32)?;
        t.SetUINT32(&MF_MT_AVG_BITRATE, (m.frame_bytes() as u64 * 8 * m.fps as u64).min(u32::MAX as u64) as u32)?;
        Ok(t)
    }
}

/// The mode of the media type the app set on the stream.
fn current_mode(sd: &IMFStreamDescriptor) -> Result<Mode> {
    unsafe {
        let t = sd.GetMediaTypeHandler()?.GetCurrentMediaType()?;
        let size = t.GetUINT64(&MF_MT_FRAME_SIZE)?;
        let rate = t.GetUINT64(&MF_MT_FRAME_RATE)?;
        let den = (rate & 0xffff_ffff).max(1);
        let m = Mode { width: (size >> 32) as u32, height: size as u32, fps: ((rate >> 32) / den) as u32 };
        if m.is_valid() {
            Ok(m)
        } else {
            Err(MF_E_INVALIDMEDIATYPE.into())
        }
    }
}

forward_attributes!(Source_Impl, attrs);
forward_events!(Source_Impl, queue);
impl IKsControl_Impl for Source_Impl {
    fn KsProperty(&self, property: *const KSIDENTIFIER, plen: u32, data: *mut core::ffi::c_void, dlen: u32, returned: *mut u32) -> Result<()> {
        let stream = self.stream()?;
        let controls = self.controls.get().ok_or(windows_core::Error::from(MF_E_SHUTDOWN))?;
        unsafe { controls.property(stream.frames(), property as *const _, plen, data, dlen, returned) }
    }

    fn KsMethod(&self, _: *const KSIDENTIFIER, _: u32, _: *mut core::ffi::c_void, _: u32, _: *mut u32) -> Result<()> {
        Err(HRESULT::from_win32(windows::Win32::Foundation::ERROR_SET_NOT_FOUND.0).into())
    }

    fn KsEvent(&self, _: *const KSIDENTIFIER, _: u32, _: *mut core::ffi::c_void, _: u32, _: *mut u32) -> Result<()> {
        Err(HRESULT::from_win32(windows::Win32::Foundation::ERROR_SET_NOT_FOUND.0).into())
    }
}

impl IMFMediaSource_Impl for Source_Impl {
    fn GetCharacteristics(&self) -> Result<u32> {
        Ok(MFMEDIASOURCE_IS_LIVE.0 as u32)
    }

    fn CreatePresentationDescriptor(&self) -> Result<IMFPresentationDescriptor> {
        unsafe { self.descriptor.Clone() }
    }

    fn Start(&self, pd: Ref<IMFPresentationDescriptor>, _format: *const GUID, position: *const PROPVARIANT) -> Result<()> {
        let pd = pd.ok()?;
        let stream = self.stream()?;
        unsafe {
            let mut selected = BOOL(0);
            let mut sd = None;
            pd.GetStreamDescriptorByIndex(0, &mut selected, &mut sd)?;
            if selected.as_bool() {
                if let Some(sd) = &sd {
                    stream.set_mode(current_mode(sd)?);
                }
                let event = if self.announced.swap(true, Ordering::AcqRel) { MEUpdatedStream } else { MENewStream };
                let unknown: IUnknown = stream.to_interface();
                self.queue.QueueEventParamUnk(event.0 as u32, &GUID::zeroed(), S_OK, &unknown)?;
                stream.start(position)?;
            }
            self.queue.QueueEventParamVar(MESourceStarted.0 as u32, &GUID::zeroed(), S_OK, position)
        }
    }

    fn Stop(&self) -> Result<()> {
        self.stream()?.stop()?;
        unsafe {
            self.queue
                .QueueEventParamVar(MESourceStopped.0 as u32, &GUID::zeroed(), S_OK, std::ptr::null())
        }
    }

    fn Pause(&self) -> Result<()> {
        Err(MF_E_INVALID_STATE_TRANSITION.into())
    }

    fn Shutdown(&self) -> Result<()> {
        if let Some(s) = self.stream.lock().unwrap().take() {
            s.shutdown();
        }
        unsafe {
            let _ = self.queue.Shutdown();
        }
        vlog!("source shut down");
        Ok(())
    }
}

impl IMFMediaSourceEx_Impl for Source_Impl {
    fn GetSourceAttributes(&self) -> Result<IMFAttributes> {
        Ok(self.attrs.clone())
    }

    fn GetStreamAttributes(&self, id: u32) -> Result<IMFAttributes> {
        if id != 0 {
            return Err(MF_E_INVALIDSTREAMNUMBER.into());
        }
        Ok(self.stream()?.attributes())
    }

    fn SetD3DManager(&self, _manager: Ref<IUnknown>) -> Result<()> {
        Ok(()) // frames are delivered in system memory
    }
}

impl IMFGetService_Impl for Source_Impl {
    fn GetService(&self, _: *const GUID, _: *const GUID, _: *mut *mut core::ffi::c_void) -> Result<()> {
        Err(MF_E_UNSUPPORTED_SERVICE.into())
    }
}

