//! JSON Schema of configurations, generated from Rust types with `#[derive(Config)]`, the
//! same way Runnel derives the schemas of its built-in actions (ADR 0014 of Runnel).

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};
use serde_json::Value as Json;

/// The JSON Schema dialect of all generated schemas.
pub const DRAFT: &str = "https://json-schema.org/draft/2020-12/schema";

/// A time span such as `30s`, `5m` or `1h30m`.
pub const FORMAT_DURATION: &str = "runnel-duration";
/// A text of several lines.
pub const FORMAT_TEXT: &str = "runnel-text";
/// A password or token; only such fields may use `${secret:…}`.
pub const FORMAT_SECRET: &str = "runnel-secret";
/// The name of a connection; set with the option `connection = "<type>|…"`.
pub const FORMAT_CONNECTION: &str = "runnel-connection";
/// The name of a field of the incoming records.
pub const FORMAT_FIELD: &str = "runnel-field";
/// The id of a transform that a data link leads to (a target of `Stream::emit`).
pub const FORMAT_TARGET: &str = "runnel-target";

/// How deeply types may nest; deeper means a recursive type.
const MAX_DEPTH: usize = 32;

/// A JSON Schema, limited to what configurations of actions, transforms and connection
/// types need. Properties keep the order of the struct fields, which is the order the
/// editor shows them in.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConfigSchema {
    pub schema: Option<String>,
    pub type_: Option<String>,
    pub description: Option<String>,
    pub format: Option<String>,
    pub enumeration: Vec<Json>,
    pub default: Option<Json>,
    pub examples: Vec<Json>,
    pub minimum: Option<i64>,
    pub items: Option<Box<ConfigSchema>>,
    pub properties: Vec<(String, ConfigSchema)>,
    pub required: Vec<String>,
    /// The field applies only when the sibling field has one of the values.
    pub only: BTreeMap<String, Vec<Json>>,
    /// The connection types a field of format runnel-connection accepts.
    pub connection: Vec<String>,
    /// The section of a form the field is shown in.
    pub group: Option<String>,
    /// `Some(None)`: no other properties (structs); `Some(Some(s))`: values of a map.
    pub additional_properties: Option<Option<Box<ConfigSchema>>>,
}

impl ConfigSchema {
    /// A schema of a JSON type: string, integer, number, boolean, array or object.
    pub fn of_type(t: &str) -> ConfigSchema {
        ConfigSchema {
            type_: Some(t.to_string()),
            ..Default::default()
        }
    }

    /// The schema of a property.
    pub fn property(&self, name: &str) -> Option<&ConfigSchema> {
        self.properties.iter().find(|(n, _)| n == name).map(|(_, s)| s)
    }

    fn is_type(&self, t: &str) -> bool {
        self.type_.as_deref() == Some(t)
    }

    /// The schema of the values of a map, if this is one.
    pub fn map_values(&self) -> Option<&ConfigSchema> {
        match &self.additional_properties {
            Some(Some(s)) => Some(s),
            _ => None,
        }
    }
}

impl Serialize for ConfigSchema {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        struct Props<'a>(&'a [(String, ConfigSchema)]);
        impl Serialize for Props<'_> {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut m = serializer.serialize_map(Some(self.0.len()))?;
                for (k, v) in self.0 {
                    m.serialize_entry(k, v)?;
                }
                m.end()
            }
        }
        let mut m = serializer.serialize_map(None)?;
        if let Some(s) = &self.schema {
            m.serialize_entry("$schema", s)?;
        }
        if let Some(t) = &self.type_ {
            m.serialize_entry("type", t)?;
        }
        if let Some(d) = self.description.as_ref().filter(|d| !d.is_empty()) {
            m.serialize_entry("description", d)?;
        }
        if let Some(f) = &self.format {
            m.serialize_entry("format", f)?;
        }
        if !self.enumeration.is_empty() {
            m.serialize_entry("enum", &self.enumeration)?;
        }
        if let Some(d) = &self.default {
            m.serialize_entry("default", d)?;
        }
        if !self.examples.is_empty() {
            m.serialize_entry("examples", &self.examples)?;
        }
        if let Some(min) = self.minimum {
            m.serialize_entry("minimum", &min)?;
        }
        if let Some(i) = &self.items {
            m.serialize_entry("items", i)?;
        }
        if !self.properties.is_empty() {
            m.serialize_entry("properties", &Props(&self.properties))?;
        }
        if !self.required.is_empty() {
            m.serialize_entry("required", &self.required)?;
        }
        if !self.only.is_empty() {
            m.serialize_entry("x-runnel-only", &self.only)?;
        }
        if !self.connection.is_empty() {
            m.serialize_entry("x-runnel-connection", &self.connection)?;
        }
        if let Some(g) = &self.group {
            m.serialize_entry("x-runnel-group", g)?;
        }
        match &self.additional_properties {
            Some(None) => m.serialize_entry("additionalProperties", &false)?,
            Some(Some(s)) => m.serialize_entry("additionalProperties", s)?,
            None => {}
        }
        m.end()
    }
}

/// A mistake in the definition of a configuration type, e.g. a default that does not fit
/// the type of the field. Adding an action or transform with such a type panics.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaError(pub String);

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SchemaError {}

/// Rust types that have a JSON Schema: texts, numbers, booleans, `Vec`, `Option` (the
/// inner type), maps with text keys, `serde_json::Value` (any value) and structs with
/// `#[derive(Config)]`.
pub trait SchemaType {
    /// The schema of the type; `depth` guards against recursive types.
    fn schema_at(depth: usize) -> Result<ConfigSchema, SchemaError>;
}

macro_rules! simple {
    ($t:expr, $($ty:ty),*) => {$(
        impl SchemaType for $ty {
            fn schema_at(_: usize) -> Result<ConfigSchema, SchemaError> {
                Ok(ConfigSchema::of_type($t))
            }
        }
    )*};
}

simple!("string", String, &'static str, char);
simple!("boolean", bool);
simple!("integer", i8, i16, i32, i64, i128, isize);
simple!("number", f32, f64);

macro_rules! unsigned {
    ($($ty:ty),*) => {$(
        impl SchemaType for $ty {
            fn schema_at(_: usize) -> Result<ConfigSchema, SchemaError> {
                Ok(ConfigSchema { minimum: Some(0), ..ConfigSchema::of_type("integer") })
            }
        }
    )*};
}

unsigned!(u8, u16, u32, u64, u128, usize);

impl SchemaType for Json {
    fn schema_at(_: usize) -> Result<ConfigSchema, SchemaError> {
        Ok(ConfigSchema::default()) // any value
    }
}

impl<T: SchemaType> SchemaType for Option<T> {
    fn schema_at(depth: usize) -> Result<ConfigSchema, SchemaError> {
        T::schema_at(depth)
    }
}

impl<T: SchemaType> SchemaType for Box<T> {
    fn schema_at(depth: usize) -> Result<ConfigSchema, SchemaError> {
        T::schema_at(depth)
    }
}

impl<T: SchemaType> SchemaType for Vec<T> {
    fn schema_at(depth: usize) -> Result<ConfigSchema, SchemaError> {
        let items = T::schema_at(depth + 1)?;
        Ok(ConfigSchema {
            items: Some(Box::new(items)),
            ..ConfigSchema::of_type("array")
        })
    }
}

impl<T: SchemaType, S> SchemaType for HashMap<String, T, S> {
    fn schema_at(depth: usize) -> Result<ConfigSchema, SchemaError> {
        let values = T::schema_at(depth + 1)?;
        Ok(ConfigSchema {
            additional_properties: Some(Some(Box::new(values))),
            ..ConfigSchema::of_type("object")
        })
    }
}

impl<T: SchemaType> SchemaType for BTreeMap<String, T> {
    fn schema_at(depth: usize) -> Result<ConfigSchema, SchemaError> {
        HashMap::<String, T>::schema_at(depth)
    }
}

/// The options of a field, from its `#[runnel(…)]` attribute and doc comment. Used by the
/// code that `#[derive(Config)]` generates.
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct FieldOptions {
    pub doc: &'static str,
    pub required: bool,
    pub default: Option<&'static str>,
    pub example: Option<&'static str>,
    pub enumeration: Option<&'static str>,
    pub format: Option<&'static str>,
    pub group: Option<&'static str>,
    pub only: Option<&'static str>,
    pub connection: Option<&'static str>,
}

/// Builds the schema of a struct field by field. Used by the code that `#[derive(Config)]`
/// generates.
#[doc(hidden)]
pub struct ObjectBuilder {
    depth: usize,
    schema: ConfigSchema,
}

impl ObjectBuilder {
    pub fn new(depth: usize) -> Result<ObjectBuilder, SchemaError> {
        if depth > MAX_DEPTH {
            return Err(SchemaError("recursive type".into()));
        }
        let schema = ConfigSchema {
            additional_properties: Some(None),
            ..ConfigSchema::of_type("object")
        };
        Ok(ObjectBuilder { depth, schema })
    }

    /// The depth for the schemas of the fields.
    pub fn depth(&self) -> usize {
        self.depth + 1
    }

    pub fn field(&mut self, name: &str, mut s: ConfigSchema, o: FieldOptions) -> Result<(), SchemaError> {
        let at = |msg: String| SchemaError(format!("field {name:?}: {msg}"));
        if self.schema.property(name).is_some() {
            return Err(at("field name used twice".into()));
        }
        s.description = Some(o.doc.to_string());
        s.group = o.group.map(str::to_string);
        if let Some(only) = o.only {
            let Some((field, values)) = only.split_once(':').filter(|(f, v)| !f.is_empty() && !v.is_empty()) else {
                return Err(at("only must be <field>:<value>|<value>…".into()));
            };
            s.only.insert(
                field.to_string(),
                values.split('|').map(|v| Json::String(v.to_string())).collect(),
            );
        }
        let mut format = o.format.map(str::to_string);
        if let Some(conn) = o.connection {
            if format.as_deref().is_some_and(|f| f != FORMAT_CONNECTION) {
                return Err(at("connection does not go with another format".into()));
            }
            s.connection = conn.split('|').map(str::to_string).collect();
            format = Some(FORMAT_CONNECTION.to_string());
        }
        if let Some(f) = format {
            if s.is_type("string") {
                s.format = Some(f);
            } else if s.is_type("array") && s.items.as_ref().is_some_and(|i| i.is_type("string")) {
                s.items.as_mut().expect("items").format = Some(f);
            } else if s.is_type("object") && s.map_values().is_some_and(|v| v.is_type("string")) {
                s.format = Some(f.clone());
                if let Some(Some(v)) = &mut s.additional_properties {
                    v.format = Some(f);
                }
            } else {
                return Err(at(
                    "format is only allowed for texts, lists of texts and maps of texts".into()
                ));
            }
        }
        let t = s.type_.clone().unwrap_or_default();
        if let Some(d) = o.default {
            s.default = Some(parse_value(&t, d).map_err(|e| at(format!("default: {e}")))?);
        }
        if let Some(e) = o.example {
            s.examples = vec![parse_value(&t, e).map_err(|e| at(format!("example: {e}")))?];
        }
        if let Some(values) = o.enumeration {
            for v in values.split('|') {
                s.enumeration
                    .push(parse_value(&t, v).map_err(|e| at(format!("values: {e}")))?);
            }
        }
        if o.required {
            self.schema.required.push(name.to_string());
        }
        self.schema.properties.push((name.to_string(), s));
        Ok(())
    }

    /// Adds the fields of a flattened struct (`#[serde(flatten)]`).
    pub fn flatten(&mut self, inner: ConfigSchema) -> Result<(), SchemaError> {
        for (name, s) in inner.properties {
            if self.schema.property(&name).is_some() {
                return Err(SchemaError(format!("field {name:?}: field name used twice")));
            }
            self.schema.properties.push((name, s));
        }
        self.schema.required.extend(inner.required);
        Ok(())
    }

    pub fn finish(self) -> Result<ConfigSchema, SchemaError> {
        for (name, p) in &self.schema.properties {
            for sibling in p.only.keys() {
                if sibling == name || self.schema.property(sibling).is_none() {
                    return Err(SchemaError(format!(
                        "field {name:?}: only names {sibling:?}, which is not another field"
                    )));
                }
            }
        }
        Ok(self.schema)
    }
}

/// Converts a value of an attribute to the JSON type of the field.
fn parse_value(t: &str, text: &str) -> Result<Json, String> {
    match t {
        "string" => Ok(Json::String(text.to_string())),
        "boolean" => text
            .parse::<bool>()
            .map(Json::Bool)
            .map_err(|_| format!("{text:?} is not true or false")),
        "integer" => text
            .parse::<i64>()
            .map(Json::from)
            .map_err(|_| format!("{text:?} is not a whole number")),
        "number" => text
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Json::Number)
            .ok_or_else(|| format!("{text:?} is not a number")),
        _ => Err(format!(
            "values are only supported for texts, numbers and booleans, not {t:?}"
        )),
    }
}
