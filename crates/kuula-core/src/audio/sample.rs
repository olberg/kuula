//! PCM samples: a bounded WAV decoder (8 or 16-bit mono PCM only), the
//! per-cart bank with its 2 MB budget, and a voice that plays a sample
//! at a 16.16 fixed-point step with nearest-sample resampling.

use std::collections::HashMap;
use std::rc::Rc;

use super::{AudioError, SAMPLE_RATE};

/// Bytes of sample data (as stored in the files) a cart may hold.
pub const SAMPLE_BUDGET: usize = 2 * 1024 * 1024;

/// Sample rates outside this range are refused.
const MIN_RATE: u32 = 4000;
const MAX_RATE: u32 = 96_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sample {
    pub data: Vec<i16>,
    pub rate: u32,
    /// Bytes charged to the budget: the size of the PCM data in the file.
    pub bytes: usize,
}

fn le_u16(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

fn le_u32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// Decode a RIFF WAVE file. Only format 1 (PCM), one channel, 8 or 16
/// bits is accepted; anything else is a `sample_error`.
pub fn decode_wav(path: &str, bytes: &[u8]) -> Result<Sample, AudioError> {
    let err = |why: &str| AudioError::Sample {
        path: path.to_string(),
        why: why.to_string(),
    };
    if bytes.len() > SAMPLE_BUDGET + 4096 {
        return Err(err("file larger than the 2 MB sample budget"));
    }
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(err("not a RIFF WAVE file"));
    }
    let mut pos = 12;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = le_u32(&bytes[pos + 4..pos + 8]) as usize;
        let body_start = pos + 8;
        let body_end = body_start
            .checked_add(len)
            .ok_or_else(|| err("chunk overflow"))?;
        if body_end > bytes.len() {
            return Err(err("chunk runs past the end of the file"));
        }
        let body = &bytes[body_start..body_end];
        match id {
            b"fmt " => {
                if body.len() < 16 {
                    return Err(err("fmt chunk too short"));
                }
                fmt = Some((
                    le_u16(&body[0..2]),
                    le_u16(&body[2..4]),
                    le_u32(&body[4..8]),
                    le_u16(&body[14..16]),
                ));
            }
            b"data" => {
                if data.is_some() {
                    return Err(err("more than one data chunk"));
                }
                data = Some(body);
            }
            _ => {}
        }
        // Chunks are word aligned.
        pos = body_end + (len & 1);
    }
    let (format, channels, rate, bits) = fmt.ok_or_else(|| err("no fmt chunk"))?;
    let data = data.ok_or_else(|| err("no data chunk"))?;
    if format != 1 {
        return Err(err("only PCM (format 1) is supported"));
    }
    if channels != 1 {
        return Err(err("only mono samples are supported"));
    }
    if !(MIN_RATE..=MAX_RATE).contains(&rate) {
        return Err(err("sample rate must be 4000 to 96000 Hz"));
    }
    if data.len() > SAMPLE_BUDGET {
        return Err(err("sample data larger than the 2 MB budget"));
    }
    let samples = match bits {
        8 => data.iter().map(|&b| ((b as i16) - 128) << 8).collect(),
        16 => {
            if data.len() % 2 != 0 {
                return Err(err("odd number of bytes in 16-bit data"));
            }
            data.as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes(*b))
                .collect()
        }
        _ => return Err(err("only 8 or 16-bit samples are supported")),
    };
    if data.is_empty() {
        return Err(err("empty sample"));
    }
    Ok(Sample {
        data: samples,
        rate,
        bytes: data.len(),
    })
}

/// The cart's decoded samples by path, within [`SAMPLE_BUDGET`].
#[derive(Default)]
pub struct SampleBank {
    samples: HashMap<String, Rc<Sample>>,
    used: usize,
}

impl SampleBank {
    pub fn get(&self, path: &str) -> Option<Rc<Sample>> {
        self.samples.get(path).cloned()
    }

    pub fn used(&self) -> usize {
        self.used
    }

    /// Decode and keep a sample, or fail if the budget would overflow.
    pub fn insert(&mut self, path: &str, bytes: &[u8]) -> Result<Rc<Sample>, AudioError> {
        if let Some(s) = self.samples.get(path) {
            return Ok(s.clone());
        }
        let sample = decode_wav(path, bytes)?;
        let total = self.used + sample.bytes;
        if total > SAMPLE_BUDGET {
            return Err(AudioError::Sample {
                path: path.to_string(),
                why: format!(
                    "sample budget exceeded: {} + {} bytes of {SAMPLE_BUDGET}",
                    self.used, sample.bytes
                ),
            });
        }
        self.used = total;
        let rc = Rc::new(sample);
        self.samples.insert(path.to_string(), rc.clone());
        Ok(rc)
    }
}

/// A sample playing once at a fixed step.
#[derive(Debug, Clone)]
pub struct SampleVoice {
    sample: Rc<Sample>,
    /// Position in 16.16.
    pos: u64,
    step: u64,
}

impl SampleVoice {
    /// `pitch` is 16.16; `1 << 16` plays at the file's own rate.
    pub fn new(sample: Rc<Sample>, pitch: u32) -> SampleVoice {
        let native = (sample.rate as u64) << 16;
        let step = (native * pitch as u64 / SAMPLE_RATE as u64) >> 16;
        SampleVoice {
            sample,
            pos: 0,
            step: step.max(1),
        }
    }

    pub fn is_off(&self) -> bool {
        (self.pos >> 16) as usize >= self.sample.data.len()
    }

    pub fn next_sample(&mut self) -> Option<i32> {
        let i = (self.pos >> 16) as usize;
        let s = *self.sample.data.get(i)?;
        self.pos += self.step;
        Some(s as i32)
    }
}

/// Build a WAV file in memory, for tests and the asset tools.
pub fn encode_wav(rate: u32, bits: u16, channels: u16, data: &[u8]) -> Vec<u8> {
    let block = channels * bits / 8;
    let mut out = Vec::with_capacity(44 + data.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * block as u32).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_8_and_16_bit_mono() {
        let s = decode_wav("samples/a.wav", &encode_wav(22050, 8, 1, &[0, 128, 255])).unwrap();
        assert_eq!(s.data, [-32768, 0, 32512]);
        assert_eq!((s.rate, s.bytes), (22050, 3));
        let s = decode_wav(
            "samples/b.wav",
            &encode_wav(44100, 16, 1, &[0x00, 0x80, 0xff, 0x7f]),
        )
        .unwrap();
        assert_eq!(s.data, [-32768, 32767]);
    }

    #[test]
    fn rejects_stereo_24_bit_oversize_and_garbage() {
        let why = |bytes: &[u8]| match decode_wav("samples/x.wav", bytes).unwrap_err() {
            AudioError::Sample { why, .. } => why,
            e => panic!("{e}"),
        };
        assert!(why(&encode_wav(44100, 16, 2, &[0; 8])).contains("mono"));
        assert!(why(&encode_wav(44100, 24, 1, &[0; 6])).contains("8 or 16"));
        assert!(why(b"RIFF\0\0\0\0WAVE").contains("no fmt"));
        assert!(why(b"OggS").contains("RIFF"));
        assert!(why(&encode_wav(1000, 8, 1, &[0; 4])).contains("rate"));
        let big = vec![0u8; SAMPLE_BUDGET + 1];
        assert!(why(&encode_wav(44100, 8, 1, &big)).contains("budget"));
        let mut truncated = encode_wav(44100, 8, 1, &[0; 100]);
        truncated.truncate(80);
        assert!(why(&truncated).contains("past the end"));
    }

    #[test]
    fn the_bank_enforces_the_total_budget_and_caches() {
        let mut bank = SampleBank::default();
        let half = vec![0u8; SAMPLE_BUDGET / 2];
        let wav = encode_wav(44100, 8, 1, &half);
        let a = bank.insert("samples/a.wav", &wav).unwrap();
        let again = bank.insert("samples/a.wav", &wav).unwrap();
        assert!(Rc::ptr_eq(&a, &again));
        bank.insert("samples/b.wav", &wav).unwrap();
        assert_eq!(bank.used(), SAMPLE_BUDGET);
        let e = bank
            .insert("samples/c.wav", &encode_wav(44100, 8, 1, &[0]))
            .unwrap_err();
        assert_eq!(e.code(), "sample_error");
        assert!(e.to_string().contains("budget exceeded"), "{e}");
    }

    #[test]
    fn the_voice_steps_at_the_pitch() {
        let s = Rc::new(Sample {
            data: vec![10, 20, 30, 40],
            rate: 44100,
            bytes: 8,
        });
        let mut v = SampleVoice::new(s.clone(), 1 << 16);
        assert_eq!(v.next_sample(), Some(10));
        assert_eq!(v.next_sample(), Some(20));
        let mut v = SampleVoice::new(s.clone(), 2 << 16);
        assert_eq!(
            (v.next_sample(), v.next_sample(), v.next_sample()),
            (Some(10), Some(30), None)
        );
        assert!(v.is_off());
        // A 22 050 Hz sample plays every source sample twice.
        let s = Rc::new(Sample {
            data: vec![1, 2],
            rate: 22050,
            bytes: 4,
        });
        let mut v = SampleVoice::new(s, 1 << 16);
        let got: Vec<_> = std::iter::from_fn(|| v.next_sample()).collect();
        assert_eq!(got, [1, 1, 2, 2]);
    }
}
