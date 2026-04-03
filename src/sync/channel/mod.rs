//! Instrumented channel wrappers.
//!
//! Each sub-module wraps the corresponding `tokio::sync` channel family.
//! Non-blocking sends do immediate acquire + release; blocking receives
//! permanently mark the resource as held for lockdep ordering.
pub mod broadcast;
pub mod mpsc;
pub mod oneshot;
pub mod watch;
