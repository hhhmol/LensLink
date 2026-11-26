# Wire protocol (version 1)

One TCP connection carries everything. All integers are big-endian.

## Connection

- Phone listens on the abstract Unix socket `lenslink`.
- PC runs `adb forward tcp:27183 localabstract:lenslink` and connects to `127.0.0.1:27183`.

## Handshake

Both sides send 8 bytes right after connecting:

| Bytes | Field | Value |
|---|---|---|
| 0–3 | magic | ASCII `LNSL` |
| 4–5 | version | `1` |
| 6–7 | reserved | `0` |

A side that does not support the peer's version closes the connection.

## Packets

Every packet is a 16-byte header followed by `length` bytes of payload.

| Bytes | Field | Notes |
|---|---|---|
| 0 | kind | `0` control, `1` video, `2` audio (reserved); unknown kinds are skipped |
| 1 | flags | bit 0 `CONFIG`, bit 1 `KEYFRAME`, bit 2 `END_OF_STREAM` |
| 2–3 | reserved | send `0`, ignore on receive |
| 4–7 | length | payload size, at most 16 MiB |
| 8–15 | pts_us | presentation time in µs (phone monotonic clock); `0` for control |

### Control (kind 0)

Payload is a UTF-8 JSON object with a `type` field. Unknown types and fields are ignored.

| type | direction | body |
|---|---|---|
| `device_info` | phone → PC | manufacturer, model, Android version |
| `capabilities` | phone → PC | the camera probe report (see `android/.../probe`) |
| `start` | PC → phone | `camera_id`, `width`, `height`, `fps`, `codec` (`h264`/`h265`), `bitrate` |
| `stop` | PC → phone | — |
| `set` | PC → phone | live camera settings, see below; kept across `start` |
| `status` | phone → PC | every 5 s: `battery` (percent), `charging`, `temperature` (battery, °C) |
| `error` | both | `message` |
| `stopped` | phone → PC | `reason`: `user` — streaming was stopped on the phone; the PC does not restart it until the phone is reconnected or the user starts it there |

### `set`

Only the keys present change; `null` returns a setting to automatic. Applied to the running stream
without restarting it, and kept when a new `start` switches the mode.

| key | value |
|---|---|
| `zoom` | logical-camera zoom ratio; below 1 selects the ultrawide lens on multi-camera phones |
| `focus_diopters` | manual focus distance (1/m, 0 = infinity); `null` = continuous autofocus |
| `focus_hold` | `true`: without `focus_diopters`, stop autofocus where it last put the lens |
| `exposure_ns`, `iso` | manual exposure; either one turns auto-exposure off, the other keeps its last auto value. Exposure is capped at one frame |
| `ev` | exposure compensation in EV (rounded to the camera's step) |
| `awb_kelvin` | white balance; the nearest preset (incandescent … shade) is used; `null` = auto |
| `antibanding` | `auto`, `50hz`, `60hz`, `off` |
| `torch` | `true` / `false` |

### Video (kind 1)

- Codec is chosen by the last `start` message.
- Payload is one access unit in Annex-B format (start codes included).
- `CONFIG` packets carry codec configuration (SPS/PPS, plus VPS for H.265) and precede the first frame.
- `KEYFRAME` marks IDR frames.

### Audio (kind 2)

Reserved. Codec and parameters will be announced by a control message.
