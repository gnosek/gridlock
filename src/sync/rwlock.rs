//! Instrumented wrapper for `tokio::sync::RwLock`.
//!
//! [`RwLock`] wraps a tokio `RwLock` and reports to a [`ResourceObserver`]
//! before/after every `.read()` and `.write()` call.
//!
//! For lockdep, both read/write paths currently use the same
//! `Resource::RwLock(id)` node.
use crate::observer::{DefaultObserver, Id, Resource, ResourceObserver, observer};
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::panic::Location;

/// Instrumented wrapper around [`tokio::sync::RwLock`].
///
/// Both `read()` and `write()` report to the observer. For lockdep, both
/// paths use the same resource node — a read/write distinction is not
/// currently modelled.
pub struct RwLock<T, O: ResourceObserver = DefaultObserver> {
    id: Id,
    inner: tokio::sync::RwLock<T>,
    observer: O,
}
impl<T> RwLock<T, DefaultObserver> {
    /// Create an RwLock with the default observer using the call site location.
    #[track_caller]
    pub fn new(value: T) -> Self {
        let caller = Location::caller();
        Self {
            id: Id::unnamed(caller),
            inner: tokio::sync::RwLock::new(value),
            observer: observer(),
        }
    }
    /// Create a named RwLock with the default observer.
    #[track_caller]
    pub fn named(name: &'static str, value: T) -> Self {
        Self {
            id: Id::named(name),
            inner: tokio::sync::RwLock::new(value),
            observer: observer(),
        }
    }
}
impl<T, O: ResourceObserver> RwLock<T, O> {
    /// Acquire a read lock.
    #[track_caller]
    pub fn read(&self) -> impl Future<Output = RwLockReadGuard<'_, T, O>> {
        let caller = Location::caller();
        async move {
            let id = Resource::RwLock(self.id);
            self.observer.on_waiting(&id, caller);
            let guard = self.inner.read().await;
            self.observer.on_acquired(&id, caller);
            RwLockReadGuard {
                guard,
                id: self.id,
                observer: &self.observer,
            }
        }
    }
    /// Acquire a write lock.
    #[track_caller]
    pub fn write(&self) -> impl Future<Output = RwLockWriteGuard<'_, T, O>> {
        let caller = Location::caller();
        async move {
            let id = Resource::RwLock(self.id);
            self.observer.on_waiting(&id, caller);
            let guard = self.inner.write().await;
            self.observer.on_acquired(&id, caller);
            RwLockWriteGuard {
                guard,
                id: self.id,
                observer: &self.observer,
            }
        }
    }

    /// Get the resource id
    pub fn id(&self) -> Id {
        self.id
    }
}
// ─── Read guard ─────────────────────────────────────────────────────────────

/// RAII read guard that reports `on_released` when dropped.
pub struct RwLockReadGuard<'a, T, O: ResourceObserver = DefaultObserver> {
    guard: tokio::sync::RwLockReadGuard<'a, T>,
    id: Id,
    observer: &'a O,
}
impl<T, O: ResourceObserver> Deref for RwLockReadGuard<'_, T, O> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.guard
    }
}
impl<T, O: ResourceObserver> Drop for RwLockReadGuard<'_, T, O> {
    #[track_caller]
    fn drop(&mut self) {
        let caller = Location::caller();
        let id = Resource::RwLock(self.id);
        self.observer.on_released(&id, caller);
    }
}
// ─── Write guard ────────────────────────────────────────────────────────────

/// RAII write guard that reports `on_released` when dropped.
pub struct RwLockWriteGuard<'a, T, O: ResourceObserver = DefaultObserver> {
    guard: tokio::sync::RwLockWriteGuard<'a, T>,
    id: Id,
    observer: &'a O,
}

impl<T, O: ResourceObserver> Deref for RwLockWriteGuard<'_, T, O> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.guard
    }
}
impl<T, O: ResourceObserver> DerefMut for RwLockWriteGuard<'_, T, O> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}
impl<T, O: ResourceObserver> Drop for RwLockWriteGuard<'_, T, O> {
    #[track_caller]
    fn drop(&mut self) {
        let caller = Location::caller();
        let id = Resource::RwLock(self.id);
        self.observer.on_released(&id, caller);
    }
}
