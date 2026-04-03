use gridlock::observer::instrument;
/// Three-mutex ring: tests that lockdep detects cycles longer than 2.
///
///   task A: m1 → m2
///   task B: m2 → m3
///   task C: m3 → m1   ← closes the ring
///
/// Usage:
///   cargo run --example deadlock_lockdep_ring3
use gridlock::sync::Mutex;
use gridlock::task;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("trace"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    tracing::info!("Starting 3-mutex ring lockdep example");

    let m1 = Mutex::named("m1", 0);
    let m2 = Mutex::named("m2", 0);
    let m3 = Mutex::named("m3", 0);

    // Task A: hold m1, acquire m2  →  records ordering m1 → m2
    task::named(
        "task_A",
        instrument(async {
            let _g1 = m1.lock().await;
            let _g2 = m2.lock().await;
            tracing::info!("task_A: held m1 + m2");
        }),
    )
    .await;

    // Task B: hold m2, acquire m3  →  records ordering m2 → m3
    task::named(
        "task_B",
        instrument(async {
            let _g2 = m2.lock().await;
            let _g3 = m3.lock().await;
            tracing::info!("task_B: held m2 + m3");
        }),
    )
    .await;

    // Task C: hold m3, acquire m1  →  should detect cycle m1 → m2 → m3 → m1
    task::named(
        "task_C",
        instrument(async {
            let _g3 = m3.lock().await;
            tracing::info!("task_C: held m3, about to acquire m1 (should trigger lockdep!)");
            let _g1 = m1.lock().await;
            tracing::info!("task_C: held m3 + m1");
        }),
    )
    .await;

    tracing::info!(
        "done (if you see this, no actual deadlock occurred — but lockdep should have warned)"
    );

    Ok(())
}
