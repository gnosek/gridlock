use crate::observer::{DefaultObserver, Id, Resource, ResourceObserver, observer};
use std::panic::Location;
/// Instrumented wrapper around [`tokio::sync::Barrier`].
///
/// `wait()` reports wait/acquire to the observer. After release, the
/// barrier resource **stays** on the held-lock stack permanently so that
/// any later acquisition records `barrier → resource`.
pub struct Barrier<O: ResourceObserver = DefaultObserver> {
    id: Id,
    inner: tokio::sync::Barrier,
    observer: O,
}
impl Barrier<DefaultObserver> {
    /// Create a barrier with the default observer using the call site location.
    #[track_caller]
    pub fn new(n: usize) -> Self {
        let caller = Location::caller();
        Self {
            id: Id::unnamed(caller),
            inner: tokio::sync::Barrier::new(n),
            observer: observer(),
        }
    }
    /// Create a named barrier with the default observer.
    #[track_caller]
    pub fn named(name: &'static str, n: usize) -> Self {
        Self {
            id: Id::named(name),
            inner: tokio::sync::Barrier::new(n),
            observer: observer(),
        }
    }
}
impl<O: ResourceObserver> Barrier<O> {
    /// Wait until all participants have arrived.
    ///
    /// The ordering edge `held_lock → barrier` is recorded at wait time.
    /// After the barrier releases, `barrier` stays on the held-lock stack
    /// permanently — any subsequent resource acquisition records
    /// `barrier → resource`.
    #[track_caller]
    pub fn wait(&self) -> impl Future<Output = tokio::sync::BarrierWaitResult> + '_ {
        let caller = Location::caller();
        let observer = &self.observer;
        let id = Resource::Barrier(self.id);
        let inner = &self.inner;
        async move {
            observer.on_waiting(&id, caller);
            let result = inner.wait().await;
            observer.on_acquired(&id, caller);
            result
        }
    }

    /// Get the resource id
    pub fn id(&self) -> Id {
        self.id
    }
}
