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
5. **Controls** — a) video modes from the phone, chosen by apps ✓; b) lens/zoom, focus, exposure, ISO, WB as standard camera properties ✓
6. **Filters** — shader filters, then segmentation, then interpolation.
7. **Polish** ← now — a) auto-reconnect and autostart of the phone service ✓; b) stream only while an
   app uses the camera, adb device events instead of polling ✓; c) the app: 1. tray, borderless
   window, own window thread ✓; 2. liquid glass shader; 3. settings curtains; 4. widgets on the video;
   d) installer; e) battery/thermal guards on the phone.

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
- Controls (step 5b): the host derives ranges from the capability report (zoom ratio range, AF modes,
  exposure and ISO ranges when the camera has MANUAL_SENSOR, EV range, AWB presets) and writes them to
  `controls.txt` next to `modes.txt`. The source answers `IKsControl` property requests for the
  standard `PROPSETID_VIDCAP_CAMERACONTROL` (zoom, exposure, focus) and `PROPSETID_VIDCAP_VIDEOPROCAMP`
  (brightness = EV compensation, gain = ISO, white balance, power-line frequency) sets: BASICSUPPORT,
  DEFAULTVALUES, GET, SET. A SET stores the value in a shared-memory table and bumps a counter; the host
  sends the phone a `set` with all current values. Windows Settings shows sliders for zoom and
  brightness; apps with a full camera property dialog reach the rest.
- Shared memory and its event live in `Global\`. Only a service may create global objects, so the
  camera side creates them with an ACL for interactive users; the host opens them while an app has the
  camera open and drops frames cheaply otherwise.
- The stream answers each `RequestSample` with the newest host frame as soon as one arrives, so the
  camera adds no fixed delay. Without host frames for 1 s it sends a dark grey placeholder at 30 fps.
- The camera log is `C:\ProgramData\LensLink\logs\vcam.log`.

## Reconnection (step 7a)

The host keeps the window, the virtual camera and the output file across connections and loops:
wait for an adb device in state `device` (reporting `unauthorized` to the user) → `adb forward` →
handshake → stream until the socket closes or no video arrives for 5 s → retry.

- When adb connects but nothing listens on the phone, the host runs
  `am start -n io.github.lenslink/.AutostartActivity`: an invisible activity that shows over the lock
  screen, starts the camera foreground service (Android 14+ only allows that from the foreground) and
  closes. It is exported but requires `android.permission.DUMP`, which adb's shell holds and apps
  cannot get. Launches are spaced 10 s apart; `pm path` tells "not installed" from "too old".
- Stop on the phone sets a persisted flag and sends `stopped` to the PC; the autostart activity then
  does nothing and the PC only probes the socket. Start in the app or in the "stopped" notification,
  or a new USB connection, clears it.
- On reconnect the phone gets the mode the camera currently publishes (or the closest one it offers,
  preferring larger sizes, scaled on the GPU) and the control values apps set, which the phone
  forgets between sessions.

## Idle cost (step 7b)

- While an app streams from the virtual camera, its stream worker stamps a heartbeat into the shared
  memory (header v4) every half frame interval. The host counts the camera as watched while the
  heartbeat is under 1.5 s old; a window, ffplay or `--out` file always counts.
- Unwatched for 5 s, the host sends the phone `stop`: the camera and encoder close, the session stays
  connected. Watched again, it sends `start` with the current mode and the camera is back in ~1–2 s.
  The phone holds its wake lock only while streaming.
- Waiting for a phone uses one long-running `adb track-devices`, which pushes the device list on
  change, instead of starting `adb devices` every second; without a window the host wakes every
  100 ms (50 ms with a connected phone) to check for apps. Measured idle: 0.00 % CPU, 46 MB.

## The app (step 7c)

`lenslink-host` is a library with two binaries: the `lenslink-host` console tool and `LensLink.exe`
(Windows subsystem, no console). The app runs the same streaming loop with a new output, the app
window (`ui` module), and the virtual camera when it is installed.

- **Two threads.** The streaming thread (adb, socket, decoder, virtual camera) is the one from the
  console tool. The window has its own thread and message loop, so dragging or resizing never stalls
  the stream and a slow phone never stalls the window. They share the D3D11 device: the streaming
  thread converts each frame to BGRA on the GPU video processor (inside the multithreaded Direct2D
  factory's lock) and posts a message; the window draws the texture with Direct2D. Frames are
  converted only while the window is on screen. Hiding it (×, Esc, Alt+F4) leaves the tray icon; the
  phone's camera then follows the virtual camera's use as in step 7b.
- **Borderless.** `WM_NCCALCSIZE` gives the whole window to the client while the thick-frame style
  keeps Windows 11's shadow, rounded corners, snapping and maximising. Dragging anywhere moves the
  window (`SC_MOVE`); only the corners resize, keeping the video's aspect ratio, because the edges
  are reserved for the settings curtains.
- **Look, decided with prototypes:** controls float as liquid glass over the video: transparent, no
  blur, a thick lens rim that refracts with RGB dispersion, shapes that merge like drops. Settings
  hide in four elastic curtains, one per edge, whose edge is a string of springs (waves, overshoot,
  bending towards the pointer); their text bends with them. Any item can be pulled onto the video as
  a floating widget. The prototypes are kept outside the repository.
- Settings live in `%APPDATA%\LensLink\settings.json` (unknown keys survive a save), the log in
  `lenslink.log` next to it. A named mutex keeps one instance; a second start shows the first window.
