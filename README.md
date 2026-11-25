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

Pre-alpha, step 3 of the [roadmap](docs/architecture.md#roadmap). Working now:

- a **camera probe** that reports what Camera2 and the hardware encoders of a phone can really do;
- **streaming** the phone camera over USB into a LensLink window on the PC, decoded on the GPU
  (≈ 200 ms glass-to-glass at 1080p30 on a Redmi Note 8T).

Not yet: the Windows virtual camera, so other apps cannot see the phone yet.

## Layout

| Path | What |
|---|---|
| `android/` | Phone app (Kotlin, no third-party dependencies) |
| `pc/` | Windows host (Rust workspace) |
| `tools/` | Latency clock |
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

## License

MIT
