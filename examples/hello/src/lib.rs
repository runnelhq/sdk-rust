//! The example plugin of the Rust SDK: the action `hello.greet` returns a greeting, the
//! transform `hello.greet` adds one to every record. The
//! program (src/main.rs) serves it; build it into `plugins/hello/bin/` of a project (see
//! README.md of the SDK).

use runnel_plugin::{
    Action, Call, Config, Error, Field, Plugin, Stream, TO_ERROR, TO_MAIN, Transform, Value, error_fields, error_row,
    field_index, types,
};
use serde::{Deserialize, Serialize};

/// Must match version in plugin.yaml (a test checks it).
pub const VERSION: &str = "1.0.0";

#[derive(Deserialize, Config, Default)]
#[serde(default)]
struct GreetConfig {
    /// Whom to greet, e.g. World
    #[runnel(required, example = "World")]
    name: String,
    /// Word to greet with
    #[runnel(values = "Hello|Hi|Moin", default = "Hello")]
    greeting: String,
    /// Write the greeting in capital letters
    shout: bool,
}

#[derive(Serialize)]
struct GreetOutput {
    message: String,
    length: usize,
}

fn greet(call: &Call, c: GreetConfig) -> Result<GreetOutput, Error> {
    if c.name.trim().is_empty() {
        return Err("name is empty".into());
    }
    let greeting = if c.greeting.is_empty() { "Hello" } else { &c.greeting };
    let mut message = format!("{greeting}, {}!", c.name);
    if c.shout {
        message = message.to_uppercase();
    }
    call.log
        .info(format_args!("greeting {} in run {}", c.name, call.run_id));
    Ok(GreetOutput {
        length: message.chars().count(),
        message,
    })
}

#[derive(Deserialize, Config, Default)]
#[serde(default)]
struct GreetFieldConfig {
    /// Field with the name, e.g. name
    #[runnel(required, format = "runnel-field", example = "name")]
    field: String,
    /// New field for the greeting
    #[runnel(default = "greeting")]
    target: String,
    /// Word to greet with
    #[runnel(values = "Hello|Hi|Moin", default = "Hello")]
    greeting: String,
}

impl GreetFieldConfig {
    fn defaults(mut self) -> Self {
        if self.target.is_empty() {
            self.target = "greeting".into();
        }
        if self.greeting.is_empty() {
            self.greeting = "Hello".into();
        }
        self
    }
}

/// Adds the greeting to every record; records without a name go to the error link with
/// _error and _errorField.
fn greet_records(s: &mut Stream, c: GreetFieldConfig) -> Result<(), Error> {
    let c = c.defaults();
    while let Some(b) = s.next()? {
        let at =
            field_index(&b.fields, &c.field).ok_or_else(|| format!("field {:?} is not in the records", c.field))?;
        let mut out = b.fields.clone();
        out.push(Field::new(&c.target, types::STRING));
        let (mut good, mut bad) = (Vec::new(), Vec::new());
        for mut row in b.rows {
            match row[at].as_str().map(str::trim).filter(|n| !n.is_empty()) {
                Some(name) => {
                    let greeting = format!("{}, {name}!", c.greeting);
                    row.push(Value::String(greeting));
                    good.push(row);
                }
                None => bad.push(error_row(&row, "name is empty", &c.field)),
            }
        }
        s.emit(TO_MAIN, &out, &good)?;
        s.emit(TO_ERROR, &error_fields(&b.fields), &bad)?;
    }
    Ok(())
}

/// The plugin with its actions; main serves it, the tests call it directly.
pub fn plugin() -> Plugin {
    let mut p = Plugin::new("hello", VERSION);
    p.action(
        Action::new("hello.greet", "Greet", "Returns a greeting for a name", greet)
            .example("type: hello.greet\nwith:\n  name: World\n"),
    );
    p.transform(
        Transform::new(
            "hello.greet",
            "Greet",
            "Adds a greeting for the name in a field of every record",
            greet_records,
        )
        .example("type: hello.greet\nwith:\n  field: name\n  target: greeting\n")
        .record_errors()
        .parallel(),
    );
    p
}
