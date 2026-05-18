mod routes;

use coin_listener_core::AppConfig;
use coin_listener_storage::{connect_postgres, connect_redis, run_migrations};
use routes::{build_router, ApiState};
use std::sync::Arc;
use tokio::net::TcpListener;
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
    let postgres = connect_postgres(&config.postgres).await?;
    run_migrations(&postgres).await?;
    let redis = connect_redis(&config.redis)?;

    let state = Arc::new(ApiState {
        postgres,
        redis: Some(redis),
        scan_queue_key: config.scan.queue_key.clone(),
        notify_queue_key: config.notify.queue_key.clone(),
        enable_dev_routes: config.server.enable_dev_routes,
    });
    let app = build_router(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let listener = TcpListener::bind(config.server_addr()).await?;
    info!(address = %listener.local_addr()?, "api server listening");

    axum::serve(listener, app).await?;
    Ok(())
}
