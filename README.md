# LensLink

Use an Android phone as a USB webcam on Windows 11 — no ads, no cloud, no bloat.

The phone captures video with Camera2, encodes it with its hardware encoder and
sends it over the USB cable (ADB). A small Windows app decodes it on the GPU,
runs optional filters and exposes the result as a regular system camera, visible
to Zoom, Teams, Discord, browsers, OBS and the Windows Camera app.

```
Phone (Kotlin)                               PC (Rust, Windows 11)
Camera2 + manual controls                    receive stream
MediaCodec H.264 / H.265        USB / ADB    GPU decode
foreground service         ───────────────►  filter chain (GPU)
video / [audio] / control  ◄───────────────  camera controls from the tray
                                             Media Foundation virtual camera
```

## Status

Pre-alpha, step 4 of the [roadmap](docs/architecture.md#roadmap) done. Working now:

- a **camera probe** that reports what Camera2 and the hardware encoders of a phone can really do;
- **streaming** the phone camera over USB into a LensLink window on the PC, decoded on the GPU
  (≈ 200 ms glass-to-glass at 1080p30 on a Redmi Note 8T);
- a **Windows 11 virtual camera** "LensLink" that any app can pick (Camera, Teams, Zoom, browsers).
  It offers every webcam mode the phone camera supports (e.g. 4K30, 1080p60, 720p30); picking one in
  an app switches the phone to it.

Not yet: camera controls from the PC, filters, a tray app, an installer.

## Layout

| Path | What |
|---|---|
| `android/` | Phone app (Kotlin, no third-party dependencies) |
| `pc/` | Windows host and virtual camera (Rust workspace) |
| `tools/` | Virtual camera installer, latency clock |
| `docs/architecture.md` | Design decisions and roadmap |
| `docs/protocol.md` | Wire protocol between phone and PC |

## Requirements

- Android 8.0+ phone with **USB debugging** enabled.
- Windows 11 (the virtual camera uses `MFCreateVirtualCamera`, Windows 11 only).
- To build: JDK 17, Android SDK (platform 35, build-tools, platform-tools), Rust (MSVC toolchain).

## Camera probe

```powershell
cd android
.\gradlew installDebug
```

Open **LensLink** on the phone. The report appears on screen and is saved to
`/sdcard/Android/data/io.github.lenslink/files/camera-report.{txt,json}`.
To read it from the PC:

```powershell
cd pc
cargo run -p lenslink-host -- probe
```

Xiaomi phones block `installDebug` unless *Install via USB* is enabled (needs a Mi account).
Alternative: copy `android/app/build/outputs/apk/debug/app-debug.apk` to the phone and open it.

## Streaming

On the phone: open LensLink and tap **Start streaming service**. On the PC:

```powershell
cd pc
cargo run --release -p lenslink-host -- stream
```

A window shows the camera; Esc closes it. Useful options: `--size 3840x2160`, `--fps 60`,
`--codec h265` (needs *HEVC Video Extensions* from the Microsoft Store), `--zoom 0.6`,
`--out file.h264`, `--ffplay` (play in FFmpeg's ffplay instead). `lenslink-host` without arguments lists all options.

## Virtual camera

Install the camera once (asks for administrator rights):

```powershell
cd pc
cargo build --release
..\tools\install-vcam.ps1
```

The script copies `lenslink_vcam.dll` to `C:\Program Files\LensLink`, registers it in HKLM (the Windows
camera service, Frame Server, does not read per-user registrations) and creates
`C:\ProgramData\LensLink\logs` for its log. `install-vcam.ps1 -Uninstall` removes it.

Then stream with `--vcam`:

```powershell
cargo run --release -p lenslink-host -- stream --vcam            # with the preview window
cargo run --release -p lenslink-host -- stream --vcam --no-play  # camera only
```

The camera "LensLink" appears in apps while `lenslink-host` runs and disappears when it exits.
On start the host asks the phone for its capabilities and offers apps the modes that the camera,
its auto-exposure and the hardware encoder all support, from 4K down to 640x360 at 30 or 60 fps.
When an app selects a mode, the phone switches to it (about half a second of grey); bitrate follows
the mode unless `--bitrate` is given. `--size`/`--fps` pick the default mode.
Without a phone stream it shows dark grey. `cargo run --release -p lenslink-vcam --example grab -- 90 out.pgm 3840x2160@30`
opens the camera like an app, lists its modes, selects one and reports the frame rate it receives.

### Camera controls

The camera exposes the phone's own controls as standard Windows camera properties, changed live
without restarting the stream:

| Windows property | Phone setting |
|---|---|
| Zoom | zoom ratio ×10 (10 = 1×); below 10 picks the ultrawide lens where there is one |
| Focus | continuous autofocus, or manual distance 0 (infinity) … 100 (10 cm) |
| Exposure | auto, or manual 2^value seconds (−5 ≈ 1/32 s) |
| Brightness | exposure compensation in tenths of EV |
| Gain | auto, or manual ISO |
| White balance | auto, or kelvin mapped to the nearest preset |
| Power-line frequency | anti-flicker: off / 50 Hz / 60 Hz / auto |

Windows Settings → Bluetooth & devices → Cameras → LensLink shows sliders for zoom and brightness.
Apps with a full camera property dialog (e.g. OBS "Configure Video") reach the rest, and
`cargo run --release -p lenslink-vcam --example controls -- zoom=20 exposure=-7 gain=800` lists and
sets them from the command line.

## License

MIT
