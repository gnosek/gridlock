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
use std::future::Future;
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

    /// Returns the number of queued messages.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if the channel is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns the number of active receivers.
    pub fn receiver_count(&self) -> usize {
        self.inner.receiver_count()
    }

    /// Returns `true` if both senders belong to the same channel.
    pub fn same_channel(&self, other: &Self) -> bool {
        self.inner.same_channel(&other.inner)
    }

    /// Wait until all receivers have been dropped.
    pub fn closed(&self) -> impl Future<Output = ()> + '_ {
        self.inner.closed()
    }

    /// Returns the number of strong senders.
    pub fn strong_count(&self) -> usize {
        self.inner.strong_count()
    }

    /// Returns the number of weak senders.
    pub fn weak_count(&self) -> usize {
        self.inner.weak_count()
    }

    /// Get the resource id.
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

    /// Try to receive without waiting.
    pub fn try_recv(&mut self) -> Result<T, tokio::sync::broadcast::error::TryRecvError> {
        self.inner.try_recv()
    }

    /// Blocking receive — panics if called from an async context.
    #[track_caller]
    pub fn blocking_recv(&mut self) -> Result<T, tokio::sync::broadcast::error::RecvError> {
        let caller = Location::caller();
        let id = Resource::Broadcast(self.id);
        self.observer.on_waiting(&id, caller);
        let result = self.inner.blocking_recv();
        if result.is_ok() {
            self.observer.on_acquired(&id, caller);
        }
        result
    }

    /// Returns the number of queued messages.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if the channel is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns `true` if both receivers belong to the same channel.
    pub fn same_channel(&self, other: &Self) -> bool {
        self.inner.same_channel(&other.inner)
    }

    /// Resubscribe to the broadcast channel (creates a new receiver
    /// that will only see messages sent after this call).
    pub fn resubscribe(&self) -> Self
    where
        O: Clone,
    {
        Receiver {
            id: self.id,
            inner: self.inner.resubscribe(),
            observer: self.observer.clone(),
        }
    }

    /// Returns `true` if the sender has been dropped.
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    /// Returns the number of strong senders.
    pub fn sender_strong_count(&self) -> usize {
        self.inner.sender_strong_count()
    }

    /// Returns the number of weak senders.
    pub fn sender_weak_count(&self) -> usize {
        self.inner.sender_weak_count()
    }
}
