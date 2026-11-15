//! Byte caps that sit beside the inbound path's entry caps; see `src/AGENTS.md` §byte-bounds.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const MIB: u64 = 1024 * 1024;

/// Row bytes one verse's live subscription holds between its pump and the command loop.
pub const LIVE_ROWS_BYTE_CAP: u64 = 64 * MIB;
/// Bytes of live kept rows per verse (`KeptRows`); superseded copies compact at twice this.
pub const KEPT_ROWS_BYTE_CAP: u64 = 64 * MIB;
/// Bytes of `role` rows kept for resend per verse.
pub const UNSENT_ROLE_ROWS_BYTE_CAP: u64 = 16 * MIB;
/// Largest single `role` row kept for resend: 16x a statement with `MAX_GRANT_WINDOWS` windows
/// and 256-byte ids (sharding test `a_quarantine_flood_cannot_lose_a_legit_editors_shards_row`).
pub const MAX_KEPT_ROLE_ROW_BYTES: u64 = 16 * 1024;
/// Bytes of rows the role/petal quarantine holds per verse.
pub const QUARANTINE_BYTE_CAP: u64 = 16 * MIB;
/// Candidate bytes in one `RejudgeReplicatedNodes` command.
pub const REJUDGE_CHUNK_BYTES: u64 = 4 * MIB;
/// Nodes in one `RejudgeReplicatedNodes` command.
pub const REJUDGE_CHUNK_NODES: usize = 256;
/// Candidate bytes one drain turn hands the DB channel before it resumes on the next turn.
pub const REJUDGE_TURN_BYTES: u64 = 16 * MIB;
/// A re-judge turn starts only while the DB channel holds at most this many commands, so
/// earlier turns' chunks drain before more are queued.
pub const REJUDGE_MAX_QUEUED: usize = 8;

/// Saturating byte counter that parks producers at its cap; an item larger than the cap passes alone.
#[derive(Debug)]
pub struct ByteGate {
    cap: u64,
    used: AtomicU64,
    peak: AtomicU64,
    notify: tokio::sync::Notify,
}

impl ByteGate {
    /// A gate admitting at most `cap` bytes at once.
    pub fn new(cap: u64) -> Arc<Self> {
        Arc::new(Self {
            cap,
            used: AtomicU64::new(0),
            peak: AtomicU64::new(0),
            notify: tokio::sync::Notify::new(),
        })
    }

    /// Charge `bytes` when they fit (or the gate is empty); `false` leaves the gate unchanged.
    pub fn try_acquire(&self, bytes: u64) -> bool {
        let mut current = self.used.load(Ordering::Acquire);
        loop {
            if current != 0 && current.saturating_add(bytes) > self.cap {
                return false;
            }
            let next = current.saturating_add(bytes);
            match self
                .used
                .compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => {
                    self.peak.fetch_max(next, Ordering::AcqRel);
                    return true;
                }
                Err(actual) => current = actual,
            }
        }
    }

    /// Wait until `bytes` fit, then charge them.
    pub async fn acquire(&self, bytes: u64) {
        loop {
            let mut notified = std::pin::pin!(self.notify.notified());
            notified.as_mut().enable();
            if self.try_acquire(bytes) {
                return;
            }
            notified.await;
        }
    }

    /// Return `bytes` (saturating: a release nobody charged is harmless) and wake parked producers.
    pub fn release(&self, bytes: u64) {
        let mut current = self.used.load(Ordering::Acquire);
        loop {
            let next = current.saturating_sub(bytes);
            match self
                .used
                .compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
        self.notify.notify_waiters();
    }

    /// Bytes charged now.
    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Acquire)
    }

    /// Highest charge ever held.
    pub fn peak(&self) -> u64 {
        self.peak.load(Ordering::Acquire)
    }

    /// The configured cap.
    pub fn cap(&self) -> u64 {
        self.cap
    }
}

/// Bytes charged on a [`ByteGate`], released when dropped.
#[derive(Debug)]
pub struct ByteLease {
    gate: Arc<ByteGate>,
    bytes: u64,
}

impl ByteLease {
    /// Take ownership of `bytes` a producer already charged on `gate`.
    pub fn adopt(gate: Arc<ByteGate>, bytes: u64) -> Self {
        Self { gate, bytes }
    }
}

impl Drop for ByteLease {
    fn drop(&mut self) {
        self.gate.release(self.bytes);
    }
}

thread_local! {
    /// The DB command channel's byte gate for this thread (the sync thread's current-thread runtime); see §byte-bounds "DB command bytes".
    static DB_COMMAND_BYTES: std::cell::RefCell<Option<fe_runtime::channels::DbCommandBytes>> =
        const { std::cell::RefCell::new(None) };
}

/// Restores the previous thread DB byte gate when dropped.
#[must_use = "the gate is unset again when the scope drops"]
pub struct DbCommandBytesScope(Option<fe_runtime::channels::DbCommandBytes>);

/// Charge byte-carrying DB sends made on this thread against `bytes` (`None` sends ungated).
pub fn scope_db_command_bytes(bytes: Option<fe_runtime::channels::DbCommandBytes>) -> DbCommandBytesScope {
    DbCommandBytesScope(DB_COMMAND_BYTES.with(|slot| slot.replace(bytes)))
}

impl Drop for DbCommandBytesScope {
    fn drop(&mut self) {
        let previous = self.0.take();
        DB_COMMAND_BYTES.with(|slot| *slot.borrow_mut() = previous);
    }
}

/// Whether the DB channel has drained enough for bulk work: at most `REJUDGE_MAX_QUEUED`
/// commands queued and this thread's byte gate at most half full (gate and channel alike).
pub fn db_channel_drained(tx: &crossbeam::channel::Sender<fe_runtime::messages::DbCommand>) -> bool {
    tx.len() <= REJUDGE_MAX_QUEUED
        && DB_COMMAND_BYTES.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_none_or(|gate| gate.current() <= fe_runtime::channels::DB_COMMAND_BYTE_CAP / 2)
        })
}

/// `try_send` that charges the command's payload on the thread's DB byte gate first; a gate at
/// `DB_COMMAND_BYTE_CAP` answers `Full`, like the channel, and a refused send releases its charge.
pub fn try_send_db(
    tx: &crossbeam::channel::Sender<fe_runtime::messages::DbCommand>,
    command: fe_runtime::messages::DbCommand,
) -> Result<(), crossbeam::channel::TrySendError<fe_runtime::messages::DbCommand>> {
    let bytes = command.payload_bytes();
    let gate = if bytes == 0 { None } else { DB_COMMAND_BYTES.with(|slot| slot.borrow().clone()) };
    let Some(gate) = gate else {
        return tx.try_send(command);
    };
    if !gate.try_charge(bytes, fe_runtime::channels::DB_COMMAND_BYTE_CAP) {
        return Err(crossbeam::channel::TrySendError::Full(command));
    }
    tx.try_send(command).inspect_err(|_| gate.release(bytes))
}
