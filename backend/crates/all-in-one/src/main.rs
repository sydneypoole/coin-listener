use all_in_one::{
    build_all_in_one_router, frontend_dist_path_from_env, service_task_result,
    ALL_IN_ONE_SERVICE_NAMES,
};
use api_server::{build_router, ApiState};
use chrono::Utc;
use coin_listener_core::AppConfig;
use coin_listener_storage::{
    connect_postgres, connect_redis, run_migrations,
    scan_queue::{connect_scan_queue, ScanQueue},
    service_heartbeats::{run_service_heartbeat, service_heartbeat_instance_id},
};
use notifier::{external::ExternalNotificationSender, NotificationOutboxDispatcherConfig};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{net::TcpListener, signal, task::JoinHandle, time};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let config = AppConfig::from_env()?;
    let postgres = connect_postgres(&config.postgres).await?;
    run_migrations(&postgres).await?;
    let redis_client = connect_redis(&config.redis)?;
    let shutdown = Arc::new(AtomicBool::new(false));

    let mut heartbeat_handles = Vec::new();
    for service_name in ALL_IN_ONE_SERVICE_NAMES {
        heartbeat_handles.push(tokio::spawn(run_service_heartbeat(
            postgres.clone(),
            service_name,
            service_heartbeat_instance_id(),
            Utc::now(),
            Arc::clone(&shutdown),
        )));
    }

    let api_state = Arc::new(ApiState {
        postgres: postgres.clone(),
        redis: Some(redis_client.clone()),
        scan_queue_key: config.scan.queue_key.clone(),
        notify_queue_key: config.notify.queue_key.clone(),
        enable_dev_routes: config.server.enable_dev_routes,
    });
    let api_router = build_router(api_state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());
    let app = build_all_in_one_router(api_router, frontend_dist_path_from_env());

    let scheduler_redis = connect_scan_queue(&redis_client).await?;
    let scheduler_queue =
        ScanQueue::new(config.scan.queue_key.clone(), config.scan.lock_ttl_seconds);
    let mut scheduler_handle = tokio::spawn(scheduler::run_scheduler(
        postgres.clone(),
        scheduler_redis,
        scheduler_queue,
        config.scan.scheduler_batch_size,
        config.scan.scheduler_tick_seconds,
        Arc::clone(&shutdown),
    ));

    let worker_redis = connect_scan_queue(&redis_client).await?;
    let worker_queue = ScanQueue::new(config.scan.queue_key.clone(), config.scan.lock_ttl_seconds);
    let mut worker_handle = tokio::spawn(worker::run_worker(
        postgres.clone(),
        worker_redis,
        worker_queue,
        Arc::clone(&shutdown),
    ));

    let dispatcher_config = NotificationOutboxDispatcherConfig::from_notify_config(&config.notify);
    let external_sender = ExternalNotificationSender::new(reqwest::Client::new());
    let mut notifier_handle = tokio::spawn(notifier::run_notifier(
        postgres.clone(),
        dispatcher_config,
        external_sender,
        Arc::clone(&shutdown),
    ));

    let listener = TcpListener::bind(config.server_addr()).await?;
    info!(address = %listener.local_addr()?, "all-in-one server listening");

    let server_shutdown = Arc::clone(&shutdown);
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        wait_for_shutdown(server_shutdown).await;
    });
    let mut server_handle = tokio::spawn(async move {
        server.await?;
        Ok::<(), anyhow::Error>(())
    });

    let event = tokio::select! {
        result = shutdown_signal() => {
            result?;
            info!("all-in-one shutdown requested");
            RuntimeEvent::Shutdown
        }
        result = &mut server_handle => RuntimeEvent::Server(server_task_result(result)),
        result = &mut scheduler_handle => RuntimeEvent::Scheduler(log_service_result("scheduler", service_task_result("scheduler", result))),
        result = &mut worker_handle => RuntimeEvent::Worker(log_service_result("worker", service_task_result("worker", result))),
        result = &mut notifier_handle => RuntimeEvent::Notifier(log_service_result("notifier", service_task_result("notifier", result))),
    };

    shutdown.store(true, Ordering::Relaxed);
    match event {
        RuntimeEvent::Shutdown => {
            let secondary = collect_shutdown_errors(vec![
                wait_for_server_shutdown(server_handle).await,
                wait_for_service_shutdown("scheduler", scheduler_handle).await,
                wait_for_service_shutdown("worker", worker_handle).await,
                wait_for_service_shutdown("notifier", notifier_handle).await,
            ]);
            wait_for_heartbeat_shutdown(heartbeat_handles).await;
            secondary
        }
        RuntimeEvent::Server(result) => {
            let secondary = collect_shutdown_errors(vec![
                wait_for_service_shutdown("scheduler", scheduler_handle).await,
                wait_for_service_shutdown("worker", worker_handle).await,
                wait_for_service_shutdown("notifier", notifier_handle).await,
            ]);
            wait_for_heartbeat_shutdown(heartbeat_handles).await;
            preserve_primary_result(result, secondary)
        }
        RuntimeEvent::Scheduler(result) => {
            let secondary = collect_shutdown_errors(vec![
                wait_for_server_shutdown(server_handle).await,
                wait_for_service_shutdown("worker", worker_handle).await,
                wait_for_service_shutdown("notifier", notifier_handle).await,
            ]);
            wait_for_heartbeat_shutdown(heartbeat_handles).await;
            preserve_primary_result(result, secondary)
        }
        RuntimeEvent::Worker(result) => {
            let secondary = collect_shutdown_errors(vec![
                wait_for_server_shutdown(server_handle).await,
                wait_for_service_shutdown("scheduler", scheduler_handle).await,
                wait_for_service_shutdown("notifier", notifier_handle).await,
            ]);
            wait_for_heartbeat_shutdown(heartbeat_handles).await;
            preserve_primary_result(result, secondary)
        }
        RuntimeEvent::Notifier(result) => {
            let secondary = collect_shutdown_errors(vec![
                wait_for_server_shutdown(server_handle).await,
                wait_for_service_shutdown("scheduler", scheduler_handle).await,
                wait_for_service_shutdown("worker", worker_handle).await,
            ]);
            wait_for_heartbeat_shutdown(heartbeat_handles).await;
            preserve_primary_result(result, secondary)
        }
    }
}

enum RuntimeEvent {
    Shutdown,
    Server(anyhow::Result<()>),
    Scheduler(anyhow::Result<()>),
    Worker(anyhow::Result<()>),
    Notifier(anyhow::Result<()>),
}

fn init_tracing() {
    let _ = tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .try_init();
}

#[cfg(unix)]
async fn shutdown_signal() -> anyhow::Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    tokio::select! {
        result = signal::ctrl_c() => result.map_err(Into::into),
        _ = terminate.recv() => Ok(()),
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() -> anyhow::Result<()> {
    signal::ctrl_c().await.map_err(Into::into)
}

async fn wait_for_shutdown(shutdown: Arc<AtomicBool>) {
    while !shutdown.load(Ordering::Relaxed) {
        time::sleep(Duration::from_millis(50)).await;
    }
}

fn server_task_result(
    result: Result<anyhow::Result<()>, tokio::task::JoinError>,
) -> anyhow::Result<()> {
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(anyhow::anyhow!("server failed: {error}")),
        Err(error) => Err(anyhow::anyhow!("server task failed: {error}")),
    }
}

async fn wait_for_server_shutdown(handle: JoinHandle<anyhow::Result<()>>) -> anyhow::Result<()> {
    match handle.await {
        Ok(result) => result,
        Err(error) => Err(anyhow::anyhow!(
            "server task failed during shutdown: {error}"
        )),
    }
}

async fn wait_for_service_shutdown(
    service: &'static str,
    handle: JoinHandle<coin_listener_core::AppResult<()>>,
) -> anyhow::Result<()> {
    log_service_result(service, service_task_result(service, handle.await))
}

async fn wait_for_heartbeat_shutdown(handles: Vec<JoinHandle<()>>) {
    for handle in handles {
        if let Err(error) = handle.await {
            error!(error = %error, "service heartbeat task failed during shutdown");
        }
    }
}

fn collect_shutdown_errors(results: Vec<anyhow::Result<()>>) -> anyhow::Result<()> {
    let mut first_error = None;
    for result in results {
        if let Err(error) = result {
            warn!(error = %error, "all-in-one secondary shutdown task failed");
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
    }

    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn preserve_primary_result(
    primary: anyhow::Result<()>,
    secondary: anyhow::Result<()>,
) -> anyhow::Result<()> {
    match primary {
        Ok(()) => secondary,
        Err(error) => {
            if let Err(secondary_error) = secondary {
                warn!(error = %secondary_error, "all-in-one ignored secondary shutdown error");
            }
            Err(error)
        }
    }
}

fn log_service_result(service: &'static str, result: anyhow::Result<()>) -> anyhow::Result<()> {
    if let Err(error) = &result {
        error!(service, error = %error, "all-in-one service stopped with error");
    }
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn main_wires_all_runtime_services() {
        let source = include_str!("main.rs");

        assert!(source.contains("scheduler::run_scheduler("));
        assert!(source.contains("worker::run_worker("));
        assert!(source.contains("notifier::run_notifier("));
        assert!(source.contains("build_all_in_one_router("));
        assert!(source.contains("run_service_heartbeat("));
        assert!(source.contains("service_task_result(\"scheduler\""));
        assert!(source.contains("service_task_result(\"worker\""));
        assert!(source.contains("service_task_result(\"notifier\""));
        assert!(source.contains("wait_for_server_shutdown("));
        assert!(source.contains("wait_for_service_shutdown(\"scheduler\""));
        assert!(source.contains("wait_for_heartbeat_shutdown("));
        assert!(source.contains("shutdown_signal()"));
        assert!(source.contains("SignalKind::terminate()"));
        assert!(source.contains("preserve_primary_result("));
    }
}
