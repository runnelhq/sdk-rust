//! The plugin of the protocol transcripts (testdata/protocol); the same as in the Go SDK,
//! so that both SDKs answer the same transcripts alike.

#![allow(dead_code)]

use runnel_plugin::{
    Action, Config, ConnectionInfo, ConnectionType, DetectedField, Detection, InitResult, OutputField, Plugin, Problem,
    Reference, TO_ERROR, TO_MAIN, TestResult, Transform, Value, error_fields, error_row, field_index,
};
use serde::Deserialize;

#[derive(Deserialize, Config, Default, Debug)]
#[serde(default)]
pub struct GoldenConn {
    #[serde(flatten)]
    pub info: ConnectionInfo,
    /// Name of the server
    #[runnel(required)]
    pub host: String,
    /// Password
    #[runnel(format = "runnel-secret")]
    pub password: String,
}

#[derive(Deserialize, Config, Default, Debug)]
#[serde(default)]
pub struct GoldenConfig {
    /// Text to return
    #[runnel(required, example = "hello")]
    pub text: String,
    /// How often
    #[runnel(default = "1")]
    pub times: i64,
    /// Connection
    #[runnel(connection = "golden")]
    pub server: GoldenConn,
}

#[derive(Deserialize, Config, Default, Debug)]
#[serde(default)]
pub struct GoldenFields {
    /// Field to write in capitals
    #[runnel(required, format = "runnel-field")]
    pub field: String,
    /// Target for the value route
    #[runnel(format = "runnel-target")]
    pub to: String,
}

pub fn golden() -> Plugin {
    let mut p = Plugin::new("golden", "1.0.0");
    p.action(
        Action::new("golden.echo", "Echo", "Returns its text", |call, c: GoldenConfig| {
            match c.text.as_str() {
                "fail" => return Err("cannot echo".into()),
                "panic" => panic!("boom"),
                _ => {}
            }
            Ok(serde_json::json!({
                "text": c.text.repeat(usize::try_from(c.times).unwrap_or(0)),
                "host": c.server.host,
                "run": call.run_id,
            }))
        })
        .example("type: golden.echo\nwith:\n  text: hello\n")
        .reference(
            Reference::new()
                .output(OutputField::new("output.text", "string", "The text, times times"))
                .output(OutputField::new(
                    "output.host",
                    "string",
                    "Host of the connection server",
                ))
                .errors("Fails when text is fail.")
                .limit("times repeats the text at most 1000 times."),
        ),
    );
    p.transform(
        Transform::new(
            "golden.upper",
            "Upper",
            "Writes a field in capitals",
            |s, c: GoldenFields| {
                while let Some(b) = s.next()? {
                    let at = field_index(&b.fields, &c.field).ok_or("no such field")?;
                    let (mut main, mut routed, mut bad) = (Vec::new(), Vec::new(), Vec::new());
                    for mut row in b.rows {
                        let v = row[at].as_str().unwrap_or_default().to_string();
                        match v.as_str() {
                            "bad" => bad.push(error_row(&row, "invalid value bad", &c.field)),
                            "route" => routed.push(row),
                            _ => {
                                row[at] = Value::String(v.to_uppercase());
                                main.push(row);
                            }
                        }
                    }
                    s.emit(TO_MAIN, &b.fields, &main)?;
                    s.emit(&c.to, &b.fields, &routed)?;
                    s.emit(TO_ERROR, &error_fields(&b.fields), &bad)?;
                }
                Ok(())
            },
        )
        .record_errors()
        .parallel()
        .reference(
            Reference::new()
                .output(OutputField::new("<field>", "string", "The value in capitals"))
                .records("Passes every record to the data links; records with the value route go to the target to.")
                .errors("Records with the value bad go to the error link."),
        )
        .init(|_ctx, input, c: GoldenFields| {
            let mut res = InitResult {
                output: Some(input.input.clone()),
                problems: Vec::new(),
            };
            if !input.input.open && field_index(&input.input.fields, &c.field).is_none() {
                res.problems.push(Problem::error(
                    "field",
                    format!("field {:?} is not in the input", c.field),
                ));
            }
            res
        })
        .detect_fields(|_ctx, _c: GoldenFields, _limit| {
            Ok(Detection {
                rows: 1,
                complete: true,
                fields: vec![DetectedField {
                    name: "name".into(),
                    type_: "string".into(),
                    values: vec!["anna".into()],
                    ..Default::default()
                }],
                ..Default::default()
            })
        }),
    );
    p.connection_type(
        ConnectionType::new("golden", "Golden", "A test service").test(|_ctx, c: GoldenConn| {
            if c.host == "down" {
                return Err("cannot reach down".into());
            }
            Ok(TestResult {
                ok: true,
                message: format!("connected to {}", c.host),
                version: "golden 1".into(),
            })
        }),
    );
    p
}
