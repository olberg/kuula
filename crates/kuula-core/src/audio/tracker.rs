//! The `.trk` text format and the cursor
//! that walks a track row by row at its tempo.
//!
//! A file is a header of `tempo`, `loop` and `inst` lines followed by
//! rows. `;` starts a comment anywhere; a line starting with `#` is a
//! comment too (`#` elsewhere is a sharp). A row has one cell per channel separated by `|`; a cell is a
//! note (`C-4`, `C#4`, `---` for nothing, `===` for note off), then
//! optionally an instrument number and a volume `v0`..`vf`.

use std::rc::Rc;

use super::synth::{Patch, Wave, MAX_NOTE};
use super::AudioError;

/// Bounds on a track: rows, columns (channels) and instruments.
pub const MAX_ROWS: usize = 1024;
pub const MAX_COLUMNS: usize = 8;
pub const MAX_INSTRUMENTS: usize = 64;
/// Largest `.trk` file accepted.
pub const MAX_TRACK_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instrument {
    Synth(Patch),
    /// A sample by asset name; the mixer resolves it through the bank.
    Sample(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Note {
    #[default]
    None,
    Off,
    On(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cell {
    pub note: Note,
    /// Instrument number as written, 1-based.
    pub inst: Option<u8>,
    /// Volume 0..=255.
    pub vol: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    /// Frames per row.
    pub tempo: u8,
    pub columns: usize,
    /// Row to jump back to at the end; `None` plays once.
    pub loop_row: Option<usize>,
    /// Indexed by number minus one; unset slots are `None`.
    pub instruments: Vec<Option<Instrument>>,
    /// `rows[r][column]`.
    pub rows: Vec<Vec<Cell>>,
}

impl Track {
    pub fn instrument(&self, n: u8) -> Option<&Instrument> {
        self.instruments
            .get(n.checked_sub(1)? as usize)
            .and_then(|i| i.as_ref())
    }

    /// Sample names the instruments refer to.
    pub fn sample_names(&self) -> impl Iterator<Item = &str> {
        self.instruments.iter().filter_map(|i| match i {
            Some(Instrument::Sample(name)) => Some(name.as_str()),
            _ => None,
        })
    }
}

fn fail(path: &str, line: usize, why: impl Into<String>) -> AudioError {
    AudioError::Track {
        path: path.to_string(),
        line: line as u32,
        why: why.into(),
    }
}

fn parse_note(tok: &str) -> Option<Note> {
    match tok {
        "---" | "..." => return Some(Note::None),
        "===" | "off" => return Some(Note::Off),
        _ => {}
    }
    let b = tok.as_bytes();
    if b.len() != 3 {
        return None;
    }
    let semitone = match (b[0].to_ascii_uppercase(), b[1]) {
        (b'C', b'-') => 0,
        (b'C', b'#') => 1,
        (b'D', b'-') => 2,
        (b'D', b'#') => 3,
        (b'E', b'-') => 4,
        (b'F', b'-') => 5,
        (b'F', b'#') => 6,
        (b'G', b'-') => 7,
        (b'G', b'#') => 8,
        (b'A', b'-') => 9,
        (b'A', b'#') => 10,
        (b'B', b'-') => 11,
        _ => return None,
    };
    let octave = (b[2] as char).to_digit(10)?;
    let n = octave * 12 + semitone;
    if n > MAX_NOTE as u32 {
        return None;
    }
    Some(Note::On(n as u8))
}

fn parse_cell(path: &str, line: usize, cell: &str) -> Result<Cell, AudioError> {
    let mut out = Cell::default();
    let mut toks = cell.split_whitespace();
    let Some(first) = toks.next() else {
        return Ok(out);
    };
    out.note = parse_note(first).ok_or_else(|| fail(path, line, format!("bad note {first:?}")))?;
    for tok in toks {
        if let Some(v) = tok.strip_prefix('v') {
            let n = u8::from_str_radix(v, 16)
                .ok()
                .filter(|_| v.len() == 1)
                .ok_or_else(|| fail(path, line, format!("bad volume {tok:?}, want v0..vf")))?;
            out.vol = Some(n * 17);
        } else {
            let n: usize = tok
                .parse()
                .ok()
                .filter(|n| (1..=MAX_INSTRUMENTS).contains(n))
                .ok_or_else(|| {
                    fail(
                        path,
                        line,
                        format!("bad instrument {tok:?}, want 1..{MAX_INSTRUMENTS}"),
                    )
                })?;
            out.inst = Some(n as u8);
        }
    }
    Ok(out)
}

fn parse_int<T: std::str::FromStr>(
    path: &str,
    line: usize,
    key: &str,
    v: &str,
) -> Result<T, AudioError> {
    v.parse()
        .map_err(|_| fail(path, line, format!("bad value {v:?} for {key}")))
}

fn parse_inst(path: &str, line: usize, rest: &str) -> Result<(usize, Instrument), AudioError> {
    let mut toks = rest.split_whitespace();
    let n: usize = toks
        .next()
        .and_then(|t| t.parse().ok())
        .filter(|n| (1..=MAX_INSTRUMENTS).contains(n))
        .ok_or_else(|| {
            fail(
                path,
                line,
                format!("inst needs a number 1..{MAX_INSTRUMENTS}"),
            )
        })?;
    let kind = toks
        .next()
        .ok_or_else(|| fail(path, line, "inst needs a waveform or sample=name"))?;
    if let Some(name) = kind.strip_prefix("sample=") {
        if !crate::manifest::valid_asset_name(name) {
            return Err(fail(path, line, format!("bad sample name {name:?}")));
        }
        return Ok((n, Instrument::Sample(name.to_string())));
    }
    let wave = Wave::parse(kind).ok_or_else(|| {
        fail(
            path,
            line,
            format!("unknown waveform {kind:?}: pulse, tri, saw, sine, noise or sample=name"),
        )
    })?;
    let mut patch = Patch {
        wave,
        ..Patch::default()
    };
    for tok in toks {
        let (key, val) = tok
            .split_once('=')
            .ok_or_else(|| fail(path, line, format!("expected key=value, got {tok:?}")))?;
        match key {
            "duty" => {
                let pct: u32 = parse_int(path, line, key, val)?;
                if pct > 100 {
                    return Err(fail(path, line, "duty is a percentage 0..100"));
                }
                patch.duty = (pct * 255 / 100) as u8;
            }
            "adsr" => {
                let parts: Vec<&str> = val.split(',').collect();
                if parts.len() != 4 {
                    return Err(fail(path, line, "adsr wants attack,decay,sustain,release"));
                }
                patch.attack = parse_int(path, line, key, parts[0])?;
                patch.decay = parse_int(path, line, key, parts[1])?;
                let sustain: u32 = parse_int(path, line, key, parts[2])?;
                if sustain > 100 {
                    return Err(fail(path, line, "sustain is a percentage 0..100"));
                }
                patch.sustain = (sustain * 255 / 100) as u8;
                patch.release = parse_int(path, line, key, parts[3])?;
            }
            "slide" => patch.slide = parse_int(path, line, key, val)?,
            "arp" => {
                let parts: Vec<&str> = val.split(',').collect();
                if parts.len() != 3 {
                    return Err(fail(path, line, "arp wants three semitone offsets"));
                }
                for (slot, p) in patch.arp.iter_mut().zip(parts) {
                    *slot = parse_int(path, line, key, p)?;
                }
            }
            _ => return Err(fail(path, line, format!("unknown instrument key {key:?}"))),
        }
    }
    Ok((n, Instrument::Synth(patch)))
}

/// Parse a `.trk` file. `path` is only for messages.
pub fn parse(path: &str, bytes: &[u8]) -> Result<Track, AudioError> {
    if bytes.len() > MAX_TRACK_BYTES {
        return Err(fail(
            path,
            0,
            format!("file larger than {MAX_TRACK_BYTES} bytes"),
        ));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| fail(path, 0, "not valid UTF-8"))?;
    let mut track = Track {
        tempo: 8,
        columns: 0,
        loop_row: None,
        instruments: Vec::new(),
        rows: Vec::new(),
    };
    let mut loop_row: Option<usize> = None;
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let content = raw.split(';').next().unwrap_or("").trim();
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        let (word, rest) = content
            .split_once(char::is_whitespace)
            .unwrap_or((content, ""));
        match word {
            "tempo" => {
                let t: u32 = parse_int(path, line, "tempo", rest.trim())?;
                if !(1..=255).contains(&t) {
                    return Err(fail(path, line, "tempo is frames per row, 1..255"));
                }
                track.tempo = t as u8;
            }
            "loop" => {
                loop_row = Some(parse_int(path, line, "loop", rest.trim())?);
            }
            "inst" => {
                let (n, inst) = parse_inst(path, line, rest)?;
                if track.instruments.len() < n {
                    track.instruments.resize(n, None);
                }
                if track.instruments[n - 1].is_some() {
                    return Err(fail(path, line, format!("instrument {n} defined twice")));
                }
                track.instruments[n - 1] = Some(inst);
            }
            _ => {
                if track.rows.len() >= MAX_ROWS {
                    return Err(fail(path, line, format!("more than {MAX_ROWS} rows")));
                }
                let cells: Vec<Cell> = content
                    .split('|')
                    .map(|c| parse_cell(path, line, c))
                    .collect::<Result<_, _>>()?;
                if cells.len() > MAX_COLUMNS {
                    return Err(fail(
                        path,
                        line,
                        format!("more than {MAX_COLUMNS} channels"),
                    ));
                }
                for cell in &cells {
                    if let Some(n) = cell.inst {
                        if track.instrument(n).is_none() {
                            return Err(fail(path, line, format!("instrument {n} is not defined")));
                        }
                    }
                }
                track.columns = track.columns.max(cells.len());
                track.rows.push(cells);
            }
        }
    }
    if track.rows.is_empty() {
        return Err(fail(path, 0, "no rows"));
    }
    if let Some(l) = loop_row {
        if l >= track.rows.len() {
            return Err(fail(path, 0, format!("loop row {l} is past the last row")));
        }
        track.loop_row = Some(l);
    }
    for row in &mut track.rows {
        row.resize(track.columns, Cell::default());
    }
    Ok(track)
}

/// What the cursor wants the mixer to do this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing new this frame.
    Hold,
    /// Apply this row's cells.
    Row(usize),
    /// The track ended (or faded out): release its channels and drop it.
    End,
}

/// Gain scale: 256 is unity.
pub const GAIN_ONE: i32 = 256;

/// A playing track on channels `base..base + columns`.
#[derive(Debug, Clone)]
pub struct Cursor {
    pub track: Rc<Track>,
    pub base: usize,
    row: usize,
    countdown: u32,
    /// 0..=256, applied to every note of the track.
    pub gain: i32,
    fade: i32,
    stop_when_silent: bool,
}

impl Cursor {
    /// `fade_in` frames from silence to unity; 0 starts at unity.
    pub fn new(track: Rc<Track>, base: usize, fade_in: u32) -> Cursor {
        let (gain, fade) = if fade_in == 0 {
            (GAIN_ONE, 0)
        } else {
            (0, (GAIN_ONE / fade_in as i32).max(1))
        };
        Cursor {
            track,
            base,
            row: 0,
            countdown: 0,
            gain,
            fade,
            stop_when_silent: false,
        }
    }

    pub fn channels(&self) -> std::ops::Range<usize> {
        self.base..self.base + self.track.columns
    }

    pub fn row(&self) -> usize {
        self.row
    }

    /// Fade to silence over `frames` and then end.
    pub fn fade_out(&mut self, frames: u32) {
        self.stop_when_silent = true;
        self.fade = if frames == 0 {
            -GAIN_ONE
        } else {
            -(GAIN_ONE / frames as i32).max(1)
        };
    }

    /// Advance one frame.
    pub fn tick(&mut self) -> Step {
        if self.fade != 0 {
            self.gain = (self.gain + self.fade).clamp(0, GAIN_ONE);
            if self.gain == GAIN_ONE && self.fade > 0 {
                self.fade = 0;
            }
            if self.gain == 0 && self.stop_when_silent {
                return Step::End;
            }
        }
        if self.countdown > 0 {
            self.countdown -= 1;
            return Step::Hold;
        }
        if self.row >= self.track.rows.len() {
            match self.track.loop_row {
                Some(l) => self.row = l,
                None => return Step::End,
            }
        }
        let r = self.row;
        self.row += 1;
        self.countdown = self.track.tempo as u32 - 1;
        Step::Row(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SONG: &str = "\
# a test song
tempo 4
loop 1
inst 1 pulse duty=25 adsr=1,2,50,3 slide=-2 arp=0,4,7
inst 2 sample=kick
inst 3 noise
C-4 1 vf | D#5 2   ; sharps are not comments
--- | === v8
G-8 3 |
";

    #[test]
    fn parses_header_and_rows() {
        let t = parse("music/a.trk", SONG.as_bytes()).unwrap();
        assert_eq!((t.tempo, t.columns, t.loop_row), (4, 2, Some(1)));
        assert_eq!(t.rows.len(), 3);
        assert_eq!(
            t.instrument(1),
            Some(&Instrument::Synth(Patch {
                wave: Wave::Pulse,
                duty: 63,
                attack: 1,
                decay: 2,
                sustain: 127,
                release: 3,
                slide: -2,
                arp: [0, 4, 7],
            }))
        );
        assert_eq!(t.instrument(2), Some(&Instrument::Sample("kick".into())));
        assert_eq!(t.instrument(4), None);
        assert_eq!(t.sample_names().collect::<Vec<_>>(), ["kick"]);
        assert_eq!(
            t.rows[0][0],
            Cell {
                note: Note::On(48),
                inst: Some(1),
                vol: Some(255)
            }
        );
        assert_eq!(t.rows[0][1].note, Note::On(63));
        assert_eq!(t.rows[1][0], Cell::default());
        assert_eq!(
            t.rows[1][1],
            Cell {
                note: Note::Off,
                inst: None,
                vol: Some(136)
            }
        );
        assert_eq!(t.rows[2][1], Cell::default(), "short rows are padded");
        assert_eq!(t.rows[2][0].note, Note::On(103));
    }

    #[test]
    fn errors_carry_the_line() {
        let err = |src: &str| match parse("sfx/x.trk", src.as_bytes()).unwrap_err() {
            AudioError::Track { line, why, .. } => (line, why),
            e => panic!("{e}"),
        };
        assert_eq!(err("tempo 0\nC-4").0, 1);
        assert!(err("inst 1 pulse\nH-4 1").1.contains("bad note"));
        assert!(err("C-4 5").1.contains("not defined"));
        assert!(err("inst 1 flute\nC-4").1.contains("unknown waveform"));
        assert!(err("inst 1 pulse duty=200\nC-4").1.contains("duty"));
        assert!(err("C-9").1.contains("bad note"));
        assert!(err("C-4 vg").1.contains("volume"));
        assert!(err("loop 3\nC-4").1.contains("loop row"));
        assert_eq!(err("").1, "no rows");
        assert!(err("---|---|---|---|---|---|---|---|---")
            .1
            .contains("channels"));
        assert!(err("inst 1 sample=../x\nC-4").1.contains("sample name"));
        let many = "C-4\n".repeat(MAX_ROWS + 1);
        assert_eq!(err(&many).0 as usize, MAX_ROWS + 1);
        assert_eq!(parse("sfx/x.trk", "C-4\n".as_bytes()).unwrap().tempo, 8);
    }

    #[test]
    fn the_cursor_steps_rows_at_the_tempo_and_loops() {
        let t = Rc::new(parse("music/a.trk", SONG.as_bytes()).unwrap());
        let mut c = Cursor::new(t.clone(), 0, 0);
        let steps: Vec<Step> = (0..13).map(|_| c.tick()).collect();
        assert_eq!(
            steps,
            [
                Step::Row(0),
                Step::Hold,
                Step::Hold,
                Step::Hold,
                Step::Row(1),
                Step::Hold,
                Step::Hold,
                Step::Hold,
                Step::Row(2),
                Step::Hold,
                Step::Hold,
                Step::Hold,
                Step::Row(1),
            ]
        );
        let once = Rc::new(parse("sfx/b.trk", b"tempo 1\nC-4\nD-4\n").unwrap());
        let mut c = Cursor::new(once, 3, 0);
        assert_eq!(c.channels(), 3..4);
        assert_eq!(c.tick(), Step::Row(0));
        assert_eq!(c.tick(), Step::Row(1));
        assert_eq!(c.tick(), Step::End);
    }

    #[test]
    fn fades_in_and_out() {
        let t = Rc::new(parse("music/a.trk", b"tempo 1\nloop 0\nC-4\n").unwrap());
        let mut c = Cursor::new(t.clone(), 0, 4);
        assert_eq!(c.gain, 0);
        c.tick();
        assert_eq!(c.gain, 64);
        for _ in 0..10 {
            c.tick();
        }
        assert_eq!(c.gain, GAIN_ONE);
        c.fade_out(2);
        assert_eq!(c.tick(), Step::Row(0));
        assert_eq!(c.gain, 128);
        assert_eq!(c.tick(), Step::End);
        let mut c = Cursor::new(t, 0, 0);
        c.fade_out(0);
        assert_eq!(c.tick(), Step::End);
    }
}
