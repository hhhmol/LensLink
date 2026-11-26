//! The glass pass: one triangle over the whole window with the pixel shader from `glass.hlsl`, which
//! draws the video and refracts it through the floating elements and the curtains. While a curtain
//! shows, three small passes first make its frost: the picture at a quarter size, blurred across
//! and down.

use windows::core::{Interface, Result};
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

pub const MAX_ELEMENTS: usize = 12;
pub const EDGE_POINTS: usize = 52;

/// The shader's constant buffer, laid out as `cbuffer Scene` in `glass.hlsl`. Lengths in pixels.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Constants {
    pub size: [f32; 2],
    /// Pixels per DIP.
    pub px: f32,
    pub lens: f32,
    pub dispersion: f32,
    /// 0 graphite, 1 frost.
    pub theme: f32,
    pub curtain_tint: f32,
    pub count: f32,
    /// x, y, w, h; w = 0 when there is no video.
    pub video: [f32; 4],
    pub background: [f32; 4],
    pub edge_max: [f32; 4],
    /// Each curtain's mean depth: where its panel sits.
    pub edge_mean: [f32; 4],
    /// Panel depth (left/right), panel depth (top/bottom), bend falloff, frost 0..1.
    pub panel: [f32; 4],
    /// Atlas size; the frost texture's size (filled in by [`Glass::draw`]).
    pub atlas: [f32; 4],
    /// Centre and half size.
    pub boxes: [[f32; 4]; MAX_ELEMENTS],
    /// Corner radius and bevel cap.
    pub corners: [[f32; 4]; MAX_ELEMENTS],
    /// rgb and amount.
    pub tints: [[f32; 4]; MAX_ELEMENTS],
    /// Curtain edges, one side per component (left, right, top, bottom).
    pub edges: [[f32; 4]; EDGE_POINTS],
}

impl Default for Constants {
    fn default() -> Self {
        Constants {
            size: [0.0; 2],
            px: 1.0,
            lens: 1.3,
            dispersion: 1.2,
            theme: 0.0,
            curtain_tint: 0.55,
            count: 0.0,
            video: [0.0; 4],
            background: [0.0; 4],
            edge_max: [0.0; 4],
            edge_mean: [0.0; 4],
            panel: [0.0; 4],
            atlas: [1.0; 4],
            boxes: [[0.0; 4]; MAX_ELEMENTS],
            corners: [[0.0; 4]; MAX_ELEMENTS],
            tints: [[0.0; 4]; MAX_ELEMENTS],
            edges: [[0.0; 4]; EDGE_POINTS],
        }
    }
}

/// `cbuffer Blur`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BlurConstants {
    dir: [f32; 2],
    sigma: f32,
    pad: f32,
}

const VS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/glass_vs.cso"));
const PS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/glass_ps.cso"));
const PS_DOWN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/glass_down.cso"));
const PS_BLUR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/glass_blur.cso"));

/// A texture to draw into and read from.
struct Target {
    view: ID3D11RenderTargetView,
    srv: ID3D11ShaderResourceView,
}

/// The frost's two quarter-size textures: the result ends in `a`.
struct Frost {
    size: (u32, u32),
    a: Target,
    b: Target,
}

pub struct Glass {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    down: ID3D11PixelShader,
    blur: ID3D11PixelShader,
    constants: ID3D11Buffer,
    blur_constants: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    /// Shader view of the video texture, kept while the texture stays the same.
    video: Option<(usize, ID3D11ShaderResourceView)>,
    frost: Option<Frost>,
}

unsafe fn buffer(device: &ID3D11Device, size: usize) -> Result<ID3D11Buffer> {
    unsafe {
        let desc = D3D11_BUFFER_DESC {
            ByteWidth: size as u32,
            Usage: D3D11_USAGE_DYNAMIC,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            ..Default::default()
        };
        let mut b = None;
        device.CreateBuffer(&desc, None, Some(&mut b))?;
        Ok(b.unwrap())
    }
}

unsafe fn upload<T: Copy>(ctx: &ID3D11DeviceContext, buffer: &ID3D11Buffer, value: &T) -> Result<()> {
    unsafe {
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(buffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))?;
        std::ptr::copy_nonoverlapping(value as *const T, mapped.pData as *mut T, 1);
        ctx.Unmap(buffer, 0);
        Ok(())
    }
}

/// A BGRA texture the GPU draws into and the shaders read, `size` pixels.
pub unsafe fn render_texture(device: &ID3D11Device, size: (u32, u32)) -> Result<ID3D11Texture2D> {
    unsafe {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: size.0.max(1),
            Height: size.1.max(1),
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut t = None;
        device.CreateTexture2D(&desc, None, Some(&mut t))?;
        Ok(t.unwrap())
    }
}

unsafe fn frost_target(device: &ID3D11Device, size: (u32, u32)) -> Result<Target> {
    unsafe {
        let t = render_texture(device, size)?;
        let mut view = None;
        device.CreateRenderTargetView(&t, None, Some(&mut view))?;
        let mut srv = None;
        device.CreateShaderResourceView(&t, None, Some(&mut srv))?;
        Ok(Target { view: view.unwrap(), srv: srv.unwrap() })
    }
}

impl Glass {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        unsafe {
            let mut vs = None;
            device.CreateVertexShader(VS, None, Some(&mut vs))?;
            let pixel = |code: &[u8]| -> Result<ID3D11PixelShader> {
                let mut ps = None;
                device.CreatePixelShader(code, None, Some(&mut ps))?;
                Ok(ps.unwrap())
            };
            let sampler_desc = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            let mut sampler = None;
            device.CreateSamplerState(&sampler_desc, Some(&mut sampler))?;
            Ok(Glass {
                device: device.clone(),
                context: device.GetImmediateContext()?,
                vs: vs.unwrap(),
                ps: pixel(PS)?,
                down: pixel(PS_DOWN)?,
                blur: pixel(PS_BLUR)?,
                constants: buffer(device, size_of::<Constants>())?,
                blur_constants: buffer(device, size_of::<BlurConstants>())?,
                sampler: sampler.unwrap(),
                video: None,
                frost: None,
            })
        }
    }

    /// Draws into `target` (`c.size` pixels). `atlas` holds the curtains' panels. The caller holds
    /// the device lock shared with the streaming thread.
    pub unsafe fn draw(
        &mut self,
        target: &ID3D11RenderTargetView,
        video: Option<&ID3D11Texture2D>,
        atlas: Option<&ID3D11ShaderResourceView>,
        c: &Constants,
    ) -> Result<()> {
        unsafe {
            let view = match video {
                Some(t) => Some(self.video_view(t)?),
                None => None,
            };
            let frosted = c.panel[3] > 0.0 && c.edge_max.iter().any(|&e| e > 0.0);
            let mut c = *c;
            if frosted {
                let q = (((c.size[0] / 4.0).ceil() as u32).max(1), ((c.size[1] / 4.0).ceil() as u32).max(1));
                if self.frost.as_ref().is_none_or(|f| f.size != q) {
                    self.frost = Some(Frost { size: q, a: frost_target(&self.device, q)?, b: frost_target(&self.device, q)? });
                }
                c.atlas[2] = q.0 as f32;
                c.atlas[3] = q.1 as f32;
            }
            let ctx = &self.context;
            upload(ctx, &self.constants, &c)?;
            ctx.IASetInputLayout(None);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.VSSetShader(&self.vs, None);
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.constants.clone()), Some(self.blur_constants.clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            ctx.RSSetState(None);
            ctx.OMSetBlendState(None, None, u32::MAX);
            ctx.OMSetDepthStencilState(None, 0);
            let viewport = |w: f32, h: f32| ctx.RSSetViewports(Some(&[D3D11_VIEWPORT { Width: w, Height: h, MaxDepth: 1.0, ..Default::default() }]));

            if let (true, Some(f)) = (frosted, self.frost.as_ref()) {
                let (qw, qh) = (f.size.0 as f32, f.size.1 as f32);
                viewport(qw, qh);
                ctx.PSSetShaderResources(0, Some(&[view.clone(), None, None]));
                ctx.OMSetRenderTargets(Some(&[Some(f.a.view.clone())]), None);
                ctx.PSSetShader(&self.down, None);
                ctx.Draw(3, 0);
                // The prototype's CSS blur of `frost × 30 px`, on a quarter-size picture.
                let sigma = c.panel[3] * 30.0 * c.px / 4.0;
                ctx.PSSetShader(&self.blur, None);
                for (from, to, dir) in [(&f.a, &f.b, [1.0 / qw, 0.0]), (&f.b, &f.a, [0.0, 1.0 / qh])] {
                    upload(ctx, &self.blur_constants, &BlurConstants { dir, sigma, pad: 0.0 })?;
                    ctx.PSSetShaderResources(2, Some(&[None]));
                    ctx.OMSetRenderTargets(Some(&[Some(to.view.clone())]), None);
                    ctx.PSSetShaderResources(2, Some(&[Some(from.srv.clone())]));
                    ctx.Draw(3, 0);
                }
                ctx.PSSetShaderResources(2, Some(&[None]));
                ctx.OMSetRenderTargets(None, None);
            }

            viewport(c.size[0], c.size[1]);
            ctx.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
            let frost = if frosted { self.frost.as_ref().map(|f| f.a.srv.clone()) } else { None };
            ctx.PSSetShaderResources(0, Some(&[view, atlas.cloned(), frost]));
            ctx.PSSetShader(&self.ps, None);
            ctx.Draw(3, 0);
            // Let go of the video: the streaming thread writes the next frame into it.
            ctx.PSSetShaderResources(0, Some(&[None, None, None]));
            ctx.OMSetRenderTargets(None, None);
            Ok(())
        }
    }

    unsafe fn video_view(&mut self, texture: &ID3D11Texture2D) -> Result<ID3D11ShaderResourceView> {
        unsafe {
            let key = texture.as_raw() as usize;
            if let Some((k, v)) = &self.video {
                if *k == key {
                    return Ok(v.clone());
                }
            }
            let mut view = None;
            texture.GetDevice()?.CreateShaderResourceView(texture, None, Some(&mut view))?;
            let view = view.unwrap();
            self.video = Some((key, view.clone()));
            Ok(view)
        }
    }
}

/// Draws a scene exported from the prototype with this shader, to compare the two pixel by pixel.
/// The folder `LENSLINK_GLASS_SCENE` holds `scene.json` (the prototype's uniforms), its background
/// `bg.bgra` and, with curtains, its atlas `atlas.bgra` (raw BGRA, premultiplied); the result goes to
/// `native.bgra`. Needs a GPU: `cargo test glass_draws_the_prototype_scene -- --ignored`.
#[cfg(test)]
mod compare {
    use super::*;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};

    fn texture(device: &ID3D11Device, w: u32, h: u32, usage: D3D11_USAGE, bind: u32, cpu: u32, data: Option<&[u8]>) -> ID3D11Texture2D {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: usage,
            BindFlags: bind,
            CPUAccessFlags: cpu,
            MiscFlags: 0,
        };
        let init = data.map(|d| D3D11_SUBRESOURCE_DATA { pSysMem: d.as_ptr().cast(), SysMemPitch: w * 4, SysMemSlicePitch: 0 });
        let mut t = None;
        unsafe { device.CreateTexture2D(&desc, init.as_ref().map(|i| i as *const _), Some(&mut t)) }.unwrap();
        t.unwrap()
    }

    #[test]
    #[ignore]
    fn glass_draws_the_prototype_scene() {
        let dir = std::path::PathBuf::from(std::env::var("LENSLINK_GLASS_SCENE").expect("LENSLINK_GLASS_SCENE"));
        let scene: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("scene.json")).unwrap()).unwrap();
        let f = |v: &serde_json::Value| v.as_f64().unwrap_or(0.0) as f32;
        let four = |v: &serde_json::Value| -> [f32; 4] { std::array::from_fn(|k| f(&v[k])) };
        let (w, h) = (scene["R"][0].as_u64().unwrap() as u32, scene["R"][1].as_u64().unwrap() as u32);
        let bg = std::fs::read(dir.join("bg.bgra")).unwrap();
        assert_eq!(bg.len(), (w * h * 4) as usize);

        let mut device = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
        }
        .unwrap();
        let device = device.unwrap();
        let video = texture(&device, w, h, D3D11_USAGE_IMMUTABLE, D3D11_BIND_SHADER_RESOURCE.0 as u32, 0, Some(&bg));
        let target = texture(&device, w, h, D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET.0 as u32, 0, None);
        let staging = texture(&device, w, h, D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0 as u32, None);
        let mut view = None;
        unsafe { device.CreateRenderTargetView(&target, None, Some(&mut view)) }.unwrap();

        let n = scene["N"].as_u64().unwrap() as usize;
        let mut c = Constants {
            size: [w as f32, h as f32],
            px: f(&scene["PX"]),
            lens: f(&scene["S"]),
            dispersion: f(&scene["D"]),
            theme: f(&scene["TH"]),
            curtain_tint: f(&scene["TI"]),
            count: n as f32,
            video: [0.0, 0.0, w as f32, h as f32],
            ..Default::default()
        };
        for i in 0..n {
            c.boxes[i] = std::array::from_fn(|k| f(&scene["B"][i * 4 + k]));
            c.corners[i] = [f(&scene["Rd"][i]), f(&scene["Cp"][i]), 0.0, 0.0];
        }
        let mut atlas_view = None;
        if scene["E"].is_array() {
            for i in 0..EDGE_POINTS {
                c.edges[i] = std::array::from_fn(|k| f(&scene["E"][i * 4 + k]));
            }
            c.edge_max = four(&scene["EM"]);
            c.edge_mean = four(&scene["MN"]);
            c.panel = [f(&scene["DW"]), f(&scene["DH"]), f(&scene["LAM"]), f(&scene["BL"])];
            let (aw, ah) = (scene["UA"][0].as_u64().unwrap() as u32, scene["UA"][1].as_u64().unwrap() as u32);
            c.atlas = [aw as f32, ah as f32, 1.0, 1.0];
            let pixels = std::fs::read(dir.join("atlas.bgra")).unwrap();
            let atlas = texture(&device, aw, ah, D3D11_USAGE_IMMUTABLE, D3D11_BIND_SHADER_RESOURCE.0 as u32, 0, Some(&pixels));
            unsafe { device.CreateShaderResourceView(&atlas, None, Some(&mut atlas_view)) }.unwrap();
        }
        let mut glass = Glass::new(&device).unwrap();
        unsafe {
            glass.draw(view.as_ref().unwrap(), Some(&video), atlas_view.as_ref(), &c).unwrap();
            let ctx = device.GetImmediateContext().unwrap();
            ctx.CopyResource(&staging, &target);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m)).unwrap();
            let mut out = Vec::with_capacity(bg.len());
            for y in 0..h as usize {
                let row = std::slice::from_raw_parts((m.pData as *const u8).add(y * m.RowPitch as usize), w as usize * 4);
                out.extend_from_slice(row);
            }
            ctx.Unmap(&staging, 0);
            std::fs::write(dir.join("native.bgra"), out).unwrap();
        }
    }
}
