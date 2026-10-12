//! In-flight Modbus TCP transaction. Port of `gomodbus/transport/transaction.go`.
//!
//! Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP Header): the Transaction
//! Identifier lets a client match a response to the request that caused it. The wait/complete
//! machinery below (Mutex + Condvar instead of Go channels) is an implementation detail, not spec.

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::common::{Context, Error, Request, Response, Result};

/// Slice used when waiting so cancellation of a [`Context`] is noticed promptly
/// (implementation detail, not spec).
const WAIT_SLICE: Duration = Duration::from_millis(10);

/// Outcome slot (Go's `ResponseCh` + `ErrCh`, each with capacity 1; here a single slot where
/// the first completion wins).
struct Slot {
    completed: bool,
    outcome: Option<Result<Box<dyn Response>>>,
}

/// Transaction represents an ongoing transaction with a request, a result slot and a context.
/// The Modbus TCP protocol uses a transaction ID to match requests and responses.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP Header)
pub struct Transaction {
    /// The Modbus request (with its transaction ID already assigned).
    pub request: Box<dyn Request>,
    slot: Mutex<Slot>,
    cond: Condvar,
    /// Context for cancellation (child of the caller's context; cancelled on completion).
    ctx: Context,
    /// Time when the transaction was created, used for timeout detection.
    create_time: Instant,
}

impl Transaction {
    /// Creates a new transaction with a given request and context.
    pub fn new(ctx: &Context, request: Box<dyn Request>) -> Transaction {
        Transaction {
            request,
            slot: Mutex::new(Slot { completed: false, outcome: None }),
            cond: Condvar::new(),
            ctx: ctx.with_cancel(),
            create_time: Instant::now(),
        }
    }

    /// Signals the transaction is complete with either a response or an error. Only the first
    /// completion is delivered; later ones are dropped (Go: non-blocking send into a full
    /// channel). The transaction's context is cancelled in every case.
    pub fn complete(&self, result: Result<Box<dyn Response>>) {
        {
            let mut slot = self.slot.lock().unwrap();
            if !slot.completed {
                slot.completed = true;
                slot.outcome = Some(result);
            }
        }
        self.cond.notify_all();
        self.ctx.cancel();
    }

    /// Cancels the transaction with an error.
    pub fn cancel(&self, err: Error) {
        self.complete(Err(err));
    }

    /// Returns the transaction's context (done once the transaction completed or the
    /// caller's context ended).
    pub fn context(&self) -> &Context {
        &self.ctx
    }

    /// Returns the transaction's lifetime.
    pub fn get_lifetime(&self) -> Duration {
        self.create_time.elapsed()
    }

    /// Takes the outcome if the transaction already completed (non-blocking).
    pub fn try_take(&self) -> Option<Result<Box<dyn Response>>> {
        self.slot.lock().unwrap().outcome.take()
    }

    /// Waits for the outcome for at most `timeout`; `None` if nothing arrived in time.
    pub fn wait_timeout(&self, timeout: Duration) -> Option<Result<Box<dyn Response>>> {
        let end = Instant::now() + timeout;
        let mut slot = self.slot.lock().unwrap();
        loop {
            if let Some(o) = slot.outcome.take() {
                return Some(o);
            }
            let now = Instant::now();
            if now >= end {
                return None;
            }
            slot = self.cond.wait_timeout(slot, end - now).unwrap().0;
        }
    }

    /// Waits for the outcome or for `ctx` to end. When `ctx` ends first, returns its error
    /// (`ContextCanceled` / `DeadlineExceeded`) and leaves the transaction in place.
    pub fn wait(&self, ctx: &Context) -> Result<Box<dyn Response>> {
        let mut slot = self.slot.lock().unwrap();
        loop {
            if let Some(o) = slot.outcome.take() {
                return o;
            }
            if let Some(e) = ctx.err() {
                return Err(e);
            }
            slot = self.cond.wait_timeout(slot, ctx.clamp(WAIT_SLICE)).unwrap().0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::test_support::MockRequest;
    use crate::common::test_support::MockResponse;
    use crate::common::{FunctionCode, TransactionId, UnitId};
    use std::sync::Arc;

    fn req() -> Box<dyn Request> {
        Box::new(MockRequest::new(TransactionId(1), UnitId(1), FunctionCode(3), vec![0, 1, 0, 1]))
    }

    fn resp() -> Box<dyn Response> {
        Box::new(MockResponse::new(TransactionId(1), UnitId(1), FunctionCode(3), vec![2, 0, 1]))
    }

    #[test]
    fn complete_delivers_response_and_cancels_ctx() {
        let tx = Transaction::new(&Context::background(), req());
        assert!(!tx.context().is_done());
        tx.complete(Ok(resp()));
        assert!(tx.context().is_done());
        let r = tx.wait(&Context::background()).unwrap();
        assert_eq!(r.get_pdu().function_code, FunctionCode(3));
    }

    #[test]
    fn first_completion_wins() {
        let tx = Transaction::new(&Context::background(), req());
        tx.cancel(Error::TransportClosing);
        tx.complete(Ok(resp()));
        assert_eq!(tx.try_take().unwrap().err(), Some(Error::TransportClosing));
        assert!(tx.try_take().is_none());
    }

    #[test]
    fn wait_from_other_thread() {
        let tx = Arc::new(Transaction::new(&Context::background(), req()));
        let t2 = tx.clone();
        let h = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            t2.complete(Ok(resp()));
        });
        assert!(tx.wait(&Context::background().with_timeout(Duration::from_secs(2))).is_ok());
        h.join().unwrap();
    }

    #[test]
    fn wait_honours_context() {
        let tx = Transaction::new(&Context::background(), req());
        let ctx = Context::background().with_timeout(Duration::from_millis(30));
        assert_eq!(tx.wait(&ctx).err(), Some(Error::DeadlineExceeded));
        let c = Context::background().with_cancel();
        c.cancel();
        assert_eq!(tx.wait(&c).err(), Some(Error::ContextCanceled));
        assert!(tx.wait_timeout(Duration::from_millis(10)).is_none());
    }

    #[test]
    fn parent_cancel_propagates_to_transaction_context() {
        let parent = Context::background().with_cancel();
        let tx = Transaction::new(&parent, req());
        parent.cancel();
        assert!(tx.context().is_done());
        assert!(tx.get_lifetime() < Duration::from_secs(5));
    }
}
