//! LensLink wire protocol, version 1. The format is specified in `docs/protocol.md`.

use std::fmt;
use std::io::{self, Read, Write};

pub const MAGIC: [u8; 4] = *b"LNSL";
pub const VERSION: u16 = 1;
pub const HANDSHAKE_LEN: usize = 8;
pub const HEADER_LEN: usize = 16;
pub const MAX_PAYLOAD: u32 = 16 * 1024 * 1024;

/// Abstract Unix socket the phone listens on.
pub const SOCKET_NAME: &str = "lenslink";
/// Local TCP port the PC forwards to the phone socket.
pub const DEFAULT_TCP_PORT: u16 = 27183;

/// Packet flags (header byte 1).
pub mod flags {
    pub const CONFIG: u8 = 1 << 0;
    pub const KEYFRAME: u8 = 1 << 1;
    pub const END_OF_STREAM: u8 = 1 << 2;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Control,
    Video,
    Audio,
    /// A kind from a newer protocol revision; receivers skip it.
    Unknown(u8),
}

impl Kind {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Kind::Control,
            1 => Kind::Video,
            2 => Kind::Audio,
            other => Kind::Unknown(other),
        }
    }

    pub fn as_u8(self) -> u8 {
        match self {
            Kind::Control => 0,
            Kind::Video => 1,
            Kind::Audio => 2,
            Kind::Unknown(v) => v,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub kind: Kind,
    pub flags: u8,
    pub len: u32,
    pub pts_us: u64,
}

impl Header {
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        b[0] = self.kind.as_u8();
        b[1] = self.flags;
        // b[2..4] reserved, zero
        b[4..8].copy_from_slice(&self.len.to_be_bytes());
        b[8..16].copy_from_slice(&self.pts_us.to_be_bytes());
        b
    }

    pub fn decode(b: &[u8; HEADER_LEN]) -> Result<Self, ProtocolError> {
        let len = u32::from_be_bytes(b[4..8].try_into().unwrap());
        if len > MAX_PAYLOAD {
            return Err(ProtocolError::PayloadTooLarge(len));
        }
        Ok(Header {
            kind: Kind::from_u8(b[0]),
            flags: b[1],
            len,
            pts_us: u64::from_be_bytes(b[8..16].try_into().unwrap()),
        })
    }

    pub fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub header: Header,
    pub payload: Vec<u8>,
}

#[derive(Debug)]
pub enum ProtocolError {
    Io(io::Error),
    BadMagic([u8; 4]),
    UnsupportedVersion(u16),
    PayloadTooLarge(u32),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolError::Io(e) => write!(f, "i/o error: {e}"),
            ProtocolError::BadMagic(m) => write!(f, "not a LensLink peer (magic {m:02x?})"),
            ProtocolError::UnsupportedVersion(v) => {
                write!(f, "peer speaks protocol v{v}, this build speaks v{VERSION}")
            }
            ProtocolError::PayloadTooLarge(n) => {
                write!(f, "payload of {n} bytes exceeds the {MAX_PAYLOAD} byte limit")
            }
        }
    }
}

impl std::error::Error for ProtocolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProtocolError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for ProtocolError {
    fn from(e: io::Error) -> Self {
        ProtocolError::Io(e)
    }
}

pub fn write_handshake<W: Write>(w: &mut W) -> io::Result<()> {
    let mut b = [0u8; HANDSHAKE_LEN];
    b[0..4].copy_from_slice(&MAGIC);
    b[4..6].copy_from_slice(&VERSION.to_be_bytes());
    w.write_all(&b)
}

/// Reads the peer's handshake and returns its protocol version.
pub fn read_handshake<R: Read>(r: &mut R) -> Result<u16, ProtocolError> {
    let mut b = [0u8; HANDSHAKE_LEN];
    r.read_exact(&mut b)?;
    let magic: [u8; 4] = b[0..4].try_into().unwrap();
    if magic != MAGIC {
        return Err(ProtocolError::BadMagic(magic));
    }
    let version = u16::from_be_bytes([b[4], b[5]]);
    if version != VERSION {
        return Err(ProtocolError::UnsupportedVersion(version));
    }
    Ok(version)
}

pub fn write_packet<W: Write>(
    w: &mut W,
    kind: Kind,
    flags: u8,
    pts_us: u64,
    payload: &[u8],
) -> Result<(), ProtocolError> {
    let len = u32::try_from(payload.len())
        .ok()
        .filter(|&n| n <= MAX_PAYLOAD)
        .ok_or(ProtocolError::PayloadTooLarge(payload.len().min(u32::MAX as usize) as u32))?;
    let header = Header { kind, flags, len, pts_us };
    w.write_all(&header.encode())?;
    w.write_all(payload)?;
    Ok(())
}

/// Reads one packet. Returns `Ok(None)` if the stream ends cleanly between packets.
pub fn read_packet<R: Read>(r: &mut R) -> Result<Option<Packet>, ProtocolError> {
    let mut hb = [0u8; HEADER_LEN];
    if !read_exact_or_eof(r, &mut hb)? {
        return Ok(None);
    }
    let header = Header::decode(&hb)?;
    let mut payload = vec![0u8; header.len as usize];
    r.read_exact(&mut payload)?;
    Ok(Some(Packet { header, payload }))
}

/// Like `read_exact`, but a clean EOF before the first byte returns `Ok(false)`.
fn read_exact_or_eof<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<bool> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn handshake_roundtrip() {
        let mut buf = Vec::new();
        write_handshake(&mut buf).unwrap();
        assert_eq!(buf.len(), HANDSHAKE_LEN);
        assert_eq!(read_handshake(&mut Cursor::new(buf)).unwrap(), VERSION);
    }

    #[test]
    fn handshake_rejects_foreign_peer() {
        let err = read_handshake(&mut Cursor::new(b"HTTP/1.1".to_vec())).unwrap_err();
        assert!(matches!(err, ProtocolError::BadMagic(_)));
    }

    #[test]
    fn handshake_rejects_other_version() {
        let mut b = Vec::from(MAGIC);
        b.extend_from_slice(&2u16.to_be_bytes());
        b.extend_from_slice(&[0, 0]);
        let err = read_handshake(&mut Cursor::new(b)).unwrap_err();
        assert!(matches!(err, ProtocolError::UnsupportedVersion(2)));
    }

    #[test]
    fn packets_roundtrip() {
        let mut buf = Vec::new();
        write_packet(&mut buf, Kind::Control, 0, 0, br#"{"type":"stop"}"#).unwrap();
        write_packet(&mut buf, Kind::Video, flags::CONFIG, 0, &[0, 0, 0, 1, 0x67]).unwrap();
        write_packet(&mut buf, Kind::Video, flags::KEYFRAME, 33_333, &[0, 0, 0, 1, 0x65]).unwrap();

        let mut r = Cursor::new(buf);
        let a = read_packet(&mut r).unwrap().unwrap();
        assert_eq!(a.header.kind, Kind::Control);
        assert_eq!(a.payload, br#"{"type":"stop"}"#);

        let b = read_packet(&mut r).unwrap().unwrap();
        assert!(b.header.has(flags::CONFIG));

        let c = read_packet(&mut r).unwrap().unwrap();
        assert!(c.header.has(flags::KEYFRAME));
        assert_eq!(c.header.pts_us, 33_333);

        assert!(read_packet(&mut r).unwrap().is_none());
    }

    #[test]
    fn unknown_kind_is_preserved() {
        let mut buf = Vec::new();
        write_packet(&mut buf, Kind::Unknown(9), 0, 0, b"x").unwrap();
        let p = read_packet(&mut Cursor::new(buf)).unwrap().unwrap();
        assert_eq!(p.header.kind, Kind::Unknown(9));
    }

    #[test]
    fn oversized_header_is_rejected() {
        let h = Header { kind: Kind::Video, flags: 0, len: MAX_PAYLOAD + 1, pts_us: 0 };
        let err = read_packet(&mut Cursor::new(h.encode().to_vec())).unwrap_err();
        assert!(matches!(err, ProtocolError::PayloadTooLarge(_)));
    }

    #[test]
    fn truncated_packet_is_an_error() {
        let mut buf = Vec::new();
        write_packet(&mut buf, Kind::Video, 0, 0, &[1, 2, 3, 4]).unwrap();
        buf.truncate(HEADER_LEN + 2);
        assert!(matches!(read_packet(&mut Cursor::new(buf)), Err(ProtocolError::Io(_))));
    }
}
