//! LensLink virtual camera.
//!
//! Windows 11 virtual cameras (`MFCreateVirtualCamera`) are Media Foundation media sources in a COM DLL.
//! The Frame Server service loads the DLL in its own process (session 0, LOCAL SERVICE) and shares the
//! camera with every app that opens it. The LensLink host decodes the phone stream and passes NV12
//! frames to the DLL through shared memory ([`shm`]).
//!
//! COM object chain: class factory → [`activator::Activator`] (`IMFActivate`) → `source::Source`
//! (`IMFMediaSourceEx`) → `stream::Stream` (`IMFMediaStream2`), which hands out the frames.

#![allow(non_snake_case)] // COM method names

pub mod modes;
pub mod shm;

mod activator;
mod attrs;
mod dll;
mod log;
mod source;
mod stream;

use windows_core::GUID;

/// COM class id of [`activator::Activator`]; also the virtual camera's source id.
pub const CLSID: GUID = GUID::from_u128(0x500dfec2_f092_44aa_8c97_4604030626c1);
pub const CLSID_STR: &str = "{500DFEC2-F092-44AA-8C97-4604030626C1}";
/// Name shown in camera pickers.
pub const FRIENDLY_NAME: &str = "LensLink";

/// The object the Frame Server creates; exposed for in-process tests.
#[doc(hidden)]
pub fn activator() -> windows_core::Result<windows::Win32::Media::MediaFoundation::IMFActivate> {
    Ok(activator::Activator::new()?.into())
}
