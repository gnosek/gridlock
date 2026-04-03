use crate::observer::{DefaultObserver, Id, Resource, ResourceObserver, observer};
use std::panic::Location;
/// Instrumented wrapper around [`tokio::sync::Semaphore`].
///
/// `acquire()` reports wait/acquire to the observer. Note: the returned
/// permit is the **raw** [`tokio::sync::SemaphorePermit`] — `on_released` is
/// not called when the permit is dropped (semaphores are typically not
/// part of lock-ordering cycles).
pub struct Semaphore<O: ResourceObserver = DefaultObserver> {
    id: Id,
    inner: tokio::sync::Semaphore,
    observer: O,
}
impl Semaphore<DefaultObserver> {
    /// Create a semaphore with the default observer using the call site location.
    #[track_caller]
    pub fn new(permits: usize) -> Self {
        let caller = Location::caller();
        Self {
            id: Id::unnamed(caller),
            inner: tokio::sync::Semaphore::new(permits),
            observer: observer(),
        }
    }
    /// Create a named semaphore with the default observer.
    #[track_caller]
    pub fn named(name: &'static str, permits: usize) -> Self {
        Self {
            id: Id::named(name),
            inner: tokio::sync::Semaphore::new(permits),
            observer: observer(),
        }
    }
}
impl<O: ResourceObserver> Semaphore<O> {
    /// Acquire a permit from the semaphore.
    #[track_caller]
    pub fn acquire(&self) -> impl Future<Output = tokio::sync::SemaphorePermit<'_>> + '_ {
        let caller = Location::caller();
        async move {
            let id = Resource::Semaphore(self.id);
            self.observer.on_waiting(&id, caller);
            let permit = self.inner.acquire().await.unwrap();
            self.observer.on_acquired(&id, caller);
            permit
        }
    }

    /// Get the resource id
    pub fn id(&self) -> Id {
        self.id
    }
}
