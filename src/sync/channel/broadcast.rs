//! Instrumented wrapper for `tokio::sync::broadcast`.
//!
//! [`Sender`] and [`Receiver`] wrap the tokio
//! broadcast channel and report to a [`ResourceObserver`].
//!
//! A broadcast channel delivers each sent value to *every* active receiver.
//!
//! For lockdep purposes:
//!
//! - **`send()`** is non-blocking — immediate acquire + release (records
//!   `held_lock → broadcast` at wait time for tracing).
//!
//! - **`recv().await`** blocks until a message is available.  Once it
//!   completes, the broadcast resource stays on the held-lock stack
//!   permanently — any resource acquired later records
//!   `broadcast → resource`.
use crate::observer::{DefaultObserver, Id, Resource, ResourceObserver, observer};
use std::panic::Location;

/// Create a broadcast channel with an explicit `Id` and the crate-wide default observer.
#[track_caller]
pub fn channel<T: Clone>(
    id: Id,
    capacity: usize,
) -> (Sender<T, DefaultObserver>, Receiver<T, DefaultObserver>) {
    let observer = observer();
    let (tx, rx) = tokio::sync::broadcast::channel(capacity);
    (
        Sender {
            id,
            rx_id: id,
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

/// Instrumented wrapper around `tokio::sync::broadcast::Sender`.
///
/// `send()` is non-blocking — it delivers the value to all receivers and
/// returns immediately.  Immediate acquire + release for tracing.
#[derive(Clone, Debug)]
pub struct Sender<T: Clone, O: ResourceObserver = DefaultObserver> {
    /// The resource identifier for this sender.
    pub id: Id,
    rx_id: Id,
    inner: tokio::sync::broadcast::Sender<T>,
    observer: O,
}

impl<T: Clone, O: ResourceObserver> Sender<T, O> {
    /// Send a value to all active receivers.
    ///
    /// Non-blocking — records immediate acquire + release.
    /// Returns `Err` if there are no active receivers.
    #[track_caller]
    pub fn send(&self, value: T) -> Result<usize, tokio::sync::broadcast::error::SendError<T>> {
        let caller = Location::caller();
        let id = Resource::Broadcast(self.id);
        self.observer.on_waiting(&id, caller);
        let result = self.inner.send(value);
        match &result {
            Ok(_) => {
                self.observer.on_acquired(&id, caller);
                self.observer.on_released(&id, caller);
            }
            Err(_) => {}
        }
        result
    }

    /// Create a new receiver subscribed to this sender.
    pub fn subscribe(&self) -> Receiver<T, O>
    where
        O: Clone,
    {
        Receiver {
            id: self.rx_id,
            inner: self.inner.subscribe(),
            observer: self.observer.clone(),
        }
    }

    /// Get the resource id
    pub fn id(&self) -> Id {
        self.id
    }
}


/// Instrumented wrapper around `tokio::sync::broadcast::Receiver`.
///
/// `recv().await` blocks until a message is available.  Once it completes,
/// the broadcast resource stays on the held-lock stack permanently.
pub struct Receiver<T: Clone, O: ResourceObserver = DefaultObserver> {
    /// The resource identifier for this receiver.
    pub id: Id,
    inner: tokio::sync::broadcast::Receiver<T>,
    observer: O,
}

impl<T: Clone, O: ResourceObserver> Receiver<T, O> {
    /// Receive the next broadcast message, blocking until one is available.
    ///
    /// After a successful receive, the broadcast resource is permanently
    /// marked as held. Any subsequent resource acquisition records
    /// `broadcast → resource`.
    ///
    /// Returns `Err` on lag (missed messages) or if the sender was dropped.
    #[track_caller]
    pub fn recv(
        &mut self,
    ) -> impl Future<Output = Result<T, tokio::sync::broadcast::error::RecvError>> + '_ {
        let caller = Location::caller();
        async move {
            let id = Resource::Broadcast(self.id);
            self.observer.on_waiting(&id, caller);
            let result = self.inner.recv().await;
            if result.is_ok() {
                self.observer.on_acquired(&id, caller);
                // No on_released: broadcast stays on the held-lock stack permanently.
            }
            result
        }
    }
}
