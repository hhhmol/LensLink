//! Draws the window on a flip-model swap chain: the glass pass (video, floating elements, curtains)
//! with Direct3D, then the elements' labels with Direct2D and DirectWrite on top.

use std::cell::RefCell;

use windows::core::{w, Interface, Result};
use windows::Win32::Foundation::{CloseHandle, D2DERR_RECREATE_TARGET, HANDLE, HWND};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows_numerics::Vector2;

use super::glass::{Constants, Glass, EDGE_POINTS, MAX_ELEMENTS};

/// Colour around the video when the window's shape differs from the picture's.
const BACKGROUND: [f32; 4] = [0.035, 0.04, 0.05, 1.0];
/// How far outside the window a closed curtain's edge rests, in DIPs: further than the drops' reach.
const CURTAIN_REST: f32 = 24.0;

pub enum Label<'a> {
    Glyph(&'a str),
    /// Wrapped at `wrap` DIPs.
    Text { text: &'a str, large: bool, wrap: f32 },
}

/// A floating glass element, in DIPs.
pub struct Element<'a> {
    pub centre: (f32, f32),
    pub half: (f32, f32),
    /// Glass colour: rgb and amount.
    pub tint: [f32; 4],
    pub label: Label<'a>,
    pub label_alpha: f32,
}

pub struct Frame<'a> {
    /// The video texture and the picture's size in it.
    pub video: Option<(&'a ID3D11Texture2D, (u32, u32))>,
    pub elements: &'a [Element<'a>],
}

pub struct Renderer {
    swapchain: IDXGISwapChain2,
    waitable: HANDLE,
    glass: Glass,
    view: Option<ID3D11RenderTargetView>,
    /// The streaming thread converts frames on the same device context.
    lock: ID2D1Multithread,
    dc: ID2D1DeviceContext,
    target: Option<ID2D1Bitmap1>,
    dwrite: IDWriteFactory,
    small: IDWriteTextFormat,
    large: IDWriteTextFormat,
    glyphs: IDWriteTextFormat,
    brush: ID2D1SolidColorBrush,
    dpi: f32,
    /// Recently used text layouts: the same labels are drawn on every frame.
    layouts: RefCell<Vec<(LayoutKey, IDWriteTextLayout, DWRITE_TEXT_METRICS)>>,
}

#[derive(PartialEq)]
struct LayoutKey {
    text: String,
    format: u8,
    wrap: u32,
}

const LAYOUTS: usize = 16;
const GLYPH: u8 = 2;

const SWAP_FLAGS: DXGI_SWAP_CHAIN_FLAG = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT;

fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

impl Renderer {
    pub fn new(device: &ID3D11Device, factory: &ID2D1Factory1, hwnd: HWND, dpi: f32) -> Result<Self> {
        unsafe {
            let dxgi: IDXGIDevice = device.cast()?;
            let dxgi_factory: IDXGIFactory2 = dxgi.GetAdapter()?.GetParent()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                Flags: SWAP_FLAGS.0 as u32,
                ..Default::default()
            };
            let swapchain: IDXGISwapChain2 = dxgi_factory.CreateSwapChainForHwnd(device, hwnd, &desc, None, None)?.cast()?;
            dxgi_factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)?;
            // One frame in flight: the window answers the pointer without lag.
            swapchain.SetMaximumFrameLatency(1)?;
            let waitable = swapchain.GetFrameLatencyWaitableObject();

            let dc = factory.CreateDevice(&dxgi)?.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            dc.SetDpi(dpi, dpi);
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let format = |family, weight, size: f32| {
                dwrite.CreateTextFormat(family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("ru-ru"))
            };
            let small = format(w!("Segoe UI Variable Text"), DWRITE_FONT_WEIGHT_NORMAL, 12.5)?;
            small.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            let large = format(w!("Segoe UI Variable Text"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 15.0)?;
            large.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            let glyphs = format(w!("Segoe Fluent Icons"), DWRITE_FONT_WEIGHT_NORMAL, 10.0)?;
            glyphs.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            glyphs.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            let brush = dc.CreateSolidColorBrush(&color(1.0, 1.0, 1.0, 1.0), None)?;
            Ok(Renderer {
                swapchain,
                waitable,
                glass: Glass::new(device)?,
                view: None,
                lock: factory.cast()?,
                dc,
                target: None,
                dwrite,
                small,
                large,
                glyphs,
                brush,
                dpi,
                layouts: RefCell::default(),
            })
        }
    }

    /// Signalled when the swap chain takes the next frame: wait for it before drawing.
    pub fn waitable(&self) -> HANDLE {
        self.waitable
    }

    pub fn set_dpi(&mut self, dpi: f32) {
        self.dpi = dpi;
        self.target = None;
        unsafe { self.dc.SetDpi(dpi, dpi) };
    }

    /// The window changed size: the swap chain follows.
    pub fn resize(&mut self) -> Result<()> {
        unsafe {
            self.target = None;
            self.view = None;
            self.dc.SetTarget(None::<&ID2D1Image>);
            self.swapchain.ResizeBuffers(0, 0, 0, DXGI_FORMAT_UNKNOWN, SWAP_FLAGS)
        }
    }

    pub fn pixels(&self) -> (u32, u32) {
        let desc = unsafe { self.swapchain.GetDesc1() }.unwrap_or_default();
        (desc.Width, desc.Height)
    }

    /// Window size in DIPs.
    pub fn size(&self) -> (f32, f32) {
        let (w, h) = self.pixels();
        (w as f32 * 96.0 / self.dpi, h as f32 * 96.0 / self.dpi)
    }

    /// Size of `text` as a label wrapped at `wrap`, in DIPs.
    pub fn measure(&self, text: &str, large: bool, wrap: f32) -> (f32, f32) {
        self.layout(text, large as u8, wrap).map_or((0.0, 0.0), |(_, m)| (m.width, m.height))
    }

    /// A centred layout of `text` in a box `wrap` wide, and its metrics.
    fn layout(&self, text: &str, format: u8, wrap: f32) -> Option<(IDWriteTextLayout, DWRITE_TEXT_METRICS)> {
        let key = LayoutKey { text: text.to_string(), format, wrap: (wrap.max(1.0) * 4.0) as u32 };
        let mut cache = self.layouts.borrow_mut();
        if let Some(i) = cache.iter().position(|(k, ..)| *k == key) {
            let entry = cache.remove(i);
            let found = (entry.1.clone(), entry.2);
            cache.push(entry);
            return Some(found);
        }
        let f = match format {
            GLYPH => &self.glyphs,
            1 => &self.large,
            _ => &self.small,
        };
        let utf16: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let layout = self.dwrite.CreateTextLayout(&utf16, f, key.wrap as f32 / 4.0, 1000.0).ok()?;
            layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER).ok()?;
            layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR).ok()?;
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m).ok()?;
            if cache.len() >= LAYOUTS {
                cache.remove(0);
            }
            cache.push((key, layout.clone(), m));
            Some((layout, m))
        }
    }

    pub fn draw(&mut self, frame: &Frame) -> Result<()> {
        unsafe {
            let (pw, ph) = self.pixels();
            if pw == 0 || ph == 0 {
                return Ok(());
            }
            if self.view.is_none() {
                let buffer: ID3D11Texture2D = self.swapchain.GetBuffer(0)?;
                let mut view = None;
                buffer.GetDevice()?.CreateRenderTargetView(&buffer, None, Some(&mut view))?;
                self.view = view;
            }
            if self.target.is_none() {
                let surface: IDXGISurface = self.swapchain.GetBuffer(0)?;
                let props = D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                    dpiX: self.dpi,
                    dpiY: self.dpi,
                    bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                    ..Default::default()
                };
                let target = self.dc.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
                self.dc.SetTarget(&target);
                self.target = Some(target);
            }

            let px = self.dpi / 96.0;
            let mut c = Constants { size: [pw as f32, ph as f32], px, background: BACKGROUND, ..Default::default() };
            // Closed curtains rest just outside the window, out of reach of the elements near its edges.
            let rest = -CURTAIN_REST * px;
            c.edges = [[rest; 4]; EDGE_POINTS];
            c.edge_max = [rest; 4];
            if let Some((_, picture)) = frame.video {
                c.video = letterbox(picture, (pw, ph));
            }
            for (i, e) in frame.elements.iter().take(MAX_ELEMENTS).enumerate() {
                let (hw, hh) = (e.half.0 * px, e.half.1 * px);
                let r = hw.min(hh);
                c.boxes[i] = [e.centre.0 * px, e.centre.1 * px, hw, hh];
                c.corners[i] = [r, r, 0.0, 0.0];
                c.tints[i] = e.tint;
                c.count = (i + 1) as f32;
            }

            self.lock.Enter();
            let result = self.draw_locked(frame, &c);
            self.lock.Leave();
            if let Err(e) = &result {
                if e.code() == D2DERR_RECREATE_TARGET {
                    self.target = None;
                    self.view = None;
                }
            }
            result?;
            self.swapchain.Present(1, DXGI_PRESENT(0)).ok()
        }
    }

    unsafe fn draw_locked(&mut self, frame: &Frame, c: &Constants) -> Result<()> {
        unsafe {
            let view = self.view.clone().unwrap();
            self.glass.draw(&view, frame.video.map(|v| v.0), c)?;

            let dc = self.dc.clone();
            dc.BeginDraw();
            for e in frame.elements.iter().take(MAX_ELEMENTS) {
                if e.label_alpha <= 0.01 {
                    continue;
                }
                let (text, format, wrap) = match e.label {
                    Label::Glyph(g) => (g, GLYPH, 100.0),
                    Label::Text { text, large, wrap } => (text, large as u8, wrap),
                };
                let Some((layout, m)) = self.layout(text, format, wrap) else { continue };
                // The layout is centred in its box: place the text's own middle on the element's.
                let origin = |dy: f32| Vector2 {
                    X: e.centre.0 - m.width / 2.0 - m.left,
                    Y: e.centre.1 - m.height / 2.0 - m.top + dy,
                };
                // A faint shadow keeps the light text readable over a light picture.
                self.brush.SetColor(&color(0.0, 0.0, 0.0, 0.4 * e.label_alpha));
                dc.DrawTextLayout(origin(1.0), &layout, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
                self.brush.SetColor(&color(0.96, 0.97, 0.98, e.label_alpha));
                dc.DrawTextLayout(origin(0.0), &layout, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
            dc.EndDraw(None, None)
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.waitable);
        }
    }
}

/// Largest rectangle with the picture's aspect ratio, centred in `window`, in whole pixels:
/// x, y, w, h.
pub fn letterbox(picture: (u32, u32), window: (u32, u32)) -> [f32; 4] {
    let (pw, ph) = (picture.0.max(1) as f32, picture.1.max(1) as f32);
    let scale = (window.0 as f32 / pw).min(window.1 as f32 / ph);
    let (w, h) = ((pw * scale).round(), (ph * scale).round());
    let (x, y) = (((window.0 as f32 - w) / 2.0).floor(), ((window.1 as f32 - h) / 2.0).floor());
    [x, y, w, h]
}

#[cfg(test)]
mod tests {
    use super::letterbox;

    #[test]
    fn letterbox_fills_one_side_in_whole_pixels() {
        assert_eq!(letterbox((1920, 1080), (1280, 720)), [0.0, 0.0, 1280.0, 720.0]);
        assert_eq!(letterbox((1920, 1080), (1000, 1000)), [0.0, 218.0, 1000.0, 563.0]);
        assert_eq!(letterbox((1080, 1920), (1600, 900)), [547.0, 0.0, 506.0, 900.0]);
    }
}
