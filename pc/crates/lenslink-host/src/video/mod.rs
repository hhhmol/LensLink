//! GPU decode and preview: encoded packets in, frames on screen.

pub mod decoder;
mod preview;
mod vcam;

use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Media::MediaFoundation::{MFStartup, MFSTARTUP_NOSOCKET, MF_VERSION};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};

pub use crate::stream::Codec;
use decoder::Decoder;
use lenslink_vcam::controls::{Control, ControlDesc};
use lenslink_vcam::modes::Mode;
use preview::Preview;
use vcam::VirtualCamera;

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

/// Decoder plus its outputs (window and/or virtual camera), driven from the thread that created it.
pub struct Viewer {
    device: ID3D11Device,
    codec: Codec,
    mode: Mode,
    decoder: Decoder,
    preview: Option<Preview>,
    camera: Option<VirtualCamera>,
    controls: Vec<ControlDesc>,
}

pub struct Outputs {
    pub window: bool,
    pub virtual_camera: bool,
}

impl Viewer {
    pub fn new(codec: Codec, mode: Mode, outputs: Outputs) -> Result<Self, String> {
        if !decoder::available(codec) {
            return Err(match codec {
                Codec::H265 => "Windows has no H.265 decoder. Install \"HEVC Video Extensions\" from the \
                                Microsoft Store, or use --codec h264"
                    .into(),
                Codec::H264 => "Windows has no hardware H.264 decoder; use --ffplay".into(),
            });
        }
        let device = create_device().map_err(|e| format!("Direct3D 11: {e}"))?;
        let decoder = Decoder::new(&device, codec, mode.width, mode.height, mode.fps)
            .map_err(|e| format!("decoder: {e}"))?;
        let picture = (mode.width, mode.height);
        let camera = if outputs.virtual_camera {
            let c = VirtualCamera::new(&device, picture, mode)?;
            eprintln!("virtual camera \"{}\" is available to apps", lenslink_vcam::FRIENDLY_NAME);
            Some(c)
        } else {
            None
        };
        let preview = if outputs.window {
            Some(Preview::new(&device, &title(codec, mode), picture).map_err(|e| format!("window: {e}"))?)
        } else {
            None
        };
        Ok(Viewer { device, codec, mode, decoder, preview, camera, controls: Vec::new() })
    }

    /// Returns false once the user closed the window.
    pub fn pump(&mut self) -> bool {
        self.preview.as_mut().is_none_or(Preview::pump)
    }

    /// A mode an app selected on the virtual camera, once per change.
    pub fn take_request(&mut self) -> Option<Mode> {
        self.camera.as_mut()?.take_request()
    }

    /// The controls offered on the virtual camera.
    pub fn set_controls(&mut self, controls: Vec<ControlDesc>) {
        self.controls = controls;
    }

    /// All control values after an app changed one, once per change.
    pub fn take_controls(&mut self) -> Option<Vec<(Control, i32, u32)>> {
        let descs = &self.controls;
        self.camera.as_mut()?.take_controls(descs)
    }

    /// The phone starts a new stream in `mode`: a fresh decoder, new picture size.
    pub fn set_stream(&mut self, mode: Mode) -> Result<(), String> {
        self.decoder = Decoder::new(&self.device, self.codec, mode.width, mode.height, mode.fps)
            .map_err(|e| format!("decoder: {e}"))?;
        self.mode = mode;
        let picture = (mode.width, mode.height);
        if let Some(c) = self.camera.as_mut() {
            c.set_picture(picture);
        }
        if let Some(p) = self.preview.as_mut() {
            p.set_picture(picture);
            p.set_title(&title(self.codec, mode));
        }
        Ok(())
    }

    /// Size and rate the virtual camera publishes (the mode an app selected).
    pub fn set_output(&mut self, mode: Mode) {
        if let Some(c) = self.camera.as_mut() {
            c.set_output(mode);
        }
    }

    /// Makes the next `take_controls` report the values apps set, for a freshly connected phone.
    pub fn resend_controls(&mut self) {
        if let Some(c) = self.camera.as_mut() {
            c.resend_controls();
        }
    }

    pub fn packet(&mut self, data: &[u8], pts_us: u64, config: bool, keyframe: bool) -> Result<(), String> {
        let frames = self
            .decoder
            .decode(data, pts_us, config, keyframe)
            .map_err(|e| format!("{:?} decode: {e}", self.decoder.codec()))?;
        // Only the newest frame matters for a live view.
        if let Some(frame) = frames.last() {
            if let Some(c) = self.camera.as_mut() {
                c.push(frame).map_err(|e| format!("virtual camera: {e}"))?;
            }
            if let Some(p) = self.preview.as_mut() {
                p.show(frame).map_err(|e| format!("display: {e}"))?;
            }
        }
        Ok(())
    }

    pub fn status(&self, text: &str) {
        if let Some(p) = &self.preview {
            let cam = match &self.camera {
                Some(c) if c.connected() => "  camera in use",
                _ => "",
            };
            p.set_title(&format!("{} — {text}{cam}", title(self.codec, self.mode)));
        }
    }
}

fn title(codec: Codec, mode: Mode) -> String {
    format!("LensLink {mode} {}", codec.wire_name())
}
