//! References (ADR 0014, E15b) in describe and the check for plugin authors.

use runnel_plugin::testing::{Client, check_references};
use runnel_plugin::{Action, Config, NoConfig, OutputField, Plugin, Reference, Transform};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize, Config, Default)]
#[serde(default)]
struct Fill {
    /// Name of the new field
    target: String,
    /// Fields
    fields: Vec<Named>,
}

#[derive(Deserialize, Config, Default)]
#[serde(default)]
struct Named {
    /// Name
    name: String,
}

fn plugin(action: Reference, transform: Reference) -> Plugin {
    let mut p = Plugin::new("demo", "1.0.0");
    p.action(Action::new("demo.check", "Check", "Checks", |_c, _: NoConfig| Ok(true)).reference(action));
    p.transform(Transform::new("demo.fill", "Fill", "Fills", |_s, _: Fill| Ok(())).reference(transform));
    p.action(Action::new(
        "demo.plain",
        "Plain",
        "Has no reference",
        |_c, _: NoConfig| Ok(1),
    ));
    p
}

fn good() -> (Reference, Reference) {
    (
        Reference::new()
            .output(OutputField::new(
                "output.valid",
                "boolean",
                "Whether the value is valid",
            ))
            .errors("Fails when the value is empty.")
            .limit("The value may have at most 34 characters."),
        Reference::new()
            .output(OutputField::new("<target>", "string", "The new value"))
            .output(OutputField::new("<fields[].name>", "any", "Each named field"))
            .records("Passes every record on over the data links.")
            .errors("Records without value go to the error link."),
    )
}

#[test]
fn describe_has_the_reference() {
    let (a, t) = good();
    let c = Client::start(&plugin(a, t));
    let d = c.describe().unwrap();
    let check = &d["actions"][0];
    assert_eq!(
        check["outputs"],
        json!([{"name": "output.valid", "type": "boolean", "description": "Whether the value is valid"}])
    );
    assert_eq!(check["errors"], "Fails when the value is empty.");
    assert_eq!(check["limits"], json!(["The value may have at most 34 characters."]));
    assert!(check.get("records").is_none());
    let fill = &d["transforms"][0];
    assert_eq!(fill["records"], "Passes every record on over the data links.");
    assert_eq!(fill["outputs"][1]["name"], "<fields[].name>");
    let plain = &d["actions"][1];
    for k in ["outputs", "records", "errors", "limits"] {
        assert!(plain.get(k).is_none(), "empty {k} is left out: {plain}");
    }
}

#[test]
fn check_finds_mistakes() {
    let (a, t) = good();
    let mut p = plugin(a, t);
    // demo.plain has no errors
    let err = check_references(&p).unwrap_err();
    assert_eq!(
        err,
        "action demo.plain: missing Errors: when does it fail (and what goes to the error link)?"
    );
    p = Plugin::new("demo", "1.0.0");
    p.action(
        Action::new("demo.bad", "Bad", "Bad", |_c, _: NoConfig| Ok(1)).reference(
            Reference::new()
                .output(OutputField::new("valid", "boolean", "Whether it is valid."))
                .output(OutputField::new("valid", "string", " "))
                .errors("no period")
                .limit(""),
        ),
    );
    p.transform(
        Transform::new("demo.bad", "Bad", "Bad", |_s, _: Fill| Ok(())).reference(
            Reference::new()
                .output(OutputField::new("<nope>", "int", "A field"))
                .output(OutputField::new("<fields[].other>", "string", "Another"))
                .output(OutputField::new("<target", "string", "Unclosed"))
                .errors("Fails."),
        ),
    );
    let err = check_references(&p).unwrap_err();
    let want = [
        r#"action demo.bad: Errors "no period" must be English sentences ending with a period"#,
        r#"action demo.bad: Limits "" must be English sentences ending with a period"#,
        r#"action demo.bad: output "valid": description "Whether it is valid." must be one phrase without a final period"#,
        r#"action demo.bad: output "valid" must be named by its path, e.g. output, output.valid or output[].dn"#,
        r#"action demo.bad: output "valid" is described twice"#,
        r#"action demo.bad: output "valid": description " " must be one phrase without a final period"#,
        r#"action demo.bad: output "valid" must be named by its path"#,
        "transform demo.bad: missing Records: which records does it pass on, over which links?",
        r#"transform demo.bad: output "<nope>": "nope" is no field of the configuration"#,
        r#"transform demo.bad: output "<fields[].other>": "fields[].other" is no field of the configuration"#,
        r#"transform demo.bad: output "<target": "target" is no field of the configuration"#,
    ];
    let lines: Vec<&str> = err.lines().collect();
    assert_eq!(lines.len(), want.len(), "{err}");
    for (l, w) in lines.iter().zip(want) {
        assert!(l.starts_with(w), "\n got {l}\nwant {w}");
    }
}

type Case = Box<dyn FnOnce() + std::panic::UnwindSafe>;

#[test]
fn definition_mistakes_panic() {
    let cases: Vec<(&str, Case)> = vec![
        (
            r#"action "demo.a": Records is for transforms only"#,
            Box::new(|| {
                Plugin::new("demo", "1").action(
                    Action::new("demo.a", "A", "A", |_c, _: NoConfig| Ok(1)).reference(Reference::new().records("No.")),
                );
            }),
        ),
        (
            r#"action "demo.a": output "output.x" has type "int", not one of string, integer"#,
            Box::new(|| {
                Plugin::new("demo", "1").action(
                    Action::new("demo.a", "A", "A", |_c, _: NoConfig| Ok(1))
                        .reference(Reference::new().output(OutputField::new("output.x", "int", "X"))),
                );
            }),
        ),
        (
            r#"transform "demo.t": output "<x>" has type "integer", not one of string, int"#,
            Box::new(|| {
                Plugin::new("demo", "1").transform(
                    Transform::new("demo.t", "T", "T", |_s, _: NoConfig| Ok(()))
                        .reference(Reference::new().output(OutputField::new("<x>", "integer", "X"))),
                );
            }),
        ),
        (
            r#"transform "demo.t": an output without name"#,
            Box::new(|| {
                Plugin::new("demo", "1").transform(
                    Transform::new("demo.t", "T", "T", |_s, _: NoConfig| Ok(()))
                        .reference(Reference::new().output(OutputField::new("", "string", "X"))),
                );
            }),
        ),
    ];
    for (want, f) in cases {
        let err = std::panic::catch_unwind(f).expect_err("no panic");
        let msg = err.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(msg.contains(want), "panic {msg:?}, want {want:?}");
    }
}

#[test]
fn check_accepts_good_references() {
    let (a, t) = good();
    let mut p = Plugin::new("demo", "1.0.0");
    p.action(Action::new("demo.check", "Check", "Checks", |_c, _: NoConfig| Ok(true)).reference(a));
    p.transform(Transform::new("demo.fill", "Fill", "Fills", |_s, _: Fill| Ok(())).reference(t));
    check_references(&p).unwrap();
}
