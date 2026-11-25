//! Transforms as streams: batches in, records out, with credits in both directions
//! (ADR 0019 of Runnel, Nachtrag S50a).

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use serde_json::value::RawValue;

use crate::serve::{RunContext, Server, byte_size, fits, safely};
use crate::value::{Field, Row, decode_row};
use crate::{Cancel, Config, ConfigSchema, Error, Logger, MAX_MESSAGE_SIZE, Plugin, code, decode};

/// Categories of transforms in the palette of the editor (besides
/// [`category::EXECUTION`]).
pub mod transform_category {
    pub const INPUT: &str = "input";
    pub const TRANSFORM: &str = "transform";
    pub const FLOW: &str = "flow";
    pub const COMBINE: &str = "combine";
    pub const OUTPUT: &str = "output";

    pub(crate) const ALL: &[&str] = &[INPUT, TRANSFORM, FLOW, crate::category::EXECUTION, COMBINE, OUTPUT];
}

/// All data links that do not lead to a target of the configuration.
pub const TO_MAIN: &str = "main";
/// The error link: records with `_error` and `_errorField` (see [`crate::error_fields`]).
pub const TO_ERROR: &str = "error";

/// How many batches the SDK takes from Runnel before the transform has read them.
pub const STREAM_WINDOW: usize = 4;

/// A batch of records as Runnel sends them: rows follow the order of the fields.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Batch {
    pub fields: Vec<Field>,
    pub rows: Vec<Row>,
}

type RunFn<C> = dyn Fn(&mut Stream, C) -> Result<(), Error> + Send + Sync;
type StreamFn = dyn Fn(&mut Stream, Option<&RawValue>) -> Result<(), Error> + Send + Sync;

/// A transform of the plugin with its configuration type C. Create it with
/// [`Transform::new`] and add it with [`Plugin::transform`]. The same type may also be an
/// action.
pub struct Transform<C> {
    type_: String,
    category: String,
    label: String,
    description: String,
    example: String,
    record_errors: bool,
    parallel: bool,
    run: Arc<RunFn<C>>,
}

impl<C: Config> Transform<C> {
    /// A transform: type `"<namespace>.<name>"`, label, description and the function that
    /// processes one stream. `run` reads batches with [`Stream::next`] until there are no
    /// more and writes records with [`Stream::emit`]; an input (no incoming links) does not
    /// call `next` and only emits. Returning ends the stream; an error fails the transform
    /// and the pipeline. `run` should stop when [`Stream::is_cancelled`] becomes true.
    pub fn new<F>(type_: &str, label: &str, description: &str, run: F) -> Transform<C>
    where
        F: Fn(&mut Stream, C) -> Result<(), Error> + Send + Sync + 'static,
    {
        Transform {
            type_: type_.to_string(),
            category: String::new(),
            label: label.to_string(),
            description: description.to_string(),
            example: String::new(),
            record_errors: false,
            parallel: false,
            run: Arc::new(run),
        }
    }

    /// Optional: one of [`transform_category`] (or [`category::EXECUTION`]) puts the
    /// transform into that group of the palette; without, it is in the group of the plugin
    /// and behaves like one of category transform. An input needs
    /// [`transform_category::INPUT`], so that the editor offers no link into it.
    pub fn category(mut self, category: &str) -> Self {
        self.category = category.to_string();
        self
    }

    /// Optional YAML of the transform for the help in the editor.
    pub fn example(mut self, yaml: &str) -> Self {
        self.example = yaml.to_string();
        self
    }

    /// The transform may have an error link (`emit` to [`TO_ERROR`]).
    pub fn record_errors(mut self) -> Self {
        self.record_errors = true;
        self
    }

    /// The transform may run in several copies (`copies:` in the pipeline).
    pub fn parallel(mut self) -> Self {
        self.parallel = true;
        self
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TransformDescriptor {
    #[serde(rename = "type")]
    pub(crate) type_: String,
    kind: &'static str,
    #[serde(skip_serializing_if = "String::is_empty")]
    category: String,
    label: String,
    description: String,
    schema: ConfigSchema,
    #[serde(skip_serializing_if = "String::is_empty")]
    example: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    record_errors: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    parallel: bool,
}

#[derive(Clone)]
pub(crate) struct TransformEntry {
    pub(crate) descriptor: TransformDescriptor,
    run: Arc<StreamFn>,
}

impl Plugin {
    /// Adds a transform. Like [`Plugin::action`], it panics on mistakes in the definition.
    pub fn transform<C: Config>(&mut self, t: Transform<C>) -> &mut Self {
        self.check_type("transform", &t.type_);
        assert!(
            !self.transforms.iter().any(|e| e.descriptor.type_ == t.type_),
            "runnel: transform type {:?} added twice",
            t.type_
        );
        assert!(
            t.category.is_empty() || transform_category::ALL.contains(&t.category.as_str()),
            "runnel: transform {:?}: unknown category {:?}",
            t.type_,
            t.category
        );
        assert!(
            !t.label.is_empty() && !t.description.is_empty(),
            "runnel: transform {:?} needs a label and a description",
            t.type_
        );
        let schema = C::schema().unwrap_or_else(|e| panic!("runnel: transform {:?}: {e}", t.type_));
        let s = schema.clone();
        let run = t.run.clone();
        self.transforms.push(TransformEntry {
            descriptor: TransformDescriptor {
                type_: t.type_,
                kind: "transform",
                category: t.category,
                label: t.label,
                description: t.description,
                schema,
                example: t.example,
                record_errors: t.record_errors,
                parallel: t.parallel,
            },
            run: Arc::new(move |stream, raw| {
                let c: C = decode::decode(raw, &s)?;
                run(stream, c)
            }),
        });
        self
    }
}

/// What a stream shares with the reader of stdin.
pub(crate) struct Shared {
    state: Mutex<StreamState>,
    cv: Condvar,
    cancel: Cancel,
}

#[derive(Default)]
struct StreamState {
    /// transform.batch params; None for transform.end.
    queue: VecDeque<Option<Box<RawValue>>>,
    batches: usize,
    /// emits Runnel still takes
    credits: i64,
    /// set when Runnel did not keep to the protocol; ends the stream
    broken: Option<String>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, StreamState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Ends the stream: its run sees the cancellation.
    pub(crate) fn cancel(&self) {
        self.cancel.cancel();
        let _guard = self.lock();
        self.cv.notify_all();
    }

    /// Ends the stream with an error of the protocol, which transform.closed reports
    /// instead of what run returns.
    fn fail(&self, err: String) {
        {
            let mut st = self.lock();
            if st.broken.is_none() {
                st.broken = Some(err);
            }
        }
        self.cancel();
    }
}

/// One running transform: the batches it reads and the records it writes, with the credits
/// of the protocol in both directions. Runnel sends at most [`STREAM_WINDOW`] batches that
/// [`Stream::next`] has not returned yet, and [`Stream::emit`] waits until Runnel took the
/// earlier records, so a slow transform behind the plugin slows the plugin down.
pub struct Stream {
    /// ID of the run.
    pub run_id: String,
    /// id of the transform in the pipeline.
    pub transform_id: String,
    /// Project folder; also the working directory of the plugin.
    pub project_dir: String,
    /// Name of the environment, empty without one.
    pub environment: String,
    /// Writes to the log of the run.
    pub log: Logger,
    id: u64,
    srv: Arc<Server>,
    shared: Arc<Shared>,
    fields: Vec<Field>,
}

#[derive(Deserialize)]
struct BatchParams {
    #[serde(default)]
    fields: Option<Vec<Field>>,
    #[serde(default)]
    rows: Vec<Vec<Json>>,
}

#[derive(Serialize)]
struct EmitMessage<'a> {
    jsonrpc: &'static str,
    method: &'static str,
    params: EmitParams<'a>,
}

#[derive(Serialize)]
struct EmitParams<'a> {
    stream: u64,
    to: &'a str,
    fields: &'a [Field],
    rows: &'a [Row],
}

impl Stream {
    /// Whether Runnel cancelled the stream (or the plugin ends); `run` should stop.
    pub fn is_cancelled(&self) -> bool {
        self.shared.cancel.is_cancelled()
    }

    /// The cancellation, e.g. to wait with [`Cancel::sleep`].
    pub fn cancel(&self) -> &Cancel {
        &self.shared.cancel
    }

    fn interrupted(&self, st: &StreamState) -> Option<Error> {
        if let Some(b) = &st.broken {
            return Some(b.clone().into());
        }
        self.shared
            .cancel
            .is_cancelled()
            .then(|| "the stream was cancelled".into())
    }

    /// The next batch, or None after the last one.
    #[allow(clippy::should_implement_trait)] // returns a Result, unlike Iterator::next
    pub fn next(&mut self) -> Result<Option<Batch>, Error> {
        let raw = {
            let mut st = self.shared.lock();
            loop {
                if let Some(e) = self.interrupted(&st) {
                    return Err(e);
                }
                match st.queue.pop_front() {
                    Some(Some(raw)) => {
                        st.batches -= 1;
                        break raw;
                    }
                    Some(None) => {
                        st.queue.push_front(None); // next keeps answering None
                        return Ok(None);
                    }
                    None => st = self.shared.cv.wait(st).unwrap_or_else(|e| e.into_inner()),
                }
            }
        };
        // The batch has left the buffer: Runnel may send the next one.
        self.srv
            .notify("$/ack", serde_json::json!({"stream": self.id, "batches": 1}));
        let p: BatchParams = serde_json::from_str(raw.get()).map_err(|e| format!("invalid transform.batch: {e}"))?;
        if let Some(f) = p.fields {
            self.fields = f;
        }
        let mut rows = Vec::with_capacity(p.rows.len());
        for (i, r) in p.rows.into_iter().enumerate() {
            rows.push(decode_row(r, &self.fields).map_err(|e| format!("row {} of a batch: {e}", i + 1))?);
        }
        Ok(Some(Batch {
            fields: self.fields.clone(),
            rows,
        }))
    }

    /// Writes rows with the given fields to a target: [`TO_MAIN`], [`TO_ERROR`] or the id
    /// of a transform from a field with format `runnel-target`. It waits until Runnel takes
    /// more records. Rows that do not fit into one message (16 MiB) are sent in several; a
    /// single record larger than that is an error.
    pub fn emit(&self, to: &str, fields: &[Field], rows: &[Row]) -> Result<(), Error> {
        if rows.is_empty() {
            return Ok(());
        }
        // fields go with every emit: the protocol allows leaving them out when they did not
        // change, but Runnel before runnel#228 reads such an emit as one without fields.
        let msg = EmitMessage {
            jsonrpc: "2.0",
            method: "transform.emit",
            params: EmitParams {
                stream: self.id,
                to,
                fields,
                rows,
            },
        };
        let data = serde_json::to_vec(&msg).map_err(|e| format!("cannot encode the records for {to:?}: {e}"))?;
        if !fits(&data) {
            if rows.len() == 1 {
                return Err(format!(
                    "a record for {to:?} has {}, more than the {} a message may have",
                    byte_size(data.len() + 1),
                    byte_size(MAX_MESSAGE_SIZE)
                )
                .into());
            }
            let (a, b) = rows.split_at(rows.len() / 2);
            self.emit(to, fields, a)?;
            return self.emit(to, fields, b);
        }
        {
            let mut st = self.shared.lock();
            while st.credits <= 0 {
                if let Some(e) = self.interrupted(&st) {
                    return Err(e);
                }
                st = self.shared.cv.wait(st).unwrap_or_else(|e| e.into_inner());
            }
            st.credits -= 1;
        }
        self.srv.send_raw(&data);
        Ok(())
    }
}

impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Stream({}, {})", self.id, self.transform_id)
    }
}

impl Server {
    pub(crate) fn describe_transforms(&self, result: &mut serde_json::Map<String, Json>) {
        if !self.p.transforms.is_empty() {
            let ts: Vec<_> = self.p.transforms.iter().map(|t| &t.descriptor).collect();
            result.insert("transforms".into(), serde_json::to_value(ts).unwrap_or_default());
        }
    }

    /// Requests of transforms; false for an unknown method.
    pub(crate) fn request(self: &Arc<Self>, method: &str, id: &RawValue, params: Option<&RawValue>) -> bool {
        match method {
            "transform.open" => self.open_stream(id, params),
            _ => return false,
        }
        true
    }

    fn open_stream(self: &Arc<Self>, id: &RawValue, params: Option<&RawValue>) {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct P {
            stream: u64,
            #[serde(rename = "type", default)]
            type_: String,
            #[serde(default)]
            config: Option<Box<RawValue>>,
            #[serde(default)]
            context: RunContext,
            #[serde(default)]
            emit_window: Option<i64>,
        }
        let Some(p) = params.and_then(|p| serde_json::from_str::<P>(p.get()).ok()) else {
            return self.fail(
                Some(id),
                code::INVALID_PARAMS,
                "invalid parameters of transform.open".into(),
            );
        };
        let Some(entry) = self
            .p
            .transforms
            .iter()
            .find(|t| t.descriptor.type_ == p.type_)
            .cloned()
        else {
            return self.fail(
                Some(id),
                code::INVALID_PARAMS,
                format!("unknown transform type {:?}", p.type_),
            );
        };
        let shared = Arc::new(Shared {
            state: Mutex::new(StreamState {
                credits: p.emit_window.unwrap_or(4),
                ..Default::default()
            }),
            cv: Condvar::new(),
            cancel: Cancel::new(),
        });
        {
            let mut st = self.lock();
            if st.streams.contains_key(&p.stream) {
                drop(st);
                return self.fail(
                    Some(id),
                    code::INVALID_PARAMS,
                    format!("stream {} is open already", p.stream),
                );
            }
            st.streams.insert(p.stream, Arc::clone(&shared));
        }
        self.reply(id, serde_json::json!({"window": STREAM_WINDOW}));
        let mut stream = Stream {
            run_id: p.context.run_id,
            transform_id: p.context.transform_id,
            project_dir: p.context.project_dir,
            environment: p.context.environment,
            log: self.log.clone(),
            id: p.stream,
            srv: Arc::clone(self),
            shared: Arc::clone(&shared),
            fields: Vec::new(),
        };
        let srv = Arc::clone(self);
        let config = p.config;
        let type_ = p.type_;
        self.spawn(move || {
            let result = safely(&srv.log, &type_, || (entry.run)(&mut stream, config.as_deref()));
            drop(stream);
            srv.lock().streams.remove(&p.stream);
            // The cause, not the cancellation it led to.
            let broken = shared.lock().broken.clone();
            let error = broken.or_else(|| result.err().map(|e| e.to_string()));
            #[derive(Serialize)]
            struct Closed {
                stream: u64,
                #[serde(skip_serializing_if = "Option::is_none")]
                error: Option<Message>,
            }
            #[derive(Serialize)]
            struct Message {
                message: String,
            }
            srv.notify(
                "transform.closed",
                Closed {
                    stream: p.stream,
                    error: error.map(|message| Message { message }),
                },
            );
        });
    }

    /// transform.batch, transform.end and `$/ack` of a stream. It never blocks: Runnel
    /// sends at most STREAM_WINDOW batches ahead; more is an error of the host, which ends
    /// the stream with an error instead of losing records.
    pub(crate) fn stream_notification(&self, method: &str, params: Option<&RawValue>) {
        #[derive(Deserialize)]
        struct P {
            stream: u64,
            #[serde(default)]
            batches: i64,
        }
        if !matches!(method, "transform.batch" | "transform.end" | "$/ack") {
            return; // unknown notifications are ignored
        }
        let Some(params) = params else { return };
        let Ok(p) = serde_json::from_str::<P>(params.get()) else {
            return;
        };
        let Some(shared) = self.lock().streams.get(&p.stream).cloned() else {
            return;
        };
        let mut st = shared.lock();
        match method {
            "$/ack" if p.batches > 0 => st.credits += p.batches,
            "transform.batch" if st.broken.is_none() => {
                if st.batches >= STREAM_WINDOW {
                    drop(st);
                    shared.fail(format!(
                        "more than {STREAM_WINDOW} batches arrived from Runnel without waiting for $/ack; the records cannot be processed completely"
                    ));
                    return;
                }
                st.batches += 1;
                st.queue.push_back(Some(params.to_owned()));
            }
            "transform.end" => st.queue.push_back(None),
            _ => return,
        }
        shared.cv.notify_all();
    }
}
