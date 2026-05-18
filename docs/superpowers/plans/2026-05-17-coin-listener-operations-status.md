# Coin Listener Operations Status Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a lightweight operations status center that exposes Redis queue backlog and PostgreSQL scan/event/notification/provider summaries through an API and a Semi Design frontend page.

**Architecture:** Add shared status DTOs in `core`, Redis queue depth helpers in existing queue wrappers, and a focused `storage::system_status` read model for PostgreSQL aggregates. The API server injects optional Redis access into `ApiState`, returns partial queue errors without failing the whole status endpoint, and the frontend polls `GET /api/system/status` every 10 seconds.

**Tech Stack:** Rust, Axum, SQLx, PostgreSQL, Redis, Chrono, Serde, Tokio, React, TypeScript, Vite, TanStack Query, Semi Design.

---

## Scope

Implements `docs/superpowers/specs/2026-05-17-coin-listener-operations-status-design.md`.

Included:

- Shared `SystemStatus` model family.
- `GET /api/system/status` route.
- Redis `LLEN` queue depth helpers for scan and notify queues.
- PostgreSQL status aggregates for scans, events, notifications, and providers.
- API server state wiring for Redis client and queue keys.
- Frontend typed client and `SystemStatusPage`.
- Navigation item for `系统状态`.
- Backend tests and frontend build verification.

Excluded:

- Prometheus/Grafana/OpenTelemetry.
- Persistent service heartbeat table.
- Real provider RPC ping.
- WebSocket status push.
- Alerting rules.

## Git note

The current project directory has been observed as not being a git repository. Each task ends with a verification checkpoint instead of a required commit. If a future worker runs this plan inside a git repository, commit after the checkpoint with the exact message shown in that task.

## File Structure

Modify:

```text
backend/crates/core/src/models.rs
backend/crates/storage/src/lib.rs
backend/crates/storage/src/scan_queue.rs
backend/crates/storage/src/notify_queue.rs
backend/crates/api-server/Cargo.toml
backend/crates/api-server/src/main.rs
backend/crates/api-server/src/routes.rs
frontend/src/api/types.ts
frontend/src/api/client.ts
frontend/src/App.tsx
```

Create:

```text
backend/crates/storage/src/system_status.rs
frontend/src/pages/SystemStatusPage.tsx
```

## Task 1: Add shared operations status models

**Files:**

- Modify: `backend/crates/core/src/models.rs`
- Verify: `cargo test -p coin-listener-core system_status --manifest-path backend/Cargo.toml`

- [ ] **Step 1: Write the failing serialization tests**

At the bottom of `backend/crates/core/src/models.rs`, inside the existing `#[cfg(test)] mod tests`, update the import that currently includes task models so it includes the status models:

```rust
use super::{
    EventStatus, NotificationStatus, ProviderChainStatus, ProviderStatus, ProviderStatusItem,
    QueueStatus, ScanAddressTask, ScanStatus, SystemStatus,
};
```

If the import currently includes `NotifyEventTask`, keep it too:

```rust
use super::{
    EventStatus, NotificationStatus, NotifyEventTask, ProviderChainStatus, ProviderStatus,
    ProviderStatusItem, QueueStatus, ScanAddressTask, ScanStatus, SystemStatus,
};
```

Add these tests in the same test module:

```rust
#[test]
fn system_status_round_trips_as_json() {
    let status = SystemStatus {
        generated_at: Utc.with_ymd_and_hms(2026, 5, 17, 10, 0, 0).unwrap(),
        queues: QueueStatus {
            scan_queue_key: "scan:address:queue".to_string(),
            scan_queue_depth: Some(3),
            notify_queue_key: "notify:event:queue".to_string(),
            notify_queue_depth: Some(1),
            queue_errors: vec![],
        },
        scans: ScanStatus {
            active_addresses: 12,
            due_addresses: 2,
            overdue_addresses: 1,
            last_scanned_at: Some(Utc.with_ymd_and_hms(2026, 5, 17, 9, 58, 0).unwrap()),
        },
        events: EventStatus {
            last_24h_total: 40,
            last_24h_transfers: 35,
            last_24h_non_transfers: 5,
        },
        notifications: NotificationStatus {
            last_24h_sent: 20,
            last_24h_skipped: 2,
            last_24h_failed: 1,
            unread_in_app: 4,
        },
        providers: ProviderStatus {
            active: 4,
            inactive: 1,
            by_chain: vec![ProviderChainStatus {
                chain_id: Uuid::from_u128(1),
                chain_name: "Ethereum".to_string(),
                active: 2,
                inactive: 0,
            }],
            items: vec![ProviderStatusItem {
                id: Uuid::from_u128(10),
                chain_id: Uuid::from_u128(1),
                chain_name: "Ethereum".to_string(),
                provider_type: "rpc".to_string(),
                name: "Ethereum RPC".to_string(),
                base_url: "https://example.invalid".to_string(),
                priority: 1,
                qps_limit: 10,
                timeout_ms: 5000,
                status: "active".to_string(),
            }],
        },
    };

    let payload = serde_json::to_string(&status).expect("serialize system status");
    let decoded: SystemStatus = serde_json::from_str(&payload).expect("deserialize system status");

    assert_eq!(decoded, status);
    assert!(payload.contains("\"scan_queue_depth\":3"));
    assert!(payload.contains("\"unread_in_app\":4"));
}

#[test]
fn queue_status_allows_missing_depths_with_errors() {
    let queues = QueueStatus {
        scan_queue_key: "scan:address:queue".to_string(),
        scan_queue_depth: None,
        notify_queue_key: "notify:event:queue".to_string(),
        notify_queue_depth: None,
        queue_errors: vec!["redis unavailable".to_string()],
    };

    let payload = serde_json::to_string(&queues).expect("serialize queue status");
    let decoded: QueueStatus = serde_json::from_str(&payload).expect("deserialize queue status");

    assert_eq!(decoded.scan_queue_depth, None);
    assert_eq!(decoded.notify_queue_depth, None);
    assert_eq!(decoded.queue_errors, vec!["redis unavailable"]);
    assert!(payload.contains("\"scan_queue_depth\":null"));
}
```

- [ ] **Step 2: Run the tests to verify RED**

Run:

```bash
cargo test -p coin-listener-core system_status --manifest-path backend/Cargo.toml
```

Expected: FAIL because `SystemStatus`, `QueueStatus`, `ScanStatus`, `EventStatus`, `NotificationStatus`, `ProviderStatus`, `ProviderChainStatus`, and `ProviderStatusItem` are not defined yet.

- [ ] **Step 3: Add the status models**

In `backend/crates/core/src/models.rs`, add these model definitions near the other response/query models:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemStatus {
    pub generated_at: DateTime<Utc>,
    pub queues: QueueStatus,
    pub scans: ScanStatus,
    pub events: EventStatus,
    pub notifications: NotificationStatus,
    pub providers: ProviderStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueStatus {
    pub scan_queue_key: String,
    pub scan_queue_depth: Option<i64>,
    pub notify_queue_key: String,
    pub notify_queue_depth: Option<i64>,
    pub queue_errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanStatus {
    pub active_addresses: i64,
    pub due_addresses: i64,
    pub overdue_addresses: i64,
    pub last_scanned_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventStatus {
    pub last_24h_total: i64,
    pub last_24h_transfers: i64,
    pub last_24h_non_transfers: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationStatus {
    pub last_24h_sent: i64,
    pub last_24h_skipped: i64,
    pub last_24h_failed: i64,
    pub unread_in_app: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderStatus {
    pub active: i64,
    pub inactive: i64,
    pub by_chain: Vec<ProviderChainStatus>,
    pub items: Vec<ProviderStatusItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderChainStatus {
    pub chain_id: Uuid,
    pub chain_name: String,
    pub active: i64,
    pub inactive: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderStatusItem {
    pub id: Uuid,
    pub chain_id: Uuid,
    pub chain_name: String,
    pub provider_type: String,
    pub name: String,
    pub base_url: String,
    pub priority: i32,
    pub qps_limit: i32,
    pub timeout_ms: i32,
    pub status: String,
}
```

- [ ] **Step 4: Run the tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-core system_status --manifest-path backend/Cargo.toml
```

Expected: PASS with the two new tests.

- [ ] **Step 5: Check package**

Run:

```bash
cargo check -p coin-listener-core --manifest-path backend/Cargo.toml
```

Expected: PASS.

## Task 2: Add Redis queue depth helpers

**Files:**

- Modify: `backend/crates/storage/src/scan_queue.rs`
- Modify: `backend/crates/storage/src/notify_queue.rs`
- Verify: `cargo test -p coin-listener-storage queue_depth --manifest-path backend/Cargo.toml`

- [ ] **Step 1: Write failing queue depth command tests**

In `backend/crates/storage/src/scan_queue.rs`, update the test import:

```rust
use super::{deserialize_scan_task, queue_depth_command, scan_lock_key, serialize_scan_task};
```

Add this test:

```rust
#[test]
fn queue_depth_command_uses_llen_and_queue_key() {
    assert_eq!(queue_depth_command("scan:address:queue"), ["LLEN", "scan:address:queue"]);
}
```

In `backend/crates/storage/src/notify_queue.rs`, update the test import:

```rust
use super::{deserialize_notify_task, queue_depth_command, serialize_notify_task};
```

Add this test:

```rust
#[test]
fn queue_depth_command_uses_llen_and_queue_key() {
    assert_eq!(queue_depth_command("notify:event:queue"), ["LLEN", "notify:event:queue"]);
}
```

- [ ] **Step 2: Run the tests to verify RED**

Run:

```bash
cargo test -p coin-listener-storage queue_depth --manifest-path backend/Cargo.toml
```

Expected: FAIL because `queue_depth_command` is not defined.

- [ ] **Step 3: Add scan queue depth helper**

In `backend/crates/storage/src/scan_queue.rs`, add this method inside `impl ScanQueue` after `queue_key()`:

```rust
pub async fn depth(&self, connection: &mut MultiplexedConnection) -> AppResult<i64> {
    let depth: i64 = redis::cmd("LLEN")
        .arg(&self.queue_key)
        .query_async(connection)
        .await
        .map_err(|error| AppError::Redis(error.to_string()))?;
    Ok(depth)
}
```

Add this helper near `scan_lock_key`:

```rust
pub fn queue_depth_command(queue_key: &str) -> [&str; 2] {
    ["LLEN", queue_key]
}
```

- [ ] **Step 4: Add notify queue depth helper**

In `backend/crates/storage/src/notify_queue.rs`, add this method inside `impl NotifyQueue` after `queue_key()`:

```rust
pub async fn depth(&self, connection: &mut MultiplexedConnection) -> AppResult<i64> {
    let depth: i64 = redis::cmd("LLEN")
        .arg(&self.queue_key)
        .query_async(connection)
        .await
        .map_err(|error| AppError::Redis(error.to_string()))?;
    Ok(depth)
}
```

Add this helper before `serialize_notify_task`:

```rust
pub fn queue_depth_command(queue_key: &str) -> [&str; 2] {
    ["LLEN", queue_key]
}
```

- [ ] **Step 5: Run the tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-storage queue_depth --manifest-path backend/Cargo.toml
```

Expected: PASS with both queue depth tests.

## Task 3: Add PostgreSQL system status read model

**Files:**

- Create: `backend/crates/storage/src/system_status.rs`
- Modify: `backend/crates/storage/src/lib.rs`
- Verify: `cargo test -p coin-listener-storage system_status --manifest-path backend/Cargo.toml`

- [ ] **Step 1: Write the new storage module with tests first**

Create `backend/crates/storage/src/system_status.rs` with this test-only starting content:

```rust
#[cfg(test)]
mod tests {
    use crate::system_status::{
        EVENT_STATUS_QUERY, NOTIFICATION_STATUS_QUERY, PROVIDER_CHAIN_STATUS_QUERY,
        PROVIDER_ITEMS_QUERY, SCAN_STATUS_QUERY,
    };

    #[test]
    fn scan_status_query_counts_active_due_and_overdue_addresses() {
        assert!(SCAN_STATUS_QUERY.contains("status = 'active'"));
        assert!(SCAN_STATUS_QUERY.contains("next_scan_at <= NOW()"));
        assert!(SCAN_STATUS_QUERY.contains("last_scanned_at"));
    }

    #[test]
    fn event_status_query_counts_last_24h_transfers() {
        assert!(EVENT_STATUS_QUERY.contains("created_at >= NOW() - INTERVAL '24 hours'"));
        assert!(EVENT_STATUS_QUERY.contains("is_transfer = TRUE"));
        assert!(EVENT_STATUS_QUERY.contains("is_transfer = FALSE"));
    }

    #[test]
    fn notification_status_query_counts_delivery_statuses_and_unread() {
        assert!(NOTIFICATION_STATUS_QUERY.contains("status = 'sent'"));
        assert!(NOTIFICATION_STATUS_QUERY.contains("status = 'skipped'"));
        assert!(NOTIFICATION_STATUS_QUERY.contains("status = 'failed'"));
        assert!(NOTIFICATION_STATUS_QUERY.contains("read_at IS NULL"));
    }

    #[test]
    fn provider_queries_include_chain_names() {
        assert!(PROVIDER_CHAIN_STATUS_QUERY.contains("JOIN chains"));
        assert!(PROVIDER_CHAIN_STATUS_QUERY.contains("chain_name"));
        assert!(PROVIDER_ITEMS_QUERY.contains("JOIN chains"));
        assert!(PROVIDER_ITEMS_QUERY.contains("chain_name"));
    }
}
```

- [ ] **Step 2: Export the module and run RED**

Add this line to `backend/crates/storage/src/lib.rs`:

```rust
pub mod system_status;
```

Run:

```bash
cargo test -p coin-listener-storage system_status --manifest-path backend/Cargo.toml
```

Expected: FAIL because the query constants and functions are not defined yet.

- [ ] **Step 3: Add query constants and row structs**

Replace `backend/crates/storage/src/system_status.rs` with this content above the tests:

```rust
use coin_listener_core::{
    models::{
        EventStatus, NotificationStatus, ProviderChainStatus, ProviderStatus, ProviderStatusItem,
        ScanStatus,
    },
    AppError, AppResult,
};
use sqlx::{FromRow, PgPool};

pub const SCAN_STATUS_QUERY: &str = r#"
    SELECT
        COUNT(*) FILTER (WHERE status = 'active') AS active_addresses,
        COUNT(*) FILTER (WHERE status = 'active' AND next_scan_at <= NOW()) AS due_addresses,
        COUNT(*) FILTER (
            WHERE status = 'active'
              AND next_scan_at <= NOW() - INTERVAL '5 minutes'
        ) AS overdue_addresses,
        MAX(last_scanned_at) AS last_scanned_at
    FROM watched_addresses
"#;

pub const EVENT_STATUS_QUERY: &str = r#"
    SELECT
        COUNT(*) AS last_24h_total,
        COUNT(*) FILTER (WHERE is_transfer = TRUE) AS last_24h_transfers,
        COUNT(*) FILTER (WHERE is_transfer = FALSE) AS last_24h_non_transfers
    FROM address_events
    WHERE created_at >= NOW() - INTERVAL '24 hours'
"#;

pub const NOTIFICATION_STATUS_QUERY: &str = r#"
    SELECT
        COUNT(*) FILTER (WHERE nd.status = 'sent') AS last_24h_sent,
        COUNT(*) FILTER (WHERE nd.status = 'skipped') AS last_24h_skipped,
        COUNT(*) FILTER (WHERE nd.status = 'failed') AS last_24h_failed,
        (
            SELECT COUNT(*)
            FROM in_app_notifications ian
            WHERE ian.read_at IS NULL
        ) AS unread_in_app
    FROM notification_deliveries nd
    WHERE nd.created_at >= NOW() - INTERVAL '24 hours'
"#;

pub const PROVIDER_CHAIN_STATUS_QUERY: &str = r#"
    SELECT
        c.id AS chain_id,
        c.name AS chain_name,
        COUNT(p.id) FILTER (WHERE p.status = 'active') AS active,
        COUNT(p.id) FILTER (WHERE p.status <> 'active') AS inactive
    FROM chains c
    JOIN providers p ON p.chain_id = c.id
    GROUP BY c.id, c.name
    ORDER BY c.name ASC
"#;

pub const PROVIDER_ITEMS_QUERY: &str = r#"
    SELECT
        p.id,
        p.chain_id,
        c.name AS chain_name,
        p.provider_type,
        p.name,
        p.base_url,
        p.priority,
        p.qps_limit,
        p.timeout_ms,
        p.status
    FROM providers p
    JOIN chains c ON c.id = p.chain_id
    ORDER BY c.name ASC, p.priority ASC, p.name ASC
"#;

#[derive(Debug, FromRow)]
struct ProviderTotalsRow {
    active: i64,
    inactive: i64,
}
```

- [ ] **Step 4: Add aggregate functions**

Add these functions below the row struct and above the tests:

```rust
pub async fn system_scan_status(pool: &PgPool) -> AppResult<ScanStatus> {
    sqlx::query_as::<_, ScanStatus>(SCAN_STATUS_QUERY)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn system_event_status(pool: &PgPool) -> AppResult<EventStatus> {
    sqlx::query_as::<_, EventStatus>(EVENT_STATUS_QUERY)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn system_notification_status(pool: &PgPool) -> AppResult<NotificationStatus> {
    sqlx::query_as::<_, NotificationStatus>(NOTIFICATION_STATUS_QUERY)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn system_provider_status(pool: &PgPool) -> AppResult<ProviderStatus> {
    let totals = sqlx::query_as::<_, ProviderTotalsRow>(
        r#"
        SELECT
            COUNT(*) FILTER (WHERE status = 'active') AS active,
            COUNT(*) FILTER (WHERE status <> 'active') AS inactive
        FROM providers
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?;

    let by_chain = sqlx::query_as::<_, ProviderChainStatus>(PROVIDER_CHAIN_STATUS_QUERY)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let items = sqlx::query_as::<_, ProviderStatusItem>(PROVIDER_ITEMS_QUERY)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    Ok(ProviderStatus {
        active: totals.active,
        inactive: totals.inactive,
        by_chain,
        items,
    })
}
```

- [ ] **Step 5: Run tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-storage system_status --manifest-path backend/Cargo.toml
```

Expected: PASS with the new SQL string tests.

- [ ] **Step 6: Check storage package**

Run:

```bash
cargo check -p coin-listener-storage --manifest-path backend/Cargo.toml
```

Expected: PASS.

## Task 4: Add system status API route and API state wiring

**Files:**

- Modify: `backend/crates/api-server/Cargo.toml`
- Modify: `backend/crates/api-server/src/routes.rs`
- Modify: `backend/crates/api-server/src/main.rs`
- Verify: `cargo test -p api-server system_status --manifest-path backend/Cargo.toml`

- [ ] **Step 1: Add API server dependencies needed by the status endpoint**

`backend/crates/api-server/src/routes.rs` will use `chrono::Utc` and `redis::Client`, so add these workspace dependencies to `backend/crates/api-server/Cargo.toml`:

```toml
chrono.workspace = true
redis.workspace = true
```

The dependency block should include these lines with the existing dependencies:

```toml
[dependencies]
anyhow.workspace = true
coin-listener-core = { path = "../core" }
coin-listener-storage = { path = "../storage" }
axum.workspace = true
chrono.workspace = true
redis.workspace = true
serde.workspace = true
serde_json.workspace = true
sqlx.workspace = true
tokio.workspace = true
tower-http.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
uuid.workspace = true
```

- [ ] **Step 2: Write failing route exposure test**

In `backend/crates/api-server/src/routes.rs`, update the `ApiState` struct to the intended shape before tests are changed:

```rust
#[derive(Clone)]
pub struct ApiState {
    pub postgres: PgPool,
    pub redis: Option<redis::Client>,
    pub scan_queue_key: String,
    pub notify_queue_key: String,
    pub enable_dev_routes: bool,
}
```

Then update every test `ApiState` initializer in the same file to include:

```rust
redis: None,
scan_queue_key: "scan:address:queue".to_string(),
notify_queue_key: "notify:event:queue".to_string(),
```

Add this test in the route tests module:

```rust
#[tokio::test]
async fn router_exposes_system_status_route() {
    let app = build_router(Arc::new(ApiState {
        postgres: PgPool::connect_lazy(
            "postgres://postgres:postgres@localhost/coin_listener_test",
        )
        .expect("valid postgres url"),
        redis: None,
        scan_queue_key: "scan:address:queue".to_string(),
        notify_queue_key: "notify:event:queue".to_string(),
        enable_dev_routes: false,
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/system/status")
                .body(Body::empty())
                .expect("valid request"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}
```

- [ ] **Step 3: Run route test to verify RED**

Run:

```bash
cargo test -p api-server system_status --manifest-path backend/Cargo.toml
```

Expected: FAIL with route returning `404 NOT_FOUND` or compile failure until imports/route are added.

- [ ] **Step 4: Add imports and route**

In `backend/crates/api-server/src/routes.rs`, update imports:

```rust
use chrono::Utc;
use coin_listener_core::{
    models::{
        CreateNotificationChannelRequest, CreateNotificationRuleRequest, CreateProviderRequest,
        CreateWatchedAddressRequest, EventQuery, InAppNotificationQuery, LoginRequest,
        LoginResponse, QueueStatus, SystemStatus, UserSummary,
    },
    AppError,
};
use coin_listener_storage::{
    notifications,
    notify_queue::{connect_notify_queue, NotifyQueue},
    repositories,
    scan_queue::{connect_scan_queue, ScanQueue},
    system_status,
};
```

Add the route in `build_router` after `/health` so the endpoint is not controlled by `enable_dev_routes`:

```rust
.route("/api/system/status", get(system_status_handler))
```

- [ ] **Step 5: Add queue status helper and handler**

In `backend/crates/api-server/src/routes.rs`, add these functions after `health`:

```rust
async fn system_status_handler(State(state): State<Arc<ApiState>>) -> Result<Response, ApiError> {
    let queues = queue_status(&state).await;
    let scans = system_status::system_scan_status(&state.postgres).await?;
    let events = system_status::system_event_status(&state.postgres).await?;
    let notifications = system_status::system_notification_status(&state.postgres).await?;
    let providers = system_status::system_provider_status(&state.postgres).await?;

    Ok(Json(SystemStatus {
        generated_at: Utc::now(),
        queues,
        scans,
        events,
        notifications,
        providers,
    })
    .into_response())
}

async fn queue_status(state: &ApiState) -> QueueStatus {
    let mut queue_errors = Vec::new();
    let mut scan_queue_depth = None;
    let mut notify_queue_depth = None;

    if let Some(redis_client) = &state.redis {
        match connect_scan_queue(redis_client).await {
            Ok(mut connection) => {
                let queue = ScanQueue::new(state.scan_queue_key.clone(), 1);
                match queue.depth(&mut connection).await {
                    Ok(depth) => scan_queue_depth = Some(depth),
                    Err(error) => queue_errors.push(format!("scan queue depth unavailable: {error}")),
                }
            }
            Err(error) => queue_errors.push(format!("scan queue redis unavailable: {error}")),
        }

        match connect_notify_queue(redis_client).await {
            Ok(mut connection) => {
                let queue = NotifyQueue::new(state.notify_queue_key.clone());
                match queue.depth(&mut connection).await {
                    Ok(depth) => notify_queue_depth = Some(depth),
                    Err(error) => queue_errors.push(format!("notify queue depth unavailable: {error}")),
                }
            }
            Err(error) => queue_errors.push(format!("notify queue redis unavailable: {error}")),
        }
    } else {
        queue_errors.push("redis unavailable".to_string());
    }

    QueueStatus {
        scan_queue_key: state.scan_queue_key.clone(),
        scan_queue_depth,
        notify_queue_key: state.notify_queue_key.clone(),
        notify_queue_depth,
        queue_errors,
    }
}
```

- [ ] **Step 6: Update API server main**

In `backend/crates/api-server/src/main.rs`, update imports:

```rust
use coin_listener_storage::{connect_postgres, connect_redis, run_migrations};
```

After migrations, create Redis client:

```rust
let redis = connect_redis(&config.redis)?;
```

Update `ApiState` initialization:

```rust
let state = Arc::new(ApiState {
    postgres,
    redis: Some(redis),
    scan_queue_key: config.scan.queue_key.clone(),
    notify_queue_key: config.notify.queue_key.clone(),
    enable_dev_routes: config.server.enable_dev_routes,
});
```

- [ ] **Step 7: Run API tests to verify GREEN**

Run:

```bash
cargo test -p api-server system_status --manifest-path backend/Cargo.toml
```

Expected: PASS with `router_exposes_system_status_route`.

- [ ] **Step 8: Check API package**

Run:

```bash
cargo check -p api-server --manifest-path backend/Cargo.toml
```

Expected: PASS.

## Task 5: Add frontend system status client and page

**Files:**

- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/api/client.ts`
- Create: `frontend/src/pages/SystemStatusPage.tsx`
- Modify: `frontend/src/App.tsx`
- Verify: `npm run build --prefix frontend`

- [ ] **Step 1: Add frontend types**

Append these types to `frontend/src/api/types.ts`:

```typescript
export type QueueStatus = {
  scan_queue_key: string;
  scan_queue_depth?: number | null;
  notify_queue_key: string;
  notify_queue_depth?: number | null;
  queue_errors: string[];
};

export type ScanStatus = {
  active_addresses: number;
  due_addresses: number;
  overdue_addresses: number;
  last_scanned_at?: string | null;
};

export type EventStatus = {
  last_24h_total: number;
  last_24h_transfers: number;
  last_24h_non_transfers: number;
};

export type NotificationStatus = {
  last_24h_sent: number;
  last_24h_skipped: number;
  last_24h_failed: number;
  unread_in_app: number;
};

export type ProviderChainStatus = {
  chain_id: string;
  chain_name: string;
  active: number;
  inactive: number;
};

export type ProviderStatusItem = {
  id: string;
  chain_id: string;
  chain_name: string;
  provider_type: string;
  name: string;
  base_url: string;
  priority: number;
  qps_limit: number;
  timeout_ms: number;
  status: string;
};

export type ProviderStatus = {
  active: number;
  inactive: number;
  by_chain: ProviderChainStatus[];
  items: ProviderStatusItem[];
};

export type SystemStatus = {
  generated_at: string;
  queues: QueueStatus;
  scans: ScanStatus;
  events: EventStatus;
  notifications: NotificationStatus;
  providers: ProviderStatus;
};
```

- [ ] **Step 2: Add frontend API client**

Update the type import in `frontend/src/api/client.ts` to include `SystemStatus`:

```typescript
  SystemStatus,
```

Add this function at the end of `frontend/src/api/client.ts`:

```typescript
export function getSystemStatus(): Promise<SystemStatus> {
  return request<SystemStatus>('/api/system/status');
}
```

- [ ] **Step 3: Create SystemStatusPage**

Create `frontend/src/pages/SystemStatusPage.tsx`:

```typescript
import { useQuery } from '@tanstack/react-query';
import { Banner, Card, Col, Row, Space, Table, Tag, Typography } from '@douyinfe/semi-ui';
import { getSystemStatus } from '../api/client';
import type { ProviderChainStatus, ProviderStatusItem } from '../api/types';

const { Text, Title } = Typography;

function formatDepth(depth?: number | null) {
  return depth === null || depth === undefined ? '-' : String(depth);
}

function formatTime(value?: string | null) {
  return value ? new Date(value).toLocaleString() : '-';
}

function statusColor(status: string): 'green' | 'grey' {
  return status === 'active' ? 'green' : 'grey';
}

export function SystemStatusPage() {
  const statusQuery = useQuery({
    queryKey: ['system-status'],
    queryFn: getSystemStatus,
    refetchInterval: 10_000,
  });

  const status = statusQuery.data;

  return (
    <Space vertical align="start" spacing={16} className="content-stack">
      {statusQuery.isError ? (
        <Banner
          type="danger"
          title="系统状态加载失败"
          description={statusQuery.error instanceof Error ? statusQuery.error.message : '请求失败'}
        />
      ) : null}

      {status?.queues.queue_errors.length ? (
        <Banner
          type="warning"
          title="队列状态部分不可用"
          description={status.queues.queue_errors.join('；')}
        />
      ) : null}

      <Card title="运维状态总览" loading={statusQuery.isLoading}>
        <Row gutter={[16, 16]}>
          <Col span={8}><Metric title="Scan Queue" value={formatDepth(status?.queues.scan_queue_depth)} hint={status?.queues.scan_queue_key ?? '-'} /></Col>
          <Col span={8}><Metric title="Notify Queue" value={formatDepth(status?.queues.notify_queue_depth)} hint={status?.queues.notify_queue_key ?? '-'} /></Col>
          <Col span={8}><Metric title="Active 地址" value={status?.scans.active_addresses ?? 0} hint="status = active" /></Col>
          <Col span={8}><Metric title="Due 地址" value={status?.scans.due_addresses ?? 0} hint="next_scan_at <= now" /></Col>
          <Col span={8}><Metric title="24h 事件" value={status?.events.last_24h_total ?? 0} hint={`transfer ${status?.events.last_24h_transfers ?? 0}`} /></Col>
          <Col span={8}><Metric title="24h 通知失败" value={status?.notifications.last_24h_failed ?? 0} hint={`unread ${status?.notifications.unread_in_app ?? 0}`} /></Col>
        </Row>
      </Card>

      <Card title="扫描与通知摘要" loading={statusQuery.isLoading}>
        <Space vertical align="start">
          <Text>最近扫描时间：{formatTime(status?.scans.last_scanned_at)}</Text>
          <Text>过期未扫描地址：{status?.scans.overdue_addresses ?? 0}</Text>
          <Text>24h 非转账事件：{status?.events.last_24h_non_transfers ?? 0}</Text>
          <Text>24h 通知：sent {status?.notifications.last_24h_sent ?? 0} / skipped {status?.notifications.last_24h_skipped ?? 0} / failed {status?.notifications.last_24h_failed ?? 0}</Text>
        </Space>
      </Card>

      <Card title="Provider 按链状态" loading={statusQuery.isLoading}>
        <Table<ProviderChainStatus>
          dataSource={status?.providers.by_chain ?? []}
          rowKey="chain_id"
          pagination={false}
          columns={[
            { title: '链', dataIndex: 'chain_name' },
            { title: 'Active', dataIndex: 'active' },
            { title: 'Inactive', dataIndex: 'inactive' },
          ]}
        />
      </Card>

      <Card title="Provider 明细" loading={statusQuery.isLoading}>
        <Table<ProviderStatusItem>
          dataSource={status?.providers.items ?? []}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          columns={[
            { title: '链', dataIndex: 'chain_name' },
            { title: '名称', dataIndex: 'name' },
            { title: '类型', dataIndex: 'provider_type' },
            { title: '状态', dataIndex: 'status', render: value => <Tag color={statusColor(String(value))}>{String(value)}</Tag> },
            { title: '优先级', dataIndex: 'priority' },
            { title: 'QPS', dataIndex: 'qps_limit' },
            { title: '超时(ms)', dataIndex: 'timeout_ms' },
          ]}
        />
      </Card>
    </Space>
  );
}

function Metric({ title, value, hint }: { title: string; value: string | number; hint: string }) {
  return (
    <Card className="status-card">
      <Space vertical align="start">
        <Text type="tertiary">{title}</Text>
        <Title heading={3}>{value}</Title>
        <Text type="tertiary">{hint}</Text>
      </Space>
    </Card>
  );
}
```

- [ ] **Step 4: Wire navigation**

In `frontend/src/App.tsx`, add import:

```typescript
import { SystemStatusPage } from './pages/SystemStatusPage';
```

Extend `PageKey`:

```typescript
  | 'system-status';
```

Add nav item after dashboard:

```typescript
{ itemKey: 'system-status', text: '系统状态', icon: <IconPulse /> },
```

Add render branch before dashboard fallback:

```typescript
if (page === 'system-status') return <SystemStatusPage />;
```

Update the dashboard banner description:

```typescript
description="当前版本提供登录、链配置、资产配置、Provider 配置、监听地址管理、事件中心、通知规则、站内通知和系统状态。"
```

- [ ] **Step 5: Run frontend build**

Run:

```bash
npm run build --prefix frontend
```

Expected: PASS. Existing lottie eval and chunk-size warnings are acceptable if exit code is 0.

## Task 6: Final verification

**Files:**

- Verify all changed backend/frontend files.

- [ ] **Step 1: Run Rust format check**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: PASS with no diff.

- [ ] **Step 2: Run backend workspace check**

Run:

```bash
cargo check --workspace --manifest-path backend/Cargo.toml
```

Expected: PASS. Existing `sqlx-postgres v0.7.4` future-incompat warning is acceptable if exit code is 0.

- [ ] **Step 3: Run backend workspace tests**

Run:

```bash
cargo test --workspace --manifest-path backend/Cargo.toml
```

Expected: PASS. Existing `sqlx-postgres v0.7.4` future-incompat warning is acceptable if exit code is 0.

- [ ] **Step 4: Run frontend build**

Run:

```bash
npm run build --prefix frontend
```

Expected: PASS. Existing lottie eval and chunk-size warnings are acceptable if exit code is 0.

- [ ] **Step 5: Validate Docker Compose config**

Run:

```bash
docker compose -f docker-compose.yml config
```

Expected: PASS with rendered config.

- [ ] **Step 6: Report verification evidence**

Report the exact command results. Do not claim completion unless all five verification commands exit 0.
