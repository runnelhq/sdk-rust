//! Log of the run: stderr of the plugin, one line per entry with its level.

use std::fmt::Display;
use std::io::Write;
use std::sync::{Arc, Mutex};

/// The longest line the logger writes; Runnel keeps 64 KiB per line.
const MAX_LOG_LINE: usize = 60 << 10;

/// Writes to the log of the run: Runnel reads stderr of the plugin line by line and takes
/// `DEBUG`, `INFO`, `WARN` or `ERROR` at the start of a line as its level. A message of
/// several lines becomes one line (` | ` between them); lines are cut at 60 KiB. A Logger
/// may be cloned and used by several threads. Runnel masks secrets it passed to the
/// plugin, but they should not be logged in the first place.
#[derive(Clone)]
pub struct Logger {
    w: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl Logger {
    /// A logger that writes to w, e.g. `std::io::stderr()`.
    pub fn new(w: impl Write + Send + 'static) -> Logger {
        Logger {
            w: Arc::new(Mutex::new(Box::new(w))),
        }
    }

    /// Logs a message with level debug (shown with `runnel run -v`).
    pub fn debug(&self, msg: impl Display) {
        self.write("DEBUG", msg);
    }

    /// Logs a message with level info.
    pub fn info(&self, msg: impl Display) {
        self.write("INFO", msg);
    }

    /// Logs a message with level warn.
    pub fn warn(&self, msg: impl Display) {
        self.write("WARN", msg);
    }

    /// Logs a message with level error. It does not fail the action; return an error for
    /// that.
    pub fn error(&self, msg: impl Display) {
        self.write("ERROR", msg);
    }

    fn write(&self, level: &str, msg: impl Display) {
        let mut msg = msg.to_string().replace("\r\n", " | ").replace(['\n', '\r'], " | ");
        if msg.len() > MAX_LOG_LINE {
            let mut cut = MAX_LOG_LINE;
            while !msg.is_char_boundary(cut) {
                cut -= 1;
            }
            msg.truncate(cut);
            msg.push('…');
        }
        let mut w = self.w.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writeln!(w, "{level} {msg}");
        let _ = w.flush();
    }
}

impl std::fmt::Debug for Logger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Logger")
    }
}
