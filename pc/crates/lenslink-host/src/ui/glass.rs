//! The glass pass: one triangle over the whole window with the pixel shader from `glass.hlsl`, which
//! draws the video and refracts it through the floating elements and the curtains.

use windows::core::{Interface, Result};
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;

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
            boxes: [[0.0; 4]; MAX_ELEMENTS],
            corners: [[0.0; 4]; MAX_ELEMENTS],
            tints: [[0.0; 4]; MAX_ELEMENTS],
            edges: [[0.0; 4]; EDGE_POINTS],
        }
    }
}

const VS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/glass_vs.cso"));
const PS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/glass_ps.cso"));

pub struct Glass {
    context: ID3D11DeviceContext,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    constants: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    /// Shader view of the video texture, kept while the texture stays the same.
    video: Option<(usize, ID3D11ShaderResourceView)>,
}

impl Glass {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        unsafe {
            let mut vs = None;
            device.CreateVertexShader(VS, None, Some(&mut vs))?;
            let mut ps = None;
            device.CreatePixelShader(PS, None, Some(&mut ps))?;
            let desc = D3D11_BUFFER_DESC {
                ByteWidth: size_of::<Constants>() as u32,
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                ..Default::default()
            };
            let mut constants = None;
            device.CreateBuffer(&desc, None, Some(&mut constants))?;
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
                context: device.GetImmediateContext()?,
                vs: vs.unwrap(),
                ps: ps.unwrap(),
                constants: constants.unwrap(),
                sampler: sampler.unwrap(),
                video: None,
            })
        }
    }

    /// Draws into `target` (`size` pixels). The caller holds the device lock shared with the streaming
    /// thread.
    pub unsafe fn draw(&mut self, target: &ID3D11RenderTargetView, video: Option<&ID3D11Texture2D>, c: &Constants) -> Result<()> {
        unsafe {
            let view = match video {
                Some(t) => Some(self.video_view(t)?),
                None => None,
            };
            let ctx = &self.context;
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&self.constants, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))?;
            std::ptr::copy_nonoverlapping(c as *const Constants, mapped.pData as *mut Constants, 1);
            ctx.Unmap(&self.constants, 0);

            ctx.IASetInputLayout(None);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.VSSetShader(&self.vs, None);
            ctx.PSSetShader(&self.ps, None);
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
            ctx.PSSetShaderResources(0, Some(&[view]));
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            ctx.RSSetState(None);
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT { Width: c.size[0], Height: c.size[1], MaxDepth: 1.0, ..Default::default() }]));
            ctx.OMSetBlendState(None, None, u32::MAX);
            ctx.OMSetDepthStencilState(None, 0);
            ctx.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
            ctx.Draw(3, 0);
            // Let go of the video: the streaming thread writes the next frame into it.
            ctx.PSSetShaderResources(0, Some(&[None]));
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
