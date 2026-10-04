//! What a cell does to its channel: its events (docs/omt.md section 6) and its curves, the named
//! effects (section 7).

use super::voice::Voice;
use super::{floor_div, freq, pitch_of, vibrato_wave, EventKind, Player, MAX_BACKGROUND};
use crate::cell::{Effect, Note};
use crate::song::{Action, Cell, Duplicate, Engine, MAX_PITCH};
use crate::tables::SINE;

impl Player {
    /// `past`: every background voice of the channel is cut, released or faded.
    pub(super) fn past(&mut self, c: usize, action: Action) {
        let voices = std::mem::take(&mut self.channels[c].background);
        let kept: Vec<Voice> = voices.into_iter().filter_map(|v| self.act(c, v, action)).collect();
        self.channels[c].background = kept;
    }

    /// A note on (section 6): the duplicate check, the foreground's new-note action, then the new
    /// voice. `nna` overrides the new voice's own. The check and the action happen too when the
    /// instrument can play nothing for the note, and then no voice starts.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn note_on(&mut self, c: usize, ins_number: u8, note: i32, vol: i32, pan: i32, offset: u32, nna: Option<Action>) {
        let song = self.song.clone();
        let ins = song.instrument(ins_number);
        // The duplicate check, on the new note's instrument: its starting pitch (the note with the
        // instrument's and the range's transpose) and its sample, whether or not it can play.
        if let Some(ins) = ins
            && let Engine::Sampler(s) = &ins.engine
            && s.dct != Duplicate::Off
        {
            let range = s.sample_for(note.div_euclid(256));
            let pitch = note + ins.transpose + range.map_or(0, |r| r.1);
            let sample = range.map(|r| r.0);
            let is_duplicate = |v: &Voice| match s.dct {
                Duplicate::Note => v.ins.number == ins.number && v.start_pitch == pitch,
                Duplicate::Sample => sample.is_some() && v.sample_index == sample,
                Duplicate::Instrument => v.ins.number == ins.number,
                Duplicate::Off => false,
            };
            // Oldest first, the foreground voice last.
            let voices = std::mem::take(&mut self.channels[c].background);
            let mut kept = Vec::new();
            for v in voices {
                if is_duplicate(&v) {
                    if let Some(v) = self.act(c, v, s.dca) {
                        kept.push(v);
                    }
                } else {
                    kept.push(v);
                }
            }
            self.channels[c].background = kept;
            if let Some(fg) = self.channels[c].voice.take() {
                self.channels[c].voice = if is_duplicate(&fg) { self.act(c, fg, s.dca) } else { Some(fg) };
            }
        }
        let voice = ins.and_then(|ins| Voice::new(&song, ins, note, note + ins.transpose, vol, pan, offset));
        // The foreground's new-note action.
        if let Some(fg) = self.channels[c].voice.take() {
            let action = fg.nna;
            if let Some(v) = self.act(c, fg, action) {
                let bg = &mut self.channels[c].background;
                bg.push(v);
                if bg.len() > MAX_BACKGROUND {
                    bg.remove(0);
                    self.event(c, EventKind::Cut, 0, 0);
                }
            }
        }
        let started = voice.as_ref().map(|v| v.base);
        self.channels[c].voice = voice.map(|mut v| {
            if let Some(a) = nna {
                v.nna = a;
            }
            v
        });
        if let Some(p) = started {
            self.event(c, EventKind::On, ins_number, p);
        }
    }

    /// A cell's note, instrument and volume events, and `past` (section 6).
    pub(super) fn cell_events(&mut self, c: usize, cell: &Cell) {
        let pan = self.song.channels[c].pan;
        let mut offset = 0;
        let mut port = false;
        let mut nna = None;
        for e in &cell.effects {
            match *e {
                Effect::Offset(f) => offset = f as u32,
                Effect::Port(_) | Effect::Glide(_) | Effect::FGlide(_) => port = true,
                Effect::Nna(x) => nna = Some([Action::Cut, Action::Continue, Action::Release, Action::Fade][x as usize]),
                Effect::Past(x) => self.past(c, [Action::Cut, Action::Release, Action::Fade][x as usize]),
                _ => {}
            }
        }
        match cell.note {
            Note::On(p) => {
                if port && self.channels[c].voice.is_some() {
                    // A retarget (section 6): the note with the voice's instrument's transpose and
                    // its key-map range's, whatever instrument the cell names; its written volume.
                    let v = self.channels[c].voice.as_mut().unwrap();
                    v.target = p + v.ins.transpose + v.zone_transpose;
                    v.noise_note = p + v.ins.transpose;
                    if let Some(vol) = cell.vol {
                        v.vol = vol as i32;
                    }
                    let (ins, target) = (v.ins.number, v.target);
                    self.event(c, EventKind::Retarget, ins, target);
                } else {
                    self.note_on(c, cell.ins, p, cell.note_vol as i32, pan, offset, nna);
                }
            }
            // A volume with `===` or `~~~` applies first.
            Note::Release => {
                self.set_volume(c, cell.vol);
                self.release(c);
            }
            Note::Fade => {
                self.set_volume(c, cell.vol);
                self.fade(c);
            }
            Note::Cut => self.cut(c),
            Note::None => self.set_volume(c, cell.vol),
        }
    }

    /// A cell's written volume on the foreground voice.
    pub(super) fn set_volume(&mut self, c: usize, vol: Option<u8>) {
        if let (Some(vol), Some(v)) = (vol, self.channels[c].voice.as_mut()) {
            v.vol = vol as i32;
        }
    }

    /// A restart of the foreground voice: its instrument, base pitch, note volume and pan, the old
    /// voice cut rather than given its new-note action; a sampled voice starts again at the cell's
    /// `offset`, or 0 (section 6).
    pub(super) fn retrigger(&mut self, c: usize, change: Option<i32>, offset: u32) {
        let Some(old) = self.channels[c].voice.take() else { return };
        let song = self.song.clone();
        let mut vol = old.vol;
        if let Some(x) = change {
            vol = match x {
                1 => vol - 1,
                2 => vol - 2,
                3 => vol - 4,
                4 => vol - 8,
                5 => vol - 16,
                6 => vol * 2 / 3,
                7 => vol / 2,
                9 => vol + 1,
                10 => vol + 2,
                11 => vol + 4,
                12 => vol + 8,
                13 => vol + 16,
                14 => vol * 3 / 2,
                15 => vol * 2,
                _ => vol,
            }
            .clamp(0, 64);
        }
        // The sample was chosen by the note; a restart plays the same one at the same pitch and pan.
        let voice = Voice::new(&song, &old.ins, old.note, old.note + old.ins.transpose, vol, old.pan, offset).map(|mut v| {
            v.base = old.base;
            v.target = old.target;
            v.noise_note = old.noise_note;
            v.pan = old.pan;
            v.nna = old.nna;
            v
        });
        let started = voice.as_ref().map(|v| (v.ins.number, v.base));
        self.channels[c].voice = voice;
        if let Some((ins, p)) = started {
            self.event(c, EventKind::On, ins, p);
        }
    }

    /// A cell's curves on tick `k` after its events (section 7).
    /// `last` tells the row's last tick, where a line of one tick more arrives (section 7).
    pub(super) fn curves(&mut self, c: usize, cell: &Cell, k: u32, last: bool) {
        let rate = self.song.rate;
        let ch = &mut self.channels[c];
        let Some(v) = ch.voice.as_mut() else { return };
        if k == 0 {
            // The lines start from the base pitch and note volume the events left, and `vibw`
            // comes before the cell's curves.
            ch.line_pitch = v.base;
            ch.line_vol = v.vol;
            for e in &cell.effects {
                if let Effect::Vibw(w, p) = *e {
                    v.vib_wave = w as u8;
                    v.vib_phase = p;
                }
            }
        }
        let (p0, v0) = (ch.line_pitch, ch.line_vol);
        let k64 = k as i64;
        for e in &cell.effects {
            match *e {
                Effect::Slide(r) if k >= 1 => v.base = (v.base + r).clamp(0, MAX_PITCH),
                Effect::FSlide(r) if k == 0 => v.base = (v.base + r).clamp(0, MAX_PITCH),
                Effect::Port(r) if k >= 1 => {
                    v.base = if v.base < v.target { (v.base + r).min(v.target) } else { (v.base - r).max(v.target) };
                }
                Effect::Vib(d, q) => {
                    let phase = ((v.vib as u64 * 256 / q as u64) as i64 + v.vib_phase as i64).rem_euclid(256);
                    v.curve_offset += (vibrato_wave(v.vib_wave, phase as i32) * d) >> 15;
                    v.vib = v.vib.wrapping_add(1);
                }
                Effect::Arp(x, y) => v.curve_offset += 256 * [0, x, y][(k % 3) as usize],
                Effect::Arp4(a, b, cc, h) => v.curve_offset += 256 * [0, a, b, cc][(k / h as u32 % 4) as usize],
                // The lines take each tick's value from its middle, (2k + 1) ÷ 2d of the way.
                Effect::Glide(d) => {
                    v.base = if k < d as u32 {
                        p0 + floor_div((v.target - p0) as i64 * (2 * k64 + 1), 2 * d as i64) as i32
                    } else {
                        v.target
                    };
                }
                Effect::FGlide(d) => {
                    v.base = if k < d as u32 {
                        let (f0, ft) = (freq(p0) as i64, freq(v.target) as i64);
                        pitch_of((f0 + floor_div((ft - f0) * (2 * k64 + 1), 2 * d as i64)) as u64)
                    } else {
                        v.target
                    };
                }
                Effect::Drop(d) => {
                    if k < d as u32 {
                        let p = v.base;
                        v.curve_offset += pitch_of(freq(p) * (2 * (d as u32 - k) - 1) as u64 / (2 * d) as u64) - p;
                    } else {
                        v.silent = true;
                    }
                }
                Effect::VGlide(target, d) => {
                    v.vol = if k < d as u32 {
                        v0 + floor_div((target - v0) as i64 * (2 * k64 + 1), 2 * d as i64) as i32
                    } else {
                        target
                    };
                }
                Effect::VSlide(r) if k >= 1 => v.vol = (v.vol + r).clamp(0, 64),
                Effect::FVSlide(r) if k == 0 => v.vol = (v.vol + r).clamp(0, 64),
                Effect::Trem(d, q) => {
                    let phase = (v.trem as u64 * 256 / q as u64) % 256;
                    v.tick_vol = Some((v.vol + ((SINE[phase as usize] as i32 * d) >> 15)).clamp(0, 64));
                    v.trem = v.trem.wrapping_add(1);
                }
                Effect::Tremor(a, b) => {
                    if v.tremor % (a + b) as u32 >= a as u32 {
                        v.silent = true;
                    }
                    v.tremor = v.tremor.wrapping_add(1);
                }
                Effect::Pan(p) if k == 0 => v.pan = p,
                Effect::PSlide(r) if k >= 1 => v.pan = (v.pan + r).clamp(-256, 256),
                Effect::Panbr(d, q) => {
                    let phase = (v.panbr as u64 * 256 / q as u64) % 256;
                    v.pan_offset += (SINE[phase as usize] as i32 * d) >> 15;
                    v.panbr = v.panbr.wrapping_add(1);
                }
                Effect::Cutoff(x) if k == 0 => {
                    let r = v.resonance;
                    v.set_filter(x as u8, r, rate);
                }
                Effect::Reso(x) if k == 0 => {
                    let cut = v.cutoff;
                    v.set_filter(cut, x as u8, rate);
                }
                _ => {}
            }
        }
        // A line whose tick d is the first after the row arrives as the row ends.
        if last {
            for e in &cell.effects {
                match *e {
                    Effect::Glide(d) | Effect::FGlide(d) if k + 1 == d as u32 => ch.arrive_pitch = Some(v.target),
                    Effect::VGlide(target, d) if k + 1 == d as u32 => ch.arrive_vol = Some(target),
                    _ => {}
                }
            }
        }
    }
}
