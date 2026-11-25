# Device notes

What the camera probe found on real phones. Raw reports live in `reports/` (not committed).

## Summary

| | Redmi Note 8T | POCO F8 Pro |
|---|---|---|
| SoC / Android | Snapdragon 665 / Android 11 (SDK 30) | Snapdragon 8 Elite (SM8750) / Android 17 (SDK 37) |
| Camera2 level | LEVEL_3, manual sensor, RAW | LEVEL_3, manual sensor, RAW, 10-bit |
| Cameras exposed | main (12 MP binned), front | logical back (0.6–10× zoom: ultrawide, main, ~2.5× tele), front |
| Hidden ids found by probing | none | 2, 3, 4 (physical lenses), 5, 6 (duplicates of the logical camera) |
| Max video, main camera | 4K30, 1080p30 | 4K33, 1440p60, 1080p60 |
| High-speed sessions | 1080p120, 720p240 | 1080p240, 720p480 |
| Stabilisation | none | EIS, preview stabilisation, OIS |
| HW encoders | H.264 (VBR only), H.265 (VBR/CBR), up to 4096×2160 | H.264, H.265 incl. Main10/HDR10+, VBR/CBR, up to 8K; AV1 software only |
| Min focus distance | not reported | not reported |

## Measured streaming (step 2)

| Device | Mode | Result |
|---|---|---|
| Redmi Note 8T | 1080p30 H.264, 20 Mbit/s requested | steady 30.0 fps, 33 ms frame spacing, 11–21 Mbit/s (VBR only), no B-frames, High profile |

Glass-to-glass latency (`tools/latency.html`, phone filming the clock, ffplay window next to it,
4 screenshots): **≈ 200–250 ms**, Note 8T, 1080p30.
Before `-thread_type slice` in ffplay it was ≈ 0.5–1 s: FFmpeg frame threading queued one frame per CPU thread.

## Consequences for the app

- **Capabilities must be discovered at runtime.** Frame rates, stabilisation and lenses differ completely between devices.
- **Lens selection goes through the logical camera's zoom ratio** (`CONTROL_ZOOM_RATIO`), not by opening hidden physical ids.
  The physical ids are visible to `getCameraCharacteristics` but are not in `cameraIdList`, so opening them directly is not guaranteed.
- **Xiaomi omits `LENS_INFO_MINIMUM_FOCUS_DISTANCE`** on both a 2019 MIUI and a 2026 HyperOS device.
  Whether `LENS_FOCUS_DISTANCE` still works in `AF_MODE_OFF` must be tested on the device.
- **Default codec:** H.265 when a hardware encoder exists, H.264 as fallback. Bitrate is not the constraint over USB.
- **Default webcam mode:** 1080p at the highest regular fps (30 on Note 8T, 60 on POCO), stabilisation on when available.
- Vendor "measured" encoder rates (`getAchievableFrameRatesFor`) are pessimistic: Note 8T reports 7–16 fps at 4K
  while its stock camera records 4K30. Real throughput is measured in step 2.
- 4K60 and 8K are not offered to third-party apps on the POCO; they stay reserved for the stock camera app.
