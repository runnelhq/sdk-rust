//! Values and fields of records (ADR 0007 of Runnel) as they go over the protocol.

use serde::ser::{Serialize, Serializer};
use serde::{Deserialize, Serialize as SerializeDerive};
use serde_json::Value as Json;

use crate::Error;

/// Field types of records.
pub mod types {
    pub const STRING: &str = "string";
    pub const INT: &str = "int";
    pub const FLOAT: &str = "float";
    pub const DECIMAL: &str = "decimal";
    pub const BOOL: &str = "bool";
    pub const DATE: &str = "date";
    pub const DATETIME: &str = "datetime";
    pub const BINARY: &str = "binary";
    pub const JSON: &str = "json";
}

/// A field of records: name and type (one of [`types`], empty for any JSON value).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, SerializeDerive)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type", default, skip_serializing_if = "String::is_empty")]
    pub type_: String,
}

impl Field {
    /// A field with name and type, e.g. `Field::new("id", types::INT)`.
    pub fn new(name: &str, type_: &str) -> Field {
        Field {
            name: name.to_string(),
            type_: type_.to_string(),
        }
    }
}

/// The position of a field by name.
pub fn field_index(fields: &[Field], name: &str) -> Option<usize> {
    fields.iter().position(|f| f.name == name)
}

/// A value of a record, by the type of its field: `int` as `Int`, `float` as `Float`,
/// `decimal` as exact text (`"1234.56"`), `date` as text (`"2026-11-24"`), `datetime` as
/// RFC 3339 text with offset, `binary` as bytes (Base64 on the wire), `json` as any JSON
/// value; `Null` in every type. Values of fields without type are `Bool`, `Int`, `Float`,
/// `String` or `Json`.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Decimal(String),
    String(String),
    Date(String),
    Datetime(String),
    Binary(Vec<u8>),
    Json(Json),
}

/// A record: values in the order of its fields.
pub type Row = Vec<Value>;

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// The text of String, Decimal, Date and Datetime.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) | Value::Decimal(s) | Value::Date(s) | Value::Datetime(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(n) => Some(*n as f64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The value as JSON, as it goes over the wire.
    pub fn to_json(&self) -> Json {
        serde_json::to_value(self).unwrap_or(Json::Null)
    }

    /// Reads a value of the wire by the type of its field. A value that does not fit the
    /// type is an error.
    pub fn from_json(v: Json, type_: &str) -> Result<Value, String> {
        let mismatch = |v: &Json| format!("{} is not a value of type {type_}", short(v));
        Ok(match (type_, v) {
            (_, Json::Null) => Value::Null,
            (types::STRING, Json::String(s)) => Value::String(s),
            (types::INT, Json::Number(n)) => match n.as_i64() {
                Some(i) => Value::Int(i),
                None => return Err(mismatch(&Json::Number(n))),
            },
            (types::FLOAT, Json::Number(n)) => Value::Float(n.as_f64().unwrap_or(f64::NAN)),
            (types::DECIMAL, Json::String(s)) => Value::Decimal(s),
            (types::DECIMAL, Json::Number(n)) => Value::Decimal(n.to_string()),
            (types::BOOL, Json::Bool(b)) => Value::Bool(b),
            (types::DATE, Json::String(s)) if is_date(&s) => Value::Date(s),
            (types::DATETIME, Json::String(s)) if is_datetime(&s) => Value::Datetime(s),
            (types::BINARY, Json::String(s)) => match base64_decode(&s) {
                Some(b) => Value::Binary(b),
                None => return Err("the value is not Base64".into()),
            },
            (types::JSON, v) => Value::Json(v),
            ("", Json::Bool(b)) => Value::Bool(b),
            ("", Json::Number(n)) => match n.as_i64() {
                Some(i) => Value::Int(i),
                None => Value::Float(n.as_f64().unwrap_or(f64::NAN)),
            },
            ("", Json::String(s)) => Value::String(s),
            ("", v) => Value::Json(v),
            (_, v) => return Err(mismatch(&v)),
        })
    }
}

fn short(v: &Json) -> String {
    let s = v.to_string();
    if s.chars().count() > 40 {
        format!("{}…", s.chars().take(40).collect::<String>())
    } else {
        s
    }
}

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Value::Null => s.serialize_unit(),
            Value::Bool(b) => s.serialize_bool(*b),
            Value::Int(n) => s.serialize_i64(*n),
            Value::Float(f) if f.is_finite() => s.serialize_f64(*f),
            Value::Float(_) => s.serialize_unit(), // JSON has no NaN or infinity
            Value::Decimal(t) | Value::String(t) | Value::Date(t) | Value::Datetime(t) => s.serialize_str(t),
            Value::Binary(b) => s.serialize_str(&base64_encode(b)),
            Value::Json(j) => j.serialize(s),
        }
    }
}

macro_rules! from {
    ($($t:ty => $v:ident $(as $c:ty)?),*) => {$(
        impl From<$t> for Value {
            fn from(x: $t) -> Value {
                Value::$v(x $(as $c)?)
            }
        }
    )*};
}

from!(bool => Bool, i64 => Int, i32 => Int as i64, u32 => Int as i64, f64 => Float, String => String, Vec<u8> => Binary, Json => Json);

impl From<&str> for Value {
    fn from(s: &str) -> Value {
        Value::String(s.to_string())
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(o: Option<T>) -> Value {
        o.map(Into::into).unwrap_or(Value::Null)
    }
}

/// `2026-11-24`
pub(crate) fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && [0, 1, 2, 3, 5, 6, 8, 9].iter().all(|&i| b[i].is_ascii_digit())
        && (1..=12).contains(&s[5..7].parse::<u8>().unwrap_or(0))
        && (1..=31).contains(&s[8..10].parse::<u8>().unwrap_or(0))
}

/// RFC 3339 with offset, e.g. `2026-11-24T10:30:00+01:00` or `…00.123Z`.
pub(crate) fn is_datetime(s: &str) -> bool {
    if s.len() < 20 || !s.is_char_boundary(10) || !is_date(&s[..10]) {
        return false;
    }
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| r.into_iter().all(|i| b.get(i).is_some_and(u8::is_ascii_digit));
    if !matches!(b[10], b'T' | b't' | b' ')
        || !digits(11..13)
        || b[13] != b':'
        || !digits(14..16)
        || b[16] != b':'
        || !digits(17..19)
    {
        return false;
    }
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    match &s[i..] {
        "Z" | "z" => true,
        off => {
            off.len() == 6
                && matches!(&off[..1], "+" | "-")
                && off.as_bytes()[3] == b':'
                && digits(i + 1..i + 3)
                && digits(i + 4..i + 6)
        }
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub(crate) fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if !b.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 4 * 3);
    for (ci, chunk) in b.chunks(4).enumerate() {
        let last = ci == b.len() / 4 - 1;
        let mut n = 0u32;
        let mut pad = 0;
        for (i, &c) in chunk.iter().enumerate() {
            let v = match c {
                b'A'..=b'Z' => c - b'A',
                b'a'..=b'z' => c - b'a' + 26,
                b'0'..=b'9' => c - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' if last && i >= 2 => {
                    pad += 1;
                    0
                }
                _ => return None,
            };
            if pad > 0 && c != b'=' {
                return None;
            }
            n = (n << 6) | u32::from(v);
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    Some(out)
}

/// The fields of records on the error link: the fields and `_error`, `_errorField`
/// (Runnel adds `_errorTransform`).
pub fn error_fields(fields: &[Field]) -> Vec<Field> {
    let mut f = fields.to_vec();
    f.push(Field::new("_error", types::STRING));
    f.push(Field::new("_errorField", types::STRING));
    f
}

/// A copy of row with the message and the name of the field at fault, for
/// [`error_fields`].
pub fn error_row(row: &[Value], message: &str, field: &str) -> Row {
    let mut r = row.to_vec();
    r.push(Value::String(message.to_string()));
    r.push(Value::String(field.to_string()));
    r
}

pub(crate) fn decode_row(raw: Vec<Json>, fields: &[Field]) -> Result<Row, Error> {
    raw.into_iter()
        .enumerate()
        .map(|(i, v)| {
            let f = fields.get(i);
            Value::from_json(v, f.map(|f| f.type_.as_str()).unwrap_or("")).map_err(|e| match f {
                Some(f) => format!("field {:?}: {e}", f.name).into(),
                None => e.into(),
            })
        })
        .collect()
}
