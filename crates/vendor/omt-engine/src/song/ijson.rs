//! The payload as I-JSON (docs/omt.md section 1): strict parsing into a `Value`.

use std::fmt;

use serde_json::{Map, Value};

/// The deepest nesting a payload may have (section 1): the root object is level 1, and every
/// nested object or array adds one.
pub const MAX_DEPTH: usize = 32;
const MAX_INTEGER: f64 = 9007199254740991.0; // 2^53 - 1

/// Parses an I-JSON text (RFC 7493) by the container's rules for its manifest (section 1): member
/// names unique within an object, no noncharacters, every number within ±(2^53 − 1) once rounded
/// to binary64, and at most `MAX_DEPTH` levels. A number whose value is an integer is one, however
/// it is spelled: `44100.0` and `4.41e4` read as 44100, as the canonical form writes them. serde_json
/// already refuses invalid UTF-8, lone surrogates and numbers beyond binary64.
pub(crate) fn parse_ijson(text: &[u8]) -> Option<Value> {
    use serde::de::DeserializeSeed;
    let mut de = serde_json::Deserializer::from_slice(text);
    let value = Strict { level: 1 }.deserialize(&mut de).ok()?;
    de.end().ok()?;
    Some(value)
}

/// A value at nesting `level`, read into a `Value` while the I-JSON rules are checked.
struct Strict {
    level: usize,
}

fn noncharacter(s: &str) -> bool {
    s.chars().any(|c| matches!(c as u32, 0xFDD0..=0xFDEF) || c as u32 & 0xFFFE == 0xFFFE)
}

impl<'de> serde::de::DeserializeSeed<'de> for Strict {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> serde::de::Visitor<'de> for Strict {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an I-JSON value")
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_bool<E>(self, b: bool) -> Result<Value, E> {
        Ok(Value::Bool(b))
    }

    fn visit_i64<E: serde::de::Error>(self, n: i64) -> Result<Value, E> {
        if n.unsigned_abs() > MAX_INTEGER as u64 {
            return Err(E::custom("an integer beyond 2^53 - 1"));
        }
        Ok(Value::from(n))
    }

    fn visit_u64<E: serde::de::Error>(self, n: u64) -> Result<Value, E> {
        if n > MAX_INTEGER as u64 {
            return Err(E::custom("an integer beyond 2^53 - 1"));
        }
        Ok(Value::from(n as i64))
    }

    fn visit_f64<E: serde::de::Error>(self, x: f64) -> Result<Value, E> {
        if !x.is_finite() || x.abs() > MAX_INTEGER {
            return Err(E::custom("a number beyond 2^53 - 1"));
        }
        if x.fract() == 0.0 {
            return Ok(Value::from(x as i64)); // -0 is 0
        }
        Ok(serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number))
    }

    fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Value, E> {
        if noncharacter(s) {
            return Err(E::custom("a noncharacter"));
        }
        Ok(Value::String(s.to_string()))
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        if self.level > MAX_DEPTH {
            return Err(serde::de::Error::custom("nested too deep"));
        }
        let mut out = Vec::new();
        while let Some(v) = seq.next_element_seed(Strict { level: self.level + 1 })? {
            out.push(v);
        }
        Ok(Value::Array(out))
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        use serde::de::Error;
        if self.level > MAX_DEPTH {
            return Err(A::Error::custom("nested too deep"));
        }
        let mut out = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if noncharacter(&key) {
                return Err(A::Error::custom("a noncharacter"));
            }
            let value = map.next_value_seed(Strict { level: self.level + 1 })?;
            if out.insert(key, value).is_some() {
                return Err(A::Error::custom("a member name twice"));
            }
        }
        Ok(Value::Object(out))
    }
}
