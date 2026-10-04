//! The deploy wire, version 1: the offer, the answer byte and the result
//! frame, the limits and the stable codes.
//! Everything here is
//! pure: bytes in, bytes or a refusal out, every limit checked before
//! anything is allocated or written, so the refusals are tested without a
//! socket. Integers are big-endian.

use std::fmt;
use std::time::Duration;

/// Offer magic.
pub const MAGIC: [u8; 4] = *b"KUUD";

/// The protocol version carried in the offer; the ALPN's number.
pub const VERSION: u16 = 1;

/// Shortest and longest cart name, in bytes.
pub const MIN_NAME: usize = 1;
pub const MAX_NAME: usize = 32;

/// Largest package: the snapshot's total limit.
pub const MAX_PACKAGE: u32 = 64 * 1024 * 1024;

/// Bytes of the offer before the name: magic, version, name length.
pub const OFFER_PREFIX: usize = 7;

/// Bytes of the offer after the name: package length and SHA-256.
pub const OFFER_TAIL: usize = 36;

/// Bytes of the result header: status, restart, detail length.
pub const RESULT_HEADER: usize = 4;

/// Longest result detail.
pub const MAX_DETAIL: usize = 512;

/// Bytes read or written at a time.
pub const CHUNK: usize = 64 * 1024;

/// The offer must be complete this long after the connection is.
pub const OFFER_DEADLINE: Duration = Duration::from_secs(5);

/// How long a data read or write may make no progress before the
/// transfer counts as interrupted.
pub const STALL_DEADLINE: Duration = Duration::from_secs(5);

/// How long the sender waits for the answer byte.
pub const ANSWER_DEADLINE: Duration = Duration::from_secs(5);

/// How long the sender waits for the result after its last byte.
pub const RESULT_DEADLINE: Duration = Duration::from_secs(15);

/// How long the receiver waits for the console to report the restart.
pub const RESTART_DEADLINE: Duration = Duration::from_secs(3);

/// After the result, how long the receiver waits for the sender to
/// close before closing itself.
pub const LINGER: Duration = Duration::from_secs(2);

/// QUIC application close codes of the deploy channel. 0 to 4 mean what
/// they mean in `kuula/play/1`.
pub mod close {
    pub const BYE: u32 = 0;
    pub const BUSY: u32 = 1;
    pub const PROTOCOL: u32 = 2;
    pub const TIMEOUT: u32 = 3;
    pub const SHUTDOWN: u32 = 4;
    pub const UNPAIRED: u32 = 5;

    pub fn reason(code: u32) -> &'static [u8] {
        match code {
            BYE => b"bye",
            BUSY => b"busy",
            PROTOCOL => b"protocol",
            TIMEOUT => b"timeout",
            UNPAIRED => b"unpaired",
            _ => b"shutdown",
        }
    }
}

/// The stable deploy codes. Connection failures keep the `net_*` codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeployCode {
    Ok,
    Unpaired,
    Busy,
    Offer,
    TooLarge,
    Digest,
    Invalid,
    Interrupted,
    Install,
    Cancelled,
}

impl DeployCode {
    pub const ALL: [DeployCode; 10] = [
        DeployCode::Ok,
        DeployCode::Unpaired,
        DeployCode::Busy,
        DeployCode::Offer,
        DeployCode::TooLarge,
        DeployCode::Digest,
        DeployCode::Invalid,
        DeployCode::Interrupted,
        DeployCode::Install,
        DeployCode::Cancelled,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            DeployCode::Ok => "deploy_ok",
            DeployCode::Unpaired => "deploy_unpaired",
            DeployCode::Busy => "deploy_busy",
            DeployCode::Offer => "deploy_offer",
            DeployCode::TooLarge => "deploy_too_large",
            DeployCode::Digest => "deploy_digest",
            DeployCode::Invalid => "deploy_invalid",
            DeployCode::Interrupted => "deploy_interrupted",
            DeployCode::Install => "deploy_install",
            DeployCode::Cancelled => "deploy_cancelled",
        }
    }

    /// The byte that carries this code in an answer or a result.
    /// `deploy_unpaired` and `deploy_busy` have none: they are close
    /// codes 5 and 1, because a refused connection never gets a stream.
    pub fn status(self) -> Option<u8> {
        match self {
            DeployCode::Ok => Some(0),
            DeployCode::Offer => Some(1),
            DeployCode::TooLarge => Some(2),
            DeployCode::Digest => Some(3),
            DeployCode::Invalid => Some(4),
            DeployCode::Interrupted => Some(5),
            DeployCode::Install => Some(6),
            DeployCode::Cancelled => Some(7),
            DeployCode::Unpaired | DeployCode::Busy => None,
        }
    }

    pub fn from_status(byte: u8) -> Option<DeployCode> {
        DeployCode::ALL
            .into_iter()
            .find(|c| c.status() == Some(byte))
    }
}

impl fmt::Display for DeployCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What happened to the running cart after an install.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Restart {
    /// The console loaded the installed cart and it is running.
    Started,
    /// The console loaded it and it faulted; the detail carries the code.
    Faulted,
    /// Nothing was asked of a console: the receiver has none, or
    /// nothing was installed.
    NotRun,
    /// The console did not answer within 3 s.
    Timeout,
}

impl Restart {
    pub fn as_str(self) -> &'static str {
        match self {
            Restart::Started => "started",
            Restart::Faulted => "faulted",
            Restart::NotRun => "not_run",
            Restart::Timeout => "timeout",
        }
    }

    pub fn byte(self) -> u8 {
        match self {
            Restart::Started => 0,
            Restart::Faulted => 1,
            Restart::NotRun => 2,
            Restart::Timeout => 3,
        }
    }

    pub fn from_byte(b: u8) -> Option<Restart> {
        [
            Restart::Started,
            Restart::Faulted,
            Restart::NotRun,
            Restart::Timeout,
        ]
        .into_iter()
        .find(|r| r.byte() == b)
    }
}

impl fmt::Display for Restart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A refused offer or result: the code and a human-readable detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: DeployCode,
    pub detail: String,
}

impl Refusal {
    pub fn new(code: DeployCode, detail: impl Into<String>) -> Refusal {
        Refusal {
            code,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.code, self.detail)
    }
}

impl std::error::Error for Refusal {}

/// A decoded offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub name: String,
    pub len: u32,
    pub digest: [u8; 32],
}

/// Whether `name` is one Windows keeps for a device, with any extension:
/// `<carts>/con.cart` cannot be created there.
fn is_device_name(name: &str) -> bool {
    match name {
        "con" | "prn" | "aux" | "nul" => true,
        _ => {
            name.len() == 4
                && (name.starts_with("com") || name.starts_with("lpt"))
                && name.as_bytes()[3].is_ascii_digit()
        }
    }
}

/// A cart name: 1 to 32 bytes of `[a-z0-9_-]`, and not a Windows device
/// name, whatever the receiver runs on.
pub fn check_name(name: &str) -> Result<(), Refusal> {
    let len = name.len();
    if !(MIN_NAME..=MAX_NAME).contains(&len) {
        return Err(Refusal::new(
            DeployCode::Offer,
            format!("cart name of {len} bytes, must be {MIN_NAME} to {MAX_NAME}"),
        ));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Err(Refusal::new(
            DeployCode::Offer,
            "cart name may only use a-z, 0-9, '_' and '-'",
        ));
    }
    if is_device_name(name) {
        return Err(Refusal::new(
            DeployCode::Offer,
            format!("cart name '{name}' is a device name on Windows"),
        ));
    }
    Ok(())
}

/// Check the package length against the limits.
pub fn check_len(len: u32) -> Result<(), Refusal> {
    if len == 0 {
        return Err(Refusal::new(DeployCode::Offer, "package of 0 bytes"));
    }
    if len > MAX_PACKAGE {
        return Err(Refusal::new(
            DeployCode::TooLarge,
            format!("package of {len} bytes, at most {MAX_PACKAGE}"),
        ));
    }
    Ok(())
}

/// Encode an offer; the same checks as the receiver's apply first.
pub fn encode_offer(name: &str, len: u32, digest: &[u8; 32]) -> Result<Vec<u8>, Refusal> {
    check_name(name)?;
    check_len(len)?;
    let mut out = Vec::with_capacity(OFFER_PREFIX + name.len() + OFFER_TAIL);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_be_bytes());
    out.push(name.len() as u8);
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(digest);
    Ok(out)
}

/// Check the first seven bytes of an offer; the name length to read
/// next. Nothing is allocated for a name that cannot be valid.
pub fn offer_prefix(p: &[u8; OFFER_PREFIX]) -> Result<usize, Refusal> {
    if p[..4] != MAGIC {
        return Err(Refusal::new(DeployCode::Offer, "bad magic"));
    }
    let version = u16::from_be_bytes([p[4], p[5]]);
    if version != VERSION {
        return Err(Refusal::new(
            DeployCode::Offer,
            format!("protocol version {version}, this receiver speaks {VERSION}"),
        ));
    }
    let n = p[6] as usize;
    if !(MIN_NAME..=MAX_NAME).contains(&n) {
        return Err(Refusal::new(
            DeployCode::Offer,
            format!("cart name of {n} bytes, must be {MIN_NAME} to {MAX_NAME}"),
        ));
    }
    Ok(n)
}

/// Check the name bytes of an offer.
pub fn offer_name(name: &[u8]) -> Result<String, Refusal> {
    let name = std::str::from_utf8(name)
        .map_err(|_| Refusal::new(DeployCode::Offer, "cart name is not UTF-8"))?;
    check_name(name)?;
    Ok(name.to_string())
}

/// Check the last 36 bytes of an offer: the package length against the
/// limits, before any staging file exists.
pub fn offer_tail(name: String, t: &[u8; OFFER_TAIL]) -> Result<Offer, Refusal> {
    let len = u32::from_be_bytes([t[0], t[1], t[2], t[3]]);
    check_len(len)?;
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&t[4..]);
    Ok(Offer { name, len, digest })
}

/// Encode a result frame. A detail over 512 bytes is cut at a character
/// boundary.
pub fn encode_result(code: DeployCode, restart: Restart, detail: &str) -> Vec<u8> {
    // Busy and unpaired are close codes and never reach a result; a
    // caller that passes one anyway gets the generic install failure.
    let status = code.status().unwrap_or(6);
    let mut end = detail.len().min(MAX_DETAIL);
    while !detail.is_char_boundary(end) {
        end -= 1;
    }
    let detail = &detail.as_bytes()[..end];
    let mut out = Vec::with_capacity(RESULT_HEADER + detail.len());
    out.push(status);
    out.push(restart.byte());
    out.extend_from_slice(&(detail.len() as u16).to_be_bytes());
    out.extend_from_slice(detail);
    out
}

/// Check a result header: the code, the restart status and the detail
/// length to read next, which is at most 512.
pub fn result_header(h: &[u8; RESULT_HEADER]) -> Result<(DeployCode, Restart, usize), String> {
    let code =
        DeployCode::from_status(h[0]).ok_or_else(|| format!("unknown result status {}", h[0]))?;
    let restart =
        Restart::from_byte(h[1]).ok_or_else(|| format!("unknown restart status {}", h[1]))?;
    let n = u16::from_be_bytes([h[2], h[3]]) as usize;
    if n > MAX_DETAIL {
        return Err(format!("result detail of {n} bytes, at most {MAX_DETAIL}"));
    }
    Ok((code, restart, n))
}

/// Check a result detail.
pub fn result_detail(body: &[u8]) -> Result<String, String> {
    std::str::from_utf8(body)
        .map(str::to_string)
        .map_err(|_| "result detail is not UTF-8".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer_bytes(name: &str, len: u32) -> Vec<u8> {
        encode_offer(name, len, &[9; 32]).unwrap()
    }

    /// Decode an offer the way the receiver reads it.
    fn decode(bytes: &[u8]) -> Result<Offer, Refusal> {
        let mut p = [0u8; OFFER_PREFIX];
        p.copy_from_slice(&bytes[..OFFER_PREFIX]);
        let n = offer_prefix(&p)?;
        let name = offer_name(&bytes[OFFER_PREFIX..OFFER_PREFIX + n])?;
        let mut t = [0u8; OFFER_TAIL];
        t.copy_from_slice(&bytes[OFFER_PREFIX + n..]);
        offer_tail(name, &t)
    }

    #[test]
    fn an_offer_round_trips_and_is_big_endian() {
        let bytes = offer_bytes("hello", 0x0102_0304);
        assert_eq!(&bytes[..7], b"KUUD\x00\x01\x05");
        assert_eq!(&bytes[7..12], b"hello");
        assert_eq!(&bytes[12..16], [1, 2, 3, 4]);
        assert_eq!(bytes.len(), OFFER_PREFIX + 5 + OFFER_TAIL);
        let offer = decode(&bytes).unwrap();
        assert_eq!(offer.name, "hello");
        assert_eq!(offer.len, 0x0102_0304);
        assert_eq!(offer.digest, [9; 32]);
    }

    #[test]
    fn every_offer_refusal_comes_before_any_allocation() {
        // Magic, version and name length are refused from the seven
        // prefix bytes alone, before the name is read.
        let good = offer_bytes("a", 1);
        let prefix = |edit: &dyn Fn(&mut [u8; OFFER_PREFIX])| {
            let mut p = [0u8; OFFER_PREFIX];
            p.copy_from_slice(&good[..OFFER_PREFIX]);
            edit(&mut p);
            offer_prefix(&p)
        };
        assert_eq!(prefix(&|_| {}).unwrap(), 1);
        let e = prefix(&|p| p[0] = b'X').unwrap_err();
        assert_eq!(
            (e.code, e.detail.as_str()),
            (DeployCode::Offer, "bad magic")
        );
        let e = prefix(&|p| p[5] = 2).unwrap_err();
        assert_eq!(e.code, DeployCode::Offer);
        assert!(e.detail.contains("version 2"), "{e}");
        assert_eq!(prefix(&|p| p[6] = 0).unwrap_err().code, DeployCode::Offer);
        let e = prefix(&|p| p[6] = 33).unwrap_err();
        assert_eq!(e.code, DeployCode::Offer);
        assert!(e.detail.contains("33 bytes"), "{e}");
        assert_eq!(prefix(&|p| p[6] = 255).unwrap_err().code, DeployCode::Offer);

        // Names: characters, case, length, UTF-8.
        for bad in ["Hello", "a b", "a.b", "a/b", "../x", "\u{e4}", "A"] {
            assert_eq!(
                check_name(bad).unwrap_err().code,
                DeployCode::Offer,
                "{bad}"
            );
        }
        assert_eq!(offer_name(&[0xff]).unwrap_err().code, DeployCode::Offer);
        assert!(check_name("a_b-9").is_ok());
        assert!(check_name(&"x".repeat(32)).is_ok());
        assert_eq!(
            check_name(&"x".repeat(33)).unwrap_err().code,
            DeployCode::Offer
        );
        assert_eq!(check_name("").unwrap_err().code, DeployCode::Offer);

        // Names Windows keeps for devices would fail at the install, after
        // the whole transfer: they are refused with the offer, and the
        // sender's own encoder refuses them first.
        for device in ["con", "prn", "aux", "nul", "com0", "com1", "com9", "lpt1"] {
            let e = check_name(device).unwrap_err();
            assert_eq!(e.code, DeployCode::Offer, "{device}");
            assert!(e.detail.contains("device name"), "{e}");
            assert!(offer_name(device.as_bytes()).is_err(), "{device}");
            assert!(encode_offer(device, 1, &[0; 32]).is_err(), "{device}");
        }
        for fine in [
            "cons", "com", "com10", "comx", "lpt", "null", "con-1", "a-con",
        ] {
            assert!(check_name(fine).is_ok(), "{fine}");
        }

        // Lengths: zero is an offer fault, over the limit is too large,
        // and the limit itself is allowed.
        let tail = |len: u32| {
            let mut t = [0u8; OFFER_TAIL];
            t[..4].copy_from_slice(&len.to_be_bytes());
            offer_tail("a".into(), &t)
        };
        assert_eq!(tail(0).unwrap_err().code, DeployCode::Offer);
        assert_eq!(
            tail(MAX_PACKAGE + 1).unwrap_err().code,
            DeployCode::TooLarge
        );
        assert_eq!(tail(u32::MAX).unwrap_err().code, DeployCode::TooLarge);
        assert_eq!(tail(MAX_PACKAGE).unwrap().len, MAX_PACKAGE);
        assert_eq!(tail(1).unwrap().len, 1);

        // The sender's encoder applies the same limits.
        assert_eq!(
            encode_offer("a", MAX_PACKAGE + 1, &[0; 32])
                .unwrap_err()
                .code,
            DeployCode::TooLarge
        );
        assert_eq!(
            encode_offer("Bad", 1, &[0; 32]).unwrap_err().code,
            DeployCode::Offer
        );
    }

    #[test]
    fn a_result_round_trips_and_its_detail_is_bounded() {
        let bytes = encode_result(
            DeployCode::Invalid,
            Restart::NotRun,
            "limit_exceeded x: big",
        );
        assert_eq!(&bytes[..4], [4, 2, 0, 21]);
        let (code, restart, n) = result_header(&bytes[..4].try_into().unwrap()).unwrap();
        assert_eq!(
            (code, restart, n),
            (DeployCode::Invalid, Restart::NotRun, 21)
        );
        assert_eq!(result_detail(&bytes[4..]).unwrap(), "limit_exceeded x: big");

        // Cut at a character boundary, never over 512 bytes.
        let long = "\u{e4}".repeat(400);
        let bytes = encode_result(DeployCode::Invalid, Restart::NotRun, &long);
        assert!(bytes.len() - RESULT_HEADER <= MAX_DETAIL);
        assert!(result_detail(&bytes[RESULT_HEADER..]).is_ok());

        // A header claiming more than 512 is refused before the read.
        let mut h = [0u8, 0, 0, 0];
        h[2..].copy_from_slice(&513u16.to_be_bytes());
        assert!(result_header(&h).unwrap_err().contains("513"));
        h[2..].copy_from_slice(&512u16.to_be_bytes());
        assert_eq!(result_header(&h).unwrap().2, 512);
        assert!(result_header(&[99, 0, 0, 0]).is_err());
        assert!(result_header(&[0, 99, 0, 0]).is_err());
        assert!(result_detail(&[0xff]).is_err());
    }

    #[test]
    fn codes_and_statuses_are_stable() {
        let names: Vec<&str> = DeployCode::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(
            names,
            [
                "deploy_ok",
                "deploy_unpaired",
                "deploy_busy",
                "deploy_offer",
                "deploy_too_large",
                "deploy_digest",
                "deploy_invalid",
                "deploy_interrupted",
                "deploy_install",
                "deploy_cancelled",
            ]
        );
        for code in DeployCode::ALL {
            if let Some(s) = code.status() {
                assert_eq!(DeployCode::from_status(s), Some(code));
            }
        }
        assert_eq!(DeployCode::Busy.status(), None);
        assert_eq!(DeployCode::Unpaired.status(), None);
        assert_eq!(DeployCode::from_status(8), None);
        for r in [
            Restart::Started,
            Restart::Faulted,
            Restart::NotRun,
            Restart::Timeout,
        ] {
            assert_eq!(Restart::from_byte(r.byte()), Some(r));
        }
        assert_eq!(Restart::from_byte(4), None);
        assert_eq!(
            [
                Restart::Started,
                Restart::Faulted,
                Restart::NotRun,
                Restart::Timeout
            ]
            .map(|r| r.as_str()),
            ["started", "faulted", "not_run", "timeout"]
        );
    }
}
