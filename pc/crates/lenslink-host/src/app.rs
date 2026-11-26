//! `LensLink`: the tray app. Connects to the phone by itself, shows the video in its window and
//! publishes the virtual camera; closing the window keeps it running in the tray.
//!
//! `--hidden` starts it in the tray (for starting with Windows).

#![windows_subsystem = "windows"]

use std::os::windows::io::AsRawHandle;

use lenslink_host::{settings, stream, ui};
use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HANDLE, LPARAM, WPARAM};
use windows::Win32::System::Console::{SetStdHandle, STD_ERROR_HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, MessageBoxW, PostMessageW, MB_ICONERROR, MB_OK};

fn main() {
    log_to_file();
    unsafe {
        // One LensLink at a time: a second start brings the first one's window up.
        let _mutex = CreateMutexW(None, false, w!("Local\\io.github.lenslink.app"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(hwnd) = FindWindowW(ui::CLASS_NAME, None) {
                let _ = PostMessageW(Some(hwnd), ui::WM_APP_SHOW, WPARAM(0), LPARAM(0));
            }
            return;
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    ui::start_hidden(std::env::args().any(|a| a == "--hidden"));
    eprintln!("LensLink {} started", env!("CARGO_PKG_VERSION"));

    let options = stream::Options { player: stream::Player::App, virtual_camera: true, ..Default::default() };
    if let Err(e) = stream::run(&options) {
        eprintln!("error: {e}");
        unsafe {
            MessageBoxW(None, &HSTRING::from(e), w!("LensLink"), MB_OK | MB_ICONERROR);
        }
    }
    eprintln!("LensLink stopped");
}

/// The app has no console: what the host prints goes to `%APPDATA%\LensLink\lenslink.log`, started
/// afresh each run.
fn log_to_file() {
    let Some(dir) = settings::dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(file) = std::fs::File::create(dir.join("lenslink.log")) {
        unsafe {
            if SetStdHandle(STD_ERROR_HANDLE, HANDLE(file.as_raw_handle())).is_ok() {
                // The handle now belongs to stderr for the rest of the process.
                std::mem::forget(file);
            }
        }
    }
}
