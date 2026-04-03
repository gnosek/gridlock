//! Instrumented replacements for [`tokio::sync`] primitives.
//!
//! Every type wraps the corresponding tokio type and delegates all operations,
//! but calls [`ResourceObserver`](crate::observer::ResourceObserver) hooks on
//! wait / acquire / release so that tracing and lock-ordering validation happen
//! transparently.
//!
//! The module layout mirrors `tokio::sync`: locks and `Notify`/`Barrier` live
//! at the top level, while channel families live in sub-modules (`mpsc`,
//! `broadcast`, `watch`, `oneshot`).  Types that gridlock doesn't wrap
//! (e.g. `UnboundedSender`, error types) are re-exported from tokio directly.

// private implementation modules — file names kept as-is
mod barrier;
mod channel;
mod mutex;
mod notify;
mod rwlock;
mod semaphore;

// ── Flat top-level exports (match tokio::sync) ────────────────────────────
pub use barrier::Barrier;
pub use mutex::{Mutex, MutexGuard, OwnedMutexGuard};
pub use notify::Notify;
pub use rwlock::{
    OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock, RwLockReadGuard, RwLockWriteGuard,
};
pub use semaphore::{OwnedSemaphorePermit, Semaphore};
pub use tokio::sync::{AcquireError, BarrierWaitResult, SemaphorePermit, TryAcquireError, TryLockError};

// ── Channel family sub-modules (match tokio::sync::mpsc / broadcast / …) ──

/// Instrumented bounded MPSC channel.
///
/// `send()` reports wait/acquire/release (back-pressure is a blocking point).
/// `recv()` reports wait/acquire but **never releases** — the ordering edge
/// `channel → later_resource` is permanent, correctly modelling the task's
/// dataflow dependency.
///
/// Unbounded variants and error types are re-exported from tokio unchanged.
pub mod mpsc {
    pub use super::channel::mpsc::{Receiver, Sender, channel, named_channel};
    pub use tokio::sync::mpsc::{
        OwnedPermit, Permit, UnboundedReceiver, UnboundedSender, WeakSender, error,
        unbounded_channel,
    };
}

/// Instrumented broadcast channel.
///
/// `send()` is non-blocking (immediate acquire + release).
/// `recv()` blocks and permanently marks the resource as held.
pub mod broadcast {
    pub use super::channel::broadcast::{Receiver, Sender, channel};
    pub use tokio::sync::broadcast::error;
}

/// Instrumented watch channel (single-value, latest-wins).
///
/// `send()` is non-blocking (immediate acquire + release).
/// `changed().await` blocks and permanently marks the resource as held.
/// `borrow()` / `borrow_and_update()` are non-blocking and not instrumented.
pub mod watch {
    pub use super::channel::watch::{Receiver, Sender, channel, named_channel};
    pub use tokio::sync::watch::{Ref, error};
}

/// Instrumented oneshot channel.
///
/// `send()` is non-blocking (immediate acquire + release).
/// `recv()` blocks and permanently marks the resource as held.
/// An implicit lockdep edge `rx → tx` is registered at creation time.
pub mod oneshot {
    pub use super::channel::oneshot::{Receiver, Sender, channel, named_channel};
    pub use tokio::sync::oneshot::error;
}
