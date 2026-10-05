//! RIFF WAVE, as a cue's audio file (docs/omq.md section 4): recognised by the rule the container
//! gives for a cue entry (docs/container.md, *RIFF WAVE*), and read when it is of the kind a bank's
//! plain cue plays exactly: integer PCM of 8 or 16 bits, one or two channels, 1000 to 384000 Hz.

/// What a payload is, as a cue's file.
#[derive(Debug, PartialEq, Eq)]
pub enum Read {
    /// It doesn't start with `RIFF` and have `WAVE` at offset 8.
    NotWave,
    /// A WAVE of the kind section 4 plays: its frames, interleaved, as 16-bit samples.
    Pcm { rate: u32, channels: u32, pcm: Vec<i16> },
    /// A WAVE whose `fmt ` chunk says another coding or depth, more than two channels or a rate
    /// outside 1000 to 384000: left to the player (section 4).
    Other,
    /// A WAVE of the kind that is damaged: a chunk running past the payload, no `fmt ` or `data`
    /// chunk, a block alignment that isn't the format's, a `data` chunk cut short or not a whole
    /// number of frames; or one that has no frames or more than `max_frames`.
    Damaged,
}

/// The `fmt ` chunk's fields of a WAVE: format tag, channels, rate, bits a sample, block alignment.
struct Format {
    tag: u16,
    channels: u16,
    rate: u32,
    bits: u16,
    align: u16,
}

/// The chunks of a RIFF WAVE from offset 12, as the container walks them: a four-byte id, a 32-bit
/// little-endian size and that many bytes of data, padded to an even length. Ends at the first
/// chunk that doesn't lie whole inside the payload: that one is `Err`.
fn chunks(bytes: &[u8]) -> impl Iterator<Item = Result<([u8; 4], &[u8]), ()>> {
    let mut at = 12usize;
    std::iter::from_fn(move || {
        if at + 8 > bytes.len() {
            return None;
        }
        let id: [u8; 4] = bytes[at..at + 4].try_into().unwrap();
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let data = bytes.get(at + 8..at + 8 + size);
        at += 8 + size + (size & 1);
        Some(data.map(|d| (id, d)).ok_or(()))
    })
}

/// Reads a cue's file as a WAVE. `max_frames` is the most frames it may have (2^24).
pub fn read(bytes: &[u8], max_frames: u32) -> Read {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Read::NotWave;
    }
    // The first `fmt ` chunk, then the first `data` chunk after it; the others are skipped.
    let mut format = None;
    let mut data = None;
    for chunk in chunks(bytes) {
        let Ok((id, body)) = chunk else { break };
        match (&id, &format) {
            (b"fmt ", None) => {
                if body.len() < 16 {
                    return Read::Damaged;
                }
                let u16le = |i: usize| u16::from_le_bytes([body[i], body[i + 1]]);
                format = Some(Format {
                    tag: u16le(0),
                    channels: u16le(2),
                    rate: u32::from_le_bytes([body[4], body[5], body[6], body[7]]),
                    bits: u16le(14),
                    align: u16le(12),
                });
            }
            (b"data", Some(_)) => {
                data = Some(body);
                break;
            }
            _ => {}
        }
    }
    let Some(f) = format else { return Read::Damaged };
    if f.tag != 1 || !matches!(f.bits, 8 | 16) || !(1..=2).contains(&f.channels) || !(1000..=384000).contains(&f.rate) {
        return Read::Other;
    }
    let width = f.channels * f.bits / 8;
    let Some(data) = data.filter(|d| f.align == width && d.len() % width as usize == 0) else { return Read::Damaged };
    let frames = data.len() / width as usize;
    if frames == 0 || frames > max_frames as usize {
        return Read::Damaged;
    }
    let pcm = if f.bits == 16 {
        data.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect()
    } else {
        data.iter().map(|&b| (b as i16 - 128) * 256).collect()
    };
    Read::Pcm { rate: f.rate, channels: f.channels as u32, pcm }
}

#[cfg(test)]
#[path = "wave_tests.rs"]
mod tests;
