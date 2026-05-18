# Coin Listener Notification System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the first runnable Coin Listener notification skeleton so stored `address_events` enqueue notification tasks, `notifier` consumes them, matching rules create delivery audit rows, and in-app notifications are visible and readable from the frontend.

**Architecture:** Add shared notification models/config in `core`, notification tables and repositories in `storage`, and a Redis `NotifyQueue` as the worker-notifier boundary. The worker enqueues `NotifyEventTask` after a mock EVM event is inserted and before scan timestamps advance; the notifier consumes tasks, matches enabled rules, resolves channels, writes `notification_deliveries`, and creates `in_app_notifications` for `in_app` channels while marking unsupported channels as skipped.

**Tech Stack:** Rust, Tokio, SQLx, PostgreSQL, Redis, Serde, Chrono, Uuid, Axum, tracing, React, TypeScript, Vite, TanStack Query, Semi Design, Docker Compose.

---

## Scope

Implements `docs/superpowers/specs/2026-05-17-coin-listener-notification-system-design.md`.

Included:

- Notification tables: `notification_channels`, `notification_rules`, `notification_deliveries`, `in_app_notifications`.
- Shared `NotifyEventTask` queue message.
- `NOTIFY_QUEUE_KEY` config defaulting to `notify:event:queue`.
- Redis notify queue wrapper using `LPUSH` + `BRPOP`.
- Notification channel, rule, delivery, and in-app notification repository functions.
- Notification API endpoints for channels, rules, in-app list, and mark-read.
- Notifier rule matching, amount threshold comparison, in-app content generation, channel decisions, and consume loop.
- Worker integration that enqueues notify tasks after successful EVM mock event creation and before scan timestamp advancement.
- Frontend notification rules page and in-app notifications page.
- Backend unit tests and frontend build verification.

Excluded:

- Real Telegram sending.
- Real WebSocket push.
- Email, Discord, Enterprise WeChat, or webhook delivery execution.
- Retry queue and dead-letter queue.
- Notification templates.
- RBAC/team notification scope.
- Notification statistics dashboard.

## Git note

The current project directory has been observed as not being a git repository. Each task ends with a verification checkpoint instead of a required commit. If a future worker runs this plan inside a git repository, commit after the checkpoint with the exact message shown in that task.

## File Structure

Modify:

```text
.env.example
backend/crates/core/src/config.rs
backend/crates/core/src/lib.rs
backend/crates/core/src/models.rs
backend/crates/storage/src/lib.rs
backend/crates/api-server/src/routes.rs
backend/crates/notifier/Cargo.toml
backend/crates/notifier/src/main.rs
backend/crates/worker/src/lib.rs
backend/crates/worker/src/main.rs
frontend/src/api/client.ts
frontend/src/api/types.ts
frontend/src/App.tsx
```

Create:

```text
backend/crates/storage/migrations/0005_notifications.sql
backend/crates/storage/src/notify_queue.rs
backend/crates/storage/src/notifications.rs
backend/crates/notifier/src/lib.rs
frontend/src/pages/NotificationRulesPage.tsx
frontend/src/pages/InAppNotificationsPage.tsx
```

## Task 1: Add shared notification models, config, migration, and env

**Files:**

- Modify: `backend/crates/core/src/models.rs`
- Modify: `backend/crates/core/src/config.rs`
- Modify: `backend/crates/core/src/lib.rs`
- Create: `backend/crates/storage/migrations/0005_notifications.sql`
- Modify: `.env.example`
- Verify: `coin-listener-core` tests and backend check

- [ ] **Step 1: Write the failing notify task serialization test**

Update the test module at the bottom of `backend/crates/core/src/models.rs`.

Change the import:

```rust
use super::{NotifyEventTask, ScanAddressTask};
```

Add this test next to `scan_address_task_round_trips_as_json` before adding `NotifyEventTask`:

```rust
#[test]
fn notify_event_task_round_trips_as_json() {
    let task = NotifyEventTask {
        task_id: Uuid::from_u128(11),
        event_id: Uuid::from_u128(12),
        tenant_id: Uuid::from_u128(13),
        attempt: 1,
        enqueued_at: Utc.with_ymd_and_hms(2026, 5, 17, 15, 0, 0).unwrap(),
    };

    let payload = serde_json::to_string(&task).expect("serialize notify task");
    let decoded: NotifyEventTask = serde_json::from_str(&payload).expect("deserialize notify task");

    assert_eq!(decoded, task);
    assert!(payload.contains("\"attempt\":1"));
}
```

- [ ] **Step 2: Run the test to verify RED**

Run:

```bash
cargo test -p coin-listener-core notify_event_task_round_trips_as_json --manifest-path backend/Cargo.toml
```

Expected: FAIL with an unresolved import or missing type for `NotifyEventTask`.

- [ ] **Step 3: Add shared notification models**

Add these structs after `ScanAddressTask` in `backend/crates/core/src/models.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyEventTask {
    pub task_id: Uuid,
    pub event_id: Uuid,
    pub tenant_id: Uuid,
    pub attempt: u16,
    pub enqueued_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationChannel {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub channel_type: String,
    pub name: String,
    pub config: serde_json::Value,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateNotificationChannelRequest {
    pub channel_type: String,
    pub name: String,
    pub config: Option<serde_json::Value>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationRule {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub chain_id: Option<Uuid>,
    pub address_id: Option<Uuid>,
    pub asset_id: Option<Uuid>,
    pub event_type: Option<String>,
    pub is_transfer: Option<bool>,
    pub min_amount_raw: Option<String>,
    pub direction: Option<String>,
    pub channel_ids: Vec<Uuid>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateNotificationRuleRequest {
    pub name: String,
    pub chain_id: Option<Uuid>,
    pub address_id: Option<Uuid>,
    pub asset_id: Option<Uuid>,
    pub event_type: Option<String>,
    pub is_transfer: Option<bool>,
    pub min_amount_raw: Option<String>,
    pub direction: Option<String>,
    pub channel_ids: Option<Vec<Uuid>>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationDelivery {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub event_id: Uuid,
    pub rule_id: Option<Uuid>,
    pub channel_id: Option<Uuid>,
    pub status: String,
    pub attempt_count: i32,
    pub last_error: Option<String>,
    pub sent_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct InAppNotification {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub event_id: Uuid,
    pub delivery_id: Option<Uuid>,
    pub title: String,
    pub body: String,
    pub read_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InAppNotificationQuery {
    pub unread_only: Option<bool>,
}
```

- [ ] **Step 4: Add notify queue config**

Update `backend/crates/core/src/config.rs`.

Change `AppConfig` to include `notify`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub postgres: PostgresConfig,
    pub redis: RedisConfig,
    pub scan: ScanConfig,
    pub notify: NotifyConfig,
}
```

Add this struct after `ScanConfig`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct NotifyConfig {
    pub queue_key: String,
}
```

Add this merge to `AppConfig::from_env()` after the scan config merges:

```rust
.merge((
    "notify.queue_key",
    env::var("NOTIFY_QUEUE_KEY").unwrap_or_else(|_| "notify:event:queue".to_string()),
))
```

- [ ] **Step 5: Export `NotifyConfig`**

Update `backend/crates/core/src/lib.rs`:

```rust
pub use config::{AppConfig, NotifyConfig, PostgresConfig, RedisConfig, ScanConfig, ServerConfig};
```

- [ ] **Step 6: Create notification migration**

Create `backend/crates/storage/migrations/0005_notifications.sql` with this SQL:

```sql
CREATE TABLE IF NOT EXISTS notification_channels (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    channel_type TEXT NOT NULL,
    name TEXT NOT NULL,
    config JSONB NOT NULL DEFAULT '{}'::jsonb,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_notification_channels_tenant_status
    ON notification_channels(tenant_id, status, channel_type);

CREATE UNIQUE INDEX IF NOT EXISTS idx_notification_channels_default_in_app
    ON notification_channels(tenant_id, channel_type, name)
    WHERE channel_type = 'in_app' AND name = 'Default In-App';

CREATE TABLE IF NOT EXISTS notification_rules (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    chain_id UUID REFERENCES chains(id) ON DELETE CASCADE,
    address_id UUID REFERENCES watched_addresses(id) ON DELETE CASCADE,
    asset_id UUID REFERENCES assets(id) ON DELETE CASCADE,
    event_type TEXT,
    is_transfer BOOLEAN,
    min_amount_raw TEXT,
    direction TEXT,
    channel_ids UUID[] NOT NULL DEFAULT '{}'::uuid[],
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_notification_rules_tenant_enabled
    ON notification_rules(tenant_id, enabled, created_at DESC);

CREATE TABLE IF NOT EXISTS notification_deliveries (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    event_id UUID NOT NULL REFERENCES address_events(id) ON DELETE CASCADE,
    rule_id UUID REFERENCES notification_rules(id) ON DELETE SET NULL,
    channel_id UUID REFERENCES notification_channels(id) ON DELETE SET NULL,
    status TEXT NOT NULL,
    attempt_count INTEGER NOT NULL DEFAULT 1,
    last_error TEXT,
    sent_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_notification_deliveries_event
    ON notification_deliveries(event_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_notification_deliveries_tenant
    ON notification_deliveries(tenant_id, created_at DESC);

CREATE TABLE IF NOT EXISTS in_app_notifications (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    event_id UUID NOT NULL REFERENCES address_events(id) ON DELETE CASCADE,
    delivery_id UUID REFERENCES notification_deliveries(id) ON DELETE SET NULL,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    read_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_in_app_notifications_tenant_created
    ON in_app_notifications(tenant_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_in_app_notifications_unread
    ON in_app_notifications(tenant_id, created_at DESC)
    WHERE read_at IS NULL;
```

- [ ] **Step 7: Add notify env default**

Append this line to `.env.example`:

```text
NOTIFY_QUEUE_KEY=notify:event:queue
```

- [ ] **Step 8: Run the core test to verify GREEN**

Run:

```bash
cargo test -p coin-listener-core notify_event_task_round_trips_as_json --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 9: Checkpoint**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
cargo check -p coin-listener-core --manifest-path backend/Cargo.toml
```

Expected: both commands exit 0. If running inside git, commit with:

```bash
git add .env.example backend/crates/core/src/models.rs backend/crates/core/src/config.rs backend/crates/core/src/lib.rs backend/crates/storage/migrations/0005_notifications.sql
git commit -m "feat: add notification schema models"
```

## Task 2: Add Redis notify queue wrapper

**Files:**

- Create: `backend/crates/storage/src/notify_queue.rs`
- Modify: `backend/crates/storage/src/lib.rs`
- Verify: `coin-listener-storage` notify queue tests

- [ ] **Step 1: Write failing notify queue payload tests**

Create `backend/crates/storage/src/notify_queue.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::{deserialize_notify_task, serialize_notify_task};
    use chrono::{TimeZone, Utc};
    use coin_listener_core::models::NotifyEventTask;
    use uuid::Uuid;

    #[test]
    fn notify_task_payload_round_trips() {
        let task = NotifyEventTask {
            task_id: Uuid::from_u128(21),
            event_id: Uuid::from_u128(22),
            tenant_id: Uuid::from_u128(23),
            attempt: 1,
            enqueued_at: Utc.with_ymd_and_hms(2026, 5, 17, 16, 0, 0).unwrap(),
        };

        let payload = serialize_notify_task(&task).expect("serialize task");
        let decoded = deserialize_notify_task(&payload).expect("deserialize task");

        assert_eq!(decoded, task);
    }

    #[test]
    fn malformed_notify_task_payload_returns_error() {
        let result = deserialize_notify_task("not-json");

        assert!(result.is_err());
    }
}
```

Update `backend/crates/storage/src/lib.rs`:

```rust
pub mod notify_queue;
```

- [ ] **Step 2: Run tests to verify RED**

Run:

```bash
cargo test -p coin-listener-storage notify_task_payload --manifest-path backend/Cargo.toml
```

Expected: FAIL with missing `serialize_notify_task` and `deserialize_notify_task`.

- [ ] **Step 3: Implement `NotifyQueue`**

Replace the contents of `backend/crates/storage/src/notify_queue.rs` with:

```rust
use coin_listener_core::{models::NotifyEventTask, AppError, AppResult};
use redis::{aio::MultiplexedConnection, Client};

#[derive(Debug, Clone)]
pub struct NotifyQueue {
    queue_key: String,
}

impl NotifyQueue {
    pub fn new(queue_key: String) -> Self {
        Self { queue_key }
    }

    pub fn queue_key(&self) -> &str {
        &self.queue_key
    }

    pub async fn enqueue(
        &self,
        connection: &mut MultiplexedConnection,
        task: &NotifyEventTask,
    ) -> AppResult<()> {
        let payload = serialize_notify_task(task)?;
        let _: usize = redis::cmd("LPUSH")
            .arg(&self.queue_key)
            .arg(payload)
            .query_async(connection)
            .await
            .map_err(|error| AppError::Redis(error.to_string()))?;
        Ok(())
    }

    pub async fn dequeue(
        &self,
        connection: &mut MultiplexedConnection,
        timeout_seconds: usize,
    ) -> AppResult<Option<NotifyEventTask>> {
        let result: Option<(String, String)> = redis::cmd("BRPOP")
            .arg(&self.queue_key)
            .arg(timeout_seconds)
            .query_async(connection)
            .await
            .map_err(|error| AppError::Redis(error.to_string()))?;

        result
            .map(|(_, payload)| deserialize_notify_task(&payload))
            .transpose()
    }
}

pub async fn connect_notify_queue(client: &Client) -> AppResult<MultiplexedConnection> {
    client
        .get_multiplexed_async_connection()
        .await
        .map_err(|error| AppError::Redis(error.to_string()))
}

pub fn serialize_notify_task(task: &NotifyEventTask) -> AppResult<String> {
    serde_json::to_string(task).map_err(|error| AppError::Validation(error.to_string()))
}

pub fn deserialize_notify_task(payload: &str) -> AppResult<NotifyEventTask> {
    serde_json::from_str(payload).map_err(|error| AppError::Validation(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{deserialize_notify_task, serialize_notify_task};
    use chrono::{TimeZone, Utc};
    use coin_listener_core::models::NotifyEventTask;
    use uuid::Uuid;

    #[test]
    fn notify_task_payload_round_trips() {
        let task = NotifyEventTask {
            task_id: Uuid::from_u128(21),
            event_id: Uuid::from_u128(22),
            tenant_id: Uuid::from_u128(23),
            attempt: 1,
            enqueued_at: Utc.with_ymd_and_hms(2026, 5, 17, 16, 0, 0).unwrap(),
        };

        let payload = serialize_notify_task(&task).expect("serialize task");
        let decoded = deserialize_notify_task(&payload).expect("deserialize task");

        assert_eq!(decoded, task);
    }

    #[test]
    fn malformed_notify_task_payload_returns_error() {
        let result = deserialize_notify_task("not-json");

        assert!(result.is_err());
    }
}
```

Ensure `backend/crates/storage/src/lib.rs` contains this module export:

```rust
pub mod notify_queue;
```

- [ ] **Step 4: Run tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-storage notify_task_payload --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 5: Checkpoint**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
cargo check -p coin-listener-storage --manifest-path backend/Cargo.toml
```

Expected: both commands exit 0. If running inside git, commit with:

```bash
git add backend/crates/storage/src/notify_queue.rs backend/crates/storage/src/lib.rs
git commit -m "feat: add notify redis queue"
```

## Task 3: Add notification storage repository

**Files:**

- Create: `backend/crates/storage/src/notifications.rs`
- Modify: `backend/crates/storage/src/lib.rs`
- Verify: `coin-listener-storage` notification repository tests

- [ ] **Step 1: Write failing repository validation tests**

Create `backend/crates/storage/src/notifications.rs` with these tests first:

```rust
use coin_listener_core::{models::CreateNotificationChannelRequest, AppError, AppResult};

pub const DEFAULT_TENANT_ID: uuid::Uuid = uuid::Uuid::from_u128(1);
pub const DEFAULT_IN_APP_CHANNEL_NAME: &str = "Default In-App";

pub fn validate_notification_channel_request(
    _request: &CreateNotificationChannelRequest,
) -> AppResult<()> {
    Err(AppError::Validation("not implemented".to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        validate_notification_channel_request, validate_notification_rule_request,
        DEFAULT_IN_APP_CHANNEL_NAME,
    };
    use coin_listener_core::{
        models::{CreateNotificationChannelRequest, CreateNotificationRuleRequest},
        AppError,
    };

    #[test]
    fn channel_validation_rejects_unknown_type() {
        let request = CreateNotificationChannelRequest {
            channel_type: "email".to_string(),
            name: "Email".to_string(),
            config: None,
            status: Some("active".to_string()),
        };

        let result = validate_notification_channel_request(&request);

        assert!(matches!(
            result,
            Err(AppError::Validation(message)) if message == "channel_type must be in_app, telegram, or webhook"
        ));
    }

    #[test]
    fn rule_validation_rejects_invalid_min_amount_raw() {
        let request = CreateNotificationRuleRequest {
            name: "Large transfers".to_string(),
            chain_id: None,
            address_id: None,
            asset_id: None,
            event_type: None,
            is_transfer: None,
            min_amount_raw: Some("12.5".to_string()),
            direction: None,
            channel_ids: None,
            enabled: Some(true),
        };

        let result = validate_notification_rule_request(&request);

        assert!(matches!(
            result,
            Err(AppError::Validation(message)) if message == "min_amount_raw must be a non-negative integer string"
        ));
    }

    #[test]
    fn default_in_app_channel_name_is_stable() {
        assert_eq!(DEFAULT_IN_APP_CHANNEL_NAME, "Default In-App");
    }
}
```

Update `backend/crates/storage/src/lib.rs`:

```rust
pub mod notifications;
```

- [ ] **Step 2: Run tests to verify RED**

Run:

```bash
cargo test -p coin-listener-storage notification --manifest-path backend/Cargo.toml
```

Expected: FAIL because `validate_notification_rule_request` is missing or channel validation returns the temporary validation message.

- [ ] **Step 3: Implement validation helpers and repository functions**

Replace `backend/crates/storage/src/notifications.rs` with this implementation:

```rust
use chrono::{DateTime, Utc};
use coin_listener_core::{
    models::{
        AddressEvent, CreateNotificationChannelRequest, CreateNotificationRuleRequest,
        InAppNotification, InAppNotificationQuery, NotificationChannel, NotificationDelivery,
        NotificationRule,
    },
    AppError, AppResult,
};
use sqlx::PgPool;
use uuid::Uuid;

pub const DEFAULT_TENANT_ID: Uuid = Uuid::from_u128(1);
pub const DEFAULT_IN_APP_CHANNEL_NAME: &str = "Default In-App";
const CHANNEL_TYPE_IN_APP: &str = "in_app";
const CHANNEL_TYPE_TELEGRAM: &str = "telegram";
const CHANNEL_TYPE_WEBHOOK: &str = "webhook";
const STATUS_ACTIVE: &str = "active";
const STATUS_INACTIVE: &str = "inactive";

pub fn validate_notification_channel_request(
    request: &CreateNotificationChannelRequest,
) -> AppResult<()> {
    if request.name.trim().is_empty() {
        return Err(AppError::Validation("channel name is required".to_string()));
    }
    if !matches!(
        request.channel_type.as_str(),
        CHANNEL_TYPE_IN_APP | CHANNEL_TYPE_TELEGRAM | CHANNEL_TYPE_WEBHOOK
    ) {
        return Err(AppError::Validation(
            "channel_type must be in_app, telegram, or webhook".to_string(),
        ));
    }
    if let Some(status) = &request.status {
        if !matches!(status.as_str(), STATUS_ACTIVE | STATUS_INACTIVE) {
            return Err(AppError::Validation(
                "status must be active or inactive".to_string(),
            ));
        }
    }
    Ok(())
}

pub fn validate_notification_rule_request(request: &CreateNotificationRuleRequest) -> AppResult<()> {
    if request.name.trim().is_empty() {
        return Err(AppError::Validation("rule name is required".to_string()));
    }
    if let Some(min_amount_raw) = &request.min_amount_raw {
        if min_amount_raw.is_empty() || !min_amount_raw.chars().all(|character| character.is_ascii_digit()) {
            return Err(AppError::Validation(
                "min_amount_raw must be a non-negative integer string".to_string(),
            ));
        }
    }
    if let Some(direction) = &request.direction {
        if !matches!(direction.as_str(), "in" | "out" | "self" | "unknown") {
            return Err(AppError::Validation(
                "direction must be in, out, self, or unknown".to_string(),
            ));
        }
    }
    Ok(())
}

pub async fn list_notification_channels(pool: &PgPool) -> AppResult<Vec<NotificationChannel>> {
    sqlx::query_as::<_, NotificationChannel>(
        r#"
        SELECT id, tenant_id, channel_type, name, config, status, created_at, updated_at
        FROM notification_channels
        WHERE tenant_id = $1
        ORDER BY created_at DESC
        "#,
    )
    .bind(DEFAULT_TENANT_ID)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn create_notification_channel(
    pool: &PgPool,
    request: CreateNotificationChannelRequest,
) -> AppResult<NotificationChannel> {
    validate_notification_channel_request(&request)?;
    let config = request.config.unwrap_or_else(|| serde_json::json!({}));
    let status = request.status.unwrap_or_else(|| STATUS_ACTIVE.to_string());

    sqlx::query_as::<_, NotificationChannel>(
        r#"
        INSERT INTO notification_channels (tenant_id, channel_type, name, config, status)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, tenant_id, channel_type, name, config, status, created_at, updated_at
        "#,
    )
    .bind(DEFAULT_TENANT_ID)
    .bind(request.channel_type)
    .bind(request.name)
    .bind(config)
    .bind(status)
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn list_notification_rules(pool: &PgPool) -> AppResult<Vec<NotificationRule>> {
    sqlx::query_as::<_, NotificationRule>(
        r#"
        SELECT id, tenant_id, name, chain_id, address_id, asset_id, event_type, is_transfer,
               min_amount_raw, direction, channel_ids, enabled, created_at, updated_at
        FROM notification_rules
        WHERE tenant_id = $1
        ORDER BY created_at DESC
        "#,
    )
    .bind(DEFAULT_TENANT_ID)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn list_enabled_notification_rules(
    pool: &PgPool,
    tenant_id: Uuid,
) -> AppResult<Vec<NotificationRule>> {
    sqlx::query_as::<_, NotificationRule>(
        r#"
        SELECT id, tenant_id, name, chain_id, address_id, asset_id, event_type, is_transfer,
               min_amount_raw, direction, channel_ids, enabled, created_at, updated_at
        FROM notification_rules
        WHERE tenant_id = $1
          AND enabled = TRUE
        ORDER BY created_at ASC
        "#,
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn create_notification_rule(
    pool: &PgPool,
    request: CreateNotificationRuleRequest,
) -> AppResult<NotificationRule> {
    validate_notification_rule_request(&request)?;
    let channel_ids = request.channel_ids.unwrap_or_default();
    let enabled = request.enabled.unwrap_or(true);

    sqlx::query_as::<_, NotificationRule>(
        r#"
        INSERT INTO notification_rules (
            tenant_id, name, chain_id, address_id, asset_id, event_type, is_transfer,
            min_amount_raw, direction, channel_ids, enabled
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        RETURNING id, tenant_id, name, chain_id, address_id, asset_id, event_type, is_transfer,
                  min_amount_raw, direction, channel_ids, enabled, created_at, updated_at
        "#,
    )
    .bind(DEFAULT_TENANT_ID)
    .bind(request.name)
    .bind(request.chain_id)
    .bind(request.address_id)
    .bind(request.asset_id)
    .bind(request.event_type)
    .bind(request.is_transfer)
    .bind(request.min_amount_raw)
    .bind(request.direction)
    .bind(channel_ids)
    .bind(enabled)
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn update_notification_rule(
    pool: &PgPool,
    id: Uuid,
    request: CreateNotificationRuleRequest,
) -> AppResult<NotificationRule> {
    validate_notification_rule_request(&request)?;
    let channel_ids = request.channel_ids.unwrap_or_default();
    let enabled = request.enabled.unwrap_or(true);

    sqlx::query_as::<_, NotificationRule>(
        r#"
        UPDATE notification_rules
        SET name = $2,
            chain_id = $3,
            address_id = $4,
            asset_id = $5,
            event_type = $6,
            is_transfer = $7,
            min_amount_raw = $8,
            direction = $9,
            channel_ids = $10,
            enabled = $11,
            updated_at = NOW()
        WHERE id = $1
          AND tenant_id = $12
        RETURNING id, tenant_id, name, chain_id, address_id, asset_id, event_type, is_transfer,
                  min_amount_raw, direction, channel_ids, enabled, created_at, updated_at
        "#,
    )
    .bind(id)
    .bind(request.name)
    .bind(request.chain_id)
    .bind(request.address_id)
    .bind(request.asset_id)
    .bind(request.event_type)
    .bind(request.is_transfer)
    .bind(request.min_amount_raw)
    .bind(request.direction)
    .bind(channel_ids)
    .bind(enabled)
    .bind(DEFAULT_TENANT_ID)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("notification rule".to_string()))
}

pub async fn delete_notification_rule(pool: &PgPool, id: Uuid) -> AppResult<()> {
    let result = sqlx::query("DELETE FROM notification_rules WHERE id = $1 AND tenant_id = $2")
        .bind(id)
        .bind(DEFAULT_TENANT_ID)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("notification rule".to_string()));
    }

    Ok(())
}

pub async fn list_in_app_notifications(
    pool: &PgPool,
    query: InAppNotificationQuery,
) -> AppResult<Vec<InAppNotification>> {
    sqlx::query_as::<_, InAppNotification>(
        r#"
        SELECT id, tenant_id, event_id, delivery_id, title, body, read_at, created_at
        FROM in_app_notifications
        WHERE tenant_id = $1
          AND ($2::boolean IS NULL OR read_at IS NULL)
        ORDER BY created_at DESC
        LIMIT 200
        "#,
    )
    .bind(DEFAULT_TENANT_ID)
    .bind(query.unread_only.filter(|value| *value))
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn mark_in_app_notification_read(pool: &PgPool, id: Uuid) -> AppResult<InAppNotification> {
    sqlx::query_as::<_, InAppNotification>(
        r#"
        UPDATE in_app_notifications
        SET read_at = COALESCE(read_at, NOW())
        WHERE id = $1
          AND tenant_id = $2
        RETURNING id, tenant_id, event_id, delivery_id, title, body, read_at, created_at
        "#,
    )
    .bind(id)
    .bind(DEFAULT_TENANT_ID)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("in-app notification".to_string()))
}

pub async fn get_address_event(pool: &PgPool, event_id: Uuid) -> AppResult<AddressEvent> {
    sqlx::query_as::<_, AddressEvent>(
        r#"
        SELECT id, tenant_id, chain_id, address_id, asset_id, event_type, direction, is_transfer,
               tx_hash, log_index, block_number, block_hash, confirmations, from_address, to_address,
               amount_raw, amount_decimal, balance_before_raw, balance_after_raw, balance_delta_raw,
               metadata, detected_at, created_at
        FROM address_events
        WHERE id = $1
        "#,
    )
    .bind(event_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("address event".to_string()))
}

pub async fn list_active_channels_by_ids(
    pool: &PgPool,
    tenant_id: Uuid,
    channel_ids: &[Uuid],
) -> AppResult<Vec<NotificationChannel>> {
    sqlx::query_as::<_, NotificationChannel>(
        r#"
        SELECT id, tenant_id, channel_type, name, config, status, created_at, updated_at
        FROM notification_channels
        WHERE tenant_id = $1
          AND status = 'active'
          AND id = ANY($2)
        ORDER BY created_at ASC
        "#,
    )
    .bind(tenant_id)
    .bind(channel_ids)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn get_or_create_default_in_app_channel(
    pool: &PgPool,
    tenant_id: Uuid,
) -> AppResult<NotificationChannel> {
    if let Some(channel) = sqlx::query_as::<_, NotificationChannel>(
        r#"
        SELECT id, tenant_id, channel_type, name, config, status, created_at, updated_at
        FROM notification_channels
        WHERE tenant_id = $1
          AND channel_type = 'in_app'
          AND status = 'active'
        ORDER BY created_at ASC
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    {
        return Ok(channel);
    }

    let inserted = sqlx::query_as::<_, NotificationChannel>(
        r#"
        INSERT INTO notification_channels (tenant_id, channel_type, name, config, status)
        VALUES ($1, 'in_app', $2, '{}'::jsonb, 'active')
        ON CONFLICT DO NOTHING
        RETURNING id, tenant_id, channel_type, name, config, status, created_at, updated_at
        "#,
    )
    .bind(tenant_id)
    .bind(DEFAULT_IN_APP_CHANNEL_NAME)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?;

    if let Some(channel) = inserted {
        return Ok(channel);
    }

    sqlx::query_as::<_, NotificationChannel>(
        r#"
        SELECT id, tenant_id, channel_type, name, config, status, created_at, updated_at
        FROM notification_channels
        WHERE tenant_id = $1
          AND channel_type = 'in_app'
          AND name = $2
          AND status = 'active'
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .bind(DEFAULT_IN_APP_CHANNEL_NAME)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("default in-app channel".to_string()))
}

pub async fn create_notification_delivery(
    pool: &PgPool,
    tenant_id: Uuid,
    event_id: Uuid,
    rule_id: Option<Uuid>,
    channel_id: Option<Uuid>,
    status: &str,
    last_error: Option<String>,
    sent_at: Option<DateTime<Utc>>,
) -> AppResult<NotificationDelivery> {
    sqlx::query_as::<_, NotificationDelivery>(
        r#"
        INSERT INTO notification_deliveries (
            tenant_id, event_id, rule_id, channel_id, status, last_error, sent_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, tenant_id, event_id, rule_id, channel_id, status, attempt_count,
                  last_error, sent_at, created_at
        "#,
    )
    .bind(tenant_id)
    .bind(event_id)
    .bind(rule_id)
    .bind(channel_id)
    .bind(status)
    .bind(last_error)
    .bind(sent_at)
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn update_notification_delivery_status(
    pool: &PgPool,
    id: Uuid,
    status: &str,
    last_error: Option<String>,
    sent_at: Option<DateTime<Utc>>,
) -> AppResult<NotificationDelivery> {
    sqlx::query_as::<_, NotificationDelivery>(
        r#"
        UPDATE notification_deliveries
        SET status = $2,
            last_error = $3,
            sent_at = $4
        WHERE id = $1
        RETURNING id, tenant_id, event_id, rule_id, channel_id, status, attempt_count,
                  last_error, sent_at, created_at
        "#,
    )
    .bind(id)
    .bind(status)
    .bind(last_error)
    .bind(sent_at)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("notification delivery".to_string()))
}

pub async fn create_in_app_notification(
    pool: &PgPool,
    tenant_id: Uuid,
    event_id: Uuid,
    delivery_id: Option<Uuid>,
    title: String,
    body: String,
) -> AppResult<InAppNotification> {
    sqlx::query_as::<_, InAppNotification>(
        r#"
        INSERT INTO in_app_notifications (tenant_id, event_id, delivery_id, title, body)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, tenant_id, event_id, delivery_id, title, body, read_at, created_at
        "#,
    )
    .bind(tenant_id)
    .bind(event_id)
    .bind(delivery_id)
    .bind(title)
    .bind(body)
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        validate_notification_channel_request, validate_notification_rule_request,
        DEFAULT_IN_APP_CHANNEL_NAME,
    };
    use coin_listener_core::{
        models::{CreateNotificationChannelRequest, CreateNotificationRuleRequest},
        AppError,
    };

    #[test]
    fn channel_validation_rejects_unknown_type() {
        let request = CreateNotificationChannelRequest {
            channel_type: "email".to_string(),
            name: "Email".to_string(),
            config: None,
            status: Some("active".to_string()),
        };

        let result = validate_notification_channel_request(&request);

        assert!(matches!(
            result,
            Err(AppError::Validation(message)) if message == "channel_type must be in_app, telegram, or webhook"
        ));
    }

    #[test]
    fn rule_validation_rejects_invalid_min_amount_raw() {
        let request = CreateNotificationRuleRequest {
            name: "Large transfers".to_string(),
            chain_id: None,
            address_id: None,
            asset_id: None,
            event_type: None,
            is_transfer: None,
            min_amount_raw: Some("12.5".to_string()),
            direction: None,
            channel_ids: None,
            enabled: Some(true),
        };

        let result = validate_notification_rule_request(&request);

        assert!(matches!(
            result,
            Err(AppError::Validation(message)) if message == "min_amount_raw must be a non-negative integer string"
        ));
    }

    #[test]
    fn default_in_app_channel_name_is_stable() {
        assert_eq!(DEFAULT_IN_APP_CHANNEL_NAME, "Default In-App");
    }
}
```

Ensure `backend/crates/storage/src/lib.rs` contains:

```rust
pub mod notifications;
```

- [ ] **Step 4: Run tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-storage notification --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 5: Checkpoint**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
cargo check -p coin-listener-storage --manifest-path backend/Cargo.toml
```

Expected: both commands exit 0. If running inside git, commit with:

```bash
git add backend/crates/storage/src/notifications.rs backend/crates/storage/src/lib.rs
git commit -m "feat: add notification repositories"
```

## Task 4: Add notification API routes

**Files:**

- Modify: `backend/crates/api-server/src/routes.rs`
- Verify: `api-server` route tests and backend check

- [ ] **Step 1: Write failing route exposure tests**

Add these tests inside the existing `#[cfg(test)] mod tests` in `backend/crates/api-server/src/routes.rs`:

```rust
#[tokio::test]
async fn router_exposes_in_app_notifications_query() {
    let app = build_router(Arc::new(ApiState {
        postgres: PgPool::connect_lazy("postgres://postgres:postgres@localhost/coin_listener_test")
            .expect("valid postgres url"),
        enable_dev_routes: true,
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/in-app-notifications?unread_only=not-bool")
                .body(Body::empty())
                .expect("valid request"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn router_exposes_notification_rule_id_routes() {
    let app = build_router(Arc::new(ApiState {
        postgres: PgPool::connect_lazy("postgres://postgres:postgres@localhost/coin_listener_test")
            .expect("valid postgres url"),
        enable_dev_routes: true,
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/api/notification-rules/not-a-uuid")
                .body(Body::empty())
                .expect("valid request"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn router_exposes_mark_in_app_notification_read_route() {
    let app = build_router(Arc::new(ApiState {
        postgres: PgPool::connect_lazy("postgres://postgres:postgres@localhost/coin_listener_test")
            .expect("valid postgres url"),
        enable_dev_routes: true,
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/in-app-notifications/not-a-uuid/read")
                .body(Body::empty())
                .expect("valid request"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
```

- [ ] **Step 2: Run tests to verify RED**

Run:

```bash
cargo test -p api-server router_exposes_in_app_notifications_query router_exposes_notification_rule_id_routes router_exposes_mark_in_app_notification_read_route --manifest-path backend/Cargo.toml
```

Expected: at least one test FAILS with `404 NOT_FOUND` because the routes are not registered.

- [ ] **Step 3: Add imports for notification models and repository module**

Update the imports at the top of `backend/crates/api-server/src/routes.rs`.

Change the model import to include notification DTOs:

```rust
models::{
    CreateNotificationChannelRequest, CreateNotificationRuleRequest, CreateProviderRequest,
    CreateWatchedAddressRequest, EventQuery, InAppNotificationQuery, LoginRequest, LoginResponse,
    UserSummary,
},
```

Change the storage import:

```rust
use coin_listener_storage::{notifications, repositories};
```

- [ ] **Step 4: Register notification routes**

Update `build_router` so the main router includes these routes before the semicolon:

```rust
.route(
    "/api/notification-channels",
    get(list_notification_channels).post(create_notification_channel),
)
.route(
    "/api/notification-rules",
    get(list_notification_rules).post(create_notification_rule),
)
.route(
    "/api/notification-rules/:id",
    put(update_notification_rule).delete(delete_notification_rule),
)
.route("/api/in-app-notifications", get(list_in_app_notifications))
.route(
    "/api/in-app-notifications/:id/read",
    post(mark_in_app_notification_read),
)
```

The router chain should still include `.route("/api/events", get(list_events))` and the dev route gating should remain unchanged.

- [ ] **Step 5: Add notification route handlers**

Add these handlers after `list_events` and before `scan_address`:

```rust
async fn list_notification_channels(
    State(state): State<Arc<ApiState>>,
) -> Result<Response, ApiError> {
    let channels = notifications::list_notification_channels(&state.postgres).await?;
    Ok(Json(channels).into_response())
}

async fn create_notification_channel(
    State(state): State<Arc<ApiState>>,
    Json(request): Json<CreateNotificationChannelRequest>,
) -> Result<Response, ApiError> {
    let channel = notifications::create_notification_channel(&state.postgres, request).await?;
    Ok((StatusCode::CREATED, Json(channel)).into_response())
}

async fn list_notification_rules(State(state): State<Arc<ApiState>>) -> Result<Response, ApiError> {
    let rules = notifications::list_notification_rules(&state.postgres).await?;
    Ok(Json(rules).into_response())
}

async fn create_notification_rule(
    State(state): State<Arc<ApiState>>,
    Json(request): Json<CreateNotificationRuleRequest>,
) -> Result<Response, ApiError> {
    let rule = notifications::create_notification_rule(&state.postgres, request).await?;
    Ok((StatusCode::CREATED, Json(rule)).into_response())
}

async fn update_notification_rule(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<Uuid>,
    Json(request): Json<CreateNotificationRuleRequest>,
) -> Result<Response, ApiError> {
    let rule = notifications::update_notification_rule(&state.postgres, id, request).await?;
    Ok(Json(rule).into_response())
}

async fn delete_notification_rule(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    notifications::delete_notification_rule(&state.postgres, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_in_app_notifications(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<InAppNotificationQuery>,
) -> Result<Response, ApiError> {
    let notifications = notifications::list_in_app_notifications(&state.postgres, query).await?;
    Ok(Json(notifications).into_response())
}

async fn mark_in_app_notification_read(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let notification = notifications::mark_in_app_notification_read(&state.postgres, id).await?;
    Ok(Json(notification).into_response())
}
```

- [ ] **Step 6: Run route tests to verify GREEN**

Run:

```bash
cargo test -p api-server router_exposes_in_app_notifications_query router_exposes_notification_rule_id_routes router_exposes_mark_in_app_notification_read_route --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 7: Checkpoint**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
cargo check -p api-server --manifest-path backend/Cargo.toml
```

Expected: both commands exit 0. If running inside git, commit with:

```bash
git add backend/crates/api-server/src/routes.rs
git commit -m "feat: expose notification api routes"
```

## Task 5: Add notifier matching and decision logic

**Files:**

- Modify: `backend/crates/notifier/Cargo.toml`
- Create: `backend/crates/notifier/src/lib.rs`
- Verify: `notifier` unit tests

- [ ] **Step 1: Add notifier dependencies**

Update `backend/crates/notifier/Cargo.toml` dependencies to include storage, Redis, SQLx, and Chrono:

```toml
[dependencies]
anyhow.workspace = true
chrono.workspace = true
coin-listener-core = { path = "../core" }
coin-listener-storage = { path = "../storage" }
redis.workspace = true
sqlx.workspace = true
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
```

- [ ] **Step 2: Write failing matching tests**

Create `backend/crates/notifier/src/lib.rs` with these tests first:

```rust
#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use coin_listener_core::models::{AddressEvent, NotificationRule};
    use uuid::Uuid;

    use crate::{
        amount_raw_meets_minimum, build_in_app_notification_content,
        notification_rule_matches_event, notify_channel_decision, NotifyChannelDecision,
    };

    fn event() -> AddressEvent {
        AddressEvent {
            id: Uuid::from_u128(1),
            tenant_id: Uuid::from_u128(2),
            chain_id: Uuid::from_u128(3),
            address_id: Uuid::from_u128(4),
            asset_id: Uuid::from_u128(5),
            event_type: "transfer".to_string(),
            direction: "in".to_string(),
            is_transfer: true,
            tx_hash: Some("0xabc".to_string()),
            log_index: Some(0),
            block_number: Some(100),
            block_hash: None,
            confirmations: 12,
            from_address: Some("0xfrom".to_string()),
            to_address: Some("0xto".to_string()),
            amount_raw: Some("1000".to_string()),
            amount_decimal: Some("0.000000000000001".to_string()),
            balance_before_raw: None,
            balance_after_raw: None,
            balance_delta_raw: None,
            metadata: serde_json::json!({}),
            detected_at: Utc.with_ymd_and_hms(2026, 5, 17, 17, 0, 0).unwrap(),
            created_at: Utc.with_ymd_and_hms(2026, 5, 17, 17, 0, 1).unwrap(),
        }
    }

    fn rule() -> NotificationRule {
        NotificationRule {
            id: Uuid::from_u128(10),
            tenant_id: Uuid::from_u128(2),
            name: "Inbound transfers".to_string(),
            chain_id: Some(Uuid::from_u128(3)),
            address_id: Some(Uuid::from_u128(4)),
            asset_id: Some(Uuid::from_u128(5)),
            event_type: Some("transfer".to_string()),
            is_transfer: Some(true),
            min_amount_raw: Some("1000".to_string()),
            direction: Some("in".to_string()),
            channel_ids: vec![],
            enabled: true,
            created_at: Utc.with_ymd_and_hms(2026, 5, 17, 16, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 5, 17, 16, 0, 0).unwrap(),
        }
    }

    #[test]
    fn rule_matches_when_all_filters_match() {
        assert!(notification_rule_matches_event(&rule(), &event()));
    }

    #[test]
    fn rule_does_not_match_when_amount_is_below_minimum() {
        let mut event = event();
        event.amount_raw = Some("999".to_string());

        assert!(!notification_rule_matches_event(&rule(), &event));
    }

    #[test]
    fn amount_comparison_handles_large_integer_strings() {
        assert!(amount_raw_meets_minimum(
            Some("100000000000000000000"),
            Some("99999999999999999999")
        ));
        assert!(amount_raw_meets_minimum(Some("00100"), Some("100")));
        assert!(!amount_raw_meets_minimum(Some("99"), Some("100")));
        assert!(!amount_raw_meets_minimum(None, Some("1")));
        assert!(!amount_raw_meets_minimum(Some("abc"), Some("1")));
    }

    #[test]
    fn in_app_content_uses_stable_event_fields() {
        let (title, body) = build_in_app_notification_content(&event());

        assert_eq!(title, "transfer in");
        assert!(body.contains("address: 00000000-0000-0000-0000-000000000004"));
        assert!(body.contains("asset: 00000000-0000-0000-0000-000000000005"));
        assert!(body.contains("amount: 0.000000000000001"));
        assert!(body.contains("tx: 0xabc"));
    }

    #[test]
    fn unsupported_channel_is_skipped() {
        assert_eq!(
            notify_channel_decision("telegram"),
            NotifyChannelDecision::Skipped {
                last_error: "channel type not implemented"
            }
        );
    }
}
```

- [ ] **Step 3: Run tests to verify RED**

Run:

```bash
cargo test -p notifier --manifest-path backend/Cargo.toml
```

Expected: FAIL with missing functions and types in `notifier/src/lib.rs`.

- [ ] **Step 4: Implement pure notifier logic**

Add this code above the test module in `backend/crates/notifier/src/lib.rs`:

```rust
use std::cmp::Ordering;

use coin_listener_core::models::{AddressEvent, NotificationRule};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyChannelDecision {
    InApp,
    Skipped { last_error: &'static str },
}

pub fn notification_rule_matches_event(rule: &NotificationRule, event: &AddressEvent) -> bool {
    if rule.tenant_id != event.tenant_id || !rule.enabled {
        return false;
    }
    if rule.chain_id.is_some_and(|chain_id| chain_id != event.chain_id) {
        return false;
    }
    if rule.address_id.is_some_and(|address_id| address_id != event.address_id) {
        return false;
    }
    if rule.asset_id.is_some_and(|asset_id| asset_id != event.asset_id) {
        return false;
    }
    if rule
        .event_type
        .as_ref()
        .is_some_and(|event_type| event_type != &event.event_type)
    {
        return false;
    }
    if rule.is_transfer.is_some_and(|is_transfer| is_transfer != event.is_transfer) {
        return false;
    }
    if rule
        .direction
        .as_ref()
        .is_some_and(|direction| direction != &event.direction)
    {
        return false;
    }

    amount_raw_meets_minimum(event.amount_raw.as_deref(), rule.min_amount_raw.as_deref())
}

pub fn amount_raw_meets_minimum(amount_raw: Option<&str>, min_amount_raw: Option<&str>) -> bool {
    let Some(min_amount_raw) = min_amount_raw else {
        return true;
    };
    let Some(amount_raw) = amount_raw else {
        return false;
    };
    let Some(amount) = normalize_non_negative_integer(amount_raw) else {
        return false;
    };
    let Some(minimum) = normalize_non_negative_integer(min_amount_raw) else {
        return false;
    };

    match amount.len().cmp(&minimum.len()) {
        Ordering::Greater => true,
        Ordering::Less => false,
        Ordering::Equal => amount >= minimum,
    }
}

pub fn build_in_app_notification_content(event: &AddressEvent) -> (String, String) {
    let title = format!("{} {}", event.event_type, event.direction);
    let amount = event
        .amount_decimal
        .as_deref()
        .or(event.amount_raw.as_deref())
        .unwrap_or("-");
    let tx_hash = event.tx_hash.as_deref().unwrap_or("-");
    let body = format!(
        "address: {}; asset: {}; amount: {}; tx: {}",
        event.address_id, event.asset_id, amount, tx_hash
    );

    (title, body)
}

pub fn notify_channel_decision(channel_type: &str) -> NotifyChannelDecision {
    match channel_type {
        "in_app" => NotifyChannelDecision::InApp,
        _ => NotifyChannelDecision::Skipped {
            last_error: "channel type not implemented",
        },
    }
}

fn normalize_non_negative_integer(value: &str) -> Option<&str> {
    if value.is_empty() || !value.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    let normalized = value.trim_start_matches('0');
    if normalized.is_empty() {
        Some("0")
    } else {
        Some(normalized)
    }
}
```

- [ ] **Step 5: Run tests to verify GREEN**

Run:

```bash
cargo test -p notifier --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 6: Checkpoint**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
cargo check -p notifier --manifest-path backend/Cargo.toml
```

Expected: both commands exit 0. If running inside git, commit with:

```bash
git add backend/crates/notifier/Cargo.toml backend/crates/notifier/src/lib.rs
git commit -m "feat: add notifier rule matching"
```

## Task 6: Add notifier queue processing loop

**Files:**

- Modify: `backend/crates/notifier/src/lib.rs`
- Modify: `backend/crates/notifier/src/main.rs`
- Verify: `notifier` tests and backend check

- [ ] **Step 1: Write failing notifier shutdown test**

Add this test inside the existing test module in `backend/crates/notifier/src/lib.rs`:

```rust
#[test]
fn set_shutdown_flag_stops_notifier_loop() {
    use std::sync::atomic::AtomicBool;

    let shutdown = AtomicBool::new(true);

    assert!(crate::notifier_shutdown_requested(&shutdown));
}
```

- [ ] **Step 2: Run test to verify RED**

Run:

```bash
cargo test -p notifier set_shutdown_flag_stops_notifier_loop --manifest-path backend/Cargo.toml
```

Expected: FAIL with missing `notifier_shutdown_requested`.

- [ ] **Step 3: Add notifier orchestration code**

Add these imports to the top of `backend/crates/notifier/src/lib.rs`:

```rust
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering as AtomicOrdering},
    Arc,
};

use chrono::Utc;
use coin_listener_core::{models::NotifyEventTask, AppError, AppResult};
use coin_listener_storage::{notifications, notify_queue::NotifyQueue};
use redis::aio::MultiplexedConnection;
use sqlx::PgPool;
use tracing::{info, warn};
use uuid::Uuid;
```

Keep the existing `std::cmp::Ordering` import for amount comparison. If both imports use `Ordering`, alias the atomic one as shown above.

Add these types and functions above the test module:

```rust
#[derive(Debug, Clone)]
pub enum ResolvedNotifyChannel {
    Active(coin_listener_core::models::NotificationChannel),
    Unavailable(Uuid),
}

pub fn notifier_shutdown_requested(shutdown: &AtomicBool) -> bool {
    shutdown.load(AtomicOrdering::Relaxed)
}

pub async fn process_notify_task(pool: &PgPool, task: NotifyEventTask) -> AppResult<usize> {
    let event = notifications::get_address_event(pool, task.event_id).await?;
    if event.tenant_id != task.tenant_id {
        return Err(AppError::Validation(
            "notify task tenant does not match event".to_string(),
        ));
    }

    let rules = notifications::list_enabled_notification_rules(pool, task.tenant_id).await?;
    let mut deliveries = 0usize;

    for rule in rules {
        if !notification_rule_matches_event(&rule, &event) {
            continue;
        }

        let channels = resolve_rule_channels(pool, &rule).await?;
        for channel in channels {
            match channel {
                ResolvedNotifyChannel::Active(channel) => {
                    process_active_channel(pool, &event, rule.id, &channel).await?;
                    deliveries += 1;
                }
                ResolvedNotifyChannel::Unavailable(channel_id) => {
                    notifications::create_notification_delivery(
                        pool,
                        event.tenant_id,
                        event.id,
                        Some(rule.id),
                        Some(channel_id),
                        "skipped",
                        Some("channel unavailable".to_string()),
                        None,
                    )
                    .await?;
                    deliveries += 1;
                }
            }
        }
    }

    Ok(deliveries)
}

pub async fn run_notifier(
    pool: PgPool,
    mut redis: MultiplexedConnection,
    queue: NotifyQueue,
    shutdown: Arc<AtomicBool>,
) -> AppResult<()> {
    while !notifier_shutdown_requested(&shutdown) {
        match queue.dequeue(&mut redis, 5).await {
            Ok(Some(task)) => {
                let task_id = task.task_id;
                let event_id = task.event_id;
                match process_notify_task(&pool, task).await {
                    Ok(deliveries) => info!(
                        task_id = %task_id,
                        event_id = %event_id,
                        deliveries,
                        "notify task processed"
                    ),
                    Err(error) => warn!(
                        task_id = %task_id,
                        event_id = %event_id,
                        error = %error,
                        "notify task failed"
                    ),
                }
            }
            Ok(None) => {}
            Err(error) => warn!(error = %error, "discarded invalid or failed notify queue message"),
        }
    }

    Ok(())
}

async fn resolve_rule_channels(
    pool: &PgPool,
    rule: &coin_listener_core::models::NotificationRule,
) -> AppResult<Vec<ResolvedNotifyChannel>> {
    if rule.channel_ids.is_empty() {
        let channel = notifications::get_or_create_default_in_app_channel(pool, rule.tenant_id).await?;
        return Ok(vec![ResolvedNotifyChannel::Active(channel)]);
    }

    let active_channels = notifications::list_active_channels_by_ids(pool, rule.tenant_id, &rule.channel_ids).await?;
    let active_by_id: HashMap<Uuid, coin_listener_core::models::NotificationChannel> = active_channels
        .into_iter()
        .map(|channel| (channel.id, channel))
        .collect();

    Ok(rule
        .channel_ids
        .iter()
        .copied()
        .map(|channel_id| {
            active_by_id
                .get(&channel_id)
                .cloned()
                .map(ResolvedNotifyChannel::Active)
                .unwrap_or(ResolvedNotifyChannel::Unavailable(channel_id))
        })
        .collect())
}

async fn process_active_channel(
    pool: &PgPool,
    event: &coin_listener_core::models::AddressEvent,
    rule_id: Uuid,
    channel: &coin_listener_core::models::NotificationChannel,
) -> AppResult<()> {
    match notify_channel_decision(&channel.channel_type) {
        NotifyChannelDecision::InApp => {
            let now = Utc::now();
            let delivery = notifications::create_notification_delivery(
                pool,
                event.tenant_id,
                event.id,
                Some(rule_id),
                Some(channel.id),
                "sent",
                None,
                Some(now),
            )
            .await?;
            let (title, body) = build_in_app_notification_content(event);
            if let Err(error) = notifications::create_in_app_notification(
                pool,
                event.tenant_id,
                event.id,
                Some(delivery.id),
                title,
                body,
            )
            .await
            {
                notifications::update_notification_delivery_status(
                    pool,
                    delivery.id,
                    "failed",
                    Some(error.to_string()),
                    None,
                )
                .await?;
                warn!(delivery_id = %delivery.id, error = %error, "failed to create in-app notification");
            }
            Ok(())
        }
        NotifyChannelDecision::Skipped { last_error } => {
            notifications::create_notification_delivery(
                pool,
                event.tenant_id,
                event.id,
                Some(rule_id),
                Some(channel.id),
                "skipped",
                Some(last_error.to_string()),
                None,
            )
            .await?;
            Ok(())
        }
    }
}
```

- [ ] **Step 4: Replace notifier main stub**

Replace `backend/crates/notifier/src/main.rs` with:

```rust
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
    let queue = NotifyQueue::new(config.notify.queue_key.clone());

    info!(
        service = "notifier",
        queue_key = queue.queue_key(),
        "service started"
    );

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_signal = Arc::clone(&shutdown);
    tokio::spawn(async move {
        if signal::ctrl_c().await.is_ok() {
            shutdown_signal.store(true, Ordering::Relaxed);
        }
    });

    run_notifier(postgres, redis, queue, shutdown).await?;

    info!(service = "notifier", "service stopped");
    Ok(())
}
```

- [ ] **Step 5: Run tests to verify GREEN**

Run:

```bash
cargo test -p notifier --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 6: Checkpoint**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
cargo check -p notifier --manifest-path backend/Cargo.toml
```

Expected: both commands exit 0. If running inside git, commit with:

```bash
git add backend/crates/notifier/src/lib.rs backend/crates/notifier/src/main.rs
git commit -m "feat: process notify tasks"
```

## Task 7: Enqueue notify tasks from worker

**Files:**

- Modify: `backend/crates/worker/src/lib.rs`
- Modify: `backend/crates/worker/src/main.rs`
- Verify: `worker` tests and backend check

- [ ] **Step 1: Write failing notify task builder test**

Add this test module inside `#[cfg(test)] mod tests` in `backend/crates/worker/src/lib.rs`:

```rust
mod build_notify_event_task {
    use chrono::{TimeZone, Utc};
    use coin_listener_core::models::AddressEvent;
    use uuid::Uuid;

    use crate::build_notify_event_task;

    fn event() -> AddressEvent {
        AddressEvent {
            id: Uuid::from_u128(31),
            tenant_id: Uuid::from_u128(32),
            chain_id: Uuid::from_u128(33),
            address_id: Uuid::from_u128(34),
            asset_id: Uuid::from_u128(35),
            event_type: "transfer".to_string(),
            direction: "out".to_string(),
            is_transfer: true,
            tx_hash: Some("0xworker".to_string()),
            log_index: Some(1),
            block_number: Some(200),
            block_hash: None,
            confirmations: 12,
            from_address: None,
            to_address: None,
            amount_raw: Some("500".to_string()),
            amount_decimal: Some("0.0000000000000005".to_string()),
            balance_before_raw: None,
            balance_after_raw: None,
            balance_delta_raw: None,
            metadata: serde_json::json!({}),
            detected_at: Utc.with_ymd_and_hms(2026, 5, 17, 18, 0, 0).unwrap(),
            created_at: Utc.with_ymd_and_hms(2026, 5, 17, 18, 0, 1).unwrap(),
        }
    }

    #[test]
    fn uses_event_id_tenant_and_first_attempt() {
        let now = Utc.with_ymd_and_hms(2026, 5, 17, 18, 1, 0).unwrap();
        let event = event();

        let task = build_notify_event_task(&event, now);

        assert_eq!(task.event_id, event.id);
        assert_eq!(task.tenant_id, event.tenant_id);
        assert_eq!(task.attempt, 1);
        assert_eq!(task.enqueued_at, now);
    }
}
```

- [ ] **Step 2: Run test to verify RED**

Run:

```bash
cargo test -p worker build_notify_event_task --manifest-path backend/Cargo.toml
```

Expected: FAIL with missing `build_notify_event_task`.

- [ ] **Step 3: Add notify queue imports and task builder**

Update imports in `backend/crates/worker/src/lib.rs`:

```rust
use coin_listener_core::{
    models::{AddressEvent, NotifyEventTask, ScanAddressTask},
    AppResult,
};
use coin_listener_storage::{notify_queue::NotifyQueue, repositories, scan_queue::ScanQueue};
```

Add this function near `build_scan_task`-style helpers:

```rust
pub fn build_notify_event_task(event: &AddressEvent, now: DateTime<Utc>) -> NotifyEventTask {
    NotifyEventTask {
        task_id: Uuid::new_v4(),
        event_id: event.id,
        tenant_id: event.tenant_id,
        attempt: 1,
        enqueued_at: now,
    }
}
```

If `Uuid` is not already imported in `worker/src/lib.rs`, add:

```rust
use uuid::Uuid;
```

- [ ] **Step 4: Thread `NotifyQueue` through worker processing**

Change `process_scan_task` signature in `backend/crates/worker/src/lib.rs`:

```rust
pub async fn process_scan_task(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    scan_queue: &ScanQueue,
    notify_queue: &NotifyQueue,
    task: ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<ScanTaskOutcome> {
```

Within that function, replace `queue` references with `scan_queue`, and call locked processing with Redis and notify queue:

```rust
let acquired = scan_queue
    .acquire_lock(redis, task.address_id, task.task_id)
    .await?;
if !acquired {
    return Ok(ScanTaskOutcome::Locked);
}

let outcome = process_locked_scan_task(pool, redis, notify_queue, &task, now).await;
if let Err(error) = scan_queue
    .release_lock(redis, task.address_id, task.task_id)
    .await
{
    warn!(
        task_id = %task.task_id,
        address_id = %task.address_id,
        error = %error,
        "failed to release scan lock"
    );
}

outcome
```

Change `process_locked_scan_task` signature:

```rust
async fn process_locked_scan_task(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    notify_queue: &NotifyQueue,
    task: &ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<ScanTaskOutcome> {
```

Replace the EVM branch with this code so notify enqueue happens before scan timestamps advance:

```rust
ScanPlan::MockEvm => {
    let event = repositories::create_mock_evm_event(pool, task.address_id).await?;
    let notify_task = build_notify_event_task(&event, now);
    notify_queue.enqueue(redis, &notify_task).await?;
    repositories::finish_address_scan(pool, task.address_id, now, next_scan_at).await?;
    Ok(ScanTaskOutcome::Scanned)
}
```

- [ ] **Step 5: Update worker loop signature and call site**

Change `run_worker` signature in `backend/crates/worker/src/lib.rs`:

```rust
pub async fn run_worker(
    pool: PgPool,
    mut redis: MultiplexedConnection,
    scan_queue: ScanQueue,
    notify_queue: NotifyQueue,
    shutdown: Arc<AtomicBool>,
) -> AppResult<()> {
```

Within `run_worker`, replace `queue.dequeue` with `scan_queue.dequeue`, and update `process_scan_task` call:

```rust
match process_scan_task(
    &pool,
    &mut redis,
    &scan_queue,
    &notify_queue,
    task,
    Utc::now(),
)
.await
```

- [ ] **Step 6: Update worker main to construct notify queue**

Update imports in `backend/crates/worker/src/main.rs`:

```rust
use coin_listener_storage::{
    connect_postgres, connect_redis,
    notify_queue::NotifyQueue,
    run_migrations,
    scan_queue::{connect_scan_queue, ScanQueue},
};
```

After creating `scan_queue`, create `notify_queue`:

```rust
let scan_queue = ScanQueue::new(config.scan.queue_key.clone(), config.scan.lock_ttl_seconds);
let notify_queue = NotifyQueue::new(config.notify.queue_key.clone());
```

Update the startup log:

```rust
info!(
    service = "worker",
    scan_queue_key = scan_queue.queue_key(),
    notify_queue_key = notify_queue.queue_key(),
    lock_ttl_seconds = config.scan.lock_ttl_seconds,
    "service started"
);
```

Update the worker run call:

```rust
run_worker(postgres, redis, scan_queue, notify_queue, shutdown).await?;
```

- [ ] **Step 7: Run tests to verify GREEN**

Run:

```bash
cargo test -p worker build_notify_event_task --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 8: Checkpoint**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
cargo check -p worker --manifest-path backend/Cargo.toml
```

Expected: both commands exit 0. If running inside git, commit with:

```bash
git add backend/crates/worker/src/lib.rs backend/crates/worker/src/main.rs
git commit -m "feat: enqueue notify tasks from worker"
```

## Task 8: Add frontend notification API client

**Files:**

- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/api/client.ts`
- Verify: frontend TypeScript build in a later page task

- [ ] **Step 1: Add notification API types**

Append these types to `frontend/src/api/types.ts`:

```typescript
export type NotificationChannel = {
  id: string;
  tenant_id: string;
  channel_type: string;
  name: string;
  config: Record<string, unknown>;
  status: string;
  created_at: string;
  updated_at: string;
};

export type CreateNotificationChannelRequest = {
  channel_type: string;
  name: string;
  config?: Record<string, unknown>;
  status?: string;
};

export type NotificationRule = {
  id: string;
  tenant_id: string;
  name: string;
  chain_id?: string | null;
  address_id?: string | null;
  asset_id?: string | null;
  event_type?: string | null;
  is_transfer?: boolean | null;
  min_amount_raw?: string | null;
  direction?: string | null;
  channel_ids: string[];
  enabled: boolean;
  created_at: string;
  updated_at: string;
};

export type CreateNotificationRuleRequest = {
  name: string;
  chain_id?: string | null;
  address_id?: string | null;
  asset_id?: string | null;
  event_type?: string | null;
  is_transfer?: boolean | null;
  min_amount_raw?: string | null;
  direction?: string | null;
  channel_ids?: string[];
  enabled?: boolean;
};

export type InAppNotification = {
  id: string;
  tenant_id: string;
  event_id: string;
  delivery_id?: string | null;
  title: string;
  body: string;
  read_at?: string | null;
  created_at: string;
};

export type InAppNotificationQuery = {
  unread_only?: boolean;
};
```

- [ ] **Step 2: Import notification types in client**

Update `frontend/src/api/client.ts` type imports:

```typescript
import type {
  AddressEvent,
  Asset,
  Chain,
  CreateNotificationChannelRequest,
  CreateNotificationRuleRequest,
  CreateProviderRequest,
  CreateWatchedAddressRequest,
  EventQuery,
  InAppNotification,
  InAppNotificationQuery,
  LoginResponse,
  NotificationChannel,
  NotificationRule,
  Provider,
  WatchedAddress,
} from './types';
```

- [ ] **Step 3: Add notification API functions**

Append these functions to `frontend/src/api/client.ts`:

```typescript
export function listNotificationChannels(): Promise<NotificationChannel[]> {
  return request<NotificationChannel[]>('/api/notification-channels');
}

export function createNotificationChannel(payload: CreateNotificationChannelRequest): Promise<NotificationChannel> {
  return request<NotificationChannel>('/api/notification-channels', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export function listNotificationRules(): Promise<NotificationRule[]> {
  return request<NotificationRule[]>('/api/notification-rules');
}

export function createNotificationRule(payload: CreateNotificationRuleRequest): Promise<NotificationRule> {
  return request<NotificationRule>('/api/notification-rules', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export function updateNotificationRule(id: string, payload: CreateNotificationRuleRequest): Promise<NotificationRule> {
  return request<NotificationRule>(`/api/notification-rules/${id}`, {
    method: 'PUT',
    body: JSON.stringify(payload),
  });
}

export function deleteNotificationRule(id: string): Promise<void> {
  return request<void>(`/api/notification-rules/${id}`, {
    method: 'DELETE',
  });
}

export function listInAppNotifications(filters: InAppNotificationQuery = {}): Promise<InAppNotification[]> {
  const params = new URLSearchParams();
  if (filters.unread_only !== undefined) {
    params.set('unread_only', String(filters.unread_only));
  }

  const query = params.toString();
  return request<InAppNotification[]>(`/api/in-app-notifications${query ? `?${query}` : ''}`);
}

export function markInAppNotificationRead(id: string): Promise<InAppNotification> {
  return request<InAppNotification>(`/api/in-app-notifications/${id}/read`, {
    method: 'POST',
  });
}
```

- [ ] **Step 4: Checkpoint**

Run:

```bash
npm run build --prefix frontend
```

Expected: PASS. Existing Vite dependency warnings may print, but the command must exit 0. If running inside git, commit with:

```bash
git add frontend/src/api/types.ts frontend/src/api/client.ts
git commit -m "feat: add notification frontend client"
```

## Task 9: Add notification rules page

**Files:**

- Create: `frontend/src/pages/NotificationRulesPage.tsx`
- Verify: frontend build

- [ ] **Step 1: Create notification rules page component**

Create `frontend/src/pages/NotificationRulesPage.tsx` with:

```typescript
import { useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Banner, Button, Card, Form, Modal, Space, Table, Tag, Toast } from '@douyinfe/semi-ui';
import {
  createNotificationRule,
  deleteNotificationRule,
  listAssets,
  listChains,
  listNotificationChannels,
  listNotificationRules,
  listWatchedAddresses,
  updateNotificationRule,
} from '../api/client';
import type { CreateNotificationRuleRequest, NotificationRule } from '../api/types';

const eventTypeOptions = [
  { label: 'transfer', value: 'transfer' },
  { label: 'balance_change', value: 'balance_change' },
  { label: 'fee_only_change', value: 'fee_only_change' },
  { label: 'contract_interaction', value: 'contract_interaction' },
  { label: 'unknown', value: 'unknown' },
];

const directionOptions = [
  { label: 'in', value: 'in' },
  { label: 'out', value: 'out' },
  { label: 'self', value: 'self' },
  { label: 'unknown', value: 'unknown' },
];

type RuleForm = {
  name: string;
  chain_id?: string;
  address_id?: string;
  asset_id?: string;
  event_type?: string;
  direction?: string;
  is_transfer?: string;
  min_amount_raw?: string;
  channel_ids?: string[];
  enabled?: boolean;
};

export function NotificationRulesPage() {
  const [editingRule, setEditingRule] = useState<NotificationRule | null>(null);
  const [modalVisible, setModalVisible] = useState(false);
  const queryClient = useQueryClient();

  const rulesQuery = useQuery({ queryKey: ['notification-rules'], queryFn: listNotificationRules });
  const channelsQuery = useQuery({ queryKey: ['notification-channels'], queryFn: listNotificationChannels });
  const chainsQuery = useQuery({ queryKey: ['chains'], queryFn: listChains });
  const assetsQuery = useQuery({ queryKey: ['assets'], queryFn: listAssets });
  const addressesQuery = useQuery({ queryKey: ['addresses'], queryFn: listWatchedAddresses });

  const chainMap = useMemo(() => new Map((chainsQuery.data ?? []).map(chain => [chain.id, chain.name])), [chainsQuery.data]);
  const assetMap = useMemo(() => new Map((assetsQuery.data ?? []).map(asset => [asset.id, asset.symbol])), [assetsQuery.data]);
  const addressMap = useMemo(() => new Map((addressesQuery.data ?? []).map(address => [address.id, address])), [addressesQuery.data]);
  const channelMap = useMemo(() => new Map((channelsQuery.data ?? []).map(channel => [channel.id, channel.name])), [channelsQuery.data]);

  const saveMutation = useMutation({
    mutationFn: (payload: CreateNotificationRuleRequest) => (
      editingRule ? updateNotificationRule(editingRule.id, payload) : createNotificationRule(payload)
    ),
    onSuccess: () => {
      Toast.success(editingRule ? '通知规则已更新' : '通知规则已创建');
      setModalVisible(false);
      setEditingRule(null);
      queryClient.invalidateQueries({ queryKey: ['notification-rules'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '通知规则保存失败'),
  });

  const deleteMutation = useMutation({
    mutationFn: deleteNotificationRule,
    onSuccess: () => {
      Toast.success('通知规则已删除');
      queryClient.invalidateQueries({ queryKey: ['notification-rules'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '通知规则删除失败'),
  });

  function openCreateModal() {
    setEditingRule(null);
    setModalVisible(true);
  }

  function openEditModal(rule: NotificationRule) {
    setEditingRule(rule);
    setModalVisible(true);
  }

  function handleSubmit(values: Record<string, unknown>) {
    const form = values as RuleForm;
    saveMutation.mutate({
      name: form.name,
      chain_id: form.chain_id || null,
      address_id: form.address_id || null,
      asset_id: form.asset_id || null,
      event_type: form.event_type || null,
      direction: form.direction || null,
      is_transfer: form.is_transfer === undefined ? null : form.is_transfer === 'true',
      min_amount_raw: form.min_amount_raw || null,
      channel_ids: form.channel_ids ?? [],
      enabled: form.enabled ?? true,
    });
  }

  function initialValues(): RuleForm {
    if (!editingRule) {
      return { enabled: true, channel_ids: [] };
    }
    return {
      name: editingRule.name,
      chain_id: editingRule.chain_id ?? undefined,
      address_id: editingRule.address_id ?? undefined,
      asset_id: editingRule.asset_id ?? undefined,
      event_type: editingRule.event_type ?? undefined,
      direction: editingRule.direction ?? undefined,
      is_transfer: editingRule.is_transfer === null || editingRule.is_transfer === undefined ? undefined : String(editingRule.is_transfer),
      min_amount_raw: editingRule.min_amount_raw ?? undefined,
      channel_ids: editingRule.channel_ids,
      enabled: editingRule.enabled,
    };
  }

  function renderAddress(addressId?: string | null) {
    if (!addressId) return '-';
    const address = addressMap.get(addressId);
    if (!address) return addressId;
    return address.label ? `${address.label} / ${address.address}` : address.address;
  }

  return (
    <Space vertical align="start" spacing={16} className="content-stack">
      {rulesQuery.isError ? (
        <Banner
          type="danger"
          title="通知规则加载失败"
          description={rulesQuery.error instanceof Error ? rulesQuery.error.message : '请求失败'}
        />
      ) : null}

      <Card
        title="通知规则"
        headerExtraContent={<Button type="primary" onClick={openCreateModal}>创建规则</Button>}
      >
        <Table<NotificationRule>
          loading={rulesQuery.isLoading}
          dataSource={rulesQuery.data ?? []}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          scroll={{ x: 1300 }}
          columns={[
            { title: '名称', dataIndex: 'name', width: 180 },
            { title: '启用', dataIndex: 'enabled', width: 80, render: value => <Tag color={value ? 'green' : 'grey'}>{value ? '启用' : '停用'}</Tag> },
            { title: '链', dataIndex: 'chain_id', width: 140, render: value => value ? chainMap.get(String(value)) ?? String(value) : '-' },
            { title: '地址', dataIndex: 'address_id', width: 260, render: value => renderAddress(value ? String(value) : null) },
            { title: '资产', dataIndex: 'asset_id', width: 120, render: value => value ? assetMap.get(String(value)) ?? String(value) : '-' },
            { title: '事件类型', dataIndex: 'event_type', width: 150, render: value => value ? <Tag>{String(value)}</Tag> : '-' },
            { title: '方向', dataIndex: 'direction', width: 90, render: value => value ? String(value) : '-' },
            { title: '最小金额 raw', dataIndex: 'min_amount_raw', width: 150, render: value => value ? String(value) : '-' },
            {
              title: '渠道',
              dataIndex: 'channel_ids',
              width: 220,
              render: value => {
                const channelIds = Array.isArray(value) ? value as string[] : [];
                if (channelIds.length === 0) return <Tag color="blue">默认站内</Tag>;
                return channelIds.map(id => <Tag key={id}>{channelMap.get(id) ?? id}</Tag>);
              },
            },
            {
              title: '操作',
              width: 150,
              fixed: 'right',
              render: (_, rule) => (
                <Space>
                  <Button size="small" onClick={() => openEditModal(rule)}>编辑</Button>
                  <Button size="small" type="danger" loading={deleteMutation.isPending} onClick={() => deleteMutation.mutate(rule.id)}>删除</Button>
                </Space>
              ),
            },
          ]}
        />
      </Card>

      <Modal
        title={editingRule ? '编辑通知规则' : '创建通知规则'}
        visible={modalVisible}
        onCancel={() => {
          setModalVisible(false);
          setEditingRule(null);
        }}
        footer={null}
      >
        <Form<RuleForm> initValues={initialValues()} onSubmit={handleSubmit} labelPosition="left" labelWidth={110}>
          <Form.Input field="name" label="名称" rules={[{ required: true, message: '请输入规则名称' }]} />
          <Form.Select field="chain_id" label="链" showClear placeholder="不过滤链" filter>
            {(chainsQuery.data ?? []).map(chain => <Form.Select.Option key={chain.id} value={chain.id}>{chain.name}</Form.Select.Option>)}
          </Form.Select>
          <Form.Select field="address_id" label="地址" showClear placeholder="不过滤地址" filter>
            {(addressesQuery.data ?? []).map(address => (
              <Form.Select.Option key={address.id} value={address.id}>{address.label ? `${address.label} / ${address.address}` : address.address}</Form.Select.Option>
            ))}
          </Form.Select>
          <Form.Select field="asset_id" label="资产" showClear placeholder="不过滤资产" filter>
            {(assetsQuery.data ?? []).map(asset => <Form.Select.Option key={asset.id} value={asset.id}>{asset.symbol}</Form.Select.Option>)}
          </Form.Select>
          <Form.Select field="event_type" label="事件类型" showClear placeholder="不过滤类型" optionList={eventTypeOptions} />
          <Form.Select field="direction" label="方向" showClear placeholder="不过滤方向" optionList={directionOptions} />
          <Form.Select field="is_transfer" label="是否转账" showClear placeholder="不过滤">
            <Form.Select.Option value="true">是</Form.Select.Option>
            <Form.Select.Option value="false">否</Form.Select.Option>
          </Form.Select>
          <Form.Input field="min_amount_raw" label="最小金额 raw" placeholder="留空表示不过滤金额" />
          <Form.Select field="channel_ids" label="渠道" multiple showClear placeholder="留空使用默认站内渠道" filter>
            {(channelsQuery.data ?? []).map(channel => <Form.Select.Option key={channel.id} value={channel.id}>{channel.name} / {channel.channel_type}</Form.Select.Option>)}
          </Form.Select>
          <Form.Switch field="enabled" label="启用" />
          <Space>
            <Button htmlType="submit" type="primary" loading={saveMutation.isPending}>保存</Button>
            <Button onClick={() => setModalVisible(false)}>取消</Button>
          </Space>
        </Form>
      </Modal>
    </Space>
  );
}
```

- [ ] **Step 2: Run frontend build**

Run:

```bash
npm run build --prefix frontend
```

Expected: PASS. Existing Vite dependency warnings may print, but the command must exit 0.

- [ ] **Step 3: Checkpoint**

If running inside git, commit with:

```bash
git add frontend/src/pages/NotificationRulesPage.tsx
git commit -m "feat: add notification rules page"
```

## Task 10: Add in-app notifications page and navigation

**Files:**

- Create: `frontend/src/pages/InAppNotificationsPage.tsx`
- Modify: `frontend/src/App.tsx`
- Verify: frontend build

- [ ] **Step 1: Create in-app notifications page**

Create `frontend/src/pages/InAppNotificationsPage.tsx` with:

```typescript
import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Banner, Button, Card, Space, Switch, Table, Tag, Toast } from '@douyinfe/semi-ui';
import { listInAppNotifications, markInAppNotificationRead } from '../api/client';
import type { InAppNotification } from '../api/types';

export function InAppNotificationsPage() {
  const [unreadOnly, setUnreadOnly] = useState(false);
  const queryClient = useQueryClient();

  const notificationsQuery = useQuery({
    queryKey: ['in-app-notifications', unreadOnly],
    queryFn: () => listInAppNotifications({ unread_only: unreadOnly || undefined }),
  });

  const markReadMutation = useMutation({
    mutationFn: markInAppNotificationRead,
    onSuccess: () => {
      Toast.success('已标记为已读');
      queryClient.invalidateQueries({ queryKey: ['in-app-notifications'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '标记已读失败'),
  });

  return (
    <Space vertical align="start" spacing={16} className="content-stack">
      {notificationsQuery.isError ? (
        <Banner
          type="danger"
          title="站内通知加载失败"
          description={notificationsQuery.error instanceof Error ? notificationsQuery.error.message : '请求失败'}
        />
      ) : null}

      <Card title="站内通知筛选" className="filter-card">
        <Space>
          <Switch checked={unreadOnly} onChange={checked => setUnreadOnly(Boolean(checked))} />
          <span>只看未读</span>
          <Button onClick={() => notificationsQuery.refetch()}>刷新</Button>
        </Space>
      </Card>

      <Card title="站内通知">
        <Table<InAppNotification>
          loading={notificationsQuery.isLoading}
          dataSource={notificationsQuery.data ?? []}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          scroll={{ x: 1000 }}
          columns={[
            { title: '时间', dataIndex: 'created_at', width: 180, render: value => new Date(String(value)).toLocaleString() },
            { title: '标题', dataIndex: 'title', width: 180 },
            { title: '内容', dataIndex: 'body', width: 420, ellipsis: { showTitle: true } },
            {
              title: '状态',
              dataIndex: 'read_at',
              width: 100,
              render: value => value ? <Tag color="grey">已读</Tag> : <Tag color="red">未读</Tag>,
            },
            {
              title: '操作',
              width: 120,
              render: (_, notification) => (
                <Button
                  size="small"
                  disabled={Boolean(notification.read_at)}
                  loading={markReadMutation.isPending}
                  onClick={() => markReadMutation.mutate(notification.id)}
                >
                  标记已读
                </Button>
              ),
            },
          ]}
        />
      </Card>
    </Space>
  );
}
```

- [ ] **Step 2: Add pages to app navigation**

Update imports in `frontend/src/App.tsx`:

```typescript
import { InAppNotificationsPage } from './pages/InAppNotificationsPage';
import { NotificationRulesPage } from './pages/NotificationRulesPage';
```

Change `PageKey`:

```typescript
type PageKey =
  | 'dashboard'
  | 'chains'
  | 'assets'
  | 'providers'
  | 'addresses'
  | 'events'
  | 'notification-rules'
  | 'in-app-notifications';
```

Add navigation items after the events item:

```typescript
{ itemKey: 'notification-rules', text: '通知规则', icon: <IconBell /> },
{ itemKey: 'in-app-notifications', text: '站内通知', icon: <IconBell /> },
```

Add render branches in `renderPage` after the events branch:

```typescript
if (page === 'notification-rules') return <NotificationRulesPage />;
if (page === 'in-app-notifications') return <InAppNotificationsPage />;
```

Update the dashboard banner description:

```typescript
description="当前版本提供登录、链配置、资产配置、Provider 配置、监听地址管理、事件中心、通知规则和站内通知。"
```

- [ ] **Step 3: Run frontend build**

Run:

```bash
npm run build --prefix frontend
```

Expected: PASS. Existing Vite dependency warnings may print, but the command must exit 0.

- [ ] **Step 4: Checkpoint**

If running inside git, commit with:

```bash
git add frontend/src/pages/InAppNotificationsPage.tsx frontend/src/App.tsx
git commit -m "feat: add in-app notifications page"
```

## Task 11: Final verification

**Files:**

- Verify all files changed by Tasks 1-10

- [ ] **Step 1: Run backend formatting**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: exit 0.

- [ ] **Step 2: Run backend workspace check**

Run:

```bash
cargo check --workspace --manifest-path backend/Cargo.toml
```

Expected: exit 0. Existing dependency warnings from `sqlx-postgres` may print, but the command must exit 0.

- [ ] **Step 3: Run backend tests**

Run:

```bash
cargo test --workspace --manifest-path backend/Cargo.toml
```

Expected: exit 0 with all tests passing.

- [ ] **Step 4: Run frontend build**

Run:

```bash
npm run build --prefix frontend
```

Expected: exit 0. Existing Vite dependency warnings may print, but the command must exit 0.

- [ ] **Step 5: Validate Docker Compose config**

Run:

```bash
docker compose -f docker-compose.yml config
```

Expected: exit 0 and rendered compose configuration includes the existing `notifier` service.

- [ ] **Step 6: Optional local integration smoke when Docker daemon is available**

Run:

```bash
docker compose up -d postgres redis
DATABASE_URL=postgres://coin_listener:coin_listener_password@localhost:5432/coin_listener REDIS_URL=redis://localhost:6379 ENABLE_DEV_ROUTES=true cargo run --manifest-path backend/Cargo.toml -p api-server
```

In another terminal, run:

```bash
DATABASE_URL=postgres://coin_listener:coin_listener_password@localhost:5432/coin_listener REDIS_URL=redis://localhost:6379 cargo run --manifest-path backend/Cargo.toml -p notifier
```

Expected: API and notifier start without migration or Redis connection errors. Then create an `in_app` channel and notification rule through the API or frontend, trigger `/api/dev/scan-address/:id`, and confirm `GET /api/in-app-notifications` returns at least one notification for a matching rule.

## Self-review notes

Spec coverage:

- Tables and indexes: Task 1.
- `NotifyEventTask`: Task 1.
- Redis notify queue: Task 2.
- Notification repository: Task 3.
- API endpoints: Task 4.
- Rule matching, amount thresholds, in-app content, unsupported-channel decision: Task 5.
- Notifier consume loop and delivery creation: Task 6.
- Worker enqueue integration: Task 7.
- Frontend API client, notification rules page, in-app notifications page: Tasks 8-10.
- Final verification commands: Task 11.

Type consistency:

- Rust queue message name is `NotifyEventTask` in `core`, `storage::notify_queue`, `worker`, and `notifier`.
- Redis wrapper name is `NotifyQueue` with `enqueue` and `dequeue`, matching `ScanQueue` style.
- API and frontend use snake_case fields matching Rust serde defaults.
- Channel skip error is exactly `channel type not implemented`.
- Default queue key is exactly `notify:event:queue`.
