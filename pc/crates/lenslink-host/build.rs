//! Compiles the window's shaders (src/ui/*.hlsl) with the D3D compiler that ships with Windows, so a
//! shader error is a build error and the app loads ready bytecode.

use std::path::PathBuf;

use windows::core::{s, PCSTR};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCompile, D3DCOMPILE_ENABLE_STRICTNESS, D3DCOMPILE_OPTIMIZATION_LEVEL3};
use windows::Win32::Graphics::Direct3D::ID3DBlob;

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let source = "src/ui/glass.hlsl";
    println!("cargo:rerun-if-changed={source}");
    let text = std::fs::read(source).unwrap();
    compile(&text, s!("vs"), s!("vs_5_0"), &out.join("glass_vs.cso"));
    compile(&text, s!("ps"), s!("ps_5_0"), &out.join("glass_ps.cso"));
    compile(&text, s!("ps_down"), s!("ps_5_0"), &out.join("glass_down.cso"));
    compile(&text, s!("ps_blur"), s!("ps_5_0"), &out.join("glass_blur.cso"));
}

fn compile(text: &[u8], entry: PCSTR, target: PCSTR, to: &std::path::Path) {
    unsafe {
        let mut code: Option<ID3DBlob> = None;
        let mut errors: Option<ID3DBlob> = None;
        let r = D3DCompile(
            text.as_ptr().cast(),
            text.len(),
            s!("glass.hlsl"),
            None,
            None,
            entry,
            target,
            D3DCOMPILE_ENABLE_STRICTNESS | D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            Some(&mut errors),
        );
        let message = |b: &ID3DBlob| String::from_utf8_lossy(std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize())).into_owned();
        if let Some(e) = &errors {
            let m = message(e);
            for line in m.lines().filter(|l| !l.trim().is_empty()) {
                println!("cargo:warning={line}");
            }
        }
        if let Err(e) = r {
            panic!("{} {}: {e}", entry.display(), target.display());
        }
        let code = code.unwrap();
        std::fs::write(to, std::slice::from_raw_parts(code.GetBufferPointer() as *const u8, code.GetBufferSize())).unwrap();
    }
}
