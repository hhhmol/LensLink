//! `lenslink-host stream`: connect to the phone, start the camera and play or save the video.

use std::io::{self, BufReader, IsTerminal, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use lenslink_protocol::{self as proto, flags, Kind, ProtocolError};
use serde_json::{json, Value};

use crate::adb;
use crate::{controls, modes};
use crate::video::{self, Viewer};
use lenslink_vcam::modes::Mode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    H264,
    H265,
}

impl Codec {
    pub fn wire_name(self) -> &'static str {
        match self {
            Codec::H264 => "h264",
            Codec::H265 => "h265",
        }
    }

    /// Demuxer name for ffplay / ffmpeg.
    fn ffmpeg_format(self) -> &'static str {
        match self {
            Codec::H264 => "h264",
            Codec::H265 => "hevc",
        }
    }
}

/// Where the live picture goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Player {
    /// Our own window: GPU decode, lowest latency.
    Window,
    /// ffplay, kept for comparison and for machines without a hardware decoder.
    Ffplay,
    /// The LensLink app's window, on its own thread, hidden in the tray when closed.
    App,
    None,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub serial: Option<String>,
    pub port: u16,
    pub camera: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: Codec,
    /// `None`: chosen per mode.
    pub bitrate: Option<u32>,
    pub zoom: Option<f32>,
    pub stabilization: bool,
    pub player: Player,
    pub virtual_camera: bool,
    pub out: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            serial: None,
            port: proto::DEFAULT_TCP_PORT,
            camera: "0".into(),
            width: 1920,
            height: 1080,
            fps: 30,
            codec: Codec::H264,
            bitrate: None,
            zoom: None,
            stabilization: true,
            player: Player::Window,
            virtual_camera: false,
            out: None,
        }
    }
}

pub const USAGE: &str = "\
stream options:
  -s, --serial <id>      phone serial when several are connected (see `adb devices`)
  --camera <id>          camera id (default 0)
  --size <WxH>           e.g. 1920x1080 (default), 3840x2160
  --fps <n>              frames per second (default 30)
  --codec <h264|h265>    default h264
  --bitrate <n[k|M]>     e.g. 20M (default: by mode, 20M at 1080p30)
  --zoom <ratio>         logical-camera zoom, e.g. 0.6 for ultrawide
  --no-stabilization     disable EIS/OIS
  --out <file>           also save the raw stream (.h264 / .h265)
  --vcam                 also publish the video as the \"LensLink\" virtual camera
  --ffplay               show the video in ffplay instead of the LensLink window
  --no-play              no window (use with --vcam or --out)";

pub fn parse(args: &[String]) -> Result<Options, String> {
    let mut o = Options::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "-s" | "--serial" => o.serial = Some(value(arg)?),
            "--camera" => o.camera = value(arg)?,
            "--size" => {
                let v = value(arg)?;
                let (w, h) = v
                    .split_once(['x', 'X'])
                    .ok_or_else(|| format!("bad size {v}, expected WxH"))?;
                o.width = w.parse().map_err(|_| format!("bad width in {v}"))?;
                o.height = h.parse().map_err(|_| format!("bad height in {v}"))?;
            }
            "--fps" => o.fps = value(arg)?.parse().map_err(|_| "bad --fps".to_string())?,
            "--codec" => {
                o.codec = match value(arg)?.to_ascii_lowercase().as_str() {
                    "h264" | "avc" => Codec::H264,
                    "h265" | "hevc" => Codec::H265,
                    other => return Err(format!("unknown codec {other}")),
                }
            }
            "--bitrate" => o.bitrate = Some(parse_bitrate(&value(arg)?)?),
            "--zoom" => o.zoom = Some(value(arg)?.parse().map_err(|_| "bad --zoom".to_string())?),
            "--no-stabilization" => o.stabilization = false,
            "--out" => o.out = Some(PathBuf::from(value(arg)?)),
            "--vcam" => o.virtual_camera = true,
            "--ffplay" => o.player = Player::Ffplay,
            "--no-play" => o.player = Player::None,
            other => return Err(format!("unknown option {other}")),
        }
    }
    if o.player == Player::None && o.out.is_none() && !o.virtual_camera {
        return Err("--no-play without --out or --vcam would discard the video".into());
    }
    Ok(o)
}

fn parse_bitrate(s: &str) -> Result<u32, String> {
    let s = s.trim();
    let (digits, mult) = match s.chars().last() {
        Some('k' | 'K') => (&s[..s.len() - 1], 1_000.0),
        Some('m' | 'M') => (&s[..s.len() - 1], 1_000_000.0),
        _ => (s, 1.0),
    };
    let n: f64 = digits.parse().map_err(|_| format!("bad bitrate {s}"))?;
    Ok((n * mult) as u32)
}

/// Component that starts the phone's streaming service without user interaction (see
/// `AutostartActivity` in the Android app).
const AUTOSTART: &str = "io.github.lenslink/.AutostartActivity";
/// How often the phone app may be launched while it does not come up (locked phone, before first
/// unlock after a reboot).
const LAUNCH_INTERVAL: Duration = Duration::from_secs(10);
const STOPPED: &str = "streaming was stopped on the phone: tap \"Start streaming service\" in LensLink \
                       to continue";
const IDLE: &str = "phone ready; its camera turns on when an app opens LensLink";
/// The phone camera stays on this long after the last app stopped watching: apps often close and
/// reopen the camera (switching modes, settings previews).
const IDLE_GRACE: Duration = Duration::from_secs(5);
/// A started stream that delivers nothing for this long is treated as lost.
const STALL: Duration = Duration::from_secs(5);

/// Streams until the user quits, reconnecting whenever the phone goes away: cable pulled, phone
/// rebooted, app stopped. The window and the virtual camera stay up meanwhile.
pub fn run(o: &Options) -> Result<(), String> {
    let file = match &o.out {
        Some(path) => Some(
            std::fs::File::create(path).map_err(|e| format!("cannot create {}: {e}", path.display()))?,
        ),
        None => None,
    };
    let mode = Mode { width: o.width, height: o.height, fps: o.fps };
    let viewer = if matches!(o.player, Player::Window | Player::App) || o.virtual_camera {
        video::init()?;
        let outputs = video::Outputs {
            window: o.player == Player::Window,
            app: o.player == Player::App,
            virtual_camera: o.virtual_camera,
        };
        Some(Viewer::new(o.codec, mode, outputs)?)
    } else {
        None
    };
    // The app runs without the virtual camera when it is not installed.
    let without_camera;
    let o = if o.virtual_camera && !viewer.as_ref().is_some_and(Viewer::has_camera) {
        without_camera = Options { virtual_camera: false, ..o.clone() };
        &without_camera
    } else {
        o
    };
    let mut host = Host {
        o,
        viewer,
        file,
        output: mode,
        app_selected: false,
        offered: Vec::new(),
        last_launch: None,
        devices: None,
        user_stopped: false,
        said: String::new(),
    };
    loop {
        let Some(serial) = host.wait_for_phone()? else { return Ok(()) };
        match host.connect(&serial) {
            Ok(End::Quit) => return Ok(()),
            Ok(End::Lost(why, retry)) | Err(Fail::Retry(why, retry)) => {
                host.say(&why);
                if !host.idle(retry) {
                    return Ok(());
                }
            }
            Err(Fail::Fatal(e)) => return Err(e),
        }
    }
}

/// How one connection ended.
enum End {
    /// The user closed the window.
    Quit,
    /// Try again after the delay.
    Lost(String, Duration),
}

enum Fail {
    Retry(String, Duration),
    /// A problem on this PC that retrying will not fix.
    Fatal(String),
}

/// Connection problems are worth retrying.
impl From<String> for Fail {
    fn from(e: String) -> Self {
        Fail::Retry(e, Duration::from_secs(1))
    }
}

/// State that outlives one connection.
struct Host<'a> {
    o: &'a Options,
    viewer: Option<Viewer>,
    file: Option<std::fs::File>,
    /// What the virtual camera publishes: the mode an app selected. The phone streams the closest
    /// mode it offers.
    output: Mode,
    app_selected: bool,
    /// Modes of the connected phone, as last written for the virtual camera.
    offered: Vec<Mode>,
    last_launch: Option<Instant>,
    devices: Option<adb::DeviceWatch>,
    /// The user stopped streaming on the phone: do not start it again until the phone is reconnected
    /// or the user starts it there.
    user_stopped: bool,
    said: String,
}

impl Host<'_> {
    /// Status line, printed once per change.
    fn say(&mut self, text: &str) {
        if self.said != text {
            eprintln!("{text}");
            self.said = text.to_string();
        }
        if let Some(v) = &self.viewer {
            v.status(text);
        }
    }

    /// Whether the video goes anywhere now. Without a window, file or player that depends on apps
    /// using the virtual camera.
    fn watched(&mut self) -> bool {
        self.file.is_some()
            || self.o.player == Player::Ffplay
            || self.viewer.as_mut().is_some_and(Viewer::watched)
    }

    /// Keeps the window responsive for `d`. False once the user closed it.
    fn idle(&mut self, d: Duration) -> bool {
        let until = Instant::now() + d;
        while Instant::now() < until {
            if let Some(v) = self.viewer.as_mut() {
                if !v.pump() {
                    return false;
                }
                if let Some(r) = v.take_request() {
                    self.select(r);
                }
            }
            // A window needs its messages handled promptly; otherwise wake rarely.
            let window = self.viewer.as_ref().is_some_and(Viewer::has_window);
            std::thread::sleep(Duration::from_millis(if window { 15 } else { 100 }));
        }
        true
    }

    /// An app selected `r` on the virtual camera.
    fn select(&mut self, r: Mode) {
        eprintln!("an app selected {r}");
        self.output = r;
        self.app_selected = true;
        if let Some(v) = self.viewer.as_mut() {
            v.set_output(r);
        }
    }

    /// Serial of a phone ready for adb; None once the user closed the window.
    fn wait_for_phone(&mut self) -> Result<Option<String>, String> {
        loop {
            let Some(devices) = self.device_list()? else {
                if !self.idle(Duration::from_millis(100)) {
                    return Ok(None);
                }
                continue;
            };
            let wanted = |s: &str| self.o.serial.as_deref().is_none_or(|w| w == s);
            let mine: Vec<_> = devices.iter().filter(|(s, _)| wanted(s)).collect();
            if let Some((serial, _)) = mine.iter().find(|(_, state)| state == "device") {
                return Ok(Some(serial.clone()));
            }
            // A new connection is a fresh start.
            self.user_stopped = false;
            let text = if mine.iter().any(|(_, state)| state == "unauthorized") {
                "the phone asks to allow USB debugging: confirm on its screen"
            } else {
                "waiting for the phone: connect it with USB (USB debugging on)"
            };
            self.say(text);
            if !self.idle(Duration::from_millis(250)) {
                return Ok(None);
            }
        }
    }

    /// Devices from the running `adb track-devices` (restarted when adb quits); `None` until adb
    /// reported. Falls back to `adb devices` if the watch cannot run.
    fn device_list(&mut self) -> Result<Option<Vec<(String, String)>>, String> {
        if self.devices.is_none() {
            match adb::DeviceWatch::start() {
                Ok(w) => self.devices = Some(w),
                Err(_) => return adb::devices().map(Some),
            }
        }
        match self.devices.as_mut().unwrap().devices() {
            Ok(list) => Ok(list.map(<[_]>::to_vec)),
            Err(()) => {
                self.devices = None;
                adb::devices().map(Some)
            }
        }
    }

    fn connect(&mut self, serial: &str) -> Result<End, Fail> {
        let local = format!("tcp:{}", self.o.port);
        let remote = format!("localabstract:{}", proto::SOCKET_NAME);
        adb::check(Some(serial), &["forward", &local, &remote])?;
        let result = self.session(serial);
        let _ = adb::run(Some(serial), &["forward", "--remove", &local]);
        result
    }

    fn session(&mut self, serial: &str) -> Result<End, Fail> {
        let o = self.o;
        let sock = TcpStream::connect(("127.0.0.1", o.port)).map_err(|e| format!("connect: {e}"))?;
        sock.set_nodelay(true).ok();
        let mut writer = sock.try_clone().map_err(|e| e.to_string())?;
        let mut reader = BufReader::with_capacity(1 << 20, sock);

        proto::write_handshake(&mut writer).map_err(|e| e.to_string())?;
        match proto::read_handshake(&mut reader) {
            Ok(_) => {}
            // adb accepted the connection but nothing listens on the phone.
            Err(ProtocolError::Io(e)) if is_closed(&e) => return Ok(self.launch_app(serial)),
            Err(e) => return Err(e.to_string().into()),
        }
        self.last_launch = None;
        self.user_stopped = false;
        self.say("phone connected");

        let mut phone_mode = self.output;
        // The bit rate the window asks for, if any.
        let mut phone_bitrate = None;
        let app = o.player == Player::App;
        if o.virtual_camera || app {
            send_control(&mut writer, &json!({ "type": "get_capabilities" }))?;
            let (caps, others) = wait_for_capabilities(&mut reader)?;
            for m in &others {
                self.phone_message(m);
            }
            let offered = modes::from_capabilities(&caps, &o.camera, o.codec)
                .map_err(|e| Fail::Retry(e, Duration::from_secs(5)))?;
            self.camera_modes(offered).map_err(Fail::Fatal)?;
            let (want, bitrate) = self.wanted();
            phone_mode = modes::closest(&self.offered, want);
            phone_bitrate = bitrate;
            let descs = controls::from_capabilities(&caps, &o.camera);
            if o.virtual_camera {
                lenslink_vcam::controls::write(&descs)
                    .map_err(|e| Fail::Fatal(format!("cannot write {} ({e})", lenslink_vcam::controls::CONTROLS_PATH)))?;
                eprintln!("camera controls: {}", descs.iter().map(|d| d.control.name()).collect::<Vec<_>>().join(", "));
            }
            if let Some(a) = self.viewer.as_ref().and_then(Viewer::app) {
                let range = |c: lenslink_vcam::controls::Control, scale: f32| {
                    descs.iter().find(|d| d.control == c).map(|d| (d.min as f32 / scale, d.max as f32 / scale))
                };
                use lenslink_vcam::controls::Control;
                let modes = self.offered.clone();
                let max_mbits = modes::max_bitrate(&caps, o.codec).map(|b| b as f32 / 1e6);
                a.info(|i| {
                    i.zoom = range(Control::Zoom, 10.0);
                    i.ev = range(Control::Brightness, 10.0);
                    i.iso = range(Control::Gain, 1.0);
                    i.modes = modes;
                    i.max_mbits = max_mbits;
                });
            }
            if let Some(v) = self.viewer.as_mut() {
                v.set_controls(descs);
            }
        }
        // The window's camera settings go to the phone once it is connected, after what apps set.
        let mut app_controls_sent = 0;
        if let Some(v) = self.viewer.as_mut() {
            v.set_output(self.output);
            // The phone starts with automatic settings; restore what apps set.
            v.resend_controls();
        }
        let title = format!("LensLink {phone_mode} {}", o.codec.wire_name());
        let mut ffplay = if o.player == Player::Ffplay { Some(spawn_ffplay(o, &title).map_err(Fail::Fatal)?) } else { None };

        // The socket is read on its own thread so the window keeps responding while no frame arrives.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || loop {
            let r = proto::read_packet(&mut reader);
            let last = !matches!(r, Ok(Some(_)));
            if tx.send(r).is_err() || last {
                break;
            }
        });
        let result = (|| -> Result<End, Fail> {
            let mut stats = Stats::new();
            let mut last_video = Instant::now();
            // The phone camera runs only while something shows the video.
            let mut streaming = false;
            let mut last_watched = Instant::now().checked_sub(IDLE_GRACE).unwrap_or_else(Instant::now);
            if !self.watched() {
                self.say(IDLE);
            }
            loop {
                if let Some(v) = self.viewer.as_mut() {
                    if !v.pump() {
                        return Ok(End::Quit);
                    }
                }
                let mut restart = false;
                if let Some(r) = self.viewer.as_mut().and_then(Viewer::take_request) {
                    self.select(r);
                }
                // The window's choice, else what an app selected on the virtual camera.
                let (want, bitrate) = self.wanted();
                let p = modes::closest(&self.offered, want);
                if p != phone_mode || bitrate != phone_bitrate {
                    restart = streaming;
                    phone_mode = p;
                    phone_bitrate = bitrate;
                }
                if self.watched() {
                    last_watched = Instant::now();
                    restart |= !streaming;
                }
                if restart && last_watched.elapsed() < IDLE_GRACE {
                    send_control(&mut writer, &start_message(o, phone_mode, phone_bitrate))?;
                    if let Some(a) = self.viewer.as_ref().and_then(Viewer::app) {
                        a.info(|i| i.mode = Some(phone_mode));
                    }
                    if let Some(v) = self.viewer.as_mut() {
                        v.set_stream(phone_mode).map_err(|e| Fail::Fatal(format!("starting {phone_mode}: {e}")))?;
                    }
                    if !streaming {
                        self.say("streaming");
                    }
                    streaming = true;
                    last_video = Instant::now();
                } else if streaming && last_watched.elapsed() >= IDLE_GRACE {
                    send_control(&mut writer, &json!({ "type": "stop" }))?;
                    streaming = false;
                    if let Some(a) = self.viewer.as_ref().and_then(Viewer::app) {
                        a.info(|i| {
                            i.mode = None;
                            i.stats = None;
                        });
                    }
                    if io::stderr().is_terminal() {
                        eprintln!();
                    }
                    self.say(IDLE);
                }
                if let Some(values) = self.viewer.as_mut().and_then(Viewer::take_controls) {
                    let msg = controls::to_phone(&values);
                    eprintln!("controls: {msg}");
                    send_control(&mut writer, &msg)?;
                }
                if let Some(msg) = self.viewer.as_ref().and_then(Viewer::app).and_then(|a| a.take_controls(&mut app_controls_sent)) {
                    eprintln!("controls: {msg}");
                    send_control(&mut writer, &msg)?;
                }
                // Short waits keep the window responsive while streaming; idle, the loop barely runs.
                let wait = if streaming { 5 } else { 50 };
                let packet = match rx.recv_timeout(Duration::from_millis(wait)) {
                    Ok(Ok(Some(p))) => p,
                    Ok(Ok(None)) | Err(RecvTimeoutError::Disconnected) => {
                        return Ok(End::Lost("the phone disconnected".into(), Duration::from_secs(1)))
                    }
                    Ok(Err(e)) => return Ok(End::Lost(format!("connection: {e}"), Duration::from_secs(1))),
                    Err(RecvTimeoutError::Timeout) => {
                        if streaming && last_video.elapsed() > STALL {
                            return Ok(End::Lost("no video from the phone for 5 s".into(), Duration::from_secs(1)));
                        }
                        continue;
                    }
                };
                match packet.header.kind {
                    Kind::Control => {
                        let msg = control_message(&packet.payload)
                            .map_err(|e| Fail::Retry(e, Duration::from_secs(3)))?;
                        if msg["type"] == "stopped" {
                            self.user_stopped = true;
                            return Ok(End::Lost(STOPPED.into(), Duration::from_secs(2)));
                        }
                        self.phone_message(&msg);
                    }
                    Kind::Video => {
                        last_video = Instant::now();
                        let h = &packet.header;
                        if let Some(f) = self.file.as_mut() {
                            f.write_all(&packet.payload).map_err(|e| Fail::Fatal(format!("write file: {e}")))?;
                        }
                        if let Some(p) = ffplay.as_mut() {
                            let stdin = p.stdin.as_mut().expect("ffplay stdin is piped");
                            if stdin.write_all(&packet.payload).is_err() {
                                eprintln!("\nplayer window closed");
                                return Ok(End::Quit);
                            }
                        }
                        if let Some(v) = self.viewer.as_mut() {
                            v.packet(&packet.payload, h.pts_us, h.has(flags::CONFIG), h.has(flags::KEYFRAME))?;
                        }
                        if let Some((line, figures)) = stats.add(h, packet.payload.len()) {
                            if let Some(v) = self.viewer.as_ref() {
                                v.status(&line);
                                if let Some(a) = v.app() {
                                    a.info(|i| i.stats = Some(figures));
                                }
                            }
                        }
                    }
                    Kind::Audio | Kind::Unknown(_) => {}
                }
            }
        })();
        // Unblocks the reader thread.
        let _ = writer.shutdown(std::net::Shutdown::Both);
        if let Some(a) = self.viewer.as_ref().and_then(Viewer::app) {
            // What the phone can do stays known; what it does now goes.
            a.info(|i| {
                i.mode = None;
                i.stats = None;
                i.battery = None;
                i.temperature = None;
            });
        }
        if let Some(mut p) = ffplay {
            let _ = p.kill();
        }
        if io::stderr().is_terminal() {
            eprintln!();
        }
        self.said.clear();
        result
    }

    /// Nothing answers on the phone: start the LensLink service there.
    fn launch_app(&mut self, serial: &str) -> End {
        let retry = Duration::from_millis(1500);
        if self.user_stopped {
            // Only probe whether the user started it again.
            return End::Lost(STOPPED.into(), Duration::from_secs(2));
        }
        if self.last_launch.is_some_and(|t| t.elapsed() < LAUNCH_INTERVAL) {
            return End::Lost(
                "the LensLink app on the phone does not answer: unlock the phone, or open LensLink and \
                 tap \"Start streaming service\""
                    .into(),
                retry,
            );
        }
        self.last_launch = Some(Instant::now());
        match adb::run(Some(serial), &["shell", "am", "start", "-n", AUTOSTART]) {
            Ok(out) => {
                let text = String::from_utf8_lossy(&out.stdout) + String::from_utf8_lossy(&out.stderr);
                if text.contains("does not exist") {
                    let installed = adb::run(Some(serial), &["shell", "pm", "path", crate::APP_ID])
                        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("package:"));
                    let why = if installed {
                        "the LensLink app on the phone is too old to start by itself: update it, or tap \
                         \"Start streaming service\" in it"
                    } else {
                        "the LensLink app is not installed on the phone"
                    };
                    End::Lost(why.into(), Duration::from_secs(5))
                } else if text.contains("Error") {
                    End::Lost(format!("cannot start the LensLink app: {}", text.trim()), Duration::from_secs(5))
                } else {
                    End::Lost("starting the LensLink app on the phone".into(), retry)
                }
            }
            Err(e) => End::Lost(e, retry),
        }
    }

    /// The mode to ask the phone for (before picking the closest it offers) and the bit rate: what
    /// the app's window chose, else what the virtual camera publishes.
    fn wanted(&self) -> (Mode, Option<u32>) {
        match self.viewer.as_ref().and_then(Viewer::app).and_then(crate::ui::Link::video) {
            Some((mode, bitrate)) => (mode, Some(bitrate)),
            None => (self.output, None),
        }
    }

    /// What the phone tells about itself, for the app's widgets.
    fn phone_message(&self, msg: &Value) {
        let Some(a) = self.viewer.as_ref().and_then(Viewer::app) else { return };
        match msg["type"].as_str() {
            Some("device_info") => {
                let model = msg["model"].as_str().unwrap_or_default().to_string();
                a.info(|i| i.phone = model);
            }
            Some("status") => a.info(|i| {
                i.battery = msg["battery"].as_u64().map(|b| (b as u32, msg["charging"].as_bool().unwrap_or(false)));
                i.temperature = msg["temperature"].as_f64().map(|t| t as f32);
            }),
            _ => {}
        }
    }

    /// Writes the phone's modes for the virtual camera when they changed (first connection, other
    /// phone). The first line is the camera's default mode.
    fn camera_modes(&mut self, offered: Vec<Mode>) -> Result<(), String> {
        if offered == self.offered {
            return Ok(());
        }
        let wanted = Mode { width: self.o.width, height: self.o.height, fps: self.o.fps };
        let default = if offered.contains(&wanted) {
            wanted
        } else if offered.contains(&Mode::DEFAULT) {
            Mode::DEFAULT
        } else {
            offered[0]
        };
        if !self.app_selected {
            self.output = default;
        }
        let mut list = vec![default];
        list.extend(offered.iter().copied().filter(|m| *m != default));
        if !self.o.virtual_camera {
            self.offered = offered;
            return Ok(());
        }
        lenslink_vcam::modes::write(&list).map_err(|e| {
            format!(
                "cannot write {} ({e}). Reinstall the virtual camera: tools/install-vcam.ps1",
                lenslink_vcam::modes::MODES_PATH
            )
        })?;
        eprintln!("camera modes: {}", list.iter().map(Mode::to_string).collect::<Vec<_>>().join(", "));
        self.offered = offered;
        Ok(())
    }
}

/// `bitrate`: what the app's window chose; else the command line's, else one that suits the mode.
fn start_message(o: &Options, mode: Mode, bitrate: Option<u32>) -> Value {
    let bitrate = bitrate.or(o.bitrate).unwrap_or_else(|| modes::bitrate(mode));
    let mut start = json!({
        "type": "start",
        "camera_id": o.camera,
        "width": mode.width,
        "height": mode.height,
        "fps": mode.fps,
        "codec": o.codec.wire_name(),
        "bitrate": bitrate,
        "stabilization": o.stabilization,
    });
    if let Some(z) = o.zoom {
        start["zoom"] = json!(z);
    }
    eprintln!(
        "requested camera {} {mode} {} {:.1} Mbit/s",
        o.camera,
        o.codec.wire_name(),
        bitrate as f64 / 1e6
    );
    start
}

/// Prints a control message from the phone; returns it parsed. A phone error ends the session.
fn control_message(payload: &[u8]) -> Result<Value, String> {
    let text = String::from_utf8_lossy(payload);
    let msg: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    match msg["type"].as_str() {
        Some("error") => return Err(format!("phone: {}", msg["message"].as_str().unwrap_or("unknown error"))),
        Some("device_info") => eprintln!(
            "phone: {} {}, Android {}",
            msg["manufacturer"].as_str().unwrap_or("?"),
            msg["model"].as_str().unwrap_or("?"),
            msg["android"].as_str().unwrap_or("?")
        ),
        Some("capabilities" | "stopped" | "status") => {}
        _ => eprintln!("phone: {text}"),
    }
    Ok(msg)
}

/// Reads until the phone answers `get_capabilities` (nothing is streaming yet); also returns the
/// other messages that came first.
fn wait_for_capabilities(reader: &mut impl io::Read) -> Result<(Value, Vec<Value>), String> {
    let mut others = Vec::new();
    loop {
        match proto::read_packet(reader) {
            Ok(Some(p)) if p.header.kind == Kind::Control => {
                let msg = control_message(&p.payload)?;
                if msg["type"] == "capabilities" {
                    return Ok((msg, others));
                }
                others.push(msg);
            }
            Ok(Some(_)) => {}
            Ok(None) => return Err("the phone closed the connection".into()),
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn send_control(w: &mut impl Write, msg: &Value) -> Result<(), String> {
    proto::write_packet(w, Kind::Control, 0, 0, msg.to_string().as_bytes()).map_err(|e| e.to_string())
}

fn is_closed(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
    )
}

/// ffplay tuned for latency: no input buffering, no probing, drop late frames.
/// `-thread_type slice` matters most: FFmpeg's default frame threading queues one frame per CPU thread
/// (16 threads at 30 fps = 0.5 s). `setpts=0` shows each frame as soon as it is decoded.
fn spawn_ffplay(o: &Options, title: &str) -> Result<Child, String> {
    let fps = o.fps.to_string();
    Command::new("ffplay")
        .args([
            "-hide_banner", "-loglevel", "error",
            "-fflags", "nobuffer", "-flags", "low_delay", "-framedrop",
            "-probesize", "32", "-analyzeduration", "0", "-sync", "ext",
            "-thread_type", "slice",
            "-f", o.codec.ffmpeg_format(), "-framerate", &fps,
            "-window_title", title,
            "-i", "-",
            "-vf", "setpts=0",
        ])
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start ffplay ({e}); install FFmpeg or use --no-play --out file"))
}

struct Stats {
    started: Instant,
    window: Instant,
    frames: u32,
    keyframes: u32,
    bytes: usize,
    total_frames: u64,
    last_pts: Option<u64>,
    max_gap_us: u64,
}

impl Stats {
    fn new() -> Self {
        let now = Instant::now();
        Stats {
            started: now,
            window: now,
            frames: 0,
            keyframes: 0,
            bytes: 0,
            total_frames: 0,
            last_pts: None,
            max_gap_us: 0,
        }
    }

    /// Returns the summary line and its figures once per second.
    fn add(&mut self, h: &proto::Header, len: usize) -> Option<(String, crate::ui::Stats)> {
        self.bytes += len;
        if h.has(flags::CONFIG) {
            return None;
        }
        self.frames += 1;
        self.total_frames += 1;
        if h.has(flags::KEYFRAME) {
            self.keyframes += 1;
        }
        if let Some(prev) = self.last_pts {
            self.max_gap_us = self.max_gap_us.max(h.pts_us.saturating_sub(prev));
        }
        self.last_pts = Some(h.pts_us);

        let elapsed = self.window.elapsed();
        if elapsed >= Duration::from_secs(1) {
            let secs = elapsed.as_secs_f64();
            let figures = crate::ui::Stats {
                seconds: self.started.elapsed().as_secs_f64(),
                fps: self.frames as f64 / secs,
                mbits: self.bytes as f64 * 8.0 / secs / 1e6,
                keyframes: self.keyframes,
                gap_ms: self.max_gap_us as f64 / 1000.0,
            };
            let line = format!(
                "{:>4.0}s  {:5.1} fps  {:6.2} Mbit/s  keyframes {}  max frame gap {:4.0} ms",
                figures.seconds, figures.fps, figures.mbits, figures.keyframes, figures.gap_ms,
            );
            // Rewrite one line in a console; one line per second when redirected to a log.
            if io::stderr().is_terminal() {
                eprint!("\r{line}   ");
            } else {
                eprintln!("{line}");
            }
            let _ = io::stderr().flush();
            self.window = Instant::now();
            self.frames = 0;
            self.keyframes = 0;
            self.bytes = 0;
            self.max_gap_us = 0;
            return Some((line, figures));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn defaults() {
        let o = parse(&[]).unwrap();
        assert_eq!((o.width, o.height, o.fps, o.codec), (1920, 1080, 30, Codec::H264));
        assert_eq!(o.player, Player::Window);
    }

    #[test]
    fn full_options() {
        let o = parse(&args(
            "-s abc --camera 1 --size 3840x2160 --fps 60 --codec hevc --bitrate 45M --zoom 0.6 --no-stabilization --out a.h265",
        ))
        .unwrap();
        assert_eq!(o.serial.as_deref(), Some("abc"));
        assert_eq!((o.width, o.height, o.fps), (3840, 2160, 60));
        assert_eq!(o.codec, Codec::H265);
        assert_eq!(o.bitrate, Some(45_000_000));
        assert_eq!(o.zoom, Some(0.6));
        assert!(!o.stabilization);
    }

    #[test]
    fn bitrate_units() {
        assert_eq!(parse_bitrate("8000k").unwrap(), 8_000_000);
        assert_eq!(parse_bitrate("2.5M").unwrap(), 2_500_000);
        assert_eq!(parse_bitrate("1000").unwrap(), 1000);
    }

    #[test]
    fn rejects_discarding_video() {
        assert!(parse(&args("--no-play")).is_err());
    }
}
