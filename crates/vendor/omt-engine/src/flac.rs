//! FLAC streams of 16-bit samples, read as RFC 9639 defines FLAC (docs/omt.md section 9): `fLaC`, a
//! STREAMINFO block, the other metadata blocks skipped by their lengths and never parsed, then
//! frames to the stream's last byte. Every coding RFC 9639 defines for such frames is decoded,
//! escape-coded Rice partitions included, and a value it forbids is damage. The decoder is exact
//! integer arithmetic; it is small because a sample record's FLAC is 16-bit and 1 or 2 channels.

/// The interleaved samples of a FLAC stream of `channels` channels and `frames` frames of 16-bit
/// samples, or `None` when the stream is damaged: bytes before `fLaC` or after the last frame, a
/// STREAMINFO other than section 9 describes, a frame whose header CRC-8 or CRC-16 fails or that
/// holds a value RFC 9639 forbids, a stream ending inside a frame, frames decoding to other than
/// the record's frames or channels or to other than 16-bit samples, or a STREAMINFO total that
/// isn't 0 (unknown) and differs from the frames decoded. The MD5, the sample rate and the frame
/// sizes aren't checked.
pub fn decode(bytes: &[u8], channels: u32, frames: u32) -> Option<Vec<i16>> {
    let (info, data) = header(bytes)?;
    if info.channels != channels || info.bits != 16 {
        return None;
    }
    let want = frames as usize * channels as usize;
    let out = body(data, &info, want)?;
    (out.len() == want).then_some(out)
}

/// What a cue's FLAC file says of itself (OMQ section 4), without decoding it.
pub struct Info {
    pub rate: u32,
    pub channels: u32,
    pub bits: u32,
    /// STREAMINFO's total samples of a channel, 0 when unknown.
    pub total: u64,
}

/// STREAMINFO of a stream that starts with `fLaC` and whose metadata blocks are whole, or `None`.
pub fn info(bytes: &[u8]) -> Option<Info> {
    let (i, _) = header(bytes)?;
    Some(Info { rate: i.rate, channels: i.channels, bits: i.bits, total: i.total })
}

/// What a cue's FLAC says of its kind, and nothing else (OMQ section 4): STREAMINFO's fixed fields,
/// read when the payload has at least 42 bytes and its first metadata block is of type 0 and 34
/// bytes long, or `None`, a FLAC without its STREAMINFO. The rest of the metadata and the frames
/// aren't looked at: a file whose header says it isn't of the kind is left to the player however
/// damaged it is, and only one of the kind can be damaged.
pub fn stream_info(bytes: &[u8]) -> Option<Info> {
    let rest = bytes.strip_prefix(b"fLaC")?;
    if bytes.len() < 42 || rest[0] & 0x7f != 0 || u32::from_be_bytes([0, rest[1], rest[2], rest[3]]) != 34 {
        return None;
    }
    let (rate, channels, bits, total) = fixed_fields(&rest[4..38]);
    Some(Info { rate, channels, bits, total })
}

/// The interleaved samples of a FLAC stream of 16-bit samples, its channels and rate as STREAMINFO
/// gives them, and at most `max_frames` frames: the damage section 9 lists, a stream that isn't 16
/// bits, or one of more frames than that, is `None`. A cue's file (OMQ section 4) is read this way,
/// with no sample record to say what it holds.
pub fn decode_stream(bytes: &[u8], max_frames: u32) -> Option<Vec<i16>> {
    let (info, data) = header(bytes)?;
    if info.bits != 16 {
        return None;
    }
    body(data, &info, max_frames as usize * info.channels as usize)
}

/// STREAMINFO and the stream's frames: the metadata blocks, STREAMINFO first and the others
/// skipped by their lengths.
fn header(bytes: &[u8]) -> Option<(StreamInfo, &[u8])> {
    let rest = bytes.strip_prefix(b"fLaC")?;
    let (mut at, mut info) = (0, None);
    loop {
        let header = rest.get(at..at + 4)?;
        let len = u32::from_be_bytes([0, header[1], header[2], header[3]]) as usize;
        let body = rest.get(at + 4..at + 4 + len)?;
        // Type 127 is forbidden (RFC 9639 section 8.1); every other block but the first is
        // skipped unread.
        if header[0] & 0x7f == 127 {
            return None;
        }
        if at == 0 {
            if header[0] & 0x7f != 0 {
                return None;
            }
            info = Some(StreamInfo::read(body)?);
        }
        at += 4 + len;
        if header[0] & 0x80 != 0 {
            break;
        }
    }
    Some((info?, &rest[at..]))
}

/// The frames of a stream, `data`, as the samples of `info.channels` channels, at most `want` of
/// them in all.
fn body(data: &[u8], info: &StreamInfo, want: usize) -> Option<Vec<i16>> {
    let channels = info.channels;
    let mut out = Vec::with_capacity(want.min(1 << 20));
    let (mut pos, mut strategy, mut index) = (0, None, 0u64);
    while pos < data.len() {
        // The frames before this one, and the samples of a channel before it: what its coded
        // number is.
        let position = (index, (out.len() / channels as usize) as u64);
        let before = out.len();
        let (len, variable) = frame(&data[pos..], channels, position, &mut out, want)?;
        index += 1;
        // The frame's block size within STREAMINFO's bounds, the last frame's at most the maximum.
        let block = (out.len() - before) / channels as usize;
        let last = pos + len == data.len();
        if block > info.max_block || (!last && block < info.min_block) {
            return None;
        }
        // The blocking strategy MUST NOT change during the stream.
        if strategy.is_some_and(|s| s != variable) {
            return None;
        }
        strategy = Some(variable);
        pos += len;
    }
    let decoded = (out.len() / channels as usize) as u64;
    if info.total != 0 && info.total != decoded {
        return None;
    }
    Some(out)
}

/// What section 9 reads of STREAMINFO.
struct StreamInfo {
    rate: u32,
    /// The minimum and maximum block sizes: every frame's is within them, the last one's at most
    /// the maximum (RFC 9639 section 8.2).
    min_block: usize,
    max_block: usize,
    channels: u32,
    bits: u32,
    total: u64,
}

impl StreamInfo {
    /// 34 bytes; the minimum block size at least 16 and at most the maximum. The sample rate and
    /// the frame sizes aren't checked, nor the MD5.
    fn read(b: &[u8]) -> Option<StreamInfo> {
        if b.len() != 34 {
            return None;
        }
        let (min, max) = (u16::from_be_bytes([b[0], b[1]]), u16::from_be_bytes([b[2], b[3]]));
        if min < 16 || min > max {
            return None;
        }
        let (rate, channels, bits, total) = fixed_fields(b);
        Some(StreamInfo { rate, min_block: min as usize, max_block: max as usize, channels, bits, total })
    }
}

/// STREAMINFO's sample rate, channels, bits per sample and total samples (RFC 9639 section 8.2),
/// from its 34 bytes.
fn fixed_fields(b: &[u8]) -> (u32, u32, u32, u64) {
    (
        (b[10] as u32) << 12 | (b[11] as u32) << 4 | (b[12] >> 4) as u32,
        ((b[12] >> 1) & 7) as u32 + 1,
        (((b[12] & 1) << 4) | (b[13] >> 4)) as u32 + 1,
        ((b[13] & 0x0f) as u64) << 32 | u32::from_be_bytes([b[14], b[15], b[16], b[17]]) as u64,
    )
}

/// CRC-8 of a frame header (RFC 9639 section 9.1.8): polynomial x^8 + x^2 + x + 1, initial 0.
pub(crate) fn crc8(bytes: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in bytes {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
        }
    }
    crc
}

/// CRC-16 of a frame (section 9.3): polynomial x^16 + x^15 + x^2 + 1, initial 0.
pub(crate) fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in bytes {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x8005 } else { crc << 1 };
        }
    }
    crc
}

/// Bits, most significant first.
struct Bits<'a> {
    data: &'a [u8],
    bit: usize,
}

impl Bits<'_> {
    fn read(&mut self, n: u32) -> Option<u64> {
        let (mut v, mut n) = (0u64, n);
        while n > 0 {
            let byte = *self.data.get(self.bit / 8)? as u64;
            let left = 8 - (self.bit % 8) as u32; // bits left in this byte
            let take = left.min(n);
            v = v << take | (byte >> (left - take)) & ((1 << take) - 1);
            self.bit += take as usize;
            n -= take;
        }
        Some(v)
    }

    /// A signed two's complement number of `n` bits.
    fn signed(&mut self, n: u32) -> Option<i64> {
        let v = self.read(n)? as i64;
        Some(if n > 0 && v >> (n - 1) & 1 == 1 { v - (1 << n) } else { v })
    }

    /// Zero bits up to a one bit: their count.
    fn unary(&mut self) -> Option<u64> {
        let mut n = 0;
        loop {
            let byte = *self.data.get(self.bit / 8)?;
            let rest = byte << (self.bit % 8); // this byte's unread bits, at the top
            let left = 8 - self.bit % 8;
            let zeros = (rest.leading_zeros() as usize).min(left);
            if zeros < left {
                self.bit += zeros + 1;
                return Some(n + zeros as u64);
            }
            n += left as u64;
            self.bit += left;
        }
    }
}

/// Decodes one frame at the start of `data` into `out`, interleaved: its length in bytes and its
/// blocking strategy (variable or fixed), or `None` when it is damaged or would take `out` past
/// `want` samples. `position` is the frames and the samples of a channel before it, which its
/// coded number must be (section 9.1.5).
fn frame(data: &[u8], channels: u32, position: (u64, u64), out: &mut Vec<i16>, want: usize) -> Option<(usize, bool)> {
    let mut b = Bits { data, bit: 0 };
    // The sync code, 0b111111111111100, and the blocking strategy.
    if b.read(15)? != 0x7ffc {
        return None;
    }
    let variable = b.read(1)? == 1;
    let size_bits = b.read(4)?;
    let rate_bits = b.read(4)?;
    let assignment = b.read(4)?;
    let depth_bits = b.read(3)?;
    if b.read(1)? != 0 || rate_bits == 0b1111 || size_bits == 0 {
        return None; // the reserved bit, a forbidden rate, a reserved block size
    }
    // The frame's channels, which must be the record's, and its bit depth, 16 (code 100) or
    // STREAMINFO's (code 000), which is 16.
    let frame_channels = match assignment {
        0..=7 => assignment as u32 + 1,
        8..=10 => 2,
        _ => return None,
    };
    if frame_channels != channels || !(depth_bits == 0b100 || depth_bits == 0) {
        return None;
    }
    // The coded number, UTF-8-like, at most 7 bytes (36 bits), 6 (31 bits) for a frame number.
    let first = b.read(8)?;
    let extra = match first {
        0x00..=0x7f => 0,
        0xc0..=0xdf => 1,
        0xe0..=0xef => 2,
        0xf0..=0xf7 => 3,
        0xf8..=0xfb => 4,
        0xfc..=0xfd => 5,
        0xfe if variable => 6,
        _ => return None,
    };
    let mut number = first & (0x7f >> extra); // the lead byte's value bits
    for _ in 0..extra {
        if b.read(2)? != 0b10 {
            return None;
        }
        number = number << 6 | b.read(6)?;
    }
    // Not overlong (RFC 3629's rule, which section 9.1.5 follows): each length has a least value.
    const LEAST: [u64; 7] = [0, 0x80, 0x800, 0x1_0000, 0x20_0000, 0x400_0000, 0x8000_0000];
    if number < LEAST[extra] {
        return None;
    }
    // The frame's index in a fixed block size stream, its first sample's in a variable one.
    if number != if variable { position.1 } else { position.0 } {
        return None;
    }
    let block = match size_bits {
        1 => 192,
        2..=5 => 144 << size_bits,
        6 => b.read(8)? as usize + 1,
        7 => {
            let v = b.read(16)? as usize;
            if v == 65535 {
                return None; // a block size of 65536 is forbidden
            }
            v + 1
        }
        _ => 1 << size_bits,
    };
    match rate_bits {
        0b1100 => {
            b.read(8)?;
        }
        0b1101 | 0b1110 => {
            b.read(16)?;
        }
        _ => {}
    }
    let header_len = b.bit / 8;
    if crc8(&data[..header_len]) != b.read(8)? as u8 {
        return None;
    }
    // The subframes, then the channels restored.
    let mut subframes: Vec<Vec<i64>> = Vec::with_capacity(channels as usize);
    for c in 0..channels {
        let side = matches!((assignment, c), (8, 1) | (9, 0) | (10, 1));
        subframes.push(subframe(&mut b, block, 16 + side as u32)?);
    }
    // Zero bits to the byte, then the CRC-16 of the frame.
    while b.bit % 8 != 0 {
        if b.read(1)? != 0 {
            return None;
        }
    }
    let end = b.bit / 8;
    let crc = b.read(16)? as u16;
    if crc16(&data[..end]) != crc {
        return None;
    }
    // A block of 1 to 15 samples is only for the last frame.
    if block < 16 && end + 2 < data.len() {
        return None;
    }
    if out.len() + block * channels as usize > want {
        return None;
    }
    for i in 0..block {
        let (l, r) = match assignment {
            8 => (subframes[0][i], subframes[0][i] - subframes[1][i]),
            9 => (subframes[0][i] + subframes[1][i], subframes[1][i]),
            10 => {
                let side = subframes[1][i];
                let mid = subframes[0][i] << 1 | (side & 1);
                ((mid + side) >> 1, (mid - side) >> 1)
            }
            _ => {
                for s in &subframes {
                    out.push(i16::try_from(s[i]).ok()?);
                }
                continue;
            }
        };
        out.push(i16::try_from(l).ok()?);
        out.push(i16::try_from(r).ok()?);
    }
    Some((end + 2, variable))
}

/// One subframe of `block` samples at bit depth `depth` (17 for a side channel), wasted bits
/// restored.
fn subframe(b: &mut Bits, block: usize, depth: u32) -> Option<Vec<i64>> {
    if b.read(1)? != 0 {
        return None;
    }
    let kind = b.read(6)?;
    let wasted = if b.read(1)? == 1 { b.unary()? as u32 + 1 } else { 0 };
    if wasted >= depth {
        return None; // the bit depth left must be larger than zero
    }
    let bits = depth - wasted;
    let fits = |v: i64| v >= -(1 << (bits - 1)) && v < 1 << (bits - 1);
    let mut s: Vec<i64> = Vec::with_capacity(block);
    match kind {
        0 => {
            let v = b.signed(bits)?;
            s.resize(block, v);
        }
        1 => {
            for _ in 0..block {
                s.push(b.signed(bits)?);
            }
        }
        8..=12 => {
            let order = (kind - 8) as usize;
            if order > block {
                return None;
            }
            for _ in 0..order {
                s.push(b.signed(bits)?);
            }
            let residual = residual(b, block, order)?;
            for r in residual {
                let n = s.len();
                let p = match order {
                    0 => 0,
                    1 => s[n - 1],
                    2 => 2 * s[n - 1] - s[n - 2],
                    3 => 3 * s[n - 1] - 3 * s[n - 2] + s[n - 3],
                    _ => 4 * s[n - 1] - 6 * s[n - 2] + 4 * s[n - 3] - s[n - 4],
                };
                let v = p + r;
                if !fits(v) {
                    return None;
                }
                s.push(v);
            }
        }
        32..=63 => {
            let order = (kind - 31) as usize;
            if order > block {
                return None;
            }
            for _ in 0..order {
                s.push(b.signed(bits)?);
            }
            let precision = b.read(4)? as u32 + 1;
            // A precision of 16 (0b1111) is forbidden, and so is a negative shift (RFC 9639
            // sections 9.2.6 and B.4).
            let shift = b.signed(5)?;
            if precision == 16 || shift < 0 {
                return None;
            }
            let coefficients: Vec<i64> = (0..order).map(|_| b.signed(precision)).collect::<Option<_>>()?;
            let residual = residual(b, block, order)?;
            for r in residual {
                let n = s.len();
                let sum: i64 = coefficients.iter().enumerate().map(|(j, c)| c * s[n - 1 - j]).sum();
                let v = (sum >> shift) + r;
                if !fits(v) {
                    return None;
                }
                s.push(v);
            }
        }
        _ => return None, // a reserved subframe type
    }
    if s.iter().any(|&v| !fits(v)) {
        return None;
    }
    Some(s.into_iter().map(|v| v << wasted).collect())
}

/// A coded residual (section 9.2.7): partitioned Rice codes with 4- or 5-bit parameters, escaped
/// partitions included. Every residual is below 2^31 in magnitude.
fn residual(b: &mut Bits, block: usize, order: usize) -> Option<Vec<i64>> {
    let parameter_bits = match b.read(2)? {
        0 => 4,
        1 => 5,
        _ => return None,
    };
    let escape = (1 << parameter_bits) - 1;
    let partition_order = b.read(4)? as u32;
    let partitions = 1usize << partition_order;
    if block % partitions != 0 || block >> partition_order <= order {
        return None;
    }
    let mut out = Vec::with_capacity(block - order);
    for p in 0..partitions {
        let count = (block >> partition_order) - if p == 0 { order } else { 0 };
        let parameter = b.read(parameter_bits)?;
        if parameter == escape {
            let n = b.read(5)? as u32;
            for _ in 0..count {
                out.push(b.signed(n)?);
            }
        } else {
            for _ in 0..count {
                let q = b.unary()?;
                if q > u32::MAX as u64 >> parameter {
                    return None;
                }
                let folded = q << parameter | b.read(parameter as u32)?;
                let v = if folded & 1 == 0 { (folded >> 1) as i64 } else { -((folded >> 1) as i64) - 1 };
                out.push(v);
            }
        }
    }
    if out.iter().any(|v| v.abs() >= 1 << 31) {
        return None;
    }
    Some(out)
}

#[cfg(test)]
#[path = "flac_tests.rs"]
mod tests;
