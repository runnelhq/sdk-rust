//! Streams of transforms in the test client, with credits like Runnel.

use std::collections::HashMap;
use std::sync::mpsc;

use serde_json::{Map, Value as Json, json};

use super::{Client, RpcError, Shared, TIMEOUT, client_error};
use crate::transform::Batch;
use crate::value::Field;

/// The number of transform.emit the client takes before it acknowledges them, like Runnel.
pub const EMIT_WINDOW: usize = 4;

pub(crate) enum Event {
    Ack(usize),
    Emit {
        to: String,
        fields: Option<Vec<Field>>,
        rows: Vec<Vec<Json>>,
    },
    Closed(Option<String>),
}

/// One transform.emit of a plugin, with the fields that apply to its rows (values as JSON).
#[derive(Debug, Clone, PartialEq)]
pub struct Emitted {
    pub to: String,
    pub fields: Vec<Field>,
    pub rows: Vec<Vec<Json>>,
}

/// Everything a transform wrote, and the error it closed with.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StreamResult {
    pub emits: Vec<Emitted>,
    /// The message of transform.closed, None if the transform succeeded.
    pub error: Option<String>,
}

impl StreamResult {
    /// The rows sent to a target (`TO_MAIN`, `TO_ERROR` or an id) as maps from field name
    /// to value, in the order of the fields.
    pub fn records(&self, to: &str) -> Vec<Map<String, Json>> {
        let mut out = Vec::new();
        for e in self.emits.iter().filter(|e| e.to == to) {
            for row in &e.rows {
                out.push(
                    e.fields
                        .iter()
                        .zip(row)
                        .map(|(f, v)| (f.name.clone(), v.clone()))
                        .collect(),
                );
            }
        }
        out
    }
}

pub(crate) fn notification(shared: &std::sync::Mutex<Shared>, m: &Json) {
    let method = m.get("method").and_then(Json::as_str).unwrap_or_default().to_string();
    let p = m.get("params").cloned().unwrap_or_default();
    let stream = p.get("stream").and_then(Json::as_u64).unwrap_or(0);
    let mut s = shared.lock().unwrap();
    let Some(tx) = s.streams.get(&stream).cloned() else {
        s.violations.push(format!("{method} for unknown stream {stream}"));
        return;
    };
    let event = match method.as_str() {
        "$/ack" => Event::Ack(p.get("batches").and_then(Json::as_u64).unwrap_or(0) as usize),
        "transform.emit" => {
            let fields = match p.get("fields") {
                Some(f) => match serde_json::from_value(f.clone()) {
                    Ok(f) => Some(f),
                    Err(e) => {
                        s.violations.push(format!("invalid fields of transform.emit: {e}"));
                        return;
                    }
                },
                None => None,
            };
            let rows = match serde_json::from_value(p.get("rows").cloned().unwrap_or_default()) {
                Ok(r) => r,
                Err(e) => {
                    s.violations.push(format!("invalid rows of transform.emit: {e}"));
                    return;
                }
            };
            let to = p.get("to").and_then(Json::as_str).unwrap_or("main").to_string();
            Event::Emit { to, fields, rows }
        }
        "transform.closed" => Event::Closed(p.pointer("/error/message").and_then(Json::as_str).map(str::to_string)),
        _ => {
            s.violations.push(format!("unexpected notification {method}"));
            return;
        }
    };
    let _ = tx.send(event);
}

impl Client {
    /// Runs a transform like Runnel: transform.open, the batches as long as the plugin
    /// grants credits, transform.end, and collects transform.emit until transform.closed.
    /// For an input, pass no batches. Breaches of the protocol (more emits than credits,
    /// rows without fields) fail the client when it closes.
    pub fn run_transform(&self, type_: &str, config: Json, batches: &[Batch]) -> Result<StreamResult, RpcError> {
        let id = self.next_stream_id();
        let (tx, rx) = mpsc::channel();
        self.shared.lock().unwrap().streams.insert(id, tx);
        let result = self.drive(id, type_, config, batches, &rx);
        self.shared.lock().unwrap().streams.remove(&id);
        result
    }

    fn drive(
        &self,
        id: u64,
        type_: &str,
        config: Json,
        batches: &[Batch],
        rx: &mpsc::Receiver<Event>,
    ) -> Result<StreamResult, RpcError> {
        let open = self.call(
            "transform.open",
            json!({"stream": id, "type": type_, "config": config,
                "context": {"runId": "01TESTRUN", "transformId": "test", "projectDir": "."}, "emitWindow": EMIT_WINDOW}),
        )?;
        let window = open.get("window").and_then(Json::as_u64).unwrap_or(0) as usize;
        if !(1..=64).contains(&window) {
            return Err(client_error(format!(
                "transform.open answered window {window} (1 to 64)"
            )));
        }
        let mut credits = window;
        let mut next = 0;
        let mut ended = false;
        let mut last: Option<&[Field]> = None;
        let mut res = StreamResult::default();
        let mut fields: Option<Vec<Field>> = None;
        let mut by_target: HashMap<String, usize> = HashMap::new();
        loop {
            // Send what the credits allow, then wait for the plugin.
            while credits > 0 && next < batches.len() {
                let b = &batches[next];
                let mut params = json!({"stream": id, "rows": b.rows});
                if last != Some(b.fields.as_slice()) {
                    params["fields"] = serde_json::to_value(&b.fields).unwrap_or_default();
                    last = Some(&b.fields);
                }
                self.write(&json!({"jsonrpc": "2.0", "method": "transform.batch", "params": params}));
                credits -= 1;
                next += 1;
            }
            if next == batches.len() && !ended {
                self.write(&json!({"jsonrpc": "2.0", "method": "transform.end", "params": {"stream": id}}));
                ended = true;
            }
            let event = match rx.recv_timeout(TIMEOUT) {
                Ok(e) => e,
                Err(mpsc::RecvTimeoutError::Timeout) if next < batches.len() && credits == 0 => {
                    return Err(client_error("the plugin grants no credits for batches ($/ack)"));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(client_error(format!(
                        "stream {id}: no transform.closed within {TIMEOUT:?}"
                    )));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(client_error("the plugin ended")),
            };
            match event {
                Event::Ack(n) => credits += n,
                Event::Emit { to, fields: f, rows } => {
                    if f.is_some() {
                        fields = f;
                    }
                    let Some(fs) = fields.clone() else {
                        self.shared
                            .lock()
                            .unwrap()
                            .violations
                            .push(format!("stream {id}: transform.emit without fields"));
                        continue;
                    };
                    *by_target.entry(to.clone()).or_default() += rows.len();
                    res.emits.push(Emitted { to, fields: fs, rows });
                    // Runnel acknowledges once the records are with the next transforms;
                    // the client has them at once. EMIT_WINDOW holds since every emit is
                    // acknowledged before the next is read.
                    self.write(&json!({"jsonrpc": "2.0", "method": "$/ack", "params": {"stream": id, "batches": 1}}));
                }
                Event::Closed(error) => {
                    res.error = error;
                    return Ok(res);
                }
            }
        }
    }
}
