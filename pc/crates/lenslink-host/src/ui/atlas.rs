//! Drawing settings items: names, sliders, choices and values, as in the prototype: light text with
//! a soft shadow (graphite) or dark text with a glow (frost), sliders as a track and a thumb,
//! choices as a row of pills.
//!
//! The curtains' panels are drawn at rest into one texture, the atlas, which the glass shader bends
//! with each curtain's edge. Slots: left panel at (0, 0), right at (panel width, 0), top at
//! (0, window height), bottom below it. Floating widgets are drawn the same way straight onto the
//! window, over their glass.

use windows::core::{Interface, Result};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectWrite::{IDWriteTextLayout, DWRITE_TEXT_METRICS};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::IDXGISurface;
use windows_numerics::{Matrix3x2, Vector2};

use super::curtain::{Side, DEPTH_END, DEPTH_SIDE, SIDES};
use super::glass::render_texture;
use super::panel::{self, Item, Key, Values};

/// A text's layout and metrics in one of the panel fonts.
pub type Layouts<'a> = &'a dyn Fn(&str, panel::Font) -> Option<(IDWriteTextLayout, DWRITE_TEXT_METRICS)>;

fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

/// Items drawn from `origin`, clipped to `clip` (a size from the origin) if given.
pub struct Group<'a> {
    pub origin: (f32, f32),
    pub clip: Option<(f32, f32)>,
    pub items: &'a [Item],
}

/// Draws items onto a target: the text goes through its own bitmap first, to cast its shadow.
pub struct Painter {
    text: ID2D1Bitmap1,
    shadow: ID2D1Effect,
    style: ID2D1StrokeStyle,
    pixels: (u32, u32),
    dpi: f32,
}

impl Painter {
    pub unsafe fn new(dc: &ID2D1DeviceContext, pixels: (u32, u32), dpi: f32) -> Result<Self> {
        unsafe {
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: dpi,
                dpiY: dpi,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                ..Default::default()
            };
            let text = dc.CreateBitmap(D2D_SIZE_U { width: pixels.0.max(1), height: pixels.1.max(1) }, None, 0, &props)?;
            let shadow = dc.CreateEffect(&CLSID_D2D1Shadow)?;
            shadow.SetInput(0, &text, true);
            let style = dc.GetFactory()?.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES { startCap: D2D1_CAP_STYLE_ROUND, endCap: D2D1_CAP_STYLE_ROUND, ..Default::default() },
                None,
            )?;
            Ok(Painter { text, shadow, style, pixels, dpi })
        }
    }

    pub fn fits(&self, pixels: (u32, u32), dpi: f32) -> bool {
        self.pixels == pixels && self.dpi == dpi
    }

    /// Draws `groups` onto `target`, cleared first if `clear`. Leaves `dc` without a target.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn paint(
        &self,
        dc: &ID2D1DeviceContext,
        brush: &ID2D1SolidColorBrush,
        layouts: Layouts,
        groups: &[Group],
        values: &Values,
        target: &ID2D1Image,
        clear: bool,
    ) -> Result<()> {
        unsafe {
            let frost = values.frost_theme();
            let ink = if frost { color(0.063, 0.078, 0.09, 1.0) } else { color(0.961, 0.969, 0.976, 1.0) };
            let enter = |g: &Group| {
                dc.SetTransform(&Matrix3x2::translation(g.origin.0, g.origin.1));
                if let Some((w, h)) = g.clip {
                    dc.PushAxisAlignedClip(&D2D_RECT_F { left: 0.0, top: 0.0, right: w, bottom: h }, D2D1_ANTIALIAS_MODE_ALIASED);
                }
            };
            let leave = |g: &Group| {
                if g.clip.is_some() {
                    dc.PopAxisAlignedClip();
                }
            };
            let text = |t: &str, font, x: f32, cy: f32, align: f32, alpha: f32| {
                let Some((layout, m)) = layouts(t, font) else { return };
                // align: 0 left of x, 0.5 centred on x, 1 right of x.
                let origin = Vector2 { X: x - m.width * align - m.left, Y: cy - m.height / 2.0 - m.top };
                brush.SetColor(&D2D1_COLOR_F { a: alpha, ..ink });
                dc.DrawTextLayout(origin, &layout, brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
            };

            // The text, on its own.
            dc.SetTarget(&self.text);
            dc.BeginDraw();
            dc.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));
            for g in groups {
                enter(g);
                for item in g.items {
                    match item {
                        Item::Section { text: t, rect } => text(t, panel::Font::Section, rect.x, rect.centre().1, 0.0, 0.6),
                        Item::Label { key, rect } => text(panel::row(*key).label, panel::Font::Text, rect.x, rect.centre().1, 0.0, 1.0),
                        Item::Output { key, rect } => text(&values.text(*key), panel::Font::Text, rect.right(), rect.centre().1, 1.0, 0.8),
                        Item::Choice { key, options, .. } => {
                            for ((name, _), r) in values.options(*key).iter().zip(options) {
                                let (cx, cy) = r.centre();
                                text(name, panel::Font::Text, cx, cy, 0.5, 1.0);
                            }
                        }
                        Item::Info { key, rect } => text(values.info(*key), panel::Font::Strong, rect.right(), rect.centre().1, 1.0, 1.0),
                        Item::Slider { .. } => {}
                    }
                }
                leave(g);
            }
            dc.SetTransform(&Matrix3x2::identity());
            dc.EndDraw(None, None)?;

            // Pills and sliders, then the text with its shadow over them.
            dc.SetTarget(target);
            dc.BeginDraw();
            if clear {
                dc.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));
            }
            for g in groups {
                enter(g);
                for item in g.items {
                    match item {
                        Item::Slider { key, rect } => {
                            let f = panel::slider_fraction(*key, values);
                            let (_, cy) = rect.centre();
                            let (x0, x1) = (rect.x + 7.0, rect.right() - 7.0);
                            let tx = x0 + f * (x1 - x0);
                            brush.SetColor(&color(0.5, 0.5, 0.5, 0.5));
                            dc.DrawLine(Vector2 { X: x0, Y: cy }, Vector2 { X: x1, Y: cy }, brush, 3.0, &self.style);
                            // Left to the camera, the track stays unfilled and the thumb faint.
                            let auto = values.auto(*key);
                            if !auto {
                                brush.SetColor(&ink);
                                dc.DrawLine(Vector2 { X: x0, Y: cy }, Vector2 { X: tx, Y: cy }, brush, 3.0, &self.style);
                            }
                            // The thumb's soft shadow, then the thumb.
                            for (r, a) in [(8.5, 0.08), (7.8, 0.12), (7.2, 0.15)] {
                                brush.SetColor(&color(0.0, 0.0, 0.0, a));
                                dc.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: tx, Y: cy + 1.0 }, radiusX: r, radiusY: r }, brush);
                            }
                            brush.SetColor(&D2D1_COLOR_F { a: if auto { 0.55 } else { 1.0 }, ..ink });
                            dc.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: tx, Y: cy }, radiusX: 7.0, radiusY: 7.0 }, brush);
                        }
                        Item::Choice { key, rect, options } => {
                            let round = |r: &panel::Rect, radius: f32| D2D1_ROUNDED_RECT {
                                rect: D2D_RECT_F { left: r.x, top: r.y, right: r.right(), bottom: r.bottom() },
                                radiusX: radius,
                                radiusY: radius,
                            };
                            brush.SetColor(&color(0.5, 0.5, 0.5, 0.2));
                            dc.FillRoundedRectangle(&round(rect, 8.0), brush);
                            if let Some(on) = values.chosen(*key).and_then(|i| options.get(i)) {
                                brush.SetColor(&if frost { color(0.0, 0.0, 0.0, 0.12) } else { color(1.0, 1.0, 1.0, 0.22) });
                                dc.FillRoundedRectangle(&round(on, 6.0), brush);
                            }
                        }
                        Item::Info { key: Key::Phone, rect } if !values.info(Key::Phone).is_empty() => {
                            // A green light before the phone's name: connected.
                            let width = layouts(values.info(Key::Phone), panel::Font::Strong).map_or(0.0, |(_, m)| m.width);
                            let at = Vector2 { X: rect.right() - width - 10.0, Y: rect.centre().1 };
                            for (r, a) in [(9.0, 0.06), (6.5, 0.12), (5.0, 0.2)] {
                                brush.SetColor(&color(0.204, 0.827, 0.6, a));
                                dc.FillEllipse(&D2D1_ELLIPSE { point: at, radiusX: r, radiusY: r }, brush);
                            }
                            brush.SetColor(&color(0.204, 0.827, 0.6, 1.0));
                            dc.FillEllipse(&D2D1_ELLIPSE { point: at, radiusX: 3.5, radiusY: 3.5 }, brush);
                        }
                        _ => {}
                    }
                }
                leave(g);
            }
            dc.SetTransform(&Matrix3x2::identity());
            // Graphite: a dark shadow just below; frost: a white glow around.
            let (sigma, shade, offset) = if frost { (3.0f32, [1.0f32, 1.0, 1.0, 0.55], 0.0) } else { (1.0, [0.0, 0.0, 0.0, 0.45], 1.0) };
            self.shadow.SetValue(D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION.0 as u32, D2D1_PROPERTY_TYPE_FLOAT, &sigma.to_ne_bytes())?;
            let bytes: Vec<u8> = shade.iter().flat_map(|v| v.to_ne_bytes()).collect();
            self.shadow.SetValue(D2D1_SHADOW_PROP_COLOR.0 as u32, D2D1_PROPERTY_TYPE_VECTOR4, &bytes)?;
            let out = self.shadow.GetOutput()?;
            dc.DrawImage(&out, Some(&Vector2 { X: 0.0, Y: offset }), None, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_COMPOSITE_MODE_SOURCE_OVER);
            dc.DrawImage(&self.text, None, None, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, D2D1_COMPOSITE_MODE_SOURCE_OVER);
            let r = dc.EndDraw(None, None);
            dc.SetTarget(None::<&ID2D1Image>);
            r
        }
    }
}

pub struct Atlas {
    /// The window size and DPI it was made for.
    window: (f32, f32),
    dpi: f32,
    /// Size in pixels.
    pub pixels: (u32, u32),
    pub view: ID3D11ShaderResourceView,
    bitmap: ID2D1Bitmap1,
    painter: Painter,
}

/// Where `side`'s panel starts in the atlas, in DIPs.
pub fn slot(side: Side, window: (f32, f32)) -> (f32, f32) {
    match side {
        Side::Left => (0.0, 0.0),
        Side::Right => (DEPTH_SIDE, 0.0),
        Side::Top => (0.0, window.1),
        Side::Bottom => (0.0, window.1 + DEPTH_END),
    }
}

impl Atlas {
    pub unsafe fn new(device: &ID3D11Device, dc: &ID2D1DeviceContext, window: (f32, f32), dpi: f32) -> Result<Self> {
        unsafe {
            let px = dpi / 96.0;
            let dips = ((2.0 * DEPTH_SIDE).max(window.0), window.1 + 2.0 * DEPTH_END);
            let pixels = ((dips.0 * px).round() as u32, (dips.1 * px).round() as u32);
            let texture = render_texture(device, pixels)?;
            let mut view = None;
            device.CreateShaderResourceView(&texture, None, Some(&mut view))?;
            let format = D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED };
            let props = D2D1_BITMAP_PROPERTIES1 { pixelFormat: format, dpiX: dpi, dpiY: dpi, bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET, ..Default::default() };
            let surface: IDXGISurface = texture.cast()?;
            let bitmap = dc.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            Ok(Atlas { window, dpi, pixels, view: view.unwrap(), bitmap, painter: Painter::new(dc, pixels, dpi)? })
        }
    }

    pub fn fits(&self, window: (f32, f32), dpi: f32) -> bool {
        self.window == window && self.dpi == dpi
    }

    /// Draws every panel. Leaves `dc` without a target.
    pub unsafe fn draw(&self, dc: &ID2D1DeviceContext, brush: &ID2D1SolidColorBrush, layouts: Layouts, panels: &[Vec<Item>; 4], values: &Values) -> Result<()> {
        let groups: Vec<Group> = SIDES
            .iter()
            .map(|&side| Group {
                origin: slot(side, self.window),
                clip: Some(panel::panel_size(side, self.window)),
                items: &panels[side.index()],
            })
            .collect();
        unsafe { self.painter.paint(dc, brush, layouts, &groups, values, &self.bitmap, true) }
    }
}
