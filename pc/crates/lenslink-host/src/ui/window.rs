//! The window thread: a borderless window, its tray icon and its message loop.
//!
//! The window has no frame of its own (`WM_NCCALCSIZE` hands the whole rectangle to the client), but
//! keeps a thick-frame style so Windows still gives it a shadow, rounded corners, snapping and
//! maximising. It is moved by dragging anywhere on the video and resized at the corners: the edges
//! are left free for the settings curtains that come next. Closing hides it in the tray.

use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use windows::core::{w, PCWSTR, Result};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::ID2D1Factory1;
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::{MonitorFromRect, ValidateRect, MONITOR_DEFAULTTONULL};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{MARGINS, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::render::{button_at, Button, Renderer, Scene};
use super::{Com, Shared};
use crate::settings::Settings;

pub const CLASS_NAME: PCWSTR = w!("LensLinkApp");
pub const WM_APP_FRAME: u32 = WM_APP + 1;
pub const WM_APP_STATUS: u32 = WM_APP + 2;
const WM_APP_TRAY: u32 = WM_APP + 3;
pub const WM_APP_EXIT: u32 = WM_APP + 4;
/// Another LensLink started: show this window instead.
pub const WM_APP_SHOW: u32 = WM_APP + 5;

/// No frame for this long: the stream is not live, show the status in the middle.
const LIVE: Duration = Duration::from_millis(1500);
/// Corner area that resizes the window, in DIPs.
const CORNER: i32 = 18;

const MENU_SHOW: usize = 1;
const MENU_EXIT: usize = 2;

struct Win {
    hwnd: HWND,
    shared: Arc<Shared>,
    renderer: Renderer,
    icon_small: HICON,
    icon_large: HICON,
    taskbar_created: u32,
    /// Pointer position in DIPs while it is over the window.
    mouse: Option<(f32, f32)>,
    pressed: Option<Button>,
    /// Width / height of the video, kept when the window is resized.
    aspect: f32,
}

pub fn run(shared: Arc<Shared>, device: Com<ID3D11Device>, factory: Com<ID2D1Factory1>, hidden: bool, ready: Sender<std::result::Result<(), String>>) {
    match create(shared.clone(), &device.0, &factory.0, hidden) {
        Ok(win) => {
            let _ = ready.send(Ok(()));
            message_loop();
            drop(win);
        }
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
        }
    }
    shared.quit.store(true, Ordering::Release);
    shared.hwnd.store(0, Ordering::Release);
}

fn message_loop() {
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn create(shared: Arc<Shared>, device: &ID3D11Device, factory: &ID2D1Factory1, hidden: bool) -> Result<Box<Win>> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let instance = GetModuleHandleW(None)?;
        let icon_large = super::icon::lens(GetSystemMetrics(SM_CXICON).max(32))?;
        let icon_small = super::icon::lens(GetSystemMetrics(SM_CXSMICON).max(16))?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hIcon: icon_large,
            hIconSm: icon_small,
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        RegisterClassExW(&class);

        let settings = Settings::load();
        let size = shared.state().size;
        let aspect = if size.0 > 0 && size.1 > 0 { size.0 as f32 / size.1 as f32 } else { 16.0 / 9.0 };
        let scale = GetDpiForSystem() as f32 / 96.0;
        let saved = settings.window.filter(|r| {
            let rect = RECT { left: r[0], top: r[1], right: r[0] + r[2], bottom: r[1] + r[3] };
            !MonitorFromRect(&rect, MONITOR_DEFAULTTONULL).is_invalid()
        });
        let (x, y, w, h) = match saved {
            Some(r) => (r[0], r[1], r[2], r[3]),
            None => {
                let w = (960.0 * scale) as i32;
                (CW_USEDEFAULT, CW_USEDEFAULT, w, (w as f32 / aspect) as i32)
            }
        };
        let style = WS_POPUP | WS_THICKFRAME | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX;
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE::default(), CLASS_NAME, w!("LensLink"), style, x, y, w, h, None, None, Some(instance.into()), None)?;

        let corner = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const _, size_of_val(&corner) as u32);
        let dark = windows::core::BOOL(1);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const _, size_of_val(&dark) as u32);
        // A sliver of DWM frame keeps the shadow.
        let _ = DwmExtendFrameIntoClientArea(hwnd, &MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 1, cyBottomHeight: 0 });

        let renderer = Renderer::new(device, factory, hwnd, GetDpiForWindow(hwnd) as f32)?;
        let mut win = Box::new(Win {
            hwnd,
            shared: shared.clone(),
            renderer,
            icon_small,
            icon_large,
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            mouse: None,
            pressed: None,
            aspect,
        });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *win as *mut Win as isize);
        // Apply WM_NCCALCSIZE now that the state is in place.
        SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE)?;
        win.tray(NIM_ADD);
        shared.hwnd.store(hwnd.0 as isize, Ordering::Release);
        if !hidden {
            let _ = ShowWindow(hwnd, if settings.maximized { SW_SHOWMAXIMIZED } else { SW_SHOWNORMAL });
            let _ = SetForegroundWindow(hwnd);
        }
        win.update_visible();
        Ok(win)
    }
}

impl Drop for Win {
    fn drop(&mut self) {
        unsafe {
            self.tray(NIM_DELETE);
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
            let _ = DestroyIcon(self.icon_small);
            let _ = DestroyIcon(self.icon_large);
        }
    }
}

impl Win {
    fn dpi(&self) -> f32 {
        unsafe { GetDpiForWindow(self.hwnd) as f32 }
    }

    /// Client point in DIPs from a mouse message.
    fn point(&self, lparam: LPARAM) -> (f32, f32) {
        let s = 96.0 / self.dpi();
        let x = (lparam.0 & 0xffff) as i16 as f32;
        let y = ((lparam.0 >> 16) & 0xffff) as i16 as f32;
        (x * s, y * s)
    }

    fn update_visible(&self) {
        let visible = unsafe { IsWindowVisible(self.hwnd).as_bool() && !IsIconic(self.hwnd).as_bool() };
        self.shared.visible.store(visible, Ordering::Release);
    }

    fn show(&mut self) {
        unsafe {
            if IsIconic(self.hwnd).as_bool() {
                let _ = ShowWindow(self.hwnd, SW_RESTORE);
            } else {
                let _ = ShowWindow(self.hwnd, SW_SHOW);
            }
            let _ = SetForegroundWindow(self.hwnd);
        }
        self.update_visible();
        self.draw();
    }

    /// Into the tray: the phone's camera turns off unless an app uses the virtual camera.
    fn hide(&mut self) {
        self.save();
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        self.mouse = None;
        self.pressed = None;
        self.update_visible();
    }

    fn exit(&mut self) {
        self.save();
        self.shared.quit.store(true, Ordering::Release);
        unsafe {
            PostQuitMessage(0);
        }
    }

    fn save(&self) {
        unsafe {
            let mut p = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
            if GetWindowPlacement(self.hwnd, &mut p).is_ok() {
                let r = p.rcNormalPosition;
                // rcNormalPosition is in workspace coordinates; for a window without a taskbar on the
                // top or left edge they equal screen coordinates.
                let mut s = Settings::load();
                s.window = Some([r.left, r.top, r.right - r.left, r.bottom - r.top]);
                s.maximized = IsZoomed(self.hwnd).as_bool();
                s.save();
            }
        }
    }

    fn tray(&self, action: NOTIFY_ICON_MESSAGE) {
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_APP_TRAY,
            hIcon: self.icon_small,
            ..Default::default()
        };
        let tip: Vec<u16> = "LensLink".encode_utf16().collect();
        data.szTip[..tip.len()].copy_from_slice(&tip);
        unsafe {
            let _ = Shell_NotifyIconW(action, &data);
        }
    }

    fn tray_menu(&mut self) {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let shown = IsWindowVisible(self.hwnd).as_bool();
            let _ = AppendMenuW(menu, MF_STRING, MENU_SHOW, if shown { w!("Скрыть окно") } else { w!("Открыть LensLink") });
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            let _ = AppendMenuW(menu, MF_STRING, MENU_EXIT, w!("Выход"));
            let _ = SetMenuDefaultItem(menu, MENU_SHOW as u32, 0);
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            // Without this the menu does not close when clicking elsewhere.
            let _ = SetForegroundWindow(self.hwnd);
            let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, None, self.hwnd, None);
            let _ = PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);
            match cmd.0 as usize {
                MENU_SHOW if shown => self.hide(),
                MENU_SHOW => self.show(),
                MENU_EXIT => self.exit(),
                _ => {}
            }
        }
    }

    fn draw(&mut self) {
        if !self.shared.visible.load(Ordering::Acquire) {
            return;
        }
        let (width, _) = self.renderer.size();
        let hot = self.mouse.and_then(|(x, y)| button_at(x, y, width));
        let (video, status, live) = {
            let st = self.shared.state();
            let live = st.last_frame.is_some_and(|t| t.elapsed() < LIVE);
            (st.video.as_ref().map(|v| (v.0.clone(), st.size)), st.status.clone(), live)
        };
        let scene = Scene {
            video: video.as_ref().map(|(t, s)| (t, *s)),
            status: &status,
            live,
            hover: self.mouse.is_some(),
            hot,
            pressed: self.pressed,
        };
        if let Err(e) = self.renderer.draw(&scene) {
            eprintln!("window: draw: {e}");
        }
    }

    /// The video's shape changed (another mode): keep the width, fit the height.
    fn follow_aspect(&mut self) {
        let size = self.shared.state().size;
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        let aspect = size.0 as f32 / size.1 as f32;
        if (aspect - self.aspect).abs() < 0.01 {
            return;
        }
        self.aspect = aspect;
        unsafe {
            if IsZoomed(self.hwnd).as_bool() {
                return;
            }
            let mut r = RECT::default();
            if GetWindowRect(self.hwnd, &mut r).is_ok() {
                let h = ((r.right - r.left) as f32 / aspect).round() as i32;
                let _ = SetWindowPos(self.hwnd, None, 0, 0, r.right - r.left, h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
    }

    unsafe fn handle(&mut self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        unsafe {
            match msg {
                WM_SIZE => {
                    if let Err(e) = self.renderer.resize() {
                        eprintln!("window: resize: {e}");
                    }
                    self.update_visible();
                    self.draw();
                }
                WM_DPICHANGED => {
                    self.renderer.set_dpi((wparam.0 & 0xffff) as f32);
                    let r = &*(lparam.0 as *const RECT);
                    let _ = SetWindowPos(self.hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                }
                WM_SIZING => {
                    let r = &mut *(lparam.0 as *mut RECT);
                    let h = ((r.right - r.left) as f32 / self.aspect).round() as i32;
                    match wparam.0 as u32 {
                        WMSZ_TOPLEFT | WMSZ_TOPRIGHT | WMSZ_TOP => r.top = r.bottom - h,
                        _ => r.bottom = r.top + h,
                    }
                    return Some(LRESULT(1));
                }
                WM_PAINT => {
                    let _ = ValidateRect(Some(self.hwnd), None);
                    self.draw();
                }
                WM_MOUSEMOVE => {
                    if self.mouse.is_none() {
                        let mut t = TRACKMOUSEEVENT {
                            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: self.hwnd,
                            dwHoverTime: 0,
                        };
                        let _ = TrackMouseEvent(&mut t);
                    }
                    let p = self.point(lparam);
                    let (width, _) = self.renderer.size();
                    let before = self.mouse.map(|(x, y)| button_at(x, y, width));
                    self.mouse = Some(p);
                    if before != Some(button_at(p.0, p.1, width)) {
                        self.draw();
                    }
                }
                WM_MOUSELEAVE => {
                    self.mouse = None;
                    self.draw();
                }
                WM_LBUTTONDOWN => {
                    let (x, y) = self.point(lparam);
                    let (width, _) = self.renderer.size();
                    if let Some(b) = button_at(x, y, width) {
                        self.pressed = Some(b);
                        SetCapture(self.hwnd);
                        self.draw();
                    } else {
                        // Drag the window by the video.
                        let _ = ReleaseCapture();
                        SendMessageW(self.hwnd, WM_SYSCOMMAND, Some(WPARAM((SC_MOVE | 2) as usize)), Some(LPARAM(0)));
                    }
                }
                WM_LBUTTONUP => {
                    if let Some(b) = self.pressed.take() {
                        let _ = ReleaseCapture();
                        let (x, y) = self.point(lparam);
                        let (width, _) = self.renderer.size();
                        if button_at(x, y, width) == Some(b) {
                            match b {
                                Button::Close => self.hide(),
                                Button::Minimize => {
                                    let _ = ShowWindow(self.hwnd, SW_MINIMIZE);
                                }
                            }
                        }
                        self.draw();
                    }
                }
                WM_LBUTTONDBLCLK => {
                    let (x, y) = self.point(lparam);
                    let (width, _) = self.renderer.size();
                    if button_at(x, y, width).is_none() {
                        let _ = ShowWindow(self.hwnd, if IsZoomed(self.hwnd).as_bool() { SW_RESTORE } else { SW_MAXIMIZE });
                    }
                }
                WM_KEYDOWN if wparam.0 == VK_ESCAPE.0 as usize => self.hide(),
                WM_CLOSE => self.hide(),
                WM_SHOWWINDOW => self.update_visible(),
                WM_APP_FRAME => {
                    self.shared.frame_pending.store(false, Ordering::Release);
                    self.follow_aspect();
                    self.draw();
                }
                WM_APP_STATUS => self.draw(),
                WM_APP_SHOW => self.show(),
                WM_APP_EXIT => self.exit(),
                WM_APP_TRAY => match (lparam.0 & 0xffff) as u32 {
                    WM_LBUTTONUP => {
                        if IsWindowVisible(self.hwnd).as_bool() && !IsIconic(self.hwnd).as_bool() {
                            self.hide();
                        } else {
                            self.show();
                        }
                    }
                    WM_RBUTTONUP | WM_CONTEXTMENU => self.tray_menu(),
                    _ => {}
                },
                m if m == self.taskbar_created && m != 0 => self.tray(NIM_ADD),
                _ => return None,
            }
            Some(LRESULT(0))
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            // The whole window is client area. Maximised, Windows makes the window larger than the
            // screen by the frame it expects; keep the content on the screen.
            WM_NCCALCSIZE if wparam.0 != 0 => {
                if IsZoomed(hwnd).as_bool() {
                    let p = &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS);
                    let dpi = GetDpiForWindow(hwnd);
                    let pad = GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
                    let fx = GetSystemMetricsForDpi(SM_CXFRAME, dpi) + pad;
                    let fy = GetSystemMetricsForDpi(SM_CYFRAME, dpi) + pad;
                    let r = &mut p.rgrc[0];
                    r.left += fx;
                    r.right -= fx;
                    r.top += fy;
                    r.bottom -= fy;
                }
                return LRESULT(0);
            }
            WM_NCHITTEST => {
                if IsZoomed(hwnd).as_bool() {
                    return LRESULT(HTCLIENT as isize);
                }
                let x = (lparam.0 & 0xffff) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                let mut r = RECT::default();
                let _ = GetWindowRect(hwnd, &mut r);
                let c = CORNER * GetDpiForWindow(hwnd) as i32 / 96;
                let (left, right) = (x < r.left + c, x >= r.right - c);
                let (top, bottom) = (y < r.top + c, y >= r.bottom - c);
                let hit = match (left, right, top, bottom) {
                    (true, _, true, _) => HTTOPLEFT,
                    (_, true, true, _) => HTTOPRIGHT,
                    (true, _, _, true) => HTBOTTOMLEFT,
                    (_, true, _, true) => HTBOTTOMRIGHT,
                    _ => HTCLIENT,
                };
                return LRESULT(hit as isize);
            }
            WM_GETMINMAXINFO => {
                let m = &mut *(lparam.0 as *mut MINMAXINFO);
                let s = GetDpiForWindow(hwnd).max(96) as i32;
                m.ptMinTrackSize = POINT { x: 320 * s / 96, y: 180 * s / 96 };
                return LRESULT(0);
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_DESTROY => return LRESULT(0),
            _ => {}
        }
        let win = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Win;
        if !win.is_null() {
            if let Some(r) = (*win).handle(msg, wparam, lparam) {
                return r;
            }
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}
