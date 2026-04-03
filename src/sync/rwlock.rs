//! Instrumented wrapper for `tokio::sync::RwLock`.
//!
//! [`RwLock`] wraps a tokio `RwLock` and reports to a [`ResourceObserver`]
//! before/after every `.read()` and `.write()` call.
//!
//! For lockdep, both read/write paths currently use the same
//! `Resource::RwLock(id)` node.
use crate::observer::{DefaultObserver, Id, Resource, ResourceObserver, observer};
use std::future::Future;
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};
use std::panic::Location;
use std::sync::Arc;

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
    // ── Async read ──────────────────────────────────────────────────────

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
                caller,
            }
        }
    }

    // ── Async write ─────────────────────────────────────────────────────

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
                caller,
            }
        }
    }

    // ── Try read / write ────────────────────────────────────────────────

    /// Try to acquire a read lock without waiting.
    #[track_caller]
    pub fn try_read(&self) -> Result<RwLockReadGuard<'_, T, O>, tokio::sync::TryLockError> {
        let caller = Location::caller();
        let id = Resource::RwLock(self.id);
        self.observer.on_waiting(&id, caller);
        match self.inner.try_read() {
            Ok(guard) => {
                self.observer.on_acquired(&id, caller);
                Ok(RwLockReadGuard {
                    guard,
                    id: self.id,
                    observer: &self.observer,
                    caller,
                })
            }
            Err(e) => Err(e),
        }
    }

    /// Try to acquire a write lock without waiting.
    #[track_caller]
    pub fn try_write(&self) -> Result<RwLockWriteGuard<'_, T, O>, tokio::sync::TryLockError> {
        let caller = Location::caller();
        let id = Resource::RwLock(self.id);
        self.observer.on_waiting(&id, caller);
        match self.inner.try_write() {
            Ok(guard) => {
                self.observer.on_acquired(&id, caller);
                Ok(RwLockWriteGuard {
                    guard,
                    id: self.id,
                    observer: &self.observer,
                    caller,
                })
            }
            Err(e) => Err(e),
        }
    }

    // ── Blocking read / write ───────────────────────────────────────────

    /// Blocking read lock — panics if called from an async context.
    #[track_caller]
    pub fn blocking_read(&self) -> RwLockReadGuard<'_, T, O> {
        let caller = Location::caller();
        let id = Resource::RwLock(self.id);
        self.observer.on_waiting(&id, caller);
        let guard = self.inner.blocking_read();
        self.observer.on_acquired(&id, caller);
        RwLockReadGuard {
            guard,
            id: self.id,
            observer: &self.observer,
            caller,
        }
    }

    /// Blocking write lock — panics if called from an async context.
    #[track_caller]
    pub fn blocking_write(&self) -> RwLockWriteGuard<'_, T, O> {
        let caller = Location::caller();
        let id = Resource::RwLock(self.id);
        self.observer.on_waiting(&id, caller);
        let guard = self.inner.blocking_write();
        self.observer.on_acquired(&id, caller);
        RwLockWriteGuard {
            guard,
            id: self.id,
            observer: &self.observer,
            caller,
        }
    }

    // ── Passthrough helpers ─────────────────────────────────────────────

    /// Returns a mutable reference to the underlying data.
    pub fn get_mut(&mut self) -> &mut T {
        self.inner.get_mut()
    }

    /// Consume the lock, returning the underlying data.
    pub fn into_inner(self) -> T {
        self.inner.into_inner()
    }

    /// Get the resource id.
    pub fn id(&self) -> Id {
        self.id
    }
}

// ── Owned lock methods (require T: 'static for the lifetime transmute) ──────

impl<T: 'static, O: ResourceObserver> RwLock<T, O> {
    /// Acquire an owned read lock.
    #[track_caller]
    pub fn read_owned(self: Arc<Self>) -> impl Future<Output = OwnedRwLockReadGuard<T, O>> {
        let caller = Location::caller();
        async move {
            let id = self.id;
            self.observer.on_waiting(&Resource::RwLock(id), caller);
            let guard = self.inner.read().await;
            self.observer.on_acquired(&Resource::RwLock(id), caller);
            let guard = unsafe {
                std::mem::transmute::<
                    tokio::sync::RwLockReadGuard<'_, T>,
                    tokio::sync::RwLockReadGuard<'static, T>,
                >(guard)
            };
            OwnedRwLockReadGuard {
                guard: ManuallyDrop::new(guard),
                lock: self,
                id,
                caller,
            }
        }
    }

    /// Acquire an owned write lock.
    #[track_caller]
    pub fn write_owned(self: Arc<Self>) -> impl Future<Output = OwnedRwLockWriteGuard<T, O>> {
        let caller = Location::caller();
        async move {
            let id = self.id;
            self.observer.on_waiting(&Resource::RwLock(id), caller);
            let guard = self.inner.write().await;
            self.observer.on_acquired(&Resource::RwLock(id), caller);
            let guard = unsafe {
                std::mem::transmute::<
                    tokio::sync::RwLockWriteGuard<'_, T>,
                    tokio::sync::RwLockWriteGuard<'static, T>,
                >(guard)
            };
            OwnedRwLockWriteGuard {
                guard: ManuallyDrop::new(guard),
                lock: self,
                id,
                caller,
            }
        }
    }

    /// Try to acquire an owned read lock without waiting.
    #[track_caller]
    pub fn try_read_owned(
        self: Arc<Self>,
    ) -> Result<OwnedRwLockReadGuard<T, O>, tokio::sync::TryLockError> {
        let caller = Location::caller();
        let id = self.id;
        self.observer.on_waiting(&Resource::RwLock(id), caller);
        let acquired = {
            match self.inner.try_read() {
                Ok(guard) => {
                    let guard = unsafe {
                        std::mem::transmute::<
                            tokio::sync::RwLockReadGuard<'_, T>,
                            tokio::sync::RwLockReadGuard<'static, T>,
                        >(guard)
                    };
                    Ok(guard)
                }
                Err(e) => Err(e),
            }
        };
        match acquired {
            Ok(guard) => {
                self.observer.on_acquired(&Resource::RwLock(id), caller);
                Ok(OwnedRwLockReadGuard {
                    guard: ManuallyDrop::new(guard),
                    lock: self,
                    id,
                    caller,
                })
            }
            Err(e) => Err(e),
        }
    }

    /// Try to acquire an owned write lock without waiting.
    #[track_caller]
    pub fn try_write_owned(
        self: Arc<Self>,
    ) -> Result<OwnedRwLockWriteGuard<T, O>, tokio::sync::TryLockError> {
        let caller = Location::caller();
        let id = self.id;
        self.observer.on_waiting(&Resource::RwLock(id), caller);
        let acquired = {
            match self.inner.try_write() {
                Ok(guard) => {
                    let guard = unsafe {
                        std::mem::transmute::<
                            tokio::sync::RwLockWriteGuard<'_, T>,
                            tokio::sync::RwLockWriteGuard<'static, T>,
                        >(guard)
                    };
                    Ok(guard)
                }
                Err(e) => Err(e),
            }
        };
        match acquired {
            Ok(guard) => {
                self.observer.on_acquired(&Resource::RwLock(id), caller);
                Ok(OwnedRwLockWriteGuard {
                    guard: ManuallyDrop::new(guard),
                    lock: self,
                    id,
                    caller,
                })
            }
            Err(e) => Err(e),
        }
    }
}

// ─── Read guard ─────────────────────────────────────────────────────────────

/// RAII read guard that reports `on_released` when dropped.
pub struct RwLockReadGuard<'a, T, O: ResourceObserver = DefaultObserver> {
    guard: tokio::sync::RwLockReadGuard<'a, T>,
    id: Id,
    observer: &'a O,
    caller: &'static Location<'static>,
}
impl<T, O: ResourceObserver> Deref for RwLockReadGuard<'_, T, O> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.guard
    }
}
impl<T, O: ResourceObserver> Drop for RwLockReadGuard<'_, T, O> {
    fn drop(&mut self) {
        let id = Resource::RwLock(self.id);
        self.observer.on_released(&id, self.caller);
    }
}

// ─── Write guard ────────────────────────────────────────────────────────────

/// RAII write guard that reports `on_released` when dropped.
pub struct RwLockWriteGuard<'a, T, O: ResourceObserver = DefaultObserver> {
    guard: tokio::sync::RwLockWriteGuard<'a, T>,
    id: Id,
    observer: &'a O,
    caller: &'static Location<'static>,
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
    fn drop(&mut self) {
        let id = Resource::RwLock(self.id);
        self.observer.on_released(&id, self.caller);
    }
}

// ─── Owned read guard ───────────────────────────────────────────────────────

/// An owned read guard that keeps the `Arc<RwLock<T, O>>` alive.
pub struct OwnedRwLockReadGuard<T: 'static, O: ResourceObserver = DefaultObserver> {
    guard: ManuallyDrop<tokio::sync::RwLockReadGuard<'static, T>>,
    lock: Arc<RwLock<T, O>>,
    id: Id,
    caller: &'static Location<'static>,
}

unsafe impl<T: 'static + Send + Sync, O: ResourceObserver> Send for OwnedRwLockReadGuard<T, O> {}
unsafe impl<T: 'static + Send + Sync, O: ResourceObserver> Sync for OwnedRwLockReadGuard<T, O> {}

impl<T: 'static, O: ResourceObserver> OwnedRwLockReadGuard<T, O> {
    /// Returns a reference to the original `Arc<RwLock<T, O>>`.
    pub fn rwlock(this: &Self) -> &Arc<RwLock<T, O>> {
        &this.lock
    }
}

impl<T: 'static, O: ResourceObserver> Deref for OwnedRwLockReadGuard<T, O> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.guard
    }
}
impl<T: 'static, O: ResourceObserver> Drop for OwnedRwLockReadGuard<T, O> {
    fn drop(&mut self) {
        unsafe { ManuallyDrop::drop(&mut self.guard) };
        self.lock
            .observer
            .on_released(&Resource::RwLock(self.id), self.caller);
    }
}

// ─── Owned write guard ──────────────────────────────────────────────────────

/// An owned write guard that keeps the `Arc<RwLock<T, O>>` alive.
pub struct OwnedRwLockWriteGuard<T: 'static, O: ResourceObserver = DefaultObserver> {
    guard: ManuallyDrop<tokio::sync::RwLockWriteGuard<'static, T>>,
    lock: Arc<RwLock<T, O>>,
    id: Id,
    caller: &'static Location<'static>,
}

unsafe impl<T: 'static + Send + Sync, O: ResourceObserver> Send for OwnedRwLockWriteGuard<T, O> {}
unsafe impl<T: 'static + Send + Sync, O: ResourceObserver> Sync for OwnedRwLockWriteGuard<T, O> {}

impl<T: 'static, O: ResourceObserver> OwnedRwLockWriteGuard<T, O> {
    /// Returns a reference to the original `Arc<RwLock<T, O>>`.
    pub fn rwlock(this: &Self) -> &Arc<RwLock<T, O>> {
        &this.lock
    }
}

impl<T: 'static, O: ResourceObserver> Deref for OwnedRwLockWriteGuard<T, O> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.guard
    }
}
impl<T: 'static, O: ResourceObserver> DerefMut for OwnedRwLockWriteGuard<T, O> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}
impl<T: 'static, O: ResourceObserver> Drop for OwnedRwLockWriteGuard<T, O> {
    fn drop(&mut self) {
        unsafe { ManuallyDrop::drop(&mut self.guard) };
        self.lock
            .observer
            .on_released(&Resource::RwLock(self.id), self.caller);
    }
}
