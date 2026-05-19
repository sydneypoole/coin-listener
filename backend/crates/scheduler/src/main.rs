use chrono::Utc;
use coin_listener_core::AppConfig;
use coin_listener_storage::{
    connect_postgres, connect_redis, run_migrations,
    scan_queue::{connect_scan_queue, ScanQueue},
    service_heartbeats::{run_service_heartbeat, service_heartbeat_instance_id},
};
use scheduler::run_scheduler;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::signal;
use tracing::info;
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
    let redis = connect_scan_queue(&redis_client).await?;
    let queue = ScanQueue::new(config.scan.queue_key.clone(), config.scan.lock_ttl_seconds);
    let shutdown = Arc::new(AtomicBool::new(false));
    let heartbeat_shutdown = Arc::clone(&shutdown);
    tokio::spawn(run_service_heartbeat(
        postgres.clone(),
        "scheduler",
        service_heartbeat_instance_id(),
        Utc::now(),
        heartbeat_shutdown,
    ));

    info!(
        service = "scheduler",
        queue_key = queue.queue_key(),
        batch_size = config.scan.scheduler_batch_size,
        tick_seconds = config.scan.scheduler_tick_seconds,
        "service started"
    );

    let shutdown_signal = Arc::clone(&shutdown);
    tokio::spawn(async move {
        if signal::ctrl_c().await.is_ok() {
            shutdown_signal.store(true, Ordering::Relaxed);
        }
    });

    run_scheduler(
        postgres,
        redis,
        queue,
        config.scan.scheduler_batch_size,
        config.scan.scheduler_tick_seconds,
        shutdown,
    )
    .await?;

    info!(service = "scheduler", "service stopped");
    Ok(())
}
