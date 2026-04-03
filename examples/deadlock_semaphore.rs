use gridlock::observer::instrument;
/// Semaphore + Mutex ordering violation demo.
///
/// Scenario:
///   - task A: acquires semaphore `pool`, then locks mutex `state`
///   - task B: locks mutex `state`, then acquires semaphore `pool`
///
/// If `pool` has limited permits and all are held, task B blocks on `pool`
/// while holding `state`, and task A blocks on `state` while holding `pool`.
///
/// Usage:
///   cargo run --example deadlock_semaphore
use gridlock::sync::Mutex;
use gridlock::sync::Semaphore;
use gridlock::task;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("trace"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    tracing::info!("Semaphore + Mutex ordering violation demo");

    let pool = Semaphore::named("pool", 2);
    let state = Mutex::named("state", 0i32);

    // Task A: acquire pool permit, then lock state
    task::named(
        "task_A",
        instrument(async {
            let _permit = pool.acquire().await;
            tracing::info!("task_A: holding pool permit, locking state");
            let mut s = state.lock().await;
            *s += 1;
            tracing::info!(state = *s, "task_A: updated state");
            drop(s);
        }),
    )
    .await;

    // Task B: lock state, then acquire pool permit — opposite order!
    task::named(
        "task_B",
        instrument(async {
            let mut s = state.lock().await;
            tracing::info!(state = *s, "task_B: holding state, acquiring pool permit");
            let _permit = pool.acquire().await;
            *s += 1;
            tracing::info!(state = *s, "task_B: updated state with permit");
            drop(s);
        }),
    )
    .await;

    tracing::info!("done");
    Ok(())
}
