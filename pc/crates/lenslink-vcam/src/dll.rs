//! COM in-process server exports.
//!
//! Registration goes to HKLM because the Frame Server service cannot see the user's HKCU, so
//! `regsvr32` must run elevated (see `tools/install-vcam.ps1`).

use std::sync::atomic::{AtomicPtr, Ordering};

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_POINTER, HINSTANCE, HMODULE, MAX_PATH, S_FALSE, S_OK,
};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_WRITE,
    REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows_core::{implement, IUnknown, Interface, Ref, Result, BOOL, GUID, HRESULT};

use crate::activator::Activator;
use crate::{CLSID, CLSID_STR};

static MODULE: AtomicPtr<core::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());

#[no_mangle]
extern "system" fn DllMain(module: HINSTANCE, reason: u32, _: *mut core::ffi::c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(module.0, Ordering::Release);
    }
    true.into()
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<IUnknown>, iid: *const GUID, object: *mut *mut core::ffi::c_void) -> Result<()> {
        if !outer.is_null() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let activator: windows::Win32::Media::MediaFoundation::IMFActivate = Activator::new()?.into();
        unsafe { activator.query(iid, object).ok() }
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

#[no_mangle]
unsafe extern "system" fn DllGetClassObject(clsid: *const GUID, iid: *const GUID, out: *mut *mut core::ffi::c_void) -> HRESULT {
    if out.is_null() || clsid.is_null() {
        return E_POINTER;
    }
    unsafe {
        *out = std::ptr::null_mut();
        if *clsid != CLSID {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = Factory.into();
        factory.query(iid, out)
    }
}

/// The DLL stays loaded once used: the Frame Server keeps sources alive between apps anyway.
#[no_mangle]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}

#[no_mangle]
extern "system" fn DllRegisterServer() -> HRESULT {
    match register() {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    }
}

#[no_mangle]
extern "system" fn DllUnregisterServer() -> HRESULT {
    let key = HSTRING::from(format!(r"SOFTWARE\Classes\CLSID\{CLSID_STR}"));
    let r = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, &key) };
    if r.is_ok() || r == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND {
        S_OK
    } else {
        r.to_hresult()
    }
}

fn register() -> Result<()> {
    let mut path = [0u16; MAX_PATH as usize];
    let module = HMODULE(MODULE.load(Ordering::Acquire));
    let len = unsafe { GetModuleFileNameW(Some(module), &mut path) } as usize;
    if len == 0 {
        return Err(windows_core::Error::from_thread());
    }
    let path = String::from_utf16_lossy(&path[..len]);
    let base = format!(r"SOFTWARE\Classes\CLSID\{CLSID_STR}");
    set_string(&base, None, "LensLink virtual camera")?;
    let inproc = format!(r"{base}\InprocServer32");
    set_string(&inproc, None, &path)?;
    set_string(&inproc, Some("ThreadingModel"), "Both")
}

fn set_string(key: &str, name: Option<&str>, value: &str) -> Result<()> {
    unsafe {
        let mut hkey = HKEY::default();
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            &HSTRING::from(key),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
        .ok()?;
        let data: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
        let bytes = std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2);
        let name = name.map(HSTRING::from);
        let r = RegSetValueExW(
            hkey,
            name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr())),
            None,
            REG_SZ,
            Some(bytes),
        );
        let _ = RegCloseKey(hkey);
        r.ok()
    }
}

