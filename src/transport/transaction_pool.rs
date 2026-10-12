//! Transaction pool: transaction ID allocation and response matching.
//! Port of `gomodbus/transport/transaction_pool.go`.
//!
//! Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP Header): the Transaction
//! Identifier is a 2 byte field (0..=65535) "for transaction pairing"; the server copies it
//! into the response so the client can match them. Everything else here (free-ID queue,
//! timeout monitor thread, locking) is implementation detail, not spec.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Duration;

use crate::common::{Context, Error, LoggerInterface, Request, Result, TransactionId};
use crate::logging;

use super::request::Request as TcpRequest;
use super::transaction::Transaction;

/// MaxTransactions is the maximum number of possible transaction IDs.
///
/// Ref: Section 4.1, Transaction Identifier is 2 bytes => 0xFFFF + 1 distinct values.
pub const MAX_TRANSACTIONS: usize = 0xFFFF + 1;

/// DefaultTimeout is the default timeout for transactions (implementation detail, not spec).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// How often the monitor looks for timed out transactions (Go: 1 second ticker).
const MONITOR_INTERVAL: Duration = Duration::from_secs(1);
/// Granularity at which the monitor notices Close / drop of the pool.
const MONITOR_SLICE: Duration = Duration::from_millis(50);

/// Configuration collected from [`TransactionPoolOption`]s.
pub struct PoolConfig {
    logger: Arc<dyn LoggerInterface>,
    timeout: Duration,
}

/// A function that configures a [`TransactionPool`] (Go `TransactionPoolOption`).
pub type TransactionPoolOption = Box<dyn FnOnce(&mut PoolConfig)>;

/// Sets the timeout duration for transactions (ignored when zero, like Go's `timeout > 0`).
pub fn with_timeout(timeout: Duration) -> TransactionPoolOption {
    Box::new(move |c| {
        if !timeout.is_zero() {
            c.timeout = timeout;
        }
    })
}

/// Sets the logger for the transaction pool.
pub fn with_logger(logger: Arc<dyn LoggerInterface>) -> TransactionPoolOption {
    Box::new(move |c| c.logger = logger)
}

struct State {
    transactions: HashMap<TransactionId, Arc<Transaction>>,
    /// FIFO queue of free IDs (Go: a buffered channel pre-populated with 0..=0xFFFF).
    free_ids: VecDeque<u16>,
    closed: bool,
}

impl State {
    fn fresh_free_ids() -> VecDeque<u16> {
        (0..MAX_TRANSACTIONS).map(|i| i as u16).collect()
    }
}

struct Shared {
    logger: RwLock<Arc<dyn LoggerInterface>>,
    state: Mutex<State>,
    done: AtomicBool,
    timeout: Duration,
}

impl Shared {
    fn logger(&self) -> Arc<dyn LoggerInterface> {
        self.logger.read().unwrap().clone()
    }

    /// Caller must hold the state lock. Removes the transaction and returns its ID to the free
    /// queue (Go `unsafeRelease`).
    fn unsafe_release(state: &mut State, tx_id: TransactionId) {
        if state.transactions.remove(&tx_id).is_some() && !state.closed {
            state.free_ids.push_back(tx_id.0);
        }
    }

    /// Looks for timed out transactions and cancels them (Go `checkTimeouts`).
    fn check_timeouts(&self) {
        let ctx = Context::background();
        let logger = self.logger();
        let mut expired = Vec::new();
        {
            let mut state = self.state.lock().unwrap();
            let ids: Vec<TransactionId> = state
                .transactions
                .iter()
                .filter(|(_, tx)| tx.get_lifetime() > self.timeout)
                .map(|(id, _)| *id)
                .collect();
            for id in ids {
                if let Some(tx) = state.transactions.get(&id).cloned() {
                    logger.warn(&ctx, format_args!("Transaction {} timed out after {:?}", id.0, tx.get_lifetime()));
                    Self::unsafe_release(&mut state, id);
                    expired.push(tx);
                }
            }
        }
        // Cancel the transactions with a timeout error.
        for tx in expired {
            tx.cancel(Error::TransactionTimeout);
        }
    }
}

/// TransactionPool manages a pool of active transactions.
///
/// IDs are handed out in FIFO order starting at 0; a released ID goes to the back of the
/// queue, so an ID is not reused until the other 65535 have been used.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP Header, Transaction Identifier)
pub struct TransactionPool {
    shared: Arc<Shared>,
}

impl Default for TransactionPool {
    fn default() -> Self {
        TransactionPool::new()
    }
}

impl TransactionPool {
    /// Creates a new transaction pool with the default timeout ([`DEFAULT_TIMEOUT`]) and the
    /// default logger.
    pub fn new() -> TransactionPool {
        TransactionPool::with_options(Vec::new())
    }

    /// Creates a new transaction pool and applies `options` in order. Starts the timeout
    /// monitor thread (implementation detail); it ends on [`close`](Self::close) or drop.
    pub fn with_options(options: Vec<TransactionPoolOption>) -> TransactionPool {
        let mut config = PoolConfig { logger: logging::default_logger(), timeout: DEFAULT_TIMEOUT };
        for option in options {
            option(&mut config);
        }
        let shared = Arc::new(Shared {
            logger: RwLock::new(config.logger),
            state: Mutex::new(State {
                transactions: HashMap::new(),
                free_ids: State::fresh_free_ids(),
                closed: false,
            }),
            done: AtomicBool::new(false),
            timeout: config.timeout,
        });

        // Start the timeout monitor thread (implementation detail, not spec).
        let weak: Weak<Shared> = Arc::downgrade(&shared);
        std::thread::spawn(move || timeout_monitor(weak));

        TransactionPool { shared }
    }

    /// Shuts down the transaction pool: pending transactions are cancelled with
    /// `Error::TransportClosing`, and further `place` calls fail. Safe to call repeatedly.
    pub fn close(&self) {
        self.shared.logger().info(&Context::background(), format_args!("Closing transaction pool"));
        self.close_quietly();
    }

    fn close_quietly(&self) {
        let ctx = Context::background();
        let logger = self.shared.logger();

        let drained: Vec<(TransactionId, Arc<Transaction>)> = {
            let mut state = self.shared.state.lock().unwrap();
            self.shared.done.store(true, Ordering::SeqCst);
            state.closed = true;
            state.free_ids.clear();
            state.transactions.drain().collect()
        };

        // Cancel all pending transactions
        for (tx_id, tx) in drained {
            logger.debug(&ctx, format_args!("Cancelling transaction {}", tx_id.0));
            tx.cancel(Error::TransportClosing);
        }
    }

    /// Returns the current count of active transactions.
    pub fn get_count(&self) -> usize {
        self.shared.state.lock().unwrap().transactions.len()
    }

    /// Adds a transaction to the pool and assigns it a transaction ID, which is also written
    /// into `request` via `set_transaction_id`.
    ///
    /// The pool keeps a snapshot of the request (ID, unit, PDU) because a `&mut dyn Request`
    /// borrow cannot be handed to the writer thread. Errors: `"transaction pool is full (no IDs
    /// in free list)"` and `"transaction pool is closed"`.
    ///
    /// Ref: Section 4.1 (MBAP Header): the ID is chosen by the client and echoed by the server.
    pub fn place(&self, ctx: &Context, request: &mut dyn Request) -> Result<Arc<Transaction>> {
        let tx_id;
        let tx;
        {
            let mut state = self.shared.state.lock().unwrap();
            // The pool may have been closed: do not hand out IDs any more.
            if state.closed {
                return Err(Error::msg("transaction pool is closed"));
            }
            // Try to get an ID from the free list
            let Some(id) = state.free_ids.pop_front() else {
                return Err(Error::msg("transaction pool is full (no IDs in free list)"));
            };
            tx_id = TransactionId(id);

            // Set the transaction ID on the request
            request.set_transaction_id(tx_id);

            self.shared.logger().debug(ctx, format_args!("Placing transaction with ID: {}", tx_id.0));

            // Create a new transaction around a snapshot of the request
            let mut snapshot = TcpRequest::new(request.get_unit_id(), request.get_pdu().function_code, request.get_pdu().data.clone());
            snapshot.transaction_id = tx_id;
            tx = Arc::new(Transaction::new(ctx, Box::new(snapshot)));

            // Store in the pool
            state.transactions.insert(tx_id, tx.clone());
        }
        Ok(tx)
    }

    /// Retrieves a transaction by its ID without removing it.
    pub fn get(&self, tx_id: TransactionId) -> Option<Arc<Transaction>> {
        self.shared.state.lock().unwrap().transactions.get(&tx_id).cloned()
    }

    /// Removes a transaction from the pool and returns it (its ID becomes free again).
    pub fn release(&self, tx_id: TransactionId) -> Option<Arc<Transaction>> {
        let mut state = self.shared.state.lock().unwrap();
        let result = state.transactions.get(&tx_id).cloned();
        if result.is_some() {
            Shared::unsafe_release(&mut state, tx_id);
        }
        result
    }

    /// Like [`release`](Self::release) but only if the pooled transaction is exactly `tx`.
    /// Protects a newer transaction that reuses the ID after a reset (Rust-side hardening).
    pub fn release_tx(&self, tx_id: TransactionId, tx: &Arc<Transaction>) -> bool {
        let mut state = self.shared.state.lock().unwrap();
        match state.transactions.get(&tx_id) {
            Some(cur) if Arc::ptr_eq(cur, tx) => {
                Shared::unsafe_release(&mut state, tx_id);
                true
            }
            _ => false,
        }
    }

    /// Cancels every pending transaction with `Error::TransportClosing`, empties the map and
    /// restores the full free-ID queue (Go `unsafeReset`, called with the lock held).
    pub fn reset(&self) {
        let ctx = Context::background();
        let logger = self.shared.logger();
        let old: Vec<(TransactionId, Arc<Transaction>)> = {
            let mut state = self.shared.state.lock().unwrap();
            let old = state.transactions.drain().collect();
            state.free_ids = State::fresh_free_ids();
            old
        };
        // Cancel all transactions with a consistent error message
        for (tx_id, tx) in old {
            logger.debug(&ctx, format_args!("Cancelling transaction {} during reset", tx_id.0));
            tx.cancel(Error::TransportClosing);
        }
    }

    /// Replaces the pool's logger.
    pub fn set_logger(&self, logger: Arc<dyn LoggerInterface>) {
        *self.shared.logger.write().unwrap() = logger;
    }

    /// Runs one timeout sweep immediately (the monitor thread does this every second).
    pub fn check_timeouts(&self) {
        self.shared.check_timeouts();
    }
}

impl Drop for TransactionPool {
    fn drop(&mut self) {
        // Stop the monitor thread; pending transactions are cancelled like in `close`.
        if !self.shared.done.load(Ordering::SeqCst) {
            self.close_quietly();
        }
    }
}

/// Periodically checks for timed out transactions (Go `timeoutMonitor`); exits when the pool
/// is closed or dropped (implementation detail, not spec).
fn timeout_monitor(weak: Weak<Shared>) {
    let mut waited = Duration::ZERO;
    loop {
        std::thread::sleep(MONITOR_SLICE);
        let Some(shared) = weak.upgrade() else { return };
        if shared.done.load(Ordering::SeqCst) {
            return;
        }
        waited += MONITOR_SLICE;
        if waited >= MONITOR_INTERVAL {
            waited = Duration::ZERO;
            shared.check_timeouts();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::test_support::MockRequest;
    use crate::common::{FunctionCode, UnitId};

    fn quiet(options: Vec<TransactionPoolOption>) -> TransactionPool {
        let mut opts = vec![with_logger(logging::noop())];
        opts.extend(options);
        TransactionPool::with_options(opts)
    }

    fn mock() -> MockRequest {
        MockRequest::new(TransactionId(0), UnitId(1), FunctionCode(3), vec![0, 1, 0, 1])
    }

    #[test]
    fn ids_are_sequential_and_set_on_request() {
        let pool = quiet(vec![]);
        let ctx = Context::background();
        let mut r1 = mock();
        let mut r2 = mock();
        let t1 = pool.place(&ctx, &mut r1).unwrap();
        let t2 = pool.place(&ctx, &mut r2).unwrap();
        assert_eq!(crate::common::Request::get_transaction_id(&r1), TransactionId(0));
        assert_eq!(crate::common::Request::get_transaction_id(&r2), TransactionId(1));
        assert_eq!(t2.request.get_transaction_id(), TransactionId(1));
        assert_eq!(pool.get_count(), 2);
        assert!(pool.get(TransactionId(0)).is_some());
        assert!(Arc::ptr_eq(&pool.release(TransactionId(0)).unwrap(), &t1));
        assert!(pool.release(TransactionId(0)).is_none());
        assert_eq!(pool.get_count(), 1);
    }

    #[test]
    fn released_ids_are_reused_last() {
        let pool = quiet(vec![]);
        let ctx = Context::background();
        let mut r = mock();
        pool.place(&ctx, &mut r).unwrap();
        pool.release(TransactionId(0)).unwrap();
        let mut r = mock();
        pool.place(&ctx, &mut r).unwrap();
        // 0 went to the back of the queue, so the next ID is 1
        assert_eq!(crate::common::Request::get_transaction_id(&r), TransactionId(1));
    }

    #[test]
    fn pool_full_error() {
        let pool = quiet(vec![]);
        let ctx = Context::background();
        for _ in 0..MAX_TRANSACTIONS {
            pool.place(&ctx, &mut mock()).unwrap();
        }
        assert_eq!(pool.get_count(), MAX_TRANSACTIONS);
        let err = pool.place(&ctx, &mut mock()).err().unwrap();
        assert_eq!(err.to_string(), "transaction pool is full (no IDs in free list)");
        pool.release(TransactionId(7)).unwrap();
        let mut r = mock();
        pool.place(&ctx, &mut r).unwrap();
        assert_eq!(crate::common::Request::get_transaction_id(&r), TransactionId(7));
    }

    #[test]
    fn close_cancels_pending_and_rejects_place() {
        let pool = quiet(vec![]);
        let ctx = Context::background();
        let tx = pool.place(&ctx, &mut mock()).unwrap();
        pool.close();
        pool.close(); // idempotent
        assert_eq!(tx.wait_timeout(Duration::from_secs(1)).unwrap().err(), Some(Error::TransportClosing));
        assert_eq!(pool.get_count(), 0);
        let err = pool.place(&ctx, &mut mock()).err().unwrap();
        assert_eq!(err.to_string(), "transaction pool is closed");
    }

    #[test]
    fn reset_cancels_and_restores_ids() {
        let pool = quiet(vec![]);
        let ctx = Context::background();
        let tx = pool.place(&ctx, &mut mock()).unwrap();
        pool.reset();
        assert_eq!(tx.wait_timeout(Duration::from_secs(1)).unwrap().err(), Some(Error::TransportClosing));
        assert_eq!(pool.get_count(), 0);
        let mut r = mock();
        pool.place(&ctx, &mut r).unwrap();
        assert_eq!(crate::common::Request::get_transaction_id(&r), TransactionId(0));
        // a stale release of an old transaction must not free the new one
        assert!(!pool.release_tx(TransactionId(0), &tx));
        assert_eq!(pool.get_count(), 1);
    }

    #[test]
    fn timeout_cancels_transaction_and_frees_id() {
        let pool = quiet(vec![with_timeout(Duration::from_millis(20))]);
        let ctx = Context::background();
        let tx = pool.place(&ctx, &mut mock()).unwrap();
        // the monitor thread sweeps every second
        let out = tx.wait_timeout(Duration::from_secs(4)).expect("timed out waiting for monitor");
        assert_eq!(out.err(), Some(Error::TransactionTimeout));
        assert_eq!(pool.get_count(), 0);
    }

    #[test]
    fn zero_timeout_option_keeps_default() {
        let pool = quiet(vec![with_timeout(Duration::ZERO)]);
        assert_eq!(pool.shared.timeout, DEFAULT_TIMEOUT);
    }

    #[test]
    fn caller_context_cancel_reaches_transaction() {
        let pool = quiet(vec![]);
        let parent = Context::background().with_cancel();
        let tx = pool.place(&parent, &mut mock()).unwrap();
        parent.cancel();
        assert!(tx.context().is_done());
    }
}
