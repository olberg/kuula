//! The canonical codec: the value
//! model, the total key order, the limits and the error codes. The text
//! form lives in [`text`], the JSON form in [`json`]. Nothing here runs
//! Lua; every input is parsed by hand and bounded before it is
//! allocated.

mod json;
mod text;

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

pub use json::{from_json, to_json};
pub use text::{decode, decode_bytes, encode};

/// Most nested tables; the top-level table is depth 1.
pub const MAX_DEPTH: usize = 32;
/// Most table entries in one document, all tables together.
pub const MAX_ENTRIES: usize = 65_536;
/// Most bytes of encoded text.
pub const MAX_ENCODED: usize = 256 * 1024;
/// Most decoded bytes: 16 per value plus the payload of every string and
/// blob.
pub const MAX_DECODED: usize = 1024 * 1024;
/// Bytes every value counts for against [`MAX_DECODED`].
pub const VALUE_COST: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecError {
    /// Stable code, one of the `codec_*` constants below.
    pub code: &'static str,
    pub message: String,
}

impl CodecError {
    pub const CYCLE: &'static str = "codec_cycle";
    pub const UNSUPPORTED: &'static str = "codec_unsupported";
    pub const DEPTH: &'static str = "codec_depth";
    pub const SIZE: &'static str = "codec_size";
    pub const SYNTAX: &'static str = "codec_syntax";
    pub const KEY: &'static str = "codec_key";
    pub const NUMBER: &'static str = "codec_number";

    pub fn new(code: &'static str, message: impl Into<String>) -> CodecError {
        CodecError {
            code,
            message: message.into(),
        }
    }

    pub fn depth() -> CodecError {
        CodecError::new(
            Self::DEPTH,
            format!("tables nested deeper than {MAX_DEPTH}"),
        )
    }

    pub fn entries() -> CodecError {
        CodecError::new(Self::SIZE, format!("more than {MAX_ENTRIES} table entries"))
    }

    pub fn encoded() -> CodecError {
        CodecError::new(
            Self::SIZE,
            format!("encoded text longer than {MAX_ENCODED} bytes"),
        )
    }

    pub fn decoded() -> CodecError {
        CodecError::new(
            Self::SIZE,
            format!("decoded value larger than {MAX_DECODED} bytes"),
        )
    }
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CodecError {}

/// A value of the model. Floats are finite by the constructors that
/// matter (`Table::insert`, the decoders); a `Value::Float` built by hand
/// is checked again when it is encoded.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Vec<u8>),
    Table(Table),
    Blob(Vec<u8>),
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::Str(s.as_bytes().to_vec())
    }

    fn kind_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "boolean",
            Value::Int(_) => "integer",
            Value::Float(_) => "float",
            Value::Str(_) => "string",
            Value::Table(_) => "table",
            Value::Blob(_) => "blob",
        }
    }
}

/// A table key: boolean, integer, float or string, in that order.
#[derive(Debug, Clone)]
pub enum Key {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Vec<u8>),
}

impl Key {
    pub fn str(s: &str) -> Key {
        Key::Str(s.as_bytes().to_vec())
    }

    /// The key a value makes, applying the canonical key rules
    /// section 1.1.
    pub fn from_value(v: Value) -> Result<Key, CodecError> {
        match v {
            Value::Bool(b) => Ok(Key::Bool(b)),
            Value::Int(i) => Ok(Key::Int(i)),
            Value::Float(f) => Key::float(f),
            Value::Str(s) => Ok(Key::Str(s)),
            other => Err(CodecError::new(
                CodecError::KEY,
                format!("{} keys are not allowed", other.kind_name()),
            )),
        }
    }

    /// A float key: finite and not an integer value.
    pub fn float(f: f64) -> Result<Key, CodecError> {
        check_float(f)?;
        if f.fract() == 0.0
            && (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&f)
        {
            return Err(CodecError::new(
                CodecError::KEY,
                format!("float key {f} has an integer value; use an integer key"),
            ));
        }
        Ok(Key::Float(f))
    }

    pub fn into_value(self) -> Value {
        match self {
            Key::Bool(b) => Value::Bool(b),
            Key::Int(i) => Value::Int(i),
            Key::Float(f) => Value::Float(f),
            Key::Str(s) => Value::Str(s),
        }
    }

    fn rank(&self) -> u8 {
        match self {
            Key::Bool(_) => 0,
            Key::Int(_) | Key::Float(_) => 1,
            Key::Str(_) => 2,
        }
    }
}

/// Exact comparison of an integer with a finite float.
fn cmp_int_float(i: i64, f: f64) -> Ordering {
    const TWO63: f64 = 9_223_372_036_854_775_808.0;
    if f >= TWO63 {
        return Ordering::Less;
    }
    if f < -TWO63 {
        return Ordering::Greater;
    }
    let t = f.trunc();
    match i.cmp(&(t as i64)) {
        Ordering::Equal => {
            let frac = f - t;
            if frac > 0.0 {
                Ordering::Less
            } else if frac < 0.0 {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        }
        other => other,
    }
}

impl Ord for Key {
    fn cmp(&self, other: &Key) -> Ordering {
        match (self, other) {
            (Key::Bool(a), Key::Bool(b)) => a.cmp(b),
            (Key::Int(a), Key::Int(b)) => a.cmp(b),
            (Key::Int(a), Key::Float(b)) => cmp_int_float(*a, *b).then(Ordering::Less),
            (Key::Float(a), Key::Int(b)) => cmp_int_float(*b, *a).reverse().then(Ordering::Greater),
            (Key::Float(a), Key::Float(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
            (Key::Str(a), Key::Str(b)) => a.cmp(b),
            _ => self.rank().cmp(&other.rank()),
        }
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Key) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Key) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Key {}

/// A table: keys in the total order, values never nil.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    entries: BTreeMap<Key, Value>,
}

impl Table {
    pub fn new() -> Table {
        Table::default()
    }

    /// Insert, applying the key rules; a nil value is `codec_unsupported`.
    /// Returns whatever the key held before.
    pub fn insert(&mut self, key: Key, value: Value) -> Result<Option<Value>, CodecError> {
        if let Key::Float(f) = key {
            Key::float(f)?;
        }
        if let Value::Nil = value {
            return Err(CodecError::new(
                CodecError::UNSUPPORTED,
                "nil is not a table value",
            ));
        }
        Ok(self.entries.insert(key, value))
    }

    pub fn get(&self, key: &Key) -> Option<&Value> {
        self.entries.get(key)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Key, &Value)> {
        self.entries.iter()
    }
}

impl IntoIterator for Table {
    type Item = (Key, Value);
    type IntoIter = std::collections::btree_map::IntoIter<Key, Value>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

/// NaN and the infinities have no encoding.
pub fn check_float(f: f64) -> Result<(), CodecError> {
    if f.is_finite() {
        Ok(())
    } else {
        Err(CodecError::new(
            CodecError::NUMBER,
            format!("{f} cannot be encoded"),
        ))
    }
}

/// Running totals a walk keeps against the limits.
#[derive(Debug, Default, Clone, Copy)]
pub struct Budget {
    pub depth: usize,
    pub entries: usize,
    pub decoded: usize,
}

impl Budget {
    /// Enter a nested table. On failure the depth is unchanged, so a
    /// lenient walk that drops the table needs no matching `leave`.
    pub fn enter(&mut self) -> Result<(), CodecError> {
        if self.depth >= MAX_DEPTH {
            return Err(CodecError::depth());
        }
        self.depth += 1;
        Ok(())
    }

    pub fn leave(&mut self) {
        self.depth -= 1;
    }

    pub fn entry(&mut self) -> Result<(), CodecError> {
        self.entries += 1;
        if self.entries > MAX_ENTRIES {
            return Err(CodecError::entries());
        }
        Ok(())
    }

    /// One value with `payload` bytes of string or blob.
    pub fn value(&mut self, payload: usize) -> Result<(), CodecError> {
        self.decoded = self
            .decoded
            .saturating_add(VALUE_COST)
            .saturating_add(payload);
        if self.decoded > MAX_DECODED {
            return Err(CodecError::decoded());
        }
        Ok(())
    }
}

// ----- base64 ---------------------------------------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Strict decode: canonical padding, no whitespace, zero padding bits.
pub fn base64_decode(text: &[u8]) -> Result<Vec<u8>, CodecError> {
    let bad = || CodecError::new(CodecError::SYNTAX, "malformed base64");
    if !text.len().is_multiple_of(4) {
        return Err(bad());
    }
    let val = |c: u8| -> Result<u32, CodecError> {
        Ok(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a') as u32 + 26,
            b'0'..=b'9' => (c - b'0') as u32 + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(bad()),
        })
    };
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    for (i, q) in text.chunks(4).enumerate() {
        let last = i + 1 == text.len() / 4;
        let pad = q.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && !last) || q[..4 - pad].contains(&b'=') {
            return Err(bad());
        }
        let mut n = 0u32;
        for &c in &q[..4 - pad] {
            n = n << 6 | val(c)?;
        }
        n <<= 6 * pad as u32;
        let bytes = n.to_be_bytes();
        // Padding bits must be zero so an encoding is unique.
        let keep = 3 - pad;
        if (pad == 1 && n & 0xff != 0) || (pad == 2 && n & 0xffff != 0) {
            return Err(bad());
        }
        out.extend_from_slice(&bytes[1..1 + keep]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_order_is_bool_number_string() {
        let mut keys = vec![
            Key::str("b"),
            Key::Float(1.5),
            Key::Int(2),
            Key::Bool(true),
            Key::str("a"),
            Key::Int(-1),
            Key::Bool(false),
            Key::Float(-0.5),
            Key::str("ab"),
            Key::Str(vec![0xff]),
        ];
        keys.sort();
        let shown: Vec<String> = keys.iter().map(|k| format!("{k:?}")).collect();
        assert_eq!(
            shown,
            [
                "Bool(false)",
                "Bool(true)",
                "Int(-1)",
                "Float(-0.5)",
                "Float(1.5)",
                "Int(2)",
                "Str([97])",
                "Str([97, 98])",
                "Str([98])",
                "Str([255])",
            ]
        );
    }

    #[test]
    fn int_and_float_compare_exactly() {
        assert_eq!(
            cmp_int_float(9007199254740993, 9007199254740992.0),
            Ordering::Greater
        );
        assert_eq!(cmp_int_float(i64::MAX, 9.3e18), Ordering::Less);
        assert_eq!(cmp_int_float(i64::MIN, -9.3e18), Ordering::Greater);
        assert_eq!(cmp_int_float(3, 3.5), Ordering::Less);
        assert_eq!(cmp_int_float(-3, -3.5), Ordering::Greater);
        assert!(Key::Int(3) < Key::Float(3.0), "int before an equal float");
    }

    #[test]
    fn key_rules() {
        assert_eq!(Key::float(2.0).unwrap_err().code, "codec_key");
        assert_eq!(Key::float(-0.0).unwrap_err().code, "codec_key");
        assert_eq!(Key::float(f64::NAN).unwrap_err().code, "codec_number");
        assert!(Key::float(1e300).is_ok(), "out of i64 range is a float key");
        assert_eq!(
            Key::from_value(Value::Table(Table::new()))
                .unwrap_err()
                .code,
            "codec_key"
        );
        assert_eq!(Key::from_value(Value::Nil).unwrap_err().code, "codec_key");
        let mut t = Table::new();
        assert_eq!(
            t.insert(Key::str("x"), Value::Nil).unwrap_err().code,
            "codec_unsupported"
        );
    }

    #[test]
    fn base64_round_trips_and_is_strict() {
        for (raw, text) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (&[0, 1, 2, 3][..], "AAECAw=="),
            (&[0xff, 0xfe][..], "//4="),
        ] {
            assert_eq!(base64_encode(raw), text);
            assert_eq!(base64_decode(text.as_bytes()).unwrap(), raw);
        }
        for bad in [
            "Zg", "Zg=", "Z===", "Zm8=Zg==", "Zg =", "Zh==", "Zm9=", "Z*==",
        ] {
            assert!(base64_decode(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_refused_enter_leaves_the_depth_alone() {
        let mut b = Budget::default();
        for _ in 0..MAX_DEPTH {
            b.enter().unwrap();
        }
        assert_eq!(b.enter().unwrap_err().code, CodecError::DEPTH);
        assert_eq!(b.depth, MAX_DEPTH, "a refused enter needs no leave");
        b.leave();
        b.enter().unwrap();
    }
}
