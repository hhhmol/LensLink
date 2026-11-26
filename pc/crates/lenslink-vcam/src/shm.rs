//! Frame channel from the host process to the camera source.
//!
//! The camera source runs in session 0 and the host in the user's session, so the objects live in the
//! `Global\` namespace. Only a service may create global objects, so the camera side creates them
//! (with an ACL that lets interactive users write) and the host opens them, retrying until the camera
//! is in use. Two frame slots: the host fills the one the camera is not reading, then publishes it.
//! The camera writes back the mode the app chose, and the host switches the phone to it.

use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};

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

use crate::controls::{Control, MAX_CONTROLS};
use crate::modes::Mode;

/// Largest frame a slot holds (NV12): DCI 4K, or UHD with room to spare.
pub const MAX_WIDTH: u32 = 4096;
pub const MAX_HEIGHT: u32 = 2304;
const SLOT_BYTES: usize = (MAX_WIDTH * MAX_HEIGHT * 3 / 2) as usize;

const MAPPING_NAME: &str = "Global\\LensLinkVirtualCamera";
const EVENT_NAME: &str = "Global\\LensLinkVirtualCameraFrame";
const MAGIC: u32 = u32::from_le_bytes(*b"LLVC");
const VERSION: u32 = 3;
const HEADER_BYTES: usize = 4096;
const TOTAL_BYTES: usize = HEADER_BYTES + 2 * SLOT_BYTES;

#[repr(C)]
struct Header {
    magic: u32,
    version: u32,
    slot_bytes: u32,
    _pad0: u32,
    /// Frames published so far.
    seq: AtomicU64,
    /// Slot of the newest frame (0 or 1).
    latest: AtomicU32,
    _pad: u32,
    /// `GetTickCount64` of the last publish; 0 = never.
    written_ms: AtomicU64,
    /// [`Mode::pack`] of the frame in each slot.
    slot_mode: [AtomicU64; 2],
    /// Mode the app selected, [`Mode::pack`]; 0 = none yet.
    requested: AtomicU64,
    /// Bumped by the camera after an app changed a control.
    controls_seq: AtomicU64,
    /// Current value and `FLAG_AUTO`/`FLAG_MANUAL` per [`crate::controls::Control::index`].
    control_value: [AtomicI32; MAX_CONTROLS],
    control_flags: [AtomicU32; MAX_CONTROLS],
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
                (*h).slot_bytes = SLOT_BYTES as u32;
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
            if h.magic != MAGIC || h.version != VERSION || h.slot_bytes != SLOT_BYTES as u32 {
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
        unsafe { (self.view.Value as *mut u8).add(HEADER_BYTES + index as usize * SLOT_BYTES) }
    }

    /// Host: writes a frame of `mode` into the free slot, publishes it and wakes the camera.
    pub fn publish(&self, mode: Mode, fill: impl FnOnce(&mut [u8])) {
        assert!(mode.is_valid(), "frame size {mode} does not fit the shared memory");
        let h = self.header();
        let back = h.latest.load(Ordering::Acquire) ^ 1;
        fill(unsafe { std::slice::from_raw_parts_mut(self.slot(back), mode.frame_bytes()) });
        h.slot_mode[back as usize].store(mode.pack(), Ordering::Release);
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

    /// Size of the newest frame, if any was published.
    pub fn latest_size(&self) -> Option<(u32, u32)> {
        let h = self.header();
        let slot = h.latest.load(Ordering::Acquire) as usize;
        Mode::unpack(h.slot_mode[slot & 1].load(Ordering::Acquire)).map(|m| (m.width, m.height))
    }

    /// Camera: copies the newest frame into `dst` (sized for `width`×`height` NV12) and returns its
    /// sequence number, or `None` if the newest frame has another size (the host is still switching).
    /// The host only overwrites this slot after it has filled the other one, a whole frame interval
    /// later, so a copy (a few milliseconds even at 4K) is not torn.
    pub fn read(&self, dst: &mut [u8], width: u32, height: u32) -> Option<u64> {
        let h = self.header();
        let seq = h.seq.load(Ordering::Acquire);
        let slot = h.latest.load(Ordering::Acquire) & 1;
        let m = Mode::unpack(h.slot_mode[slot as usize].load(Ordering::Acquire))?;
        if (m.width, m.height) != (width, height) || dst.len() < m.frame_bytes() {
            return None;
        }
        unsafe { std::ptr::copy_nonoverlapping(self.slot(slot), dst.as_mut_ptr(), m.frame_bytes()) };
        Some(seq)
    }

    /// Camera: records the mode the app selected.
    pub fn request(&self, mode: Mode) {
        self.header().requested.store(mode.pack(), Ordering::Release);
    }

    /// Host: the mode the app selected, if any.
    pub fn requested(&self) -> Option<Mode> {
        Mode::unpack(self.header().requested.load(Ordering::Acquire))
    }

    /// Changes to controls so far; 0 = none since the camera was created.
    pub fn controls_seq(&self) -> u64 {
        self.header().controls_seq.load(Ordering::Acquire)
    }

    /// Value and flags of a control (flags 0 = never set).
    pub fn control(&self, c: Control) -> (i32, u32) {
        let h = self.header();
        (h.control_value[c.index()].load(Ordering::Acquire), h.control_flags[c.index()].load(Ordering::Acquire))
    }

    /// Camera: stores a control; `changed` tells the host to apply it.
    pub fn set_control(&self, c: Control, value: i32, flags: u32, changed: bool) {
        let h = self.header();
        h.control_value[c.index()].store(value, Ordering::Release);
        h.control_flags[c.index()].store(flags, Ordering::Release);
        if changed {
            h.controls_seq.fetch_add(1, Ordering::AcqRel);
        }
    }
}

const _: () = assert!(size_of::<Header>() <= HEADER_BYTES);

impl Drop for SharedFrames {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.mapping);
            let _ = CloseHandle(self.event);
        }
    }
}
