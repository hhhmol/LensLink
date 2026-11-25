//! A plain Win32 window that shows decoded frames.
//!
//! The D3D11 video processor converts the decoder's NV12 texture to RGB and scales it straight into a
//! flip-model swap chain, so a frame never leaves the GPU. Presenting with sync interval 0 hands the
//! frame to the compositor at once instead of queueing it behind earlier ones.

use std::cell::Cell;

use windows::core::{w, Interface, HSTRING};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::decoder::Frame;

thread_local! {
    static RESIZED: Cell<bool> = const { Cell::new(false) };
    static CLOSED: Cell<bool> = const { Cell::new(false) };
}

struct Processor {
    input_size: (u32, u32),
    output_size: (u32, u32),
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
}

pub struct Preview {
    hwnd: HWND,
    swapchain: IDXGISwapChain1,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext1,
    processor: Option<Processor>,
    output_view: Option<ID3D11VideoProcessorOutputView>,
    /// Visible picture size; decoder textures are padded (1080 → 1088).
    picture: (u32, u32),
}

impl Preview {
    pub fn new(device: &ID3D11Device, title: &str, picture: (u32, u32)) -> windows::core::Result<Self> {
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let instance = GetModuleHandleW(None)?;
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                lpszClassName: w!("LensLinkPreview"),
                ..Default::default()
            };
            RegisterClassExW(&class);

            // Half of a 1080p picture fits comfortably next to other windows; the user can resize.
            let scale = (960.0 / picture.0 as f64).min(1.0);
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: (picture.0 as f64 * scale) as i32,
                bottom: (picture.1 as f64 * scale) as i32,
            };
            AdjustWindowRect(&mut rect, WS_OVERLAPPEDWINDOW, false)?;
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("LensLinkPreview"),
                &HSTRING::from(title),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                rect.right - rect.left,
                rect.bottom - rect.top,
                None,
                None,
                Some(instance.into()),
                None,
            )?;
            CLOSED.with(|c| c.set(false));

            let factory: IDXGIFactory2 = device.cast::<IDXGIDevice>()?.GetAdapter()?.GetParent()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                ..Default::default()
            };
            let swapchain = factory.CreateSwapChainForHwnd(device, hwnd, &desc, None, None)?;
            factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)?;
            if let Ok(d) = device.cast::<IDXGIDevice1>() {
                let _ = d.SetMaximumFrameLatency(1);
            }

            Ok(Preview {
                hwnd,
                swapchain,
                video_device: device.cast()?,
                video_context: device.GetImmediateContext()?.cast()?,
                processor: None,
                output_view: None,
                picture,
            })
        }
    }

    /// Handles window messages. Returns false once the window is closed.
    pub fn pump(&mut self) -> bool {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        !CLOSED.with(Cell::get)
    }

    pub fn set_title(&self, title: &str) {
        unsafe {
            let _ = SetWindowTextW(self.hwnd, &HSTRING::from(title));
        }
    }

    pub fn show(&mut self, frame: &Frame) -> windows::core::Result<()> {
        unsafe {
            if RESIZED.with(|r| r.replace(false)) {
                self.output_view = None;
                self.swapchain.ResizeBuffers(0, 0, 0, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
            }
            let desc = self.swapchain.GetDesc1()?;
            let out = (desc.Width, desc.Height);
            if out.0 == 0 || out.1 == 0 {
                return Ok(()); // minimised
            }

            let mut tex = D3D11_TEXTURE2D_DESC::default();
            frame.texture.GetDesc(&mut tex);
            let input = (tex.Width, tex.Height);
            if self.processor.as_ref().is_none_or(|p| p.input_size != input || p.output_size != out) {
                self.processor = Some(self.create_processor(input, out)?);
                self.output_view = None;
            }
            let p = self.processor.as_ref().unwrap();

            if self.output_view.is_none() {
                let back: ID3D11Texture2D = self.swapchain.GetBuffer(0)?;
                let view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                    ..Default::default()
                };
                let mut view = None;
                self.video_device
                    .CreateVideoProcessorOutputView(&back, &p.enumerator, &view_desc, Some(&mut view))?;
                self.output_view = view;
            }

            let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: frame.index },
                },
            };
            let mut in_view = None;
            self.video_device
                .CreateVideoProcessorInputView(&frame.texture, &p.enumerator, &in_desc, Some(&mut in_view))?;

            let vc = &self.video_context;
            let (pw, ph) = (self.picture.0.min(input.0), self.picture.1.min(input.1));
            let source = RECT { left: 0, top: 0, right: pw as i32, bottom: ph as i32 };
            let target = RECT { left: 0, top: 0, right: out.0 as i32, bottom: out.1 as i32 };
            vc.VideoProcessorSetStreamSourceRect(&p.processor, 0, true, Some(&source));
            vc.VideoProcessorSetStreamDestRect(&p.processor, 0, true, Some(&letterbox((pw, ph), out)));
            vc.VideoProcessorSetOutputTargetRect(&p.processor, true, Some(&target));

            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: std::mem::ManuallyDrop::new(in_view),
                ..Default::default()
            };
            let streams = [stream];
            let result = vc.VideoProcessorBlt(&p.processor, self.output_view.as_ref().unwrap(), 0, &streams);
            let [mut stream] = streams;
            std::mem::ManuallyDrop::drop(&mut stream.pInputSurface);
            result?;

            self.swapchain.Present(0, DXGI_PRESENT(0)).ok()
        }
    }

    unsafe fn create_processor(&self, input: (u32, u32), output: (u32, u32)) -> windows::core::Result<Processor> {
        unsafe {
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
            // Phone encoders write limited-range BT.709 for HD video.
            vc.VideoProcessorSetStreamColorSpace1(&processor, 0, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709);
            vc.VideoProcessorSetOutputColorSpace1(&processor, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709);
            // No driver "enhancements": they cost time and change the picture.
            vc.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            let black = D3D11_VIDEO_COLOR {
                Anonymous: D3D11_VIDEO_COLOR_0 {
                    RGBA: D3D11_VIDEO_COLOR_RGBA { R: 0.0, G: 0.0, B: 0.0, A: 1.0 },
                },
            };
            vc.VideoProcessorSetOutputBackgroundColor(&processor, false, &black);
            Ok(Processor { input_size: input, output_size: output, enumerator, processor })
        }
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        unsafe {
            if !CLOSED.with(Cell::get) {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

/// Largest rectangle with the picture's aspect ratio, centred in the window.
fn letterbox(picture: (u32, u32), window: (u32, u32)) -> RECT {
    let scale = (window.0 as f64 / picture.0 as f64).min(window.1 as f64 / picture.1 as f64);
    let w = (picture.0 as f64 * scale).round() as i32;
    let h = (picture.1 as f64 * scale).round() as i32;
    let x = (window.0 as i32 - w) / 2;
    let y = (window.1 as i32 - h) / 2;
    RECT { left: x, top: y, right: x + w, bottom: y + h }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_SIZE => {
                RESIZED.with(|r| r.set(true));
                LRESULT(0)
            }
            WM_KEYDOWN if wparam.0 == VK_ESCAPE.0 as usize => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                CLOSED.with(|c| c.set(true));
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::letterbox;

    #[test]
    fn letterbox_keeps_aspect() {
        let r = letterbox((1920, 1080), (1000, 1000));
        assert_eq!((r.left, r.top, r.right, r.bottom), (0, 218, 1000, 781));
        let r = letterbox((1920, 1080), (1920, 1080));
        assert_eq!((r.left, r.top, r.right, r.bottom), (0, 0, 1920, 1080));
    }
}
