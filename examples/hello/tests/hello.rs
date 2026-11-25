use runnel_plugin::testing::{Client, Project};
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
}
