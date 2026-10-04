//! The cell grammar of docs/omt.md, section 5: `"C-4 01 40 vib 48 2"`.

/// A cell's note field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    None,
    /// A pitch in 1/256 semitone.
    On(i32),
    Release,
    Cut,
    Fade,
}

/// A named effect with its arguments, as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Slide(i32),
    Port(i32),
    Vib(i32, i32),
    Arp(i32, i32),
    VSlide(i32),
    Pan(i32),
    Offset(i32),
    Delay(i32),
    Cut(i32),
    Retrig(i32),
    FSlide(i32),
    FVSlide(i32),
    Trem(i32, i32),
    Tremor(i32, i32),
    PSlide(i32),
    Panbr(i32, i32),
    Past(i32),
    Nna(i32),
    Retrigv(i32, i32),
    Cutoff(i32),
    Reso(i32),
    Arp4(i32, i32, i32, i32),
    Glide(i32),
    FGlide(i32),
    Drop(i32),
    VGlide(i32, i32),
    Vibw(i32, i32),
}

/// The effects of section 7: name, arguments and each argument's range.
pub const EFFECTS: &[(&str, &[(i64, i64)])] = &[
    ("slide", &[(-4096, 4096)]),
    ("port", &[(1, 4096)]),
    ("vib", &[(0, 4096), (1, 256)]),
    ("arp", &[(0, 96), (0, 96)]),
    ("vslide", &[(-64, 64)]),
    ("pan", &[(-256, 256)]),
    ("offset", &[(0, 1 << 24)]),
    ("delay", &[(1, 255)]),
    ("cut", &[(0, 255)]),
    ("retrig", &[(1, 255)]),
    ("fslide", &[(-4096, 4096)]),
    ("fvslide", &[(-64, 64)]),
    ("trem", &[(0, 64), (1, 256)]),
    ("tremor", &[(1, 255), (1, 255)]),
    ("pslide", &[(-256, 256)]),
    ("panbr", &[(0, 256), (1, 256)]),
    ("past", &[(0, 2)]),
    ("nna", &[(0, 3)]),
    ("retrigv", &[(1, 255), (0, 15)]),
    ("cutoff", &[(0, 127)]),
    ("reso", &[(0, 127)]),
    ("arp4", &[(-96, 96), (-96, 96), (-96, 96), (1, 255)]),
    ("glide", &[(1, 255)]),
    ("fglide", &[(1, 255)]),
    ("drop", &[(1, 255)]),
    ("vglide", &[(0, 64), (1, 255)]),
    ("vibw", &[(0, 3), (0, 255)]),
];

impl Effect {
    pub fn name(&self) -> &'static str {
        match self {
            Effect::Slide(_) => "slide",
            Effect::Port(_) => "port",
            Effect::Vib(..) => "vib",
            Effect::Arp(..) => "arp",
            Effect::VSlide(_) => "vslide",
            Effect::Pan(_) => "pan",
            Effect::Offset(_) => "offset",
            Effect::Delay(_) => "delay",
            Effect::Cut(_) => "cut",
            Effect::Retrig(_) => "retrig",
            Effect::FSlide(_) => "fslide",
            Effect::FVSlide(_) => "fvslide",
            Effect::Trem(..) => "trem",
            Effect::Tremor(..) => "tremor",
            Effect::PSlide(_) => "pslide",
            Effect::Panbr(..) => "panbr",
            Effect::Past(_) => "past",
            Effect::Nna(_) => "nna",
            Effect::Retrigv(..) => "retrigv",
            Effect::Cutoff(_) => "cutoff",
            Effect::Reso(_) => "reso",
            Effect::Arp4(..) => "arp4",
            Effect::Glide(_) => "glide",
            Effect::FGlide(_) => "fglide",
            Effect::Drop(_) => "drop",
            Effect::VGlide(..) => "vglide",
            Effect::Vibw(..) => "vibw",
        }
    }

    fn build(name: &str, a: &[i32]) -> Effect {
        match name {
            "slide" => Effect::Slide(a[0]),
            "port" => Effect::Port(a[0]),
            "vib" => Effect::Vib(a[0], a[1]),
            "arp" => Effect::Arp(a[0], a[1]),
            "vslide" => Effect::VSlide(a[0]),
            "pan" => Effect::Pan(a[0]),
            "offset" => Effect::Offset(a[0]),
            "delay" => Effect::Delay(a[0]),
            "cut" => Effect::Cut(a[0]),
            "retrig" => Effect::Retrig(a[0]),
            "fslide" => Effect::FSlide(a[0]),
            "fvslide" => Effect::FVSlide(a[0]),
            "trem" => Effect::Trem(a[0], a[1]),
            "tremor" => Effect::Tremor(a[0], a[1]),
            "pslide" => Effect::PSlide(a[0]),
            "panbr" => Effect::Panbr(a[0], a[1]),
            "past" => Effect::Past(a[0]),
            "nna" => Effect::Nna(a[0]),
            "retrigv" => Effect::Retrigv(a[0], a[1]),
            "cutoff" => Effect::Cutoff(a[0]),
            "arp4" => Effect::Arp4(a[0], a[1], a[2], a[3]),
            "glide" => Effect::Glide(a[0]),
            "fglide" => Effect::FGlide(a[0]),
            "drop" => Effect::Drop(a[0]),
            "vglide" => Effect::VGlide(a[0], a[1]),
            "vibw" => Effect::Vibw(a[0], a[1]),
            _ => Effect::Reso(a[0]),
        }
    }

    pub fn args(&self) -> Vec<i32> {
        match *self {
            Effect::Vib(a, b) | Effect::Arp(a, b) | Effect::Trem(a, b) | Effect::Tremor(a, b) | Effect::Panbr(a, b)
            | Effect::Retrigv(a, b) | Effect::VGlide(a, b) | Effect::Vibw(a, b) => vec![a, b],
            Effect::Arp4(a, b, c, h) => vec![a, b, c, h],
            Effect::Slide(a)
            | Effect::Port(a)
            | Effect::VSlide(a)
            | Effect::Pan(a)
            | Effect::Offset(a)
            | Effect::Delay(a)
            | Effect::Cut(a)
            | Effect::Retrig(a)
            | Effect::FSlide(a)
            | Effect::FVSlide(a)
            | Effect::PSlide(a)
            | Effect::Past(a)
            | Effect::Nna(a)
            | Effect::Cutoff(a)
            | Effect::Reso(a)
            | Effect::Glide(a)
            | Effect::FGlide(a)
            | Effect::Drop(a) => vec![a],
        }
    }
}

/// A cell as written: its fields before the track resolves them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub note: Note,
    /// 1..=99, or 0 when the cell leaves it out.
    pub ins: u8,
    /// 0..=64, or `None` when left out.
    pub vol: Option<u8>,
    pub effects: Vec<Effect>,
}

/// Why a cell didn't parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellError {
    /// `bad-cell`: malformed, or an argument out of range.
    Bad(String),
    /// `unknown-effect`.
    UnknownEffect(String),
}

const LETTERS: [(u8, i32, bool); 7] = [
    (b'C', 0, true),
    (b'D', 2, true),
    (b'E', 4, false),
    (b'F', 5, true),
    (b'G', 7, true),
    (b'A', 9, true),
    (b'B', 11, false),
];

/// The pitch of a note name such as `C-4` or `F#2`, octaves 0 to 9.
pub fn parse_note_name(s: &str) -> Option<i32> {
    let b = s.as_bytes();
    if b.len() != 3 || !b[2].is_ascii_digit() {
        return None;
    }
    let (_, semitone, sharpable) = *LETTERS.iter().find(|l| l.0 == b[0])?;
    let semitone = match b[1] {
        b'-' => semitone,
        b'#' if sharpable => semitone + 1,
        _ => return None,
    };
    let octave = (b[2] - b'0') as i32;
    Some((12 * (octave + 1) + semitone) * 256)
}

/// The name of a pitch that is a whole note in octaves 0 to 9, such as `C-4`.
pub fn note_name(pitch: i32) -> Option<String> {
    if pitch % 256 != 0 {
        return None;
    }
    let midi = pitch / 256;
    if !(12..=131).contains(&midi) {
        return None;
    }
    const NAMES: [&str; 12] = ["C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-"];
    Some(format!("{}{}", NAMES[(midi % 12) as usize], midi / 12 - 1))
}

fn parse_int(s: &str) -> Option<i64> {
    let digits = s.strip_prefix('-').unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) || digits.len() > 12 {
        return None;
    }
    if digits.len() > 1 && digits.starts_with('0') {
        return None;
    }
    if s.starts_with('-') && digits == "0" {
        return None;
    }
    s.parse().ok()
}

fn two_digits(s: &str) -> Option<u8> {
    let b = s.as_bytes();
    if b.len() == 2 && b[0].is_ascii_digit() && b[1].is_ascii_digit() {
        Some((b[0] - b'0') * 10 + (b[1] - b'0'))
    } else {
        None
    }
}

/// Parses a cell. On success, the second value tells whether it is spelled canonically.
pub fn parse(text: &str) -> Result<(Cell, bool), CellError> {
    parse_as(text, false).map(|(cell, canonical, _)| (cell, canonical))
}

/// Parses a cell of a song whose minor version is above the reader's when `later`: an effect name
/// this version doesn't define, with the integers after it, is then left out of the cell rather
/// than an error (section 1). The third value counts the effects left out.
pub fn parse_as(text: &str, later: bool) -> Result<(Cell, bool, usize), CellError> {
    let bad = |why: &str| Err(CellError::Bad(why.to_string()));
    let tokens: Vec<&str> = text.split(' ').collect();
    if tokens.iter().any(|t| t.is_empty()) {
        return bad("fields are separated by exactly one space");
    }
    let note = match tokens[0] {
        "..." => Note::None,
        "===" => Note::Release,
        "^^^" => Note::Cut,
        "~~~" => Note::Fade,
        name => match parse_note_name(name) {
            Some(p) => Note::On(p),
            None => return bad("not a note"),
        },
    };
    let mut ins = 0;
    if let Some(&t) = tokens.get(1) {
        if t != ".." {
            match two_digits(t) {
                Some(n) if (1..=99).contains(&n) => ins = n,
                _ => return bad("the instrument is 01 to 99"),
            }
        }
    }
    let mut vol = None;
    if let Some(&t) = tokens.get(2) {
        if t != ".." {
            match two_digits(t) {
                Some(n) if n <= 64 => vol = Some(n),
                _ => return bad("the volume is 00 to 64"),
            }
        }
    }
    let mut effects: Vec<Effect> = Vec::new();
    // Every effect as written, the ones left out included, for the canonical spelling.
    let mut written: Vec<String> = Vec::new();
    let mut names: Vec<&str> = Vec::new();
    let mut unknown: Vec<&str> = Vec::new();
    let mut i = 3;
    while i < tokens.len() {
        let name = tokens[i];
        let word = name.as_bytes()[0].is_ascii_lowercase() && name.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
        if !word {
            return bad("an effect is a lowercase letter, then letters and digits");
        }
        if names.contains(&name) {
            return bad(&format!("{name} twice in one cell"));
        }
        names.push(name);
        let Some((_, ranges)) = EFFECTS.iter().find(|e| e.0 == name) else {
            // As many integers as it takes: every one up to the next word. The rest of the cell
            // must still parse: a malformed cell is a bad cell, whatever its effects.
            let mut text = name.to_string();
            i += 1;
            while let Some(v) = tokens.get(i).and_then(|t| parse_int(t)) {
                text.push_str(&format!(" {v}"));
                i += 1;
            }
            written.push(text);
            unknown.push(name);
            continue;
        };
        let mut args = Vec::new();
        for (j, &(lo, hi)) in ranges.iter().enumerate() {
            let Some(v) = tokens.get(i + 1 + j).and_then(|t| parse_int(t)) else {
                return bad(&format!("{name} takes {} integers", ranges.len()));
            };
            if v < lo || v > hi {
                return bad(&format!("{name}'s argument {} is {lo} to {hi}", j + 1));
            }
            args.push(v as i32);
        }
        let effect = Effect::build(name, &args);
        written.push(effect_text(&effect));
        effects.push(effect);
        i += 1 + ranges.len();
    }
    if let (Some(name), false) = (unknown.first(), later) {
        return Err(CellError::UnknownEffect(name.to_string()));
    }
    let cell = Cell { note, ins, vol, effects };
    let canonical = spell(&cell, &written) == text;
    Ok((cell, canonical, unknown.len()))
}

fn effect_text(e: &Effect) -> String {
    let mut parts = vec![e.name().to_string()];
    parts.extend(e.args().iter().map(|a| a.to_string()));
    parts.join(" ")
}

/// A cell in its canonical spelling; `""` for a cell with nothing in it.
pub fn format(cell: &Cell) -> String {
    let effects: Vec<String> = cell.effects.iter().map(effect_text).collect();
    spell(cell, &effects)
}

/// The canonical spelling of a cell's note, instrument and volume with `effects` as written.
fn spell(cell: &Cell, effects: &[String]) -> String {
    let note = match cell.note {
        Note::None => "...".to_string(),
        Note::Release => "===".to_string(),
        Note::Cut => "^^^".to_string(),
        Note::Fade => "~~~".to_string(),
        Note::On(p) => note_name(p).unwrap_or_else(|| "???".to_string()),
    };
    let ins = if cell.ins == 0 { "..".to_string() } else { format!("{:02}", cell.ins) };
    let vol = cell.vol.map_or("..".to_string(), |v| format!("{v:02}"));
    let mut parts = vec![note, ins, vol];
    parts.extend(effects.iter().cloned());
    if effects.is_empty() {
        while parts.len() > 1 && parts.last().is_some_and(|p| p == "..") {
            parts.pop();
        }
        if parts.len() == 1 && parts[0] == "..." {
            return String::new();
        }
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_example_cells() {
        let (c, canon) = parse("D-4 .. .. delay 3").unwrap();
        assert!(canon);
        assert_eq!(c.note, Note::On(62 * 256));
        assert_eq!(c.effects, [Effect::Delay(3)]);
        let (c, _) = parse("E-4 .. 48 vib 64 8 vslide -2").unwrap();
        assert_eq!(c.vol, Some(48));
        assert_eq!(c.effects, [Effect::Vib(64, 8), Effect::VSlide(-2)]);
        let (c, _) = parse("G-4 .. .. port 96").unwrap();
        assert_eq!(c.effects, [Effect::Port(96)]);
        let (c, canon) = parse("... .. 20").unwrap();
        assert!(canon);
        assert_eq!((c.note, c.ins, c.vol), (Note::None, 0, Some(20)));
    }

    #[test]
    fn notes_follow_scientific_pitch() {
        assert_eq!(parse_note_name("C-4"), Some(60 * 256));
        assert_eq!(parse_note_name("A-4"), Some(69 * 256));
        assert_eq!(parse_note_name("C-0"), Some(12 * 256));
        assert_eq!(parse_note_name("B-9"), Some(131 * 256));
        assert_eq!(parse_note_name("E#4"), None);
        assert_eq!(parse_note_name("c-4"), None);
        assert_eq!(note_name(69 * 256).as_deref(), Some("A-4"));
        assert_eq!(note_name(61 * 256).as_deref(), Some("C#4"));
        assert_eq!(note_name(11 * 256), None);
    }

    #[test]
    fn spelling_and_errors() {
        assert!(parse("C-4 01").unwrap().1);
        assert!(!parse("C-4 01 ..").unwrap().1, "trailing empty field");
        assert!(!parse("...").unwrap().1, "an empty cell");
        assert!(parse("===").unwrap().1);
        let bad = |s: &str| matches!(parse(s), Err(CellError::Bad(_)));
        assert!(bad("C-4  01"));
        assert!(bad("C-4 00"));
        assert!(bad("C-4 01 65"));
        assert!(bad("C-4 01 40 vib 48"));
        assert!(bad("C-4 01 40 vib 48 0"));
        assert!(bad("C-4 01 40 slide -0"));
        assert!(bad("C-4 01 40 slide 01"));
        assert!(bad("C-4 01 40 arp 3 4 arp 3 4"));
        assert!(bad("H-4"));
        assert_eq!(parse("C-4 01 40 wobble 3"), Err(CellError::UnknownEffect("wobble".into())));
        let (c, _) = parse("C-4 01 40 vib 48 2").unwrap();
        assert_eq!(format(&c), "C-4 01 40 vib 48 2");
    }
}
