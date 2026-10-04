//! The Open Module Track engine (docs/omt.md): loads and validates OMT songs and renders them
//! exactly. One production engine, shared by the tracker and Kuula (the draft's decision 16).

pub mod cell;
pub mod flac;
pub mod ogg;
pub mod omc;
pub mod player;
pub mod song;
pub mod tables;

pub use player::{render, Event, EventKind, Mode, Player, Rendering};
pub use song::{load, Diag, Loaded, Song};

/// Loads the song of an OMC file, with its diagnostics. `Err` when the file has no default-format
/// song to read.
pub fn load_omc(file: &[u8]) -> Result<(Loaded, usize), String> {
    let entry = omc::read_song(file)?;
    Ok((load(&entry.payload, &entry.resources), entry.subsong))
}

/// SHA-256 of bytes, in lowercase hexadecimal.
pub fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The JSON Canonicalization Scheme (RFC 8785) of a payload (section 15): members sorted by UTF-16
/// code units, no whitespace, numbers spelled as ECMAScript spells them, so `44100.0` is `44100`.
/// `None` if it isn't I-JSON, as section 1 reads it.
pub fn canonical(payload: &[u8]) -> Option<Vec<u8>> {
    let v = song::parse_ijson(payload)?;
    let mut out = String::new();
    jcs(&v, &mut out);
    Some(out.into_bytes())
}

fn jcs(v: &serde_json::Value, out: &mut String) {
    use serde_json::Value;
    match v {
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).unwrap());
                out.push(':');
                jcs(&o[k.as_str()], out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                jcs(x, out);
            }
            out.push(']');
        }
        Value::String(s) => jcs_string(s, out),
        Value::Number(n) => match n.as_i64() {
            Some(i) => out.push_str(&i.to_string()),
            None => jcs_number(n.as_f64().unwrap_or(0.0), out),
        },
        other => out.push_str(&other.to_string()),
    }
}

/// A number that isn't an integer, as ECMAScript's Number::toString writes it (RFC 8785, section
/// 3.2.2.3): the shortest digits that round-trip, in plain notation from 10^-6 up, else with an
/// exponent. I-JSON keeps numbers below 2^53, so the large exponent form never occurs.
fn jcs_number(x: f64, out: &mut String) {
    let sci = format!("{:e}", x.abs()); // shortest round-trip digits: "1.5e-7"
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let n = exp.parse::<i32>().unwrap_or(0) + 1; // the decimal point sits after n digits
    let k = digits.len() as i32;
    if x < 0.0 {
        out.push('-');
    }
    if 0 < n && n <= 21 {
        if k <= n {
            out.push_str(&digits);
            out.push_str(&"0".repeat((n - k) as usize));
        } else {
            out.push_str(&digits[..n as usize]);
            out.push('.');
            out.push_str(&digits[n as usize..]);
        }
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        out.push_str(&"0".repeat(-n as usize));
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push_str(&format!("e{}{}", if n - 1 < 0 { "-" } else { "+" }, (n - 1).abs()));
    }
}

/// RFC 8785 strings: only `"`, `\` and control characters are escaped, controls as `\b \t \n \f
/// \r` or `\u00xx`.
fn jcs_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}
