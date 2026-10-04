//! One sounding note: its envelopes, its filter, its oscillators or its place in a sample, and the
//! frame it gives (docs/omt.md sections 8, 9 and 10).

use std::sync::Arc;

use super::{
    env_frames, env_step, floor_div, increment, sample_step, EventKind, ENV_ONE, FADE_ONE, NOISE_SEED, SOFT_NOISE_PITCH,
};
use crate::song::{
    sequence_waveform, Action, Engine, Instrument, LoopMode, PointEnvelope, Resampling, Sample, Sequence, Song,
    VibratoWave, Waveform, MAX_PITCH,
};
use crate::tables::SINE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    Attack,
    Decay,
    Sustain,
    Release,
}

/// A point envelope's place: its position, and the value it gave this tick.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct EnvState {
    pub(super) position: u32,
    pub(super) value: i32,
}

/// The value of a point envelope at position `p` (section 9).
pub(super) fn envelope_value(env: &PointEnvelope, p: u32) -> i32 {
    let points = &env.points;
    let last = points.len() - 1;
    if p >= points[last].0 {
        return points[last].1;
    }
    let i = points.iter().rposition(|&(t, _)| t <= p).unwrap_or(0);
    let (t0, v0) = points[i];
    let (t1, v1) = points[i + 1];
    v0 + floor_div((v1 - v0) as i64 * (p - t0) as i64, (t1 - t0) as i64) as i32
}

/// The position after `p`: back to the sustain loop's start while held, else to the loop's.
pub(super) fn envelope_advance(env: &PointEnvelope, p: u32, released: bool) -> u32 {
    let next = p + 1;
    match (env.sustain, env.looping) {
        (Some((a, b)), _) if !released && next > env.points[b].0 => env.points[a].0,
        (_, Some((a, b))) if next > env.points[b].0 => env.points[a].0,
        _ => next,
    }
}

/// Impulse Tracker's resonant low-pass (section 9), in floating point: the one faithful part.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Filter {
    a0: f64,
    b0: f64,
    b1: f64,
    y: [[f64; 2]; 2],
}

impl Filter {
    pub(super) fn coefficients(&mut self, cutoff: u8, resonance: u8, rate: u32) {
        let fs = rate as f64;
        let fc = (110.0 * 2f64.powf(cutoff as f64 / 24.0 + 0.25)).min(fs / 2.0);
        let damping = 10f64.powf(-(resonance as f64) * 24.0 / (128.0 * 20.0));
        let r = fs / (2.0 * std::f64::consts::PI * fc);
        let d = damping * (r + 1.0) - 1.0;
        let e = r * r;
        self.a0 = 1.0 / (1.0 + d + e);
        self.b0 = (d + e + e) / (1.0 + d + e);
        self.b1 = -e / (1.0 + d + e);
    }

    /// One frame. The history is clipped to twice the 16-bit range when it is used, as OpenMPT
    /// clips it: at high resonance near half the output rate the filter is unstable, and would
    /// otherwise grow without bound.
    pub(super) fn run(&mut self, side: usize, x: i64) -> i64 {
        let [y1, y2] = self.y[side].map(|y| y.clamp(-65536.0, 65535.0));
        let y = self.a0 * x as f64 + self.b0 * y1 + self.b1 * y2;
        self.y[side] = [y, y1];
        y.round() as i64
    }
}

/// One sounding note.
#[derive(Debug, Clone)]
pub struct Voice {
    pub(super) ins: Arc<Instrument>,
    pub(super) sample: Option<Arc<Sample>>,
    pub(super) sample_index: Option<usize>,
    /// The note as written, which chose the sample.
    pub(super) note: i32,
    /// The base pitch it started at, for the duplicate check.
    pub(super) start_pitch: i32,
    /// The base pitch, moved by `slide`, `fslide` and `port`.
    pub(super) base: i32,
    pub(super) target: i32,
    /// The note volume, 0..=64, and this tick's after `trem` and `tremor`.
    pub(super) vol: i32,
    pub(super) tick_vol: Option<i32>,
    /// Whether `tremor` silences this tick, whatever `trem` does.
    pub(super) silent: bool,
    pub(super) cur_vol: i32,
    /// The pitch this tick sounds at: the base pitch and the offsets, clamped.
    pub(super) cur_pitch: i32,
    pub(super) pan: i32,
    /// This tick's pan offset from `panbr`, and the pan the tick sounds at.
    pub(super) pan_offset: i32,
    pub(super) cur_pan: i32,
    pub(super) stage: Stage,
    pub(super) level: u32,
    pub(super) attack_step: u32,
    pub(super) decay_step: u32,
    pub(super) release_step: u32,
    pub(super) sustain_level: u32,
    pub(super) released: bool,
    pub(super) nna: Action,
    /// The key-map range's transpose, of the range the voice plays (0 without one): part of a
    /// retarget's target (section 6).
    pub(super) zone_transpose: i32,
    /// The fade level, and whether the voice fades; it falls after each tick once it does.
    pub(super) fade: u32,
    pub(super) fading: bool,
    /// The volume envelope has ended at 0: the voice ends after this tick (section 9).
    pub(super) gone: bool,
    pub(super) envelopes: [EnvState; 3],
    /// Ticks since the voice started, for the automatic vibrato.
    pub(super) age: u32,
    pub(super) vib: u32,
    pub(super) trem: u32,
    pub(super) tremor: u32,
    pub(super) panbr: u32,
    /// This tick's pitch offset from the cell's curves.
    pub(super) curve_offset: i32,
    pub(super) seq_pos: [usize; 5],
    /// The `volume` sequence's gain, 0..=64.
    pub(super) seq_gain: i32,
    pub(super) duty: i32,
    pub(super) waveform: Waveform,
    pub(super) phase: u32,
    pub(super) inc: u32,
    /// The second oscillator's phase and increment, with `second`.
    pub(super) phase2: u32,
    pub(super) inc2: u32,
    pub(super) lfsr: u16,
    pub(super) noise_bit: i32,
    pub(super) noise_clock: u32,
    /// Soft noise's generator state (31 bits), its filter value, and the increment of MIDI 99 at
    /// the output rate.
    pub(super) soft_x: u32,
    pub(super) soft_y: i64,
    /// Soft noise's note, for its gain: the written note with the instrument's transpose, which a
    /// retarget's note replaces.
    pub(super) noise_note: i32,
    /// The `arpeggio` sequence's step this tick, in pitch units: part of soft noise's note.
    pub(super) seq_arp: i32,
    pub(super) soft_r: u64,
    /// The vibrato waveform and phase `vibw` set.
    pub(super) vib_wave: u8,
    pub(super) vib_phase: i32,
    /// Ticks each sequence has spent on its step.
    pub(super) seq_count: [u32; 5],
    /// Sampled voices: the position in 32.32 frames, and the step.
    pub(super) q: u64,
    pub(super) step: u64,
    pub(super) cutoff: u8,
    pub(super) resonance: u8,
    pub(super) filter: Option<Filter>,
}

impl Voice {
    /// A voice of `ins` for the note `note` (a pitch as written) at base pitch `base` (the
    /// instrument's transpose included), or `None` if it plays nothing.
    pub(super) fn new(song: &Song, ins: &Arc<Instrument>, note: i32, base: i32, vol: i32, pan: i32, offset: u32) -> Option<Voice> {
        let mut base = base;
        let mut zone_transpose = 0;
        let (sample, sample_index, duty, waveform, cutoff, resonance) = match &ins.engine {
            Engine::Wave(w) => (None, None, w.duty, w.waveform, 127, 0),
            Engine::Sampler(s) => {
                let (index, transpose) = s.sample_for(note.div_euclid(256))?;
                let sample = song.samples.get(index)?;
                sample.data.as_ref()?;
                base += transpose;
                zone_transpose = transpose;
                let (c, r) = s.filter.unwrap_or((127, 0));
                (Some(sample.clone()), Some(index), 128, Waveform::Pulse, c, r)
            }
            Engine::Unknown(_) => return None,
        };
        let rate = song.rate;
        let env = ins.envelope;
        let sustain_level = env.sustain << 18;
        let mut voice = Voice {
            ins: ins.clone(),
            sample,
            sample_index,
            note,
            start_pitch: base,
            base,
            target: base,
            vol,
            tick_vol: None,
            silent: false,
            cur_vol: vol,
            cur_pitch: base,
            pan: ins.pan.unwrap_or(pan),
            pan_offset: 0,
            cur_pan: 0,
            stage: Stage::Attack,
            level: 0,
            attack_step: env_step(env_frames(env.attack, rate), ENV_ONE),
            decay_step: env_step(env_frames(env.decay, rate), ENV_ONE - sustain_level.min(ENV_ONE)),
            release_step: env_step(env_frames(env.release, rate), ENV_ONE),
            sustain_level,
            released: false,
            nna: match &ins.engine {
                Engine::Sampler(s) => s.nna,
                Engine::Wave(w) => w.nna,
                _ => Action::Cut,
            },
            zone_transpose,
            fade: FADE_ONE,
            fading: false,
            gone: false,
            envelopes: [EnvState::default(); 3],
            age: 0,
            vib: 0,
            trem: 0,
            tremor: 0,
            panbr: 0,
            curve_offset: 0,
            seq_pos: [0; 5],
            seq_gain: 64,
            duty,
            waveform,
            phase: 0,
            inc: 0,
            phase2: 0,
            inc2: 0,
            lfsr: NOISE_SEED,
            noise_bit: 1,
            noise_clock: 0,
            soft_x: 0x2a7f,
            soft_y: 0,
            noise_note: note + ins.transpose,
            seq_arp: 0,
            soft_r: increment(SOFT_NOISE_PITCH, rate) as u64,
            vib_wave: 0,
            vib_phase: 0,
            seq_count: [0; 5],
            q: (offset as u64) << 32,
            step: 0,
            cutoff,
            resonance,
            filter: None,
        };
        voice.cur_pan = voice.pan;
        voice.set_filter(cutoff, resonance, rate);
        Some(voice)
    }

    pub(super) fn set_filter(&mut self, cutoff: u8, resonance: u8, rate: u32) {
        self.cutoff = cutoff;
        self.resonance = resonance;
        if cutoff == 127 && resonance == 0 {
            self.filter = None;
            return;
        }
        let mut f = self.filter.unwrap_or_default();
        f.coefficients(cutoff, resonance, rate);
        self.filter = Some(f);
    }

    pub(super) fn release(&mut self) {
        self.released = true;
        self.stage = Stage::Release;
    }

    /// A fade: the voice fades when its instrument has a `fadeout`, and is released otherwise.
    /// Returns what it did, for the trace.
    pub(super) fn fade(&mut self) -> EventKind {
        match &self.ins.engine {
            Engine::Sampler(s) if s.fadeout.is_some() => {
                self.fading = true;
                EventKind::Fade
            }
            _ => {
                self.release();
                EventKind::Release
            }
        }
    }

    /// Cut, release or fade: what a new-note, duplicate or `past` action does; `None` when the
    /// voice ends. The trace kind goes to `kind`.
    pub(super) fn act(mut self, action: Action, kind: &mut EventKind) -> Option<Voice> {
        match action {
            Action::Cut => {
                *kind = EventKind::Cut;
                None
            }
            Action::Continue => Some(self),
            Action::Release => {
                self.release();
                *kind = EventKind::Release;
                Some(self)
            }
            Action::Fade => {
                *kind = self.fade();
                Some(self)
            }
        }
    }

    pub(super) fn sampler(&self) -> Option<&crate::song::Sampler> {
        match &self.ins.engine {
            Engine::Sampler(s) => Some(s),
            _ => None,
        }
    }

    fn sequences(ins: &Instrument) -> [Option<&Sequence>; 5] {
        match &ins.engine {
            Engine::Wave(w) => {
                let s = &w.sequences;
                [s.volume.as_ref(), s.arpeggio.as_ref(), s.pitch.as_ref(), s.duty.as_ref(), s.waveform.as_ref()]
            }
            _ => [None; 5],
        }
    }

    /// After a tick's frames: the fade falls, and a voice whose fade reaches 0, or whose volume
    /// envelope ended on the tick, is gone before the next tick's events (section 9). `false` when
    /// it is.
    pub(super) fn after_tick(&mut self) -> bool {
        if self.gone {
            return false;
        }
        if self.fading {
            let fadeout = self.sampler().and_then(|s| s.fadeout).unwrap_or(FADE_ONE);
            self.fade = self.fade.saturating_sub(fadeout);
            if self.fade == 0 {
                return false;
            }
        }
        true
    }

    /// Once a tick, after the curves: the sequences, the point envelopes and the automatic
    /// vibrato, then the pitch, volume and pan this tick sounds at.
    pub(super) fn tick(&mut self, rate: u32) {
        let mut offset = self.curve_offset;
        self.curve_offset = 0;
        let ins = self.ins.clone();
        let seqs = Voice::sequences(&ins);
        let mut next = self.seq_pos;
        for (i, seq) in seqs.iter().enumerate() {
            let Some(seq) = seq else { continue };
            let pos = self.seq_pos[i].min(seq.steps.len() - 1);
            let v = seq.steps[pos];
            match i {
                0 => self.seq_gain = v,
                1 => {
                    offset += 256 * v;
                    self.seq_arp = 256 * v;
                }
                2 => offset += v,
                3 => self.duty = v,
                _ => self.waveform = sequence_waveform(v),
            }
            // A step lasts the sequence's speed in ticks.
            self.seq_count[i] += 1;
            next[i] = if self.seq_count[i] >= seq.speed {
                self.seq_count[i] = 0;
                advance(seq, pos, self.released)
            } else {
                pos
            };
        }
        self.seq_pos = next;

        let mut pan = self.pan + self.pan_offset;
        self.pan_offset = 0;
        if let Engine::Sampler(s) = &ins.engine {
            let envs = [&s.volume_envelope, &s.pan_envelope, &s.pitch_envelope];
            for (i, env) in envs.iter().enumerate() {
                let Some(env) = env else { continue };
                let state = &mut self.envelopes[i];
                state.value = envelope_value(env, state.position);
                // A volume envelope at its end at 0, which no loop takes back, ends the voice
                // after this tick.
                if i == 0 {
                    let last = env.points.len() - 1;
                    let looped = env.looping.is_some_and(|(_, b)| b == last)
                        || (!self.released && env.sustain.is_some_and(|(_, b)| b == last));
                    if state.position >= env.points[last].0 && env.points[last].1 == 0 && !looped {
                        self.gone = true;
                    }
                }
                state.position = envelope_advance(env, state.position, self.released);
            }
            if s.pan_envelope.is_some() {
                pan += self.envelopes[1].value;
            }
            if s.pitch_envelope.is_some() {
                offset += self.envelopes[2].value;
            }
            if let Some(v) = s.autovibrato {
                let phase = (self.age as u64 * v.rate as u64 % 256) as i32;
                let depth = if v.sweep == 0 { v.depth } else { (v.depth as i64 * self.age.min(v.sweep) as i64 / v.sweep as i64) as i32 };
                let w = match v.waveform {
                    VibratoWave::Sine => SINE[phase as usize] as i32,
                    VibratoWave::Square => if phase < 128 { 32767 } else { -32767 },
                    VibratoWave::Ramp => ((128 - phase) * 256).min(32767),
                };
                offset += (w * depth) >> 15;
            }
        }
        self.age = self.age.saturating_add(1);
        self.cur_pan = pan.clamp(-256, 256);
        let tick_vol = self.tick_vol.take().unwrap_or(self.vol);
        self.cur_vol = if std::mem::take(&mut self.silent) { 0 } else { tick_vol };

        let p = (self.base + offset).clamp(0, MAX_PITCH);
        self.cur_pitch = p;
        match &self.sample {
            Some(s) => self.step = sample_step(p, s.root, s.rate, rate),
            None => {
                self.inc = increment(p, rate);
                if let Engine::Wave(w) = &ins.engine
                    && let Some((t, _)) = w.second
                {
                    self.inc2 = increment((p + t).clamp(0, MAX_PITCH), rate);
                }
            }
        }
    }

    /// Steps the ADSR envelope a frame: the level, or `None` when the voice ends (section 8).
    fn envelope(&mut self) -> Option<u32> {
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
                    return None;
                }
            }
        }
        if self.stage == Stage::Sustain && self.level == 0 {
            return None;
        }
        Some(self.level)
    }

    /// A periodic waveform's value at `phase`: the first oscillator's and the second's.
    fn periodic(&self, waveform: Waveform, phase: u32) -> i32 {
        match waveform {
            Waveform::Pulse => {
                if ((phase >> 24) as i32) < self.duty { 32767 } else { -32767 }
            }
            Waveform::Saw => (phase >> 16) as i32 - 32768,
            Waveform::Triangle => {
                let t = (phase >> 16) as i32;
                if t < 32768 { 2 * t - 32767 } else { 32767 - 2 * (t - 32768) }
            }
            Waveform::Sine => SINE[(phase >> 24) as usize] as i32,
            Waveform::Table(i) => match &self.ins.engine {
                Engine::Wave(w) => match w.tables.get(i) {
                    Some(table) => {
                        let bits = table.len().trailing_zeros();
                        table[(phase >> (32 - bits)) as usize] as i32
                    }
                    None => 0,
                },
                _ => 0,
            },
            Waveform::Noise | Waveform::SoftNoise => 0,
        }
    }

    /// Steps the noise register (section 8).
    fn step_register(&mut self) {
        let bit = (self.lfsr ^ (self.lfsr >> 1)) & 1;
        self.lfsr = (self.lfsr >> 1) | (bit << 14);
    }

    fn oscillator(&mut self) -> i32 {
        let mut w = match self.waveform {
            Waveform::Noise => {
                let clock = self.phase >> 27;
                if clock != self.noise_clock {
                    self.noise_clock = clock;
                    self.step_register();
                    self.noise_bit = if self.lfsr & 1 == 1 { 1 } else { -1 };
                }
                self.noise_bit * 32767
            }
            Waveform::SoftNoise => {
                // A linear congruential generator, a step a frame: r from its top 15 bits.
                self.soft_x = ((1_103_515_245u64 * self.soft_x as u64 + 12345) & 0x7fff_ffff) as u32;
                let r = 2 * (self.soft_x >> 16) as i64 - 32767;
                let inc = self.inc as u64;
                let c = (65536 * inc / (inc + self.soft_r)) as i64;
                self.soft_y += ((r - self.soft_y) * c) >> 16;
                // The gain follows the voice's note, its written pitch with the instrument's
                // transpose, which effects don't move.
                let n = self.noise_note + self.seq_arp;
                let d = (SOFT_NOISE_PITCH - n).clamp(0, 16128) as i64;
                let g = 384 + 384 * d * d / (16128 * 16128);
                ((self.soft_y * g) >> 8).clamp(-32767, 32767) as i32
            }
            wf => self.periodic(wf, self.phase),
        };
        if let Engine::Wave(wave) = &self.ins.engine
            && let Some((_, level)) = wave.second
        {
            let w2 = self.periodic(self.waveform, self.phase2);
            w = (w + ((w2 * level) >> 6)).clamp(-32767, 32767);
            self.phase2 = self.phase2.wrapping_add(self.inc2);
        }
        self.phase = self.phase.wrapping_add(self.inc);
        w
    }

    /// The position of a sampled voice in 32.32 frames, or `None` past the end (section 9).
    fn position(q: u64, s: &Sample) -> Option<u64> {
        match s.looping {
            None => ((q >> 32) < s.frames as u64).then_some(q),
            Some(l) => {
                let start = (l.start as u64) << 32;
                let end = (l.end as u64) << 32;
                if q < end {
                    return Some(q);
                }
                let len = end - start;
                Some(match l.mode {
                    LoopMode::Forward => start + (q - start) % len,
                    LoopMode::PingPong => {
                        let m = (q - start) % (2 * len);
                        if m < len { start + m } else { start + 2 * len - 1 - m }
                    }
                })
            }
        }
    }

    fn sample_frame(&mut self, resampling: Resampling) -> Option<(i32, i32)> {
        let s = self.sample.as_ref()?;
        let data = s.data.as_ref()?;
        let pos = Voice::position(self.q, s)?;
        let frame = (pos >> 32) as usize;
        let ch = s.channels as usize;
        let at = |f: usize, c: usize| data[f * ch + c.min(ch - 1)] as i32;
        let out = match resampling {
            Resampling::Nearest => (at(frame, 0), at(frame, 1)),
            Resampling::Linear => {
                let next = match s.looping {
                    Some(l) if frame + 1 >= l.end as usize => match l.mode {
                        LoopMode::Forward => l.start as usize,
                        LoopMode::PingPong => frame.saturating_sub(1).max(l.start as usize),
                    },
                    _ if frame + 1 >= s.frames as usize => frame,
                    _ => frame + 1,
                };
                let f = ((pos >> 16) & 0xffff) as i32;
                let lerp = |c: usize| {
                    let w0 = at(frame, c);
                    w0 + (((at(next, c) - w0) as i64 * f as i64) >> 16) as i32
                };
                (lerp(0), lerp(1))
            }
        };
        self.q = self.q.wrapping_add(self.step);
        Some(out)
    }

    /// One frame, mixed as section 10 has it with the channel's volume, or `None` when the voice
    /// ends.
    pub(super) fn frame(&mut self, resampling: Resampling, channel_volume: i64) -> Option<(i64, i64)> {
        let level = self.envelope()?;
        let (wl, wr) = if self.sample.is_some() {
            self.sample_frame(resampling)?
        } else {
            let w = self.oscillator();
            (w, w)
        };
        let e = (level >> 8) as i64;
        let g = self.cur_vol as i64 * self.seq_gain as i64 * self.ins.volume as i64 * channel_volume;
        let volume_envelope = match self.sampler() {
            Some(s) if s.volume_envelope.is_some() => self.envelopes[0].value as i64,
            _ => 64,
        };
        let gain = self.sample.as_ref().map_or(64, |s| s.gain as i64);
        let fade = self.fade as i64;
        let mix = |w: i32| {
            let a = (w as i64 * e) >> 16;
            let b = (a * g) >> 24;
            let b = (b * volume_envelope) >> 6;
            let b = (b * gain) >> 6;
            (b * fade) >> 16
        };
        let (mut bl, mut br) = (mix(wl), mix(wr));
        if let Some(f) = &mut self.filter {
            bl = f.run(0, bl);
            br = f.run(1, br);
        }
        let p = self.cur_pan;
        Some(((bl * (256 - p.max(0)) as i64) >> 8, (br * (256 + p.min(0)) as i64) >> 8))
    }
}

/// A sequence's next step (section 8).
pub(super) fn advance(seq: &Sequence, pos: usize, released: bool) -> usize {
    if !released && seq.release == Some(pos) {
        return pos;
    }
    if pos + 1 < seq.steps.len() {
        return pos + 1;
    }
    match seq.loop_step {
        Some(l) if !released || seq.release.is_none_or(|r| l > r) => l,
        _ => pos,
    }
}
