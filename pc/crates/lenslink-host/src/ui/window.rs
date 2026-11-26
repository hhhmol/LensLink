//! The window thread: a borderless window, its tray icon and its message loop.
//!
//! The window has no frame of its own (`WM_NCCALCSIZE` hands the whole rectangle to the client), but
//! keeps a thick-frame style so Windows still gives it a shadow, rounded corners, snapping and
//! maximising. It is moved by dragging anywhere on the video and resized at the corners: the edges
//! are left free for the settings curtains that come next. Closing hides it in the tray.
//!
//! Drawing happens on demand: a new frame, the pointer, a status. While something animates, the
//! loop draws once per display refresh, paced by the swap chain.
//!
//! A press goes, in order, to a caption button, to a control on a widget or a curtain (or the
//! nearest one within its invisible halo), to an item's name on a curtain (which pulls the item out
//! as a widget), to a widget (which moves it), to a curtain's edge, and otherwise moves the window.

use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR, Result};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::ID2D1Factory1;
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::{MonitorFromRect, ScreenToClient, ValidateRect, MONITOR_DEFAULTTONULL};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::INFINITE;
use windows::Win32::UI::Controls::{MARGINS, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::curtain::{Curtains, Side, DEPTH_END, DEPTH_SIDE, SIDES};
use super::overlay::{button_at, Area, Button, Input, Overlay};
use super::atlas::Group;
use super::panel::{self, Control, Item, Key, Values, AUTOMATIC, CAMERA, VIDEO};
use super::render::{letterbox, Element, Frame, Label, Renderer};
use super::widgets::Widgets;
use super::Info;
use lenslink_vcam::modes::Mode;
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
/// Timers: animation while Windows runs its own loop for moving and sizing; the end of live video.
const TIMER_ANIMATE: usize = 1;
const TIMER_LIVE: usize = 2;

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
    overlay: Overlay,
    /// Something changed since the last frame.
    dirty: bool,
    /// The overlay is moving: keep drawing.
    animating: bool,
    last_step: Option<Instant>,
    /// The modal loop Windows runs while the window is moved or sized is running instead of ours.
    sizing: bool,
    curtains: Curtains,
    values: Values,
    /// Each side's panel laid out for `panels_size`.
    panels: [Vec<Item>; 4],
    panels_size: (f32, f32),
    panels_changed: bool,
    /// What the left button holds.
    grab: Grab,
    /// When the pointer last moved, for the speed of a flick.
    last_move: Instant,
    widgets: Widgets,
    /// Each widget's contents and width.
    widget_items: Vec<(Vec<Item>, f32)>,
    /// Widgets to put on the video once its size is known.
    pending: Option<Pending>,
}

enum Pending {
    /// Saved: item and centre as a fraction of the window.
    Saved(Vec<(Key, f32, f32)>),
    /// First start: the prototype's three.
    Defaults,
}

/// Where items sit: a curtain's panel or a widget.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Place {
    Panel(Side),
    Widget(usize),
}

#[derive(Clone, Copy, PartialEq)]
enum Grab {
    None,
    Curtain,
    Slider { place: Place, key: Key },
    Widget,
}

/// Where `side`'s panel sits in the window: it moves with its curtain's mean depth.
fn panel_origin(side: Side, curtains: &Curtains, size: (f32, f32)) -> (f32, f32) {
    let m = curtains.edge(side).mean();
    match side {
        Side::Left => (m - DEPTH_SIDE, 0.0),
        Side::Right => (size.0 - m, 0.0),
        Side::Top => (0.0, m - DEPTH_END),
        Side::Bottom => (0.0, size.1 - m),
    }
}

pub fn run(shared: Arc<Shared>, device: Com<ID3D11Device>, factory: Com<ID2D1Factory1>, hidden: bool, ready: Sender<std::result::Result<(), String>>) {
    match create(shared.clone(), &device.0, &factory.0, hidden) {
        Ok(win) => {
            let _ = ready.send(Ok(()));
            message_loop(win.hwnd);
            drop(win);
        }
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
        }
    }
    shared.quit.store(true, Ordering::Release);
    shared.hwnd.store(0, Ordering::Release);
}

/// Messages, and a frame whenever one is due and the swap chain takes it.
fn message_loop(hwnd: HWND) {
    unsafe {
        let mut msg = MSG::default();
        loop {
            // The same pointer the window procedure uses; no reference outlives a dispatch.
            let win = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Win;
            if win.is_null() {
                return;
            }
            let due = (*win).due();
            let waitable = [(*win).renderer.waitable()];
            let handles: &[HANDLE] = if due { &waitable } else { &[] };
            let r = MsgWaitForMultipleObjectsEx(Some(handles), INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            if due && r == WAIT_OBJECT_0 {
                (*win).draw();
            }
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
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
            overlay: Overlay::default(),
            dirty: true,
            animating: false,
            last_step: None,
            sizing: false,
            curtains: Curtains::default(),
            values: Values::load(&settings),
            panels: Default::default(),
            panels_size: (0.0, 0.0),
            panels_changed: true,
            grab: Grab::None,
            last_move: Instant::now(),
            widgets: Widgets::default(),
            widget_items: Vec::new(),
            pending: Some(match &settings.widgets {
                Some(list) => Pending::Saved(
                    list.iter().filter_map(|(k, x, y)| Some((panel::key_by_name(k)?, *x as f32, *y as f32))).collect(),
                ),
                None => Pending::Defaults,
            }),
        });
        // The stream starts from what the window shows.
        win.push_controls();
        win.push_video();
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
        self.invalidate();
    }

    /// Into the tray: the phone's camera turns off unless an app uses the virtual camera.
    fn hide(&mut self) {
        self.save();
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        self.mouse = None;
        self.pressed = None;
        self.end_grab();
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

    /// The video the curtains leave free, where the buttons and the status float.
    fn area(&self) -> Area {
        let (w, h) = self.renderer.size();
        let m = |side: Side| self.curtains.edge(side).mean().max(0.0);
        (m(Side::Left), m(Side::Top), w - m(Side::Right), h - m(Side::Bottom))
    }

    /// Where a place's items start in the window.
    fn origin(&self, place: Place) -> (f32, f32) {
        match place {
            Place::Panel(side) => panel_origin(side, &self.curtains, self.renderer.size()),
            Place::Widget(i) => self.widgets.list[i].origin(),
        }
    }

    fn items(&self, place: Place) -> &[Item] {
        match place {
            Place::Panel(side) => &self.panels[side.index()],
            Place::Widget(i) => self.widget_items.get(i).map_or(&[], |w| &w.0),
        }
    }

    /// What is under (x, y): the widget on top, else the curtains showing there.
    fn places_at(&self, x: f32, y: f32) -> Vec<Place> {
        if let Some(i) = self.widgets.at(x, y).filter(|&i| i < self.widget_items.len()) {
            return vec![Place::Widget(i)];
        }
        let size = self.renderer.size();
        SIDES
            .into_iter()
            .filter(|&side| self.curtains.edge(side).shows() && self.curtains.inside(side, x, y, size))
            .map(Place::Panel)
            .collect()
    }

    /// The control under (x, y), or the nearest one whose halo it is in.
    fn control_at(&self, x: f32, y: f32) -> Option<(Place, Control)> {
        let mut best: Option<(Place, Control, f32)> = None;
        for place in self.places_at(x, y) {
            let (ox, oy) = self.origin(place);
            if let Some((c, d)) = panel::control_at(self.items(place), x - ox, y - oy) {
                if best.is_none_or(|b| d < b.2) {
                    best = Some((place, c, d));
                }
            }
        }
        best.map(|(p, c, _)| (p, c))
    }

    /// The item whose name on a curtain is at (x, y): grabbed there, it comes out as a widget.
    fn label_at(&self, x: f32, y: f32) -> Option<Key> {
        self.places_at(x, y).into_iter().find_map(|place| match place {
            Place::Panel(_) => {
                let (ox, oy) = self.origin(place);
                panel::label_at(self.items(place), x - ox, y - oy)
            }
            Place::Widget(_) => None,
        })
    }

    /// A setting changed: redraw, and tell the stream if it concerns the phone.
    fn changed(&mut self, key: Key) {
        self.panels_changed = true;
        if CAMERA.contains(&key) {
            self.push_controls();
        }
        if VIDEO.contains(&key) {
            self.push_video();
        }
        self.invalidate();
    }

    fn set_value(&mut self, key: Key, value: f32) {
        if self.values.set(key, value) {
            self.changed(key);
        }
    }

    /// The camera settings, for the stream to send to the phone.
    fn push_controls(&self) {
        let mut st = self.shared.state();
        st.controls = (st.controls.0 + 1, Some(self.values.phone_controls()));
    }

    /// The video settings, for the stream to start the phone with.
    fn push_video(&self) {
        let (width, height, fps, bitrate) = self.values.video();
        self.shared.state().chosen = Some((Mode { width, height, fps }, bitrate));
    }

    /// The pointer drags a slider: its thumb follows.
    fn slide(&mut self, place: Place, key: Key, x: f32) {
        let (ox, _) = self.origin(place);
        let rect = self.items(place).iter().find_map(|i| match i {
            Item::Slider { key: k, rect } if *k == key => Some(*rect),
            _ => None,
        });
        let Some(rect) = rect else { return };
        if panel::slide(&mut self.values, key, rect, x - ox) {
            self.changed(key);
        }
    }

    /// The settings changed for good (a slider let go, an option chosen): keep them.
    fn store_values(&self) {
        let mut s = Settings::load();
        self.values.store(&mut s);
        s.save();
    }

    fn store_widgets(&self) {
        let size = self.renderer.size();
        let mut s = Settings::load();
        let list = self.widgets.save(size).into_iter().map(|(k, x, y)| (panel::row(k).name.to_string(), x, y)).collect();
        s.widgets = Some(list);
        s.save();
    }

    /// Half the width `key` takes as a widget now.
    fn widget_half(&self, key: Key) -> f32 {
        let measure = |t: &str, f| self.renderer.measure_panel(t, f);
        panel::widget(key, &self.values, &measure).1 / 2.0
    }

    /// A press at (x, y): a control, an item's name, a widget, a curtain, or else the window itself.
    fn press(&mut self, x: f32, y: f32) -> bool {
        let size = self.renderer.size();
        if let Some((place, control)) = self.control_at(x, y) {
            match control {
                Control::Slider { key, .. } => {
                    self.grab = Grab::Slider { place, key };
                    self.slide(place, key, x);
                }
                Control::Option { key, index, .. } => {
                    if let Some((_, value)) = self.values.options(key).get(index) {
                        self.set_value(key, *value);
                        self.store_values();
                    }
                }
            }
        } else if let Some(key) = self.label_at(x, y) {
            let half = self.widget_half(key);
            if self.widgets.pull(key, x, y, half) {
                self.grab = Grab::Widget;
            }
        } else if let Some(i) = self.widgets.at(x, y) {
            self.widgets.hold(i, x, y);
            self.grab = Grab::Widget;
        } else if let Some(side) = self.curtains.pick(x, y, size) {
            self.curtains.press(side, x, y, size);
            self.grab = Grab::Curtain;
            self.last_move = Instant::now();
        } else {
            return false;
        }
        unsafe {
            SetCapture(self.hwnd);
        }
        self.invalidate();
        true
    }

    /// A double click: on a slider the camera can set by itself, it gives the setting back to the
    /// camera; elsewhere it is just another press.
    fn double_click(&mut self, x: f32, y: f32) -> bool {
        if let Some((_, Control::Slider { key, .. })) = self.control_at(x, y) {
            if AUTOMATIC.contains(&key) {
                if self.values.set_auto(key, true) {
                    self.changed(key);
                    self.store_values();
                }
                return true;
            }
        }
        self.press(x, y)
    }

    fn end_grab(&mut self) {
        match std::mem::replace(&mut self.grab, Grab::None) {
            Grab::Curtain => self.curtains.release(&self.values.motion(), self.renderer.size()),
            Grab::Slider { .. } => self.store_values(),
            Grab::Widget => {
                if let Some(i) = self.widgets.release() {
                    // Dropped onto an open curtain, a widget goes back into it.
                    let size = self.renderer.size();
                    let w = &self.widgets.list[i];
                    let (x, y) = (w.x, w.y);
                    let back = SIDES.into_iter().any(|side| {
                        let (_, v) = side.uv(x, y, size);
                        self.curtains.edge(side).open && v < self.curtains.edge_at(side, x, y, size) - 12.0
                    });
                    if back {
                        self.widgets.remove(i);
                        if i < self.widget_items.len() {
                            self.widget_items.remove(i);
                        }
                    }
                }
                self.store_widgets();
            }
            Grab::None => return,
        }
        unsafe {
            let _ = ReleaseCapture();
        }
        self.invalidate();
    }

    /// Where the pointer is now, in DIPs. Asked when the cursor is set: the last mouse message may
    /// be a move behind, which matters at a screen's edge where the pointer stops (a maximised
    /// window's curtains).
    fn pointer(&self) -> Option<(f32, f32)> {
        unsafe {
            let mut p = POINT::default();
            GetCursorPos(&mut p).ok()?;
            ScreenToClient(self.hwnd, &mut p).ok().ok()?;
            let s = 96.0 / self.dpi();
            Some((p.x as f32 * s, p.y as f32 * s))
        }
    }

    /// Shows the cursor for the pointer at (x, y) at once.
    fn set_cursor(&self, x: f32, y: f32) {
        unsafe {
            if let Ok(c) = LoadCursorW(None, self.cursor(x, y)) {
                SetCursor(Some(c));
            }
        }
    }

    /// The cursor for the pointer at (x, y).
    fn cursor(&self, x: f32, y: f32) -> PCWSTR {
        let size = self.renderer.size();
        let along = |side: Side| if side.index() < 2 { IDC_SIZEWE } else { IDC_SIZENS };
        match self.grab {
            Grab::Slider { .. } => IDC_SIZEWE,
            Grab::Curtain | Grab::Widget => IDC_SIZEALL,
            Grab::None if button_at(x, y, self.area()).is_some() || self.control_at(x, y).is_some() => IDC_HAND,
            Grab::None if self.label_at(x, y).is_some() || self.widgets.at(x, y).is_some() => IDC_SIZEALL,
            Grab::None => self.curtains.pick(x, y, size).map_or(IDC_ARROW, along),
        }
    }

    /// What the widgets show, from what the stream tells.
    fn update_info(&mut self, info: &Info) {
        let dash = || "—".to_string();
        let mode = info.mode.map_or_else(dash, |m| {
            let name = match m.height {
                2160 => "4K".to_string(),
                h => format!("{h}p"),
            };
            format!("{name} · {} к/с", m.fps)
        });
        let s = info.stats;
        let time = s.map_or_else(dash, |s| {
            let t = s.seconds as u64;
            if t >= 3600 {
                format!("{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
            } else {
                format!("{}:{:02}", t / 60, t % 60)
            }
        });
        let texts = [
            (Key::Phone, info.phone.clone()),
            (Key::Mode, mode),
            (Key::Time, time),
            (Key::Fps, s.map_or_else(dash, |s| format!("{:.1}", s.fps))),
            (Key::Bitrate, s.map_or_else(dash, |s| format!("{:.1} Мбит/с", s.mbits))),
            (Key::Keyframes, s.map_or_else(dash, |s| s.keyframes.to_string())),
            (Key::Gap, s.map_or_else(dash, |s| format!("{:.0} мс", s.gap_ms))),
            (Key::Temperature, info.temperature.map_or_else(dash, |t| format!("{t:.1} °C"))),
            (Key::Battery, info.battery.map_or_else(dash, |(b, charging)| format!("{b} %{}", if charging { " · зарядка" } else { "" }))),
        ];
        let mut changed = false;
        for (k, t) in texts {
            changed |= self.values.set_info(k, &t);
        }
        // The phone's own ranges.
        changed |= self.values.set_limits(Key::Zoom, info.zoom);
        changed |= self.values.set_limits(Key::Exposure, info.ev);
        changed |= self.values.set_limits(Key::Iso, info.iso);
        changed |= self.values.set_limits(Key::Quality, info.max_mbits.map(|m| (2.0, m.floor())));
        if !info.modes.is_empty() && self.values.set_modes(&info.modes) {
            changed = true;
            self.push_video();
        }
        self.panels_changed |= changed;
    }

    /// Puts the saved widgets (or the first start's) on the video.
    fn place_widgets(&mut self, size: (f32, f32)) {
        let Some(pending) = self.pending.take() else { return };
        match pending {
            Pending::Saved(list) => {
                for (key, fx, fy) in list {
                    let half = self.widget_half(key);
                    self.widgets.add(key, fx * size.0, fy * size.1, half);
                }
            }
            Pending::Defaults => {
                let hh = panel::WIDGET_HEIGHT / 2.0;
                let (phone, fps, zoom) = (self.widget_half(Key::Phone), self.widget_half(Key::Fps), self.widget_half(Key::Zoom));
                self.widgets.add(Key::Phone, size.0 - 60.0 - phone, 60.0 + hh, phone);
                self.widgets.add(Key::Fps, size.0 - 60.0 - fps, 60.0 + 3.0 * hh + 12.0, fps);
                self.widgets.add(Key::Zoom, size.0 / 2.0, size.1 - 60.0 - hh, zoom);
            }
        }
    }

    /// A frame is wanted: drawn when the swap chain is ready (at once inside Windows' modal loop).
    fn invalidate(&mut self) {
        self.dirty = true;
        if self.sizing {
            self.draw();
        }
    }

    fn due(&self) -> bool {
        (self.dirty || self.animating) && self.shared.visible.load(Ordering::Acquire)
    }

    fn draw(&mut self) {
        self.dirty = false;
        if !self.shared.visible.load(Ordering::Acquire) {
            self.animating = false;
            self.last_step = None;
            return;
        }
        let now = Instant::now();
        let dt = self.last_step.map_or(1.0 / 60.0, |t| (now - t).as_secs_f32()).clamp(1.0 / 240.0, 1.0 / 30.0);
        self.last_step = Some(now);

        let size = self.renderer.size();
        let pixels = self.renderer.pixels();
        let (video, status, live) = {
            let mut st = self.shared.state();
            let live = st.last_frame.is_some_and(|t| t.elapsed() < LIVE);
            // Frames are scaled on the GPU to exactly the pixels the video covers.
            let r = letterbox(st.size, pixels);
            st.want = (r[2] as u32, r[3] as u32);
            (st.video.as_ref().map(|v| (v.0.clone(), st.size)), st.status.clone(), live)
        };
        let area = self.area();
        let over = self.mouse.and_then(|(x, y)| button_at(x, y, area));
        let input = Input {
            area,
            hover: self.mouse.is_some(),
            hot: if self.pressed.is_some() { self.pressed.filter(|&b| over == Some(b)) } else { over },
            pressed: self.pressed.filter(|&b| over == Some(b)),
            live,
            status: &status,
        };
        let info = self.shared.state().info.clone();
        self.update_info(&info);
        if self.panels_size != size && self.panels_size.0 > 0.0 {
            self.widgets.rescale(self.panels_size, size);
        }
        if size.0 > 0.0 {
            self.place_widgets(size);
        }
        let renderer = &self.renderer;
        let mut animating = self.overlay.step(dt, &input, |t, large, max| renderer.measure(t, large, max));
        animating |= self.widgets.step(dt);
        // The pointer makes an edge bulge only when it is free and not on a button.
        let hover = self.mouse.filter(|_| self.grab == Grab::None && over.is_none() && self.pressed.is_none());
        animating |= self.curtains.step(dt, &self.values.motion(), hover, size);
        self.animating = animating;
        if !self.animating {
            self.last_step = None;
        }
        if self.panels_size != size || self.panels_changed {
            let renderer = &self.renderer;
            let measure = |t: &str, f| renderer.measure_panel(t, f);
            let values = &self.values;
            self.panels = SIDES.map(|side| panel::layout(side, size, values, &measure));
            self.panels_size = size;
            self.panels_changed = true;
        }
        // The widgets' contents follow their values.
        let measure = |t: &str, f| self.renderer.measure_panel(t, f);
        self.widget_items = self.widgets.list.iter().map(|w| panel::widget(w.key, &self.values, &measure)).collect();
        for (w, (_, width)) in self.widgets.list.iter_mut().zip(&self.widget_items) {
            w.half_width = width / 2.0;
        }
        let mut elements: Vec<Element> = self
            .widgets
            .list
            .iter()
            .map(|w| Element { centre: (w.x, w.y), half: w.stretched(), tint: [0.0; 4], label: Label::None, label_alpha: 0.0 })
            .collect();
        elements.extend(self.overlay.elements(self.area()));
        let groups: Vec<Group> = self
            .widgets
            .list
            .iter()
            .zip(&self.widget_items)
            .map(|(w, (items, _))| Group { origin: w.origin(), clip: None, items })
            .collect();
        let frame = Frame {
            video: video.as_ref().map(|(t, s)| (t, *s)),
            elements: &elements,
            curtains: &self.curtains,
            values: &self.values,
            panels: &self.panels,
            panels_changed: self.panels_changed,
            widgets: &groups,
        };
        if let Err(e) = self.renderer.draw(&frame) {
            eprintln!("window: draw: {e}");
        }
        self.panels_changed = false;
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
                    // Straight away: Windows shows the new size as soon as this returns.
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
                    self.invalidate();
                }
                WM_ENTERSIZEMOVE => {
                    self.sizing = true;
                    SetTimer(Some(self.hwnd), TIMER_ANIMATE, 15, None);
                }
                WM_EXITSIZEMOVE => {
                    self.sizing = false;
                    let _ = KillTimer(Some(self.hwnd), TIMER_ANIMATE);
                }
                WM_TIMER if wparam.0 == TIMER_ANIMATE => {
                    if self.due() {
                        self.draw();
                    }
                }
                WM_TIMER if wparam.0 == TIMER_LIVE => {
                    let _ = KillTimer(Some(self.hwnd), TIMER_LIVE);
                    self.invalidate();
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
                    self.mouse = Some(p);
                    let now = Instant::now();
                    let dt = (now - self.last_move).as_secs_f32();
                    self.last_move = now;
                    match self.grab {
                        Grab::Slider { place, key } => self.slide(place, key, p.0),
                        Grab::Curtain => self.curtains.drag_to(p.0, p.1, dt, self.renderer.size()),
                        Grab::Widget => self.widgets.drag_to(p.0, p.1, self.renderer.size()),
                        Grab::None => {}
                    }
                    // The edges follow the pointer, and the cursor shows what a press would take.
                    self.set_cursor(p.0, p.1);
                    self.invalidate();
                }
                WM_SETCURSOR if (lparam.0 & 0xffff) as u32 == HTCLIENT => {
                    if let Some((x, y)) = self.pointer() {
                        self.set_cursor(x, y);
                        return Some(LRESULT(1));
                    }
                    return None;
                }
                WM_CAPTURECHANGED if self.grab != Grab::None && lparam.0 != self.hwnd.0 as isize => self.end_grab(),
                WM_MOUSELEAVE => {
                    self.mouse = None;
                    self.invalidate();
                }
                WM_LBUTTONDOWN => {
                    let (x, y) = self.point(lparam);
                    if let Some(b) = button_at(x, y, self.area()) {
                        self.pressed = Some(b);
                        SetCapture(self.hwnd);
                        self.invalidate();
                    } else if self.press(x, y) {
                    } else {
                        // Drag the window by the video.
                        let _ = ReleaseCapture();
                        SendMessageW(self.hwnd, WM_SYSCOMMAND, Some(WPARAM((SC_MOVE | 2) as usize)), Some(LPARAM(0)));
                    }
                }
                WM_LBUTTONUP => {
                    self.end_grab();
                    if let Some(b) = self.pressed.take() {
                        let _ = ReleaseCapture();
                        let (x, y) = self.point(lparam);
                        if button_at(x, y, self.area()) == Some(b) {
                            match b {
                                Button::Close => self.hide(),
                                Button::Minimize => {
                                    let _ = ShowWindow(self.hwnd, SW_MINIMIZE);
                                }
                            }
                        }
                        self.invalidate();
                    }
                }
                WM_LBUTTONDBLCLK => {
                    let (x, y) = self.point(lparam);
                    // A second click on a control or a curtain is just another click.
                    if button_at(x, y, self.area()).is_none() && !self.double_click(x, y) {
                        let _ = ShowWindow(self.hwnd, if IsZoomed(self.hwnd).as_bool() { SW_RESTORE } else { SW_MAXIMIZE });
                    }
                }
                WM_KEYDOWN if wparam.0 == VK_ESCAPE.0 as usize => self.hide(),
                WM_CLOSE => self.hide(),
                WM_SHOWWINDOW => self.update_visible(),
                WM_APP_FRAME => {
                    self.shared.frame_pending.store(false, Ordering::Release);
                    self.follow_aspect();
                    // When frames stop, the status comes back to the middle.
                    SetTimer(Some(self.hwnd), TIMER_LIVE, LIVE.as_millis() as u32 + 50, None);
                    self.invalidate();
                }
                WM_APP_STATUS => self.invalidate(),
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
