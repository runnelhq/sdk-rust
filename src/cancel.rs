//! Cancellation of requests and streams.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Tells a running action, test or stream that Runnel cancelled it (`$/cancelRequest`,
/// end of the run) or that the plugin ends. Long work should check `is_cancelled` now and
/// then and stop; `sleep` waits and wakes up early on cancellation.
#[derive(Clone, Default)]
pub struct Cancel {
    inner: Arc<(Mutex<bool>, Condvar)>,
}

impl Cancel {
    pub(crate) fn new() -> Cancel {
        Cancel::default()
    }

    /// Whether the work is cancelled.
    pub fn is_cancelled(&self) -> bool {
        *self.inner.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Waits for d or until the work is cancelled; returns true if it was cancelled.
    pub fn sleep(&self, d: Duration) -> bool {
        let (lock, cv) = &*self.inner;
        let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let (guard, _) = cv
            .wait_timeout_while(guard, d, |c| !*c)
            .unwrap_or_else(|e| e.into_inner());
        *guard
    }

    pub(crate) fn cancel(&self) {
        let (lock, cv) = &*self.inner;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cv.notify_all();
    }
}

impl std::fmt::Debug for Cancel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Cancel({})", self.is_cancelled())
    }
}
