//! What a building block returns, when it fails and which limits apply (ADR 0014 of
//! Runnel, E15b): for `runnel catalog`, the reference on the website and the help in the
//! editor.

use serde::Serialize;

use crate::schema::ConfigSchema;

/// Types of the output of actions, as expressions see it.
pub const OUTPUT_TYPES: &[&str] = &[
    "string", "integer", "number", "boolean", "datetime", "list", "object", "any",
];

/// Types of the fields a transform sets (ADR 0007), and `any`.
pub const RECORD_OUTPUT_TYPES: &[&str] = &[
    "string", "int", "decimal", "float", "bool", "date", "datetime", "binary", "json", "any",
];

/// The reference of an action or transform. All parts are optional in the protocol; the
/// built-in types of Runnel always have `errors`, transforms also `records`, and actions
/// with an output its `outputs` ([`crate::testing::check_references`] checks the same).
///
/// ```
/// use runnel_plugin::{OutputField, Reference};
///
/// let r = Reference::new()
///     .output(OutputField::new("output.valid", "boolean", "Whether the IBAN is valid"))
///     .errors("Fails when iban is empty.")
///     .limit("The IBAN may have at most 34 characters.");
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Reference {
    /// Actions: the parts of `result.output` by their path, e.g. `output.status` or
    /// `output[].path` (`output` for the whole value). Transforms: the fields they add or
    /// change; a name from the configuration is the path of its field in angle brackets,
    /// e.g. `<target>` or `<fields[].name>`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<OutputField>,
    /// Transforms only: which records they pass on and over which links (data links,
    /// targets, error link). Sentences ending with a period.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub records: String,
    /// When the type fails and, for a transform, which records go to the error link.
    /// Sentences ending with a period.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub errors: String,
    /// Limits of size, number and time with their defaults, each a sentence.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub limits: Vec<String>,
}

/// A part of the output of an action, or a field a transform sets.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct OutputField {
    pub name: String,
    /// One of [`OUTPUT_TYPES`] for actions, of [`RECORD_OUTPUT_TYPES`] for transforms.
    #[serde(rename = "type")]
    pub type_: String,
    /// One phrase without a final period, like the description of a field.
    pub description: String,
}

impl OutputField {
    pub fn new(name: &str, type_: &str, description: &str) -> OutputField {
        OutputField {
            name: name.to_string(),
            type_: type_.to_string(),
            description: description.to_string(),
        }
    }
}

impl Reference {
    pub fn new() -> Reference {
        Reference::default()
    }

    /// Adds a part of the output (actions) or a field the transform sets.
    pub fn output(mut self, field: OutputField) -> Self {
        self.outputs.push(field);
        self
    }

    /// Which records a transform passes on, over which links.
    pub fn records(mut self, text: &str) -> Self {
        self.records = text.to_string();
        self
    }

    /// When the type fails.
    pub fn errors(mut self, text: &str) -> Self {
        self.errors = text.to_string();
        self
    }

    /// Adds a limit.
    pub fn limit(mut self, text: &str) -> Self {
        self.limits.push(text.to_string());
        self
    }
}

/// The mistakes in the reference of a type, as the contract tests of Runnel find them in
/// built-in types: `errors` missing (and `records` for a transform), texts that are no
/// sentences, output types that do not exist, output names that are no path of the output
/// or name a field the configuration does not have.
pub(crate) fn check(kind: &str, r: &Reference, s: &ConfigSchema) -> Vec<String> {
    let mut problems = Vec::new();
    let sentence = |what: &str, text: &str, problems: &mut Vec<String>| {
        if text.trim().is_empty() || !text.ends_with('.') {
            problems.push(format!(
                "{what} {text:?} must be English sentences ending with a period"
            ));
        }
    };
    if r.errors.is_empty() {
        problems.push("missing Errors: when does it fail (and what goes to the error link)?".into());
    } else {
        sentence("Errors", &r.errors, &mut problems);
    }
    match (kind, r.records.is_empty()) {
        ("transform", true) => {
            problems.push("missing Records: which records does it pass on, over which links?".into())
        }
        ("transform", false) => sentence("Records", &r.records, &mut problems),
        (_, false) => problems.push("Records is for transforms only".into()),
        _ => {}
    }
    for l in &r.limits {
        sentence("Limits", l, &mut problems);
    }
    let types = if kind == "transform" {
        RECORD_OUTPUT_TYPES
    } else {
        OUTPUT_TYPES
    };
    let mut names: Vec<&str> = Vec::new();
    for o in &r.outputs {
        if names.contains(&o.name.as_str()) {
            problems.push(format!("output {:?} is described twice", o.name));
        }
        names.push(&o.name);
        if !types.contains(&o.type_.as_str()) {
            problems.push(format!(
                "output {:?}: type {:?} is not one of {types:?}",
                o.name, o.type_
            ));
        }
        if o.description.trim().is_empty() || o.description.ends_with('.') {
            problems.push(format!(
                "output {:?}: description {:?} must be one phrase without a final period",
                o.name, o.description
            ));
        }
        if kind == "action" && o.name != "output" && !o.name.starts_with("output.") && !o.name.starts_with("output[]") {
            problems.push(format!(
                "output {:?} must be named by its path, e.g. output, output.valid or output[].dn",
                o.name
            ));
        }
        let mut rest = o.name.as_str();
        while let Some(start) = rest.find('<') {
            let after = &rest[start + 1..];
            match after.find('>') {
                Some(end) if has_field(s, &after[..end]) => rest = &after[end + 1..],
                Some(end) => {
                    problems.push(format!(
                        "output {:?}: {:?} is no field of the configuration",
                        o.name,
                        &after[..end]
                    ));
                    rest = &after[end + 1..];
                }
                None => {
                    problems.push(format!(
                        "output {:?}: {:?} is no field of the configuration",
                        o.name, after
                    ));
                    break;
                }
            }
        }
    }
    problems
}

/// Panics on mistakes in a reference that Runnel could not show (like the Go SDK); the
/// style of the texts is checked by [`crate::testing::check_references`].
pub(crate) fn check_definition(kind: &str, typ: &str, r: &Reference) {
    assert!(
        kind != "action" || r.records.is_empty(),
        "runnel: action {typ:?}: Records is for transforms only"
    );
    let types = if kind == "transform" {
        RECORD_OUTPUT_TYPES
    } else {
        OUTPUT_TYPES
    };
    for o in &r.outputs {
        assert!(!o.name.is_empty(), "runnel: {kind} {typ:?}: an output without name");
        assert!(
            types.contains(&o.type_.as_str()),
            "runnel: {kind} {typ:?}: output {:?} has type {:?}, not one of {}",
            o.name,
            o.type_,
            types.join(", ")
        );
    }
}

/// Whether a path like `fields[].name` names a field of a schema.
fn has_field(s: &ConfigSchema, path: &str) -> bool {
    let mut s = s;
    for part in path.split('.') {
        let (name, list) = match part.strip_suffix("[]") {
            Some(n) => (n, true),
            None => (part, false),
        };
        let Some(next) = s.property(name) else { return false };
        s = if list {
            match &next.items {
                Some(items) => items,
                None => return false,
            }
        } else {
            next
        };
    }
    true
}
