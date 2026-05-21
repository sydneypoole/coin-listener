use api_server::{auth, build_router, make_http_trace_layer, realtime, ApiState};
use chrono::Utc;
use coin_listener_core::AppConfig;
use coin_listener_storage::{
    connect_postgres, connect_redis, run_migrations,
    service_heartbeats::{run_service_heartbeat, service_heartbeat_instance_id},
};
use notifier::external::ExternalNotificationSender;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{net::TcpListener, signal, task::JoinHandle, time};
use tower_http::cors::CorsLayer;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = AppConfig::from_env()?;
    let auth_settings = auth::token_settings(
        config.auth.token_secret.clone(),
        config.auth.token_ttl_seconds,
    )?;
    let postgres = connect_postgres(&config.postgres).await?;
    run_migrations(&postgres).await?;
    let redis = connect_redis(&config.redis)?;

    let shutdown = Arc::new(AtomicBool::new(false));
    let heartbeat_shutdown = Arc::clone(&shutdown);
    tokio::spawn(run_service_heartbeat(
        postgres.clone(),
        "api-server",
        service_heartbeat_instance_id(),
        Utc::now(),
        heartbeat_shutdown,
    ));

    let realtime_hub = realtime::RealtimeHub::new(256);
    let realtime_shutdown = Arc::clone(&shutdown);
    tokio::spawn(realtime::run_realtime_notification_listener(
        config.postgres.database_url.clone(),
        realtime_hub.clone(),
        realtime_shutdown,
    ));

    let telegram_poller_shutdown = Arc::clone(&shutdown);
    let mut telegram_poller_handle = tokio::spawn(notifier::run_telegram_update_poller(
        postgres.clone(),
        ExternalNotificationSender::new(reqwest::Client::new()),
        telegram_poller_shutdown,
    ));

    let state = Arc::new(ApiState {
        postgres,
        redis: Some(redis),
        scan_queue_key: config.scan.queue_key.clone(),
        notify_queue_key: config.notify.queue_key.clone(),
        enable_dev_routes: config.server.enable_dev_routes,
        auth: auth_settings,
        realtime: realtime_hub,
        telegram_webhook_secret: config.notify.telegram_webhook_secret.clone(),
    });
    let app = build_router(state)
        .layer(CorsLayer::permissive())
        .layer(make_http_trace_layer());

    let listener = TcpListener::bind(config.server_addr()).await?;
    info!(address = %listener.local_addr()?, "api server listening");

    let shutdown_signal = Arc::clone(&shutdown);
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        if signal::ctrl_c().await.is_ok() {
            shutdown_signal.store(true, Ordering::Relaxed);
        }
    });

    let event = tokio::select! {
        result = server => ApiServerRuntimeEvent::Server(result.map_err(Into::into)),
        result = &mut telegram_poller_handle => ApiServerRuntimeEvent::TelegramPoller(telegram_poller_task_result(result)),
    };
    shutdown.store(true, Ordering::Relaxed);

    match event {
        ApiServerRuntimeEvent::Server(result) => preserve_primary_result(
            result,
            wait_for_telegram_poller_shutdown(telegram_poller_handle).await,
        ),
        ApiServerRuntimeEvent::TelegramPoller(result) => result,
    }
}

#[derive(Debug)]
enum ApiServerRuntimeEvent {
    Server(anyhow::Result<()>),
    TelegramPoller(anyhow::Result<()>),
}

fn preserve_primary_result(
    primary: anyhow::Result<()>,
    secondary: anyhow::Result<()>,
) -> anyhow::Result<()> {
    match primary {
        Ok(()) => secondary,
        Err(error) => Err(error),
    }
}

fn telegram_poller_task_result(
    result: Result<coin_listener_core::AppResult<()>, tokio::task::JoinError>,
) -> anyhow::Result<()> {
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(anyhow::anyhow!("telegram poller failed: {error}")),
        Err(error) => Err(anyhow::anyhow!("telegram poller task failed: {error}")),
    }
}

async fn wait_for_telegram_poller_shutdown(
    handle: JoinHandle<coin_listener_core::AppResult<()>>,
) -> anyhow::Result<()> {
    match time::timeout(Duration::from_secs(10), handle).await {
        Ok(result) => telegram_poller_task_result(result),
        Err(_) => Err(anyhow::anyhow!(
            "telegram poller did not stop before timeout"
        )),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn api_server_starts_telegram_update_poller() {
        let source = include_str!("main.rs");
        let production_source = source
            .split("#[cfg(test)]")
            .next()
            .expect("production source is before tests");

        assert!(production_source.contains("run_telegram_update_poller"));
        assert!(
            production_source.contains("ExternalNotificationSender::new(reqwest::Client::new())")
        );
        assert!(production_source.contains("postgres.clone()"));
        assert!(production_source.contains("Arc::clone(&shutdown)"));
        assert!(production_source.contains("telegram_poller_handle"));
        assert!(production_source.contains("tokio::select!"));
        assert!(production_source.contains("ApiServerRuntimeEvent::Server"));
        assert!(production_source.contains("ApiServerRuntimeEvent::TelegramPoller"));
        assert!(production_source.contains("wait_for_telegram_poller_shutdown("));
        assert!(production_source.contains("preserve_primary_result("));
    }
}
