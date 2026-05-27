use all_in_one::{
    build_all_in_one_router, frontend_dist_path_from_env, service_task_result,
    ALL_IN_ONE_SERVICE_NAMES,
};
use api_server::{auth, build_router, make_http_trace_layer, realtime, ApiState};
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
use tower_http::cors::CorsLayer;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let config = AppConfig::from_env()?;
    let auth_settings = auth::token_settings(
        config.auth.token_secret.clone(),
        config.auth.token_ttl_seconds,
    )?;
    let postgres = connect_postgres(&config.postgres).await?;
    run_migrations(&postgres).await?;
    let redis_client = connect_redis(&config.redis)?;
    let shutdown = Arc::new(AtomicBool::new(false));

    let realtime_hub = realtime::RealtimeHub::new(256);
    let mut realtime_handle = tokio::spawn(realtime::run_realtime_notification_listener(
        config.postgres.database_url.clone(),
        realtime_hub.clone(),
        Arc::clone(&shutdown),
    ));

    let api_state = Arc::new(ApiState {
        postgres: postgres.clone(),
        redis: Some(redis_client.clone()),
        scan_queue_key: config.scan.queue_key.clone(),
        notify_queue_key: config.notify.queue_key.clone(),
        scan_job_max_attempts: config.scan.job_max_attempts,
        enable_dev_routes: config.server.enable_dev_routes,
        auth: auth_settings,
        realtime: realtime_hub,
        telegram_webhook_secret: config.notify.telegram_webhook_secret.clone(),
    });
    let api_router = build_router(api_state)
        .layer(CorsLayer::permissive())
        .layer(make_http_trace_layer());
    let app = build_all_in_one_router(api_router, frontend_dist_path_from_env());

    let scheduler_redis = connect_scan_queue(&redis_client).await?;
    let scheduler_queue =
        ScanQueue::new(config.scan.queue_key.clone(), config.scan.lock_ttl_seconds);
    let worker_redis = connect_scan_queue(&redis_client).await?;
    let worker_queue = ScanQueue::new(config.scan.queue_key.clone(), config.scan.lock_ttl_seconds);
    let dispatcher_config = NotificationOutboxDispatcherConfig::from_notify_config(&config.notify);
    let external_sender = ExternalNotificationSender::new(reqwest::Client::new());
    let listener = TcpListener::bind(config.server_addr()).await?;
    info!(address = %listener.local_addr()?, "all-in-one server listening");

    let mut heartbeat_handles = Vec::new();
    let worker_id = service_heartbeat_instance_id();
    info!(service = "worker", instance_id = %worker_id, "worker instance id assigned");
    for service_name in ALL_IN_ONE_SERVICE_NAMES {
        let instance_id = if service_name == "worker" {
            worker_id.clone()
        } else {
            service_heartbeat_instance_id()
        };
        heartbeat_handles.push(tokio::spawn(run_service_heartbeat(
            postgres.clone(),
            service_name,
            instance_id,
            Utc::now(),
            Arc::clone(&shutdown),
        )));
    }

    let mut scheduler_handle = tokio::spawn(scheduler::run_scheduler(
        postgres.clone(),
        scheduler_redis,
        scheduler_queue,
        config.scan.scheduler_batch_size,
        config.scan.scheduler_tick_seconds,
        config.scan.job_max_attempts,
        Arc::clone(&shutdown),
    ));

    let mut worker_handle = tokio::spawn(worker::run_worker(
        postgres.clone(),
        worker_redis,
        worker_queue,
        worker_id,
        config.scan.job_idle_sleep_ms,
        Arc::clone(&shutdown),
    ));

    let mut notifier_handle = tokio::spawn(notifier::run_notifier(
        postgres.clone(),
        dispatcher_config,
        external_sender,
        Arc::clone(&shutdown),
    ));

    let mut telegram_poller_handle = tokio::spawn(notifier::run_telegram_update_poller(
        postgres.clone(),
        ExternalNotificationSender::new(reqwest::Client::new()),
        Arc::clone(&shutdown),
    ));

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
        result = &mut telegram_poller_handle => RuntimeEvent::TelegramPoller(log_service_result("telegram-poller", service_task_result("telegram-poller", result))),
        result = &mut realtime_handle => RuntimeEvent::Realtime(log_realtime_result("realtime", realtime_task_result("realtime", result))),
    };

    shutdown.store(true, Ordering::Relaxed);
    match event {
        RuntimeEvent::Shutdown => {
            let secondary = collect_shutdown_errors(vec![
                wait_for_server_shutdown(server_handle).await,
                wait_for_service_shutdown("scheduler", scheduler_handle).await,
                wait_for_service_shutdown("worker", worker_handle).await,
                wait_for_service_shutdown("notifier", notifier_handle).await,
                wait_for_realtime_shutdown("realtime", realtime_handle).await,
                wait_for_service_shutdown("telegram-poller", telegram_poller_handle).await,
            ]);
            wait_for_heartbeat_shutdown(heartbeat_handles).await;
            secondary
        }
        RuntimeEvent::Server(result) => {
            let secondary = collect_shutdown_errors(vec![
                wait_for_service_shutdown("scheduler", scheduler_handle).await,
                wait_for_service_shutdown("worker", worker_handle).await,
                wait_for_service_shutdown("notifier", notifier_handle).await,
                wait_for_realtime_shutdown("realtime", realtime_handle).await,
                wait_for_service_shutdown("telegram-poller", telegram_poller_handle).await,
            ]);
            wait_for_heartbeat_shutdown(heartbeat_handles).await;
            preserve_primary_result(result, secondary)
        }
        RuntimeEvent::Scheduler(result) => {
            let secondary = collect_shutdown_errors(vec![
                wait_for_server_shutdown(server_handle).await,
                wait_for_service_shutdown("worker", worker_handle).await,
                wait_for_service_shutdown("notifier", notifier_handle).await,
                wait_for_realtime_shutdown("realtime", realtime_handle).await,
                wait_for_service_shutdown("telegram-poller", telegram_poller_handle).await,
            ]);
            wait_for_heartbeat_shutdown(heartbeat_handles).await;
            preserve_primary_result(result, secondary)
        }
        RuntimeEvent::Worker(result) => {
            let secondary = collect_shutdown_errors(vec![
                wait_for_server_shutdown(server_handle).await,
                wait_for_service_shutdown("scheduler", scheduler_handle).await,
                wait_for_service_shutdown("notifier", notifier_handle).await,
                wait_for_realtime_shutdown("realtime", realtime_handle).await,
                wait_for_service_shutdown("telegram-poller", telegram_poller_handle).await,
            ]);
            wait_for_heartbeat_shutdown(heartbeat_handles).await;
            preserve_primary_result(result, secondary)
        }
        RuntimeEvent::Notifier(result) => {
            let secondary = collect_shutdown_errors(vec![
                wait_for_server_shutdown(server_handle).await,
                wait_for_service_shutdown("scheduler", scheduler_handle).await,
                wait_for_service_shutdown("worker", worker_handle).await,
                wait_for_realtime_shutdown("realtime", realtime_handle).await,
                wait_for_service_shutdown("telegram-poller", telegram_poller_handle).await,
            ]);
            wait_for_heartbeat_shutdown(heartbeat_handles).await;
            preserve_primary_result(result, secondary)
        }
        RuntimeEvent::Realtime(result) => {
            let secondary = collect_shutdown_errors(vec![
                wait_for_server_shutdown(server_handle).await,
                wait_for_service_shutdown("scheduler", scheduler_handle).await,
                wait_for_service_shutdown("worker", worker_handle).await,
                wait_for_service_shutdown("notifier", notifier_handle).await,
                wait_for_service_shutdown("telegram-poller", telegram_poller_handle).await,
            ]);
            wait_for_heartbeat_shutdown(heartbeat_handles).await;
            preserve_primary_result(result, secondary)
        }
        RuntimeEvent::TelegramPoller(result) => {
            let secondary = collect_shutdown_errors(vec![
                wait_for_server_shutdown(server_handle).await,
                wait_for_service_shutdown("scheduler", scheduler_handle).await,
                wait_for_service_shutdown("worker", worker_handle).await,
                wait_for_service_shutdown("notifier", notifier_handle).await,
                wait_for_realtime_shutdown("realtime", realtime_handle).await,
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
    TelegramPoller(anyhow::Result<()>),
    Realtime(anyhow::Result<()>),
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

fn realtime_task_result(
    service: &'static str,
    result: Result<(), tokio::task::JoinError>,
) -> anyhow::Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(error) => Err(anyhow::anyhow!("{service} task failed: {error}")),
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

async fn wait_for_realtime_shutdown(
    service: &'static str,
    handle: JoinHandle<()>,
) -> anyhow::Result<()> {
    log_realtime_result(service, realtime_task_result(service, handle.await))
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

fn log_realtime_result(service: &'static str, result: anyhow::Result<()>) -> anyhow::Result<()> {
    if let Err(error) = &result {
        error!(service, error = %error, "all-in-one service stopped with error");
    }
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn all_in_one_wires_realtime_listener() {
        let source = include_str!("main.rs");

        assert!(source.contains("run_realtime_notification_listener"));
        assert!(source.contains("realtime_handle"));
        assert!(source.contains("RealtimeHub::new"));
    }

    #[test]
    fn main_wires_all_runtime_services() {
        let source = include_str!("main.rs");

        assert!(source.contains("scheduler::run_scheduler("));
        assert!(source.contains("worker::run_worker("));
        assert!(source.contains("notifier::run_notifier("));
        assert!(source.contains("build_all_in_one_router("));
        assert!(source.contains("run_service_heartbeat("));
        assert!(
            source.find("TcpListener::bind(").unwrap()
                < source.find("run_service_heartbeat(").unwrap()
        );
        assert!(
            source.find("TcpListener::bind(").unwrap()
                < source.find("scheduler::run_scheduler(").unwrap()
        );
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

    #[test]
    fn all_in_one_starts_telegram_update_poller() {
        let source = include_str!("main.rs");
        let production_source = source
            .split("#[cfg(test)]")
            .next()
            .expect("production source is before tests");

        assert!(production_source.contains("run_telegram_update_poller"));
        assert!(production_source.contains("RuntimeEvent::TelegramPoller"));
        assert!(production_source.contains("wait_for_service_shutdown(\"telegram-poller\""));
    }
}
