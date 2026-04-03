//! Pluggable instrumentation for synchronization primitives.
//!
//! The [`ResourceObserver`] trait defines hooks for the lifecycle of any
//! synchronization resource: wait → acquire → release.  Implementations can
//! log events ([`tracing::TracingObserver`]), or validate lock ordering
//! ([`lockdep::LockDepObserver`]).
//!
//! Every instrumented primitive in `crate::sync` is generic over
//! `O: ResourceObserver`, and the no-op `()` impl is used when
//! instrumentation is not needed.
pub mod lockdep;
/// Tracing-based observer that emits `tracing::trace!` events.
pub mod tracing;
use std::future::Future;
use std::panic::Location;

/// Resource identifier: either a human-readable name or a location where it was created.
///
/// This allows optional naming of resources while still tracking their origin for debugging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub enum Id {
    /// A human-provided resource name.
    Named(&'static str),
    /// A resource created at this location (used when no name is provided).
    Unnamed(&'static Location<'static>),
}

impl Id {
    /// Create a named resource ID.
    pub const fn named(name: &'static str) -> Self {
        Self::Named(name)
    }

    /// Create an unnamed resource ID from a location.
    pub const fn unnamed(loc: &'static Location<'static>) -> Self {
        Self::Unnamed(loc)
    }

    /// A filesystem- and URL-safe identifier for this resource.
    pub fn slug(&self) -> String {
        match self {
            Self::Named(name) => name.to_string(),
            Self::Unnamed(loc) => {
                let file = loc.file().replace(['/', '\\'], "_");
                format!("{}:{}:{}", file, loc.line(), loc.column())
            }
        }
    }
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Named(name) => write!(f, "{}", name),
            Self::Unnamed(loc) => write!(
                f,
                "<unnamed@{}:{}:{}>",
                loc.file(),
                loc.line(),
                loc.column()
            ),
        }
    }
}
/// What kind of synchronization resource is being operated on.
///
/// Passed to [`ResourceObserver`] methods so that implementations (e.g. the
/// tracing observer) can produce human-readable output without parsing the
/// resource name.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Resource {
    /// A mutex.
    Mutex(Id),
    /// A reader-writer lock.
    RwLock(Id),
    /// A counting semaphore.
    Semaphore(Id),
    /// A bounded MPSC channel.
    Channel(Id),
    /// A oneshot channel.
    Oneshot(Id),
    /// A barrier.
    Barrier(Id),
    /// A notify handle.
    Notify(Id),
    /// A watch channel.
    Watch(Id),
    /// A broadcast channel.
    Broadcast(Id),
}

impl Resource {
    /// A filesystem- and URL-safe identifier for this resource.
    pub fn slug(&self) -> String {
        match self {
            Self::Mutex(id) => id.slug(),
            Self::RwLock(id) => id.slug(),
            Self::Semaphore(id) => id.slug(),
            Self::Channel(id) => id.slug(),
            Self::Oneshot(id) => id.slug(),
            Self::Barrier(id) => id.slug(),
            Self::Notify(id) => id.slug(),
            Self::Watch(id) => id.slug(),
            Self::Broadcast(id) => id.slug(),
        }
    }
}

impl std::fmt::Display for Resource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mutex(id) => write!(f, "mutex {}", id),
            Self::RwLock(id) => write!(f, "rwlock {}", id),
            Self::Semaphore(id) => write!(f, "semaphore {}", id),
            Self::Channel(id) => write!(f, "channel {}", id),
            Self::Oneshot(id) => write!(f, "oneshot {}", id),
            Self::Barrier(id) => write!(f, "barrier {}", id),
            Self::Notify(id) => write!(f, "notify {}", id),
            Self::Watch(id) => write!(f, "watch {}", id),
            Self::Broadcast(id) => write!(f, "broadcast {}", id),
        }
    }
}
/// A pluggable instrumentation layer for synchronization primitives.
///
/// Implement this trait to hook into the resource lifecycle:
///
/// 1. **`on_waiting`** — called just before blocking on the resource.
/// 2. **`on_acquired`** — called right after the resource has been acquired.
/// 3. **`on_released`** — called when the resource is released (guard dropped,
///    or immediate release for fire-and-forget operations like channel send).
///
/// The trait is `Send + Sync` so it can live inside shared primitives.
pub trait ResourceObserver: Send + Sync {
    /// Wrap a future with observer-specific task instrumentation.
    fn instrument<F>(fut: F) -> impl Future<Output = F::Output>
    where
        F: Future;
    /// Called just before blocking on the resource.
    fn on_waiting(&self, resource: &Resource, caller: &'static Location<'static>);
    /// Called right after the resource has been acquired.
    fn on_acquired(&self, resource: &Resource, caller: &'static Location<'static>);
    /// Called when the resource is released.
    fn on_released(&self, resource: &Resource, caller: &'static Location<'static>);
}
/// No-op observer — all methods are inlined to nothing.
impl ResourceObserver for () {
    #[inline(always)]
    fn instrument<F>(fut: F) -> impl Future<Output = F::Output>
    where
        F: Future,
    {
        fut
    }
    #[inline(always)]
    fn on_waiting(&self, _resource: &Resource, _caller: &'static Location<'static>) {}
    #[inline(always)]
    fn on_acquired(&self, _resource: &Resource, _caller: &'static Location<'static>) {}
    #[inline(always)]
    fn on_released(&self, _resource: &Resource, _caller: &'static Location<'static>) {}
}
/// Compose two observers: both are called for every event.
impl<A: ResourceObserver, B: ResourceObserver> ResourceObserver for (A, B) {
    fn instrument<F>(fut: F) -> impl Future<Output = F::Output>
    where
        F: Future,
    {
        B::instrument(A::instrument(fut))
    }

    fn on_waiting(&self, resource: &Resource, caller: &'static Location<'static>) {
        self.0.on_waiting(resource, caller);
        self.1.on_waiting(resource, caller);
    }

    fn on_acquired(&self, resource: &Resource, caller: &'static Location<'static>) {
        self.0.on_acquired(resource, caller);
        self.1.on_acquired(resource, caller);
    }

    fn on_released(&self, resource: &Resource, caller: &'static Location<'static>) {
        self.0.on_released(resource, caller);
        self.1.on_released(resource, caller);
    }
}

/// The observer type used by `new()` / `channel()` constructors.
///
/// In debug builds: `(TracingObserver, LockDepObserver)`.
/// In release builds: `()` (zero-cost).
#[cfg(debug_assertions)]
pub(crate) type DefaultObserver = (tracing::TracingObserver, lockdep::LockDepObserver);
/// The observer type used by `new()` / `channel()` constructors.
///
/// In debug builds: `(TracingObserver, LockDepObserver)`.
/// In release builds: `()` (zero-cost).
#[cfg(not(debug_assertions))]
pub(crate) type DefaultObserver = ();

/// Build the observer used by `new()` constructors when no explicit observer
/// is provided by the caller.
#[inline]
pub(crate) fn observer() -> DefaultObserver {
    #[cfg(debug_assertions)]
    {
        (tracing::TracingObserver, lockdep::LockDepObserver)
    }
    #[cfg(not(debug_assertions))]
    {
        ()
    }
}

/// Instrument a future with the default observer.
///
/// This is the entry point used by [`crate::task::spawn`] and friends.
#[inline]
pub fn instrument<F>(fut: F) -> impl Future<Output = F::Output>
where
    F: Future,
{
    DefaultObserver::instrument(fut)
}
