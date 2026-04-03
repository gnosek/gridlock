use gridlock::observer::instrument;
/// Cross-primitive deadlock: mutex + bounded channel.
///
/// Scenario:
///   - **producer** holds mutex `db`, then sends on bounded channel `work`
///   - **consumer** receives from `work`, then needs mutex `db` to process
///
/// If the channel buffer fills up, producer blocks on `send()` while holding
/// `db`.  Consumer can't drain because it needs `db`, which producer holds.
/// → Deadlock.
///
/// lockdep detects this *before* the buffer fills, because the ordering cycle
/// is:  db → work → db
///
/// Usage:
///   cargo run --example deadlock_channel
use gridlock::sync::Mutex;
use gridlock::sync::mpsc::named_channel;
use gridlock::task;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("trace"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    tracing::info!("Cross-primitive deadlock demo: mutex + bounded channel");

    let db = Mutex::named("db", Vec::<i32>::new());
    let (tx, mut rx) = named_channel::<i32>("work", 4);

    // Producer: hold db, then send  ->  records ordering  db -> work
    // (runs sequentially — no actual deadlock, but lockdep sees the ordering)
    task::named(
        "producer",
        instrument(async {
            let guard = db.lock().await;
            tracing::info!("producer: holding db, sending on work");
            tx.send(guard.len() as i32).await.unwrap();
            drop(guard);
        }),
    )
    .await;

    // Consumer: recv from channel, then lock db  ->  records ordering  work -> db
    // Combined with producer ordering, this closes the cycle:
    //   db -> work -> db
    task::named(
        "consumer",
        instrument(async {
            let val = rx.recv().await.unwrap();
            tracing::info!(val, "consumer: received, locking db");
            let mut guard = db.lock().await;
            guard.push(val);
            drop(guard);
        }),
    )
    .await;

    tracing::info!("done — lockdep should have warned about the ordering cycle above");

    Ok(())
}
