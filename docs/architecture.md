# Architecture

## Goals

- Phone camera as a first-class Windows camera over USB: low latency, high quality.
- Full manual control of the phone camera (focus, exposure, ISO, white balance, stabilisation, lens).
- Nothing running on the PC except one small tray app. No ads, no telemetry, no network.
- One phone app for every Android version from 8.0 up, no per-device code paths
  (capabilities are discovered at runtime; vendor quirks get targeted workarounds).

## Pipeline

```
Camera2 ──Surface──► MediaCodec encoder ──Annex-B──► LocalServerSocket "lenslink"
                                                          │  adb forward tcp:27183 localabstract:lenslink
                                                          ▼
PC: TCP 127.0.0.1:27183 ─► demux ─► GPU decode (D3D11VA) ─► filter chain (GPU) ─► shared texture/memory
                                                                                   │
                                          Frame Server loads our COM media source ◄┘
                                          registered with MFCreateVirtualCamera
```

## Decisions

### Transport: ADB forward over USB
- Works on every Android version, no PC driver, no network adapter changes.
- USB 2.0 through ADB gives ~20–40 MB/s; compressed 4K30 needs ~5–8 MB/s.
- Raw frames are not an option (1080p30 NV12 ≈ 93 MB/s), so the phone always encodes.
- Phone listens on an abstract Unix socket (no `INTERNET` permission needed).

Considered and rejected: USB tethering (adds a network adapter), Android Open Accessory
(needs a WinUSB driver), Android 14 UVC webcam mode (vendor-dependent, 1080p MJPEG, no manual controls).

### Phone app instead of an ADB-shell server
scrcpy runs its server as the ADB shell user; camera access from that user needs Android 12+.
A normal app with the `CAMERA` permission works on any version and can run as a
`camera`-type foreground service with the screen off.

### Capture: Camera2, not CameraX
Camera2 exposes manual sensor control, physical lenses and per-device capability flags.
The app reads them at runtime and only offers what the device supports.

### Windows camera: Media Foundation virtual camera
`MFCreateVirtualCamera` (Windows 11) registers a system camera served by the Windows
Camera Frame Server, so both Media Foundation and DirectShow apps see it. No kernel driver,
no driver signing. The media source is a COM DLL loaded by the Frame Server process,
so frames travel from the host app to the DLL through shared memory / shared D3D11 textures.

Windows 10 is not supported. A DirectShow fallback could be added later.

### Filters
Between decode and the virtual camera sits a chain of filters, each a separate module
working on GPU textures:

| Filter | Technique |
|---|---|
| colour, LUT (`.cube`), sharpen, crop/zoom, mirror | `wgpu` shaders |
| background blur / replacement | segmentation model via ONNX Runtime + DirectML |
| frame interpolation (e.g. 30 → 60 fps) | RIFE-class model via ONNX Runtime; adds ≥ 1 frame of latency |

The virtual camera is an ordinary camera, so OBS can still use it as a source.

### Audio (reserved)
The protocol has a separate `audio` stream kind from day one. Not implemented:
a Windows virtual microphone needs a kernel audio driver. The likely first step is
playing the stream into a user-installed virtual cable.

## Roadmap

1. **Camera probe** — report Camera2 / encoder capabilities of each phone. ✓
2. **Stream MVP** — phone streams H.264 over ADB; PC saves it / plays it with `ffplay`. Measure latency. ✓
3. **Rust host** — protocol, GPU decode (Media Foundation + DXVA), preview window (D3D11 video processor). ✓
4. **Virtual camera** — Media Foundation media source + `MFCreateVirtualCamera`. ✓
5. **Controls** — a) video modes from the phone, chosen by apps ✓; b) lens, focus, exposure, ISO, WB. ← next
6. **Filters** — shader filters, then segmentation, then interpolation.
7. **Polish** — auto-reconnect, autostart, installer, battery/thermal guards on the phone.

## Virtual camera internals (step 4)

- `pc/crates/lenslink-vcam` is a COM DLL: class factory → `IMFActivate` → `IMFMediaSourceEx` with one
  `IMFMediaStream2`. The Frame Server service loads it in session 0 as LOCAL SERVICE and shares the
  camera between apps.
- The host creates the camera with `MFCreateVirtualCamera` (session lifetime, current user), scales each
  decoded frame on the GPU into an NV12 texture of the selected mode, reads it back and writes it to
  shared memory (two slots sized for 4096x2304).
- Modes (step 5a): the host asks the phone for `capabilities`, keeps the standard webcam sizes that
  the camera, auto-exposure ranges and hardware encoder all support at 30/60 fps, and writes them to
  `C:\ProgramData\LensLink\config\modes.txt` before creating the camera. The source offers one media
  type per mode. When an app starts the stream, the source stores the selected type in shared memory;
  the host sends the phone a new `start` and recreates its decoder.
- Shared memory and its event live in `Global\`. Only a service may create global objects, so the
  camera side creates them with an ACL for interactive users; the host opens them while an app has the
  camera open and drops frames cheaply otherwise.
- The stream answers each `RequestSample` with the newest host frame as soon as one arrives, so the
  camera adds no fixed delay. Without host frames for 1 s it sends a dark grey placeholder at 30 fps.
- The camera log is `C:\ProgramData\LensLink\logs\vcam.log`.
