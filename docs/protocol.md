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
| `set` | PC → phone | camera parameters, e.g. `{"af_mode":"continuous_video","iso":200}` |
| `stats` | phone → PC | fps, bitrate, encoder queue, temperature, battery |
| `error` | both | `message` |

### Video (kind 1)

- Codec is chosen by the last `start` message.
- Payload is one access unit in Annex-B format (start codes included).
- `CONFIG` packets carry codec configuration (SPS/PPS, plus VPS for H.265) and precede the first frame.
- `KEYFRAME` marks IDR frames.

### Audio (kind 2)

Reserved. Codec and parameters will be announced by a control message.
