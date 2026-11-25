//! GPU decode and preview: encoded packets in, frames on screen.

pub mod decoder;
mod preview;

use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Media::MediaFoundation::{MFStartup, MFSTARTUP_NOSOCKET, MF_VERSION};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};

pub use crate::stream::Codec;
use decoder::Decoder;
use preview::Preview;

/// COM and Media Foundation for the calling thread; call once before anything else here.
pub fn init() -> Result<(), String> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().map_err(|e| format!("COM: {e}"))?;
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).map_err(|e| format!("Media Foundation: {e}"))
    }
}

/// A hardware D3D11 device that the decoder and the window share.
/// Multithread protection is required because the decoder uses the device from its own threads.
pub fn create_device() -> windows::core::Result<ID3D11Device> {
    unsafe {
        let mut device = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )?;
        let device = device.expect("D3D11CreateDevice returned no device");
        let _ = device.cast::<ID3D11Multithread>()?.SetMultithreadProtected(true);
        Ok(device)
    }
}

/// Decoder plus window, driven from the thread that created it.
pub struct Viewer {
    decoder: Decoder,
    preview: Preview,
    title: String,
}

impl Viewer {
    pub fn new(codec: Codec, width: u32, height: u32, fps: u32, title: &str) -> Result<Self, String> {
        if !decoder::available(codec) {
            return Err(match codec {
                Codec::H265 => "Windows has no H.265 decoder. Install \"HEVC Video Extensions\" from the \
                                Microsoft Store, or use --codec h264"
                    .into(),
                Codec::H264 => "Windows has no hardware H.264 decoder; use --ffplay".into(),
            });
        }
        let device = create_device().map_err(|e| format!("Direct3D 11: {e}"))?;
        let decoder =
            Decoder::new(&device, codec, width, height, fps).map_err(|e| format!("decoder: {e}"))?;
        let preview = Preview::new(&device, title, (width, height)).map_err(|e| format!("window: {e}"))?;
        Ok(Viewer { decoder, preview, title: title.to_string() })
    }

    /// Returns false once the user closed the window.
    pub fn pump(&mut self) -> bool {
        self.preview.pump()
    }

    pub fn packet(&mut self, data: &[u8], pts_us: u64, config: bool, keyframe: bool) -> Result<(), String> {
        let frames = self
            .decoder
            .decode(data, pts_us, config, keyframe)
            .map_err(|e| format!("{:?} decode: {e}", self.decoder.codec()))?;
        // Only the newest frame matters for a live view.
        if let Some(frame) = frames.last() {
            self.preview.show(frame).map_err(|e| format!("display: {e}"))?;
        }
        Ok(())
    }

    pub fn status(&self, text: &str) {
        self.preview.set_title(&format!("{} — {text}", self.title));
    }
}
