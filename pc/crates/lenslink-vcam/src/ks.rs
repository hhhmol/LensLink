//! Kernel-streaming property requests (`IKsControl::KsProperty`) for the camera controls.
//!
//! Windows asks in four ways: BASICSUPPORT (range and step), DEFAULTVALUES, GET and SET. The
//! structures below are the C layouts from ks.h / ksmedia.h.

use std::sync::Mutex;

use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_NOT_FOUND, ERROR_SET_NOT_FOUND, E_INVALIDARG, E_POINTER};
use windows_core::{Result, GUID, HRESULT};

use crate::controls::{ControlDesc, FLAG_AUTO, FLAG_MANUAL, PROPSETID_VIDCAP_CAMERACONTROL, PROPSETID_VIDCAP_VIDEOPROCAMP};
use crate::log::vlog;
use crate::shm::SharedFrames;

const KSPROPERTY_TYPE_GET: u32 = 0x1;
const KSPROPERTY_TYPE_SET: u32 = 0x2;
const KSPROPERTY_TYPE_BASICSUPPORT: u32 = 0x200;
const KSPROPERTY_TYPE_DEFAULTVALUES: u32 = 0x10000;
const KSPROPERTY_MEMBER_STEPPEDRANGES: u32 = 2;
const KSPROPERTY_MEMBER_VALUES: u32 = 3;
const KSPROPERTY_MEMBER_FLAG_DEFAULT: u32 = 1;
const KSPROPTYPESETID_GENERAL: GUID = GUID::from_u128(0x97e99ba0_bdea_11cf_a5d6_28db04c10000);
const VT_I4: u32 = 3;

#[repr(C)]
#[derive(Clone, Copy)]
struct KsProperty {
    set: GUID,
    id: u32,
    flags: u32,
}

/// `KSPROPERTY_VIDEOPROCAMP_S` and `KSPROPERTY_CAMERACONTROL_S` share this layout.
#[repr(C)]
struct ValueS {
    property: KsProperty,
    value: i32,
    flags: u32,
    capabilities: u32,
}

#[repr(C)]
struct Description {
    access_flags: u32,
    description_size: u32,
    prop_type_set: KsProperty,
    members_list_count: u32,
    reserved: u32,
}

#[repr(C)]
struct MembersHeader {
    members_flags: u32,
    members_size: u32,
    members_count: u32,
    flags: u32,
}

#[repr(C)]
struct SteppingLong {
    stepping_delta: u32,
    reserved: u32,
    minimum: i32,
    maximum: i32,
}

#[repr(C)]
struct BasicSupport {
    description: Description,
    header: MembersHeader,
    range: SteppingLong,
}

#[repr(C)]
struct DefaultValues {
    description: Description,
    header: MembersHeader,
    value: i32,
}

/// Controls the phone supports, with their current value and flags.
pub struct Controls {
    entries: Mutex<Vec<(ControlDesc, i32, u32)>>,
}

impl Controls {
    /// Starts from the values in shared memory when a host already set some, else from defaults.
    pub fn new(descs: Vec<ControlDesc>, frames: Option<&SharedFrames>) -> Self {
        let resume = frames.is_some_and(|f| f.controls_seq() > 0);
        let entries = descs
            .into_iter()
            .map(|d| {
                let (v, f) = match frames {
                    Some(fr) if resume => {
                        let (v, f) = fr.control(d.control);
                        d.sanitize(v, f)
                    }
                    _ => (d.default, d.default_flags),
                };
                if let Some(fr) = frames {
                    fr.set_control(d.control, v, f, false);
                }
                (d, v, f)
            })
            .collect();
        Controls { entries: Mutex::new(entries) }
    }

    /// Handles one `IKsControl::KsProperty` call.
    pub unsafe fn property(
        &self,
        frames: Option<&SharedFrames>,
        property: *const core::ffi::c_void,
        property_len: u32,
        data: *mut core::ffi::c_void,
        data_len: u32,
        returned: *mut u32,
    ) -> Result<()> {
        if property.is_null() || (property_len as usize) < size_of::<KsProperty>() {
            return Err(E_INVALIDARG.into());
        }
        let p = unsafe { std::ptr::read_unaligned(property as *const KsProperty) };
        let mut entries = self.entries.lock().unwrap();
        let Some(entry) = entries.iter_mut().find(|(d, _, _)| d.control.ks() == (p.set, p.id)) else {
            let known = p.set == PROPSETID_VIDCAP_VIDEOPROCAMP || p.set == PROPSETID_VIDCAP_CAMERACONTROL;
            let code = if known { ERROR_NOT_FOUND } else { ERROR_SET_NOT_FOUND };
            return Err(HRESULT::from_win32(code.0).into());
        };
        let set_returned = |n: usize| {
            if !returned.is_null() {
                unsafe { *returned = n as u32 };
            }
        };
        let (desc, value, flags) = (entry.0, entry.1, entry.2);
        let description = |size: usize, members: u32| Description {
            access_flags: KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_SET | KSPROPERTY_TYPE_DEFAULTVALUES,
            description_size: size as u32,
            prop_type_set: KsProperty { set: KSPROPTYPESETID_GENERAL, id: VT_I4, flags: 0 },
            members_list_count: members,
            reserved: 0,
        };

        if p.flags & KSPROPERTY_TYPE_BASICSUPPORT != 0 {
            let full = BasicSupport {
                description: description(size_of::<BasicSupport>(), 1),
                header: MembersHeader {
                    members_flags: KSPROPERTY_MEMBER_STEPPEDRANGES,
                    members_size: size_of::<SteppingLong>() as u32,
                    members_count: 1,
                    flags: 0,
                },
                range: SteppingLong {
                    stepping_delta: desc.step as u32,
                    reserved: 0,
                    minimum: desc.min,
                    maximum: desc.max,
                },
            };
            return unsafe { write_prefix(&full, data, data_len, set_returned) };
        }
        if p.flags & KSPROPERTY_TYPE_DEFAULTVALUES != 0 {
            let full = DefaultValues {
                description: description(size_of::<DefaultValues>(), 1),
                header: MembersHeader {
                    members_flags: KSPROPERTY_MEMBER_VALUES,
                    members_size: size_of::<i32>() as u32,
                    members_count: 1,
                    flags: KSPROPERTY_MEMBER_FLAG_DEFAULT,
                },
                value: desc.default,
            };
            return unsafe { write_prefix(&full, data, data_len, set_returned) };
        }
        if data.is_null() {
            return Err(E_POINTER.into());
        }
        if (data_len as usize) < size_of::<ValueS>() {
            set_returned(size_of::<ValueS>());
            return Err(HRESULT::from_win32(ERROR_MORE_DATA.0).into());
        }
        let s = data as *mut ValueS;
        if p.flags & KSPROPERTY_TYPE_GET != 0 {
            unsafe {
                (*s).value = value;
                (*s).flags = flags;
                (*s).capabilities = desc.caps;
            }
            set_returned(size_of::<ValueS>());
            return Ok(());
        }
        if p.flags & KSPROPERTY_TYPE_SET != 0 {
            let (v, f) = unsafe { desc.sanitize((*s).value, (*s).flags) };
            entry.1 = v;
            entry.2 = f;
            if let Some(fr) = frames {
                fr.set_control(desc.control, v, f, true);
            }
            let mode = if f == FLAG_AUTO { "auto" } else if f == FLAG_MANUAL { "manual" } else { "?" };
            vlog!("app set {} = {v} ({mode})", desc.control.name());
            set_returned(0);
            return Ok(());
        }
        Err(E_INVALIDARG.into())
    }
}

/// Writes as much of `full` as the caller asked for: 4 bytes (access flags only), the description
/// header only, or everything. With no buffer, reports the size needed.
unsafe fn write_prefix<T>(full: &T, data: *mut core::ffi::c_void, data_len: u32, set_returned: impl Fn(usize)) -> Result<()> {
    let total = size_of::<T>();
    let len = data_len as usize;
    if data.is_null() || len == 0 {
        set_returned(total);
        return Err(HRESULT::from_win32(ERROR_MORE_DATA.0).into());
    }
    let n = if len >= total {
        total
    } else if len >= size_of::<Description>() {
        size_of::<Description>()
    } else if len >= size_of::<u32>() {
        size_of::<u32>()
    } else {
        set_returned(total);
        return Err(HRESULT::from_win32(ERROR_MORE_DATA.0).into());
    };
    unsafe { std::ptr::copy_nonoverlapping(full as *const T as *const u8, data as *mut u8, n) };
    set_returned(n);
    Ok(())
}
