//! SDK for [Runnel](https://github.com/runnelhq/runnel) plugins in Rust.
//!
//! A plugin is a program that Runnel starts with `RUNNEL_PLUGIN=1` and talks to over
//! JSON-RPC on stdin and stdout (ADR 0019 of Runnel, protocol in `docs/dev/PLUGINS.md` of
//! the Runnel repository). This crate speaks the protocol – handshake, `describe`,
//! `action.execute`, `connection.test`, cancellation, concurrency – so that a plugin only
//! defines its actions and connection types:
//!
//! ```no_run
//! use runnel_plugin::{Action, Config, Plugin};
//! use serde::Deserialize;
//!
//! #[derive(Deserialize, Config, Default)]
//! #[serde(default)]
//! struct CheckConfig {
//!     /// The IBAN to check
//!     #[runnel(required, example = "DE02120300000000202051")]
//!     iban: String,
//! }
//!
//! fn main() {
//!     let mut p = Plugin::new("iban", "1.0.0"); // name and version as in plugin.yaml
//!     p.action(Action::new("iban.check", "Check IBAN", "Checks an IBAN", |_call, c: CheckConfig| {
//!         Ok(serde_json::json!({ "valid": !c.iban.is_empty() }))
//!     }));
//!     p.main();
//! }
//! ```
//!
//! The schema of the configuration comes from the struct and its `#[runnel(…)]`
//! attributes, as for the built-in actions of Runnel (see [`Config`]). Runnel checks the
//! configuration against it and resolves variables, expressions and connections before it
//! calls the plugin.

mod cancel;
mod decode;
mod log;
pub mod schema;
mod serve;
pub mod testing;
mod transform;
mod value;

use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

pub use cancel::Cancel;
pub use log::Logger;
pub use schema::ConfigSchema;
pub use serve::MAX_MESSAGE_SIZE;
pub use transform::{Batch, STREAM_WINDOW, Stream, TO_ERROR, TO_MAIN, Transform, transform_category};
pub use value::{Field, Row, Value, error_fields, error_row, field_index, types};

/// Derives [`Config`] for a struct with named fields; see there.
pub use runnel_plugin_derive::Config;

/// The version of the plugin protocol this SDK speaks (ADR 0019).
pub const PROTOCOL_VERSION: u32 = 1;

/// How many requests a plugin of this SDK handles at the same time, unless
/// [`Plugin::concurrency`] says otherwise.
pub const DEFAULT_CONCURRENCY: usize = 4;

/// Categories of actions in the palette of the editor.
pub mod category {
    pub const GENERAL: &str = "general";
    pub const EXECUTION: &str = "execution";
    pub const FILES: &str = "files";
    pub const NETWORK: &str = "network";
    pub const DATABASE: &str = "database";

    pub(crate) const ACTIONS: &[&str] = &[GENERAL, EXECUTION, FILES, NETWORK, DATABASE];
}

/// Error codes of JSON-RPC 2.0 and of the plugin protocol.
pub mod code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const NOT_INITIALIZED: i64 = -32002;
    pub const REQUEST_CANCELLED: i64 = -32800;
    pub const ACTION_FAILED: i64 = -32000;
}

/// The error of an action, test or transform: its message is what Runnel shows. Any error
/// type converts with `?`; a text with `.into()`, e.g. `return Err("name is empty".into())`.
pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// A configuration type: a struct whose fields describe themselves. Derive it with
/// `#[derive(Deserialize, Config)]`:
///
/// ```
/// use runnel_plugin::Config;
/// use serde::Deserialize;
///
/// #[derive(Deserialize, Config, Default)]
/// #[serde(default, rename_all = "camelCase")]
/// struct SearchConfig {
///     /// Filter of the search, e.g. (objectClass=user)
///     #[runnel(required, example = "(objectClass=user)")]
///     filter: String,
///     /// How many entries to return at most
///     #[runnel(default = "100")]
///     size_limit: u32,
///     /// Scope of the search
///     #[runnel(values = "base|one|sub", default = "sub", group = "Advanced")]
///     scope: String,
/// }
/// ```
///
/// - **Names** come from serde: the field name, `#[serde(rename = "…")]` or
///   `#[serde(rename_all = "…")]` on the struct. Fields with `#[serde(skip)]` are not part
///   of the schema; `#[serde(flatten)]` takes the fields of another `Config` struct.
/// - **Descriptions** come from the doc comment of the field (English, one sentence), or
///   from `#[runnel(doc = "…")]`.
/// - **Types:** texts → `string`, whole numbers → `integer` (unsigned with `minimum: 0`),
///   floats → `number`, `bool` → `boolean`, `Vec<T>` → `array`, maps with text keys →
///   `object` with `additionalProperties`, structs with `#[derive(Config)]` → `object`
///   with their own fields, `serde_json::Value` → any value, `Option<T>` → as `T`.
///   Unknown fields are not allowed (`additionalProperties: false`).
/// - **`#[runnel(…)]`:** `required`; `default = "…"`, `example = "…"`,
///   `values = "a|b|c"` (allowed values, `enum` in the schema); `format = "…"` (e.g.
///   `runnel-secret`, `runnel-duration`, `runnel-text`, `runnel-field`, `runnel-target`,
///   see [`schema`]); `group = "Advanced"` (section of the form);
///   `only = "field:a|b"` (the field applies only when another field has one of the
///   values); `connection = "ldap|ldap-ad"` (the name of a connection of these types: the
///   field must be a struct, which receives the resolved connection, see
///   [`ConnectionInfo`]); `skip` (decoded, but not part of the schema).
///
/// Runnel sends configurations with null or without optional fields, so a struct should
/// have `#[serde(default)]` (and derive or implement `Default`). Texts in number and
/// boolean fields are converted, because resolved variables arrive as text; a value that
/// does not fit fails with a message such as `field "times": expected a whole number,
/// got string`.
pub trait Config: DeserializeOwned + Send + 'static {
    /// The schema of the struct; `depth` guards against recursive types.
    fn schema_at(depth: usize) -> Result<ConfigSchema, schema::SchemaError>;

    /// The JSON Schema of the configuration, as `describe` sends it.
    fn schema() -> Result<ConfigSchema, schema::SchemaError> {
        let mut s = Self::schema_at(0)?;
        s.schema = Some(schema::DRAFT.to_string());
        Ok(s)
    }
}

/// The configuration of a type without fields.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
pub struct NoConfig {}

impl Config for NoConfig {
    fn schema_at(depth: usize) -> Result<ConfigSchema, schema::SchemaError> {
        schema::ObjectBuilder::new(depth)?.finish()
    }
}

/// The name and type of a resolved connection. Flatten it into the struct of a connection
/// type to receive them; they are not part of the schema:
///
/// ```
/// use runnel_plugin::{Config, ConnectionInfo};
/// use serde::Deserialize;
///
/// #[derive(Deserialize, Config, Default)]
/// #[serde(default)]
/// struct LdapConnection {
///     #[serde(flatten)]
///     info: ConnectionInfo,
///     /// Address, e.g. ldaps://dc1.example.com
///     #[runnel(required)]
///     url: String,
///     /// Password
///     #[runnel(format = "runnel-secret")]
///     password: String,
/// }
/// ```
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ConnectionInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub type_: String,
}

impl Config for ConnectionInfo {
    fn schema_at(depth: usize) -> Result<ConfigSchema, schema::SchemaError> {
        schema::ObjectBuilder::new(depth)?.finish()
    }
}

/// The result of testing a connection.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct TestResult {
    /// Whether the connection works; false with a message is a failed test.
    pub ok: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    /// Version of the server, if known, e.g. "OpenLDAP 2.6".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
}

/// What every request gets: the log of the run and the cancellation.
#[derive(Debug, Clone)]
pub struct Context {
    /// Writes to the log of the run (stderr of the plugin).
    pub log: Logger,
    cancel: Cancel,
}

impl Context {
    pub(crate) fn new(log: Logger, cancel: Cancel) -> Context {
        Context { log, cancel }
    }

    /// Whether Runnel cancelled the request (or the plugin ends); long work should stop.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// The cancellation, e.g. to wait with [`Cancel::sleep`] or to hand it to a thread.
    pub fn cancel(&self) -> &Cancel {
        &self.cancel
    }
}

/// One execution of an action.
#[derive(Debug, Clone)]
pub struct Call {
    /// ID of the run.
    pub run_id: String,
    /// id of the action in the workflow.
    pub action_id: String,
    /// Project folder; also the working directory of the plugin.
    pub project_dir: String,
    /// Name of the environment, empty without one.
    pub environment: String,
    pub(crate) ctx: Context,
}

impl std::ops::Deref for Call {
    type Target = Context;
    fn deref(&self) -> &Context {
        &self.ctx
    }
}

type ActionFn = dyn Fn(&Call, Option<&serde_json::value::RawValue>) -> Result<Json, Error> + Send + Sync;
type RunFn<C> = dyn Fn(&Call, C) -> Result<Json, Error> + Send + Sync;
type TypedTestFn<C> = dyn Fn(&Context, C) -> Result<TestResult, Error> + Send + Sync;
type TestFn = dyn Fn(&Context, Option<&serde_json::value::RawValue>) -> Result<TestResult, Error> + Send + Sync;

/// An action of the plugin with its configuration type C. Create it with [`Action::new`]
/// and add it with [`Plugin::action`].
pub struct Action<C> {
    type_: String,
    category: String,
    label: String,
    description: String,
    example: String,
    run: Arc<RunFn<C>>,
}

impl<C: Config> Action<C> {
    /// An action: its type `"<namespace>.<name>"` (e.g. `"iban.check"`), the short name in
    /// the palette (e.g. `"Check IBAN"`), one sentence for editor and tools, and the
    /// function that runs it. `run` gets the configuration Runnel checked and resolved
    /// (variables, expressions, connections); its result is `result.output` in the
    /// workflow (text, number, list, map or struct). An error fails the action with its
    /// message. `run` should stop when [`Context::is_cancelled`] becomes true.
    pub fn new<R, F>(type_: &str, label: &str, description: &str, run: F) -> Action<C>
    where
        R: Serialize,
        F: Fn(&Call, C) -> Result<R, Error> + Send + Sync + 'static,
    {
        Action {
            type_: type_.to_string(),
            category: String::new(),
            label: label.to_string(),
            description: description.to_string(),
            example: String::new(),
            run: Arc::new(move |call, c| {
                let out = run(call, c)?;
                serde_json::to_value(out).map_err(|e| format!("the plugin cannot encode its result: {e}").into())
            }),
        }
    }

    /// Optional: one of the constants in [`category`] puts the action into that group of the
    /// palette; without, it is in a group of its own named after the plugin ("Plugin:
    /// <name>").
    pub fn category(mut self, category: &str) -> Self {
        self.category = category.to_string();
        self
    }

    /// Optional YAML of the action for the help in the editor.
    pub fn example(mut self, yaml: &str) -> Self {
        self.example = yaml.to_string();
        self
    }
}

/// A connection type of the plugin with the struct C of its fields. Fields holding
/// passwords or tokens need `format = "runnel-secret"`.
pub struct ConnectionType<C> {
    type_: String,
    label: String,
    description: String,
    test: Option<Arc<TypedTestFn<C>>>,
}

impl<C: Config> ConnectionType<C> {
    /// A connection type: the namespace or `"<namespace>-…"` (e.g. `"ldap"` or
    /// `"ldap-ad"`), its label and description.
    pub fn new(type_: &str, label: &str, description: &str) -> ConnectionType<C> {
        ConnectionType {
            type_: type_.to_string(),
            label: label.to_string(),
            description: description.to_string(),
            test: None,
        }
    }

    /// Optional: checks the connection for "Test connection" in the editor. It should sign
    /// in, but neither write nor send anything else. An error counts as a failed test with
    /// its message.
    pub fn test<F>(mut self, test: F) -> Self
    where
        F: Fn(&Context, C) -> Result<TestResult, Error> + Send + Sync + 'static,
    {
        self.test = Some(Arc::new(test));
        self
    }
}

#[derive(Serialize, Clone)]
pub(crate) struct ActionDescriptor {
    #[serde(rename = "type")]
    type_: String,
    kind: &'static str,
    #[serde(skip_serializing_if = "String::is_empty")]
    category: String,
    label: String,
    description: String,
    schema: ConfigSchema,
    #[serde(skip_serializing_if = "String::is_empty")]
    example: String,
}

#[derive(Serialize, Clone)]
pub(crate) struct ConnectionDescriptor {
    #[serde(rename = "type")]
    type_: String,
    label: String,
    description: String,
    schema: ConfigSchema,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    test: bool,
}

#[derive(Clone)]
pub(crate) struct ActionEntry {
    pub(crate) descriptor: ActionDescriptor,
    pub(crate) run: Arc<ActionFn>,
}

#[derive(Clone)]
pub(crate) struct ConnectionEntry {
    pub(crate) descriptor: ConnectionDescriptor,
    pub(crate) test: Option<Arc<TestFn>>,
}

/// A Runnel plugin: its name and version as in plugin.yaml, and the actions and connection
/// types it offers. Create it with [`Plugin::new`], add actions and connection types, then
/// call [`Plugin::main`].
#[derive(Clone)]
pub struct Plugin {
    name: String,
    version: String,
    namespace: String,
    concurrency: usize,
    pub(crate) actions: Vec<ActionEntry>,
    pub(crate) connections: Vec<ConnectionEntry>,
    pub(crate) transforms: Vec<transform::TransformEntry>,
}

fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn is_type_name(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() >= 2 && parts.iter().all(|p| is_name(p))
}

impl Plugin {
    /// A plugin with the name and version from plugin.yaml; the namespace is the name.
    /// Panics on an invalid name, which is a programming error.
    pub fn new(name: &str, version: &str) -> Plugin {
        assert!(
            is_name(name),
            "runnel: invalid plugin name {name:?} (lowercase letters, digits and -)"
        );
        assert!(!version.is_empty(), "runnel: the plugin needs a version");
        Plugin {
            name: name.to_string(),
            version: version.to_string(),
            namespace: name.to_string(),
            concurrency: DEFAULT_CONCURRENCY,
            actions: Vec::new(),
            connections: Vec::new(),
            transforms: Vec::new(),
        }
    }

    /// The name of the plugin.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The version of the plugin.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Sets the namespace of plugin.yaml, if it differs from the name; all types of the
    /// plugin start with it. Set it before adding actions.
    pub fn namespace(&mut self, namespace: &str) -> &mut Self {
        assert!(is_name(namespace), "runnel: invalid namespace {namespace:?}");
        assert!(
            self.actions.is_empty() && self.connections.is_empty(),
            "runnel: set the namespace before adding types"
        );
        self.namespace = namespace.to_string();
        self
    }

    /// Sets how many requests the plugin handles at the same time (at least 1, default
    /// [`DEFAULT_CONCURRENCY`]). Set 1 if the actions must not run in parallel. Streams of
    /// transforms do not count.
    pub fn concurrency(&mut self, n: usize) -> &mut Self {
        self.concurrency = n.max(1);
        self
    }

    pub(crate) fn concurrency_value(&self) -> usize {
        self.concurrency
    }

    pub(crate) fn check_type(&self, what: &str, t: &str) {
        assert!(
            is_type_name(t),
            "runnel: invalid {what} type {t:?} (<namespace>.<name>)"
        );
        let ns = t.split('.').next().unwrap_or_default();
        assert!(
            ns == self.namespace,
            "runnel: {what} type {t:?} is outside the namespace {:?}",
            self.namespace
        );
    }

    /// Adds an action. Panics on mistakes in the definition (type outside the namespace,
    /// added twice, unknown category, invalid schema attributes), which are programming
    /// errors that the first test reveals.
    pub fn action<C: Config>(&mut self, a: Action<C>) -> &mut Self {
        self.check_type("action", &a.type_);
        assert!(
            !self.actions.iter().any(|e| e.descriptor.type_ == a.type_),
            "runnel: action type {:?} added twice",
            a.type_
        );
        assert!(
            a.category.is_empty() || category::ACTIONS.contains(&a.category.as_str()),
            "runnel: action {:?}: unknown category {:?}",
            a.type_,
            a.category
        );
        assert!(
            !a.label.is_empty() && !a.description.is_empty(),
            "runnel: action {:?} needs a label and a description",
            a.type_
        );
        let schema = C::schema().unwrap_or_else(|e| panic!("runnel: action {:?}: {e}", a.type_));
        let s = schema.clone();
        let run = a.run.clone();
        self.actions.push(ActionEntry {
            descriptor: ActionDescriptor {
                type_: a.type_,
                kind: "action",
                category: a.category,
                label: a.label,
                description: a.description,
                schema,
                example: a.example,
            },
            run: Arc::new(move |call, raw| {
                let c: C = decode::decode(raw, &s)?;
                run(call, c)
            }),
        });
        self
    }

    /// Adds a connection type. Like [`Plugin::action`], it panics on mistakes in the
    /// definition.
    pub fn connection_type<C: Config>(&mut self, c: ConnectionType<C>) -> &mut Self {
        let ns = &self.namespace;
        assert!(
            c.type_ == *ns || (c.type_.starts_with(&format!("{ns}-")) && is_name(&c.type_)),
            "runnel: connection type {:?} must be {ns:?} or start with {:?}",
            c.type_,
            format!("{ns}-")
        );
        assert!(
            !self.connections.iter().any(|e| e.descriptor.type_ == c.type_),
            "runnel: connection type {:?} added twice",
            c.type_
        );
        assert!(
            !c.label.is_empty() && !c.description.is_empty(),
            "runnel: connection type {:?} needs a label and a description",
            c.type_
        );
        let schema = C::schema().unwrap_or_else(|e| panic!("runnel: connection type {:?}: {e}", c.type_));
        let s = schema.clone();
        let test = c.test.map(|t| -> Arc<TestFn> {
            Arc::new(move |ctx, raw| {
                let conn: C = decode::decode(raw, &s)?;
                t(ctx, conn)
            })
        });
        self.connections.push(ConnectionEntry {
            descriptor: ConnectionDescriptor {
                type_: c.type_,
                label: c.label,
                description: c.description,
                schema,
                test: test.is_some(),
            },
            test,
        });
        self
    }
}

/// Mistakes in the attributes are compile errors:
///
/// ```compile_fail
/// #[derive(serde::Deserialize, runnel_plugin::Config)]
/// struct C {
///     #[runnel(size = "3")]
///     x: String,
/// }
/// ```
///
/// ```compile_fail
/// #[derive(serde::Deserialize, runnel_plugin::Config)]
/// struct C(String);
/// ```
#[cfg(doctest)]
struct DeriveMistakes;
