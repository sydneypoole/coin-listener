use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use chrono::Utc;
use coin_listener_core::AppConfig;
use coin_listener_storage::{
    connect_postgres, connect_redis, run_migrations,
    scan_queue::{connect_scan_queue, ScanQueue},
    service_heartbeats::{run_service_heartbeat, service_heartbeat_instance_id},
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

    info!(
        service = "worker",
        scan_queue_key = scan_queue.queue_key(),
        lock_ttl_seconds = config.scan.lock_ttl_seconds,
        "service started"
    );

    let shutdown = Arc::new(AtomicBool::new(false));
    let worker_id = service_heartbeat_instance_id();
    info!(service = "worker", instance_id = %worker_id, "worker instance id assigned");
    let heartbeat_shutdown = Arc::clone(&shutdown);
    tokio::spawn(run_service_heartbeat(
        postgres.clone(),
        "worker",
        worker_id.clone(),
        Utc::now(),
        heartbeat_shutdown,
    ));
    let shutdown_signal = Arc::clone(&shutdown);
    tokio::spawn(async move {
        if signal::ctrl_c().await.is_ok() {
            shutdown_signal.store(true, Ordering::Relaxed);
        }
    });

    run_worker(postgres, redis, scan_queue, worker_id, shutdown).await?;

    info!(service = "worker", "service stopped");
    Ok(())
}
