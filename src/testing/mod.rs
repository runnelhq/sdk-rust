//! Tests of plugins without Runnel: [`Client::start`] serves a plugin in the test process
//! over pipes and speaks the protocol like Runnel does (initialize, describe,
//! action.execute, connection.test, `$/cancelRequest`, shutdown).
//!
//! ```
//! # use runnel_plugin::{Action, Plugin};
//! # let mut p = Plugin::new("demo", "1.0.0");
//! # p.action(Action::new("demo.echo", "Echo", "Returns its input", |_c, _cfg: runnel_plugin::NoConfig| Ok("hi")));
//! let c = runnel_plugin::testing::Client::start(&p);
//! let out = c.execute("demo.echo", serde_json::json!({})).unwrap();
//! assert_eq!(out, "hi");
//! ```
//!
//! The client does not check configurations against their schema, as Runnel does before a
//! run; tests of required fields belong into tests against Runnel itself ([`Project`]).

mod project;
mod stream;

use std::collections::HashMap;
use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value as Json, json};

use crate::{Plugin, TestResult};

pub use project::{Project, RUNNEL_ENV};
pub use stream::{EMIT_WINDOW, Emitted, StreamResult};

/// How long the client waits for an answer.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// An error answer of the plugin (`code` and `message`), or a failure of the client itself
/// (code 0), e.g. no answer in time.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

fn client_error(message: impl Into<String>) -> RpcError {
    RpcError {
        code: 0,
        message: message.into(),
    }
}

/// The answer of a plugin to initialize.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Initialized {
    pub protocol_version: u32,
    pub encoding: String,
    pub name: String,
    pub version: String,
    pub concurrency: usize,
}

pub(crate) struct Shared {
    pub(crate) waiting: HashMap<u64, mpsc::Sender<Result<Json, RpcError>>>,
    pub(crate) log: Vec<String>,
    /// Breaches of the protocol by the plugin; the client fails when it closes.
    pub(crate) violations: Vec<String>,
    /// Error answers without id, e.g. to a line that is not JSON.
    pub(crate) stray_errors: Vec<RpcError>,
    pub(crate) streams: HashMap<u64, mpsc::Sender<stream::Event>>,
    pub(crate) ended: bool,
}

/// Talks to a plugin served in the test process. When it is dropped, it sends shutdown,
/// closes stdin and waits until the plugin ended; it panics if the plugin broke the
/// protocol (e.g. wrote something other than JSON-RPC on stdout).
pub struct Client {
    input: Mutex<Option<std::io::PipeWriter>>,
    pub(crate) shared: Arc<Mutex<Shared>>,
    next_id: Mutex<u64>,
    next_stream: Mutex<u64>,
    serve: Option<JoinHandle<std::io::Result<()>>>,
    readers: Vec<JoinHandle<()>>,
    /// The answer to initialize.
    pub initialized: Initialized,
}

/// A request sent with [`Client::start_call`] whose answer is still to come.
pub struct PendingCall<'a> {
    client: &'a Client,
    id: u64,
    method: String,
    rx: mpsc::Receiver<Result<Json, RpcError>>,
}

impl PendingCall<'_> {
    /// Sends `$/cancelRequest` for the request, as Runnel does when a run is cancelled.
    pub fn cancel(&self) {
        self.client
            .write(&json!({"jsonrpc": "2.0", "method": "$/cancelRequest", "params": {"id": self.id}}));
    }

    /// Waits for the answer: the result, or the error answer.
    pub fn wait(self) -> Result<Json, RpcError> {
        match self.rx.recv_timeout(TIMEOUT) {
            Ok(r) => r,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                Err(client_error(format!("no answer to {} within {TIMEOUT:?}", self.method)))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(client_error("the plugin ended")),
        }
    }
}

impl Client {
    /// Serves p in a thread, sends initialize and returns the client.
    pub fn start(p: &Plugin) -> Client {
        let (in_r, in_w) = std::io::pipe().expect("pipe");
        let (out_r, out_w) = std::io::pipe().expect("pipe");
        let (log_r, log_w) = std::io::pipe().expect("pipe");
        let shared = Arc::new(Mutex::new(Shared {
            waiting: HashMap::new(),
            log: Vec::new(),
            violations: Vec::new(),
            stray_errors: Vec::new(),
            streams: HashMap::new(),
            ended: false,
        }));
        let plugin = p.clone();
        let serve = std::thread::spawn(move || plugin.serve(in_r, out_w, log_w));
        let s = Arc::clone(&shared);
        let log_reader = std::thread::spawn(move || {
            for line in BufReader::new(log_r).lines() {
                let Ok(line) = line else { break };
                s.lock().unwrap().log.push(line);
            }
        });
        let s = Arc::clone(&shared);
        let answers = std::thread::spawn(move || read_answers(out_r, &s));
        let mut c = Client {
            input: Mutex::new(Some(in_w)),
            shared,
            next_id: Mutex::new(0),
            next_stream: Mutex::new(0),
            serve: Some(serve),
            readers: vec![log_reader, answers],
            initialized: Initialized::default(),
        };
        let raw = c
            .call(
                "initialize",
                json!({"protocolVersion": 1, "encodings": ["json"], "runnel": {"version": "0.0.0-test"}}),
            )
            .expect("initialize");
        c.initialized = serde_json::from_value(raw).expect("answer to initialize");
        c
    }

    pub(crate) fn write(&self, v: &Json) {
        let mut data = serde_json::to_vec(v).expect("JSON");
        data.push(b'\n');
        if let Some(w) = self.input.lock().unwrap().as_mut() {
            let _ = w.write_all(&data);
        }
    }

    /// Writes a raw line to stdin of the plugin, e.g. to test invalid JSON.
    pub fn write_line(&self, line: &str) {
        if let Some(w) = self.input.lock().unwrap().as_mut() {
            let _ = w.write_all(format!("{line}\n").as_bytes());
        }
    }

    pub(crate) fn next_stream_id(&self) -> u64 {
        let mut n = self.next_stream.lock().unwrap();
        *n += 1;
        *n
    }

    /// Sends a request and returns it without waiting for the answer.
    pub fn start_call(&self, method: &str, params: Json) -> PendingCall<'_> {
        let id = {
            let mut n = self.next_id.lock().unwrap();
            *n += 1;
            *n
        };
        let (tx, rx) = mpsc::channel();
        {
            let mut s = self.shared.lock().unwrap();
            if s.ended {
                let _ = tx.send(Err(client_error("the plugin ended")));
            }
            s.waiting.insert(id, tx);
        }
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        PendingCall {
            client: self,
            id,
            method: method.to_string(),
            rx,
        }
    }

    /// Sends a request and waits for its answer: the result, or the error answer.
    pub fn call(&self, method: &str, params: Json) -> Result<Json, RpcError> {
        self.start_call(method, params).wait()
    }

    /// The answer to describe.
    pub fn describe(&self) -> Result<Json, RpcError> {
        self.call("describe", json!({}))
    }

    /// Runs an action with config (as Runnel resolved it) and returns its output; an error
    /// of the action is an [`RpcError`] with its message.
    pub fn execute(&self, type_: &str, config: Json) -> Result<Json, RpcError> {
        let r = self.call("action.execute", execute_params(type_, config))?;
        Ok(r.get("output").cloned().unwrap_or(Json::Null))
    }

    /// Starts an action without waiting, e.g. to cancel it with [`PendingCall::cancel`].
    pub fn start_execute(&self, type_: &str, config: Json) -> PendingCall<'_> {
        self.start_call("action.execute", execute_params(type_, config))
    }

    /// Calls connection.test with a resolved connection (name, type and fields).
    pub fn test_connection(&self, conn: Json) -> Result<TestResult, RpcError> {
        let t = conn.get("type").cloned().unwrap_or(Json::Null);
        let r = self.call("connection.test", json!({"type": t, "connection": conn}))?;
        serde_json::from_value(r).map_err(|e| client_error(format!("invalid answer to connection.test: {e}")))
    }

    /// The lines the plugin wrote to stderr so far.
    pub fn log(&self) -> Vec<String> {
        self.shared.lock().unwrap().log.clone()
    }

    /// Error answers without id the plugin sent so far, e.g. to a line that is not JSON.
    pub fn stray_errors(&self) -> Vec<RpcError> {
        self.shared.lock().unwrap().stray_errors.clone()
    }

    /// Waits briefly until a line of the log contains text.
    pub fn log_contains(&self, text: &str) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            if self.log().iter().any(|l| l.contains(text)) {
                return true;
            }
            if std::time::Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Sends shutdown, closes stdin and waits until the plugin ended. Returns the error
    /// the plugin ended with, and the breaches of the protocol the client saw.
    pub fn close(mut self) -> Result<(), String> {
        self.finish()
    }

    fn finish(&mut self) -> Result<(), String> {
        let Some(serve) = self.serve.take() else {
            return Ok(());
        };
        let _ = self.call("shutdown", json!({}));
        self.input.lock().unwrap().take(); // closes stdin
        let result = serve.join().map_err(|_| "the plugin panicked".to_string())?;
        for r in self.readers.drain(..) {
            let _ = r.join();
        }
        result.map_err(|e| format!("plugin ended with error: {e}"))?;
        let violations = std::mem::take(&mut self.shared.lock().unwrap().violations);
        if !violations.is_empty() {
            return Err(format!("the plugin broke the protocol: {}", violations.join("; ")));
        }
        Ok(())
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let r = self.finish();
        if let Err(e) = r
            && !std::thread::panicking()
        {
            panic!("{e}");
        }
    }
}

/// Checks the references of all actions and transforms of a plugin, as Runnel checks those
/// of its built-in types (ADR 0014, E15b): `errors` for every type, `records` for every
/// transform, texts as sentences with a period, output types that exist, output names that
/// are paths of the output or name fields of the configuration (`<field>`). Plugins call it
/// in a test if they want to; the protocol does not require references.
///
/// ```
/// # let p = runnel_plugin::Plugin::new("demo", "1.0.0");
/// runnel_plugin::testing::check_references(&p).unwrap();
/// ```
pub fn check_references(p: &Plugin) -> Result<(), String> {
    let mut problems = Vec::new();
    for a in &p.actions {
        for m in check_reference("action", &a.descriptor.reference, &a.descriptor.schema) {
            problems.push(format!("action {}: {m}", a.descriptor.type_));
        }
    }
    for t in &p.transforms {
        for m in check_reference("transform", &t.descriptor.reference, &t.descriptor.schema) {
            problems.push(format!("transform {}: {m}", t.descriptor.type_));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

/// Checks one reference of an action (`kind` "action") or transform ("transform") against
/// the schema of its configuration; see [`check_references`]. Returns the mistakes.
pub fn check_reference(kind: &str, reference: &crate::Reference, schema: &crate::ConfigSchema) -> Vec<String> {
    crate::reference::check(kind, reference, schema)
}

fn execute_params(type_: &str, config: Json) -> Json {
    json!({"type": type_, "config": config, "context": {"runId": "01TESTRUN", "actionId": "test", "projectDir": "."}})
}

fn read_answers(out: std::io::PipeReader, shared: &Mutex<Shared>) {
    let mut reader = BufReader::new(out);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches(['\n', '\r']);
        let m: Json = match serde_json::from_str::<Json>(text) {
            Ok(m) if m.get("jsonrpc") == Some(&json!("2.0")) => m,
            _ => {
                shared
                    .lock()
                    .unwrap()
                    .violations
                    .push(format!("not JSON-RPC on stdout: {text:?}"));
                continue;
            }
        };
        match m.get("id").and_then(Json::as_u64) {
            Some(id) => {
                let tx = shared.lock().unwrap().waiting.remove(&id);
                let r = match m.get("error") {
                    Some(e) => Err(serde_json::from_value(e.clone()).unwrap_or_else(|_| client_error(e.to_string()))),
                    None => Ok(m.get("result").cloned().unwrap_or(Json::Null)),
                };
                if let Some(tx) = tx {
                    let _ = tx.send(r);
                }
            }
            None if m.get("error").is_some() && m.get("id").is_some_and(Json::is_null) => {
                let e = m.get("error").cloned().unwrap_or_default();
                let e = serde_json::from_value(e.clone()).unwrap_or_else(|_| client_error(e.to_string()));
                shared.lock().unwrap().stray_errors.push(e);
            }
            None if m.get("id").is_some_and(|id| !id.is_null()) => {
                shared
                    .lock()
                    .unwrap()
                    .violations
                    .push(format!("answer with unknown id: {text}"));
            }
            None => stream::notification(shared, &m),
        }
    }
    let mut s = shared.lock().unwrap();
    s.ended = true;
    for (_, tx) in s.waiting.drain() {
        let _ = tx.send(Err(client_error("the plugin ended")));
    }
    s.streams.clear();
}
