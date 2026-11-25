//! Throughput of a transform over the protocol, with the test client and without Runnel:
//! 20 batches of 500 records with five fields through demo.upper and back. Target of the
//! SDKs (ADR 0019, Nachtrag S51): about 300,000 records/s.
//!
//! cargo test --release --test bench -- --ignored --nocapture

use std::time::Instant;

use runnel_plugin::testing::Client;
use runnel_plugin::{Batch, Config, Field, Plugin, TO_MAIN, Transform, Value, field_index, types};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize, Config, Default)]
#[serde(default)]
struct UpperConfig {
    /// Field
    field: String,
}

fn plugin() -> Plugin {
    let mut p = Plugin::new("demo", "1.0.0");
    p.transform(Transform::new(
        "demo.upper",
        "Upper",
        "Writes a field in capitals",
        |s, c: UpperConfig| {
            while let Some(mut b) = s.next()? {
                let at = field_index(&b.fields, &c.field).ok_or("no such field")?;
                for row in &mut b.rows {
                    if let Value::String(v) = &mut row[at] {
                        *v = v.to_uppercase();
                    }
                }
                s.emit(TO_MAIN, &b.fields, &b.rows)?;
            }
            Ok(())
        },
    ));
    p
}

#[test]
#[ignore = "benchmark: make bench"]
fn bench_transform_stream() {
    let c = Client::start(&plugin());
    let fields = vec![
        Field::new("id", types::INT),
        Field::new("name", types::STRING),
        Field::new("amount", types::DECIMAL),
        Field::new("day", types::DATE),
        Field::new("ok", types::BOOL),
    ];
    let rows: Vec<_> = (0..500)
        .map(|i| {
            vec![
                Value::Int(i),
                Value::from("anna"),
                Value::Decimal("12.50".into()),
                Value::Date("2026-11-24".into()),
                Value::Bool(true),
            ]
        })
        .collect();
    let batches = vec![Batch { fields, rows }; 20];
    let run = || {
        let res = c
            .run_transform("demo.upper", json!({"field": "name"}), &batches)
            .unwrap();
        assert!(res.error.is_none());
        assert_eq!(res.records(TO_MAIN).len(), 10_000);
    };
    run(); // warm up
    let rounds = 50;
    let start = Instant::now();
    for _ in 0..rounds {
        run();
    }
    let per_sec = f64::from(rounds * 10_000) / start.elapsed().as_secs_f64();
    println!("bench_transform_stream: {per_sec:.0} records/s ({rounds} × 10,000 records with five fields)");
}
