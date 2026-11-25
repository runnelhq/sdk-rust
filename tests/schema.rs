//! Schemas from Rust types: the same JSON as the Go SDK and Runnel produce (ADR 0014).

use std::collections::HashMap;

use runnel_plugin::schema::SchemaType;
use runnel_plugin::{Config, ConnectionInfo};
use serde::Deserialize;

#[derive(Deserialize, Config, Default)]
#[serde(default)]
struct Embedded {
    base: String,
}

#[derive(Deserialize, Config, Default)]
#[serde(default)]
struct Inner {
    #[runnel(default = "389")]
    port: u32,
}

#[derive(Deserialize, Config, Default)]
#[serde(default)]
#[allow(dead_code)]
struct Full {
    #[serde(flatten)]
    embedded: Embedded,
    /// Mode
    #[runnel(values = "a|b", default = "a")]
    mode: String,
    #[runnel(only = "mode:b", group = "Advanced")]
    only: String,
    #[runnel(format = "runnel-field")]
    fields: Vec<String>,
    #[runnel(format = "runnel-secret")]
    env: HashMap<String, String>,
    #[runnel(format = "runnel-duration", example = "30s")]
    wait: String,
    #[runnel(example = "0.5")]
    ratio: f64,
    inner: Option<Inner>,
    any: serde_json::Value,
    #[runnel(skip)]
    skipped: String,
    #[serde(skip)]
    hidden: String,
}

#[test]
fn schema_like_go() {
    let got = serde_json::to_string(&Full::schema().unwrap()).unwrap();
    let want = r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"#
        .to_string()
        + r#""base":{"type":"string"},"#
        + r#""mode":{"type":"string","description":"Mode","enum":["a","b"],"default":"a"},"#
        + r#""only":{"type":"string","x-runnel-only":{"mode":["b"]},"x-runnel-group":"Advanced"},"#
        + r#""fields":{"type":"array","items":{"type":"string","format":"runnel-field"}},"#
        + r#""env":{"type":"object","format":"runnel-secret","additionalProperties":{"type":"string","format":"runnel-secret"}},"#
        + r#""wait":{"type":"string","format":"runnel-duration","examples":["30s"]},"#
        + r#""ratio":{"type":"number","examples":[0.5]},"#
        + r#""inner":{"type":"object","properties":{"port":{"type":"integer","default":389,"minimum":0}},"additionalProperties":false},"#
        + r#""any":{}},"additionalProperties":false}"#;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&got).unwrap(),
        serde_json::from_str::<serde_json::Value>(&want).unwrap(),
        "\n got {got}\nwant {want}"
    );
    let _ = Full::default().skipped;
    let _ = Full::default().hidden;
}

#[derive(Deserialize, Config, Default)]
#[serde(default, rename_all = "camelCase")]
#[allow(dead_code)]
struct Renamed {
    size_limit: u32,
    #[serde(rename = "type")]
    kind: String,
    #[runnel(doc = "From the attribute")]
    r#async: bool,
    /// First line
    /// and second line.
    two_lines: String,
}

#[test]
fn names_and_descriptions() {
    let s = Renamed::schema().unwrap();
    let names: Vec<&str> = s.properties.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["sizeLimit", "type", "async", "twoLines"]);
    assert_eq!(
        s.property("async").unwrap().description.as_deref(),
        Some("From the attribute")
    );
    assert_eq!(
        s.property("twoLines").unwrap().description.as_deref(),
        Some("First line and second line.")
    );
}

#[test]
fn value_types() {
    #[derive(Deserialize, Config, Default)]
    #[serde(default)]
    struct Values {
        #[runnel(default = "true")]
        on: bool,
        #[runnel(default = "1.5", values = "1.5|2.5")]
        ratio: f64,
        #[runnel(example = "3")]
        count: i32,
    }
    let s = Values::schema().unwrap();
    assert_eq!(s.property("on").unwrap().default, Some(serde_json::json!(true)));
    assert_eq!(
        s.property("ratio").unwrap().enumeration,
        [serde_json::json!(1.5), serde_json::json!(2.5)]
    );
    assert_eq!(s.property("count").unwrap().examples, [serde_json::json!(3)]);
}

macro_rules! schema_error {
    ($name:ident, $want:expr, { $($body:tt)* }) => {
        #[test]
        fn $name() {
            #[derive(Deserialize, Config, Default)]
            #[serde(default)]
            #[allow(dead_code)]
            struct C { $($body)* }
            let err = C::schema().unwrap_err().to_string();
            assert!(err.contains($want), "error {err:?}, want {:?}", $want);
        }
    };
}

schema_error!(bad_default, r#"field "x": default: "abc" is not a whole number"#, { #[runnel(default = "abc")] x: i64, });
schema_error!(bad_bool, "is not true or false", { #[runnel(default = "maybe")] x: bool, });
schema_error!(bad_enum, "is not a number", { #[runnel(values = "a|b")] x: f64, });
schema_error!(default_on_list, "values are only supported", { #[runnel(default = "1")] x: Vec<i64>, });
schema_error!(format_on_int, "format is only allowed", { #[runnel(format = "runnel-text")] x: i64, });
schema_error!(only_without_sibling, r#"only names "y""#, { #[runnel(only = "y:a")] x: String, });
schema_error!(only_without_values, "only must be", { #[runnel(only = "y")] x: String, });
schema_error!(connection_and_format, "connection does not go with another format", { #[runnel(connection = "a", format = "runnel-text")] x: String, });
schema_error!(used_twice, "field name used twice", { #[serde(flatten)] e: Embedded, #[serde(rename = "base")] b: String, });

#[test]
fn recursive_type() {
    #[derive(Deserialize, Config, Default)]
    #[serde(default)]
    struct Node {
        children: Vec<Node>,
    }
    let err = Node::schema().unwrap_err().to_string();
    assert!(err.contains("recursive type"), "{err}");
}

#[test]
fn connection_field() {
    #[derive(Deserialize, Config, Default)]
    #[serde(default)]
    struct Conn {
        #[serde(flatten)]
        info: ConnectionInfo,
        /// Address
        #[runnel(required)]
        url: String,
    }
    #[derive(Deserialize, Config, Default)]
    #[serde(default)]
    struct Search {
        /// The server
        #[runnel(required, connection = "ldap|ldap-ad")]
        server: Conn,
    }
    let got = serde_json::to_value(Search::schema().unwrap()).unwrap();
    assert_eq!(
        got["properties"]["server"],
        serde_json::json!({"type": "string", "description": "The server", "format": "runnel-connection", "x-runnel-connection": ["ldap", "ldap-ad"]})
    );
    let conn = serde_json::to_value(Conn::schema().unwrap()).unwrap();
    assert_eq!(
        conn["properties"].as_object().unwrap().keys().collect::<Vec<_>>(),
        ["url"]
    );
    let _ = Search::default().server.info;
}

#[test]
fn builtin_types() {
    let s = serde_json::to_value(<Vec<HashMap<String, u8>>>::schema_at(0).unwrap()).unwrap();
    assert_eq!(
        s,
        serde_json::json!({"type": "array", "items": {"type": "object", "additionalProperties": {"type": "integer", "minimum": 0}}})
    );
    let unit = serde_json::to_value(<runnel_plugin::NoConfig as Config>::schema().unwrap()).unwrap();
    assert_eq!(
        unit,
        serde_json::json!({"$schema": runnel_plugin::schema::DRAFT, "type": "object", "additionalProperties": false})
    );
}
