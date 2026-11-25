//! The example plugin of the Rust SDK: the action `hello.greet` returns a greeting. The
//! program (src/main.rs) serves it; build it into `plugins/hello/bin/` of a project (see
//! README.md of the SDK).

use runnel_plugin::{Action, Call, Config, Error, Plugin};
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

/// The plugin with its actions; main serves it, the tests call it directly.
pub fn plugin() -> Plugin {
    let mut p = Plugin::new("hello", VERSION);
    p.action(
        Action::new("hello.greet", "Greet", "Returns a greeting for a name", greet)
            .example("type: hello.greet\nwith:\n  name: World\n"),
    );
    p
}
