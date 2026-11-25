//! Frame channel from the host process to the camera source.
//!
//! The camera source runs in session 0 and the host in the user's session, so the objects live in the
//! `Global\` namespace. Only a service may create global objects, so the camera side creates them
//! (with an ACL that lets interactive users write) and the host opens them, retrying until the camera
//! is in use. Two frame slots: the host fills the one the camera is not reading, then publishes it.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use windows::core::{w, Error, Result, HSTRING};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, ERROR_ALREADY_EXISTS, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
    MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::System::Threading::{
    CreateEventW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE, SYNCHRONIZATION_SYNCHRONIZE,
};

/// The camera's only format for now: 1080p NV12. The host scales whatever the phone sends.
pub const WIDTH: u32 = 1920;
pub const HEIGHT: u32 = 1080;
pub const FPS: u32 = 30;
pub const FRAME_BYTES: usize = (WIDTH * HEIGHT * 3 / 2) as usize;

const MAPPING_NAME: &str = "Global\\LensLinkVirtualCamera";
const EVENT_NAME: &str = "Global\\LensLinkVirtualCameraFrame";
const MAGIC: u32 = u32::from_le_bytes(*b"LLVC");
const VERSION: u32 = 1;
const HEADER_BYTES: usize = 4096;
const TOTAL_BYTES: usize = HEADER_BYTES + 2 * FRAME_BYTES;

#[repr(C)]
struct Header {
    magic: u32,
    version: u32,
    width: u32,
    height: u32,
    /// Frames published so far.
    seq: AtomicU64,
    /// Slot of the newest frame (0 or 1).
    latest: AtomicU32,
    _pad: u32,
    /// `GetTickCount64` of the last publish; 0 = never.
    written_ms: AtomicU64,
}

pub struct SharedFrames {
    mapping: HANDLE,
    event: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

// The view is plain shared memory guarded by the atomics in the header.
unsafe impl Send for SharedFrames {}
unsafe impl Sync for SharedFrames {}

impl SharedFrames {
    /// Camera side: creates (or reuses) the mapping and the frame event.
    pub fn create() -> Result<Self> {
        unsafe {
            // SYSTEM, LOCAL SERVICE, administrators and interactive users.
            let mut sd = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                w!("D:(A;;GA;;;SY)(A;;GA;;;LS)(A;;GA;;;BA)(A;;GA;;;IU)"),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )?;
            let sa = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: false.into(),
            };
            let mapping = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                Some(&sa),
                PAGE_READWRITE,
                0,
                TOTAL_BYTES as u32,
                &HSTRING::from(MAPPING_NAME),
            );
            let existed = GetLastError() == ERROR_ALREADY_EXISTS;
            let event = CreateEventW(Some(&sa), false, false, &HSTRING::from(EVENT_NAME));
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            let frames = Self::map(mapping?, event?)?;
            if !existed {
                let h = frames.view.Value as *mut Header;
                (*h).magic = MAGIC;
                (*h).version = VERSION;
                (*h).width = WIDTH;
                (*h).height = HEIGHT;
            }
            Ok(frames)
        }
    }

    /// Host side: opens the objects the camera created. Fails while no app is using the camera.
    pub fn open() -> Result<Self> {
        unsafe {
            let mapping = OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, &HSTRING::from(MAPPING_NAME))?;
            let event = match OpenEventW(
                EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE,
                false,
                &HSTRING::from(EVENT_NAME),
            ) {
                Ok(e) => e,
                Err(e) => {
                    let _ = CloseHandle(mapping);
                    return Err(e);
                }
            };
            let frames = Self::map(mapping, event)?;
            let h = frames.header();
            if h.magic != MAGIC || h.version != VERSION || h.width != WIDTH || h.height != HEIGHT {
                return Err(Error::new(
                    windows::Win32::Foundation::E_UNEXPECTED,
                    "the installed virtual camera DLL is a different version; reinstall it",
                ));
            }
            Ok(frames)
        }
    }

    unsafe fn map(mapping: HANDLE, event: HANDLE) -> Result<Self> {
        unsafe {
            let view = MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, TOTAL_BYTES);
            if view.Value.is_null() {
                let e = Error::from_thread();
                let _ = CloseHandle(mapping);
                let _ = CloseHandle(event);
                return Err(e);
            }
            Ok(SharedFrames { mapping, event, view })
        }
    }

    fn header(&self) -> &Header {
        unsafe { &*(self.view.Value as *const Header) }
    }

    fn slot(&self, index: u32) -> *mut u8 {
        unsafe { (self.view.Value as *mut u8).add(HEADER_BYTES + index as usize * FRAME_BYTES) }
    }

    /// Host: writes a frame into the free slot, publishes it and wakes the camera.
    pub fn publish(&self, fill: impl FnOnce(&mut [u8])) {
        let h = self.header();
        let back = h.latest.load(Ordering::Acquire) ^ 1;
        fill(unsafe { std::slice::from_raw_parts_mut(self.slot(back), FRAME_BYTES) });
        h.latest.store(back, Ordering::Release);
        h.seq.fetch_add(1, Ordering::AcqRel);
        h.written_ms.store(unsafe { GetTickCount64() }, Ordering::Release);
        unsafe {
            let _ = SetEvent(self.event);
        }
    }

    /// Camera: waits up to `ms` for a newly published frame.
    pub fn wait(&self, ms: u32) -> bool {
        unsafe { WaitForSingleObject(self.event, ms) == WAIT_OBJECT_0 }
    }

    pub fn seq(&self) -> u64 {
        self.header().seq.load(Ordering::Acquire)
    }

    /// Milliseconds since the host last published, `None` if it never did.
    pub fn age_ms(&self) -> Option<u64> {
        match self.header().written_ms.load(Ordering::Acquire) {
            0 => None,
            t => Some(unsafe { GetTickCount64() }.saturating_sub(t)),
        }
    }

    /// Camera: copies the newest frame. The host only overwrites this slot after it has filled the
    /// other one, a whole frame interval later, so a copy (well under a millisecond) is never torn.
    pub fn read(&self, dst: &mut [u8]) -> u64 {
        let h = self.header();
        let seq = h.seq.load(Ordering::Acquire);
        let slot = h.latest.load(Ordering::Acquire);
        let n = dst.len().min(FRAME_BYTES);
        unsafe { std::ptr::copy_nonoverlapping(self.slot(slot), dst.as_mut_ptr(), n) };
        seq
    }
}

impl Drop for SharedFrames {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.mapping);
            let _ = CloseHandle(self.event);
        }
    }
}
