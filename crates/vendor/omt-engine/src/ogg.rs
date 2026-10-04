//! Ogg Opus streams (RFC 7845) recognised from their headers without decoding them: what a sample
//! record's `channels` and `frames` are checked against (docs/omt.md, section 9). This engine
//! doesn't decode Opus; a program that does gives it the PCM.

/// What the headers of an Ogg Opus stream say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opus {
    pub channels: u32,
    /// Decoded frames on the 48 kHz timeline, from the first audible one, when the pages say.
    pub frames: Option<u64>,
}

/// Whether `data` is an Ogg Opus stream of `channels` channels and exactly `frames` frames.
pub fn matches(data: &[u8], channels: u32, frames: u32) -> bool {
    opus(data).is_some_and(|o| o.channels == channels && o.frames == Some(frames as u64))
}

/// The stream's channels and frames: the last granule position, less pre-skip and the stream's
/// starting position (RFC 7845 sections 4.3 and 4.4). `None` when it isn't a valid Ogg Opus stream,
/// or is damaged (docs/omt.md section 9): a page fails its CRC, or a logical stream's page
/// sequence numbers don't start at 0 and go up by one.
///
/// The starting position is the granule position of the first page on which an audio packet ends,
/// less the duration of the audio packets that end on it. A negative one is invalid, unless that
/// page also ends the stream, whose end trimming then explains it.
pub fn opus(data: &[u8]) -> Option<Opus> {
    // The first packet, which must be an OpusHead, as it builds up; the second, taken as OpusTags,
    // isn't read (docs/omt.md section 9).
    let mut head: Option<Vec<u8>> = None;
    let mut first: Vec<u8> = Vec::new();
    let mut serial: Option<u32> = None;
    let (mut last, mut start): (i64, Option<i64>) = (0, None);
    let mut packets = 0u64; // packets ended so far: OpusHead and OpusTags, then audio
    let mut packet: Vec<u8> = Vec::with_capacity(2); // the first two bytes of the packet being read: its TOC
    let mut sequences: Vec<(u32, u32)> = Vec::new(); // each logical stream's next sequence number
    for page in pages(data) {
        if !page.crc_ok {
            return None;
        }
        match sequences.iter_mut().find(|s| s.0 == page.serial) {
            Some(s) if s.1 == page.sequence => s.1 = s.1.wrapping_add(1),
            None if page.sequence == 0 => sequences.push((page.serial, 1)),
            _ => return None,
        }
        if serial.is_none() {
            // The first page holds exactly one packet, the OpusHead (RFC 7845 section 3): its
            // segments all 255 but the last, which ends it.
            match page.laces.split_last() {
                Some((&last, rest)) if last < 255 && rest.iter().all(|&l| l == 255) => {}
                _ => return None,
            }
        }
        if page.serial != *serial.get_or_insert(page.serial) {
            continue;
        }
        let (mut ended, mut p, mut ended_here) = (0i64, 0usize, false);
        for &lace in page.laces {
            if packets == 0 {
                first.extend_from_slice(page.body.get(p..p + lace as usize)?);
                if lace < 255 {
                    // An OpusHead of at least 19 bytes, version major 0, at least one channel.
                    if first.len() < 19 || &first[..8] != b"OpusHead" || first[8] >> 4 != 0 || first[9] == 0 {
                        return None;
                    }
                    head = Some(std::mem::take(&mut first));
                }
            }
            let take = (lace as usize).min(2 - packet.len());
            packet.extend_from_slice(&page.body[p..p + take]);
            p += lace as usize;
            if lace < 255 {
                if packets >= 2 {
                    ended += packet_duration(&packet)? as i64;
                }
                packets += 1;
                packet.clear();
                ended_here = true;
            }
        }
        if page.granule == -1 {
            if ended_here {
                return None; // a page on which a packet ends has a granule position (RFC 3533)
            }
            continue;
        }
        last = page.granule;
        if start.is_none() && packets > 2 {
            // Saturating: a granule position near -2^63 is as negative as it gets.
            let s = page.granule.saturating_sub(ended);
            if s < 0 && page.flags & 4 == 0 {
                return None;
            }
            start = Some(s.max(0));
        }
    }
    let head = head?;
    let pre_skip = u16::from_le_bytes([head[10], head[11]]) as i128;
    let frames = match start {
        None => None,
        Some(s) => {
            let f = last as i128 - pre_skip - s as i128;
            if f < 0 {
                return None;
            }
            Some(f.min(u64::MAX as i128) as u64)
        }
    };
    Some(Opus { channels: head[9] as u32, frames })
}

/// An Opus packet's samples at 48 kHz, from its TOC byte (RFC 6716 section 3.1); `None` if invalid.
fn packet_duration(packet: &[u8]) -> Option<u32> {
    let Some(&toc) = packet.first() else { return Some(0) }; // an empty packet: nothing decoded
    let (config, code) = (toc >> 3, toc & 3);
    let size = if config < 12 {
        [480, 960, 1920, 2880][(config & 3) as usize] // SILK: 10, 20, 40, 60 ms
    } else if config < 16 {
        [480, 960][(config & 1) as usize] // hybrid: 10, 20 ms
    } else {
        [120, 240, 480, 960][(config & 3) as usize] // CELT: 2.5, 5, 10, 20 ms
    };
    let count = match code {
        0 => 1,
        1 | 2 => 2,
        _ => match packet.get(1) {
            Some(&b) if b & 0x3F != 0 => (b & 0x3F) as u32,
            _ => return None,
        },
    };
    (size * count <= 5760).then_some(size * count) // at most 120 ms
}

struct Page<'a> {
    granule: i64,
    serial: u32,
    sequence: u32,
    /// Whether the page's CRC matches.
    crc_ok: bool,
    flags: u8,
    laces: &'a [u8],
    body: &'a [u8],
}

/// Ogg's CRC-32 of a page (RFC 3533): polynomial 0x04C11DB7, unreflected, initial value 0, no
/// final XOR, over the page with its CRC field (bytes 22 to 25) taken as zero.
pub fn page_crc(page: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut r = (i as u32) << 24;
            for _ in 0..8 {
                r = if r & 0x8000_0000 != 0 { (r << 1) ^ 0x04C1_1DB7 } else { r << 1 };
            }
            *e = r;
        }
        t
    });
    let mut crc = 0u32;
    for (i, &b) in page.iter().enumerate() {
        let b = if (22..26).contains(&i) { 0 } else { b };
        crc = (crc << 8) ^ table[((crc >> 24) as u8 ^ b) as usize];
    }
    crc
}

/// Every complete page, from the start of the data.
fn pages(data: &[u8]) -> impl Iterator<Item = Page<'_>> {
    let mut p = 0usize;
    std::iter::from_fn(move || {
        let h = data.get(p..p + 27)?;
        if &h[..4] != b"OggS" || h[4] != 0 {
            return None;
        }
        let granule = i64::from_le_bytes(h[6..14].try_into().unwrap());
        let serial = u32::from_le_bytes(h[14..18].try_into().unwrap());
        let start = p + 27 + h[26] as usize;
        let laces = data.get(p + 27..start)?;
        let end = start + laces.iter().map(|&l| l as usize).sum::<usize>();
        let body = data.get(start..end)?;
        let sequence = u32::from_le_bytes(h[18..22].try_into().unwrap());
        let crc = u32::from_le_bytes(h[22..26].try_into().unwrap());
        let crc_ok = crc == page_crc(&data[p..end]);
        p = end;
        Some(Page { granule, serial, sequence, crc_ok, flags: h[5], laces, body })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An Ogg page of one logical stream, with its sequence number and CRC.
    fn page(sequence: u32, granule: i64, flags: u8, packets: &[&[u8]]) -> Vec<u8> {
        let (mut laces, mut body) = (Vec::new(), Vec::new());
        for p in packets {
            laces.extend(std::iter::repeat_n(255u8, p.len() / 255));
            laces.push((p.len() % 255) as u8);
            body.extend_from_slice(p);
        }
        let mut out = b"OggS".to_vec();
        out.extend([0, flags]);
        out.extend(granule.to_le_bytes());
        out.extend(1u32.to_le_bytes()); // serial
        out.extend(sequence.to_le_bytes());
        out.extend([0; 4]); // the CRC, below
        out.push(laces.len() as u8);
        out.extend(laces);
        out.extend(body);
        let crc = page_crc(&out);
        out[22..26].copy_from_slice(&crc.to_le_bytes());
        out
    }

    /// A mono stream with a pre-skip of 312 and one 20 ms CELT packet on a page at `granule`.
    fn stream(granule: i64, flags: u8) -> Vec<u8> {
        let mut head = b"OpusHead".to_vec();
        head.extend([1, 1]);
        head.extend(312u16.to_le_bytes());
        head.extend(48000u32.to_le_bytes());
        head.extend([0, 0, 0]);
        let mut s = page(0, 0, 2, &[&head]);
        s.extend(page(1, 0, 0, &[b"OpusTags\0\0\0\0\0\0\0\0"]));
        s.extend(page(2, granule, flags, &[&[0xf8, 0]]));
        s
    }

    #[test]
    fn frames_from_the_granule_positions() {
        assert_eq!(opus(&stream(960, 4)), Some(Opus { channels: 1, frames: Some(648) }));
    }

    #[test]
    fn the_crc_is_oggs() {
        // "OggS", version 0, a BOS page of granule 0, serial 0, sequence 0 and no segments: the CRC
        // worked by hand from the polynomial, bit by bit, over its 27 bytes.
        let mut empty = b"OggS".to_vec();
        empty.extend([0, 2]);
        empty.extend([0; 21]);
        let bitwise = |bytes: &[u8]| {
            let mut crc = 0u32;
            for &b in bytes {
                crc ^= (b as u32) << 24;
                for _ in 0..8 {
                    crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04C1_1DB7 } else { crc << 1 };
                }
            }
            crc
        };
        assert_eq!(page_crc(&empty), bitwise(&empty));
        // The CRC of "123456789" as the unreflected CRC-32 with a zero start and no final XOR
        // (the CRC catalogue's CRC-32/MPEG-2 differs only in starting from all ones).
        assert_eq!(bitwise(b"123456789"), 0x89A1_897F);
        assert_eq!(page_crc(b"123456789"), 0x89A1_897F);
    }

    #[test]
    fn a_damaged_page_or_a_skipped_sequence_number_is_damage() {
        let good = stream(960, 4);
        assert!(opus(&good).is_some());
        let mut flipped = good.clone();
        let last = flipped.len() - 1;
        flipped[last] ^= 1;
        assert_eq!(opus(&flipped), None, "a CRC that fails");
        let head = page(0, 0, 2, &[b"OpusHead\x01\x01\x38\x01\x80\xbb\0\0\0\0\0"]);
        let tags = page(1, 0, 0, &[b"OpusTags\0\0\0\0\0\0\0\0"]);
        let mut skipped = head.clone();
        skipped.extend(&tags);
        skipped.extend(page(3, 960, 4, &[&[0xf8, 0]]));
        assert_eq!(opus(&skipped), None, "sequence number 2 skipped");
        let mut late = page(1, 0, 2, &[b"OpusHead\x01\x01\x38\x01\x80\xbb\0\0\0\0\0"]);
        late.extend(page(2, 0, 0, &[b"OpusTags\0\0\0\0\0\0\0\0"]));
        late.extend(page(3, 960, 4, &[&[0xf8, 0]]));
        assert_eq!(opus(&late), None, "a stream whose first page is numbered 1");
        let mut fine = head;
        fine.extend(&tags);
        fine.extend(page(2, 960, 4, &[&[0xf8, 0]]));
        assert_eq!(opus(&fine), Some(Opus { channels: 1, frames: Some(648) }));
    }

    #[test]
    fn the_first_packet_is_an_opushead_and_the_second_isnt_read() {
        // A stream of `head` and `tags` as its first two packets, then one 20 ms CELT packet.
        let with = |head: &[u8], tags: &[u8]| {
            let mut s = page(0, 0, 2, &[head]);
            s.extend(page(1, 0, 0, &[tags]));
            s.extend(page(2, 960, 4, &[&[0xf8, 0]]));
            opus(&s)
        };
        let head = |version: u8, channels: u8| {
            let mut h = b"OpusHead".to_vec();
            h.extend([version, channels]);
            h.extend(312u16.to_le_bytes());
            h.extend(48000u32.to_le_bytes());
            h.extend([0, 0, 0]);
            h
        };
        let tags = b"OpusTags\0\0\0\0\0\0\0\0";
        let fine = Some(Opus { channels: 1, frames: Some(648) });
        assert_eq!(with(&head(1, 1), tags), fine);
        // Version major 0 with any minor version, and more than 19 bytes, are an OpusHead.
        assert_eq!(with(&head(0x0f, 1), tags), fine);
        assert_eq!(with(&[head(1, 1), vec![7; 5]].concat(), tags), fine);
        // Damaged: under 19 bytes, version major 1, no channels, not an OpusHead.
        assert_eq!(with(&head(1, 1)[..18], tags), None);
        assert_eq!(with(&head(0x10, 1), tags), None);
        assert_eq!(with(&head(1, 0), tags), None);
        assert_eq!(with(&[b"OpusHeaD".as_slice(), &head(1, 1)[8..]].concat(), tags), None);
        // The second packet is taken as OpusTags without being read: anything goes.
        assert_eq!(with(&head(1, 1), b"not tags at all"), fine);
        assert_eq!(with(&head(1, 1), b""), fine);
        // The OpusHead is the first packet, not the first page's body: a first page laced 10 and
        // 30, a 10-byte fragment of an OpusHead and then a packet of 30, is damaged.
        let h = head(1, 1);
        let mut s = page(0, 0, 2, &[&h[..10], &[&h[10..], &[0; 21][..]].concat()]);
        s.extend(page(1, 0, 0, &[tags]));
        s.extend(page(2, 960, 4, &[&[0xf8, 0]]));
        assert_eq!(opus(&s), None);
        // An OpusHead spread over two pages, laced 255 and then the rest, is damaged: the first
        // page holds exactly one complete packet.
        let long = [h.clone(), vec![0; 300]].concat();
        // A first page whose only lace is 255: the packet goes on.
        let mut first = b"OggS".to_vec();
        first.extend([0, 2]);
        first.extend(0i64.to_le_bytes());
        first.extend(1u32.to_le_bytes());
        first.extend(0u32.to_le_bytes());
        first.extend([0; 4]);
        first.push(1);
        first.push(255);
        first.extend(&long[..255]);
        let crc = page_crc(&first);
        first[22..26].copy_from_slice(&crc.to_le_bytes());
        let mut rest = page(1, 0, 1, &[&long[255..]]);
        rest[5] = 1; // a continued packet
        let crc = page_crc(&{ let mut r = rest.clone(); r[22..26].fill(0); r });
        rest[22..26].copy_from_slice(&crc.to_le_bytes());
        let mut s2 = first;
        s2.extend(rest);
        s2.extend(page(2, 0, 0, &[tags]));
        s2.extend(page(3, 960, 4, &[&[0xf8, 0]]));
        assert_eq!(opus(&s2), None);
        // The OpusHead's channels are the record's.
        let mut s = page(0, 0, 2, &[&head(1, 2)]);
        s.extend(page(1, 0, 0, &[tags]));
        s.extend(page(2, 960, 4, &[&[0xf8, 0]]));
        assert!(matches(&s, 2, 648) && !matches(&s, 1, 648));
    }

    #[test]
    fn the_first_page_holds_the_opushead_alone() {
        // RFC 7845 section 3: the first page holds exactly one packet, the OpusHead.
        let mut head = b"OpusHead".to_vec();
        head.extend([1, 1]);
        head.extend(312u16.to_le_bytes());
        head.extend(48000u32.to_le_bytes());
        head.extend([0, 0, 0]);
        let tags: &[u8] = b"OpusTags        ";
        let rest = |from: u32| {
            let mut s = page(from, 0, 0, &[tags]);
            s.extend(page(from + 1, 960, 4, &[&[0xf8, 0]]));
            s
        };
        let good = [page(0, 0, 2, &[&head]), rest(1)].concat();
        assert_eq!(opus(&good), Some(Opus { channels: 1, frames: Some(648) }));
        // A first page of 0 segments, the OpusHead on the next.
        let empty = [page(0, 0, 2, &[]), page(1, 0, 0, &[&head]), rest(2)].concat();
        assert_eq!(opus(&empty), None);
        // Two packets on the first page: the OpusHead, then the OpusTags.
        let two = [page(0, 0, 2, &[&head, tags]), page(1, 960, 4, &[&[0xf8, 0]])].concat();
        assert_eq!(opus(&two), None);
        // A packet continuing onto the next page: a first page whose one segment is 255.
        let long = [head.clone(), vec![0; 300]].concat();
        // page() laces 255 bytes as 255 then 0; rebuild its header with the one segment of 255.
        let whole = page(0, 0, 2, &[&long[..255]]);
        let mut first = [&whole[..26], &[1, 255][..], &long[..255]].concat();
        first[22..26].fill(0);
        let crc = page_crc(&first);
        first[22..26].copy_from_slice(&crc.to_le_bytes());
        let next = page(1, 0, 1, &[&long[255..]]);
        assert_eq!(opus(&[first, next, rest(2)].concat()), None);
    }

    #[test]
    fn a_granule_position_near_the_minimum_is_invalid() {
        // Found by the fuzzer: the starting position overflowed.
        assert_eq!(opus(&stream(i64::MIN + 1, 0)), None);
        assert_eq!(opus(&stream(i64::MIN + 1, 4)), None);
    }
}
