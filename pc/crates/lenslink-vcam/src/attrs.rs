//! `IMFAttributes` for our COM objects, forwarded to an attribute store made by `MFCreateAttributes`.
//! `IMFActivate` derives from it, and the Frame Server reads source and stream attributes.

use core::ffi::c_void;

use windows_core::Ref;

/// The raw interface pointer behind a borrowed COM parameter.
pub fn raw<T: windows_core::Interface>(r: &Ref<T>) -> *mut c_void {
    r.as_ref().map_or(std::ptr::null_mut(), |i| i.as_raw())
}

macro_rules! forward_attributes {
    ($ty:ty, $field:ident) => {
        impl windows::Win32::Media::MediaFoundation::IMFAttributes_Impl for $ty {
            fn GetItem(&self, k: *const windows_core::GUID, v: *mut windows::Win32::System::Com::StructuredStorage::PROPVARIANT) -> windows_core::Result<()> {
                unsafe { (windows_core::Interface::vtable(&self.$field).GetItem)(windows_core::Interface::as_raw(&self.$field), k, v).ok() }
            }
            fn GetItemType(&self, k: *const windows_core::GUID) -> windows_core::Result<windows::Win32::Media::MediaFoundation::MF_ATTRIBUTE_TYPE> {
                unsafe { self.$field.GetItemType(k) }
            }
            fn CompareItem(&self, k: *const windows_core::GUID, v: *const windows::Win32::System::Com::StructuredStorage::PROPVARIANT) -> windows_core::Result<windows_core::BOOL> {
                let mut r = windows_core::BOOL::default();
                unsafe { (windows_core::Interface::vtable(&self.$field).CompareItem)(windows_core::Interface::as_raw(&self.$field), k, v, &mut r).ok()? };
                Ok(r)
            }
            fn Compare(&self, theirs: windows_core::Ref<windows::Win32::Media::MediaFoundation::IMFAttributes>, t: windows::Win32::Media::MediaFoundation::MF_ATTRIBUTES_MATCH_TYPE) -> windows_core::Result<windows_core::BOOL> {
                let mut r = windows_core::BOOL::default();
                unsafe { (windows_core::Interface::vtable(&self.$field).Compare)(windows_core::Interface::as_raw(&self.$field), $crate::attrs::raw(&theirs), t, &mut r).ok()? };
                Ok(r)
            }
            fn GetUINT32(&self, k: *const windows_core::GUID) -> windows_core::Result<u32> { unsafe { self.$field.GetUINT32(k) } }
            fn GetUINT64(&self, k: *const windows_core::GUID) -> windows_core::Result<u64> { unsafe { self.$field.GetUINT64(k) } }
            fn GetDouble(&self, k: *const windows_core::GUID) -> windows_core::Result<f64> { unsafe { self.$field.GetDouble(k) } }
            fn GetGUID(&self, k: *const windows_core::GUID) -> windows_core::Result<windows_core::GUID> { unsafe { self.$field.GetGUID(k) } }
            fn GetStringLength(&self, k: *const windows_core::GUID) -> windows_core::Result<u32> { unsafe { self.$field.GetStringLength(k) } }
            fn GetString(&self, k: *const windows_core::GUID, v: windows_core::PWSTR, size: u32, len: *mut u32) -> windows_core::Result<()> {
                unsafe { (windows_core::Interface::vtable(&self.$field).GetString)(windows_core::Interface::as_raw(&self.$field), k, v, size, len).ok() }
            }
            fn GetAllocatedString(&self, k: *const windows_core::GUID, v: *mut windows_core::PWSTR, len: *mut u32) -> windows_core::Result<()> {
                unsafe { (windows_core::Interface::vtable(&self.$field).GetAllocatedString)(windows_core::Interface::as_raw(&self.$field), k, v, len).ok() }
            }
            fn GetBlobSize(&self, k: *const windows_core::GUID) -> windows_core::Result<u32> { unsafe { self.$field.GetBlobSize(k) } }
            fn GetBlob(&self, k: *const windows_core::GUID, buf: *mut u8, size: u32, len: *mut u32) -> windows_core::Result<()> {
                unsafe { (windows_core::Interface::vtable(&self.$field).GetBlob)(windows_core::Interface::as_raw(&self.$field), k, buf, size, len).ok() }
            }
            fn GetAllocatedBlob(&self, k: *const windows_core::GUID, buf: *mut *mut u8, size: *mut u32) -> windows_core::Result<()> {
                unsafe { (windows_core::Interface::vtable(&self.$field).GetAllocatedBlob)(windows_core::Interface::as_raw(&self.$field), k, buf, size).ok() }
            }
            fn GetUnknown(&self, k: *const windows_core::GUID, iid: *const windows_core::GUID, v: *mut *mut core::ffi::c_void) -> windows_core::Result<()> {
                unsafe { (windows_core::Interface::vtable(&self.$field).GetUnknown)(windows_core::Interface::as_raw(&self.$field), k, iid, v).ok() }
            }
            fn SetItem(&self, k: *const windows_core::GUID, v: *const windows::Win32::System::Com::StructuredStorage::PROPVARIANT) -> windows_core::Result<()> {
                unsafe { (windows_core::Interface::vtable(&self.$field).SetItem)(windows_core::Interface::as_raw(&self.$field), k, v).ok() }
            }
            fn DeleteItem(&self, k: *const windows_core::GUID) -> windows_core::Result<()> { unsafe { self.$field.DeleteItem(k) } }
            fn DeleteAllItems(&self) -> windows_core::Result<()> { unsafe { self.$field.DeleteAllItems() } }
            fn SetUINT32(&self, k: *const windows_core::GUID, v: u32) -> windows_core::Result<()> { unsafe { self.$field.SetUINT32(k, v) } }
            fn SetUINT64(&self, k: *const windows_core::GUID, v: u64) -> windows_core::Result<()> { unsafe { self.$field.SetUINT64(k, v) } }
            fn SetDouble(&self, k: *const windows_core::GUID, v: f64) -> windows_core::Result<()> { unsafe { self.$field.SetDouble(k, v) } }
            fn SetGUID(&self, k: *const windows_core::GUID, v: *const windows_core::GUID) -> windows_core::Result<()> { unsafe { self.$field.SetGUID(k, v) } }
            fn SetString(&self, k: *const windows_core::GUID, v: &windows_core::PCWSTR) -> windows_core::Result<()> {
                unsafe { (windows_core::Interface::vtable(&self.$field).SetString)(windows_core::Interface::as_raw(&self.$field), k, *v).ok() }
            }
            fn SetBlob(&self, k: *const windows_core::GUID, buf: *const u8, size: u32) -> windows_core::Result<()> {
                unsafe { (windows_core::Interface::vtable(&self.$field).SetBlob)(windows_core::Interface::as_raw(&self.$field), k, buf, size).ok() }
            }
            fn SetUnknown(&self, k: *const windows_core::GUID, v: windows_core::Ref<windows_core::IUnknown>) -> windows_core::Result<()> {
                unsafe { (windows_core::Interface::vtable(&self.$field).SetUnknown)(windows_core::Interface::as_raw(&self.$field), k, $crate::attrs::raw(&v)).ok() }
            }
            fn LockStore(&self) -> windows_core::Result<()> { unsafe { self.$field.LockStore() } }
            fn UnlockStore(&self) -> windows_core::Result<()> { unsafe { self.$field.UnlockStore() } }
            fn GetCount(&self) -> windows_core::Result<u32> { unsafe { self.$field.GetCount() } }
            fn GetItemByIndex(&self, i: u32, k: *mut windows_core::GUID, v: *mut windows::Win32::System::Com::StructuredStorage::PROPVARIANT) -> windows_core::Result<()> {
                unsafe { (windows_core::Interface::vtable(&self.$field).GetItemByIndex)(windows_core::Interface::as_raw(&self.$field), i, k, v).ok() }
            }
            fn CopyAllItems(&self, dest: windows_core::Ref<windows::Win32::Media::MediaFoundation::IMFAttributes>) -> windows_core::Result<()> {
                unsafe { (windows_core::Interface::vtable(&self.$field).CopyAllItems)(windows_core::Interface::as_raw(&self.$field), $crate::attrs::raw(&dest)).ok() }
            }
        }
    };
}
pub(crate) use forward_attributes;
