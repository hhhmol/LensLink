//! The app icon, drawn in code until the installer brings a designed one: a lens ring with a glint.

use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

/// A `size`×`size` icon with alpha.
pub fn lens(size: i32) -> windows::core::Result<HICON> {
    let s = size as f32;
    let mut pixels = vec![0u32; (size * size) as usize];
    let c = s / 2.0;
    let outer = s * 0.46;
    let ring = s * 0.13;
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5 - c, y as f32 + 0.5 - c);
            let r = (px * px + py * py).sqrt();
            // Antialiased ring.
            let a_out = (outer - r + 0.5).clamp(0.0, 1.0);
            let a_in = (r - (outer - ring) + 0.5).clamp(0.0, 1.0);
            let mut a = a_out * a_in;
            let mut v = 0.93;
            // Glass inside the ring: faint, with a glint at the upper left.
            let inside = (outer - ring - r + 0.5).clamp(0.0, 1.0);
            if inside > 0.0 {
                let (gx, gy) = (px + s * 0.12, py + s * 0.12);
                let glint = (1.0 - (gx * gx + gy * gy).sqrt() / (s * 0.12)).clamp(0.0, 1.0);
                let glass = 0.28 * inside + 0.7 * glint;
                if glass > a {
                    a = glass;
                    v = 0.85 + 0.15 * glint;
                }
            }
            let alpha = (a * 255.0).round() as u32;
            let g = (v * a * 255.0).round() as u32; // premultiplied
            pixels[(y * size + x) as usize] = (alpha << 24) | (g << 16) | (g << 8) | g;
        }
    }
    unsafe {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u32, pixels.len());
        let mask = CreateBitmap(size, size, 1, 1, None);
        let icon = CreateIconIndirect(&ICONINFO { fIcon: true.into(), hbmMask: mask, hbmColor: color, ..Default::default() });
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}
