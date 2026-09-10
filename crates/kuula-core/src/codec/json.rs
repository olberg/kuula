//! The JSON form for the MCP boundary:
//! plain JSON where it carries the type, `$`-tagged objects where it
//! would not.

use serde_json::{Map, Number, Value as Json};

use super::{base64_decode, base64_encode, check_float, Budget, CodecError, Key, Table, Value};

pub fn to_json(value: &Value) -> Result<Json, CodecError> {
    let mut budget = Budget::default();
    json_of(value, &mut budget)
}

fn json_of(value: &Value, budget: &mut Budget) -> Result<Json, CodecError> {
    Ok(match value {
        Value::Nil => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Int(i) => Json::Number(Number::from(*i)),
        Value::Float(f) => {
            check_float(*f)?;
            Json::Number(Number::from_f64(*f).expect("finite"))
        }
        Value::Str(s) => match std::str::from_utf8(s) {
            Ok(text) => Json::String(text.to_string()),
            Err(_) => tagged("$bytes", Json::String(base64_encode(s))),
        },
        Value::Blob(b) => tagged("$blob", Json::String(base64_encode(b))),
        Value::Table(t) => {
            budget.enter()?;
            let plain = t.iter().all(|(k, _)| {
                matches!(k, Key::Str(s) if std::str::from_utf8(s).is_ok_and(|s| !s.starts_with('$')))
            });
            let out = if plain {
                let mut map = Map::new();
                for (k, v) in t.iter() {
                    budget.entry()?;
                    let Key::Str(s) = k else { unreachable!() };
                    let name = std::str::from_utf8(s).expect("checked").to_string();
                    map.insert(name, json_of(v, budget)?);
                }
                Json::Object(map)
            } else {
                let mut pairs = Vec::with_capacity(t.len());
                for (k, v) in t.iter() {
                    budget.entry()?;
                    let key = json_of(&k.clone().into_value(), budget)?;
                    pairs.push(Json::Array(vec![key, json_of(v, budget)?]));
                }
                tagged("$table", Json::Array(pairs))
            };
            budget.leave();
            out
        }
    })
}

fn tagged(tag: &str, payload: Json) -> Json {
    let mut map = Map::new();
    map.insert(tag.to_string(), payload);
    Json::Object(map)
}

pub fn from_json(json: &Json) -> Result<Value, CodecError> {
    let mut budget = Budget::default();
    value_of(json, &mut budget)
}

fn base64_field(payload: &Json, tag: &str) -> Result<Vec<u8>, CodecError> {
    match payload {
        Json::String(s) => base64_decode(s.as_bytes()),
        _ => Err(CodecError::new(
            CodecError::SYNTAX,
            format!("{tag} must hold a base64 string"),
        )),
    }
}

fn value_of(json: &Json, budget: &mut Budget) -> Result<Value, CodecError> {
    match json {
        Json::Null => {
            budget.value(0)?;
            Ok(Value::Nil)
        }
        Json::Bool(b) => {
            budget.value(0)?;
            Ok(Value::Bool(*b))
        }
        Json::Number(n) => {
            budget.value(0)?;
            if let Some(i) = n.as_i64() {
                Ok(Value::Int(i))
            } else if n.is_u64() {
                Err(CodecError::new(
                    CodecError::NUMBER,
                    format!("integer {n} does not fit 64 bits"),
                ))
            } else {
                let f = n.as_f64().unwrap_or(f64::NAN);
                check_float(f)?;
                Ok(Value::Float(f))
            }
        }
        Json::String(s) => {
            budget.value(s.len())?;
            Ok(Value::Str(s.as_bytes().to_vec()))
        }
        Json::Array(_) => Err(CodecError::new(
            CodecError::SYNTAX,
            "a bare JSON array has no table form; use {\"$table\": [[k, v], ...]}",
        )),
        Json::Object(map) => {
            if map.len() == 1 {
                let (tag, payload) = map.iter().next().expect("one entry");
                match tag.as_str() {
                    "$bytes" => {
                        let b = base64_field(payload, tag)?;
                        budget.value(b.len())?;
                        return Ok(Value::Str(b));
                    }
                    "$blob" => {
                        let b = base64_field(payload, tag)?;
                        budget.value(b.len())?;
                        return Ok(Value::Blob(b));
                    }
                    "$table" => return table_pairs(payload, budget),
                    _ => {}
                }
            }
            budget.enter()?;
            budget.value(0)?;
            let mut table = Table::new();
            for (k, v) in map {
                budget.entry()?;
                budget.value(k.len())?;
                let value = value_of(v, budget)?;
                if let Value::Nil = value {
                    return Err(CodecError::new(
                        CodecError::UNSUPPORTED,
                        format!("null is not a table value (key {k:?})"),
                    ));
                }
                table.insert(Key::Str(k.as_bytes().to_vec()), value)?;
            }
            budget.leave();
            Ok(Value::Table(table))
        }
    }
}

fn table_pairs(payload: &Json, budget: &mut Budget) -> Result<Value, CodecError> {
    let Json::Array(pairs) = payload else {
        return Err(CodecError::new(
            CodecError::SYNTAX,
            "$table must hold an array of [key, value] pairs",
        ));
    };
    budget.enter()?;
    budget.value(0)?;
    let mut table = Table::new();
    for pair in pairs {
        let Json::Array(kv) = pair else {
            return Err(CodecError::new(
                CodecError::SYNTAX,
                "$table entries must be [key, value] arrays",
            ));
        };
        let [k, v] = kv.as_slice() else {
            return Err(CodecError::new(
                CodecError::SYNTAX,
                "$table entries must have exactly a key and a value",
            ));
        };
        budget.entry()?;
        let key = Key::from_value(value_of(k, budget)?)?;
        let value = value_of(v, budget)?;
        if let Value::Nil = value {
            return Err(CodecError::new(
                CodecError::UNSUPPORTED,
                "null is not a table value",
            ));
        }
        if table.insert(key, value)?.is_some() {
            return Err(CodecError::new(CodecError::KEY, "duplicate key in $table"));
        }
    }
    budget.leave();
    Ok(Value::Table(table))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{decode, encode};

    fn round(text: &str, json: &str) {
        let v = decode(text).unwrap();
        let j = to_json(&v).unwrap();
        assert_eq!(serde_json::to_string(&j).unwrap(), json, "{text}");
        let back = from_json(&j).unwrap();
        assert_eq!(encode(&back).unwrap(), encode(&v).unwrap(), "{json}");
    }

    #[test]
    fn plain_shapes() {
        round("nil", "null");
        round("true", "true");
        round("42", "42");
        round("-9223372036854775808", "-9223372036854775808");
        round("1.0", "1.0");
        round("-0.0", "-0.0");
        round("1.5e-7", "1.5e-7");
        round("\"h\\xc3\\xa9\"", "\"h\u{e9}\"");
        round("{}", "{}");
        round(
            "{ a = 1, b = { c = \"x\" } }",
            "{\"a\":1,\"b\":{\"c\":\"x\"}}",
        );
    }

    #[test]
    fn tagged_shapes() {
        round("\"\\xff\"", "{\"$bytes\":\"/w==\"}");
        round("blob\"AAECAw==\"", "{\"$blob\":\"AAECAw==\"}");
        round(
            "{ [1] = \"a\", [2] = \"b\" }",
            "{\"$table\":[[1,\"a\"],[2,\"b\"]]}",
        );
        round(
            "{ [false] = 1, [1.5] = 2, x = 3 }",
            "{\"$table\":[[false,1],[1.5,2],[\"x\",3]]}",
        );
        round("{ [\"$x\"] = 1 }", "{\"$table\":[[\"$x\",1]]}");
        round(
            "{ [\"\\xff\"] = 1 }",
            "{\"$table\":[[{\"$bytes\":\"/w==\"},1]]}",
        );
    }

    #[test]
    fn lenient_input() {
        let j: Json = serde_json::from_str("{\"$x\": 1, \"y\": 2}").unwrap();
        assert_eq!(
            encode(&from_json(&j).unwrap()).unwrap(),
            "{ [\"$x\"] = 1, y = 2 }"
        );
        let j: Json = serde_json::from_str("{\"$blob\": 5}").unwrap();
        assert_eq!(from_json(&j).unwrap_err().code, "codec_syntax");
        let j: Json = serde_json::from_str("[1, 2]").unwrap();
        assert_eq!(from_json(&j).unwrap_err().code, "codec_syntax");
        let j: Json = serde_json::from_str("18446744073709551615").unwrap();
        assert_eq!(from_json(&j).unwrap_err().code, "codec_number");
        let j: Json = serde_json::from_str("{\"a\": null}").unwrap();
        assert_eq!(from_json(&j).unwrap_err().code, "codec_unsupported");
        let j: Json = serde_json::from_str("{\"$table\": [[1, 1], [1, 2]]}").unwrap();
        assert_eq!(from_json(&j).unwrap_err().code, "codec_key");
        let j: Json = serde_json::from_str("{\"$table\": [[null, 1]]}").unwrap();
        assert_eq!(from_json(&j).unwrap_err().code, "codec_key");
    }

    #[test]
    fn depth_is_bounded_on_the_way_in() {
        let text = "{\"a\":".repeat(40) + "1" + &"}".repeat(40);
        let j: Json = serde_json::from_str(&text).unwrap();
        assert_eq!(from_json(&j).unwrap_err().code, "codec_depth");
    }
}
