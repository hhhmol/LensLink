//! The media source: one live NV12 video stream.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use windows::Win32::Foundation::S_OK;
use windows::Win32::Media::KernelStreaming::{IKsControl, IKsControl_Impl, KSIDENTIFIER, PINNAME_VIDEO_CAPTURE};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows_core::{implement, ComObject, Interface, IUnknown, Ref, Result, BOOL, GUID, HRESULT};

use crate::attrs::{forward_attributes, raw};
use crate::log::vlog;
use crate::shm::{FPS, FRAME_BYTES, HEIGHT, WIDTH};
use crate::stream::{forward_events, no_ks_properties, Stream};

#[implement(IMFMediaSourceEx, IMFGetService, IKsControl, IMFAttributes)]
pub struct Source {
    attrs: IMFAttributes,
    queue: IMFMediaEventQueue,
    descriptor: IMFPresentationDescriptor,
    stream: Mutex<Option<ComObject<Stream>>>,
    announced: AtomicBool,
}

impl Source {
    pub fn create() -> Result<IMFMediaSourceEx> {
        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)?;
            let media_type = media_type()?;
            let sd = MFCreateStreamDescriptor(0, &[Some(media_type.clone())])?;
            sd.GetMediaTypeHandler()?.SetCurrentMediaType(&media_type)?;
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
            });
            let stream = ComObject::new(Stream::new(sd, stream_attrs)?);
            let ex: IMFMediaSourceEx = source.to_interface();
            stream.set_source(ex.cast()?);
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

fn media_type() -> Result<IMFMediaType> {
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, ((WIDTH as u64) << 32) | HEIGHT as u64)?;
        t.SetUINT64(&MF_MT_FRAME_RATE, ((FPS as u64) << 32) | 1)?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1)?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        t.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
        t.SetUINT32(&MF_MT_DEFAULT_STRIDE, WIDTH)?;
        t.SetUINT32(&MF_MT_SAMPLE_SIZE, FRAME_BYTES as u32)?;
        t.SetUINT32(&MF_MT_AVG_BITRATE, WIDTH * HEIGHT * 12 * FPS)?;
        Ok(t)
    }
}

forward_attributes!(Source_Impl, attrs);
forward_events!(Source_Impl, queue);
no_ks_properties!(Source_Impl);

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

