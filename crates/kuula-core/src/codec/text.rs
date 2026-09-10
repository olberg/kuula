//! The Lua-like text form: a canonical
//! encoder and a hand-written, bounded parser.

use super::{
    base64_decode, base64_encode, check_float, Budget, CodecError, Key, Table, Value, MAX_ENCODED,
};

const KEYWORDS: [&str; 22] = [
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// Whether a string key is written bare.
fn is_name(s: &[u8]) -> bool {
    let Some(&first) = s.first() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    if !s.iter().all(|&c| c.is_ascii_alphanumeric() || c == b'_') {
        return false;
    }
    let s = std::str::from_utf8(s).expect("ASCII");
    !KEYWORDS.contains(&s)
}

// ----- encoding ---------------------------------------------------------

/// Encode a value canonically. A `Value::Float` that is not finite and a
/// table deeper or larger than the limits are errors.
pub fn encode(value: &Value) -> Result<String, CodecError> {
    let mut out = String::new();
    let mut budget = Budget::default();
    write_value(&mut out, value, &mut budget)?;
    if out.len() > MAX_ENCODED {
        return Err(CodecError::encoded());
    }
    Ok(out)
}

fn write_value(out: &mut String, value: &Value, budget: &mut Budget) -> Result<(), CodecError> {
    match value {
        Value::Nil => out.push_str("nil"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(i) => out.push_str(&i.to_string()),
        Value::Float(f) => {
            check_float(*f)?;
            write_float(out, *f);
        }
        Value::Str(s) => write_string(out, s),
        Value::Blob(b) => {
            out.push_str("blob\"");
            out.push_str(&base64_encode(b));
            out.push('"');
        }
        Value::Table(t) => write_table(out, t, budget)?,
    }
    if out.len() > MAX_ENCODED {
        return Err(CodecError::encoded());
    }
    Ok(())
}

fn write_table(out: &mut String, table: &Table, budget: &mut Budget) -> Result<(), CodecError> {
    budget.enter()?;
    if table.is_empty() {
        out.push_str("{}");
        budget.leave();
        return Ok(());
    }
    out.push_str("{ ");
    let mut first = true;
    for (key, value) in table.iter() {
        budget.entry()?;
        if !first {
            out.push_str(", ");
        }
        first = false;
        match key {
            Key::Str(s) if is_name(s) => {
                out.push_str(std::str::from_utf8(s).expect("ASCII"));
            }
            other => {
                out.push('[');
                write_value(out, &other.clone().into_value(), budget)?;
                out.push(']');
            }
        }
        out.push_str(" = ");
        if let Value::Nil = value {
            return Err(CodecError::new(
                CodecError::UNSUPPORTED,
                "nil is not a table value",
            ));
        }
        write_value(out, value, budget)?;
    }
    out.push_str(" }");
    budget.leave();
    Ok(())
}

/// Shortest round-trip digits, positional for exponents -5 to 16 and
/// scientific otherwise; always a `.` or an `e`.
pub fn write_float(out: &mut String, f: f64) {
    let sci = format!("{f:e}");
    let (mantissa, exp) = sci.split_once('e').expect("{:e} has an exponent");
    let exp: i32 = exp.parse().expect("exponent is an integer");
    let (neg, mantissa) = match mantissa.strip_prefix('-') {
        Some(m) => (true, m),
        None => (false, mantissa),
    };
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    if neg {
        out.push('-');
    }
    if (-5..17).contains(&exp) {
        if exp >= 0 {
            let split = exp as usize + 1;
            if digits.len() > split {
                out.push_str(&digits[..split]);
                out.push('.');
                out.push_str(&digits[split..]);
            } else {
                out.push_str(&digits);
                for _ in digits.len()..split {
                    out.push('0');
                }
                out.push_str(".0");
            }
        } else {
            out.push_str("0.");
            for _ in 0..(-exp - 1) {
                out.push('0');
            }
            out.push_str(&digits);
        }
    } else {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push_str(&exp.to_string());
    }
}

fn write_string(out: &mut String, s: &[u8]) {
    out.push('"');
    for &c in s {
        match c {
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            b'\\' => out.push_str("\\\\"),
            b'"' => out.push_str("\\\""),
            0x20..=0x7e => out.push(c as char),
            _ => out.push_str(&format!("\\x{c:02x}")),
        }
    }
    out.push('"');
}

// ----- decoding ---------------------------------------------------------

/// Parse text in the canonical grammar, within the
/// limits of section 3.
pub fn decode(text: &str) -> Result<Value, CodecError> {
    decode_bytes(text.as_bytes())
}

/// The same for raw bytes, which is what a save slot holds.
pub fn decode_bytes(src: &[u8]) -> Result<Value, CodecError> {
    if src.len() > MAX_ENCODED {
        return Err(CodecError::encoded());
    }
    let mut p = Parser {
        src,
        pos: 0,
        budget: Budget::default(),
    };
    p.skip_ws();
    let value = p.value()?;
    p.skip_ws();
    if p.pos != p.src.len() {
        return Err(p.err("trailing characters after the value"));
    }
    Ok(value)
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
    budget: Budget,
}

impl<'a> Parser<'a> {
    fn err(&self, what: &str) -> CodecError {
        CodecError::new(CodecError::SYNTAX, format!("{what} at byte {}", self.pos))
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, c: u8) -> Result<(), CodecError> {
        if self.peek() == Some(c) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.err(&format!("expected '{}'", c as char)))
        }
    }

    fn word(&mut self) -> &'a [u8] {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || c == b'_') {
            self.pos += 1;
        }
        &self.src[start..self.pos]
    }

    fn value(&mut self) -> Result<Value, CodecError> {
        match self.peek() {
            None => Err(self.err("unexpected end of input")),
            Some(b'{') => self.table(),
            Some(b'"') => {
                let s = self.string()?;
                self.budget.value(s.len())?;
                Ok(Value::Str(s))
            }
            Some(b'-' | b'0'..=b'9') => {
                let v = self.number()?;
                self.budget.value(0)?;
                Ok(v)
            }
            Some(c) if c.is_ascii_alphabetic() => {
                let w = self.word();
                match w {
                    b"nil" => {
                        self.budget.value(0)?;
                        Ok(Value::Nil)
                    }
                    b"true" => {
                        self.budget.value(0)?;
                        Ok(Value::Bool(true))
                    }
                    b"false" => {
                        self.budget.value(0)?;
                        Ok(Value::Bool(false))
                    }
                    b"blob" => {
                        if self.peek() != Some(b'"') {
                            return Err(self.err("expected '\"' after blob"));
                        }
                        let text = self.string()?;
                        if text.contains(&b'\\') {
                            return Err(self.err("escapes are not allowed in base64"));
                        }
                        let bytes = base64_decode(&text)?;
                        self.budget.value(bytes.len())?;
                        Ok(Value::Blob(bytes))
                    }
                    _ => Err(self.err("unknown word")),
                }
            }
            Some(_) => Err(self.err("unexpected character")),
        }
    }

    fn table(&mut self) -> Result<Value, CodecError> {
        self.expect(b'{')?;
        self.budget.enter()?;
        self.budget.value(0)?;
        let mut table = Table::new();
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                None => return Err(self.err("unterminated table")),
                _ => {}
            }
            let key = self.key()?;
            self.skip_ws();
            self.expect(b'=')?;
            self.skip_ws();
            let value = self.value()?;
            if let Value::Nil = value {
                return Err(self.err("nil is not a table value"));
            }
            self.budget.entry()?;
            if table.insert(key, value)?.is_some() {
                return Err(CodecError::new(
                    CodecError::KEY,
                    format!("duplicate key at byte {}", self.pos),
                ));
            }
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {}
                _ => return Err(self.err("expected ',' or '}'")),
            }
        }
        self.budget.leave();
        Ok(Value::Table(table))
    }

    fn key(&mut self) -> Result<Key, CodecError> {
        if self.peek() == Some(b'[') {
            self.pos += 1;
            self.skip_ws();
            let v = self.value()?;
            self.skip_ws();
            self.expect(b']')?;
            return Key::from_value(v);
        }
        let start = self.pos;
        let w = self.word();
        if w.is_empty() {
            return Err(self.err("expected a key"));
        }
        if !is_name(w) {
            self.pos = start;
            return Err(self.err("bare key is not a name"));
        }
        self.budget.value(w.len())?;
        Ok(Key::Str(w.to_vec()))
    }

    fn number(&mut self) -> Result<Value, CodecError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        let digits = |p: &mut Parser| {
            let s = p.pos;
            while matches!(p.peek(), Some(b'0'..=b'9')) {
                p.pos += 1;
            }
            p.pos > s
        };
        if !digits(self) {
            return Err(self.err("expected digits"));
        }
        let mut is_float = false;
        if self.peek() == Some(b'.') {
            self.pos += 1;
            is_float = true;
            if !digits(self) {
                return Err(self.err("expected digits after '.'"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            is_float = true;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !digits(self) {
                return Err(self.err("expected digits in exponent"));
            }
        }
        let text = std::str::from_utf8(&self.src[start..self.pos]).expect("ASCII");
        if is_float {
            let f: f64 = text
                .parse()
                .map_err(|_| CodecError::new(CodecError::NUMBER, format!("bad float {text}")))?;
            check_float(f)?;
            Ok(Value::Float(f))
        } else {
            let i: i64 = text.parse().map_err(|_| {
                CodecError::new(
                    CodecError::NUMBER,
                    format!("integer {text} does not fit 64 bits"),
                )
            })?;
            Ok(Value::Int(i))
        }
    }

    fn string(&mut self) -> Result<Vec<u8>, CodecError> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        loop {
            let Some(c) = self.peek() else {
                return Err(self.err("unterminated string"));
            };
            self.pos += 1;
            match c {
                b'"' => return Ok(out),
                b'\\' => {
                    let Some(e) = self.peek() else {
                        return Err(self.err("unterminated escape"));
                    };
                    self.pos += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'\\' => out.push(b'\\'),
                        b'"' => out.push(b'"'),
                        b'x' => {
                            let hex = self
                                .src
                                .get(self.pos..self.pos + 2)
                                .and_then(|h| std::str::from_utf8(h).ok())
                                .and_then(|h| u8::from_str_radix(h, 16).ok());
                            let Some(b) = hex else {
                                return Err(self.err("bad \\x escape"));
                            };
                            self.pos += 2;
                            out.push(b);
                        }
                        _ => return Err(self.err("unknown escape")),
                    }
                }
                0x00..=0x1f | 0x7f => return Err(self.err("control character in string")),
                _ => out.push(c),
            }
            if out.len() > MAX_ENCODED {
                return Err(CodecError::encoded());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::MAX_DEPTH;

    fn t(entries: Vec<(Key, Value)>) -> Value {
        let mut table = Table::new();
        for (k, v) in entries {
            table.insert(k, v).unwrap();
        }
        Value::Table(table)
    }

    /// The fixture table, pinned byte for byte.
    #[test]
    fn fixture_encodes_to_pinned_bytes() {
        let v = t(vec![
            (Key::str("x"), Value::Float(1.5)),
            (Key::Int(1), Value::Int(2)),
            (Key::str("y"), Value::Bool(true)),
            (Key::str("b"), Value::Blob(vec![0, 1, 2, 3])),
            (Key::str("k"), Value::str("v")),
            (Key::Bool(false), Value::Int(0)),
            (Key::str("end"), Value::str("kw")),
            (Key::str("a b"), Value::str("sp")),
            (Key::Float(2.5), Value::str("f")),
            (Key::str("s"), Value::Str(b"q\"\\\n\t\x01\xff~".to_vec())),
            (Key::str("neg"), Value::Int(i64::MIN)),
            (Key::str("z"), Value::Float(-0.0)),
            (Key::str("e"), t(vec![])),
            (
                Key::str("n"),
                t(vec![(Key::Int(1), t(vec![(Key::str("d"), Value::Int(3))]))]),
            ),
        ]);
        let text = encode(&v).unwrap();
        assert_eq!(
            text,
            "{ [false] = 0, [1] = 2, [2.5] = \"f\", [\"a b\"] = \"sp\", b = blob\"AAECAw==\", \
             e = {}, [\"end\"] = \"kw\", k = \"v\", n = { [1] = { d = 3 } }, \
             neg = -9223372036854775808, s = \"q\\\"\\\\\\n\\t\\x01\\xff~\", x = 1.5, \
             y = true, z = -0.0 }"
        );
        let back = decode(&text).unwrap();
        assert_eq!(encode(&back).unwrap(), text);
        let Value::Table(bt) = &back else { panic!() };
        assert!(matches!(bt.get(&Key::str("z")), Some(Value::Float(f)) if f.is_sign_negative()));
        assert_eq!(bt.get(&Key::Int(1)), Some(&Value::Int(2)));
        assert_eq!(bt.get(&Key::str("neg")), Some(&Value::Int(i64::MIN)));
    }

    #[test]
    fn float_formatting_rule() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (1.5, "1.5"),
            (100.0, "100.0"),
            (123456.789, "123456.789"),
            (0.1, "0.1"),
            (0.00001, "0.00001"),
            (0.000001, "1e-6"),
            (1.5e-7, "1.5e-7"),
            (1e16, "10000000000000000.0"),
            (1e17, "1e17"),
            (-2.5e300, "-2.5e300"),
            (f64::MAX, "1.7976931348623157e308"),
            (f64::MIN_POSITIVE, "2.2250738585072014e-308"),
            (5e-324, "5e-324"),
            (1.0 / 3.0, "0.3333333333333333"),
            (9007199254740993.0, "9007199254740992.0"),
        ];
        for (f, want) in cases {
            let mut s = String::new();
            write_float(&mut s, *f);
            assert_eq!(&s, want, "{f:?}");
            let back = decode(&s).unwrap();
            assert!(
                matches!(back, Value::Float(g) if g.to_bits() == f.to_bits()),
                "{s}"
            );
        }
    }

    #[test]
    fn top_level_scalars() {
        for (text, canon) in [
            ("nil", "nil"),
            (" true ", "true"),
            ("false", "false"),
            ("42", "42"),
            ("-7", "-7"),
            ("1.50", "1.5"),
            ("1e2", "100.0"),
            ("1E+2", "100.0"),
            ("\"a\\x41\\\\X41\"", "\"aA\\\\X41\""),
            ("\"\"", "\"\""),
            ("blob\"\"", "blob\"\""),
            ("{x=1,}", "{ x = 1 }"),
            ("{\n  x = 1,\n  y = 2\n}", "{ x = 1, y = 2 }"),
        ] {
            let v = decode(text).unwrap();
            assert_eq!(encode(&v).unwrap(), canon, "{text}");
        }
    }

    #[test]
    fn syntax_errors() {
        for bad in [
            "",
            "{",
            "{ x = }",
            "{ x = 1 y = 2 }",
            "{ 1, 2 }",
            "\"abc",
            "\"a\\q\"",
            "\"a\\x4\"",
            "\"a\nb\"",
            "blob\"Zg\"",
            "blob \"Zg==\"",
            "1 2",
            "'a'",
            "{ x = nil }",
            "{ [nil] = 1 }",
            "{ end = 1 }",
            "-",
            "1.",
            "1e",
            "0x10",
            "--comment\n1",
            "nul",
        ] {
            let e = decode(bad).unwrap_err();
            assert!(
                e.code == "codec_syntax" || (bad == "{ [nil] = 1 }" && e.code == "codec_key"),
                "{bad:?} -> {e}"
            );
        }
    }

    #[test]
    fn number_errors() {
        assert_eq!(
            decode("9223372036854775808").unwrap_err().code,
            "codec_number"
        );
        assert_eq!(
            decode("-9223372036854775809").unwrap_err().code,
            "codec_number"
        );
        assert_eq!(decode("1e999").unwrap_err().code, "codec_number");
        assert_eq!(
            encode(&Value::Float(f64::NAN)).unwrap_err().code,
            "codec_number"
        );
        assert_eq!(
            encode(&Value::Float(f64::INFINITY)).unwrap_err().code,
            "codec_number"
        );
    }

    #[test]
    fn key_errors() {
        assert_eq!(decode("{ x = 1, x = 2 }").unwrap_err().code, "codec_key");
        assert_eq!(
            decode("{ [1] = 1, [1] = 2 }").unwrap_err().code,
            "codec_key"
        );
        assert_eq!(decode("{ [1.0] = 1 }").unwrap_err().code, "codec_key");
        assert_eq!(decode("{ [{}] = 1 }").unwrap_err().code, "codec_key");
        assert_eq!(decode("{ [blob\"\"] = 1 }").unwrap_err().code, "codec_key");
        assert_eq!(
            decode("{ x = 1, [\"x\"] = 2 }").unwrap_err().code,
            "codec_key"
        );
    }

    #[test]
    fn unsupported_nil_in_table() {
        let mut table = Table::new();
        assert_eq!(
            table.insert(Key::Int(1), Value::Nil).unwrap_err().code,
            "codec_unsupported"
        );
    }

    #[test]
    fn depth_limit() {
        let ok = "{ a = ".repeat(MAX_DEPTH) + "1" + &" }".repeat(MAX_DEPTH);
        assert!(decode(&ok).is_ok());
        let deep = "{ a = ".repeat(MAX_DEPTH + 1) + "1" + &" }".repeat(MAX_DEPTH + 1);
        assert_eq!(decode(&deep).unwrap_err().code, "codec_depth");
        let mut v = Value::Int(1);
        for _ in 0..MAX_DEPTH + 1 {
            v = t(vec![(Key::str("a"), v)]);
        }
        assert_eq!(encode(&v).unwrap_err().code, "codec_depth");
    }

    #[test]
    fn size_limits() {
        // Entries: 65 537 one-byte entries fit the encoded limit? No:
        // "[n] = 1, " is 9+ bytes, so build the value in memory instead.
        let mut table = Table::new();
        for i in 0..(crate::codec::MAX_ENTRIES as i64 + 1) {
            table.insert(Key::Int(i), Value::Bool(true)).unwrap();
        }
        assert_eq!(encode(&Value::Table(table)).unwrap_err().code, "codec_size");
        // Encoded: a string just over the limit.
        let big = Value::Str(vec![b'a'; MAX_ENCODED]);
        assert_eq!(encode(&big).unwrap_err().code, "codec_size");
        let fits = Value::Str(vec![b'a'; MAX_ENCODED - 2]);
        assert!(encode(&fits).is_ok());
        let text = "\"".to_string() + &"a".repeat(MAX_ENCODED) + "\"";
        assert_eq!(decode(&text).unwrap_err().code, "codec_size");
    }
}
