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
use crate::modes;
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

pub fn run(o: &Options) -> Result<(), String> {
    let serial = o.serial.as_deref();
    let local = format!("tcp:{}", o.port);
    let remote = format!("localabstract:{}", proto::SOCKET_NAME);
    adb::check(serial, &["forward", &local, &remote])?;
    let result = stream(o);
    let _ = adb::run(serial, &["forward", "--remove", &local]);
    result
}

fn stream(o: &Options) -> Result<(), String> {
    let sock = TcpStream::connect(("127.0.0.1", o.port)).map_err(|e| format!("connect: {e}"))?;
    sock.set_nodelay(true).ok();
    let mut writer = sock.try_clone().map_err(|e| e.to_string())?;
    let mut reader = BufReader::with_capacity(1 << 20, sock);

    proto::write_handshake(&mut writer).map_err(|e| e.to_string())?;
    match proto::read_handshake(&mut reader) {
        Ok(_) => {}
        Err(ProtocolError::Io(e)) if is_closed(&e) => {
            return Err("the phone did not answer. Open LensLink on the phone and tap \
                        \"Start streaming service\"."
                .into())
        }
        Err(e) => return Err(e.to_string()),
    }

    let mut mode = Mode { width: o.width, height: o.height, fps: o.fps };
    if o.virtual_camera {
        // The camera offers apps the modes this phone camera supports.
        send_control(&mut writer, &json!({ "type": "get_capabilities" }))?;
        let caps = wait_for_capabilities(&mut reader)?;
        let offered = modes::from_capabilities(&caps, &o.camera, o.codec)?;
        if !offered.contains(&mode) {
            let fallback = if offered.contains(&Mode::DEFAULT) { Mode::DEFAULT } else { offered[0] };
            eprintln!("camera {} has no {mode} mode; starting with {fallback}", o.camera);
            mode = fallback;
        }
        // The first line is the camera's default mode.
        let mut list = vec![mode];
        list.extend(offered.iter().copied().filter(|m| *m != mode));
        lenslink_vcam::modes::write(&list).map_err(|e| {
            format!(
                "cannot write {} ({e}). Reinstall the virtual camera: tools/install-vcam.ps1",
                lenslink_vcam::modes::MODES_PATH
            )
        })?;
        eprintln!("camera modes: {}", list.iter().map(Mode::to_string).collect::<Vec<_>>().join(", "));
    }
    send_control(&mut writer, &start_message(o, mode))?;

    let mut file = match &o.out {
        Some(path) => Some(
            std::fs::File::create(path).map_err(|e| format!("cannot create {}: {e}", path.display()))?,
        ),
        None => None,
    };
    let title = format!("LensLink {mode} {}", o.codec.wire_name());
    let mut ffplay = if o.player == Player::Ffplay { Some(spawn_ffplay(o, &title)?) } else { None };
    let mut viewer = if o.player == Player::Window || o.virtual_camera {
        video::init()?;
        let outputs = video::Outputs { window: o.player == Player::Window, virtual_camera: o.virtual_camera };
        Some(Viewer::new(o.codec, mode, outputs)?)
    } else {
        None
    };

    // The socket is read on its own thread so the window keeps responding while no frame arrives.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || loop {
        let r = proto::read_packet(&mut reader);
        let last = !matches!(r, Ok(Some(_)));
        if tx.send(r).is_err() || last {
            break;
        }
    });
    let result = (|| {
        let mut stats = Stats::new();
        loop {
            if let Some(v) = viewer.as_mut() {
                if !v.pump() {
                    eprintln!("
window closed");
                    return Ok(());
                }
                if let Some(requested) = v.take_request().filter(|r| *r != mode) {
                    eprintln!("
an app selected {requested}: switching the phone");
                    mode = requested;
                    send_control(&mut writer, &start_message(o, mode))?;
                    v.set_mode(mode).map_err(|e| format!("switching to {mode}: {e}"))?;
                }
            }
            let packet = match rx.recv_timeout(Duration::from_millis(5)) {
                Ok(Ok(Some(p))) => p,
                Ok(Ok(None)) | Err(RecvTimeoutError::Disconnected) => {
                    return Err("the phone closed the connection".to_string())
                }
                Ok(Err(e)) => return Err(e.to_string()),
                Err(RecvTimeoutError::Timeout) => continue,
            };
            match packet.header.kind {
                Kind::Control => {
                    control_message(&packet.payload)?;
                }
                Kind::Video => {
                    let h = &packet.header;
                    if let Some(f) = file.as_mut() {
                        f.write_all(&packet.payload).map_err(|e| format!("write file: {e}"))?;
                    }
                    if let Some(p) = ffplay.as_mut() {
                        let stdin = p.stdin.as_mut().expect("ffplay stdin is piped");
                        if stdin.write_all(&packet.payload).is_err() {
                            eprintln!("
player window closed");
                            return Ok(());
                        }
                    }
                    if let Some(v) = viewer.as_mut() {
                        v.packet(&packet.payload, h.pts_us, h.has(flags::CONFIG), h.has(flags::KEYFRAME))?;
                    }
                    if let Some(line) = stats.add(h, packet.payload.len()) {
                        if let Some(v) = viewer.as_ref() {
                            v.status(&line);
                        }
                    }
                }
                Kind::Audio | Kind::Unknown(_) => {}
            }
        }
    })();
    // Unblocks the reader thread.
    let _ = writer.shutdown(std::net::Shutdown::Both);
    result
}

fn start_message(o: &Options, mode: Mode) -> Value {
    let bitrate = o.bitrate.unwrap_or_else(|| modes::bitrate(mode));
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
        Some("capabilities") => {}
        _ => eprintln!("phone: {text}"),
    }
    Ok(msg)
}

/// Reads until the phone answers `get_capabilities` (nothing is streaming yet).
fn wait_for_capabilities(reader: &mut impl io::Read) -> Result<Value, String> {
    loop {
        match proto::read_packet(reader) {
            Ok(Some(p)) if p.header.kind == Kind::Control => {
                let msg = control_message(&p.payload)?;
                if msg["type"] == "capabilities" {
                    return Ok(msg);
                }
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

    /// Returns the summary line once per second.
    fn add(&mut self, h: &proto::Header, len: usize) -> Option<String> {
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
            let line = format!(
                "{:>4.0}s  {:5.1} fps  {:6.2} Mbit/s  keyframes {}  max frame gap {:4.0} ms",
                self.started.elapsed().as_secs_f64(),
                self.frames as f64 / secs,
                self.bytes as f64 * 8.0 / secs / 1e6,
                self.keyframes,
                self.max_gap_us as f64 / 1000.0,
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
            return Some(line);
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
