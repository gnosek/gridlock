use gridlock::observer::instrument;
/// Request/Response deadlock demo (channels + mutex).
///
/// A common real-world pattern:
///   - **worker**: receives work from a channel, locks mutex `state` to process
///   - **dispatcher**: locks mutex `state`, sends work on the channel
///
/// Ordering cycle:
///   worker:     work → state  (recv, then lock)
///   dispatcher: state → work  (lock, then send)
///   Cycle:  state → work → state
///
/// Usage:
///   cargo run --example deadlock_oneshot
use gridlock::sync::Mutex;
use gridlock::sync::mpsc::named_channel;
use gridlock::task;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("trace"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    tracing::info!("Request/Response deadlock demo");

    let state = Mutex::named("state", 0i32);
    let (work_tx, mut work_rx) = named_channel::<i32>("work", 1);

    // Worker: recv from work, then lock state -> records  work -> state
    // Pre-load the channel so recv() completes immediately.
    work_tx.try_send(42).unwrap();
    task::named(
        "worker",
        instrument(async {
            let val = work_rx.recv().await.unwrap();
            tracing::info!(val, "worker: received work, locking state");
            let mut s = state.lock().await;
            *s += val;
            tracing::info!(state = *s, "worker: updated state");
            drop(s);
        }),
    )
    .await;

    // Dispatcher: lock state, then send on work -> records  state -> work
    // Combined with worker's ordering, this closes the cycle:
    //   state -> work -> state  <- CYCLE!
    task::named(
        "dispatcher",
        instrument(async {
            let s = state.lock().await;
            tracing::info!(state = *s, "dispatcher: holding state, sending work");
            work_tx.send(*s).await.unwrap();
            tracing::info!("dispatcher: sent work while holding state");
            drop(s);
        }),
    )
    .await;

    tracing::info!("done — lockdep should have warned about the ordering cycle");
    Ok(())
}
