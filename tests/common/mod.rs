//! The plugin of the protocol transcripts (testdata/protocol); the same as in the Go SDK,
//! so that both SDKs answer the same transcripts alike.

#![allow(dead_code)]

use runnel_plugin::{Action, Config, ConnectionInfo, ConnectionType, Plugin, TestResult};
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
        .example("type: golden.echo\nwith:\n  text: hello\n"),
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
