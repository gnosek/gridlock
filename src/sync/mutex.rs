use crate::observer::{DefaultObserver, Id, Resource, ResourceObserver, observer};
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::panic::Location;

/// Instrumented wrapper around [`tokio::sync::Mutex`].
///
/// Every `lock().await` calls the observer's `on_waiting` / `on_acquired`
/// hooks; the returned [`MutexGuard`] calls `on_released` when dropped.
///
/// Use `new()` for location-tracked resources or `named()` for
/// human-readable diagnostics.
pub struct Mutex<T, O: ResourceObserver = DefaultObserver> {
    id: Id,
    inner: tokio::sync::Mutex<T>,
    observer: O,
}
impl<T> Mutex<T, DefaultObserver> {
    /// Create a mutex with the default observer using the call site location.
    #[track_caller]
    pub fn new(value: T) -> Self {
        let caller = Location::caller();
        Self {
            id: Id::unnamed(caller),
            inner: tokio::sync::Mutex::new(value),
            observer: observer(),
        }
    }
    /// Create a named mutex with the default observer.
    #[track_caller]
    pub fn named(name: &'static str, value: T) -> Self {
        Self {
            id: Id::named(name),
            inner: tokio::sync::Mutex::new(value),
            observer: observer(),
        }
    }
}

impl<T, O: ResourceObserver> Mutex<T, O> {
    /// Acquire the mutex, reporting to the observer before and after.
    #[track_caller]
    pub fn lock(&self) -> impl Future<Output = MutexGuard<'_, T, O>> {
        let caller = Location::caller();
        async move {
            let id = Resource::Mutex(self.id);
            self.observer.on_waiting(&id, caller);
            let guard = self.inner.lock().await;
            self.observer.on_acquired(&id, caller);
            MutexGuard {
                guard,
                id: self.id,
                observer: &self.observer,
                caller,
            }
        }
    }

    /// Get the resource id
    pub fn id(&self) -> Id {
        self.id
    }
}
/// RAII guard that reports `on_released` when the lock is dropped.
pub struct MutexGuard<'a, T, O: ResourceObserver = DefaultObserver> {
    guard: tokio::sync::MutexGuard<'a, T>,
    id: Id,
    observer: &'a O,
    caller: &'static Location<'static>,
}
impl<T, O: ResourceObserver> Deref for MutexGuard<'_, T, O> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.guard
    }
}
impl<T, O: ResourceObserver> DerefMut for MutexGuard<'_, T, O> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}
impl<T, O: ResourceObserver> Drop for MutexGuard<'_, T, O> {
    fn drop(&mut self) {
        let id = Resource::Mutex(self.id);
        self.observer.on_released(&id, self.caller);
    }
}
