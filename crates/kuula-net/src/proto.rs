//! Wire protocol version 1, as `docs/net.md` specifies it: the ALPN, the
//! handshake frame and the five message frames (`Data` was added to
//! version 1 before any release). Everything here is pure:
//! bytes in, bytes or a stable error code out, so the limits are tested
//! without a socket. Integers are big-endian.

use std::time::Duration;

use crate::{Code, NetError};

/// The ALPN of wire protocol version 1. The number is [`VERSION`].
pub const ALPN: &[u8] = b"kuula/play/1";

/// Reserved for a future deploy channel; never registered.
pub const DEPLOY_ALPN: &[u8] = b"kuula/deploy/1";

/// The protocol version carried in the handshake.
pub const VERSION: u16 = 1;

/// Handshake magic.
pub const MAGIC: [u8; 4] = *b"KUUL";

/// Longest runtime version string a handshake may carry.
pub const MAX_RUNTIME_VERSION: usize = 64;

/// Longest `Text` payload.
pub const MAX_TEXT: usize = 1024;

/// Longest `Data` payload: the cart message limit.
pub const MAX_DATA: usize = 1024;

/// Bytes of a handshake header: magic, version, length.
pub const HANDSHAKE_HEADER: usize = 8;

/// Bytes of a message header: type, length.
pub const FRAME_HEADER: usize = 5;

/// The whole handshake must be done this long after the connection is.
pub const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(5);

/// A frame whose first byte has arrived must be complete this soon.
pub const FRAME_DEADLINE: Duration = Duration::from_secs(5);

/// How long `ping` waits for its `Pong`.
pub const PING_DEADLINE: Duration = Duration::from_secs(5);

/// How long a send waits for room before the session ends.
pub const SEND_DEADLINE: Duration = Duration::from_secs(5);

/// After `Bye`, how long the sender waits for the peer's code-0 close
/// before closing itself.
pub const BYE_DEADLINE: Duration = Duration::from_secs(2);

/// How long a join waits for the connection.
pub const CONNECT_DEADLINE: Duration = Duration::from_secs(5);

/// How long a listener waits for its first direct address.
pub const ADDRESS_DEADLINE: Duration = Duration::from_secs(5);

/// QUIC keep-alive interval and idle timeout: a vanished peer is
/// `PeerLost` within about [`IDLE_TIMEOUT`].
pub const KEEP_ALIVE: Duration = Duration::from_secs(1);
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(5);

/// Bounded queues: outgoing frames and incoming events.
pub const OUTGOING_QUEUE: usize = 64;
pub const EVENT_QUEUE: usize = 256;

/// QUIC application close codes.
pub mod close {
    pub const BYE: u32 = 0;
    pub const BUSY: u32 = 1;
    pub const PROTOCOL: u32 = 2;
    pub const TIMEOUT: u32 = 3;
    pub const SHUTDOWN: u32 = 4;

    pub fn reason(code: u32) -> &'static [u8] {
        match code {
            BYE => b"bye",
            BUSY => b"busy",
            PROTOCOL => b"protocol",
            TIMEOUT => b"timeout",
            _ => b"shutdown",
        }
    }
}

/// Message type numbers.
pub const TYPE_PING: u8 = 1;
pub const TYPE_PONG: u8 = 2;
pub const TYPE_TEXT: u8 = 3;
pub const TYPE_BYE: u8 = 4;
pub const TYPE_DATA: u8 = 5;

/// One message after the handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Ping(u64),
    Pong(u64),
    Text(String),
    Bye,
    /// A cart message: 1 to 1024 bytes of anything.
    Data(Vec<u8>),
}

/// The handshake frame with our runtime version.
pub fn handshake(runtime_version: &str) -> Vec<u8> {
    let v = runtime_version.as_bytes();
    let v = &v[..v.len().min(MAX_RUNTIME_VERSION)];
    let mut out = Vec::with_capacity(HANDSHAKE_HEADER + v.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_be_bytes());
    out.extend_from_slice(&(v.len() as u16).to_be_bytes());
    out.extend_from_slice(v);
    out
}

/// Check a handshake header; the body length to read next, or the error.
pub fn handshake_header(h: &[u8; HANDSHAKE_HEADER]) -> Result<usize, NetError> {
    if h[..4] != MAGIC {
        return Err(NetError::new(Code::Handshake, "bad magic"));
    }
    let version = u16::from_be_bytes([h[4], h[5]]);
    if version != VERSION {
        return Err(NetError::new(
            Code::Handshake,
            format!("protocol version {version}, this runtime speaks {VERSION}"),
        ));
    }
    let len = u16::from_be_bytes([h[6], h[7]]) as usize;
    if len > MAX_RUNTIME_VERSION {
        return Err(NetError::new(
            Code::Handshake,
            format!("runtime version of {len} bytes, at most {MAX_RUNTIME_VERSION}"),
        ));
    }
    Ok(len)
}

/// Check a handshake body.
pub fn handshake_body(body: &[u8]) -> Result<String, NetError> {
    std::str::from_utf8(body)
        .map(str::to_string)
        .map_err(|_| NetError::new(Code::Handshake, "runtime version is not UTF-8"))
}

/// Encode a message. `Text` is checked against the limits first.
pub fn encode(frame: &Frame) -> Result<Vec<u8>, NetError> {
    let (ty, payload): (u8, &[u8]) = match frame {
        Frame::Ping(n) => (TYPE_PING, &n.to_be_bytes()),
        Frame::Pong(n) => (TYPE_PONG, &n.to_be_bytes()),
        Frame::Text(s) => {
            check_text_len(s.len())?;
            (TYPE_TEXT, s.as_bytes())
        }
        Frame::Bye => (TYPE_BYE, &[]),
        Frame::Data(d) => {
            check_data_len(d.len())?;
            (TYPE_DATA, d.as_slice())
        }
    };
    let mut out = Vec::with_capacity(FRAME_HEADER + payload.len());
    out.push(ty);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

fn check_text_len(len: usize) -> Result<(), NetError> {
    if len == 0 {
        return Err(NetError::new(Code::Frame, "empty text"));
    }
    if len > MAX_TEXT {
        return Err(NetError::new(
            Code::Frame,
            format!("text of {len} bytes, at most {MAX_TEXT}"),
        ));
    }
    Ok(())
}

fn check_data_len(len: usize) -> Result<(), NetError> {
    if len == 0 {
        return Err(NetError::new(Code::Frame, "empty data"));
    }
    if len > MAX_DATA {
        return Err(NetError::new(
            Code::Frame,
            format!("data of {len} bytes, at most {MAX_DATA}"),
        ));
    }
    Ok(())
}

/// Check a message header before anything is allocated: the payload
/// length to read, or the error.
pub fn frame_header(h: &[u8; FRAME_HEADER]) -> Result<(u8, usize), NetError> {
    let ty = h[0];
    let len = u32::from_be_bytes([h[1], h[2], h[3], h[4]]) as usize;
    match ty {
        TYPE_PING | TYPE_PONG if len != 8 => Err(NetError::new(
            Code::Frame,
            format!("ping/pong of {len} bytes, must be 8"),
        )),
        TYPE_BYE if len != 0 => Err(NetError::new(
            Code::Frame,
            format!("bye of {len} bytes, must be empty"),
        )),
        TYPE_TEXT => check_text_len(len).map(|()| (ty, len)),
        TYPE_DATA => check_data_len(len).map(|()| (ty, len)),
        TYPE_PING | TYPE_PONG | TYPE_BYE => Ok((ty, len)),
        other => Err(NetError::new(
            Code::Frame,
            format!("unknown frame type {other}"),
        )),
    }
}

/// Decode a payload whose header passed [`frame_header`].
pub fn frame_body(ty: u8, body: &[u8]) -> Result<Frame, NetError> {
    let nonce = |b: &[u8]| {
        let mut n = [0u8; 8];
        n.copy_from_slice(b);
        u64::from_be_bytes(n)
    };
    match ty {
        TYPE_PING => Ok(Frame::Ping(nonce(body))),
        TYPE_PONG => Ok(Frame::Pong(nonce(body))),
        TYPE_TEXT => std::str::from_utf8(body)
            .map(|s| Frame::Text(s.to_string()))
            .map_err(|_| NetError::new(Code::Frame, "text is not UTF-8")),
        TYPE_BYE => Ok(Frame::Bye),
        TYPE_DATA => Ok(Frame::Data(body.to_vec())),
        other => Err(NetError::new(
            Code::Frame,
            format!("unknown frame type {other}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(bytes: &[u8]) -> [u8; FRAME_HEADER] {
        let mut h = [0u8; FRAME_HEADER];
        h.copy_from_slice(&bytes[..FRAME_HEADER]);
        h
    }

    fn roundtrip(f: Frame) {
        let bytes = encode(&f).unwrap();
        let (ty, len) = frame_header(&header(&bytes)).unwrap();
        assert_eq!(len, bytes.len() - FRAME_HEADER);
        assert_eq!(frame_body(ty, &bytes[FRAME_HEADER..]).unwrap(), f);
    }

    #[test]
    fn frames_round_trip() {
        roundtrip(Frame::Ping(0x0102_0304_0506_0708));
        roundtrip(Frame::Pong(u64::MAX));
        roundtrip(Frame::Text("hei".into()));
        roundtrip(Frame::Text("x".repeat(MAX_TEXT)));
        roundtrip(Frame::Bye);
        roundtrip(Frame::Data(vec![0, 255, 254]));
        roundtrip(Frame::Data(vec![7; MAX_DATA]));
        assert_eq!(encode(&Frame::Data(vec![])).unwrap_err().code, Code::Frame);
        assert_eq!(
            encode(&Frame::Data(vec![0; MAX_DATA + 1]))
                .unwrap_err()
                .code,
            Code::Frame
        );
        let mut h = [TYPE_DATA, 0, 0, 0, 0];
        h[1..].copy_from_slice(&(MAX_DATA as u32 + 1).to_be_bytes());
        assert_eq!(frame_header(&h).unwrap_err().code, Code::Frame);
        assert!(frame_header(&[TYPE_DATA, 0, 0, 0, 0]).is_err());
    }

    #[test]
    fn integers_are_big_endian() {
        let bytes = encode(&Frame::Ping(1)).unwrap();
        assert_eq!(bytes, [1, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 1]);
        let hs = handshake("0.0.7");
        assert_eq!(&hs[..8], b"KUUL\x00\x01\x00\x05");
        assert_eq!(&hs[8..], b"0.0.7");
    }

    #[test]
    fn text_limits_are_checked_before_decoding() {
        assert_eq!(
            encode(&Frame::Text(String::new())).unwrap_err().code,
            Code::Frame
        );
        assert_eq!(
            encode(&Frame::Text("x".repeat(MAX_TEXT + 1)))
                .unwrap_err()
                .code,
            Code::Frame
        );
        // A header claiming 1 MiB is refused from the header alone.
        let mut h = [TYPE_TEXT, 0, 0, 0, 0];
        h[1..].copy_from_slice(&(1024u32 * 1024).to_be_bytes());
        let e = frame_header(&h).unwrap_err();
        assert_eq!(e.code, Code::Frame);
        assert!(e.detail.contains("1048576"), "{e}");
        assert_eq!(
            frame_body(TYPE_TEXT, &[0xff, 0xfe]).unwrap_err().code,
            Code::Frame
        );
    }

    #[test]
    fn fixed_length_frames_are_checked() {
        assert!(frame_header(&[TYPE_PING, 0, 0, 0, 7]).is_err());
        assert!(frame_header(&[TYPE_PONG, 0, 0, 0, 9]).is_err());
        assert!(frame_header(&[TYPE_BYE, 0, 0, 0, 1]).is_err());
        assert!(frame_header(&[9, 0, 0, 0, 0]).is_err());
        assert_eq!(
            frame_header(&[TYPE_BYE, 0, 0, 0, 0]).unwrap(),
            (TYPE_BYE, 0)
        );
    }

    #[test]
    fn handshake_is_checked_field_by_field() {
        let ok = handshake("kuula 0.0.7");
        let mut h = [0u8; HANDSHAKE_HEADER];
        h.copy_from_slice(&ok[..HANDSHAKE_HEADER]);
        assert_eq!(handshake_header(&h).unwrap(), 11);
        assert_eq!(
            handshake_body(&ok[HANDSHAKE_HEADER..]).unwrap(),
            "kuula 0.0.7"
        );

        let mut bad = h;
        bad[0] = b'X';
        assert_eq!(handshake_header(&bad).unwrap_err().code, Code::Handshake);
        let mut bad = h;
        bad[5] = 2;
        let e = handshake_header(&bad).unwrap_err();
        assert_eq!(e.code, Code::Handshake);
        assert!(e.detail.contains("version 2"), "{e}");
        let mut bad = h;
        bad[6..].copy_from_slice(&65u16.to_be_bytes());
        assert_eq!(handshake_header(&bad).unwrap_err().code, Code::Handshake);
        assert!(handshake_body(&[0xff]).is_err());
        // A long runtime version is truncated on our side, never refused.
        let long = handshake(&"v".repeat(200));
        assert_eq!(long.len(), HANDSHAKE_HEADER + MAX_RUNTIME_VERSION);
    }
}
