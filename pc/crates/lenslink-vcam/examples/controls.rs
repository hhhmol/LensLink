//! Lists and changes the LensLink camera's controls through the standard kernel-streaming properties,
//! the way camera settings dialogs do.
//!
//! `cargo run -p lenslink-vcam --example controls -- [name=value[:auto]] ...`
//! e.g. `zoom=20`, `exposure=-7`, `gain=800`, `white_balance=0:auto`.

use core::ffi::c_void;

use lenslink_vcam::controls::{Control, FLAG_AUTO, FLAG_MANUAL};
use windows::core::{Interface, PWSTR, GUID};
use windows::Win32::Media::KernelStreaming::{IKsControl, KSIDENTIFIER};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};

#[repr(C)]
#[derive(Clone, Copy)]
struct KsProperty {
    set: GUID,
    id: u32,
    flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ValueS {
    property: KsProperty,
    value: i32,
    flags: u32,
    capabilities: u32,
}

const GET: u32 = 1;
const SET: u32 = 2;
const BASICSUPPORT: u32 = 0x200;

fn main() -> windows::core::Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)?;
        let Some(source) = open_camera()? else {
            println!("no LensLink camera: is `lenslink-host stream --vcam` running?");
            return Ok(());
        };
        let ks: IKsControl = source.cast()?;

        for arg in std::env::args().skip(1) {
            let (name, rest) = arg.split_once('=').expect("name=value[:auto]");
            let control = Control::from_name(name).expect("unknown control");
            let (value, auto) = rest.strip_suffix(":auto").map_or((rest, false), |v| (v, true));
            let (set, id) = control.ks();
            let mut s = ValueS {
                property: KsProperty { set, id, flags: SET },
                value: value.parse().expect("number"),
                flags: if auto { FLAG_AUTO } else { FLAG_MANUAL },
                capabilities: 0,
            };
            let mut n = 0;
            let p = &s.property as *const KsProperty as *const KSIDENTIFIER;
            let r = ks.KsProperty(p, size_of::<ValueS>() as u32, &mut s as *mut ValueS as *mut c_void, size_of::<ValueS>() as u32, &mut n);
            println!("set {arg}: {r:?}");
        }

        for control in Control::ALL {
            let (set, id) = control.ks();
            let mut range = [0u8; 72];
            let mut n = 0;
            let p = KsProperty { set, id, flags: BASICSUPPORT };
            let r = ks.KsProperty(&p as *const _ as *const KSIDENTIFIER, size_of::<KsProperty>() as u32, range.as_mut_ptr() as *mut c_void, 72, &mut n);
            if r.is_err() {
                println!("{:<20} not offered", control.name());
                continue;
            }
            let i32_at = |o: usize| i32::from_le_bytes(range[o..o + 4].try_into().unwrap());
            let (step, min, max) = (i32_at(56), i32_at(64), i32_at(68));

            let mut s = ValueS { property: KsProperty { set, id, flags: GET }, value: 0, flags: 0, capabilities: 0 };
            let p = s.property;
            ks.KsProperty(&p as *const _ as *const KSIDENTIFIER, size_of::<ValueS>() as u32, &mut s as *mut ValueS as *mut c_void, size_of::<ValueS>() as u32, &mut n)?;
            let mode = if s.flags & FLAG_AUTO != 0 { "auto" } else { "manual" };
            let caps = match s.capabilities {
                3 => "auto+manual",
                1 => "auto",
                _ => "manual",
            };
            println!("{:<20} {:>6} {:<6}  range {min}..{max} step {step}, {caps}", control.name(), s.value, mode);
        }
        source.Shutdown()?;
    }
    Ok(())
}

unsafe fn open_camera() -> windows::core::Result<Option<IMFMediaSource>> {
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.unwrap();
        attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)?;
        let mut list = std::ptr::null_mut();
        let mut count = 0;
        MFEnumDeviceSources(&attrs, &mut list, &mut count)?;
        let mut found = None;
        for i in 0..count as usize {
            let a = (*list.add(i)).take().unwrap();
            let mut name = PWSTR::null();
            let mut len = 0;
            a.GetAllocatedString(&MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, &mut name, &mut len)?;
            // Windows appends a localized "(Windows Virtual Camera)".
            if name.to_string().unwrap_or_default().starts_with(lenslink_vcam::FRIENDLY_NAME) {
                found = Some(a);
            }
            CoTaskMemFree(Some(name.0 as _));
        }
        CoTaskMemFree(Some(list as _));
        found.map(|a| a.ActivateObject()).transpose()
    }
}
