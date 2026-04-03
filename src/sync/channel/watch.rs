//! Instrumented wrapper for `tokio::sync::watch`.
//!
//! [`Sender`] and [`Receiver`] wrap the tokio watch
//! channel and report to a [`ResourceObserver`].
//!
//! A watch channel holds a single value; receivers see the *latest* value
//! and can wait for changes.
//!
//! For lockdep purposes:
//!
//! - **`send()`** is non-blocking — immediate acquire + release (records
//!   `held_lock → watch` at wait time for tracing).
//!
//! - **`changed().await`** blocks until a new value is available.  Once it
//!   completes, the watch resource stays on the held-lock stack permanently —
//!   any resource acquired later records `watch → resource`.
//!
//! - **`borrow()`** and **`borrow_and_update()`** are non-blocking reads,
//!   not instrumented for lockdep.
use crate::observer::{DefaultObserver, Id, Resource, ResourceObserver, observer};
use std::panic::Location;

/// Create a watch channel using the crate-wide default observer.
#[track_caller]
pub fn channel<T>(init: T) -> (Sender<T, DefaultObserver>, Receiver<T, DefaultObserver>) {
    let caller = Location::caller();
    let observer = observer();
    let (tx, rx) = tokio::sync::watch::channel(init);
    let id = Id::unnamed(caller);
    (
        Sender {
            id,
            inner: tx,
            observer: observer.clone(),
        },
        Receiver {
            id,
            inner: rx,
            observer: observer.clone(),
        },
    )
}

/// Create a named watch channel using the crate-wide default observer.
#[track_caller]
pub fn named_channel<T>(
    name: &'static str,
    init: T,
) -> (Sender<T, DefaultObserver>, Receiver<T, DefaultObserver>) {
    let observer = observer();
    let (tx, rx) = tokio::sync::watch::channel(init);
    let id = Id::named(name);
    (
        Sender {
            id,
            inner: tx,
            observer: observer.clone(),
        },
        Receiver {
            id,
            inner: rx,
            observer: observer.clone(),
        },
    )
}

/// Instrumented wrapper around `tokio::sync::watch::Sender`.
///
/// `send()` is non-blocking — it replaces the current value and wakes all
/// receivers.  Immediate acquire + release for tracing.
#[derive(Clone, Debug)]
pub struct Sender<T, O: ResourceObserver = DefaultObserver> {
    id: Id,
    inner: tokio::sync::watch::Sender<T>,
    observer: O,
}
impl<T, O: ResourceObserver> Sender<T, O> {
    /// Replace the watched value, waking all receivers.
    ///
    /// Non-blocking — records immediate acquire + release.
    #[track_caller]
    pub fn send(&self, value: T) -> Result<(), tokio::sync::watch::error::SendError<T>> {
        let caller = Location::caller();
        let id = Resource::Watch(self.id);
        self.observer.on_waiting(&id, caller);
        let result = self.inner.send(value);
        match &result {
            Ok(()) => {
                self.observer.on_acquired(&id, caller);
                self.observer.on_released(&id, caller);
            }
            Err(_) => {}
        }
        result
    }
    /// Modify the watched value in-place, waking receivers if the closure
    /// returns `true`.
    ///
    /// Non-blocking — not instrumented for lockdep.
    pub fn send_modify<F: FnOnce(&mut T)>(&self, func: F) {
        self.inner.send_modify(func);
    }
    /// Returns a reference to the most recent value.
    pub fn borrow(&self) -> tokio::sync::watch::Ref<'_, T> {
        self.inner.borrow()
    }

    /// Get the resource id
    pub fn id(&self) -> Id {
        self.id
    }
}

/// Instrumented wrapper around `tokio::sync::watch::Receiver`.
///
/// `changed().await` blocks until a new value is sent.  Once it completes,
/// the watch resource stays on the held-lock stack permanently.
#[derive(Clone, Debug)]
pub struct Receiver<T, O: ResourceObserver = DefaultObserver> {
    id: Id,
    inner: tokio::sync::watch::Receiver<T>,
    observer: O,
}
impl<T, O: ResourceObserver> Receiver<T, O> {
    /// Wait until the watched value changes.
    ///
    /// After a successful return, the watch resource is permanently marked as
    /// held. Any subsequent resource acquisition records `watch → resource`.
    ///
    /// Returns `Err` if the sender was dropped.
    #[track_caller]
    pub fn changed(
        &mut self,
    ) -> impl Future<Output = Result<(), tokio::sync::watch::error::RecvError>> + '_ {
        let caller = Location::caller();
        async move {
            let id = Resource::Watch(self.id);
            self.observer.on_waiting(&id, caller);
            let result = self.inner.changed().await;
            if result.is_ok() {
                self.observer.on_acquired(&id, caller);
                // No on_released: watch stays on the held-lock stack permanently.
            }
            result
        }
    }
    /// Returns a reference to the most recent value (non-blocking, not
    /// instrumented for lockdep).
    pub fn borrow(&self) -> tokio::sync::watch::Ref<'_, T> {
        self.inner.borrow()
    }
    /// Returns a reference to the most recent value and marks it as seen
    /// (non-blocking, not instrumented for lockdep).
    pub fn borrow_and_update(&mut self) -> tokio::sync::watch::Ref<'_, T> {
        self.inner.borrow_and_update()
    }

    /// Get the resource id
    pub fn id(&self) -> Id {
        self.id
    }
}
