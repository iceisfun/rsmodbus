//! Cancellation / deadline context (replacement for Go's `context.Context`).
//!
//! Implementation detail, not part of the Modbus spec. A [`Context`] carries an
//! optional deadline and a cancel flag; deriving a child (`with_timeout`,
//! `with_cancel`) inherits the parent's deadline and cancellation, exactly like
//! Go. Cloning a `Context` shares the same cancel flag, so `ctx.clone().cancel()`
//! cancels `ctx` too.
//!
//! Blocking std I/O cannot be interrupted, so implementations poll: use
//! [`Context::remaining`] to derive socket timeouts and check
//! [`Context::is_done`]/[`Context::err`] between I/O slices.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::errors::Error;

#[derive(Debug)]
struct Inner {
    parent: Option<Context>,
    deadline: Option<Instant>,
    cancelled: AtomicBool,
}

/// Deadline + cancellation carrier passed to every blocking operation.
#[derive(Debug, Clone)]
pub struct Context {
    inner: Arc<Inner>,
}

impl Default for Context {
    fn default() -> Self {
        Context::background()
    }
}

impl Context {
    /// Root context: never cancelled, no deadline (Go `context.Background()`).
    pub fn background() -> Context {
        Context { inner: Arc::new(Inner { parent: None, deadline: None, cancelled: AtomicBool::new(false) }) }
    }

    /// Child that can be cancelled with [`Context::cancel`] (Go `context.WithCancel`).
    pub fn with_cancel(&self) -> Context {
        self.child(None)
    }

    /// Child that expires after `timeout` (Go `context.WithTimeout`). Can also be cancelled.
    pub fn with_timeout(&self, timeout: Duration) -> Context {
        self.child(Some(Instant::now() + timeout))
    }

    /// Child that expires at `deadline` (Go `context.WithDeadline`). Can also be cancelled.
    pub fn with_deadline(&self, deadline: Instant) -> Context {
        self.child(Some(deadline))
    }

    fn child(&self, deadline: Option<Instant>) -> Context {
        Context {
            inner: Arc::new(Inner {
                parent: Some(self.clone()),
                deadline,
                cancelled: AtomicBool::new(false),
            }),
        }
    }

    /// Cancels this context and every context derived from it.
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
    }

    /// True if this context or any ancestor was cancelled explicitly.
    pub fn is_cancelled(&self) -> bool {
        if self.inner.cancelled.load(Ordering::SeqCst) {
            return true;
        }
        match &self.inner.parent {
            Some(p) => p.is_cancelled(),
            None => false,
        }
    }

    /// Earliest deadline in the ancestor chain, if any (Go `ctx.Deadline()`).
    pub fn deadline(&self) -> Option<Instant> {
        let parent = self.inner.parent.as_ref().and_then(|p| p.deadline());
        match (self.inner.deadline, parent) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Time left until the deadline (`Some(ZERO)` once expired, `None` if no deadline).
    pub fn remaining(&self) -> Option<Duration> {
        self.deadline().map(|d| d.saturating_duration_since(Instant::now()))
    }

    /// True once the deadline has passed.
    pub fn is_expired(&self) -> bool {
        matches!(self.deadline(), Some(d) if Instant::now() >= d)
    }

    /// True if cancelled or expired (Go `ctx.Done()` fired).
    pub fn is_done(&self) -> bool {
        self.is_cancelled() || self.is_expired()
    }

    /// Go `ctx.Err()`: `None` while live, `Some(ContextCanceled)` after
    /// cancellation, `Some(DeadlineExceeded)` after the deadline.
    pub fn err(&self) -> Option<Error> {
        if self.is_cancelled() {
            Some(Error::ContextCanceled)
        } else if self.is_expired() {
            Some(Error::DeadlineExceeded)
        } else {
            None
        }
    }

    /// Smaller of `limit` and the time remaining; use to derive socket timeouts.
    pub fn clamp(&self, limit: Duration) -> Duration {
        match self.remaining() {
            Some(r) => r.min(limit),
            None => limit,
        }
    }

    /// Sleeps for `d`, waking early (with the ctx error) if cancelled or expired.
    pub fn sleep(&self, d: Duration) -> Result<(), Error> {
        let end = Instant::now() + d;
        loop {
            if let Some(e) = self.err() {
                return Err(e);
            }
            let now = Instant::now();
            if now >= end {
                return Ok(());
            }
            std::thread::sleep((end - now).min(Duration::from_millis(10)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_is_live() {
        let c = Context::background();
        assert!(!c.is_done());
        assert!(c.err().is_none());
        assert!(c.deadline().is_none());
    }

    #[test]
    fn cancel_propagates_to_children() {
        let parent = Context::background().with_cancel();
        let child = parent.with_timeout(Duration::from_secs(60));
        assert!(!child.is_done());
        parent.cancel();
        assert_eq!(child.err(), Some(Error::ContextCanceled));
    }

    #[test]
    fn timeout_expires_and_inherits_min_deadline() {
        let parent = Context::background().with_timeout(Duration::from_millis(20));
        let child = parent.with_timeout(Duration::from_secs(60));
        assert!(child.deadline().unwrap() <= Instant::now() + Duration::from_millis(20));
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(child.err(), Some(Error::DeadlineExceeded));
        assert_eq!(child.remaining(), Some(Duration::ZERO));
        assert_eq!(child.sleep(Duration::from_secs(1)), Err(Error::DeadlineExceeded));
    }
}
