use runnel_plugin::testing::{Client, Project};
use runnel_plugin::{Batch, Field, TO_ERROR, TO_MAIN, Value, types};
use serde_json::json;

#[test]
fn greet() {
    let c = Client::start(&hello::plugin());
    let out = c
        .execute("hello.greet", json!({"name": "World", "shout": "true"}))
        .unwrap();
    assert_eq!(out, json!({"message": "HELLO, WORLD!", "length": 13}));
    let err = c.execute("hello.greet", json!({"name": " "})).unwrap_err();
    assert_eq!(err.message, "name is empty");
    assert!(c.log_contains("INFO greeting World in run 01TESTRUN"));
}

#[test]
fn greet_records() {
    let c = Client::start(&hello::plugin());
    let b = Batch {
        fields: vec![Field::new("id", types::INT), Field::new("name", types::STRING)],
        rows: vec![
            vec![Value::Int(1), Value::from("Ada")],
            vec![Value::Int(2), Value::from(" ")],
        ],
    };
    let res = c
        .run_transform("hello.greet", json!({"field": "name", "greeting": "Moin"}), &[b])
        .unwrap();
    assert_eq!(res.error, None);
    assert_eq!(res.records(TO_MAIN)[0]["greeting"], "Moin, Ada!");
    assert_eq!(res.records(TO_ERROR)[0]["_error"], "name is empty");
}

#[test]
fn version_matches_plugin_yaml() {
    let yaml = include_str!("../plugin.yaml");
    assert!(
        yaml.lines().any(|l| l == format!("version: {}", hello::VERSION)),
        "plugin.yaml has another version than {}",
        hello::VERSION
    );
}

#[test]
fn without_runnel_the_program_explains_itself() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_hello"))
        .env_remove("RUNNEL_PLUGIN")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("is a Runnel plugin"));
}

/// The plugin in Runnel itself (RUNNEL_BIN): plugins, validate and a workflow.
#[test]
fn with_runnel() {
    let Some(p) = Project::new(
        concat!(env!("CARGO_MANIFEST_DIR"), "/plugin.yaml"),
        env!("CARGO_BIN_EXE_hello"),
    ) else {
        return;
    };
    let (out, code) = p.runnel(&["plugins", &p.dir.to_string_lossy()]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("hello.greet"), "{out}");
    p.write_file(
        "workflows/greet.workflow.yaml",
        r#"kind: Workflow
apiVersion: runnel/v1
name: greet
outputs:
  - name: MESSAGE
    value: "{{ results.greet.output.message }}"
actions:
  - id: start
    type: core.start
  - id: greet
    type: hello.greet
    with:
      name: Runnel
      shout: "${SHOUT}"
links:
  - { from: start, to: greet }
"#,
    );
    let (out, code) = p.validate();
    assert_eq!(code, 0, "{out}");
    let (out, code) = p.run("workflows/greet.workflow.yaml", &["SHOUT=true"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains(r#"output MESSAGE = "HELLO, RUNNEL!""#), "{out}");
    assert!(out.contains("hello: greeting Runnel"), "{out}");

    // The transform in a pipeline: 2,000 records through the plugin and its error link.
    p.write_file(
        "pipelines/greet.pipeline.yaml",
        r#"kind: Pipeline
apiVersion: runnel/v1
name: greet
transforms:
  - id: rows
    type: core.generate-rows
    with:
      rows: "2000"
      rowNumberField: n
      fields:
        - { name: name, value: Ada }
  - id: greet
    type: hello.greet
    with:
      field: name
  - id: write
    type: csv.output
    with:
      path: greetings.csv
      fields: [{ name: n }, { name: greeting }]
links:
  - { from: rows, to: greet }
  - { from: greet, to: write }
"#,
    );
    let (out, code) = p.validate();
    assert_eq!(code, 0, "{out}");
    let (out, code) = p.run("pipelines/greet.pipeline.yaml", &[]);
    assert_eq!(code, 0, "{out}");
    let csv = std::fs::read_to_string(p.dir.join("greetings.csv")).unwrap();
    let lines: Vec<&str> = csv.trim_start_matches('\u{feff}').trim().lines().collect();
    assert_eq!(lines.len(), 2001, "{csv}");
    assert_eq!(lines[2000].trim(), "2000,\"Hello, Ada!\"");
}
