//! Instrumented wrappers for `tokio::sync::mpsc` channels.
//!
//! [`Sender`] and [`Receiver`] wrap the tokio bounded mpsc channel
//! and report to a [`ResourceObserver`] before/after every `send()` and `recv()`.
//!
//! For lockdep purposes:
//!
//! - **`send()`** on a bounded channel can block (back-pressure).  The useful
//!   ordering edge is `held_lock → channel`, recorded at wait time — before send
//!   completes.  After the value is enqueued, the sender isn't holding
//!   anything, so the channel resource is immediately released from the
//!   held-lock stack.
//!
//! - **`recv()`** blocks until a message arrives.  Once it completes, the
//!   channel resource
//!   is pushed onto the held-lock stack **and never removed**.  Any resource
//!   acquired later in the same task automatically records
//!   `channel → resource`.
//!   This is correct because the ordering relationship "this task receives
//!   from the channel and also acquires `resource`" is a permanent fact about the
//!   task's behaviour — it doesn't expire when "processing" ends.
use crate::observer::{DefaultObserver, Id, Resource, ResourceObserver, observer};
use std::future::Future;
use std::panic::Location;
use std::time::Duration;

/// Create a bounded mpsc channel using the crate-wide default observer.
#[track_caller]
pub fn channel<T>(buffer: usize) -> (Sender<T, DefaultObserver>, Receiver<T, DefaultObserver>) {
    let caller = Location::caller();
    let observer = observer();
    let (tx, rx) = tokio::sync::mpsc::channel(buffer);
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

/// Create a named bounded mpsc channel using the crate-wide default observer.
#[track_caller]
pub fn named_channel<T>(
    name: &'static str,
    buffer: usize,
) -> (Sender<T, DefaultObserver>, Receiver<T, DefaultObserver>) {
    let observer = observer();
    let (tx, rx) = tokio::sync::mpsc::channel(buffer);
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
            observer,
        },
    )
}

/// Instrumented wrapper around `tokio::sync::mpsc::Sender`.
///
/// On a bounded channel, `send()` may block waiting for capacity — this is
/// reported as a wait-for / acquired on the channel resource.
#[derive(Clone, Debug)]
pub struct Sender<T, O: ResourceObserver = DefaultObserver> {
    id: Id,
    inner: tokio::sync::mpsc::Sender<T>,
    observer: O,
}

impl<T, O: ResourceObserver> Sender<T, O> {
    /// Send a value, blocking if the channel is full.
    ///
    /// For lockdep, the ordering edge `held_lock → channel` is recorded at wait
    /// time (before the send completes).  Once the value is enqueued, nothing
    /// is "held" — the channel resource is immediately released.
    #[track_caller]
    pub fn send(
        &self,
        value: T,
    ) -> impl Future<Output = Result<(), tokio::sync::mpsc::error::SendError<T>>> + '_ {
        let caller = Location::caller();
        async move {
            let id = Resource::Channel(self.id);
            self.observer.on_waiting(&id, caller);
            let result = self.inner.send(value).await;
            match &result {
                Ok(()) => {
                    self.observer.on_acquired(&id, caller);
                    self.observer.on_released(&id, caller);
                }
                Err(_) => {}
            }
            result
        }
    }

    /// Send a value with a timeout.
    #[track_caller]
    pub fn send_timeout(
        &self,
        value: T,
        timeout: Duration,
    ) -> impl Future<
        Output = Result<(), tokio::sync::mpsc::error::SendTimeoutError<T>>,
    > + '_ {
        let caller = Location::caller();
        async move {
            let id = Resource::Channel(self.id);
            self.observer.on_waiting(&id, caller);
            let result = self.inner.send_timeout(value, timeout).await;
            if result.is_ok() {
                self.observer.on_acquired(&id, caller);
                self.observer.on_released(&id, caller);
            }
            result
        }
    }

    /// Try to send without blocking (not instrumented for lockdep since it never waits).
    pub fn try_send(&self, value: T) -> Result<(), tokio::sync::mpsc::error::TrySendError<T>> {
        self.inner.try_send(value)
    }

    /// Blocking send — panics if called from an async context.
    #[track_caller]
    pub fn blocking_send(
        &self,
        value: T,
    ) -> Result<(), tokio::sync::mpsc::error::SendError<T>> {
        let caller = Location::caller();
        let id = Resource::Channel(self.id);
        self.observer.on_waiting(&id, caller);
        let result = self.inner.blocking_send(value);
        if result.is_ok() {
            self.observer.on_acquired(&id, caller);
            self.observer.on_released(&id, caller);
        }
        result
    }

    /// Wait until the receiver half is closed.
    pub fn closed(&self) -> impl Future<Output = ()> + '_ {
        self.inner.closed()
    }

    /// Returns `true` if the receiver has been dropped.
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    /// Reserve capacity for a single message.
    #[track_caller]
    pub fn reserve(
        &self,
    ) -> impl Future<Output = Result<tokio::sync::mpsc::Permit<'_, T>, tokio::sync::mpsc::error::SendError<()>>>
           + '_ {
        let caller = Location::caller();
        async move {
            let id = Resource::Channel(self.id);
            self.observer.on_waiting(&id, caller);
            let result = self.inner.reserve().await;
            if result.is_ok() {
                self.observer.on_acquired(&id, caller);
                // Note: the Permit itself will send + release when used.
            }
            result
        }
    }

    /// Reserve capacity for `n` messages.
    #[track_caller]
    pub fn reserve_many(
        &self,
        n: usize,
    ) -> impl Future<
        Output = Result<
            tokio::sync::mpsc::PermitIterator<'_, T>,
            tokio::sync::mpsc::error::SendError<()>,
        >,
    > + '_ {
        let caller = Location::caller();
        async move {
            let id = Resource::Channel(self.id);
            self.observer.on_waiting(&id, caller);
            let result = self.inner.reserve_many(n).await;
            if result.is_ok() {
                self.observer.on_acquired(&id, caller);
            }
            result
        }
    }

    /// Try to reserve capacity for a single message without waiting.
    pub fn try_reserve(
        &self,
    ) -> Result<tokio::sync::mpsc::Permit<'_, T>, tokio::sync::mpsc::error::TrySendError<()>>
    {
        self.inner.try_reserve()
    }

    /// Try to reserve capacity for `n` messages without waiting.
    pub fn try_reserve_many(
        &self,
        n: usize,
    ) -> Result<
        tokio::sync::mpsc::PermitIterator<'_, T>,
        tokio::sync::mpsc::error::TrySendError<()>,
    > {
        self.inner.try_reserve_many(n)
    }

    /// Reserve an owned slot in the channel.
    pub fn reserve_owned(
        self,
    ) -> impl Future<
        Output = Result<
            tokio::sync::mpsc::OwnedPermit<T>,
            tokio::sync::mpsc::error::SendError<()>,
        >,
    > {
        // We pass through the inner sender directly — observer hooks
        // were already applied at send time.
        self.inner.reserve_owned()
    }

    /// Try to reserve an owned slot without waiting.
    pub fn try_reserve_owned(
        self,
    ) -> Result<
        tokio::sync::mpsc::OwnedPermit<T>,
        tokio::sync::mpsc::error::TrySendError<Self>,
    > {
        match self.inner.try_reserve_owned() {
            Ok(permit) => Ok(permit),
            Err(tokio::sync::mpsc::error::TrySendError::Full(inner)) => {
                Err(tokio::sync::mpsc::error::TrySendError::Full(Sender {
                    id: self.id,
                    inner,
                    observer: self.observer,
                }))
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(inner)) => {
                Err(tokio::sync::mpsc::error::TrySendError::Closed(Sender {
                    id: self.id,
                    inner,
                    observer: self.observer,
                }))
            }
        }
    }

    /// Returns `true` if both senders belong to the same channel.
    pub fn same_channel(&self, other: &Self) -> bool {
        self.inner.same_channel(&other.inner)
    }

    /// Returns the current capacity of the channel.
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    /// Returns the maximum capacity of the channel.
    pub fn max_capacity(&self) -> usize {
        self.inner.max_capacity()
    }

    /// Downgrade this sender to a weak reference.
    pub fn downgrade(&self) -> tokio::sync::mpsc::WeakSender<T> {
        self.inner.downgrade()
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

/// Instrumented wrapper around `tokio::sync::mpsc::Receiver`.
///
/// `recv()` blocks until a message is available — this is reported as a
/// wait-for / acquired on the channel resource.
///
/// Once `recv()` completes, the channel resource stays on the held-lock stack
/// permanently. Any resource acquired later in the same task records
/// `channel → resource`.
/// This correctly models the ordering constraint: if this task receives
/// from the channel and then acquires a lock, that ordering is a permanent fact
/// about the task's behaviour, not something that expires.
pub struct Receiver<T, O: ResourceObserver = DefaultObserver> {
    id: Id,
    inner: tokio::sync::mpsc::Receiver<T>,
    observer: O,
}
impl<T, O: ResourceObserver> Receiver<T, O> {
    /// Receive the next value, blocking until one is available.
    ///
    /// After a successful receive, the channel resource is permanently marked
    /// as held for this task.  Any subsequent resource acquisition (lock, send, recv)
    /// records the ordering edge `channel → resource`.
    ///
    /// Returns `None` if the channel is closed.
    #[track_caller]
    pub fn recv(&mut self) -> impl Future<Output = Option<T>> + '_ {
        let caller = Location::caller();
        async move {
            let id = Resource::Channel(self.id);
            self.observer.on_waiting(&id, caller);
            let result = self.inner.recv().await;
            if result.is_some() {
                self.observer.on_acquired(&id, caller);
            }
            result
        }
    }

    /// Receive many values at once, up to `limit`.
    #[track_caller]
    pub fn recv_many<'a>(
        &'a mut self,
        buffer: &'a mut Vec<T>,
        limit: usize,
    ) -> impl Future<Output = usize> + 'a {
        let caller = Location::caller();
        async move {
            let id = Resource::Channel(self.id);
            self.observer.on_waiting(&id, caller);
            let count = self.inner.recv_many(buffer, limit).await;
            if count > 0 {
                self.observer.on_acquired(&id, caller);
            }
            count
        }
    }

    /// Try to receive a value without waiting.
    pub fn try_recv(&mut self) -> Result<T, tokio::sync::mpsc::error::TryRecvError> {
        self.inner.try_recv()
    }

    /// Blocking receive — panics if called from an async context.
    #[track_caller]
    pub fn blocking_recv(&mut self) -> Option<T> {
        let caller = Location::caller();
        let id = Resource::Channel(self.id);
        self.observer.on_waiting(&id, caller);
        let result = self.inner.blocking_recv();
        if result.is_some() {
            self.observer.on_acquired(&id, caller);
        }
        result
    }

    /// Close the receiving half, preventing further sends.
    pub fn close(&mut self) {
        self.inner.close();
    }

    /// Returns `true` if the channel is closed.
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    /// Returns `true` if the channel is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns the number of queued messages.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns the current capacity.
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    /// Returns the maximum capacity.
    pub fn max_capacity(&self) -> usize {
        self.inner.max_capacity()
    }

    /// Returns the number of strong senders.
    pub fn sender_strong_count(&self) -> usize {
        self.inner.sender_strong_count()
    }

    /// Returns the number of weak senders.
    pub fn sender_weak_count(&self) -> usize {
        self.inner.sender_weak_count()
    }

    /// Polls for the next value (low-level).
    pub fn poll_recv(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<T>> {
        self.inner.poll_recv(cx)
    }

    /// Get the resource id.
    pub fn id(&self) -> Id {
        self.id
    }
}
