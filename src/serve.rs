//! The protocol loop: reads requests from stdin, answers on stdout, one message per line.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, Once};
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as Json;
use serde_json::value::RawValue;

use crate::{Call, Cancel, Context, Error, Logger, PROTOCOL_VERSION, Plugin, code};

/// The largest message of the protocol: 16 MiB per line (ADR 0019).
pub const MAX_MESSAGE_SIZE: usize = 16 << 20;

impl Plugin {
    /// Serves the plugin on stdin and stdout, as Runnel starts it, and exits the process:
    /// with 0 after Runnel closed stdin, with 1 on an error of the protocol. Started
    /// without `RUNNEL_PLUGIN=1` (e.g. by a double click), it prints a hint and exits with
    /// 2 instead of waiting on stdin.
    ///
    /// stdout belongs to the protocol alone: the plugin must not print to it (`println!`);
    /// use the logger of the call or `eprintln!` instead.
    pub fn main(&self) -> ! {
        if std::env::var("RUNNEL_PLUGIN").as_deref() != Ok("1") {
            eprintln!(
                "{} {} is a Runnel plugin: put its folder into plugins/ of a project and run \"runnel plugins trust\".",
                self.name(),
                self.version()
            );
            std::process::exit(2);
        }
        match self.serve(io::stdin().lock(), io::stdout(), io::stderr()) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("ERROR {e}");
                std::process::exit(1);
            }
        }
    }

    /// Speaks the plugin protocol: reads requests from `input`, writes answers to `output`
    /// and the log to `log`, until `input` ends. Requests run in their own threads, at most
    /// [`Plugin::concurrency`] at a time; when `serve` returns, all of them have ended
    /// (they are cancelled when `input` ends). Tests can call it with pipes; the module
    /// [`testing`](crate::testing) does that for them.
    pub fn serve(
        &self,
        input: impl Read,
        output: impl Write + Send + 'static,
        log: impl Write + Send + 'static,
    ) -> io::Result<()> {
        install_panic_hook();
        let srv = Arc::new(Server {
            p: self.clone(),
            out: Mutex::new(BufWriter::with_capacity(64 << 10, Box::new(output))),
            log: Logger::new(log),
            state: Mutex::new(State::default()),
            slots: Semaphore::new(self.concurrency_value()),
            tasks: WaitGroup::default(),
        });
        let result = read_lines(input, |line| srv.handle(line));
        // The host is gone or broke the protocol: whatever still runs is cancelled.
        let (cancels, streams): (Vec<Cancel>, Vec<_>) = {
            let st = srv.lock();
            (
                st.running.values().cloned().collect(),
                st.streams.values().cloned().collect(),
            )
        };
        for c in cancels {
            c.cancel();
        }
        for s in streams {
            s.cancel();
        }
        srv.tasks.wait();
        result
    }
}

/// Reads lines of at most MAX_MESSAGE_SIZE bytes (a `\r` before `\n` is dropped) and hands
/// each non-empty one to f.
fn read_lines(input: impl Read, mut f: impl FnMut(&[u8])) -> io::Result<()> {
    let mut reader = BufReader::with_capacity(64 << 10, input);
    let mut buf = Vec::with_capacity(64 << 10);
    let limit = (MAX_MESSAGE_SIZE + 2) as u64;
    loop {
        buf.clear();
        let n = (&mut reader).take(limit).read_until(b'\n', &mut buf)?;
        if n == 0 {
            return Ok(());
        }
        if buf.last() == Some(&b'\n') {
            buf.pop();
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
        } else if n as u64 == limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("message longer than {MAX_MESSAGE_SIZE} bytes"),
            ));
        }
        if buf.iter().any(|b| !b.is_ascii_whitespace()) {
            f(&buf);
        }
    }
}

pub(crate) struct Server {
    pub(crate) p: Plugin,
    out: Mutex<BufWriter<Box<dyn Write + Send>>>,
    pub(crate) log: Logger,
    state: Mutex<State>,
    slots: Semaphore,
    pub(crate) tasks: WaitGroup,
}

#[derive(Default)]
pub(crate) struct State {
    initialized: bool,
    running: HashMap<String, Cancel>,
    pub(crate) streams: HashMap<u64, Arc<crate::transform::Shared>>,
}

#[derive(Deserialize)]
struct Incoming<'a> {
    #[serde(borrow, default)]
    id: Option<&'a RawValue>,
    #[serde(default)]
    method: Option<String>,
    #[serde(borrow, default)]
    params: Option<&'a RawValue>,
}

#[derive(Serialize)]
struct Reply<'a, T: Serialize> {
    jsonrpc: &'static str,
    id: &'a RawValue,
    result: T,
}

#[derive(Serialize)]
struct Failure<'a> {
    jsonrpc: &'static str,
    id: &'a RawValue,
    error: RpcError,
}

#[derive(Serialize)]
struct RpcError {
    code: i64,
    message: String,
}

#[derive(Serialize)]
pub(crate) struct Notification<T: Serialize> {
    pub(crate) jsonrpc: &'static str,
    pub(crate) method: &'static str,
    pub(crate) params: T,
}

fn null_id() -> &'static RawValue {
    // "null" is valid JSON; the conversion cannot fail.
    static NULL: std::sync::OnceLock<Box<RawValue>> = std::sync::OnceLock::new();
    NULL.get_or_init(|| RawValue::from_string("null".into()).expect("null is JSON"))
}

/// Whether a message (without its newline) is within the limit of Runnel.
pub(crate) fn fits(data: &[u8]) -> bool {
    data.len() < MAX_MESSAGE_SIZE
}

/// Writes a number of bytes for messages, e.g. 16 MiB or 1200 bytes.
pub(crate) fn byte_size(n: usize) -> String {
    if n >= 1 << 20 && n.is_multiple_of(1 << 20) {
        format!("{} MiB", n >> 20)
    } else if n >= 1 << 20 {
        format!("{:.1} MiB", n as f64 / f64::from(1 << 20))
    } else {
        format!("{n} bytes")
    }
}

impl Server {
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Writes an encoded message and its newline.
    pub(crate) fn send_raw(&self, data: &[u8]) {
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        let _ = out.write_all(data);
        let _ = out.write_all(b"\n");
        let _ = out.flush();
    }

    pub(crate) fn notify<T: Serialize>(&self, method: &'static str, params: T) {
        match serde_json::to_vec(&Notification {
            jsonrpc: "2.0",
            method,
            params,
        }) {
            Ok(data) => self.send_raw(&data),
            Err(e) => self.log.error(format_args!("cannot encode {method}: {e}")),
        }
    }

    pub(crate) fn reply<T: Serialize>(&self, id: &RawValue, result: T) {
        let data = match serde_json::to_vec(&Reply {
            jsonrpc: "2.0",
            id,
            result,
        }) {
            Ok(d) => d,
            Err(e) => {
                return self.fail(
                    Some(id),
                    code::ACTION_FAILED,
                    format!("the plugin cannot encode its result: {e}"),
                );
            }
        };
        if !fits(&data) {
            // Runnel would end the plugin for a longer line; the request fails instead.
            return self.fail(
                Some(id),
                code::ACTION_FAILED,
                format!(
                    "the result has {}, more than the {} a message may have; return less, e.g. fewer entries",
                    byte_size(data.len() + 1),
                    byte_size(MAX_MESSAGE_SIZE)
                ),
            );
        }
        self.send_raw(&data);
    }

    pub(crate) fn fail(&self, id: Option<&RawValue>, code: i64, message: String) {
        let id: &RawValue = match id {
            Some(id) => id,
            None => null_id(),
        };
        let f = Failure {
            jsonrpc: "2.0",
            id,
            error: RpcError { code, message },
        };
        if let Ok(data) = serde_json::to_vec(&f) {
            self.send_raw(&data);
        }
    }

    fn handle(self: &Arc<Self>, line: &[u8]) {
        let m: Incoming = match serde_json::from_slice(line) {
            Ok(m) => m,
            Err(_) => return self.fail(None, code::PARSE_ERROR, "invalid JSON".into()),
        };
        let id = m.id.filter(|id| id.get() != "null");
        // Ids are numbers or texts (JSON-RPC); others could not be echoed in valid JSON.
        if id.is_some_and(|id| !serde_json::from_str::<Json>(id.get()).is_ok_and(|v| v.is_number() || v.is_string())) {
            return self.fail(None, code::INVALID_REQUEST, "invalid id".into());
        }
        let Some(method) = m.method.filter(|m| !m.is_empty()) else {
            if id.is_none() {
                self.fail(None, code::INVALID_REQUEST, "message without method".into());
            }
            return; // an answer; Runnel does not answer the plugin
        };
        let Some(id) = id else {
            return self.notification(&method, m.params);
        };
        let initialized = self.lock().initialized;
        if !initialized && method != "initialize" && method != "shutdown" {
            return self.fail(Some(id), code::NOT_INITIALIZED, "initialize first".into());
        }
        match method.as_str() {
            "initialize" => self.initialize(id),
            "describe" => self.describe(id),
            "action.execute" => self.execute(id, m.params),
            "connection.test" => self.test_connection(id, m.params),
            "shutdown" => self.reply(id, Json::Null),
            _ => {
                if !self.request(&method, id, m.params) {
                    self.fail(Some(id), code::METHOD_NOT_FOUND, format!("method {method:?} not found"));
                }
            }
        }
    }

    fn notification(self: &Arc<Self>, method: &str, params: Option<&RawValue>) {
        if method == "$/cancelRequest" {
            #[derive(Deserialize)]
            struct P<'a> {
                #[serde(borrow)]
                id: &'a RawValue,
            }
            let Some(p) = params.and_then(|p| serde_json::from_str::<P>(p.get()).ok()) else {
                return;
            };
            let key = normalize_id(p.id);
            if let Some(c) = self.lock().running.get(&key) {
                c.cancel();
            }
            return;
        }
        self.stream_notification(method, params);
    }

    fn initialize(&self, id: &RawValue) {
        self.lock().initialized = true;
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Answer<'a> {
            protocol_version: u32,
            encoding: &'static str,
            name: &'a str,
            version: &'a str,
            concurrency: usize,
        }
        self.reply(
            id,
            Answer {
                protocol_version: PROTOCOL_VERSION,
                encoding: "json",
                name: self.p.name(),
                version: self.p.version(),
                concurrency: self.p.concurrency_value(),
            },
        );
    }

    fn describe(&self, id: &RawValue) {
        let mut result = serde_json::Map::new();
        let actions: Vec<_> = self.p.actions.iter().map(|a| &a.descriptor).collect();
        result.insert("actions".into(), serde_json::to_value(actions).unwrap_or_default());
        self.describe_transforms(&mut result);
        if !self.p.connections.is_empty() {
            let conns: Vec<_> = self.p.connections.iter().map(|c| &c.descriptor).collect();
            result.insert(
                "connectionTypes".into(),
                serde_json::to_value(conns).unwrap_or_default(),
            );
        }
        self.reply(id, Json::Object(result));
    }

    fn execute(self: &Arc<Self>, id: &RawValue, params: Option<&RawValue>) {
        #[derive(Deserialize)]
        struct P {
            #[serde(rename = "type", default)]
            type_: String,
            #[serde(default)]
            config: Option<Box<RawValue>>,
            #[serde(default)]
            context: RunContext,
        }
        let Some(p) = params.and_then(|p| serde_json::from_str::<P>(p.get()).ok()) else {
            return self.fail(
                Some(id),
                code::INVALID_PARAMS,
                "invalid parameters of action.execute".into(),
            );
        };
        let Some(entry) = self.p.actions.iter().find(|a| a.descriptor.type_ == p.type_).cloned() else {
            return self.fail(
                Some(id),
                code::INVALID_PARAMS,
                format!("unknown action type {:?}", p.type_),
            );
        };
        let type_ = p.type_.clone();
        self.run(id, type_, move |ctx| {
            let call = Call {
                run_id: p.context.run_id,
                action_id: p.context.action_id,
                project_dir: p.context.project_dir,
                environment: p.context.environment,
                ctx: ctx.clone(),
            };
            let output = (entry.run)(&call, p.config.as_deref())?;
            Ok(serde_json::json!({ "output": output }))
        });
    }

    fn test_connection(self: &Arc<Self>, id: &RawValue, params: Option<&RawValue>) {
        #[derive(Deserialize)]
        struct P {
            #[serde(rename = "type", default)]
            type_: String,
            #[serde(default)]
            connection: Option<Box<RawValue>>,
        }
        let Some(p) = params.and_then(|p| serde_json::from_str::<P>(p.get()).ok()) else {
            return self.fail(
                Some(id),
                code::INVALID_PARAMS,
                "invalid parameters of connection.test".into(),
            );
        };
        let test = self
            .p
            .connections
            .iter()
            .find(|c| c.descriptor.type_ == p.type_)
            .and_then(|c| c.test.clone());
        let Some(test) = test else {
            return self.fail(
                Some(id),
                code::METHOD_NOT_FOUND,
                format!("connection type {:?} has no test", p.type_),
            );
        };
        self.run(id, p.type_.clone(), move |ctx| {
            match test(ctx, p.connection.as_deref()) {
                Ok(r) => Ok(serde_json::to_value(r)?),
                // A failing test is a result, not an error of the protocol.
                Err(e) if !ctx.is_cancelled() => Ok(serde_json::json!({ "ok": false, "message": e.to_string() })),
                Err(e) => Err(e),
            }
        });
    }

    /// Runs a request in its own thread once a slot is free, so that the reader keeps
    /// reading (e.g. `$/cancelRequest`) while requests run.
    pub(crate) fn run<F>(self: &Arc<Self>, id: &RawValue, what: String, f: F)
    where
        F: FnOnce(&Context) -> Result<Json, Error> + Send + 'static,
    {
        let cancel = Cancel::new();
        let key = normalize_id(id);
        let id: Box<RawValue> = id.to_owned();
        self.lock().running.insert(key.clone(), cancel.clone());
        let srv = Arc::clone(self);
        self.spawn(move || {
            let ctx = Context::new(srv.log.clone(), cancel.clone());
            let result = if srv.slots.acquire(&cancel) {
                let r = safely(&srv.log, &what, || f(&ctx));
                srv.slots.release();
                Some(r)
            } else {
                None
            };
            srv.lock().running.remove(&key);
            match result {
                _ if cancel.is_cancelled() => srv.fail(Some(&id), code::REQUEST_CANCELLED, "request cancelled".into()),
                Some(Ok(v)) => srv.reply(&id, v),
                Some(Err(e)) => srv.fail(Some(&id), code::ACTION_FAILED, e.to_string()),
                None => srv.fail(Some(&id), code::REQUEST_CANCELLED, "request cancelled".into()),
            }
        });
    }

    /// Starts a thread that serve waits for before it returns.
    pub(crate) fn spawn(&self, f: impl FnOnce() + Send + 'static) {
        let done = self.tasks.add();
        std::thread::spawn(move || {
            let _done = done;
            f();
        });
    }
}

/// The parts of the context of a run that the SDK passes on.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct RunContext {
    pub(crate) run_id: String,
    pub(crate) action_id: String,
    pub(crate) transform_id: String,
    pub(crate) project_dir: String,
    pub(crate) environment: String,
}

/// The key of a request id: the JSON text, so that 1 and "1" stay apart.
fn normalize_id(id: &RawValue) -> String {
    serde_json::from_str::<Json>(id.get())
        .map(|v| v.to_string())
        .unwrap_or_else(|_| id.get().to_string())
}

/// Limits how many requests run at the same time.
struct Semaphore {
    free: Mutex<usize>,
    cv: Condvar,
}

impl Semaphore {
    fn new(n: usize) -> Semaphore {
        Semaphore {
            free: Mutex::new(n.max(1)),
            cv: Condvar::new(),
        }
    }

    /// Takes a slot; false if the request was cancelled while waiting.
    fn acquire(&self, cancel: &Cancel) -> bool {
        let mut free = self.free.lock().unwrap_or_else(|e| e.into_inner());
        while *free == 0 {
            if cancel.is_cancelled() {
                return false;
            }
            free = self
                .cv
                .wait_timeout(free, Duration::from_millis(20))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *free -= 1;
        true
    }

    fn release(&self) {
        *self.free.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        self.cv.notify_one();
    }
}

/// Counts running threads; serve waits until all ended.
#[derive(Default, Clone)]
pub(crate) struct WaitGroup {
    inner: Arc<(Mutex<usize>, Condvar)>,
}

pub(crate) struct Done(WaitGroup);

impl Drop for Done {
    fn drop(&mut self) {
        let (lock, cv) = &*self.0.inner;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        cv.notify_all();
    }
}

impl WaitGroup {
    pub(crate) fn add(&self) -> Done {
        *self.inner.0.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        Done(self.clone())
    }

    fn wait(&self) {
        let (lock, cv) = &*self.inner;
        let mut n = lock.lock().unwrap_or_else(|e| e.into_inner());
        while *n > 0 {
            n = cv.wait(n).unwrap_or_else(|e| e.into_inner());
        }
    }
}

thread_local! {
    /// Set while the SDK runs code of the plugin: the panic hook keeps the backtrace here
    /// instead of printing it.
    static PANIC: RefCell<Option<Option<String>>> = const { RefCell::new(None) };
}

fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let caught = PANIC.with(|p| {
                let mut p = p.borrow_mut();
                match p.as_mut() {
                    Some(slot) => {
                        let at = info
                            .location()
                            .map(|l| format!(" at {}:{}", l.file(), l.line()))
                            .unwrap_or_default();
                        *slot = Some(format!("{at}\n{}", std::backtrace::Backtrace::force_capture()));
                        true
                    }
                    None => false,
                }
            });
            if !caught {
                previous(info);
            }
        }));
    });
}

/// Runs f and turns a panic into an error, so that one faulty action does not end the
/// plugin and with it the other actions of the run. The backtrace goes to the log.
pub(crate) fn safely<T>(log: &Logger, what: &str, f: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    let outer = PANIC.with(|p| p.borrow_mut().replace(None));
    let result = panic::catch_unwind(AssertUnwindSafe(f));
    let trace = PANIC.with(|p| std::mem::replace(&mut *p.borrow_mut(), outer)).flatten();
    match result {
        Ok(r) => r,
        Err(payload) => {
            let msg = panic_message(payload.as_ref());
            log.error(format_args!("panic in {what}: {msg}{}", trace.unwrap_or_default()));
            Err(format!("internal error of the plugin in {what}: {msg}").into())
        }
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}
