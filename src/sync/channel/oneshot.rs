//! Instrumented wrapper for `tokio::sync::oneshot`.
//!
//! [`Sender`] and [`Receiver`] wrap the tokio oneshot
//! channel and report to a [`ResourceObserver`].
//!
//! - **`send()`** is non-blocking, so it does immediate acquire + release
//!   (no ordering edge remains after `send()` completes).
//!
//! - **`recv()`** blocks until the sender fires.  Once it completes, the
//!   oneshot resource
//!   stays on the held-lock stack permanently — any resource acquired later
//!   in the same task records `oneshot → resource`.
use crate::observer::lockdep::LockDepObserver;
use crate::observer::{DefaultObserver, Id, Resource, ResourceObserver, observer};
use std::panic::Location;

/// Create an unnamed oneshot channel using the crate-wide default observer.
#[track_caller]
pub fn channel<T>() -> (Sender<T, DefaultObserver>, Receiver<T, DefaultObserver>) {
    let caller = Location::caller();
    let observer = observer();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let id = Id::unnamed(caller);
    let resource = Resource::Oneshot(id);
    LockDepObserver::register_oneshot(resource, resource);
    (
        Sender {
            id,
            inner: Some(tx),
            observer: observer.clone(),
        },
        Receiver {
            id,
            inner: Some(rx),
            observer: observer.clone(),
        },
    )
}

/// Create a named oneshot channel using the crate-wide default observer.
#[track_caller]
pub fn named_channel<T>(
    name: &'static str,
) -> (Sender<T, DefaultObserver>, Receiver<T, DefaultObserver>) {
    let observer = observer();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let id = Id::named(name);
    let resource = Resource::Oneshot(id);
    LockDepObserver::register_oneshot(resource, resource);
    (
        Sender {
            id,
            inner: Some(tx),
            observer: observer.clone(),
        },
        Receiver {
            id,
            inner: Some(rx),
            observer: observer.clone(),
        },
    )
}

/// Instrumented wrapper around `tokio::sync::oneshot::Sender`.
///
/// `send()` is non-blocking (never waits) so it only reports to the observer
/// for tracing purposes, not for lockdep ordering.
pub struct Sender<T, O: ResourceObserver = DefaultObserver> {
    id: Id,
    inner: Option<tokio::sync::oneshot::Sender<T>>,
    observer: O,
}
impl<T, O: ResourceObserver> Sender<T, O> {
    /// Send a value.  This never blocks, so it doesn't create a lockdep
    /// ordering edge — but it is traced.
    #[track_caller]
    pub fn send(mut self, value: T) -> Result<(), T> {
        let caller = Location::caller();
        let tx = self.inner.take().expect("oneshot sender already consumed");
        let id = Resource::Oneshot(self.id);
        self.observer.on_waiting(&id, caller);
        match tx.send(value) {
            Ok(()) => {
                self.observer.on_acquired(&id, caller);
                self.observer.on_released(&id, caller);
                Ok(())
            }
            Err(v) => Err(v),
        }
    }

    /// Get the resource id
    pub fn id(&self) -> Id {
        self.id
    }
}

/// Instrumented wrapper around `tokio::sync::oneshot::Receiver`.
///
/// `.recv()` blocks until the sender fires.  Once it completes, the oneshot
/// resource is
/// pushed onto the held-lock stack **and never removed** — the ordering
/// relationship "this task awaited the oneshot and later acquired some resource"
/// is a permanent fact about the task's behaviour.
pub struct Receiver<T, O: ResourceObserver = DefaultObserver> {
    id: Id,
    inner: Option<tokio::sync::oneshot::Receiver<T>>,
    observer: O,
}
impl<T, O: ResourceObserver> Receiver<T, O> {
    /// Receive the value, blocking until the sender sends.
    ///
    /// After a successful receive, the oneshot resource is permanently marked
    /// as held for
    /// this task.  Any subsequent resource acquisition records the ordering
    /// edge `oneshot → resource`.
    ///
    /// Returns `Err` if the sender was dropped without sending.
    #[track_caller]
    pub fn recv(
        &mut self,
    ) -> impl Future<Output = Result<T, tokio::sync::oneshot::error::RecvError>> + '_ {
        let caller = Location::caller();
        let rx = self
            .inner
            .take()
            .expect("oneshot receiver already consumed");
        let observer = &self.observer;
        let id = self.id;
        async move {
            let id = Resource::Oneshot(id);
            observer.on_waiting(&id, caller);
            match rx.await {
                Ok(value) => {
                    observer.on_acquired(&id, caller);
                    // No on_released: oneshot stays on the held-lock stack so
                    // that any later resource acquisition records oneshot -> resource.
                    Ok(value)
                }
                Err(e) => Err(e),
            }
        }
    }

    /// Get the resource id
    pub fn id(&self) -> Id {
        self.id
    }
}
