use crate::observer::{DefaultObserver, Id, Resource, ResourceObserver, observer};
use std::future::Future;
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};
use std::panic::Location;
use std::sync::Arc;

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
    // ── Async lock ──────────────────────────────────────────────────────

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

    // ── Try lock ────────────────────────────────────────────────────────

    /// Try to acquire the mutex without waiting.
    #[track_caller]
    pub fn try_lock(&self) -> Result<MutexGuard<'_, T, O>, tokio::sync::TryLockError> {
        let caller = Location::caller();
        let id = Resource::Mutex(self.id);
        self.observer.on_waiting(&id, caller);
        match self.inner.try_lock() {
            Ok(guard) => {
                self.observer.on_acquired(&id, caller);
                Ok(MutexGuard {
                    guard,
                    id: self.id,
                    observer: &self.observer,
                    caller,
                })
            }
            Err(e) => Err(e),
        }
    }

    // ── Blocking lock ───────────────────────────────────────────────────

    /// Blocking lock — panics if called from an async context.
    #[track_caller]
    pub fn blocking_lock(&self) -> MutexGuard<'_, T, O> {
        let caller = Location::caller();
        let id = Resource::Mutex(self.id);
        self.observer.on_waiting(&id, caller);
        let guard = self.inner.blocking_lock();
        self.observer.on_acquired(&id, caller);
        MutexGuard {
            guard,
            id: self.id,
            observer: &self.observer,
            caller,
        }
    }

    // ── Passthrough helpers ─────────────────────────────────────────────

    /// Returns a mutable reference to the underlying data.
    ///
    /// No locking is needed because `&mut self` guarantees exclusivity.
    pub fn get_mut(&mut self) -> &mut T {
        self.inner.get_mut()
    }

    /// Consume the mutex, returning the underlying data.
    pub fn into_inner(self) -> T {
        self.inner.into_inner()
    }

    /// Get the resource id.
    pub fn id(&self) -> Id {
        self.id
    }
}

// ── Owned lock methods (require T: 'static for the lifetime transmute) ──────

impl<T: 'static, O: ResourceObserver> Mutex<T, O> {
    /// Acquire an owned lock on the mutex.
    #[track_caller]
    pub fn lock_owned(self: Arc<Self>) -> impl Future<Output = OwnedMutexGuard<T, O>> {
        let caller = Location::caller();
        async move {
            let id = self.id;
            self.observer.on_waiting(&Resource::Mutex(id), caller);
            let guard = self.inner.lock().await;
            self.observer.on_acquired(&Resource::Mutex(id), caller);
            // SAFETY: The Arc keeps the mutex alive for as long as the
            // OwnedMutexGuard exists. The guard is dropped before the Arc
            // refcount is decremented (ManuallyDrop in OwnedMutexGuard::drop).
            let guard = unsafe {
                std::mem::transmute::<
                    tokio::sync::MutexGuard<'_, T>,
                    tokio::sync::MutexGuard<'static, T>,
                >(guard)
            };
            OwnedMutexGuard {
                guard: ManuallyDrop::new(guard),
                mutex: self,
                id,
                caller,
            }
        }
    }

    /// Try to acquire an owned lock without waiting.
    #[track_caller]
    pub fn try_lock_owned(
        self: Arc<Self>,
    ) -> Result<OwnedMutexGuard<T, O>, tokio::sync::TryLockError> {
        let caller = Location::caller();
        let id = self.id;
        self.observer.on_waiting(&Resource::Mutex(id), caller);
        let acquired = {
            match self.inner.try_lock() {
                Ok(guard) => {
                    let guard = unsafe {
                        std::mem::transmute::<
                            tokio::sync::MutexGuard<'_, T>,
                            tokio::sync::MutexGuard<'static, T>,
                        >(guard)
                    };
                    Ok(guard)
                }
                Err(e) => Err(e),
            }
        };
        match acquired {
            Ok(guard) => {
                self.observer.on_acquired(&Resource::Mutex(id), caller);
                Ok(OwnedMutexGuard {
                    guard: ManuallyDrop::new(guard),
                    mutex: self,
                    id,
                    caller,
                })
            }
            Err(e) => Err(e),
        }
    }

    /// Blocking owned lock — panics if called from an async context.
    #[track_caller]
    pub fn blocking_lock_owned(self: Arc<Self>) -> OwnedMutexGuard<T, O> {
        let caller = Location::caller();
        let id = self.id;
        self.observer.on_waiting(&Resource::Mutex(id), caller);
        let guard = self.inner.blocking_lock();
        self.observer.on_acquired(&Resource::Mutex(id), caller);
        let guard = unsafe {
            std::mem::transmute::<
                tokio::sync::MutexGuard<'_, T>,
                tokio::sync::MutexGuard<'static, T>,
            >(guard)
        };
        OwnedMutexGuard {
            guard: ManuallyDrop::new(guard),
            mutex: self,
            id,
            caller,
        }
    }
}

// ─── Borrowed guard ─────────────────────────────────────────────────────────

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

// ─── Owned guard ────────────────────────────────────────────────────────────

/// An owned mutex guard that keeps the `Arc<Mutex<T, O>>` alive.
///
/// This is the gridlock equivalent of [`tokio::sync::OwnedMutexGuard`].
/// Internally it stores a lifetime-erased tokio `MutexGuard` alongside the
/// `Arc` that keeps the mutex alive — no inner `Arc` is needed.
pub struct OwnedMutexGuard<T: 'static, O: ResourceObserver = DefaultObserver> {
    // IMPORTANT: we use ManuallyDrop and drop the guard explicitly in our
    // Drop impl *before* the Arc refcount decrements.
    guard: ManuallyDrop<tokio::sync::MutexGuard<'static, T>>,
    mutex: Arc<Mutex<T, O>>,
    id: Id,
    caller: &'static Location<'static>,
}

// SAFETY: The guard protects `T` the same way `tokio::sync::OwnedMutexGuard`
// does — exclusive access is guaranteed while the guard is alive.
unsafe impl<T: 'static + Send, O: ResourceObserver> Send for OwnedMutexGuard<T, O> {}
unsafe impl<T: 'static + Send + Sync, O: ResourceObserver> Sync for OwnedMutexGuard<T, O> {}

impl<T: 'static, O: ResourceObserver> OwnedMutexGuard<T, O> {
    /// Returns a reference to the original `Arc<Mutex<T, O>>`.
    pub fn mutex(this: &Self) -> &Arc<Mutex<T, O>> {
        &this.mutex
    }
}

impl<T: 'static, O: ResourceObserver> Deref for OwnedMutexGuard<T, O> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.guard
    }
}
impl<T: 'static, O: ResourceObserver> DerefMut for OwnedMutexGuard<T, O> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}
impl<T: 'static, O: ResourceObserver> Drop for OwnedMutexGuard<T, O> {
    fn drop(&mut self) {
        // Drop the inner guard first (releases the lock), then the Arc
        // refcount decrements when `self.mutex` is dropped automatically.
        // SAFETY: We never use `self.guard` again after this.
        unsafe { ManuallyDrop::drop(&mut self.guard) };
        self.mutex
            .observer
            .on_released(&Resource::Mutex(self.id), self.caller);
    }
}
