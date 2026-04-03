use crate::observer::{DefaultObserver, Id, Resource, ResourceObserver, observer};
use std::panic::Location;
/// Instrumented wrapper around [`tokio::sync::Notify`].
///
/// `notified().await` reports wait/acquire to the observer.
/// `notify_one()` and `notify_waiters()` are pass-through (non-blocking,
/// not instrumented for lockdep).
pub struct Notify<O: ResourceObserver = DefaultObserver> {
    id: Id,
    inner: tokio::sync::Notify,
    observer: O,
}
impl Notify<DefaultObserver> {
    /// Create a notifier with the default observer using the call site location.
    #[track_caller]
    pub fn new() -> Self {
        let caller = Location::caller();
        Self {
            id: Id::unnamed(caller),
            inner: tokio::sync::Notify::new(),
            observer: observer(),
        }
    }
    /// Create a named notifier with the default observer.
    #[track_caller]
    pub fn named(name: &'static str) -> Self {
        Self {
            id: Id::named(name),
            inner: tokio::sync::Notify::new(),
            observer: observer(),
        }
    }
}
impl<O: ResourceObserver> Notify<O> {
    /// Wait until notified.
    #[track_caller]
    pub fn notified(&self) -> impl Future<Output = ()> + '_ {
        let caller = Location::caller();
        async move {
            let id = Resource::Notify(self.id);
            self.observer.on_waiting(&id, caller);
            self.inner.notified().await;
            self.observer.on_acquired(&id, caller);
        }
    }
    /// Notify all waiters.
    pub fn notify_waiters(&self) {
        self.inner.notify_waiters();
    }
    /// Notify one waiter.
    pub fn notify_one(&self) {
        self.inner.notify_one();
    }
    /// Notify the last waiter.
    pub fn notify_last(&self) {
        self.inner.notify_last();
    }

    /// Get the resource id
    pub fn id(&self) -> Id {
        self.id
    }
}
