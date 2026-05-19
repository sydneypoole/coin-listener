# Coin Listener All-in-One Packaging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a single `all-in-one` backend process that serves the API, serves built frontend assets, and runs scheduler, worker, notifier, and service heartbeat loops while preserving the existing multi-process binaries.

**Architecture:** Add a new `backend/crates/all-in-one` Rust crate that reuses existing API, scheduler, worker, notifier, storage, and core crates. Extract only the reusable boundaries needed from `api-server` and `scheduler`; keep Postgres and Redis external; add Docker packaging that builds frontend assets and copies them next to the all-in-one binary.

**Tech Stack:** Rust 2021, Tokio, Axum, tower-http, SQLx/PostgreSQL, Redis, reqwest, Docker Compose, React/Vite/Semi UI.

---

## File structure

- Create `backend/crates/all-in-one/Cargo.toml`: package manifest for the new all-in-one binary/library crate.
- Create `backend/crates/all-in-one/src/lib.rs`: static frontend router helpers, service name constants, and task result helpers with unit tests.
- Create `backend/crates/all-in-one/src/main.rs`: monolithic runtime entrypoint that wires config, database, Redis, API, scheduler, worker, notifier, heartbeats, and shutdown.
- Create `docker/all-in-one.Dockerfile`: root-context Dockerfile that builds frontend assets and the `all-in-one` Rust binary.
- Modify `backend/Cargo.toml`: add `crates/all-in-one` to workspace members and enable `tower-http` `fs` feature.
- Create `backend/crates/api-server/src/lib.rs`: expose reusable API route/state types for the all-in-one crate.
- Modify `backend/crates/api-server/src/main.rs`: use the library route/state exports instead of a private `mod routes`.
- Modify `backend/crates/scheduler/src/lib.rs`: extract `run_scheduler` loop and shutdown helper while preserving `enqueue_due_addresses`.
- Modify `backend/crates/scheduler/src/main.rs`: use `run_scheduler` for the existing scheduler binary.
- Modify `docker-compose.yml`: add an `all-in-one` service under an `all-in-one` profile without deleting existing services.
- Modify `.env.example`: document `COIN_LISTENER_FRONTEND_DIST` for local all-in-one static serving.

---

### Task 1: Expose API server as a reusable library

**Files:**
- Create: `backend/crates/api-server/src/lib.rs`
- Modify: `backend/crates/api-server/src/main.rs`
- Test: `backend/crates/api-server/src/lib.rs`

- [ ] **Step 1: Write the failing library export test**

Create `backend/crates/api-server/src/lib.rs` with this test-only content:

```rust
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    #[test]
    fn exposes_api_router_for_reuse() {
        let _router_builder: fn(Arc<crate::routes::ApiState>) -> axum::Router =
            crate::routes::build_router;
    }
}
```

- [ ] **Step 2: Run the API test to verify it fails**

Run:

```bash
cargo test -p api-server --manifest-path backend/Cargo.toml exposes_api_router_for_reuse
```

Expected: FAIL with an unresolved `crate::routes` module or private route export error.

- [ ] **Step 3: Add the reusable API library export**

Replace `backend/crates/api-server/src/lib.rs` with:

```rust
pub mod routes;

pub use routes::{build_router, ApiState, HealthResponse};

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    #[test]
    fn exposes_api_router_for_reuse() {
        let _router_builder: fn(Arc<crate::routes::ApiState>) -> axum::Router =
            crate::routes::build_router;
    }
}
```

- [ ] **Step 4: Update the API binary to use the library module**

In `backend/crates/api-server/src/main.rs`, delete the first line:

```rust
mod routes;
```

Then change this import:

```rust
use routes::{build_router, ApiState};
```

to:

```rust
use api_server::{build_router, ApiState};
```

Leave the rest of `main.rs` unchanged.

- [ ] **Step 5: Run the API package tests**

Run:

```bash
cargo test -p api-server --manifest-path backend/Cargo.toml
```

Expected: PASS. Existing route tests and `exposes_api_router_for_reuse` pass.

- [ ] **Step 6: Commit Task 1**

Run:

```bash
git add backend/crates/api-server/src/lib.rs backend/crates/api-server/src/main.rs
git commit -m "Expose API server routes as library"
```

---

### Task 2: Extract reusable scheduler runtime loop

**Files:**
- Modify: `backend/crates/scheduler/src/lib.rs`
- Modify: `backend/crates/scheduler/src/main.rs`
- Test: `backend/crates/scheduler/src/lib.rs`

- [ ] **Step 1: Add failing scheduler runtime tests**

Append these tests inside the existing `#[cfg(test)] mod tests` in `backend/crates/scheduler/src/lib.rs`:

```rust
    #[test]
    fn scheduler_shutdown_flag_reads_atomic_state() {
        let shutdown = std::sync::atomic::AtomicBool::new(false);
        assert!(!super::scheduler_shutdown_requested(&shutdown));

        shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(super::scheduler_shutdown_requested(&shutdown));
    }

    #[test]
    fn scheduler_exports_reusable_runtime_loop() {
        let source = include_str!("lib.rs");

        assert!(source.contains("pub async fn run_scheduler("));
        assert!(source.contains("while !scheduler_shutdown_requested(&shutdown)"));
        assert!(source.contains("enqueue_due_addresses("));
    }
```

- [ ] **Step 2: Run scheduler tests to verify they fail**

Run:

```bash
cargo test -p scheduler --manifest-path backend/Cargo.toml scheduler_
```

Expected: FAIL because `scheduler_shutdown_requested` and `run_scheduler` do not exist yet.

- [ ] **Step 3: Add scheduler shutdown helper and runtime loop**

At the top of `backend/crates/scheduler/src/lib.rs`, add these imports after the existing imports:

```rust
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use tokio::time;
use tracing::{error, info};
```

Then add this code after `build_scan_task` and before `enqueue_due_addresses`:

```rust
pub fn scheduler_shutdown_requested(shutdown: &AtomicBool) -> bool {
    shutdown.load(Ordering::Relaxed)
}

async fn wait_for_scheduler_shutdown(shutdown: Arc<AtomicBool>) {
    while !scheduler_shutdown_requested(&shutdown) {
        time::sleep(Duration::from_millis(50)).await;
    }
}

pub async fn run_scheduler(
    pool: PgPool,
    mut redis: MultiplexedConnection,
    queue: ScanQueue,
    batch_size: i64,
    tick_seconds: u64,
    shutdown: Arc<AtomicBool>,
) -> AppResult<()> {
    let mut ticker = time::interval(Duration::from_secs(tick_seconds));

    while !scheduler_shutdown_requested(&shutdown) {
        tokio::select! {
            _ = ticker.tick() => {
                if scheduler_shutdown_requested(&shutdown) {
                    break;
                }

                match enqueue_due_addresses(&pool, &mut redis, &queue, batch_size, Utc::now()).await {
                    Ok(enqueued) => info!(service = "scheduler", enqueued, "scheduler tick completed"),
                    Err(error) => {
                        error!(service = "scheduler", error = %error, "scheduler tick failed");
                        return Err(error);
                    }
                }
            }
            _ = wait_for_scheduler_shutdown(Arc::clone(&shutdown)) => break,
        }
    }

    Ok(())
}
```

The top of `backend/crates/scheduler/src/lib.rs` should now include both existing imports and the new runtime imports. Keep `enqueue_due_addresses` unchanged.

- [ ] **Step 4: Refactor the scheduler binary to call `run_scheduler`**

Replace the full contents of `backend/crates/scheduler/src/main.rs` with:

```rust
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
```

- [ ] **Step 5: Run scheduler tests**

Run:

```bash
cargo test -p scheduler --manifest-path backend/Cargo.toml
```

Expected: PASS. The scheduler package still compiles and the new shutdown/runtime tests pass.

- [ ] **Step 6: Commit Task 2**

Run:

```bash
git add backend/crates/scheduler/src/lib.rs backend/crates/scheduler/src/main.rs
git commit -m "Extract reusable scheduler runtime loop"
```

---

### Task 3: Add all-in-one crate static routing helpers

**Files:**
- Modify: `backend/Cargo.toml`
- Create: `backend/crates/all-in-one/Cargo.toml`
- Create: `backend/crates/all-in-one/src/lib.rs`
- Test: `backend/crates/all-in-one/src/lib.rs`

- [ ] **Step 1: Add the crate manifest and failing helper tests**

In `backend/Cargo.toml`, add the new workspace member and `tower-http` `fs` feature:

```toml
[workspace]
members = [
    "crates/core",
    "crates/storage",
    "crates/api-server",
    "crates/scheduler",
    "crates/worker",
    "crates/notifier",
    "crates/chain-providers",
    "crates/all-in-one",
]
resolver = "2"
```

Change the existing `tower-http` dependency line to:

```toml
tower-http = { version = "0.5", features = ["cors", "trace", "fs"] }
```

Create `backend/crates/all-in-one/Cargo.toml`:

```toml
[package]
name = "all-in-one"
edition.workspace = true
version.workspace = true
license.workspace = true

[dependencies]
anyhow.workspace = true
api-server = { path = "../api-server" }
axum.workspace = true
chrono.workspace = true
coin-listener-core = { path = "../core" }
coin-listener-storage = { path = "../storage" }
notifier = { path = "../notifier" }
redis.workspace = true
reqwest.workspace = true
scheduler = { path = "../scheduler" }
sqlx.workspace = true
tokio.workspace = true
tower-http.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
worker = { path = "../worker" }
uuid.workspace = true

[dev-dependencies]
tower = "0.5"
```

Create `backend/crates/all-in-one/src/lib.rs` with failing tests that reference helpers not yet implemented:

```rust
#[cfg(test)]
mod tests {
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
        routing::get,
        Router,
    };
    use std::{fs, path::PathBuf};
    use tower::ServiceExt;

    fn test_dist(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "coin-listener-all-in-one-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("index.html"), "frontend-shell").unwrap();
        path
    }

    #[test]
    fn frontend_dist_path_uses_default_and_env_override() {
        assert_eq!(
            crate::frontend_dist_path(None),
            PathBuf::from(crate::DEFAULT_FRONTEND_DIST)
        );
        assert_eq!(
            crate::frontend_dist_path(Some("/opt/coin-listener/dist".to_string())),
            PathBuf::from("/opt/coin-listener/dist")
        );
    }

    #[tokio::test]
    async fn api_routes_take_precedence_over_static_fallback() {
        let dist = test_dist("api-priority");
        let api = Router::new().route("/health", get(|| async { "api-health" }));
        let app = crate::build_all_in_one_router(api, dist.clone());

        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

        assert_eq!(&body[..], b"api-health");
        fs::remove_dir_all(dist).unwrap();
    }

    #[tokio::test]
    async fn non_api_routes_fall_back_to_frontend_index() {
        let dist = test_dist("spa-fallback");
        let api = Router::new().route("/health", get(|| async { "api-health" }));
        let app = crate::build_all_in_one_router(api, dist.clone());

        let response = app
            .oneshot(Request::builder().uri("/events").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

        assert_eq!(&body[..], b"frontend-shell");
        fs::remove_dir_all(dist).unwrap();
    }

    #[test]
    fn service_task_result_names_failed_service() {
        let error = coin_listener_core::AppError::Config("bad config".to_string());
        let result = crate::service_task_result("worker", Ok(Err(error)));

        assert!(result.unwrap_err().to_string().contains("worker failed"));
    }
}
```

- [ ] **Step 2: Run all-in-one tests to verify they fail**

Run:

```bash
cargo test -p all-in-one --manifest-path backend/Cargo.toml
```

Expected: FAIL because `frontend_dist_path`, `DEFAULT_FRONTEND_DIST`, `build_all_in_one_router`, and `service_task_result` are not implemented.

- [ ] **Step 3: Implement static routing and task result helpers**

Replace `backend/crates/all-in-one/src/lib.rs` with:

```rust
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get_service,
    Router,
};
use std::{env, io, path::PathBuf};
use tokio::task::JoinError;
use tower_http::services::{ServeDir, ServeFile};

pub const FRONTEND_DIST_ENV: &str = "COIN_LISTENER_FRONTEND_DIST";
pub const DEFAULT_FRONTEND_DIST: &str = "./frontend/dist";
pub const ALL_IN_ONE_SERVICE_NAMES: [&str; 4] = ["api-server", "scheduler", "worker", "notifier"];

pub fn frontend_dist_path(value: Option<String>) -> PathBuf {
    value
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_FRONTEND_DIST))
}

pub fn frontend_dist_path_from_env() -> PathBuf {
    frontend_dist_path(env::var(FRONTEND_DIST_ENV).ok())
}

pub fn build_all_in_one_router(api_router: Router, frontend_dist: PathBuf) -> Router {
    let index = frontend_dist.join("index.html");
    let static_service = ServeDir::new(frontend_dist).not_found_service(ServeFile::new(index));

    api_router.fallback_service(get_service(static_service).handle_error(static_asset_error))
}

async fn static_asset_error(error: io::Error) -> Response {
    (
        StatusCode::NOT_FOUND,
        format!("frontend asset unavailable: {error}"),
    )
        .into_response()
}

pub fn service_task_result(
    service: &'static str,
    result: Result<coin_listener_core::AppResult<()>, JoinError>,
) -> anyhow::Result<()> {
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(anyhow::anyhow!("{service} failed: {error}")),
        Err(error) => Err(anyhow::anyhow!("{service} task failed: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
        routing::get,
        Router,
    };
    use std::{fs, path::PathBuf};
    use tower::ServiceExt;

    fn test_dist(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "coin-listener-all-in-one-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("index.html"), "frontend-shell").unwrap();
        path
    }

    #[test]
    fn frontend_dist_path_uses_default_and_env_override() {
        assert_eq!(
            crate::frontend_dist_path(None),
            PathBuf::from(crate::DEFAULT_FRONTEND_DIST)
        );
        assert_eq!(
            crate::frontend_dist_path(Some("/opt/coin-listener/dist".to_string())),
            PathBuf::from("/opt/coin-listener/dist")
        );
    }

    #[tokio::test]
    async fn api_routes_take_precedence_over_static_fallback() {
        let dist = test_dist("api-priority");
        let api = Router::new().route("/health", get(|| async { "api-health" }));
        let app = crate::build_all_in_one_router(api, dist.clone());

        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

        assert_eq!(&body[..], b"api-health");
        fs::remove_dir_all(dist).unwrap();
    }

    #[tokio::test]
    async fn non_api_routes_fall_back_to_frontend_index() {
        let dist = test_dist("spa-fallback");
        let api = Router::new().route("/health", get(|| async { "api-health" }));
        let app = crate::build_all_in_one_router(api, dist.clone());

        let response = app
            .oneshot(Request::builder().uri("/events").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

        assert_eq!(&body[..], b"frontend-shell");
        fs::remove_dir_all(dist).unwrap();
    }

    #[test]
    fn service_task_result_names_failed_service() {
        let error = coin_listener_core::AppError::Config("bad config".to_string());
        let result = crate::service_task_result("worker", Ok(Err(error)));

        assert!(result.unwrap_err().to_string().contains("worker failed"));
    }
}
```

- [ ] **Step 4: Run all-in-one crate tests**

Run:

```bash
cargo test -p all-in-one --manifest-path backend/Cargo.toml
```

Expected: PASS. Static fallback, API route priority, frontend path helper, and task error helper tests pass.

- [ ] **Step 5: Commit Task 3**

Run:

```bash
git add backend/Cargo.toml backend/Cargo.lock backend/crates/all-in-one/Cargo.toml backend/crates/all-in-one/src/lib.rs
git commit -m "Add all-in-one static runtime helpers"
```

---

### Task 4: Implement all-in-one runtime entrypoint

**Files:**
- Create: `backend/crates/all-in-one/src/main.rs`
- Test: `backend/crates/all-in-one/src/main.rs`

- [ ] **Step 1: Write failing runtime wiring tests**

Create `backend/crates/all-in-one/src/main.rs` with this test-only content:

```rust
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
    }
}
```

- [ ] **Step 2: Run the all-in-one runtime test to verify it fails**

Run:

```bash
cargo test -p all-in-one --manifest-path backend/Cargo.toml main_wires_all_runtime_services
```

Expected: FAIL because the runtime wiring strings are not present.

- [ ] **Step 3: Implement the all-in-one binary**

Replace `backend/crates/all-in-one/src/main.rs` with:

```rust
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
use notifier::{
    external::ExternalNotificationSender, run_notifier, NotificationOutboxDispatcherConfig,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{net::TcpListener, signal, time};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let config = AppConfig::from_env()?;
    let postgres = connect_postgres(&config.postgres).await?;
    run_migrations(&postgres).await?;
    let redis_client = connect_redis(&config.redis)?;
    let shutdown = Arc::new(AtomicBool::new(false));

    for service_name in ALL_IN_ONE_SERVICE_NAMES {
        tokio::spawn(run_service_heartbeat(
            postgres.clone(),
            service_name,
            service_heartbeat_instance_id(),
            Utc::now(),
            Arc::clone(&shutdown),
        ));
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
    let scheduler_queue = ScanQueue::new(config.scan.queue_key.clone(), config.scan.lock_ttl_seconds);
    let scheduler_handle = tokio::spawn(scheduler::run_scheduler(
        postgres.clone(),
        scheduler_redis,
        scheduler_queue,
        config.scan.scheduler_batch_size,
        config.scan.scheduler_tick_seconds,
        Arc::clone(&shutdown),
    ));

    let worker_redis = connect_scan_queue(&redis_client).await?;
    let worker_queue = ScanQueue::new(config.scan.queue_key.clone(), config.scan.lock_ttl_seconds);
    let worker_handle = tokio::spawn(worker::run_worker(
        postgres.clone(),
        worker_redis,
        worker_queue,
        Arc::clone(&shutdown),
    ));

    let dispatcher_config = NotificationOutboxDispatcherConfig::from_notify_config(&config.notify);
    let external_sender = ExternalNotificationSender::new(reqwest::Client::new());
    let notifier_handle = tokio::spawn(run_notifier(
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

    tokio::select! {
        result = signal::ctrl_c() => {
            result?;
            shutdown.store(true, Ordering::Relaxed);
            info!("all-in-one shutdown requested");
            Ok(())
        }
        result = server => {
            shutdown.store(true, Ordering::Relaxed);
            result?;
            Ok(())
        }
        result = scheduler_handle => {
            shutdown.store(true, Ordering::Relaxed);
            log_service_result("scheduler", service_task_result("scheduler", result))
        }
        result = worker_handle => {
            shutdown.store(true, Ordering::Relaxed);
            log_service_result("worker", service_task_result("worker", result))
        }
        result = notifier_handle => {
            shutdown.store(true, Ordering::Relaxed);
            log_service_result("notifier", service_task_result("notifier", result))
        }
    }
}

fn init_tracing() {
    let _ = tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .try_init();
}

async fn wait_for_shutdown(shutdown: Arc<AtomicBool>) {
    while !shutdown.load(Ordering::Relaxed) {
        time::sleep(Duration::from_millis(50)).await;
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
    }
}
```

- [ ] **Step 4: Run all-in-one tests and binary check**

Run:

```bash
cargo test -p all-in-one --manifest-path backend/Cargo.toml
cargo check --manifest-path backend/Cargo.toml --bin all-in-one
```

Expected: both commands PASS.

- [ ] **Step 5: Commit Task 4**

Run:

```bash
git add backend/crates/all-in-one/src/main.rs
git commit -m "Wire all-in-one runtime services"
```

---

### Task 5: Add Docker and Compose packaging

**Files:**
- Create: `docker/all-in-one.Dockerfile`
- Modify: `docker-compose.yml`
- Modify: `.env.example`
- Modify: `backend/crates/all-in-one/src/lib.rs`
- Test: `backend/crates/all-in-one/src/lib.rs`

- [ ] **Step 1: Add failing packaging structure tests**

Append this test module inside the existing `#[cfg(test)] mod tests` in `backend/crates/all-in-one/src/lib.rs`:

```rust
    fn repository_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap()
            .to_path_buf()
    }

    #[test]
    fn dockerfile_builds_binary_and_copies_frontend_dist() {
        let dockerfile = fs::read_to_string(repository_root().join("docker/all-in-one.Dockerfile"))
            .unwrap();

        assert!(dockerfile.contains("npm ci"));
        assert!(dockerfile.contains("npm run build"));
        assert!(dockerfile.contains("cargo build --release --workspace --bin all-in-one"));
        assert!(dockerfile.contains("/usr/local/bin/all-in-one"));
        assert!(dockerfile.contains("/usr/local/share/coin-listener/frontend"));
        assert!(dockerfile.contains("COIN_LISTENER_FRONTEND_DIST"));
    }

    #[test]
    fn compose_exposes_all_in_one_profile_without_removing_multi_process_services() {
        let compose = fs::read_to_string(repository_root().join("docker-compose.yml")).unwrap();

        assert!(compose.contains("all-in-one:"));
        assert!(compose.contains("docker/all-in-one.Dockerfile"));
        assert!(compose.contains("profiles:"));
        assert!(compose.contains("all-in-one"));
        assert!(compose.contains("api-server:"));
        assert!(compose.contains("scheduler:"));
        assert!(compose.contains("worker:"));
        assert!(compose.contains("notifier:"));
    }

    #[test]
    fn env_example_documents_frontend_dist_path() {
        let env_example = fs::read_to_string(repository_root().join(".env.example")).unwrap();

        assert!(env_example.contains("COIN_LISTENER_FRONTEND_DIST=./frontend/dist"));
    }
```

The `tests` module already imports `std::{fs, path::PathBuf};`, so no new test imports are needed.

- [ ] **Step 2: Run packaging tests to verify they fail**

Run:

```bash
cargo test -p all-in-one --manifest-path backend/Cargo.toml dockerfile_builds_binary_and_copies_frontend_dist compose_exposes_all_in_one_profile_without_removing_multi_process_services env_example_documents_frontend_dist_path
```

Expected: FAIL because `docker/all-in-one.Dockerfile`, Compose service, and env entry do not exist yet.

- [ ] **Step 3: Create all-in-one Dockerfile**

Create `docker/all-in-one.Dockerfile`:

```dockerfile
FROM node:24-bookworm AS frontend-builder
WORKDIR /app/frontend
COPY frontend/package*.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

FROM rust:1-bookworm AS backend-builder
WORKDIR /app/backend
COPY backend/ ./
RUN cargo build --release --workspace --bin all-in-one

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=backend-builder /app/backend/target/release/all-in-one /usr/local/bin/all-in-one
COPY --from=frontend-builder /app/frontend/dist /usr/local/share/coin-listener/frontend
ENV COIN_LISTENER_FRONTEND_DIST=/usr/local/share/coin-listener/frontend
ENTRYPOINT ["all-in-one"]
```

- [ ] **Step 4: Add the all-in-one Compose service**

In `docker-compose.yml`, insert this service block after the `notifier` service and before `volumes:`:

```yaml
  all-in-one:
    profiles:
      - all-in-one
    build:
      context: .
      dockerfile: docker/all-in-one.Dockerfile
    env_file: .env
    environment:
      COIN_LISTENER_FRONTEND_DIST: /usr/local/share/coin-listener/frontend
    ports:
      - "8080:8080"
    depends_on:
      postgres:
        condition: service_healthy
      redis:
        condition: service_healthy
```

Do not delete the existing `api-server`, `scheduler`, `worker`, or `notifier` services.

- [ ] **Step 5: Document local frontend dist config**

Append this line to `.env.example`:

```dotenv
COIN_LISTENER_FRONTEND_DIST=./frontend/dist
```

- [ ] **Step 6: Run packaging tests**

Run:

```bash
cargo test -p all-in-one --manifest-path backend/Cargo.toml dockerfile_builds_binary_and_copies_frontend_dist compose_exposes_all_in_one_profile_without_removing_multi_process_services env_example_documents_frontend_dist_path
```

Expected: PASS. Tests confirm Dockerfile, Compose service, and env example are present.

- [ ] **Step 7: Commit Task 5**

Run:

```bash
git add docker/all-in-one.Dockerfile docker-compose.yml .env.example backend/crates/all-in-one/src/lib.rs
git commit -m "Add all-in-one Docker packaging"
```

---

### Task 6: Run final all-in-one verification

**Files:**
- Verify: whole repository

- [ ] **Step 1: Run Rust formatting check**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: PASS with no diff output.

- [ ] **Step 2: Run backend workspace check**

Run:

```bash
cargo check --workspace --manifest-path backend/Cargo.toml
```

Expected: PASS. Existing third-party `sqlx-postgres` future-incompatibility warning is acceptable if it appears.

- [ ] **Step 3: Run backend workspace tests**

Run:

```bash
cargo test --workspace --manifest-path backend/Cargo.toml
```

Expected: PASS. All existing tests plus all-in-one tests pass.

- [ ] **Step 4: Build the all-in-one binary**

Run:

```bash
cargo build --release --manifest-path backend/Cargo.toml --bin all-in-one
```

Expected: PASS and produce `backend/target/release/all-in-one`.

- [ ] **Step 5: Build the frontend**

Run:

```bash
npm run build --prefix frontend
```

Expected: PASS. Existing Vite/lottie direct eval and chunk-size warnings are acceptable if they remain warnings.

- [ ] **Step 6: Optionally build the Docker image if Docker is available**

Run:

```bash
docker build -f docker/all-in-one.Dockerfile -t coin-listener-all-in-one:local .
```

Expected if Docker is available: PASS. If Docker is not installed or the daemon is unavailable, record the exact failure and rely on the source tests from Task 5 plus Rust/frontend builds.

- [ ] **Step 7: Confirm repository status**

Run:

```bash
git status --short
```

Expected: no unrelated files. Do not stage or commit unrelated preserved worktree files such as `frontend/package-lock.json` from older worktrees.

- [ ] **Step 8: Commit final formatting or lockfile changes only if needed**

If Steps 1-7 changed files, commit only those files with:

```bash
git add <exact changed files from this milestone>
git commit -m "Verify all-in-one packaging"
```

If `git status --short` is empty, do not create an empty commit.

---

## Plan self-review

- Spec coverage: Task 1 covers reusable API boundary; Task 2 covers scheduler loop extraction; Tasks 3-4 cover all-in-one crate, static frontend serving, service orchestration, heartbeats, shutdown, and runtime error handling; Task 5 covers Docker/Compose/env packaging; Task 6 covers backend, frontend, release binary, and optional Docker verification.
- Placeholder scan: no placeholder instructions are intentionally left in this plan.
- Type consistency: `frontend_dist_path`, `frontend_dist_path_from_env`, `build_all_in_one_router`, `service_task_result`, `ALL_IN_ONE_SERVICE_NAMES`, and `run_scheduler` are defined before later tasks use them.
