//! The LensLink app window: borderless, on its own thread, hidden in the tray when closed.
//!
//! The streaming side (phone connection, decoder, virtual camera) runs on the thread that created the
//! [`Link`]. The window has a thread of its own so that it stays smooth whatever the connection does.
//! Frames never leave the GPU: the streaming side converts each decoded frame into an RGB texture on the
//! shared D3D11 device and posts a message, and the window draws that texture with Direct2D.

mod icon;
mod render;
mod window;

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use windows::core::Interface;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::video::decoder::Frame;

pub use window::{CLASS_NAME, WM_APP_SHOW};

static START_HIDDEN: AtomicBool = AtomicBool::new(false);

/// Start with the window hidden in the tray (when Windows starts the app at sign-in).
pub fn start_hidden(hidden: bool) {
    START_HIDDEN.store(hidden, Ordering::Relaxed);
}

/// A COM pointer handed to the other thread. The D3D11 device is multithread-protected and the
/// Direct2D factory is created multithreaded, so using them from both threads is allowed.
pub(crate) struct Com<T>(pub T);
unsafe impl<T> Send for Com<T> {}
unsafe impl<T> Sync for Com<T> {}

/// What both threads see.
pub(crate) struct Shared {
    /// The user chose Exit.
    quit: AtomicBool,
    /// The window is on screen (not hidden in the tray, not minimised).
    visible: AtomicBool,
    /// A redraw for a new frame is already posted.
    frame_pending: AtomicBool,
    hwnd: AtomicIsize,
    state: Mutex<State>,
}

#[derive(Default)]
pub(crate) struct State {
    pub status: String,
    /// The latest frame as RGB, `size` pixels.
    pub video: Option<Com<ID3D11Texture2D>>,
    pub size: (u32, u32),
    pub last_frame: Option<Instant>,
}

impl Shared {
    fn post(&self, msg: u32) {
        let hwnd = HWND(self.hwnd.load(Ordering::Acquire) as *mut _);
        if !hwnd.is_invalid() {
            unsafe {
                let _ = PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(0));
            }
        }
    }

    pub fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The streaming side's handle on the window.
pub struct Link {
    shared: Arc<Shared>,
    converter: Converter,
    /// Direct2D draws on the shared device's immediate context; our own use of it from this thread
    /// must not interleave with a Direct2D draw.
    d2d_lock: ID2D1Multithread,
    picture: (u32, u32),
    thread: Option<JoinHandle<()>>,
}

impl Link {
    pub fn new(device: &ID3D11Device, picture: (u32, u32)) -> Result<Self, String> {
        let factory: ID2D1Factory1 = unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_MULTI_THREADED, None) }
            .map_err(|e| format!("Direct2D: {e}"))?;
        let d2d_lock = factory.cast().map_err(|e| format!("Direct2D: {e}"))?;
        let shared = Arc::new(Shared {
            quit: AtomicBool::new(false),
            visible: AtomicBool::new(false),
            frame_pending: AtomicBool::new(false),
            hwnd: AtomicIsize::new(0),
            state: Mutex::new(State { size: picture, ..Default::default() }),
        });
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = {
            let shared = shared.clone();
            let device = Com(device.clone());
            let factory = Com(factory);
            let hidden = START_HIDDEN.load(Ordering::Relaxed);
            std::thread::Builder::new()
                .name("window".into())
                .spawn(move || window::run(shared, device, factory, hidden, ready_tx))
                .map_err(|e| format!("window thread: {e}"))?
        };
        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(format!("window: {e}")),
            Err(_) => return Err("window: the window thread ended".into()),
        }
        Ok(Link { shared, converter: Converter::new(device)?, d2d_lock, picture, thread: Some(thread) })
    }

    pub fn quit_requested(&self) -> bool {
        self.shared.quit.load(Ordering::Acquire)
    }

    /// The window is on screen, so the phone's camera should run.
    pub fn visible(&self) -> bool {
        self.shared.visible.load(Ordering::Acquire)
    }

    pub fn set_picture(&mut self, picture: (u32, u32)) {
        self.picture = picture;
    }

    pub fn status(&self, text: &str) {
        let mut st = self.shared.state();
        if st.status != text {
            st.status = text.to_string();
            drop(st);
            self.shared.post(window::WM_APP_STATUS);
        }
    }

    /// Converts the frame for the window and asks it to redraw. Skipped while the window is hidden.
    pub fn push(&mut self, frame: &Frame) -> windows::core::Result<()> {
        if !self.visible() {
            return Ok(());
        }
        let texture = unsafe {
            self.d2d_lock.Enter();
            let r = self.converter.convert(frame, self.picture);
            self.d2d_lock.Leave();
            r?
        };
        {
            let mut st = self.shared.state();
            st.size = self.converter.output_size();
            st.video = Some(Com(texture));
            st.last_frame = Some(Instant::now());
        }
        if !self.shared.frame_pending.swap(true, Ordering::AcqRel) {
            self.shared.post(window::WM_APP_FRAME);
        }
        Ok(())
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.shared.post(window::WM_APP_EXIT);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// NV12 decoder output to a BGRA texture of the visible picture, on the GPU's video processor.
struct Converter {
    device: ID3D11Device,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext1,
    gpu: Option<Gpu>,
}

struct Gpu {
    input: (u32, u32),
    output: (u32, u32),
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    target: ID3D11Texture2D,
    view: ID3D11VideoProcessorOutputView,
}

impl Converter {
    fn new(device: &ID3D11Device) -> Result<Self, String> {
        let e = |e: windows::core::Error| format!("window GPU setup: {e}");
        let context = unsafe { device.GetImmediateContext() }.map_err(e)?;
        Ok(Converter {
            device: device.clone(),
            video_device: device.cast().map_err(e)?,
            video_context: context.cast().map_err(e)?,
            gpu: None,
        })
    }

    fn output_size(&self) -> (u32, u32) {
        self.gpu.as_ref().map_or((0, 0), |g| g.output)
    }

    unsafe fn convert(&mut self, frame: &Frame, picture: (u32, u32)) -> windows::core::Result<ID3D11Texture2D> {
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            frame.texture.GetDesc(&mut desc);
            let input = (desc.Width, desc.Height);
            // Decoder textures are padded (1080 → 1088): only the picture is shown.
            let output = (picture.0.min(input.0), picture.1.min(input.1));
            if self.gpu.as_ref().is_none_or(|g| g.input != input || g.output != output) {
                self.gpu = None;
                self.gpu = Some(self.create(input, output)?);
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
            let rect = RECT { left: 0, top: 0, right: output.0 as i32, bottom: output.1 as i32 };
            let vc = &self.video_context;
            vc.VideoProcessorSetStreamSourceRect(&g.processor, 0, true, Some(&rect));
            vc.VideoProcessorSetStreamDestRect(&g.processor, 0, true, Some(&rect));
            vc.VideoProcessorSetOutputTargetRect(&g.processor, true, Some(&rect));
            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: std::mem::ManuallyDrop::new(in_view),
                ..Default::default()
            };
            let streams = [stream];
            let result = vc.VideoProcessorBlt(&g.processor, &g.view, 0, &streams);
            let [mut stream] = streams;
            std::mem::ManuallyDrop::drop(&mut stream.pInputSurface);
            result?;
            Ok(g.target.clone())
        }
    }

    unsafe fn create(&self, input: (u32, u32), output: (u32, u32)) -> windows::core::Result<Gpu> {
        unsafe {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: output.0,
                Height: output.1,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut target = None;
            self.device.CreateTexture2D(&desc, None, Some(&mut target))?;
            let target = target.unwrap();

            let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputWidth: input.0,
                InputHeight: input.1,
                OutputWidth: output.0,
                OutputHeight: output.1,
                Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
                ..Default::default()
            };
            let enumerator = self.video_device.CreateVideoProcessorEnumerator(&content)?;
            let processor = self.video_device.CreateVideoProcessor(&enumerator, 0)?;
            let vc = &self.video_context;
            vc.VideoProcessorSetStreamFrameFormat(&processor, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
            // Phone encoders write limited-range BT.709.
            vc.VideoProcessorSetStreamColorSpace1(&processor, 0, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709);
            vc.VideoProcessorSetOutputColorSpace1(&processor, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709);
            vc.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);

            let view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                ..Default::default()
            };
            let mut view = None;
            self.video_device.CreateVideoProcessorOutputView(&target, &enumerator, &view_desc, Some(&mut view))?;
            Ok(Gpu { input, output, enumerator, processor, target, view: view.unwrap() })
        }
    }
}
