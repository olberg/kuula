//! The console's 44.1 kHz stereo PCM at the rate the device plays.
//!
//! A phone's own rate is 48 kHz, and only a stream at that rate gets the
//! device's low-latency path: at 44.1 kHz the sound was written an eighth
//! of a second ahead of the speaker and the picture led the music. So the
//! stream is opened at the device's rate and the console's frames are
//! stretched to it here, by linear interpolation with a fixed-point phase:
//! one division when it is made, additions and a multiplication a sample
//! after that.
//!
//! This is the host's own business. What the console renders, and what a
//! conformance hash covers, is the 44.1 kHz PCM before it.

/// One whole source frame in the phase's fixed point.
const ONE: u32 = 1 << 16;

/// The most source frames an output frame advances by; a steeper pair of
/// rates is held to it.
pub const MAX_STEP: usize = 8;

/// Stereo `i16` frames from one rate to another.
pub struct Resampler {
    /// Source frames an output frame advances by, in 16.16.
    step: u32,
    /// How far the next output frame is from `prev` towards `next`,
    /// 0 to `ONE - 1`.
    phase: u32,
    prev: [i16; 2],
    next: [i16; 2],
}

impl Resampler {
    /// From `from` Hz to `to` Hz; both at least 1. The step is held to
    /// `MAX_STEP` source frames an output frame.
    pub fn new(from: u32, to: u32) -> Resampler {
        let step = ((from.max(1) as u64) << 16) / to.max(1) as u64;
        Resampler {
            step: step.clamp(1, MAX_STEP as u64 * ONE as u64) as u32,
            phase: 0,
            prev: [0; 2],
            next: [0; 2],
        }
    }

    /// Whether the two rates are the same, and `run` copies.
    pub fn is_identity(&self) -> bool {
        self.step == ONE
    }

    /// How many source frames the next `frames` output frames take.
    pub fn needs(&self, frames: usize) -> usize {
        ((self.phase as u64 + frames as u64 * self.step as u64) >> 16) as usize
    }

    /// Fill `out` (left and right for each frame) from `source`, which
    /// holds the `needs(out.len() / 2)` frames this call takes. A source
    /// that is too short is continued with its last frame.
    pub fn run(&mut self, source: &[i16], out: &mut [i16]) {
        let mut source = source.as_chunks::<2>().0.iter();
        for frame in out.as_chunks_mut::<2>().0 {
            let phase = self.phase as i64;
            for (c, sample) in frame.iter_mut().enumerate() {
                let (a, b) = (self.prev[c] as i64, self.next[c] as i64);
                *sample = (a + (((b - a) * phase) >> 16)) as i16;
            }
            self.phase += self.step;
            while self.phase >= ONE {
                self.phase -= ONE;
                self.prev = self.next;
                if let Some(frame) = source.next() {
                    self.next = *frame;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `frames` output frames in blocks of `block`, giving each block
    /// exactly the source frames it asks for from `source`.
    fn resample(r: &mut Resampler, source: &[i16], frames: usize, block: usize) -> Vec<i16> {
        let mut out = vec![0i16; frames * 2];
        let mut taken = 0;
        for chunk in out.chunks_mut(block * 2) {
            let need = r.needs(chunk.len() / 2);
            let end = (taken + need).min(source.len() / 2);
            r.run(&source[taken * 2..end * 2], chunk);
            taken += need;
        }
        out
    }

    #[test]
    fn the_same_rate_is_a_copy_delayed_by_the_two_frames_it_holds() {
        let source: Vec<i16> = (0..200).map(|i| i * 7 - 300).collect();
        let mut r = Resampler::new(44_100, 44_100);
        assert!(r.is_identity());
        assert_eq!(r.needs(100), 100);
        let out = resample(&mut r, &source, 100, 100);
        // It starts from silence: two frames of it, then the source.
        assert_eq!(out[..4], [0, 0, 0, 0]);
        assert_eq!(out[4..], source[..196]);
    }

    #[test]
    fn a_block_takes_the_source_frames_its_length_is_worth() {
        let r = Resampler::new(44_100, 48_000);
        assert!(!r.is_identity());
        // 480 frames at 48 kHz are 441 at 44.1 kHz, give or take the one
        // the phase is in the middle of.
        let need = r.needs(480);
        assert!((440..=441).contains(&need), "{need}");
        // Over a second it comes out exact to a frame.
        let mut r = Resampler::new(44_100, 48_000);
        let source = vec![0i16; 44_100 * 2 + 16];
        let mut taken = 0;
        for _ in 0..100 {
            let need = r.needs(480);
            let mut out = [0i16; 960];
            r.run(&source[taken * 2..(taken + need) * 2], &mut out);
            taken += need;
        }
        assert!((44_099..=44_100).contains(&taken), "{taken}");
    }

    #[test]
    fn the_blocks_it_is_asked_in_do_not_change_what_comes_out() {
        let source: Vec<i16> = (0..2000)
            .map(|i| (((i * 37) % 2001) - 1000) as i16 * 30)
            .collect();
        let whole = resample(&mut Resampler::new(44_100, 48_000), &source, 900, 900);
        for block in [1, 7, 96, 441] {
            let parts = resample(&mut Resampler::new(44_100, 48_000), &source, 900, block);
            assert_eq!(parts, whole, "in blocks of {block}");
        }
    }

    #[test]
    fn a_steady_level_stays_and_a_ramp_stays_a_ramp() {
        // A constant comes out as the constant once the two frames of
        // silence it starts from are behind.
        let source = [1234i16, -4321].repeat(600);
        let out = resample(&mut Resampler::new(44_100, 48_000), &source, 500, 96);
        for frame in out[8..].chunks(2) {
            assert_eq!(frame, [1234, -4321]);
        }
        // A ramp stays monotonic and inside its ends, on both channels.
        let source: Vec<i16> = (0..1000).flat_map(|i| [i * 20, 30_000 - i * 20]).collect();
        let out = resample(&mut Resampler::new(44_100, 48_000), &source, 900, 128);
        for pair in out[8..].chunks(2).collect::<Vec<_>>().windows(2) {
            assert!(pair[1][0] >= pair[0][0], "{pair:?}");
            assert!(pair[1][1] <= pair[0][1], "{pair:?}");
        }
        assert!(out.iter().all(|&s| (0..=30_000).contains(&s)));
    }

    #[test]
    fn a_source_that_runs_out_is_held_and_extremes_do_not_wrap() {
        let mut r = Resampler::new(44_100, 48_000);
        let source = [i16::MAX, i16::MIN, i16::MIN, i16::MAX];
        let mut out = [0i16; 64];
        r.run(&source, &mut out);
        // The last frame is held to the end.
        assert_eq!(out[62..], [i16::MIN, i16::MAX]);
        // Rates out of range are held to the steepest step.
        assert_eq!(Resampler::new(1_000_000, 1).needs(1), MAX_STEP);
        assert_eq!(Resampler::new(0, 0).needs(10), 10);
    }
}
