# gridlock

Drop-in instrumented replacements for `tokio::sync` primitives that automatically detect potential deadlocks at runtime.

## What it does

Every wrapper delegates to the real `tokio::sync` type but calls a pluggable [`ResourceObserver`](crate::observer::ResourceObserver) on each **wait → acquire → release** transition.  Two observers ship out of the box:

| Observer | Purpose |
|---|---|
| `TracingObserver` | Emits `tracing::trace!` spans for every lifecycle event |
| `LockDepObserver` | Builds a global lock-ordering graph and reports cycles — inspired by the Linux kernel's *lockdep* |

In **debug** builds both observers are active by default; in **release** builds the observer is `()` and all instrumentation compiles away.

## Requirements

This crate uses tokio APIs that are gated behind the `tokio_unstable` cfg flag
(e.g. `tokio::task::Builder` for named tasks). You **must** enable it or the
build will fail.

### Option A — `.cargo/config.toml` (recommended)

Create or edit `.cargo/config.toml` at the workspace root:

```toml
[build]
rustflags = ["--cfg", "tokio_unstable"]
```

This is the approach used by gridlock itself.

### Option B — environment variable

```sh
export RUSTFLAGS="--cfg tokio_unstable"
cargo build
```

Or inline:

```sh
RUSTFLAGS="--cfg tokio_unstable" cargo build
```

## Quick start

Replace `tokio::sync` imports with `gridlock::sync` and spawn tasks with `gridlock::task`:

```rust
use gridlock::sync::{Mutex, RwLock, mpsc};
use gridlock::task;

let mtx = Mutex::new(0u32);            // unnamed, tracked by source location
let rw  = RwLock::named("config", ()); // named, shows up in diagnostics

let (tx, mut rx) = mpsc::channel(8);

task::spawn(async move {
    tx.send(42).await.unwrap();
});

let val = rx.recv().await.unwrap();
```

If two tasks acquire locks in conflicting order, `LockDepObserver` logs an error **and** writes a Graphviz `.dot` file showing the cycle — even if the current run wouldn't actually deadlock.

## Naming resources

All primitives accept either `new()` (tracked by `#[track_caller]` source location) or `named("label", …)` for human-readable diagnostics.  Use `named()` for long-lived resources that appear in multiple tasks.

## Observer composition

Observers compose as a tuple — `(A, B)` calls both `A` and `B` for every event.  You can also implement `ResourceObserver` yourself for custom metrics, logging, or testing.

*Note*: there is currently no mechanism to plug in a custom `ResourceObserver` yet and the choices are effectively hardcoded to `TracingObserver` and `LockDepObserver` in debug builds, and `()` in release builds.  This is a known limitation.

## Covered primitives

`Mutex`, `RwLock`, `Semaphore`, `Barrier`, `Notify` and channel families `mpsc`, `oneshot`, `watch`, `broadcast` — all re-exported through `gridlock::sync` to mirror `tokio::sync`.

