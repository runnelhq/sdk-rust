//! Actions, connection types and the rules of the protocol, with the test client.

use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use runnel_plugin::testing::Client;
use runnel_plugin::{Action, Config, ConnectionType, NoConfig, Plugin, TestResult, code};
use serde::Deserialize;
use serde_json::{Value as Json, json};

#[derive(Deserialize, Config, Default)]
#[serde(default)]
struct EchoConfig {
    /// Text to return
    #[runnel(required)]
    text: String,
    /// How often
    times: u32,
    /// Factor
    factor: f64,
    /// Loud
    loud: bool,
    /// Tags
    tags: Vec<String>,
}

#[derive(Deserialize, Config, Default)]
#[serde(default)]
struct Host {
    /// Server
    host: String,
}

/// demo.echo returns its text; "hang" waits until cancelled, "sleep" counts the actions
/// running at the same time, "panic" panics, "fail" fails, "big" returns 17 MiB.
fn demo() -> Plugin {
    let running = Arc::new(AtomicUsize::new(0));
    let mut p = Plugin::new("demo", "1.0.0");
    p.concurrency(2);
    p.action(Action::new("demo.echo", "Echo", "Returns its text", move |call, c: EchoConfig| {
        match c.text.as_str() {
            "hang" => {
                while !call.cancel().sleep(Duration::from_secs(5)) {}
                return Ok(json!("cancelled"));
            }
            "sleep" => {
                let n = running.fetch_add(1, Ordering::SeqCst) + 1;
                std::thread::sleep(Duration::from_millis(100));
                running.fetch_sub(1, Ordering::SeqCst);
                return Ok(json!(n));
            }
            "panic" => panic!("boom"),
            "fail" => return Err("cannot echo".into()),
            "big" => return Ok(json!("x".repeat(17 << 20))),
            "log" => call.log.warn("line one\nline two"),
            _ => {}
        }
        Ok(json!({"text": c.text.repeat(c.times.max(1) as usize), "factor": c.factor, "loud": c.loud, "tags": c.tags}))
    }));
    p.connection_type(
        ConnectionType::new("demo", "Demo", "A demo service").test(|_ctx, c: Host| {
            if c.host == "down" {
                return Err("cannot reach down".into());
            }
            Ok(TestResult {
                ok: true,
                message: format!("connected to {}", c.host),
                version: "demo 1".into(),
            })
        }),
    );
    p
}

#[test]
fn initialize_and_describe() {
    let c = Client::start(&demo());
    assert_eq!(c.initialized.protocol_version, 1);
    assert_eq!(c.initialized.encoding, "json");
    assert_eq!(c.initialized.name, "demo");
    assert_eq!(c.initialized.concurrency, 2);
    let d = c.describe().unwrap();
    assert_eq!(d["actions"][0]["type"], "demo.echo");
    assert_eq!(d["actions"][0]["kind"], "action");
    assert!(d["actions"][0].get("category").is_none(), "category is optional: {d}");
    assert_eq!(d["actions"][0]["schema"]["required"], json!(["text"]));
    assert_eq!(d["connectionTypes"][0]["test"], true);
    assert!(d.get("transforms").is_none());
}

#[test]
fn texts_of_variables_are_converted() {
    let c = Client::start(&demo());
    let out = c
        .execute(
            "demo.echo",
            json!({"text": "ab", "times": " 2", "factor": "1.5", "loud": "true", "tags": ["x"], "unused": null}),
        )
        .unwrap();
    assert_eq!(out, json!({"text": "abab", "factor": 1.5, "loud": true, "tags": ["x"]}));
}

#[test]
fn errors_of_actions() {
    let c = Client::start(&demo());
    let cases = [
        (json!({"text": "fail"}), "cannot echo"),
        (
            json!({"text": "a", "times": "many"}),
            r#"field "times": expected a whole number, got string"#,
        ),
        (
            json!({"text": "a", "times": -1}),
            r#"field "times": expected a whole number not below 0, got number"#,
        ),
        (
            json!({"text": "a", "loud": "yes"}),
            r#"field "loud": expected true or false, got string"#,
        ),
        (
            json!({"text": "a", "tags": "x"}),
            r#"field "tags": expected a list, got string"#,
        ),
        (
            json!({"text": "a", "tags": [1]}),
            r#"field "tags[0]": expected a text, got number"#,
        ),
        (json!({"text": 5}), r#"field "text": expected a text, got number"#),
        (json!("text"), "configuration: expected a mapping, got string"),
        (json!({"text": "big"}), "more than the 16 MiB a message may have"),
    ];
    for (config, want) in cases {
        let err = c.execute("demo.echo", config.clone()).unwrap_err();
        assert_eq!(err.code, code::ACTION_FAILED, "{config}: {err}");
        assert!(err.message.contains(want), "{config}: {err}, want {want:?}");
        assert!(
            !err.message.contains("u32") && !err.message.contains("String"),
            "Rust type in {err}"
        );
    }
}

#[test]
fn panic_keeps_the_plugin_running() {
    let c = Client::start(&demo());
    let err = c.execute("demo.echo", json!({"text": "panic"})).unwrap_err();
    assert_eq!(err.code, code::ACTION_FAILED);
    assert_eq!(err.message, "internal error of the plugin in demo.echo: boom");
    assert!(c.execute("demo.echo", json!({"text": "ok"})).is_ok());
    assert!(
        c.log_contains("ERROR panic in demo.echo: boom at "),
        "log = {:?}",
        c.log()
    );
    let line = c.log().into_iter().find(|l| l.contains("panic in")).unwrap();
    assert!(line.contains(" | "), "backtrace in one line: {line}");
}

#[test]
fn log_lines() {
    let c = Client::start(&demo());
    c.execute("demo.echo", json!({"text": "log"})).unwrap();
    assert!(c.log_contains("WARN line one | line two"), "log = {:?}", c.log());
}

#[test]
fn cancel() {
    let c = Client::start(&demo());
    let pending = c.start_execute("demo.echo", json!({"text": "hang"}));
    std::thread::sleep(Duration::from_millis(50));
    pending.cancel();
    let err = pending.wait().unwrap_err();
    assert_eq!(err.code, code::REQUEST_CANCELLED, "{err}");
}

#[test]
fn concurrency_is_limited() {
    let c = Arc::new(Client::start(&demo()));
    let results = Arc::new(Mutex::new(Vec::new()));
    let threads: Vec<_> = (0..6)
        .map(|_| {
            let (c, results) = (Arc::clone(&c), Arc::clone(&results));
            std::thread::spawn(move || {
                let out = c.execute("demo.echo", json!({"text": "sleep"})).unwrap();
                results.lock().unwrap().push(out);
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let results = results.lock().unwrap();
    assert_eq!(results.len(), 6);
    assert!(
        results.iter().all(|n| n.as_u64().unwrap() <= 2),
        "more than 2 at a time: {results:?}"
    );
    assert!(results.iter().any(|n| n == 2), "never 2 at a time: {results:?}");
}

#[test]
fn connection_test() {
    let c = Client::start(&demo());
    let ok = c
        .test_connection(json!({"name": "srv", "type": "demo", "host": "example.org"}))
        .unwrap();
    assert_eq!(
        ok,
        TestResult {
            ok: true,
            message: "connected to example.org".into(),
            version: "demo 1".into()
        }
    );
    let failed = c
        .test_connection(json!({"name": "srv", "type": "demo", "host": "down"}))
        .unwrap();
    assert_eq!(
        failed,
        TestResult {
            ok: false,
            message: "cannot reach down".into(),
            version: String::new()
        }
    );
    let wrong = c
        .test_connection(json!({"name": "srv", "type": "demo", "host": 5}))
        .unwrap();
    assert!(
        !wrong.ok && wrong.message.contains(r#"field "host": expected a text"#),
        "{wrong:?}"
    );
}

/// Serves the lines and returns the answers.
fn serve_lines(p: &Plugin, input: &str) -> (Vec<Json>, std::io::Result<()>) {
    let out = SharedBuf::default();
    let err = p.serve(Cursor::new(input.as_bytes().to_vec()), out.clone(), std::io::sink());
    let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
    (
        text.lines()
            .map(|l| serde_json::from_str(l).expect("answer is JSON"))
            .collect(),
        err,
    )
}

#[derive(Clone, Default)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn protocol_rules() {
    let input = [
        r#"{"jsonrpc":"2.0","id":1,"method":"describe","params":{}}"#,
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1}}\r",
        "not json",
        r#"{"jsonrpc":"2.0","method":"$/unknown","params":{}}"#,
        "",
        r#"{"jsonrpc":"2.0","id":"x","method":"transform.unknown","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":3,"result":null}"#,
        r#"{"jsonrpc":"2.0"}"#,
        r#"{"jsonrpc":"2.0","id":1e999,"method":"describe","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":4,"method":"shutdown","params":{}}"#,
    ]
    .join("\n");
    let (answers, err) = serve_lines(&demo(), &input);
    err.unwrap();
    assert_eq!(answers.len(), 7, "{answers:?}");
    assert_eq!(answers[0]["error"]["code"], code::NOT_INITIALIZED);
    assert_eq!(answers[1]["result"]["name"], "demo");
    assert_eq!(answers[2]["error"]["code"], code::PARSE_ERROR);
    assert_eq!(answers[2]["id"], Json::Null);
    assert_eq!(answers[3]["error"]["code"], code::METHOD_NOT_FOUND);
    assert_eq!(answers[3]["id"], "x");
    assert_eq!(answers[4]["error"]["code"], code::INVALID_REQUEST);
    assert_eq!(
        answers[5]["error"],
        json!({"code": code::INVALID_REQUEST, "message": "invalid id"})
    );
    assert_eq!(answers[5]["id"], Json::Null);
    assert_eq!(answers[6], json!({"jsonrpc": "2.0", "id": 4, "result": null}));
}

#[test]
fn message_too_long() {
    let (_, err) = serve_lines(&demo(), &"x".repeat(runnel_plugin::MAX_MESSAGE_SIZE + 10));
    let err = err.unwrap_err();
    assert!(err.to_string().contains("message longer than"), "{err}");
    // a message of exactly the limit is read
    let line = format!(
        r#"{{"jsonrpc":"2.0","method":"$/x","params":"{}"}}"#,
        "y".repeat(runnel_plugin::MAX_MESSAGE_SIZE - 50)
    );
    let (_, err) = serve_lines(&demo(), &line[..runnel_plugin::MAX_MESSAGE_SIZE.min(line.len())]);
    err.unwrap();
}

#[test]
fn stdin_ends_while_running() {
    // Runnel ends: requests still running are cancelled and serve returns.
    let input = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"action.execute","params":{"type":"demo.echo","config":{"text":"hang"}}}"#,
    ]
    .join("\n");
    let (answers, err) = serve_lines(&demo(), &input);
    err.unwrap();
    assert_eq!(answers.last().unwrap()["error"]["code"], code::REQUEST_CANCELLED);
}

fn panics(f: impl FnOnce() + std::panic::UnwindSafe) -> String {
    let err = std::panic::catch_unwind(f).expect_err("no panic");
    err.downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default()
}

#[test]
fn definition_mistakes_panic() {
    let echo = || Action::new("demo.echo", "Echo", "Echo", |_c, _: NoConfig| Ok(1));
    type Case = Box<dyn FnOnce() + std::panic::UnwindSafe>;
    let cases: Vec<(&str, Case)> = vec![
        ("invalid plugin name", Box::new(|| drop(Plugin::new("Demo", "1.0.0")))),
        ("needs a version", Box::new(|| drop(Plugin::new("demo", "")))),
        (
            "outside the namespace",
            Box::new(move || {
                Plugin::new("demo", "1").action(Action::new("other.x", "X", "X", |_c, _: NoConfig| Ok(1)));
            }),
        ),
        (
            "invalid action type",
            Box::new(move || {
                Plugin::new("demo", "1").action(Action::new("demo", "X", "X", |_c, _: NoConfig| Ok(1)));
            }),
        ),
        (
            "added twice",
            Box::new(move || {
                Plugin::new("demo", "1").action(echo()).action(echo());
            }),
        ),
        (
            "unknown category",
            Box::new(move || {
                Plugin::new("demo", "1").action(echo().category("input"));
            }),
        ),
        (
            "needs a label",
            Box::new(move || {
                Plugin::new("demo", "1").action(Action::new("demo.x", "", "X", |_c, _: NoConfig| Ok(1)));
            }),
        ),
        (
            "must be \"demo\" or start with",
            Box::new(|| {
                Plugin::new("demo", "1").connection_type(ConnectionType::<NoConfig>::new("other", "O", "O"));
            }),
        ),
        (
            "connection type \"demo\" added twice",
            Box::new(|| {
                let mut p = Plugin::new("demo", "1");
                p.connection_type(ConnectionType::<NoConfig>::new("demo", "D", "D"));
                p.connection_type(ConnectionType::<NoConfig>::new("demo", "D", "D"));
            }),
        ),
        (
            "default: \"x\" is not a whole number",
            Box::new(|| {
                #[derive(Deserialize, Config, Default)]
                #[serde(default)]
                #[allow(dead_code)]
                struct Bad {
                    #[runnel(default = "x")]
                    n: i32,
                }
                Plugin::new("demo", "1").action(Action::new("demo.x", "X", "X", |_c, _: Bad| Ok(1)));
            }),
        ),
    ];
    // The panic hook stays: the SDK's hook belongs to the whole test process.
    for (want, f) in cases {
        let msg = panics(f);
        assert!(msg.contains(want), "panic {msg:?}, want {want:?}");
    }
    // categories of the palette are accepted, the namespace may differ from the name
    let mut p = Plugin::new("demo", "1");
    p.namespace("acme-demo");
    p.action(Action::new("acme-demo.x", "X", "X", |_c, _: NoConfig| Ok(1)).category(runnel_plugin::category::DATABASE));
}
