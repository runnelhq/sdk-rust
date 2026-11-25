//! Decoding of configurations: texts in number and boolean fields are converted (resolved
//! variables arrive as text), values that do not fit get messages without Rust type names.

use serde::de::DeserializeOwned;
use serde_json::{Map, Value as Json};

use crate::Error;
use crate::schema::{ConfigSchema, FORMAT_CONNECTION};

/// Decodes the configuration Runnel sent into the type of the action, transform or
/// connection type.
pub(crate) fn decode<C: DeserializeOwned>(
    raw: Option<&serde_json::value::RawValue>,
    s: &ConfigSchema,
) -> Result<C, Error> {
    let mut v = parse(raw)?;
    coerce(s, &mut v);
    check(s, &v, "")?;
    serde_json::from_value(v).map_err(|e| format!("invalid configuration: {}", strip_position(&e.to_string())).into())
}

/// Decodes the configuration for transform.init, where values may still be `${…}`: fields
/// whose values do not fit are left out, so that they get their default.
pub(crate) fn decode_lenient<C: DeserializeOwned>(
    raw: Option<&serde_json::value::RawValue>,
    s: &ConfigSchema,
) -> Result<C, Error> {
    let mut v = parse(raw)?;
    coerce(s, &mut v);
    if let Json::Object(m) = &mut v {
        m.retain(|k, item| s.property(k).is_none_or(|ps| check(ps, item, k).is_ok()));
    }
    serde_json::from_value(v).map_err(|e| format!("invalid configuration: {}", strip_position(&e.to_string())).into())
}

fn parse(raw: Option<&serde_json::value::RawValue>) -> Result<Json, Error> {
    let text = raw.map(|r| r.get()).unwrap_or("null");
    let v: Json = serde_json::from_str(text).map_err(|e| format!("invalid configuration: {e}"))?;
    Ok(if v.is_null() { Json::Object(Map::new()) } else { v })
}

/// Converts texts in fields of type integer, number or boolean: after Runnel resolved
/// `${…}` and `{{ … }}`, a number from a variable may arrive as text. A connection that
/// arrives as name only (transform.init) becomes `{"name": …}`. Fields with null are
/// removed, so that they get their default.
pub(crate) fn coerce(s: &ConfigSchema, v: &mut Json) {
    match v {
        Json::String(text) => {
            if s.format.as_deref() == Some(FORMAT_CONNECTION) {
                let mut m = Map::new();
                m.insert("name".into(), Json::String(std::mem::take(text)));
                *v = Json::Object(m);
                return;
            }
            let t = text.trim();
            let new = match s.type_.as_deref() {
                Some("integer") => t.parse::<i64>().ok().map(Json::from),
                Some("number") => t
                    .parse::<f64>()
                    .ok()
                    .and_then(serde_json::Number::from_f64)
                    .map(Json::Number),
                Some("boolean") => parse_bool(t).map(Json::Bool),
                _ => None,
            };
            if let Some(n) = new {
                *v = n;
            }
        }
        Json::Object(m) => {
            if s.format.as_deref() == Some(FORMAT_CONNECTION) {
                return; // a resolved connection: its fields belong to the connection type
            }
            m.retain(|_, item| !item.is_null());
            for (k, item) in m.iter_mut() {
                if let Some(ps) = s.property(k) {
                    coerce(ps, item);
                } else if let Some(vs) = s.map_values() {
                    coerce(vs, item);
                }
            }
        }
        Json::Array(items) => {
            if let Some(is) = &s.items {
                for item in items {
                    coerce(is, item);
                }
            }
        }
        _ => {}
    }
}

/// Like strconv.ParseBool of Go, which Runnel uses for the same purpose.
fn parse_bool(t: &str) -> Option<bool> {
    match t {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Some(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Some(false),
        _ => None,
    }
}

/// Checks the JSON types of the values against the schema, so that a mistake gets a
/// message such as `field "times": expected a whole number, got string`.
fn check(s: &ConfigSchema, v: &Json, path: &str) -> Result<(), Error> {
    let fail = |expected: &str| -> Result<(), Error> {
        let where_ = if path.is_empty() {
            "configuration".to_string()
        } else {
            format!("field {path:?}")
        };
        Err(format!("{where_}: expected {expected}, got {}", kind(v)).into())
    };
    if s.format.as_deref() == Some(FORMAT_CONNECTION) {
        return if v.is_object() { Ok(()) } else { fail("a connection") };
    }
    match s.type_.as_deref() {
        Some("string") if !v.is_string() => fail("a text"),
        Some("boolean") if !v.is_boolean() => fail("true or false"),
        Some("integer") => match v.as_number() {
            Some(n) if n.is_i64() || n.is_u64() => {
                if s.minimum.is_some_and(|min| n.as_i64().is_some_and(|i| i < min)) {
                    return fail("a whole number not below 0");
                }
                Ok(())
            }
            _ => fail("a whole number"),
        },
        Some("number") if !v.is_number() => fail("a number"),
        Some("array") => match v {
            Json::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    if let Some(is) = &s.items {
                        check(is, item, &format!("{path}[{i}]"))?;
                    }
                }
                Ok(())
            }
            _ => fail("a list"),
        },
        Some("object") => match v {
            Json::Object(m) => {
                for (k, item) in m {
                    let sub = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    if let Some(ps) = s.property(k) {
                        check(ps, item, &sub)?;
                    } else if let Some(vs) = s.map_values() {
                        check(vs, item, &sub)?;
                    }
                }
                Ok(())
            }
            _ => fail("a mapping"),
        },
        _ => Ok(()),
    }
}

fn kind(v: &Json) -> &'static str {
    match v {
        Json::Null => "null",
        Json::Bool(_) => "bool",
        Json::Number(_) => "number",
        Json::String(_) => "string",
        Json::Array(_) => "array",
        Json::Object(_) => "object",
    }
}

/// serde_json adds "at line 1 column 5" to errors; for a configuration that means nothing.
fn strip_position(msg: &str) -> String {
    match msg.rfind(" at line ") {
        Some(i) => msg[..i].to_string(),
        None => msg.to_string(),
    }
}
