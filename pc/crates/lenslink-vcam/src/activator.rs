//! `IMFActivate` that the Frame Server creates from our CLSID; it builds the media source on demand.

use std::sync::Mutex;

use windows::Win32::Media::MediaFoundation::*;
use windows_core::{implement, Interface, Result, GUID};

use crate::attrs::forward_attributes;
use crate::log::vlog;
use crate::source::Source;

#[implement(IMFActivate)]
pub struct Activator {
    attrs: IMFAttributes,
    source: Mutex<Option<IMFMediaSourceEx>>,
}

impl Activator {
    pub fn new() -> Result<Self> {
        let mut attrs = None;
        unsafe { MFCreateAttributes(&mut attrs, 4)? };
        Ok(Activator { attrs: attrs.expect("MFCreateAttributes returned no store"), source: Mutex::new(None) })
    }
}

forward_attributes!(Activator_Impl, attrs);

impl IMFActivate_Impl for Activator_Impl {
    fn ActivateObject(&self, riid: *const GUID, ppv: *mut *mut core::ffi::c_void) -> Result<()> {
        let mut source = self.source.lock().unwrap();
        if source.is_none() {
            *source = Some(Source::create().inspect_err(|e| vlog!("source creation failed: {e}"))?);
        }
        unsafe { source.as_ref().unwrap().query(riid, ppv).ok() }
    }

    fn ShutdownObject(&self) -> Result<()> {
        if let Some(s) = self.source.lock().unwrap().take() {
            unsafe {
                let _ = s.Shutdown();
            }
        }
        Ok(())
    }

    fn DetachObject(&self) -> Result<()> {
        self.source.lock().unwrap().take();
        Ok(())
    }
}
