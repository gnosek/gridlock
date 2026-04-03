use gridlock::observer::instrument;
use gridlock::sync::RwLock;
/// RwLock ordering violation demo.
///
/// Scenario:
///   - task A: write-locks `config`, then read-locks `cache`
///   - task B: read-locks `cache`, then write-locks `config`
///
/// lockdep detects the ordering cycle:
///   config → cache  vs  cache → config
///
/// Usage:
///   cargo run --example deadlock_rwlock
use gridlock::task;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("trace"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    tracing::info!("RwLock ordering violation demo");

    let config = RwLock::named("config", String::from("v1"));
    let cache = RwLock::named("cache", Vec::<String>::new());

    // Task A: write config, then read cache
    task::named(
        "task_A",
        instrument(async {
            let cfg = config.write().await;
            tracing::info!(cfg = %*cfg, "task_A: holding config, reading cache");
            let c = cache.read().await;
            tracing::info!(len = c.len(), "task_A: read cache");
            drop(c);
            drop(cfg);
        }),
    )
    .await;

    // Task B: read cache, then write config — opposite order!
    task::named(
        "task_B",
        instrument(async {
            let c = cache.read().await;
            tracing::info!(len = c.len(), "task_B: holding cache, writing config");
            let mut cfg = config.write().await;
            *cfg = String::from("v2");
            tracing::info!(cfg = %*cfg, "task_B: wrote config");
            drop(cfg);
            drop(c);
        }),
    )
    .await;

    tracing::info!("done");
    Ok(())
}
