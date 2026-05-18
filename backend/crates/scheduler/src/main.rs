use chrono::Utc;
use coin_listener_core::AppConfig;
use coin_listener_storage::{
    connect_postgres, connect_redis, run_migrations,
    scan_queue::{connect_scan_queue, ScanQueue},
};
use scheduler::enqueue_due_addresses;
use std::time::Duration;
use tokio::{signal, time};
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = AppConfig::from_env()?;
    let postgres = connect_postgres(&config.postgres).await?;
    run_migrations(&postgres).await?;
    let redis_client = connect_redis(&config.redis)?;
    let mut redis = connect_scan_queue(&redis_client).await?;
    let queue = ScanQueue::new(config.scan.queue_key.clone(), config.scan.lock_ttl_seconds);
    let mut ticker = time::interval(Duration::from_secs(config.scan.scheduler_tick_seconds));

    info!(
        service = "scheduler",
        queue_key = queue.queue_key(),
        batch_size = config.scan.scheduler_batch_size,
        tick_seconds = config.scan.scheduler_tick_seconds,
        "service started"
    );

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                match enqueue_due_addresses(
                    &postgres,
                    &mut redis,
                    &queue,
                    config.scan.scheduler_batch_size,
                    Utc::now(),
                ).await {
                    Ok(enqueued) => info!(service = "scheduler", enqueued, "scheduler tick completed"),
                    Err(error) => {
                        error!(service = "scheduler", error = %error, "scheduler tick failed");
                        return Err(error.into());
                    }
                }
            }
            result = signal::ctrl_c() => {
                result?;
                break;
            }
        }
    }

    info!(service = "scheduler", "service stopped");
    Ok(())
}
