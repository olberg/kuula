//! Synth voices: pulse, triangle, saw, sine and noise through an ADSR
//! envelope, with a pitch slide and an arpeggio. Everything is integer:
//! a 32-bit phase accumulator per voice, a 24-bit envelope level, and
//! the note tables in `tables.rs`. No `f32` or `f64` anywhere here, so
//! the PCM is the same on every machine.

use super::tables::{OCTAVE_8_INC, SINE};
use super::SAMPLES_PER_FRAME;

/// Seed every noise voice starts from; a 15-bit Fibonacci LFSR.
pub const NOISE_SEED: u16 = 0x2a7f;

/// Highest note: B-8. `C-0` is 0, so a note is `octave * 12 + semitone`.
pub const MAX_NOTE: u8 = 107;

/// The arpeggio changes step three times per frame.
const ARP_SAMPLES: u32 = SAMPLES_PER_FRAME as u32 / 3;

/// Envelope level resolution: 24 bits, so a 255-frame attack still steps.
const ENV_ONE: u32 = 1 << 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wave {
    Pulse,
    Triangle,
    Saw,
    Sine,
    Noise,
}

impl Wave {
    pub fn parse(s: &str) -> Option<Wave> {
        Some(match s {
            "pulse" | "square" => Wave::Pulse,
            "tri" | "triangle" => Wave::Triangle,
            "saw" => Wave::Saw,
            "sine" => Wave::Sine,
            "noise" => Wave::Noise,
            _ => return None,
        })
    }
}

/// An instrument as a tracker file defines it. Times are in frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Patch {
    pub wave: Wave,
    /// Pulse duty in 1/256ths of the period; 128 is a square.
    pub duty: u8,
    pub attack: u8,
    pub decay: u8,
    /// Sustain level, 0..=255 of the peak.
    pub sustain: u8,
    pub release: u8,
    /// Pitch slide in sixteenths of a semitone per frame.
    pub slide: i16,
    /// Arpeggio offsets in semitones, cycled three times per frame; all
    /// zero means none.
    pub arp: [i8; 3],
}

impl Default for Patch {
    fn default() -> Patch {
        Patch {
            wave: Wave::Pulse,
            duty: 128,
            attack: 0,
            decay: 0,
            sustain: 255,
            release: 0,
            slide: 0,
            arp: [0; 3],
        }
    }
}

/// Phase increment per sample for a pitch in sixteenths of a semitone
/// above C-0; clamps to the supported range.
pub fn increment(note_x16: i32) -> u32 {
    let n = note_x16.clamp(0, MAX_NOTE as i32 * 16 + 15) as u32;
    let octave = n / 192;
    OCTAVE_8_INC[(n % 192) as usize] >> (8 - octave)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Attack,
    Decay,
    Sustain,
    Release,
    Off,
}

/// One sounding synth note.
#[derive(Debug, Clone)]
pub struct SynthVoice {
    patch: Patch,
    /// The pitch in sixteenths of a semitone, moved by the slide.
    slid_x16: i32,
    phase: u32,
    inc: u32,
    lfsr: u16,
    noise_bit: i32,
    noise_clock: u32,
    arp_index: usize,
    arp_countdown: u32,
    stage: Stage,
    level: u32,
    attack_step: u32,
    decay_step: u32,
    release_step: u32,
    sustain_level: u32,
}

/// Per-sample envelope step that crosses `span` of level in `frames`.
fn step_for(frames: u8, span: u32) -> u32 {
    if frames == 0 {
        return ENV_ONE;
    }
    (span / (frames as u32 * SAMPLES_PER_FRAME as u32)).max(1)
}

impl SynthVoice {
    pub fn new(patch: Patch, note: u8) -> SynthVoice {
        let base_x16 = note.min(MAX_NOTE) as i32 * 16;
        let sustain_level = ((patch.sustain as u64 * ENV_ONE as u64) / 255) as u32;
        let mut v = SynthVoice {
            patch,
            slid_x16: base_x16,
            phase: 0,
            inc: 0,
            lfsr: NOISE_SEED,
            noise_bit: 1,
            noise_clock: 0,
            arp_index: 0,
            arp_countdown: ARP_SAMPLES,
            stage: Stage::Attack,
            level: 0,
            attack_step: step_for(patch.attack, ENV_ONE),
            decay_step: step_for(patch.decay, ENV_ONE - sustain_level),
            release_step: step_for(patch.release, ENV_ONE),
            sustain_level,
        };
        v.retune();
        v
    }

    /// Enter the release stage; the voice is done when the level hits 0.
    pub fn release(&mut self) {
        if self.stage != Stage::Off {
            self.stage = Stage::Release;
        }
    }

    pub fn is_off(&self) -> bool {
        self.stage == Stage::Off
    }

    /// Once per frame: the slide moves the pitch.
    pub fn tick_frame(&mut self) {
        if self.patch.slide != 0 {
            self.slid_x16 =
                (self.slid_x16 + self.patch.slide as i32).clamp(0, MAX_NOTE as i32 * 16 + 15);
            self.retune();
        }
    }

    fn retune(&mut self) {
        let arp = if self.patch.arp == [0; 3] {
            0
        } else {
            self.patch.arp[self.arp_index] as i32 * 16
        };
        self.inc = increment(self.slid_x16 + arp);
    }

    fn envelope(&mut self) -> u32 {
        match self.stage {
            Stage::Attack => {
                self.level = self.level.saturating_add(self.attack_step);
                if self.level >= ENV_ONE {
                    self.level = ENV_ONE;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.level = self.level.saturating_sub(self.decay_step);
                if self.level <= self.sustain_level {
                    self.level = self.sustain_level;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => {}
            Stage::Release => {
                self.level = self.level.saturating_sub(self.release_step);
                if self.level == 0 {
                    self.stage = Stage::Off;
                }
            }
            Stage::Off => {}
        }
        self.level
    }

    /// The raw waveform at the current phase, -32767..=32767.
    fn wave(&mut self) -> i32 {
        match self.patch.wave {
            Wave::Pulse => {
                if (self.phase >> 24) < self.patch.duty as u32 {
                    32767
                } else {
                    -32767
                }
            }
            Wave::Saw => (self.phase >> 16) as i32 - 32768,
            Wave::Triangle => {
                let p = (self.phase >> 16) as i32;
                if p < 32768 {
                    p * 2 - 32767
                } else {
                    32767 - (p - 32768) * 2
                }
            }
            Wave::Sine => SINE[(self.phase >> 24) as usize] as i32,
            Wave::Noise => {
                // Clock the register whenever the top five phase bits
                // move, so the noise pitch follows the note.
                let clock = self.phase >> 27;
                if clock != self.noise_clock {
                    self.noise_clock = clock;
                    let bit = (self.lfsr ^ (self.lfsr >> 1)) & 1;
                    self.lfsr = (self.lfsr >> 1) | (bit << 14);
                    self.noise_bit = if self.lfsr & 1 == 1 { 1 } else { -1 };
                }
                self.noise_bit * 32767
            }
        }
    }

    /// One sample at full volume, or `None` once the voice is off.
    pub fn next_sample(&mut self) -> Option<i32> {
        if self.stage == Stage::Off {
            return None;
        }
        let env = self.envelope();
        if self.stage == Stage::Off {
            return None;
        }
        let w = self.wave();
        self.phase = self.phase.wrapping_add(self.inc);
        if self.patch.arp != [0; 3] {
            self.arp_countdown -= 1;
            if self.arp_countdown == 0 {
                self.arp_countdown = ARP_SAMPLES;
                self.arp_index = (self.arp_index + 1) % 3;
                self.retune();
            }
        }
        // env is 24-bit; scale to 16 bits before the multiply.
        Some((w * (env >> 8) as i32) >> 16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_increments_follow_the_octaves() {
        // A-4 is 440 Hz: 440 * 2^32 / 44100.
        assert_eq!(increment(57 * 16), 42_852_281);
        assert_eq!(increment(45 * 16), 42_852_281 >> 1);
        assert_eq!(increment(0), OCTAVE_8_INC[0] >> 8);
        assert_eq!(increment(-50), increment(0));
        assert_eq!(increment(10_000), increment(MAX_NOTE as i32 * 16 + 15));
    }

    #[test]
    fn a_square_flips_at_half_its_period() {
        let mut v = SynthVoice::new(Patch::default(), 57);
        let first: Vec<i32> = (0..3).map(|_| v.next_sample().unwrap()).collect();
        assert_eq!(first, [32767, 32767, 32767]);
        let mut n = 3;
        while v.next_sample().unwrap() > 0 {
            n += 1;
        }
        // 44100 / 440 / 2 is 50.1 samples, so 51 samples are high.
        assert_eq!(n, 51);
    }

    #[test]
    fn the_envelope_attacks_decays_sustains_and_releases() {
        let p = Patch {
            attack: 1,
            decay: 1,
            sustain: 128,
            release: 1,
            wave: Wave::Saw,
            ..Patch::default()
        };
        let mut v = SynthVoice::new(p, 48);
        let mut levels = Vec::new();
        for _ in 0..(3 * SAMPLES_PER_FRAME) {
            v.next_sample().unwrap();
            levels.push(v.level);
        }
        // Integer steps land a sample or two late, never early.
        let sustain = 128 * ENV_ONE as u64 / 255;
        assert!(levels[0] < levels[100]);
        assert!(levels[SAMPLES_PER_FRAME - 2] < ENV_ONE);
        assert_eq!(levels[SAMPLES_PER_FRAME], ENV_ONE);
        assert_eq!(levels[2 * SAMPLES_PER_FRAME + 4] as u64, sustain);
        assert_eq!(levels[3 * SAMPLES_PER_FRAME - 1] as u64, sustain);
        v.release();
        let mut n = 0;
        while v.next_sample().is_some() {
            n += 1;
        }
        assert!(v.is_off());
        assert!(n <= SAMPLES_PER_FRAME, "{n}");
    }

    #[test]
    fn noise_is_deterministic_and_the_slide_moves_the_pitch() {
        let p = Patch {
            wave: Wave::Noise,
            ..Patch::default()
        };
        let mut a = SynthVoice::new(p, 60);
        let mut b = SynthVoice::new(p, 60);
        let sa: Vec<i32> = (0..500).map(|_| a.next_sample().unwrap()).collect();
        let sb: Vec<i32> = (0..500).map(|_| b.next_sample().unwrap()).collect();
        assert_eq!(sa, sb);
        assert!(sa.iter().any(|&s| s > 0) && sa.iter().any(|&s| s < 0));

        let mut v = SynthVoice::new(
            Patch {
                slide: 16,
                ..Patch::default()
            },
            48,
        );
        let before = v.inc;
        v.tick_frame();
        assert_eq!(v.inc, increment(49 * 16));
        assert!(v.inc > before);
        let mut arp = SynthVoice::new(
            Patch {
                arp: [0, 4, 7],
                ..Patch::default()
            },
            48,
        );
        for _ in 0..ARP_SAMPLES {
            arp.next_sample();
        }
        assert_eq!(arp.inc, increment(52 * 16));
    }
}
