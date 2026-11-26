//! Feeds decoded frames to the LensLink virtual camera.
//!
//! The camera exists while this object lives (`MFVirtualCameraLifetime_Session`). Each frame is scaled
//! on the GPU into an NV12 texture of the mode the app selected (letterboxed when the aspect ratio
//! differs), read back, and published to the camera through shared memory. Frames are dropped cheaply
//! while no app has the camera open, because then the camera side has not created the shared memory.

use std::time::{Duration, Instant};

use lenslink_vcam::modes::Mode;
use lenslink_vcam::shm::SharedFrames;
use windows::core::{Interface, HSTRING};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Media::MediaFoundation::*;

use super::decoder::Frame;

const RETRY: Duration = Duration::from_millis(500);

pub struct VirtualCamera {
    camera: IMFVirtualCamera,
    frames: Option<SharedFrames>,
    next_try: Instant,
    last_request: Option<Mode>,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext1,
    /// Size of the frames published to the camera.
    output: Mode,
    /// Visible picture size inside the padded decoder texture.
    picture: (u32, u32),
    gpu: Option<Gpu>,
}

/// Scaler and textures for one input/output size pair.
struct Gpu {
    input_size: (u32, u32),
    output_size: (u32, u32),
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    output_view: ID3D11VideoProcessorOutputView,
    target: ID3D11Texture2D,
    staging: ID3D11Texture2D,
}

impl VirtualCamera {
    pub fn new(device: &ID3D11Device, picture: (u32, u32), output: Mode) -> Result<Self, String> {
        let camera = unsafe {
            MFCreateVirtualCamera(
                MFVirtualCameraType_SoftwareCameraSource,
                MFVirtualCameraLifetime_Session,
                MFVirtualCameraAccess_CurrentUser,
                &HSTRING::from(lenslink_vcam::FRIENDLY_NAME),
                &HSTRING::from(lenslink_vcam::CLSID_STR),
                None,
            )
        }
        .map_err(|e| format!("cannot create the virtual camera: {e}"))?;
        unsafe { camera.Start(None) }.map_err(|e| {
            format!(
                "the virtual camera did not start ({e}). Is it installed? Run tools\\install-vcam.ps1 \
                 as administrator once."
            )
        })?;
        let gpu = |e: windows::core::Error| format!("virtual camera GPU setup: {e}");
        let context = unsafe { device.GetImmediateContext() }.map_err(gpu)?;
        Ok(VirtualCamera {
            camera,
            frames: None,
            next_try: Instant::now(),
            last_request: None,
            device: device.clone(),
            video_device: device.cast().map_err(gpu)?,
            video_context: context.cast().map_err(gpu)?,
            context,
            output,
            picture,
            gpu: None,
        })
    }

    /// Whether an app has had the camera open (as far as we can tell).
    pub fn connected(&self) -> bool {
        self.frames.is_some()
    }

    /// A mode an app newly selected, once per change.
    pub fn take_request(&mut self) -> Option<Mode> {
        let r = self.frames.as_ref()?.requested()?;
        if self.last_request == Some(r) {
            return None;
        }
        self.last_request = Some(r);
        Some(r)
    }

    /// The phone now streams `picture`-sized frames; publish `output`-sized ones.
    pub fn set_mode(&mut self, picture: (u32, u32), output: Mode) {
        self.picture = picture;
        self.output = output;
    }

    pub fn push(&mut self, frame: &Frame) -> windows::core::Result<()> {
        if self.frames.is_none() {
            if Instant::now() < self.next_try {
                return Ok(());
            }
            self.next_try = Instant::now() + RETRY;
            match SharedFrames::open() {
                Ok(f) => {
                    eprintln!("\nvirtual camera: an app opened the camera");
                    self.frames = Some(f);
                }
                Err(_) => return Ok(()),
            }
        }
        unsafe {
            self.scale(frame)?;
            let g = self.gpu.as_ref().unwrap();
            self.context.CopyResource(&g.staging, &g.target);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(&g.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let src = mapped.pData as *const u8;
            let pitch = mapped.RowPitch as usize;
            let (w, h) = (self.output.width as usize, self.output.height as usize);
            self.frames.as_ref().unwrap().publish(self.output, |dst| {
                // Luma rows, then the interleaved chroma plane, which starts after `h` padded rows.
                for row in 0..h + h / 2 {
                    std::ptr::copy_nonoverlapping(src.add(row * pitch), dst.as_mut_ptr().add(row * w), w);
                }
            });
            self.context.Unmap(&g.staging, 0);
        }
        Ok(())
    }

    unsafe fn scale(&mut self, frame: &Frame) -> windows::core::Result<()> {
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            frame.texture.GetDesc(&mut desc);
            let input = (desc.Width, desc.Height);
            let output = (self.output.width, self.output.height);
            if self.gpu.as_ref().is_none_or(|g| g.input_size != input || g.output_size != output) {
                self.gpu = None;
                self.gpu = Some(self.create_gpu(input, output)?);
            }
            let g = self.gpu.as_ref().unwrap();

            let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: frame.index },
                },
            };
            let mut in_view = None;
            self.video_device
                .CreateVideoProcessorInputView(&frame.texture, &g.enumerator, &in_desc, Some(&mut in_view))?;

            let (pw, ph) = (self.picture.0.min(input.0), self.picture.1.min(input.1));
            let vc = &self.video_context;
            let source = RECT { left: 0, top: 0, right: pw as i32, bottom: ph as i32 };
            vc.VideoProcessorSetStreamSourceRect(&g.processor, 0, true, Some(&source));
            let dest = super::preview::letterbox((pw, ph), output);
            vc.VideoProcessorSetStreamDestRect(&g.processor, 0, true, Some(&dest));

            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: std::mem::ManuallyDrop::new(in_view),
                ..Default::default()
            };
            let streams = [stream];
            let result = vc.VideoProcessorBlt(&g.processor, &g.output_view, 0, &streams);
            let [mut stream] = streams;
            std::mem::ManuallyDrop::drop(&mut stream.pInputSurface);
            result
        }
    }

    unsafe fn create_gpu(&self, input: (u32, u32), output: (u32, u32)) -> windows::core::Result<Gpu> {
        unsafe {
            let nv12 = |usage, bind: D3D11_BIND_FLAG, cpu: D3D11_CPU_ACCESS_FLAG| D3D11_TEXTURE2D_DESC {
                Width: output.0,
                Height: output.1,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_NV12,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: usage,
                BindFlags: bind.0 as u32,
                CPUAccessFlags: cpu.0 as u32,
                MiscFlags: 0,
            };
            let mut target = None;
            self.device.CreateTexture2D(
                &nv12(D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET, D3D11_CPU_ACCESS_FLAG(0)),
                None,
                Some(&mut target),
            )?;
            let target = target.unwrap();
            let mut staging = None;
            self.device.CreateTexture2D(
                &nv12(D3D11_USAGE_STAGING, D3D11_BIND_FLAG(0), D3D11_CPU_ACCESS_READ),
                None,
                Some(&mut staging),
            )?;

            let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputWidth: input.0,
                InputHeight: input.1,
                OutputWidth: output.0,
                OutputHeight: output.1,
                Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
                ..Default::default()
            };
            let enumerator = self.video_device.CreateVideoProcessorEnumerator(&desc)?;
            let processor = self.video_device.CreateVideoProcessor(&enumerator, 0)?;
            let vc = &self.video_context;
            vc.VideoProcessorSetStreamFrameFormat(&processor, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
            // YUV in, YUV out: scaling only, no colour conversion.
            vc.VideoProcessorSetStreamColorSpace1(&processor, 0, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709);
            vc.VideoProcessorSetOutputColorSpace1(&processor, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709);
            vc.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            let full = RECT { left: 0, top: 0, right: output.0 as i32, bottom: output.1 as i32 };
            vc.VideoProcessorSetOutputTargetRect(&processor, true, Some(&full));
            let black = D3D11_VIDEO_COLOR {
                Anonymous: D3D11_VIDEO_COLOR_0 {
                    YCbCr: D3D11_VIDEO_COLOR_YCbCrA { Y: 16.0 / 255.0, Cb: 0.5, Cr: 0.5, A: 1.0 },
                },
            };
            vc.VideoProcessorSetOutputBackgroundColor(&processor, true, &black);

            let out_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                ..Default::default()
            };
            let mut output_view = None;
            self.video_device
                .CreateVideoProcessorOutputView(&target, &enumerator, &out_desc, Some(&mut output_view))?;
            Ok(Gpu {
                input_size: input,
                output_size: output,
                enumerator,
                processor,
                output_view: output_view.unwrap(),
                target,
                staging: staging.unwrap(),
            })
        }
    }
}

impl Drop for VirtualCamera {
    fn drop(&mut self) {
        unsafe {
            let _ = self.camera.Remove();
            let _ = self.camera.Shutdown();
        }
    }
}
