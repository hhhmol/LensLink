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

Pre-alpha. Step 1 of the [roadmap](docs/architecture.md#roadmap): a **camera probe**
that reports what Camera2 and the hardware encoders of a given phone can really do.

## Layout

| Path | What |
|---|---|
| `android/` | Phone app (Kotlin, no third-party dependencies) |
| `pc/` | Windows host (Rust workspace) |
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

## License

MIT
