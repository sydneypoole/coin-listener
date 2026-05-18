use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use coin_listener_core::AppConfig;
use coin_listener_storage::{
    connect_postgres, connect_redis,
    notify_queue::NotifyQueue,
    run_migrations,
    scan_queue::{connect_scan_queue, ScanQueue},
};
use tokio::signal;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use worker::run_worker;

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
    let redis = connect_scan_queue(&redis_client).await?;
    let scan_queue = ScanQueue::new(config.scan.queue_key.clone(), config.scan.lock_ttl_seconds);
    let notify_queue = NotifyQueue::new(config.notify.queue_key.clone());

    info!(
        service = "worker",
        scan_queue_key = scan_queue.queue_key(),
        notify_queue_key = notify_queue.queue_key(),
        lock_ttl_seconds = config.scan.lock_ttl_seconds,
        "service started"
    );

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_signal = Arc::clone(&shutdown);
    tokio::spawn(async move {
        if signal::ctrl_c().await.is_ok() {
            shutdown_signal.store(true, Ordering::Relaxed);
        }
    });

    run_worker(postgres, redis, scan_queue, notify_queue, shutdown).await?;

    info!(service = "worker", "service stopped");
    Ok(())
}
