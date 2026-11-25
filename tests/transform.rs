//! Transforms as streams with the test client.

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use runnel_plugin::testing::Client;
use runnel_plugin::{
    Batch, Config, Field, NoConfig, Plugin, STREAM_WINDOW, TO_ERROR, TO_MAIN, Transform, Value, error_fields,
    error_row, field_index, transform_category, types,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize, Config, Default)]
#[serde(default)]
struct UpperConfig {
    /// Field
    #[runnel(required, format = "runnel-field")]
    field: String,
}

#[derive(Deserialize, Config, Default)]
#[serde(default)]
struct CountConfig {
    /// Rows
    rows: u32,
    /// Size of the text
    size: usize,
}

fn demo() -> Plugin {
    let mut p = Plugin::new("demo", "1.0.0");
    p.transform(
        Transform::new(
            "demo.upper",
            "Upper",
            "Writes a field in capitals",
            |s, c: UpperConfig| {
                while let Some(b) = s.next()? {
                    let at = field_index(&b.fields, &c.field)
                        .ok_or_else(|| format!("field {:?} is not in the records", c.field))?;
                    let (mut good, mut bad) = (Vec::new(), Vec::new());
                    for mut row in b.rows {
                        match row[at].as_str().map(str::to_uppercase) {
                            Some(v) if !v.is_empty() => {
                                row[at] = Value::String(v);
                                good.push(row);
                            }
                            _ => bad.push(error_row(&row, "empty", &c.field)),
                        }
                    }
                    s.emit(TO_MAIN, &b.fields, &good)?;
                    s.emit(TO_ERROR, &error_fields(&b.fields), &bad)?;
                }
                Ok(())
            },
        )
        .record_errors()
        .parallel(),
    );
    p.transform(
        Transform::new("demo.count", "Count", "Emits numbered records", |s, c: CountConfig| {
            let fields = [Field::new("n", types::INT), Field::new("text", types::STRING)];
            let rows: Vec<_> = (0..c.rows)
                .map(|i| vec![Value::from(i), Value::String("x".repeat(c.size))])
                .collect();
            for chunk in rows.chunks(100) {
                s.emit(TO_MAIN, &fields, chunk)?;
            }
            Ok(())
        })
        .category(transform_category::INPUT),
    );
    p.transform(Transform::new(
        "demo.types",
        "Types",
        "Returns every value with its kind",
        |s, _: NoConfig| {
            while let Some(b) = s.next()? {
                let mut fields = b.fields.clone();
                fields.push(Field::new("kinds", types::STRING));
                let rows: Vec<_> = b
                    .rows
                    .into_iter()
                    .map(|mut r| {
                        let kinds: Vec<String> = r
                            .iter()
                            .map(|v| format!("{v:?}").split('(').next().unwrap_or_default().to_string())
                            .collect();
                        r.push(Value::String(kinds.join(",")));
                        r
                    })
                    .collect();
                s.emit(TO_MAIN, &fields, &rows)?;
            }
            Ok(())
        },
    ));
    p.transform(Transform::new(
        "demo.fail",
        "Fail",
        "Fails or panics",
        |s, c: UpperConfig| {
            if c.field == "panic" {
                panic!("boom");
            }
            let _ = s.next()?;
            Err("cannot go on".into())
        },
    ));
    p.transform(Transform::new(
        "demo.stall",
        "Stall",
        "Reads nothing until cancelled",
        |s, _: NoConfig| {
            while !s.cancel().sleep(Duration::from_secs(5)) {}
            Err("must not be reported, the cause is".into())
        },
    ));
    p
}

fn batch(names: &[&str]) -> Batch {
    Batch {
        fields: vec![Field::new("id", types::INT), Field::new("name", types::STRING)],
        rows: names
            .iter()
            .enumerate()
            .map(|(i, n)| vec![Value::from(i as i64), Value::from(*n)])
            .collect(),
    }
}

#[test]
fn describe() {
    let c = Client::start(&demo());
    let d = c.describe().unwrap();
    let t = &d["transforms"][0];
    assert_eq!(t["type"], "demo.upper");
    assert_eq!(t["kind"], "transform");
    assert_eq!(t["recordErrors"], true);
    assert_eq!(t["parallel"], true);
    assert!(t.get("category").is_none());
    assert_eq!(d["transforms"][1]["category"], "input");
}

#[test]
fn stream_with_error_link() {
    let c = Client::start(&demo());
    let batches: Vec<Batch> = (0..20).map(|_| batch(&["anna", "", "ben"])).collect();
    let res = c
        .run_transform("demo.upper", json!({"field": "name"}), &batches)
        .unwrap();
    assert_eq!(res.error, None);
    let main = res.records(TO_MAIN);
    assert_eq!(main.len(), 40);
    assert_eq!(main[0], json!({"id": 0, "name": "ANNA"}).as_object().unwrap().clone());
    let errors = res.records(TO_ERROR);
    assert_eq!(errors.len(), 20);
    assert_eq!(
        errors[0],
        json!({"id": 1, "name": "", "_error": "empty", "_errorField": "name"})
            .as_object()
            .unwrap()
            .clone()
    );
}

#[test]
fn input_splits_large_emits() {
    let c = Client::start(&demo());
    let res = c
        .run_transform("demo.count", json!({"rows": 1000, "size": 10}), &[])
        .unwrap();
    assert_eq!(res.error, None);
    assert_eq!(res.records(TO_MAIN).len(), 1000);
    assert_eq!(res.emits.len(), 10);
    // records of 1 MiB do not fit 100 into one message: emit splits them
    let res = c
        .run_transform("demo.count", json!({"rows": 40, "size": 1 << 20}), &[])
        .unwrap();
    assert_eq!(res.error, None);
    assert_eq!(res.records(TO_MAIN).len(), 40);
    assert!(res.emits.len() >= 3, "{} emits", res.emits.len());
    // a single record over 16 MiB is an error of the transform
    let res = c
        .run_transform("demo.count", json!({"rows": 1, "size": 17 << 20}), &[])
        .unwrap();
    assert!(res.error.unwrap().contains("more than the 16 MiB a message may have"));
}

#[test]
fn value_types() {
    let c = Client::start(&demo());
    let fields = vec![
        Field::new("s", types::STRING),
        Field::new("i", types::INT),
        Field::new("f", types::FLOAT),
        Field::new("d", types::DECIMAL),
        Field::new("b", types::BOOL),
        Field::new("day", types::DATE),
        Field::new("at", types::DATETIME),
        Field::new("bin", types::BINARY),
        Field::new("j", types::JSON),
        Field::new("any", ""),
    ];
    let row = vec![
        Value::from("a"),
        Value::Int(9_007_199_254_740_993),
        Value::Float(1.5),
        Value::Decimal("1234.56".into()),
        Value::Bool(true),
        Value::Date("2026-11-24".into()),
        Value::Datetime("2026-11-24T10:30:00.5+01:00".into()),
        Value::Binary(vec![0, 255, 7]),
        Value::Json(json!({"k": [1]})),
        Value::Float(2.5),
    ];
    let res = c
        .run_transform(
            "demo.types",
            json!({}),
            &[Batch {
                fields: fields.clone(),
                rows: vec![row, vec![Value::Null; 10]],
            }],
        )
        .unwrap();
    assert_eq!(res.error, None);
    let r = &res.records(TO_MAIN);
    assert_eq!(
        r[0]["kinds"],
        "String,Int,Float,Decimal,Bool,Date,Datetime,Binary,Json,Float"
    );
    assert_eq!(r[0]["i"], json!(9_007_199_254_740_993_i64));
    assert_eq!(r[0]["bin"], "AP8H");
    assert_eq!(r[0]["at"], "2026-11-24T10:30:00.5+01:00");
    assert_eq!(r[1]["kinds"], "Null,Null,Null,Null,Null,Null,Null,Null,Null,Null");
}

#[test]
fn wrong_values_end_the_stream() {
    let c = Client::start(&demo());
    for (t, v, want) in [
        (
            types::INT,
            json!("x"),
            r#"row 1 of a batch: field "v": "x" is not a value of type int"#,
        ),
        (types::INT, json!(1.5), "1.5 is not a value of type int"),
        (types::DATE, json!("24.11.2026"), "is not a value of type date"),
        (
            types::DATETIME,
            json!("2026-11-24 10:30"),
            "is not a value of type datetime",
        ),
        (types::BINARY, json!("!!"), "the value is not Base64"),
        (types::BOOL, json!(1), "1 is not a value of type bool"),
    ] {
        let line = json!({"stream": 0, "fields": [{"name": "v", "type": t}], "rows": [[v]]});
        let fields = serde_json::from_value(line["fields"].clone()).unwrap();
        let rows = vec![vec![Value::Json(line["rows"][0][0].clone())]];
        let res = c
            .run_transform("demo.types", json!({}), &[Batch { fields, rows }])
            .unwrap();
        let err = res.error.unwrap_or_default();
        assert!(err.contains(want), "{t} {v}: {err:?}, want {want:?}");
    }
}

#[test]
fn errors_and_panics_close_the_stream() {
    let c = Client::start(&demo());
    let res = c
        .run_transform("demo.fail", json!({"field": "x"}), &[batch(&["a"])])
        .unwrap();
    assert_eq!(res.error.as_deref(), Some("cannot go on"));
    let res = c.run_transform("demo.fail", json!({"field": "panic"}), &[]).unwrap();
    assert_eq!(
        res.error.as_deref(),
        Some("internal error of the plugin in demo.fail: boom")
    );
    let res = c
        .run_transform("demo.upper", json!({"field": "nope"}), &[batch(&["a"])])
        .unwrap();
    assert_eq!(res.error.as_deref(), Some(r#"field "nope" is not in the records"#));
    let res = c.run_transform("demo.upper", json!({"field": 5}), &[]).unwrap();
    assert_eq!(
        res.error.as_deref(),
        Some(r#"field "field": expected a text, got number"#)
    );
    let err = c.run_transform("demo.none", json!({}), &[]).unwrap_err();
    assert_eq!(err.code, runnel_plugin::code::INVALID_PARAMS);
    // the plugin goes on
    assert!(
        c.run_transform("demo.upper", json!({"field": "name"}), &[batch(&["a"])])
            .unwrap()
            .error
            .is_none()
    );
}

#[test]
fn parallel_streams() {
    let c = Arc::new(Client::start(&demo()));
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let c = Arc::clone(&c);
            std::thread::spawn(move || {
                let batches: Vec<Batch> = (0..50).map(|_| batch(&["x", "y"])).collect();
                let res = c
                    .run_transform("demo.upper", json!({"field": "name"}), &batches)
                    .unwrap();
                assert_eq!(res.records(TO_MAIN).len(), 100, "stream {i}");
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
}

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A host that sends more batches than the window must not lose records silently: the
/// stream ends with an error (and not with the error of the cancelled transform).
#[test]
fn too_many_batches() {
    let (r, mut w) = std::io::pipe().unwrap();
    let out = Buf::default();
    let o = out.clone();
    let serve = std::thread::spawn(move || demo().serve(r, o, std::io::sink()));
    let mut lines = vec![
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#.to_string(),
        r#"{"jsonrpc":"2.0","id":2,"method":"transform.open","params":{"stream":7,"type":"demo.stall","config":{},"emitWindow":4}}"#.to_string(),
    ];
    for _ in 0..=STREAM_WINDOW {
        lines.push(r#"{"jsonrpc":"2.0","method":"transform.batch","params":{"stream":7,"fields":[{"name":"a"}],"rows":[[1]]}}"#.into());
    }
    w.write_all((lines.join("\n") + "\n").as_bytes()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !String::from_utf8_lossy(&out.0.lock().unwrap()).contains("transform.closed") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(w);
    serve.join().unwrap().unwrap();
    let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
    let closed: serde_json::Value =
        serde_json::from_str(text.lines().find(|l| l.contains("transform.closed")).unwrap()).unwrap();
    assert_eq!(
        closed["params"],
        json!({"stream": 7, "error": {"message": "more than 4 batches arrived from Runnel without waiting for $/ack; the records cannot be processed completely"}})
    );
}

/// Without $/ack of the host, emit waits for credits; when stdin ends, the stream is
/// cancelled and serve returns.
#[test]
fn emit_waits_for_credits() {
    let (r, mut w) = std::io::pipe().unwrap();
    let out = Buf::default();
    let o = out.clone();
    let serve = std::thread::spawn(move || demo().serve(r, o, std::io::sink()));
    w.write_all(
        concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"transform.open","params":{"stream":1,"type":"demo.count","config":{"rows":1000},"emitWindow":2}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"transform.end","params":{"stream":1}}"#,
            "\n"
        )
        .as_bytes(),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let emits = String::from_utf8_lossy(&out.0.lock().unwrap())
        .matches("transform.emit")
        .count();
    assert_eq!(emits, 2, "emits without credits");
    w.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"$/ack\",\"params\":{\"stream\":1,\"batches\":3}}\n")
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let emits = String::from_utf8_lossy(&out.0.lock().unwrap())
        .matches("transform.emit")
        .count();
    assert_eq!(emits, 5, "emits after 3 more credits");
    drop(w);
    serve.join().unwrap().unwrap();
    let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
    assert!(
        text.contains(r#""error":{"message":"the stream was cancelled"}"#),
        "{text}"
    );
}

#[test]
fn definition_mistakes_panic() {
    let t = || Transform::new("demo.x", "X", "X", |_s, _: NoConfig| Ok(()));
    type Case = Box<dyn FnOnce() + std::panic::UnwindSafe>;
    let cases: Vec<(&str, Case)> = vec![
        (
            "outside the namespace",
            Box::new(|| {
                Plugin::new("demo", "1").transform(Transform::new("other.x", "X", "X", |_s, _: NoConfig| Ok(())));
            }),
        ),
        (
            "added twice",
            Box::new(move || {
                Plugin::new("demo", "1").transform(t()).transform(t());
            }),
        ),
        (
            "unknown category",
            Box::new(move || {
                Plugin::new("demo", "1").transform(t().category("general"));
            }),
        ),
    ];
    for (want, f) in cases {
        let err = std::panic::catch_unwind(f).expect_err("no panic");
        let msg = err.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(msg.contains(want), "panic {msg:?}, want {want:?}");
    }
    // the same type may be an action and a transform
    let mut p = Plugin::new("demo", "1");
    p.transform(t().category(runnel_plugin::category::EXECUTION));
    p.action(runnel_plugin::Action::new("demo.x", "X", "X", |_c, _: NoConfig| Ok(1)));
}
