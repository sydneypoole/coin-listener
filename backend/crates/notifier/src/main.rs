use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use coin_listener_core::AppConfig;
use coin_listener_storage::{connect_postgres, run_migrations};
use notifier::{
    external::ExternalNotificationSender, run_notifier, NotificationOutboxDispatcherConfig,
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

    info!(
        service = "notifier",
        outbox_batch_size = config.notify.outbox_batch_size,
        outbox_max_attempts = config.notify.outbox_max_attempts,
        outbox_stale_lock_seconds = config.notify.outbox_stale_lock_seconds,
        outbox_idle_sleep_ms = config.notify.outbox_idle_sleep_ms,
        "service started"
    );

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_signal = Arc::clone(&shutdown);
    tokio::spawn(async move {
        if signal::ctrl_c().await.is_ok() {
            shutdown_signal.store(true, Ordering::Relaxed);
        }
    });

    let dispatcher_config = NotificationOutboxDispatcherConfig::from_notify_config(&config.notify);
    let external_sender = ExternalNotificationSender::new(reqwest::Client::new());
    run_notifier(postgres, dispatcher_config, external_sender, shutdown).await?;

    info!(service = "notifier", "service stopped");
    Ok(())
}
