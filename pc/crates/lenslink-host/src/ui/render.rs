//! Draws the window: the video, the status pill and the caption buttons, with Direct2D on a flip-model
//! swap chain. Glass comes in the next step; this is the plain foundation it will sit on.

use windows::core::{w, Interface, Result};
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows_numerics::Vector2;

/// Caption buttons, top right, in DIPs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Minimize,
    Close,
}

const BUTTON: f32 = 30.0;
const BUTTON_GAP: f32 = 8.0;
const MARGIN: f32 = 12.0;

pub fn button_rect(b: Button, width: f32) -> D2D_RECT_F {
    let right = width - MARGIN - if b == Button::Minimize { BUTTON + BUTTON_GAP } else { 0.0 };
    D2D_RECT_F { left: right - BUTTON, top: MARGIN, right, bottom: MARGIN + BUTTON }
}

pub fn button_at(x: f32, y: f32, width: f32) -> Option<Button> {
    [Button::Minimize, Button::Close].into_iter().find(|&b| {
        let r = button_rect(b, width);
        // A little slack around each button.
        x >= r.left - 3.0 && x <= r.right + 3.0 && y >= r.top - 3.0 && y <= r.bottom + 3.0
    })
}

/// What to draw.
pub struct Scene<'a> {
    pub video: Option<(&'a ID3D11Texture2D, (u32, u32))>,
    pub status: &'a str,
    /// Frames are arriving: the status is a statistics line, shown only on hover.
    pub live: bool,
    /// The pointer is over the window.
    pub hover: bool,
    pub hot: Option<Button>,
    pub pressed: Option<Button>,
}

pub struct Renderer {
    swapchain: IDXGISwapChain1,
    dc: ID2D1DeviceContext,
    target: Option<ID2D1Bitmap1>,
    /// Direct2D view of the video texture, kept while the texture stays the same.
    video: Option<(usize, ID2D1Bitmap1)>,
    dwrite: IDWriteFactory,
    small: IDWriteTextFormat,
    large: IDWriteTextFormat,
    glyphs: IDWriteTextFormat,
    brush: ID2D1SolidColorBrush,
    dpi: f32,
}

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
                ..Default::default()
            };
            let swapchain = dxgi_factory.CreateSwapChainForHwnd(device, hwnd, &desc, None, None)?;
            dxgi_factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)?;

            let dc = factory.CreateDevice(&dxgi)?.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            dc.SetDpi(dpi, dpi);
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
            Ok(Renderer { swapchain, dc, target: None, video: None, dwrite, small, large, glyphs, brush, dpi })
        }
    }

    pub fn set_dpi(&mut self, dpi: f32) {
        self.dpi = dpi;
        self.target = None;
        unsafe { self.dc.SetDpi(dpi, dpi) };
    }

    /// The window changed size: the swap chain follows on the next draw.
    pub fn resize(&mut self) -> Result<()> {
        unsafe {
            self.target = None;
            self.dc.SetTarget(None::<&ID2D1Image>);
            self.swapchain.ResizeBuffers(0, 0, 0, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))
        }
    }

    /// Window size in DIPs.
    pub fn size(&self) -> (f32, f32) {
        let desc = unsafe { self.swapchain.GetDesc1() }.unwrap_or_default();
        (desc.Width as f32 * 96.0 / self.dpi, desc.Height as f32 * 96.0 / self.dpi)
    }

    pub fn draw(&mut self, scene: &Scene) -> Result<()> {
        unsafe {
            let desc = self.swapchain.GetDesc1()?;
            if desc.Width == 0 || desc.Height == 0 {
                return Ok(());
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
            let (w, h) = self.size();
            let dc = self.dc.clone();
            dc.BeginDraw();
            dc.Clear(Some(&color(0.035, 0.04, 0.05, 1.0)));

            if let Some((texture, size)) = scene.video {
                let bitmap = self.video_bitmap(texture)?;
                let dest = letterbox(size, (w, h));
                dc.DrawBitmap(&bitmap, Some(&dest), 1.0, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None);
            }

            if !scene.live && !scene.status.is_empty() {
                // Nothing to watch: the status, large, in the middle.
                self.pill(scene.status, &self.large.clone(), (w / 2.0, h / 2.0), w - 64.0, true)?;
            } else if scene.hover && !scene.status.is_empty() {
                self.pill(scene.status, &self.small.clone(), (MARGIN, h - MARGIN), w - 2.0 * MARGIN, false)?;
            }

            if scene.hover {
                for (b, glyph) in [(Button::Minimize, "\u{E921}"), (Button::Close, "\u{E8BB}")] {
                    let r = button_rect(b, w);
                    let hot = scene.hot == Some(b);
                    let fill = match (b, hot, scene.pressed == Some(b)) {
                        (Button::Close, true, _) => color(0.86, 0.22, 0.2, 0.92),
                        (_, true, true) => color(1.0, 1.0, 1.0, 0.32),
                        (_, true, false) => color(1.0, 1.0, 1.0, 0.22),
                        _ => color(0.06, 0.07, 0.08, 0.55),
                    };
                    self.brush.SetColor(&fill);
                    dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: BUTTON / 2.0, radiusY: BUTTON / 2.0 }, &self.brush);
                    self.brush.SetColor(&color(1.0, 1.0, 1.0, 0.95));
                    let text: Vec<u16> = glyph.encode_utf16().collect();
                    dc.DrawText(&text, &self.glyphs, &r, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                }
            }

            let result = dc.EndDraw(None, None);
            if let Err(e) = &result {
                if e.code() == D2DERR_RECREATE_TARGET {
                    self.target = None;
                    self.video = None;
                }
            }
            result?;
            self.swapchain.Present(0, DXGI_PRESENT(0)).ok()
        }
    }

    /// A dark pill with `text`; `at` is its centre when `centred`, else its lower left corner.
    unsafe fn pill(&self, text: &str, format: &IDWriteTextFormat, at: (f32, f32), max_width: f32, centred: bool) -> Result<()> {
        unsafe {
            let text: Vec<u16> = text.encode_utf16().collect();
            let layout = self.dwrite.CreateTextLayout(&text, format, max_width.max(40.0) - 28.0, 400.0)?;
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m)?;
            let (pw, ph) = (m.width + 28.0, m.height + 14.0);
            let (x, y) = if centred { (at.0 - pw / 2.0, at.1 - ph / 2.0) } else { (at.0, at.1 - ph) };
            let rect = D2D_RECT_F { left: x, top: y, right: x + pw, bottom: y + ph };
            let radius = (ph / 2.0).min(18.0);
            self.brush.SetColor(&color(0.05, 0.06, 0.07, 0.72));
            self.dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect, radiusX: radius, radiusY: radius }, &self.brush);
            self.brush.SetColor(&color(0.96, 0.97, 0.98, 1.0));
            // The layout is as wide as the longest line; centred text is placed by its own left edge.
            self.dc.DrawTextLayout(Vector2 { X: x + 14.0 - m.left, Y: y + 7.0 }, &layout, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
            Ok(())
        }
    }

    unsafe fn video_bitmap(&mut self, texture: &ID3D11Texture2D) -> Result<ID2D1Bitmap1> {
        unsafe {
            let key = texture.as_raw() as usize;
            if let Some((k, b)) = &self.video {
                if *k == key {
                    return Ok(b.clone());
                }
            }
            let surface: IDXGISurface = texture.cast()?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            };
            let bitmap = self.dc.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            self.video = Some((key, bitmap.clone()));
            Ok(bitmap)
        }
    }
}

/// Largest rectangle with the picture's aspect ratio, centred in `window` (DIPs).
fn letterbox(picture: (u32, u32), window: (f32, f32)) -> D2D_RECT_F {
    let (pw, ph) = (picture.0.max(1) as f32, picture.1.max(1) as f32);
    let scale = (window.0 / pw).min(window.1 / ph);
    let (w, h) = (pw * scale, ph * scale);
    let (x, y) = ((window.0 - w) / 2.0, (window.1 - h) / 2.0);
    D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }
}
