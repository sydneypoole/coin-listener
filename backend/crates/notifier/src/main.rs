use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use coin_listener_core::AppConfig;
use coin_listener_storage::{
    connect_postgres, connect_redis,
    notify_queue::{connect_notify_queue, NotifyQueue},
    run_migrations,
};
use notifier::run_notifier;
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
    let redis = connect_notify_queue(&redis_client).await?;
    let notify_queue = NotifyQueue::new(config.notify.queue_key.clone());

    info!(
        service = "notifier",
        notify_queue_key = notify_queue.queue_key(),
        "service started"
    );

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_signal = Arc::clone(&shutdown);
    tokio::spawn(async move {
        if signal::ctrl_c().await.is_ok() {
            shutdown_signal.store(true, Ordering::Relaxed);
        }
    });

    run_notifier(postgres, redis, notify_queue, shutdown).await?;

    info!(service = "notifier", "service stopped");
    Ok(())
}
