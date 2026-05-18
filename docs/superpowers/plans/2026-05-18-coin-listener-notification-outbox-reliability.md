# Coin Listener Notification Outbox Reliability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Redis notify queue as the reliable notification task source with a PostgreSQL notification outbox that is written atomically with new `address_events` rows.

**Architecture:** The worker writes `address_events` and `notification_outbox` rows in one PostgreSQL transaction, then finishes scans without enqueueing Redis notification messages. The notifier claims due outbox rows with `FOR UPDATE SKIP LOCKED`, processes existing notification rule/channel logic, marks successful rows delivered, retries failures with deterministic backoff, and releases stale processing rows after crashes.

**Tech Stack:** Rust 2021, SQLx, PostgreSQL migrations, Tokio, chrono, uuid, Redis scan queue compatibility, existing Coin Listener storage/notifier/worker crates.

---

## Scope and Constraints

Implement only the approved notification reliable delivery scope from `docs/superpowers/specs/2026-05-18-coin-listener-notification-outbox-reliability-design.md`.

Do not implement Telegram/Webhook/Email sending, frontend outbox management pages, historical backfill, Kafka/RabbitMQ, metrics dashboards, or external-channel exactly-once semantics.

Keep `backend/crates/storage/src/notify_queue.rs` available for compatibility, but do not use Redis notify enqueue/dequeue as the primary path for new worker-created notification tasks.

---

## File Structure

Create:

```text
backend/crates/storage/migrations/0007_notification_outbox.sql
```

Modify:

```text
backend/crates/core/src/models.rs
backend/crates/core/src/config.rs
backend/crates/storage/src/repositories.rs
backend/crates/worker/src/lib.rs
backend/crates/worker/src/main.rs
backend/crates/notifier/src/lib.rs
backend/crates/notifier/src/main.rs
```

Responsibilities:

- `backend/crates/core/src/models.rs`: shared `NotificationOutboxItem` model for SQLx rows and notifier processing.
- `backend/crates/core/src/config.rs`: notification outbox batch/retry/stale/idle runtime settings from environment variables.
- `backend/crates/storage/migrations/0007_notification_outbox.sql`: durable outbox table and indexes.
- `backend/crates/storage/src/repositories.rs`: transactional event+outbox insert helper plus outbox claim/mark/retry/fail/stale-release helpers.
- `backend/crates/worker/src/lib.rs`: replace event-only inserts with event+outbox inserts; remove Redis notify enqueue from scan completion.
- `backend/crates/worker/src/main.rs`: stop constructing/logging `NotifyQueue` for worker notification delivery.
- `backend/crates/notifier/src/lib.rs`: add outbox item processing, retry policy, stale recovery, and DB dispatcher loop.
- `backend/crates/notifier/src/main.rs`: run the DB outbox dispatcher from `NotifyConfig` without Redis notify queue consumption.

---

### Task 1: Add core outbox model and notify config

**Files:**
- Modify: `backend/crates/core/src/models.rs:256-420`
- Modify: `backend/crates/core/src/config.rs:41-78`
- Test: `backend/crates/core/src/models.rs:411-560`
- Test: `backend/crates/core/src/config.rs` new `#[cfg(test)]` module

- [ ] **Step 1: Write failing core model/config tests**

In `backend/crates/core/src/models.rs`, update the test import block from:

```rust
use super::{
    CreateBalanceSnapshotRequest, EventStatus, NotificationStatus, NotifyEventTask,
    ProviderChainStatus, ProviderStatus, ProviderStatusItem, QueueStatus, ScanAddressTask,
    ScanCursor, ScanStatus, SystemStatus,
};
```

to:

```rust
use super::{
    CreateBalanceSnapshotRequest, EventStatus, NotificationOutboxItem, NotificationStatus,
    NotifyEventTask, ProviderChainStatus, ProviderStatus, ProviderStatusItem, QueueStatus,
    ScanAddressTask, ScanCursor, ScanStatus, SystemStatus,
};
```

Add this test after `queue_status_allows_missing_depths_with_errors`:

```rust
#[test]
fn notification_outbox_item_round_trips_as_json() {
    let item = NotificationOutboxItem {
        id: Uuid::from_u128(1),
        tenant_id: Uuid::from_u128(2),
        event_id: Uuid::from_u128(3),
        status: "processing".to_string(),
        attempt_count: 2,
        next_attempt_at: Utc.with_ymd_and_hms(2026, 5, 18, 12, 0, 0).unwrap(),
        locked_at: Some(Utc.with_ymd_and_hms(2026, 5, 18, 12, 1, 0).unwrap()),
        locked_by: Some("notifier-test".to_string()),
        last_error: Some("temporary failure".to_string()),
        delivered_at: None,
        created_at: Utc.with_ymd_and_hms(2026, 5, 18, 11, 59, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2026, 5, 18, 12, 1, 0).unwrap(),
    };

    let payload = serde_json::to_string(&item).expect("serialize notification outbox item");
    let decoded: NotificationOutboxItem =
        serde_json::from_str(&payload).expect("deserialize notification outbox item");

    assert_eq!(decoded.id, item.id);
    assert_eq!(decoded.tenant_id, item.tenant_id);
    assert_eq!(decoded.event_id, item.event_id);
    assert_eq!(decoded.status, "processing");
    assert_eq!(decoded.attempt_count, 2);
    assert_eq!(decoded.locked_by.as_deref(), Some("notifier-test"));
    assert!(payload.contains("\"last_error\":\"temporary failure\""));
}
```

In `backend/crates/core/src/config.rs`, add this test module at the end of the file:

```rust
#[cfg(test)]
mod tests {
    use crate::config::NotifyConfig;

    #[test]
    fn notify_config_carries_outbox_runtime_settings() {
        let config = NotifyConfig {
            queue_key: "notify:event:queue".to_string(),
            outbox_batch_size: 50,
            outbox_max_attempts: 10,
            outbox_stale_lock_seconds: 300,
            outbox_idle_sleep_ms: 500,
        };

        assert_eq!(config.queue_key, "notify:event:queue");
        assert_eq!(config.outbox_batch_size, 50);
        assert_eq!(config.outbox_max_attempts, 10);
        assert_eq!(config.outbox_stale_lock_seconds, 300);
        assert_eq!(config.outbox_idle_sleep_ms, 500);
    }
}
```

- [ ] **Step 2: Run core tests to verify RED**

Run:

```bash
cargo test -p coin-listener-core notification_outbox_item_round_trips_as_json --manifest-path backend/Cargo.toml
cargo test -p coin-listener-core notify_config_carries_outbox_runtime_settings --manifest-path backend/Cargo.toml
```

Expected: FAIL because `NotificationOutboxItem` and the new `NotifyConfig` fields do not exist.

- [ ] **Step 3: Add the `NotificationOutboxItem` model**

In `backend/crates/core/src/models.rs`, add this struct after `InAppNotification` and before `InAppNotificationQuery`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationOutboxItem {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub event_id: Uuid,
    pub status: String,
    pub attempt_count: i32,
    pub next_attempt_at: DateTime<Utc>,
    pub locked_at: Option<DateTime<Utc>>,
    pub locked_by: Option<String>,
    pub last_error: Option<String>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

- [ ] **Step 4: Extend `NotifyConfig` and environment loading**

In `backend/crates/core/src/config.rs`, replace `NotifyConfig` with:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct NotifyConfig {
    pub queue_key: String,
    pub outbox_batch_size: i64,
    pub outbox_max_attempts: i32,
    pub outbox_stale_lock_seconds: i64,
    pub outbox_idle_sleep_ms: u64,
}
```

In `AppConfig::from_env`, after the existing `notify.queue_key` merge block, add these merge blocks:

```rust
.merge((
    "notify.outbox_batch_size",
    env::var("NOTIFICATION_OUTBOX_BATCH_SIZE").unwrap_or_else(|_| "50".to_string()),
))
.merge((
    "notify.outbox_max_attempts",
    env::var("NOTIFICATION_OUTBOX_MAX_ATTEMPTS").unwrap_or_else(|_| "10".to_string()),
))
.merge((
    "notify.outbox_stale_lock_seconds",
    env::var("NOTIFICATION_OUTBOX_STALE_LOCK_SECONDS").unwrap_or_else(|_| "300".to_string()),
))
.merge((
    "notify.outbox_idle_sleep_ms",
    env::var("NOTIFICATION_OUTBOX_IDLE_SLEEP_MS").unwrap_or_else(|_| "500".to_string()),
))
```

The final part of the merge chain must keep `.extract().map_err(...)` unchanged.

- [ ] **Step 5: Run core tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-core notification_outbox_item_round_trips_as_json --manifest-path backend/Cargo.toml
cargo test -p coin-listener-core notify_config_carries_outbox_runtime_settings --manifest-path backend/Cargo.toml
cargo check -p coin-listener-core --manifest-path backend/Cargo.toml
cargo fmt -p coin-listener-core --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 6: Commit Task 1**

Run:

```bash
git status --short
git add backend/crates/core/src/models.rs backend/crates/core/src/config.rs
git commit -m "Add notification outbox core model and config"
```

Expected: commit succeeds and contains only the core model/config changes.

---

### Task 2: Add outbox migration and storage SQL constants

**Files:**
- Create: `backend/crates/storage/migrations/0007_notification_outbox.sql`
- Modify: `backend/crates/storage/src/repositories.rs:1-130`
- Test: `backend/crates/storage/src/repositories.rs:760-900`

- [ ] **Step 1: Write failing migration/query tests**

In `backend/crates/storage/src/repositories.rs`, extend the test import list from:

```rust
use super::{
    next_scan_at_from, ACTIVE_ASSETS_BY_TYPE_QUERY, ACTIVE_ERC20_ASSETS_QUERY,
    ACTIVE_RPC_PROVIDER_QUERY, CLAIM_ONE_DUE_SCAN_ADDRESS_QUERY, INSERT_BALANCE_SNAPSHOT_QUERY,
    INSERT_EVENT_IF_NOT_EXISTS_QUERY, LATEST_BALANCE_SNAPSHOT_QUERY,
    MARK_CLAIMED_SCAN_ENQUEUED_QUERY, SCAN_CURSOR_QUERY, UPSERT_SCAN_CURSOR_QUERY,
};
```

to:

```rust
use super::{
    next_scan_at_from, ACTIVE_ASSETS_BY_TYPE_QUERY, ACTIVE_ERC20_ASSETS_QUERY,
    ACTIVE_RPC_PROVIDER_QUERY, CLAIM_DUE_NOTIFICATION_OUTBOX_QUERY,
    CLAIM_ONE_DUE_SCAN_ADDRESS_QUERY, INSERT_BALANCE_SNAPSHOT_QUERY,
    INSERT_EVENT_IF_NOT_EXISTS_QUERY, INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY,
    LATEST_BALANCE_SNAPSHOT_QUERY, MARK_CLAIMED_SCAN_ENQUEUED_QUERY,
    MARK_NOTIFICATION_OUTBOX_DELIVERED_QUERY, MARK_NOTIFICATION_OUTBOX_FAILED_QUERY,
    MARK_NOTIFICATION_OUTBOX_RETRYABLE_QUERY, RELEASE_STALE_NOTIFICATION_OUTBOX_QUERY,
    SCAN_CURSOR_QUERY, UPSERT_SCAN_CURSOR_QUERY,
};
```

Add these tests after `insert_event_if_not_exists_query_returns_optional_event`:

```rust
#[test]
fn notification_outbox_migration_defines_reliable_task_table() {
    let migration = include_str!("../migrations/0007_notification_outbox.sql");

    assert!(migration.contains("CREATE TABLE IF NOT EXISTS notification_outbox"));
    assert!(migration.contains("tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE"));
    assert!(migration.contains("event_id UUID NOT NULL REFERENCES address_events(id) ON DELETE CASCADE"));
    assert!(migration.contains("UNIQUE(event_id)"));
    assert!(migration.contains("idx_notification_outbox_claim"));
    assert!(migration.contains("WHERE status IN ('pending', 'retryable')"));
    assert!(migration.contains("idx_notification_outbox_processing_stale"));
    assert!(migration.contains("WHERE status = 'processing'"));
}

#[test]
fn notification_outbox_insert_query_links_new_event() {
    assert!(INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY.contains("notification_outbox"));
    assert!(INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY.contains("tenant_id"));
    assert!(INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY.contains("event_id"));
    assert!(INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY.contains("'pending'"));
    assert!(INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY.contains("ON CONFLICT (event_id) DO NOTHING"));
}

#[test]
fn notification_outbox_claim_query_uses_skip_locked_and_increments_attempt() {
    assert!(CLAIM_DUE_NOTIFICATION_OUTBOX_QUERY.contains("FOR UPDATE SKIP LOCKED"));
    assert!(CLAIM_DUE_NOTIFICATION_OUTBOX_QUERY.contains("status IN ('pending', 'retryable')"));
    assert!(CLAIM_DUE_NOTIFICATION_OUTBOX_QUERY.contains("next_attempt_at <= $1"));
    assert!(CLAIM_DUE_NOTIFICATION_OUTBOX_QUERY.contains("attempt_count = attempt_count + 1"));
    assert!(CLAIM_DUE_NOTIFICATION_OUTBOX_QUERY.contains("locked_by = $3"));
}

#[test]
fn notification_outbox_mark_queries_require_processing_status() {
    for query in [
        MARK_NOTIFICATION_OUTBOX_DELIVERED_QUERY,
        MARK_NOTIFICATION_OUTBOX_RETRYABLE_QUERY,
        MARK_NOTIFICATION_OUTBOX_FAILED_QUERY,
    ] {
        assert!(query.contains("WHERE id = $1"));
        assert!(query.contains("status = 'processing'"));
    }
}

#[test]
fn notification_outbox_stale_release_only_matches_stale_processing_rows() {
    assert!(RELEASE_STALE_NOTIFICATION_OUTBOX_QUERY.contains("status = 'processing'"));
    assert!(RELEASE_STALE_NOTIFICATION_OUTBOX_QUERY.contains("locked_at < $1"));
    assert!(RELEASE_STALE_NOTIFICATION_OUTBOX_QUERY.contains("status = 'retryable'"));
    assert!(RELEASE_STALE_NOTIFICATION_OUTBOX_QUERY.contains("locked_by = NULL"));
}
```

- [ ] **Step 2: Run storage query tests to verify RED**

Run:

```bash
cargo test -p coin-listener-storage notification_outbox_migration_defines_reliable_task_table --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage notification_outbox_claim_query_uses_skip_locked_and_increments_attempt --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage notification_outbox_mark_queries_require_processing_status --manifest-path backend/Cargo.toml
```

Expected: FAIL because the migration file and outbox SQL constants do not exist.

- [ ] **Step 3: Create the notification outbox migration**

Create `backend/crates/storage/migrations/0007_notification_outbox.sql` with exactly:

```sql
CREATE TABLE IF NOT EXISTS notification_outbox (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    event_id UUID NOT NULL REFERENCES address_events(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'pending',
    attempt_count INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    locked_at TIMESTAMPTZ,
    locked_by TEXT,
    last_error TEXT,
    delivered_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(event_id)
);

CREATE INDEX IF NOT EXISTS idx_notification_outbox_claim
    ON notification_outbox(status, next_attempt_at, created_at)
    WHERE status IN ('pending', 'retryable');

CREATE INDEX IF NOT EXISTS idx_notification_outbox_processing_stale
    ON notification_outbox(status, locked_at)
    WHERE status = 'processing';

CREATE INDEX IF NOT EXISTS idx_notification_outbox_event
    ON notification_outbox(event_id);
```

- [ ] **Step 4: Add storage SQL constants**

In `backend/crates/storage/src/repositories.rs`, add these constants after `INSERT_EVENT_IF_NOT_EXISTS_QUERY`:

```rust
pub const INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY: &str = r#"
INSERT INTO notification_outbox (tenant_id, event_id, status)
VALUES ($1, $2, 'pending')
ON CONFLICT (event_id) DO NOTHING
"#;

pub const CLAIM_DUE_NOTIFICATION_OUTBOX_QUERY: &str = r#"
WITH due AS (
    SELECT id
    FROM notification_outbox
    WHERE status IN ('pending', 'retryable')
      AND next_attempt_at <= $1
    ORDER BY next_attempt_at ASC, created_at ASC
    LIMIT $2
    FOR UPDATE SKIP LOCKED
)
UPDATE notification_outbox o
SET status = 'processing',
    locked_at = $1,
    locked_by = $3,
    attempt_count = attempt_count + 1,
    updated_at = NOW()
FROM due
WHERE o.id = due.id
RETURNING o.id, o.tenant_id, o.event_id, o.status, o.attempt_count,
          o.next_attempt_at, o.locked_at, o.locked_by, o.last_error,
          o.delivered_at, o.created_at, o.updated_at
"#;

pub const MARK_NOTIFICATION_OUTBOX_DELIVERED_QUERY: &str = r#"
UPDATE notification_outbox
SET status = 'delivered',
    delivered_at = $2,
    locked_at = NULL,
    locked_by = NULL,
    last_error = NULL,
    updated_at = NOW()
WHERE id = $1
  AND status = 'processing'
"#;

pub const MARK_NOTIFICATION_OUTBOX_RETRYABLE_QUERY: &str = r#"
UPDATE notification_outbox
SET status = 'retryable',
    next_attempt_at = $2,
    locked_at = NULL,
    locked_by = NULL,
    last_error = $3,
    updated_at = NOW()
WHERE id = $1
  AND status = 'processing'
"#;

pub const MARK_NOTIFICATION_OUTBOX_FAILED_QUERY: &str = r#"
UPDATE notification_outbox
SET status = 'failed',
    locked_at = NULL,
    locked_by = NULL,
    last_error = $2,
    updated_at = NOW()
WHERE id = $1
  AND status = 'processing'
"#;

pub const RELEASE_STALE_NOTIFICATION_OUTBOX_QUERY: &str = r#"
UPDATE notification_outbox
SET status = 'retryable',
    next_attempt_at = $2,
    locked_at = NULL,
    locked_by = NULL,
    updated_at = NOW()
WHERE status = 'processing'
  AND locked_at < $1
"#;
```

- [ ] **Step 5: Run storage query tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-storage notification_outbox_migration_defines_reliable_task_table --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage notification_outbox_insert_query_links_new_event --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage notification_outbox_claim_query_uses_skip_locked_and_increments_attempt --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage notification_outbox_mark_queries_require_processing_status --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage notification_outbox_stale_release_only_matches_stale_processing_rows --manifest-path backend/Cargo.toml
cargo check -p coin-listener-storage --manifest-path backend/Cargo.toml
cargo fmt -p coin-listener-storage --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 6: Commit Task 2**

Run:

```bash
git status --short
git add backend/crates/storage/migrations/0007_notification_outbox.sql backend/crates/storage/src/repositories.rs
git commit -m "Add notification outbox migration and SQL"
```

Expected: commit succeeds and contains only the storage migration/query changes.

---

### Task 3: Add atomic event plus outbox insert helper

**Files:**
- Modify: `backend/crates/storage/src/repositories.rs:470-530`
- Test: `backend/crates/storage/src/repositories.rs:760-940`

- [ ] **Step 1: Write failing helper signature and transaction tests**

In the `#[cfg(test)] mod tests` import list in `backend/crates/storage/src/repositories.rs`, add these imports:

```rust
use std::future::Future;

use coin_listener_core::{
    models::{AddressEvent, AddressEventDraft},
    AppResult,
};
use sqlx::PgPool;
```

Add these tests after `notification_outbox_insert_query_links_new_event`:

```rust
fn assert_event_outbox_helper_signature<F, Fut>(_function: F)
where
    F: Fn(&PgPool, AddressEventDraft) -> Fut,
    Fut: Future<Output = AppResult<Option<AddressEvent>>>,
{
}

#[test]
fn insert_event_and_outbox_helper_signature_is_stable() {
    assert_event_outbox_helper_signature(super::insert_event_and_outbox_if_not_exists);
}

#[test]
fn insert_event_and_outbox_helper_uses_transaction_safe_queries() {
    assert!(INSERT_EVENT_IF_NOT_EXISTS_QUERY.contains("ON CONFLICT DO NOTHING"));
    assert!(INSERT_EVENT_IF_NOT_EXISTS_QUERY.contains("RETURNING id"));
    assert!(INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY.contains("ON CONFLICT (event_id) DO NOTHING"));
    assert!(INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY.contains("VALUES ($1, $2, 'pending')"));
}
```

- [ ] **Step 2: Run helper tests to verify RED**

Run:

```bash
cargo test -p coin-listener-storage insert_event_and_outbox_helper_signature_is_stable --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage insert_event_and_outbox_helper_uses_transaction_safe_queries --manifest-path backend/Cargo.toml
```

Expected: FAIL because `insert_event_and_outbox_if_not_exists` does not exist.

- [ ] **Step 3: Add the transactional helper**

In `backend/crates/storage/src/repositories.rs`, add this function immediately after `insert_event_if_not_exists`:

```rust
pub async fn insert_event_and_outbox_if_not_exists(
    pool: &PgPool,
    draft: AddressEventDraft,
) -> AppResult<Option<AddressEvent>> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let event = sqlx::query_as::<_, AddressEvent>(INSERT_EVENT_IF_NOT_EXISTS_QUERY)
        .bind(draft.tenant_id)
        .bind(draft.chain_id)
        .bind(draft.address_id)
        .bind(draft.asset_id)
        .bind(draft.event_type)
        .bind(draft.direction)
        .bind(draft.is_transfer)
        .bind(draft.tx_hash)
        .bind(draft.log_index)
        .bind(draft.block_number)
        .bind(draft.block_hash)
        .bind(draft.confirmations)
        .bind(draft.from_address)
        .bind(draft.to_address)
        .bind(draft.amount_raw)
        .bind(draft.amount_decimal)
        .bind(draft.balance_before_raw)
        .bind(draft.balance_after_raw)
        .bind(draft.balance_delta_raw)
        .bind(draft.metadata)
        .fetch_optional(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    if let Some(event) = event {
        sqlx::query(INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY)
            .bind(event.tenant_id)
            .bind(event.id)
            .execute(transaction.as_mut())
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;

        transaction
            .commit()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        return Ok(Some(event));
    }

    transaction
        .commit()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    Ok(None)
}
```

- [ ] **Step 4: Run helper tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-storage insert_event_and_outbox_helper_signature_is_stable --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage insert_event_and_outbox_helper_uses_transaction_safe_queries --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage insert_event_if_not_exists_query_returns_optional_event --manifest-path backend/Cargo.toml
cargo check -p coin-listener-storage --manifest-path backend/Cargo.toml
cargo fmt -p coin-listener-storage --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 5: Commit Task 3**

Run:

```bash
git status --short
git add backend/crates/storage/src/repositories.rs
git commit -m "Insert events with notification outbox rows atomically"
```

Expected: commit succeeds and contains only repository helper changes.

---

### Task 4: Add outbox claim, mark, retry, fail, and stale-release helpers

**Files:**
- Modify: `backend/crates/storage/src/repositories.rs:500-680`
- Test: `backend/crates/storage/src/repositories.rs:760-980`

- [ ] **Step 1: Write failing helper signature tests**

In `backend/crates/storage/src/repositories.rs` test imports, add `NotificationOutboxItem` to the `coin_listener_core::models` import:

```rust
use coin_listener_core::{
    models::{AddressEvent, AddressEventDraft, NotificationOutboxItem},
    AppResult,
};
```

Add these signature helpers and tests after `insert_event_and_outbox_helper_signature_is_stable`:

```rust
fn assert_claim_outbox_signature<F, Fut>(_function: F)
where
    F: Fn(&PgPool, chrono::DateTime<Utc>, &str, i64) -> Fut,
    Fut: Future<Output = AppResult<Vec<NotificationOutboxItem>>>,
{
}

fn assert_mark_outbox_delivered_signature<F, Fut>(_function: F)
where
    F: Fn(&PgPool, uuid::Uuid, chrono::DateTime<Utc>) -> Fut,
    Fut: Future<Output = AppResult<()>>,
{
}

fn assert_mark_outbox_error_signature<F, Fut>(_function: F)
where
    F: Fn(&PgPool, uuid::Uuid, &str) -> Fut,
    Fut: Future<Output = AppResult<()>>,
{
}

fn assert_mark_outbox_retryable_signature<F, Fut>(_function: F)
where
    F: Fn(&PgPool, uuid::Uuid, chrono::DateTime<Utc>, &str) -> Fut,
    Fut: Future<Output = AppResult<()>>,
{
}

fn assert_release_stale_outbox_signature<F, Fut>(_function: F)
where
    F: Fn(&PgPool, chrono::DateTime<Utc>, chrono::DateTime<Utc>) -> Fut,
    Fut: Future<Output = AppResult<u64>>,
{
}

#[test]
fn notification_outbox_repository_helper_signatures_are_stable() {
    assert_claim_outbox_signature(super::claim_due_notification_outbox);
    assert_mark_outbox_delivered_signature(super::mark_notification_outbox_delivered);
    assert_mark_outbox_retryable_signature(super::mark_notification_outbox_retryable);
    assert_mark_outbox_error_signature(super::mark_notification_outbox_failed);
    assert_release_stale_outbox_signature(super::release_stale_notification_outbox);
}
```

- [ ] **Step 2: Run helper tests to verify RED**

Run:

```bash
cargo test -p coin-listener-storage notification_outbox_repository_helper_signatures_are_stable --manifest-path backend/Cargo.toml
```

Expected: FAIL because the repository helper functions do not exist.

- [ ] **Step 3: Add claim and mark helper functions**

In `backend/crates/storage/src/repositories.rs`, add these functions after `insert_event_and_outbox_if_not_exists`:

```rust
pub async fn claim_due_notification_outbox(
    pool: &PgPool,
    now: DateTime<Utc>,
    worker_id: &str,
    limit: i64,
) -> AppResult<Vec<NotificationOutboxItem>> {
    sqlx::query_as::<_, NotificationOutboxItem>(CLAIM_DUE_NOTIFICATION_OUTBOX_QUERY)
        .bind(now)
        .bind(limit)
        .bind(worker_id)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn mark_notification_outbox_delivered(
    pool: &PgPool,
    id: Uuid,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let result = sqlx::query(MARK_NOTIFICATION_OUTBOX_DELIVERED_QUERY)
        .bind(id)
        .bind(now)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    ensure_updated(result.rows_affected())
}

pub async fn mark_notification_outbox_retryable(
    pool: &PgPool,
    id: Uuid,
    next_attempt_at: DateTime<Utc>,
    last_error: &str,
) -> AppResult<()> {
    let result = sqlx::query(MARK_NOTIFICATION_OUTBOX_RETRYABLE_QUERY)
        .bind(id)
        .bind(next_attempt_at)
        .bind(last_error)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    ensure_updated(result.rows_affected())
}

pub async fn mark_notification_outbox_failed(
    pool: &PgPool,
    id: Uuid,
    last_error: &str,
) -> AppResult<()> {
    let result = sqlx::query(MARK_NOTIFICATION_OUTBOX_FAILED_QUERY)
        .bind(id)
        .bind(last_error)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    ensure_updated(result.rows_affected())
}

pub async fn release_stale_notification_outbox(
    pool: &PgPool,
    stale_before: DateTime<Utc>,
    next_attempt_at: DateTime<Utc>,
) -> AppResult<u64> {
    let result = sqlx::query(RELEASE_STALE_NOTIFICATION_OUTBOX_QUERY)
        .bind(stale_before)
        .bind(next_attempt_at)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    Ok(result.rows_affected())
}
```

Also add `NotificationOutboxItem` to the top-level model import in `repositories.rs`:

```rust
use coin_listener_core::{
    models::{
        AddressEvent, AddressEventDraft, Asset, BalanceSnapshot, Chain,
        CreateBalanceSnapshotRequest, CreateProviderRequest, CreateWatchedAddressRequest,
        EventQuery, NotificationOutboxItem, Provider, ScanAddressCandidate, ScanAddressContext,
        ScanCursor, Tenant, User, WatchedAddress,
    },
    AppError, AppResult,
};
```

- [ ] **Step 4: Run helper tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-storage notification_outbox_repository_helper_signatures_are_stable --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage notification_outbox_claim_query_uses_skip_locked_and_increments_attempt --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage notification_outbox_mark_queries_require_processing_status --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage notification_outbox_stale_release_only_matches_stale_processing_rows --manifest-path backend/Cargo.toml
cargo check -p coin-listener-storage --manifest-path backend/Cargo.toml
cargo fmt -p coin-listener-storage --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 5: Commit Task 4**

Run:

```bash
git status --short
git add backend/crates/storage/src/repositories.rs
git commit -m "Add notification outbox repository state helpers"
```

Expected: commit succeeds and contains only outbox repository helper changes.

---

### Task 5: Wire worker scans to event plus outbox inserts

**Files:**
- Modify: `backend/crates/worker/src/lib.rs:18-780`
- Modify: `backend/crates/worker/src/main.rs:6-52`
- Test: `backend/crates/worker/src/lib.rs:1200-1380`

- [ ] **Step 1: Write failing worker source regression tests**

In `backend/crates/worker/src/lib.rs`, add these tests inside the existing `#[cfg(test)] mod tests` block after the notification task tests:

```rust
#[test]
fn worker_no_longer_enqueues_notify_tasks_after_scan() {
    let source = include_str!("lib.rs");

    assert!(!source.contains("notify_queue.enqueue"));
    assert!(!source.contains("build_notify_event_task(event, now)"));
}

#[test]
fn worker_event_insert_paths_use_outbox_helper() {
    let source = include_str!("lib.rs");

    assert!(!source.contains("insert_event_if_not_exists(pool, draft)"));
    assert!(
        source
            .matches("insert_event_and_outbox_if_not_exists(pool, draft)")
            .count()
            >= 4
    );
}
```

- [ ] **Step 2: Run worker tests to verify RED**

Run:

```bash
cargo test -p worker worker_no_longer_enqueues_notify_tasks_after_scan --manifest-path backend/Cargo.toml
cargo test -p worker worker_event_insert_paths_use_outbox_helper --manifest-path backend/Cargo.toml
```

Expected: FAIL because `process_locked_scan_task` still calls `notify_queue.enqueue` and worker event insert paths still call `insert_event_if_not_exists` or `insert_event`.

- [ ] **Step 3: Replace worker event insert calls with outbox helper**

In `backend/crates/worker/src/lib.rs`, replace this line in `scan_evm_native_balance_with_context`:

```rust
repositories::insert_event(pool, draft).await.map(Some)
```

with:

```rust
repositories::insert_event_and_outbox_if_not_exists(pool, draft).await
```

Replace every transfer event insert block shaped like this:

```rust
if let Some(event) = repositories::insert_event_if_not_exists(pool, draft).await? {
    events.push(event);
}
```

with:

```rust
if let Some(event) = repositories::insert_event_and_outbox_if_not_exists(pool, draft).await? {
    events.push(event);
}
```

Apply the replacement in all worker transfer paths:

```text
scan_evm_erc20_transfers
scan_tron_address TRX event creation
scan_tron_address TRC20 event creation
scan_btc_address transfer event creation
```

- [ ] **Step 4: Remove Redis notify enqueue from locked scan processing**

In `backend/crates/worker/src/lib.rs`, remove `NotifyEventTask` from the model import if it is only used by `build_notify_event_task` tests. Then delete these two functions and their tests if they are no longer used:

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

pub fn notify_task_for_scan_event(
    event: Option<&AddressEvent>,
    now: DateTime<Utc>,
) -> Option<NotifyEventTask> {
    event.map(|event| build_notify_event_task(event, now))
}
```

Replace the `process_scan_task` signature:

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

with:

```rust
pub async fn process_scan_task(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    scan_queue: &ScanQueue,
    task: ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<ScanTaskOutcome> {
```

Replace this call:

```rust
let outcome = process_locked_scan_task(pool, redis, notify_queue, &task, now).await;
```

with:

```rust
let outcome = process_locked_scan_task(pool, &task, now).await;
```

Replace the `process_locked_scan_task` signature:

```rust
async fn process_locked_scan_task(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    notify_queue: &NotifyQueue,
    task: &ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<ScanTaskOutcome> {
```

with:

```rust
async fn process_locked_scan_task(
    pool: &PgPool,
    task: &ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<ScanTaskOutcome> {
```

Replace each scan-plan arm that enqueues Redis notify tasks:

```rust
let events = scan_evm_address(pool, task, now).await?;
for event in &events {
    let notify_task = build_notify_event_task(event, now);
    notify_queue.enqueue(redis, &notify_task).await?;
}
repositories::finish_address_scan(pool, task.address_id, now, next_scan_at).await?;
Ok(ScanTaskOutcome::Scanned)
```

with this form:

```rust
let _events = scan_evm_address(pool, task, now).await?;
repositories::finish_address_scan(pool, task.address_id, now, next_scan_at).await?;
Ok(ScanTaskOutcome::Scanned)
```

Apply the same replacement for `ScanPlan::Tron` and `ScanPlan::Btc`, changing only the scan function name.

Replace the `run_worker` signature:

```rust
pub async fn run_worker(
    pool: PgPool,
    mut redis: MultiplexedConnection,
    scan_queue: ScanQueue,
    notify_queue: NotifyQueue,
    shutdown: Arc<AtomicBool>,
) -> AppResult<()> {
```

with:

```rust
pub async fn run_worker(
    pool: PgPool,
    mut redis: MultiplexedConnection,
    scan_queue: ScanQueue,
    shutdown: Arc<AtomicBool>,
) -> AppResult<()> {
```

Replace the `process_scan_task` call inside `run_worker`:

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

with:

```rust
match process_scan_task(&pool, &mut redis, &scan_queue, task, Utc::now()).await
```

Remove the unused `NotifyQueue` import from `worker/src/lib.rs`:

```rust
use coin_listener_storage::{repositories, scan_queue::ScanQueue};
```

- [ ] **Step 5: Stop constructing worker `NotifyQueue` in main**

In `backend/crates/worker/src/main.rs`, replace the storage import:

```rust
use coin_listener_storage::{
    connect_postgres, connect_redis,
    notify_queue::NotifyQueue,
    run_migrations,
    scan_queue::{connect_scan_queue, ScanQueue},
};
```

with:

```rust
use coin_listener_storage::{
    connect_postgres, connect_redis, run_migrations,
    scan_queue::{connect_scan_queue, ScanQueue},
};
```

Delete this line:

```rust
let notify_queue = NotifyQueue::new(config.notify.queue_key.clone());
```

Remove this field from the `info!` call:

```rust
notify_queue_key = notify_queue.queue_key(),
```

Replace:

```rust
run_worker(postgres, redis, scan_queue, notify_queue, shutdown).await?;
```

with:

```rust
run_worker(postgres, redis, scan_queue, shutdown).await?;
```

- [ ] **Step 6: Run worker integration tests to verify GREEN**

Run:

```bash
cargo test -p worker worker_no_longer_enqueues_notify_tasks_after_scan --manifest-path backend/Cargo.toml
cargo test -p worker worker_event_insert_paths_use_outbox_helper --manifest-path backend/Cargo.toml
cargo test -p worker btc_worker_helpers --manifest-path backend/Cargo.toml
cargo test -p worker tron_worker_helpers --manifest-path backend/Cargo.toml
cargo check -p worker --manifest-path backend/Cargo.toml
cargo fmt -p worker --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 7: Commit Task 5**

Run:

```bash
git status --short
git add backend/crates/worker/src/lib.rs backend/crates/worker/src/main.rs
git commit -m "Write worker notification tasks to outbox"
```

Expected: commit succeeds and contains only worker integration changes.

---

### Task 6: Add notifier outbox item processing and retry helpers

**Files:**
- Modify: `backend/crates/notifier/src/lib.rs:1-230`
- Test: `backend/crates/notifier/src/lib.rs:439-760`

- [ ] **Step 1: Write failing notifier retry tests**

In `backend/crates/notifier/src/lib.rs`, update the model import from:

```rust
models::{AddressEvent, NotificationChannel, NotificationRule, NotifyEventTask},
```

to:

```rust
models::{
    AddressEvent, NotificationChannel, NotificationOutboxItem, NotificationRule, NotifyEventTask,
},
```

Update the test import list from:

```rust
use crate::{
    amount_raw_meets_minimum, build_delivery_plan, build_in_app_notification_content,
    build_unavailable_channel_delivery_plan, delivery_sent_at, missing_channel_error,
    notification_rule_matches_event, notifier_shutdown_requested, notify_channel_decision,
    resolve_explicit_rule_channels, NotifyChannelDecision, ResolvedNotifyChannel,
    DELIVERY_STATUS_SENT, DELIVERY_STATUS_SKIPPED, MISSING_CHANNEL_ERROR_PREFIX,
    UNAVAILABLE_CHANNEL_ERROR,
};
```

to:

```rust
use crate::{
    amount_raw_meets_minimum, build_delivery_plan, build_in_app_notification_content,
    build_unavailable_channel_delivery_plan, delivery_sent_at, missing_channel_error,
    notification_outbox_next_attempt_at, notification_outbox_should_fail,
    notification_outbox_task_attempt, notification_rule_matches_event,
    notifier_shutdown_requested, notify_channel_decision, notify_task_from_outbox_item,
    resolve_explicit_rule_channels, NotifyChannelDecision, ResolvedNotifyChannel,
    DELIVERY_STATUS_SENT, DELIVERY_STATUS_SKIPPED, MISSING_CHANNEL_ERROR_PREFIX,
    UNAVAILABLE_CHANNEL_ERROR,
};
```

Add these tests after `rule_matches_when_all_filters_match`:

```rust
#[test]
fn notification_outbox_backoff_is_deterministic() {
    let now = Utc.with_ymd_and_hms(2026, 5, 18, 12, 0, 0).unwrap();

    assert_eq!(
        notification_outbox_next_attempt_at(now, 1),
        now + chrono::Duration::seconds(30)
    );
    assert_eq!(
        notification_outbox_next_attempt_at(now, 2),
        now + chrono::Duration::seconds(60)
    );
    assert_eq!(
        notification_outbox_next_attempt_at(now, 3),
        now + chrono::Duration::seconds(300)
    );
    assert_eq!(
        notification_outbox_next_attempt_at(now, 4),
        now + chrono::Duration::seconds(900)
    );
    assert_eq!(
        notification_outbox_next_attempt_at(now, 5),
        now + chrono::Duration::seconds(3600)
    );
}

#[test]
fn notification_outbox_retry_policy_fails_at_max_attempts() {
    assert!(!notification_outbox_should_fail(9, 10));
    assert!(notification_outbox_should_fail(10, 10));
    assert!(notification_outbox_should_fail(11, 10));
}

#[test]
fn notification_outbox_task_attempt_clamps_to_notify_task_type() {
    assert_eq!(notification_outbox_task_attempt(1), 1);
    assert_eq!(notification_outbox_task_attempt(i32::MAX), u16::MAX);
    assert_eq!(notification_outbox_task_attempt(-1), 0);
}

#[test]
fn outbox_item_converts_to_legacy_notify_task_for_processing() {
    let item = outbox_item(3);
    let task = notify_task_from_outbox_item(&item);

    assert_eq!(task.task_id, item.id);
    assert_eq!(task.event_id, item.event_id);
    assert_eq!(task.tenant_id, item.tenant_id);
    assert_eq!(task.attempt, 3);
    assert_eq!(task.enqueued_at, item.created_at);
}
```

Add this helper inside `mod tests` near `fn event()`:

```rust
fn outbox_item(attempt_count: i32) -> NotificationOutboxItem {
    NotificationOutboxItem {
        id: uuid(90),
        tenant_id: uuid(2),
        event_id: uuid(1),
        status: "processing".to_string(),
        attempt_count,
        next_attempt_at: Utc.with_ymd_and_hms(2026, 5, 18, 12, 0, 0).unwrap(),
        locked_at: Some(Utc.with_ymd_and_hms(2026, 5, 18, 12, 0, 1).unwrap()),
        locked_by: Some("notifier-test".to_string()),
        last_error: None,
        delivered_at: None,
        created_at: Utc.with_ymd_and_hms(2026, 5, 18, 11, 59, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2026, 5, 18, 12, 0, 1).unwrap(),
    }
}
```

- [ ] **Step 2: Run notifier retry tests to verify RED**

Run:

```bash
cargo test -p notifier notification_outbox_backoff_is_deterministic --manifest-path backend/Cargo.toml
cargo test -p notifier notification_outbox_retry_policy_fails_at_max_attempts --manifest-path backend/Cargo.toml
cargo test -p notifier outbox_item_converts_to_legacy_notify_task_for_processing --manifest-path backend/Cargo.toml
```

Expected: FAIL because retry and conversion helpers do not exist.

- [ ] **Step 3: Add retry and conversion helpers**

In `backend/crates/notifier/src/lib.rs`, add these functions after `delivery_sent_at`:

```rust
pub fn notification_outbox_next_attempt_at(
    now: DateTime<Utc>,
    attempt_count: i32,
) -> DateTime<Utc> {
    let delay_seconds = match attempt_count {
        0 | 1 => 30,
        2 => 60,
        3 => 300,
        4 => 900,
        _ => 3600,
    };
    now + chrono::Duration::seconds(delay_seconds)
}

pub fn notification_outbox_should_fail(attempt_count: i32, max_attempts: i32) -> bool {
    attempt_count >= max_attempts
}

pub fn notification_outbox_task_attempt(attempt_count: i32) -> u16 {
    if attempt_count <= 0 {
        return 0;
    }
    u16::try_from(attempt_count).unwrap_or(u16::MAX)
}

pub fn notify_task_from_outbox_item(item: &NotificationOutboxItem) -> NotifyEventTask {
    NotifyEventTask {
        task_id: item.id,
        event_id: item.event_id,
        tenant_id: item.tenant_id,
        attempt: notification_outbox_task_attempt(item.attempt_count),
        enqueued_at: item.created_at,
    }
}
```

- [ ] **Step 4: Add outbox item processing function**

In `backend/crates/notifier/src/lib.rs`, add this function immediately after `process_notify_task`:

```rust
pub async fn process_notification_outbox_item(
    pool: &PgPool,
    item: NotificationOutboxItem,
    now: DateTime<Utc>,
) -> AppResult<usize> {
    process_notify_task(pool, notify_task_from_outbox_item(&item), now).await
}
```

This function intentionally reuses the existing rule matching, channel resolution, `notification_deliveries`, and `in_app_notifications` creation path. If no rules match, `process_notify_task` returns `Ok(0)` and the dispatcher will still mark the outbox item delivered.

- [ ] **Step 5: Run notifier retry tests to verify GREEN**

Run:

```bash
cargo test -p notifier notification_outbox_backoff_is_deterministic --manifest-path backend/Cargo.toml
cargo test -p notifier notification_outbox_retry_policy_fails_at_max_attempts --manifest-path backend/Cargo.toml
cargo test -p notifier notification_outbox_task_attempt_clamps_to_notify_task_type --manifest-path backend/Cargo.toml
cargo test -p notifier outbox_item_converts_to_legacy_notify_task_for_processing --manifest-path backend/Cargo.toml
cargo check -p notifier --manifest-path backend/Cargo.toml
cargo fmt -p notifier --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 6: Commit Task 6**

Run:

```bash
git status --short
git add backend/crates/notifier/src/lib.rs
git commit -m "Process notification outbox items in notifier"
```

Expected: commit succeeds and contains only notifier processing/retry helper changes.

---

### Task 7: Replace notifier Redis BRPOP loop with DB outbox dispatcher

**Files:**
- Modify: `backend/crates/notifier/src/lib.rs:1-470`
- Modify: `backend/crates/notifier/src/main.rs:1-48`
- Test: `backend/crates/notifier/src/lib.rs:439-820`

- [ ] **Step 1: Write failing notifier dispatcher tests**

In `backend/crates/notifier/src/lib.rs`, add `NotifyConfig` to the core import:

```rust
use coin_listener_core::{
    models::{
        AddressEvent, NotificationChannel, NotificationOutboxItem, NotificationRule, NotifyEventTask,
    },
    AppResult, NotifyConfig,
};
```

Update the test import list to include `NotificationOutboxDispatcherConfig`:

```rust
use crate::{
    amount_raw_meets_minimum, build_delivery_plan, build_in_app_notification_content,
    build_unavailable_channel_delivery_plan, delivery_sent_at, missing_channel_error,
    notification_outbox_next_attempt_at, notification_outbox_should_fail,
    notification_outbox_task_attempt, notification_rule_matches_event,
    notifier_shutdown_requested, notify_channel_decision, notify_task_from_outbox_item,
    resolve_explicit_rule_channels, NotificationOutboxDispatcherConfig, NotifyChannelDecision,
    ResolvedNotifyChannel, DELIVERY_STATUS_SENT, DELIVERY_STATUS_SKIPPED,
    MISSING_CHANNEL_ERROR_PREFIX, UNAVAILABLE_CHANNEL_ERROR,
};
```

Add these tests after `notification_outbox_retry_policy_fails_at_max_attempts`:

```rust
#[test]
fn dispatcher_config_is_loaded_from_notify_config() {
    let notify = NotifyConfig {
        queue_key: "notify:event:queue".to_string(),
        outbox_batch_size: 25,
        outbox_max_attempts: 7,
        outbox_stale_lock_seconds: 120,
        outbox_idle_sleep_ms: 250,
    };

    let config = NotificationOutboxDispatcherConfig::from_notify_config(&notify);

    assert_eq!(config.batch_size, 25);
    assert_eq!(config.max_attempts, 7);
    assert_eq!(config.stale_lock_seconds, 120);
    assert_eq!(config.idle_sleep, std::time::Duration::from_millis(250));
}

#[test]
fn notifier_loop_uses_outbox_repository_instead_of_redis_dequeue() {
    let source = include_str!("lib.rs");

    assert!(source.contains("claim_due_notification_outbox"));
    assert!(source.contains("release_stale_notification_outbox"));
    assert!(source.contains("mark_notification_outbox_delivered"));
    assert!(source.contains("mark_notification_outbox_retryable"));
    assert!(source.contains("mark_notification_outbox_failed"));
    assert!(!source.contains("notify_queue.dequeue"));
}
```

- [ ] **Step 2: Run dispatcher tests to verify RED**

Run:

```bash
cargo test -p notifier dispatcher_config_is_loaded_from_notify_config --manifest-path backend/Cargo.toml
cargo test -p notifier notifier_loop_uses_outbox_repository_instead_of_redis_dequeue --manifest-path backend/Cargo.toml
```

Expected: FAIL because `NotificationOutboxDispatcherConfig` does not exist and `run_notifier` still uses Redis dequeue.

- [ ] **Step 3: Add dispatcher config and batch processing**

In `backend/crates/notifier/src/lib.rs`, replace the storage import:

```rust
use coin_listener_storage::{notifications, notify_queue::NotifyQueue};
use redis::aio::MultiplexedConnection;
```

with:

```rust
use coin_listener_storage::{notifications, repositories};
```

Add `Duration` to the `std` import:

```rust
use std::{
    cmp::Ordering,
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering as AtomicOrdering},
        Arc,
    },
    time::Duration,
};
```

Add this struct after `NotifyDeliveryPlan`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationOutboxDispatcherConfig {
    pub batch_size: i64,
    pub max_attempts: i32,
    pub stale_lock_seconds: i64,
    pub idle_sleep: Duration,
}

impl NotificationOutboxDispatcherConfig {
    pub fn from_notify_config(config: &NotifyConfig) -> Self {
        Self {
            batch_size: config.outbox_batch_size,
            max_attempts: config.outbox_max_attempts,
            stale_lock_seconds: config.outbox_stale_lock_seconds,
            idle_sleep: Duration::from_millis(config.outbox_idle_sleep_ms),
        }
    }
}
```

Add this batch function after `process_notification_outbox_item`:

```rust
pub async fn process_notification_outbox_batch(
    pool: &PgPool,
    worker_id: &str,
    config: &NotificationOutboxDispatcherConfig,
    now: DateTime<Utc>,
) -> AppResult<usize> {
    let stale_before = now - chrono::Duration::seconds(config.stale_lock_seconds);
    let released =
        repositories::release_stale_notification_outbox(pool, stale_before, now).await?;
    if released > 0 {
        info!(released, "released stale notification outbox rows");
    }

    let items = repositories::claim_due_notification_outbox(
        pool,
        now,
        worker_id,
        config.batch_size,
    )
    .await?;
    let claimed = items.len();

    for item in items {
        let outbox_id = item.id;
        let event_id = item.event_id;
        let tenant_id = item.tenant_id;
        let attempt_count = item.attempt_count;

        match process_notification_outbox_item(pool, item, now).await {
            Ok(deliveries) => {
                repositories::mark_notification_outbox_delivered(pool, outbox_id, now).await?;
                info!(
                    outbox_id = %outbox_id,
                    event_id = %event_id,
                    tenant_id = %tenant_id,
                    deliveries,
                    "notification outbox item delivered"
                );
            }
            Err(error) => {
                let last_error = error.to_string();
                if notification_outbox_should_fail(attempt_count, config.max_attempts) {
                    repositories::mark_notification_outbox_failed(pool, outbox_id, &last_error)
                        .await?;
                    warn!(
                        outbox_id = %outbox_id,
                        event_id = %event_id,
                        tenant_id = %tenant_id,
                        attempt_count,
                        error = %last_error,
                        "notification outbox item failed permanently"
                    );
                } else {
                    let next_attempt_at = notification_outbox_next_attempt_at(now, attempt_count);
                    repositories::mark_notification_outbox_retryable(
                        pool,
                        outbox_id,
                        next_attempt_at,
                        &last_error,
                    )
                    .await?;
                    warn!(
                        outbox_id = %outbox_id,
                        event_id = %event_id,
                        tenant_id = %tenant_id,
                        attempt_count,
                        next_attempt_at = %next_attempt_at,
                        error = %last_error,
                        "notification outbox item scheduled for retry"
                    );
                }
            }
        }
    }

    Ok(claimed)
}
```

- [ ] **Step 4: Replace `run_notifier` with DB outbox loop**

Replace the existing `run_notifier` function:

```rust
pub async fn run_notifier(
    pool: PgPool,
    mut redis: MultiplexedConnection,
    notify_queue: NotifyQueue,
    shutdown: Arc<AtomicBool>,
) -> AppResult<()> {
    while !notifier_shutdown_requested(&shutdown) {
        match notify_queue.dequeue(&mut redis, 5).await {
            Ok(Some(task)) => {
                let task_id = task.task_id;
                let event_id = task.event_id;
                let tenant_id = task.tenant_id;
                match process_notify_task(&pool, task, Utc::now()).await {
                    Ok(deliveries) => info!(
                        task_id = %task_id,
                        event_id = %event_id,
                        tenant_id = %tenant_id,
                        deliveries,
                        "notify task processed"
                    ),
                    Err(error) => warn!(
                        task_id = %task_id,
                        event_id = %event_id,
                        tenant_id = %tenant_id,
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
```

with:

```rust
pub async fn run_notifier(
    pool: PgPool,
    config: NotificationOutboxDispatcherConfig,
    shutdown: Arc<AtomicBool>,
) -> AppResult<()> {
    let worker_id = format!("notifier-{}", uuid::Uuid::new_v4());

    while !notifier_shutdown_requested(&shutdown) {
        let claimed = process_notification_outbox_batch(&pool, &worker_id, &config, Utc::now()).await?;
        if claimed == 0 {
            tokio::time::sleep(config.idle_sleep).await;
        }
    }

    Ok(())
}
```

- [ ] **Step 5: Update notifier main to remove Redis notify queue dependency**

In `backend/crates/notifier/src/main.rs`, replace imports:

```rust
use coin_listener_storage::{
    connect_postgres, connect_redis,
    notify_queue::{connect_notify_queue, NotifyQueue},
    run_migrations,
};
use notifier::run_notifier;
```

with:

```rust
use coin_listener_storage::{connect_postgres, run_migrations};
use notifier::{run_notifier, NotificationOutboxDispatcherConfig};
```

Delete these lines:

```rust
let redis_client = connect_redis(&config.redis)?;
let redis = connect_notify_queue(&redis_client).await?;
let notify_queue = NotifyQueue::new(config.notify.queue_key.clone());
```

Replace the `info!` call fields:

```rust
notify_queue_key = notify_queue.queue_key(),
"service started"
```

with:

```rust
outbox_batch_size = config.notify.outbox_batch_size,
outbox_max_attempts = config.notify.outbox_max_attempts,
outbox_stale_lock_seconds = config.notify.outbox_stale_lock_seconds,
outbox_idle_sleep_ms = config.notify.outbox_idle_sleep_ms,
"service started"
```

Add this line before calling `run_notifier`:

```rust
let dispatcher_config = NotificationOutboxDispatcherConfig::from_notify_config(&config.notify);
```

Replace:

```rust
run_notifier(postgres, redis, notify_queue, shutdown).await?;
```

with:

```rust
run_notifier(postgres, dispatcher_config, shutdown).await?;
```

- [ ] **Step 6: Run notifier dispatcher tests to verify GREEN**

Run:

```bash
cargo test -p notifier dispatcher_config_is_loaded_from_notify_config --manifest-path backend/Cargo.toml
cargo test -p notifier notifier_loop_uses_outbox_repository_instead_of_redis_dequeue --manifest-path backend/Cargo.toml
cargo test -p notifier notification_outbox_backoff_is_deterministic --manifest-path backend/Cargo.toml
cargo test -p notifier rule_matches_when_all_filters_match --manifest-path backend/Cargo.toml
cargo check -p notifier --manifest-path backend/Cargo.toml
cargo fmt -p notifier --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 7: Commit Task 7**

Run:

```bash
git status --short
git add backend/crates/notifier/src/lib.rs backend/crates/notifier/src/main.rs
git commit -m "Dispatch notifications from database outbox"
```

Expected: commit succeeds and contains only notifier dispatcher changes.

---

### Task 8: Run final verification

**Files:**
- Verify: full backend workspace, frontend build, Docker Compose config

- [ ] **Step 1: Run backend format check**

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

Expected: exit 0. Existing dependency future-incompat warnings may remain, but no compile errors are allowed.

- [ ] **Step 3: Run backend workspace tests**

Run:

```bash
cargo test --workspace --manifest-path backend/Cargo.toml
```

Expected: exit 0 with all Rust crate tests passing.

- [ ] **Step 4: Run frontend build regression**

Run:

```bash
npm run build --prefix frontend
```

Expected: exit 0. Existing lottie direct-eval and chunk-size warnings may remain, but no build failure is allowed.

- [ ] **Step 5: Validate Docker Compose config**

Run:

```bash
docker compose -f docker-compose.yml config
```

Expected: exit 0.

- [ ] **Step 6: Final commit**

Run:

```bash
git status --short
git add backend/crates/core/src/models.rs backend/crates/core/src/config.rs backend/crates/storage/migrations/0007_notification_outbox.sql backend/crates/storage/src/repositories.rs backend/crates/worker/src/lib.rs backend/crates/worker/src/main.rs backend/crates/notifier/src/lib.rs backend/crates/notifier/src/main.rs
git commit -m "Add notification outbox reliable delivery"
```

Expected: if previous task commits were created, Git may report no changes to commit. If there are remaining verified changes, commit them here.

---

## Self-Review Checklist

Spec coverage:

- New `notification_outbox` migration with unique `event_id`, claim index, stale-processing index, and event index: Task 2.
- New shared outbox row model: Task 1.
- Event and outbox written in the same DB transaction only when the event is newly inserted: Task 3.
- Existing events do not create duplicate outbox rows: Task 3 uses existing `ON CONFLICT DO NOTHING RETURNING id` semantics and only inserts outbox when `Some(event)` is returned.
- Worker no longer relies on Redis notify enqueue for reliable notification dispatch: Task 5.
- EVM, BTC, TRON transfer event paths use the event+outbox helper: Task 5.
- EVM balance-change address events also create outbox rows: Task 5.
- Notifier claims pending/retryable outbox rows using `FOR UPDATE SKIP LOCKED`: Task 2 and Task 4.
- Successful outbox processing marks rows delivered, including no-matching-rule `Ok(0)` outcomes: Task 6 and Task 7.
- Failed processing marks rows retryable or failed according to max attempts: Task 6 and Task 7.
- Stale processing rows are released to retryable: Task 4 and Task 7.
- Existing notification rule matching, delivery creation, and in-app creation are reused through `process_notify_task`: Task 6.
- Backend workspace, frontend build, and Docker Compose verification are included: Task 8.

Placeholder scan:

- This plan contains no `TBD` placeholders.
- Each task names exact files, commands, expected failures, expected passes, and commit commands.
- Code snippets define the types and functions later tasks reference.

Type consistency:

- `NotificationOutboxItem.attempt_count` is `i32`, matching SQL `INTEGER` and retry policy functions.
- `NotifyConfig.outbox_batch_size` is `i64`, matching SQL `LIMIT $2` binding.
- `NotifyConfig.outbox_max_attempts` and `NotificationOutboxDispatcherConfig.max_attempts` are `i32`, matching `attempt_count` comparison.
- `NotifyConfig.outbox_idle_sleep_ms` is `u64`, matching `Duration::from_millis`.
