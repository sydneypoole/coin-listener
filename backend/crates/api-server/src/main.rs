use api_server::{auth, build_router, realtime::RealtimeHub, ApiState};
use chrono::Utc;
use coin_listener_core::AppConfig;
use coin_listener_storage::{
    connect_postgres, connect_redis, run_migrations,
    service_heartbeats::{run_service_heartbeat, service_heartbeat_instance_id},
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::{net::TcpListener, signal};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
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

    let state = Arc::new(ApiState {
        postgres,
        redis: Some(redis),
        scan_queue_key: config.scan.queue_key.clone(),
        notify_queue_key: config.notify.queue_key.clone(),
        enable_dev_routes: config.server.enable_dev_routes,
        auth: auth_settings,
        realtime: RealtimeHub::new(256),
    });
    let app = build_router(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let listener = TcpListener::bind(config.server_addr()).await?;
    info!(address = %listener.local_addr()?, "api server listening");

    let shutdown_signal = Arc::clone(&shutdown);
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            if signal::ctrl_c().await.is_ok() {
                shutdown_signal.store(true, Ordering::Relaxed);
            }
        })
        .await?;
    Ok(())
}
