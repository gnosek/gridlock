use crate::observer::{DefaultObserver, Id, Resource, ResourceObserver, observer};
use std::future::Future;
use std::panic::Location;
use std::sync::Arc;

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
    // ── Borrow-based acquire ────────────────────────────────────────────

    /// Acquire a single permit from the semaphore.
    #[track_caller]
    pub fn acquire(
        &self,
    ) -> impl Future<Output = Result<tokio::sync::SemaphorePermit<'_>, tokio::sync::AcquireError>>
           + '_ {
        let caller = Location::caller();
        async move {
            let id = Resource::Semaphore(self.id);
            self.observer.on_waiting(&id, caller);
            let result = self.inner.acquire().await;
            if result.is_ok() {
                self.observer.on_acquired(&id, caller);
            }
            result
        }
    }

    /// Acquire `n` permits from the semaphore.
    #[track_caller]
    pub fn acquire_many(
        &self,
        n: u32,
    ) -> impl Future<Output = Result<tokio::sync::SemaphorePermit<'_>, tokio::sync::AcquireError>>
           + '_ {
        let caller = Location::caller();
        async move {
            let id = Resource::Semaphore(self.id);
            self.observer.on_waiting(&id, caller);
            let result = self.inner.acquire_many(n).await;
            if result.is_ok() {
                self.observer.on_acquired(&id, caller);
            }
            result
        }
    }

    /// Try to acquire a single permit without waiting.
    #[track_caller]
    pub fn try_acquire(
        &self,
    ) -> Result<tokio::sync::SemaphorePermit<'_>, tokio::sync::TryAcquireError> {
        let caller = Location::caller();
        let id = Resource::Semaphore(self.id);
        self.observer.on_waiting(&id, caller);
        match self.inner.try_acquire() {
            Ok(permit) => {
                self.observer.on_acquired(&id, caller);
                Ok(permit)
            }
            Err(e) => Err(e),
        }
    }

    /// Try to acquire `n` permits without waiting.
    #[track_caller]
    pub fn try_acquire_many(
        &self,
        n: u32,
    ) -> Result<tokio::sync::SemaphorePermit<'_>, tokio::sync::TryAcquireError> {
        let caller = Location::caller();
        let id = Resource::Semaphore(self.id);
        self.observer.on_waiting(&id, caller);
        match self.inner.try_acquire_many(n) {
            Ok(permit) => {
                self.observer.on_acquired(&id, caller);
                Ok(permit)
            }
            Err(e) => Err(e),
        }
    }

    // ── Owned acquire ───────────────────────────────────────────────────
    //
    // `tokio::sync::Semaphore::acquire_owned` needs `Arc<tokio::sync::Semaphore>`,
    // but we only have `Arc<Semaphore<O>>` — the inner semaphore is a plain field,
    // not separately reference-counted.
    //
    // Strategy: acquire via the borrow-based API, then **forget** the raw tokio
    // permit so its Drop doesn't return the permits.  Our own
    // [`OwnedSemaphorePermit`] keeps the `Arc<Semaphore<O>>` alive and calls
    // `add_permits` on drop to restore them.

    /// Acquire a single owned permit.
    #[track_caller]
    pub fn acquire_owned(
        self: Arc<Self>,
    ) -> impl Future<Output = Result<OwnedSemaphorePermit<O>, tokio::sync::AcquireError>> {
        let caller = Location::caller();
        async move {
            let id = Resource::Semaphore(self.id);
            self.observer.on_waiting(&id, caller);
            let acquired = match self.inner.acquire().await {
                Ok(permit) => {
                    permit.forget();
                    Ok(())
                }
                Err(e) => Err(e),
            };
            match acquired {
                Ok(()) => {
                    self.observer.on_acquired(&id, caller);
                    Ok(OwnedSemaphorePermit {
                        semaphore: self,
                        permits: 1,
                    })
                }
                Err(e) => Err(e),
            }
        }
    }

    /// Acquire `n` owned permits.
    #[track_caller]
    pub fn acquire_many_owned(
        self: Arc<Self>,
        n: u32,
    ) -> impl Future<Output = Result<OwnedSemaphorePermit<O>, tokio::sync::AcquireError>> {
        let caller = Location::caller();
        async move {
            let id = Resource::Semaphore(self.id);
            self.observer.on_waiting(&id, caller);
            let acquired = match self.inner.acquire_many(n).await {
                Ok(permit) => {
                    permit.forget();
                    Ok(())
                }
                Err(e) => Err(e),
            };
            match acquired {
                Ok(()) => {
                    self.observer.on_acquired(&id, caller);
                    Ok(OwnedSemaphorePermit {
                        semaphore: self,
                        permits: n,
                    })
                }
                Err(e) => Err(e),
            }
        }
    }

    /// Try to acquire a single owned permit without waiting.
    #[track_caller]
    pub fn try_acquire_owned(
        self: Arc<Self>,
    ) -> Result<OwnedSemaphorePermit<O>, tokio::sync::TryAcquireError> {
        let caller = Location::caller();
        let id = Resource::Semaphore(self.id);
        self.observer.on_waiting(&id, caller);
        // Acquire and immediately forget the borrow-based permit so the
        // borrow on `self` is released before we move `self` into the
        // OwnedSemaphorePermit.
        let acquired = {
            match self.inner.try_acquire() {
                Ok(permit) => {
                    permit.forget();
                    Ok(())
                }
                Err(e) => Err(e),
            }
        };
        match acquired {
            Ok(()) => {
                self.observer.on_acquired(&id, caller);
                Ok(OwnedSemaphorePermit {
                    semaphore: self,
                    permits: 1,
                })
            }
            Err(e) => Err(e),
        }
    }

    /// Try to acquire `n` owned permits without waiting.
    #[track_caller]
    pub fn try_acquire_many_owned(
        self: Arc<Self>,
        n: u32,
    ) -> Result<OwnedSemaphorePermit<O>, tokio::sync::TryAcquireError> {
        let caller = Location::caller();
        let id = Resource::Semaphore(self.id);
        self.observer.on_waiting(&id, caller);
        let acquired = {
            match self.inner.try_acquire_many(n) {
                Ok(permit) => {
                    permit.forget();
                    Ok(())
                }
                Err(e) => Err(e),
            }
        };
        match acquired {
            Ok(()) => {
                self.observer.on_acquired(&id, caller);
                Ok(OwnedSemaphorePermit {
                    semaphore: self,
                    permits: n,
                })
            }
            Err(e) => Err(e),
        }
    }

    // ── Passthrough helpers ─────────────────────────────────────────────

    /// Returns the current number of available permits.
    pub fn available_permits(&self) -> usize {
        self.inner.available_permits()
    }

    /// Adds `n` new permits to the semaphore.
    pub fn add_permits(&self, n: usize) {
        self.inner.add_permits(n);
    }

    /// Permanently reduces the number of available permits by at most `n`,
    /// returning the number actually forgotten.
    pub fn forget_permits(&self, n: usize) -> usize {
        self.inner.forget_permits(n)
    }

    /// Closes the semaphore.
    pub fn close(&self) {
        self.inner.close();
    }

    /// Returns `true` if the semaphore has been closed.
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    /// Get the resource id.
    pub fn id(&self) -> Id {
        self.id
    }
}

// ── Owned permit ────────────────────────────────────────────────────────────

/// An owned permit from a [`Semaphore`].
///
/// This is the gridlock equivalent of [`tokio::sync::OwnedSemaphorePermit`].
/// It keeps the `Arc<Semaphore>` alive and returns the permits when dropped,
/// without requiring the inner tokio semaphore to be separately
/// reference-counted.
pub struct OwnedSemaphorePermit<O: ResourceObserver = DefaultObserver> {
    semaphore: Arc<Semaphore<O>>,
    permits: u32,
}

impl<O: ResourceObserver> OwnedSemaphorePermit<O> {
    /// Returns a reference to the semaphore that this permit belongs to.
    pub fn semaphore(&self) -> &Arc<Semaphore<O>> {
        &self.semaphore
    }

    /// Returns the number of permits held by this owned permit.
    pub fn num_permits(&self) -> usize {
        self.permits as usize
    }

    /// Forgets the permit **without** returning it to the semaphore.
    ///
    /// This permanently reduces the number of available permits.
    pub fn forget(self) {
        let mut this = std::mem::ManuallyDrop::new(self);
        this.permits = 0;
    }

    /// Merge another permit into this one.
    ///
    /// # Panics
    ///
    /// Panics if `other` belongs to a different semaphore.
    pub fn merge(&mut self, other: Self) {
        assert!(
            Arc::ptr_eq(&self.semaphore, &other.semaphore),
            "merging permits from different semaphores"
        );
        let other = std::mem::ManuallyDrop::new(other);
        self.permits += other.permits;
    }

    /// Split `n` permits off this permit, returning them as a new
    /// `OwnedSemaphorePermit`.
    ///
    /// Returns `None` if this permit holds fewer than `n` permits.
    pub fn split(&mut self, n: usize) -> Option<OwnedSemaphorePermit<O>> {
        let n = u32::try_from(n).ok()?;
        if n > self.permits {
            return None;
        }
        self.permits -= n;
        Some(OwnedSemaphorePermit {
            semaphore: Arc::clone(&self.semaphore),
            permits: n,
        })
    }
}

impl<O: ResourceObserver> Drop for OwnedSemaphorePermit<O> {
    fn drop(&mut self) {
        if self.permits > 0 {
            self.semaphore.inner.add_permits(self.permits as usize);
        }
    }
}
