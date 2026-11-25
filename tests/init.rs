//! transform.init and transform.detectFields.

use runnel_plugin::testing::Client;
use runnel_plugin::{
    Config, ConnectionInfo, DetectedField, Detection, Fact, Field, InitInput, InitResult, NoConfig, Plugin, Problem,
    RecordSchema, Transform, code, types,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize, Config, Default, Debug)]
#[serde(default)]
struct Conn {
    #[serde(flatten)]
    info: ConnectionInfo,
    /// Server
    host: String,
    /// Password
    #[runnel(format = "runnel-secret")]
    password: String,
}

#[derive(Deserialize, Config, Default, Debug)]
#[serde(default)]
struct ReadConfig {
    /// Server
    #[runnel(connection = "demo")]
    server: Conn,
    /// How many
    #[runnel(default = "10")]
    count: i64,
    /// Field
    field: String,
    /// Target
    #[runnel(format = "runnel-target")]
    rejected: String,
}

fn demo() -> Plugin {
    let mut p = Plugin::new("demo", "1.0.0");
    p.transform(
        Transform::new("demo.read", "Read", "Reads records", |_s, _c: ReadConfig| Ok(()))
            .init(|_ctx, input: &InitInput, c: ReadConfig| {
                let mut res = InitResult::default();
                if c.field.is_empty() {
                    res.problems.push(Problem::error("field", "field is empty"));
                }
                if !c.rejected.is_empty() && !input.targets.contains(&c.rejected) {
                    res.problems.push(Problem::warning(
                        "rejected",
                        format!("no data link leads to {:?}", c.rejected),
                    ));
                }
                let mut fields = input.input.fields.clone();
                fields.push(Field::new("count", types::INT));
                fields.push(Field::new(&format!("conn_{}", c.server.info.name), types::STRING));
                fields.push(Field::new(&format!("n_{}", c.count), types::STRING));
                res.output = Some(RecordSchema {
                    fields,
                    open: input.input.open,
                });
                res
            })
            .detect_fields(|_ctx, c: ReadConfig, limit| {
                if c.server.host == "down" {
                    return Err("cannot reach down".into());
                }
                Ok(Detection {
                    source: vec![Fact {
                        name: "Server".into(),
                        value: c.server.host.clone(),
                    }],
                    rows: limit,
                    complete: false,
                    fields: vec![DetectedField {
                        name: "pw".into(),
                        type_: "string".into(),
                        values: vec![c.server.password],
                        ..Default::default()
                    }],
                })
            }),
    );
    p.transform(Transform::new(
        "demo.plain",
        "Plain",
        "Has neither init nor detect",
        |_s, _c: NoConfig| Ok(()),
    ));
    p
}

#[test]
fn describe_announces_the_methods() {
    let c = Client::start(&demo());
    let d = c.describe().unwrap();
    assert_eq!(d["transforms"][0]["init"], true);
    assert_eq!(d["transforms"][0]["detectFields"], true);
    assert!(d["transforms"][1].get("init").is_none());
    assert!(d["transforms"][1].get("detectFields").is_none());
}

#[test]
fn init_checks_and_tells_the_output() {
    let c = Client::start(&demo());
    let input = InitInput {
        input: RecordSchema {
            fields: vec![Field::new("id", types::INT)],
            open: false,
        },
        targets: vec!["write".into()],
    };
    // Connections come by name, secrets empty, variables unresolved: the count stays at its default.
    let res = c
        .init(
            "demo.read",
            json!({"server": "erp", "count": "${N}", "field": "", "rejected": "nowhere"}),
            &input,
        )
        .unwrap();
    assert_eq!(
        res.output.unwrap().fields,
        [
            Field::new("id", types::INT),
            Field::new("count", types::INT),
            Field::new("conn_erp", types::STRING),
            Field::new("n_0", types::STRING)
        ]
    );
    assert_eq!(
        res.problems,
        [
            Problem::error("field", "field is empty"),
            Problem::warning("rejected", r#"no data link leads to "nowhere""#)
        ]
    );
    let res = c
        .init("demo.read", json!({"count": "3", "field": "x"}), &InitInput::default())
        .unwrap();
    assert!(res.problems.is_empty());
    assert_eq!(res.output.unwrap().fields[2], Field::new("n_3", types::STRING));
    let err = c.init("demo.plain", json!({}), &InitInput::default()).unwrap_err();
    assert_eq!(err.code, code::METHOD_NOT_FOUND);
}

/// Runnel (Go) sends empty lists as null, e.g. for an input without incoming links.
#[test]
fn init_with_null_lists() {
    let c = Client::start(&demo());
    let r = c
        .call(
            "transform.init",
            json!({"type": "demo.read", "config": {"field": "x"}, "input": {"fields": null, "open": null}, "targets": null}),
        )
        .unwrap();
    let res: InitResult = serde_json::from_value(r).unwrap();
    assert!(res.problems.is_empty(), "{res:?}");
    assert_eq!(res.output.unwrap().fields.len(), 3);
    let r = c
        .call(
            "transform.init",
            json!({"type": "demo.read", "config": {"field": "x"}, "input": null}),
        )
        .unwrap();
    assert!(r.get("output").is_some());
}

#[test]
fn init_of_a_configuration_that_is_no_mapping() {
    let c = Client::start(&demo());
    let res = c.init("demo.read", json!("x"), &InitInput::default()).unwrap();
    assert_eq!(res.output, None);
    assert_eq!(res.problems.len(), 1, "{res:?}");
}

#[test]
fn detect_fields() {
    let c = Client::start(&demo());
    let conn = json!({"name": "erp", "type": "demo", "host": "db1", "password": "secret"});
    let d = c.detect_fields("demo.read", json!({"server": conn}), 0).unwrap();
    assert_eq!(d.rows, 1000, "limit 0 means 1000");
    assert_eq!(
        d.source,
        [Fact {
            name: "Server".into(),
            value: "db1".into()
        }]
    );
    assert_eq!(d.fields[0].values, ["secret"], "connections arrive resolved");
    let d = c.detect_fields("demo.read", json!({"server": conn}), 5).unwrap();
    assert_eq!(d.rows, 5);
    let err = c
        .detect_fields("demo.read", json!({"server": {"name": "x", "host": "down"}}), 5)
        .unwrap_err();
    assert_eq!(
        (err.code, err.message.as_str()),
        (code::ACTION_FAILED, "cannot reach down")
    );
    let err = c.detect_fields("demo.plain", json!({}), 5).unwrap_err();
    assert_eq!(err.code, code::METHOD_NOT_FOUND);
}
