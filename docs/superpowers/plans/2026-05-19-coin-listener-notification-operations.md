# Coin Listener Notification Operations Console Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a notification operations console that exposes outbox backlog, delivery diagnostics, provider metadata, and safe outbox-level retry.

**Architecture:** Keep PostgreSQL `notification_outbox` as the only reliable notification task source and expose read-only diagnostics plus a constrained retry update. Backend work adds shared DTOs, storage queries, API routes, and system-status outbox counts; frontend work adds typed API helpers and one Semi Design operations page reusing the existing React Query, Table, Tag, Modal, and Toast patterns.

**Tech Stack:** Rust 2021, Axum, SQLx, PostgreSQL migrations, chrono, uuid, serde/serde_json, React, TypeScript, React Query, Vite, Semi Design.

---

## Scope and Constraints

Implement only the approved scope from `docs/superpowers/specs/2026-05-19-coin-listener-notification-operations-design.md`.

In scope:

- Outbox list, detail, status counts, stale-processing visibility, and failed/retryable retry.
- Delivery list and delivery rows in outbox detail.
- Provider metadata display with long text truncated by default.
- System status notification outbox counts.
- Frontend navigation entry named `通知运维`.

Out of scope:

- Delivery-level retry.
- Replay of `delivered` outbox rows.
- Force unlock for active `processing` rows.
- Provider health probing, circuit breaker, rate limiting, WebSocket push, Prometheus, or channel configuration forms.

Preserve unrelated working-tree changes. If `frontend/package-lock.json` is already modified before a task starts, do not stage it unless the task itself intentionally changes package dependencies; this plan adds no dependencies.

---

## File Structure

Create:

```text
backend/crates/storage/migrations/0009_notification_ops_indexes.sql
frontend/src/pages/NotificationOperationsPage.tsx
```

Modify:

```text
backend/crates/core/src/models.rs
backend/crates/storage/src/repositories.rs
backend/crates/storage/src/notifications.rs
backend/crates/storage/src/system_status.rs
backend/crates/api-server/src/routes.rs
frontend/src/api/types.ts
frontend/src/api/client.ts
frontend/src/App.tsx
frontend/src/pages/SystemStatusPage.tsx
```

Responsibilities:

- `backend/crates/core/src/models.rs`: shared notification operations query/response DTOs and outbox status counts embedded in `NotificationStatus`.
- `backend/crates/storage/migrations/0009_notification_ops_indexes.sql`: additive indexes for outbox and delivery operations queries.
- `backend/crates/storage/src/repositories.rs`: outbox operations status validation, pagination helpers, list/detail/retry/status-count storage functions, and query string tests.
- `backend/crates/storage/src/notifications.rs`: delivery operations status/channel validation, pagination helpers, list and event-scoped delivery query functions, and query string tests.
- `backend/crates/storage/src/system_status.rs`: include outbox counts in notification system status.
- `backend/crates/api-server/src/routes.rs`: HTTP routes for outbox list/detail/retry and delivery list with validation-first error behavior.
- `frontend/src/api/types.ts`: TypeScript DTOs matching backend JSON.
- `frontend/src/api/client.ts`: query-string builders and client methods for outbox/delivery operations.
- `frontend/src/pages/NotificationOperationsPage.tsx`: Semi Design operations page with summary cards, filters, outbox table, retry action, and detail modal.
- `frontend/src/App.tsx`: navigation key, route, and nav item.
- `frontend/src/pages/SystemStatusPage.tsx`: display the new outbox counts on the existing system-status page.

---

### Task 1: Add notification operations DTOs

**Files:**
- Modify: `backend/crates/core/src/models.rs:218-379`
- Test: `backend/crates/core/src/models.rs` existing `#[cfg(test)] mod tests`

- [ ] **Step 1: Write failing DTO serde tests**

In `backend/crates/core/src/models.rs`, update the test import list from:

```rust
use super::{
    CreateBalanceSnapshotRequest, EventStatus, NotificationDelivery, NotificationOutboxItem,
    NotificationStatus, NotifyEventTask, ProviderChainStatus, ProviderStatus,
    ProviderStatusItem, QueueStatus, ScanAddressTask, ScanCursor, ScanStatus, SystemStatus,
};
```

to:

```rust
use super::{
    AddressEvent, CreateBalanceSnapshotRequest, EventStatus, NotificationDelivery,
    NotificationDeliveryListItem, NotificationDeliveryListResponse, NotificationDeliveryQuery,
    NotificationOutboxDetail, NotificationOutboxListItem, NotificationOutboxListResponse,
    NotificationOutboxQuery, NotificationOutboxItem, NotificationStatus, NotifyEventTask,
    OutboxStatusCounts, ProviderChainStatus, ProviderStatus, ProviderStatusItem, QueueStatus,
    RetryNotificationOutboxResponse, ScanAddressTask, ScanCursor, ScanStatus, SystemStatus,
};
```

Append these tests after `notification_outbox_item_round_trips_as_json`:

```rust
#[test]
fn notification_status_round_trips_outbox_counts() {
    let status = NotificationStatus {
        last_24h_sent: 20,
        last_24h_skipped: 2,
        last_24h_failed: 1,
        unread_in_app: 4,
        outbox: OutboxStatusCounts {
            pending: 3,
            retryable: 2,
            processing: 1,
            failed: 5,
            stale_processing: 1,
            next_due_at: Some(Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 0).unwrap()),
        },
    };

    let payload = serde_json::to_string(&status).expect("serialize notification status");
    let decoded: NotificationStatus =
        serde_json::from_str(&payload).expect("deserialize notification status");

    assert_eq!(decoded, status);
    assert!(payload.contains("\"pending\":3"));
    assert!(payload.contains("\"stale_processing\":1"));
}

#[test]
fn notification_operations_queries_deserialize_filters() {
    let outbox_query: NotificationOutboxQuery = serde_json::from_str(
        r#"{"status":"failed","event_id":"00000000-0000-0000-0000-000000000001","limit":50,"offset":10}"#,
    )
    .expect("deserialize outbox query");
    assert_eq!(outbox_query.status.as_deref(), Some("failed"));
    assert_eq!(outbox_query.limit, Some(50));
    assert_eq!(outbox_query.offset, Some(10));

    let delivery_query: NotificationDeliveryQuery = serde_json::from_str(
        r#"{"status":"failed","channel_type":"webhook","rule_id":"00000000-0000-0000-0000-000000000002","channel_id":"00000000-0000-0000-0000-000000000003","limit":25,"offset":5}"#,
    )
    .expect("deserialize delivery query");
    assert_eq!(delivery_query.status.as_deref(), Some("failed"));
    assert_eq!(delivery_query.channel_type.as_deref(), Some("webhook"));
    assert_eq!(delivery_query.limit, Some(25));
    assert_eq!(delivery_query.offset, Some(5));
}

#[test]
fn notification_operations_responses_round_trip_provider_metadata() {
    let created_at = Utc.with_ymd_and_hms(2026, 5, 19, 9, 0, 0).unwrap();
    let event_id = Uuid::from_u128(13);
    let outbox = NotificationOutboxListItem {
        id: Uuid::from_u128(11),
        tenant_id: Uuid::from_u128(12),
        event_id,
        status: "failed".to_string(),
        attempt_count: 5,
        next_attempt_at: Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 0).unwrap(),
        locked_at: None,
        locked_by: None,
        last_error: Some("webhook returned retryable status 500".to_string()),
        delivered_at: None,
        created_at,
        updated_at: created_at,
        event_type: Some("transfer".to_string()),
        direction: Some("in".to_string()),
        tx_hash: Some("0xabc".to_string()),
        delivery_total: 2,
        delivery_sent: 1,
        delivery_failed: 1,
        delivery_skipped: 0,
        is_stale_processing: false,
    };
    let delivery = NotificationDeliveryListItem {
        id: Uuid::from_u128(21),
        tenant_id: Uuid::from_u128(12),
        event_id,
        rule_id: Some(Uuid::from_u128(22)),
        channel_id: Some(Uuid::from_u128(23)),
        channel_type: Some("webhook".to_string()),
        status: "failed".to_string(),
        attempt_count: 3,
        last_error: Some("webhook returned retryable status 500".to_string()),
        sent_at: None,
        created_at,
        idempotency_key: Some("notification:v1:tenant:event:rule:channel".to_string()),
        provider_message_id: None,
        provider_status_code: Some(500),
        provider_response: Some("server error".to_string()),
    };
    let event = AddressEvent {
        id: event_id,
        tenant_id: Uuid::from_u128(12),
        chain_id: Uuid::from_u128(31),
        address_id: Uuid::from_u128(32),
        asset_id: Uuid::from_u128(33),
        event_type: "transfer".to_string(),
        direction: "in".to_string(),
        is_transfer: true,
        tx_hash: Some("0xabc".to_string()),
        log_index: Some(1),
        block_number: Some(100),
        block_hash: Some("0xblock".to_string()),
        confirmations: 12,
        from_address: Some("0xfrom".to_string()),
        to_address: Some("0xto".to_string()),
        amount_raw: Some("1000".to_string()),
        amount_decimal: Some("0.001".to_string()),
        balance_before_raw: None,
        balance_after_raw: None,
        balance_delta_raw: None,
        metadata: serde_json::json!({"source":"test"}),
        detected_at: created_at,
        created_at,
    };
    let detail = NotificationOutboxDetail {
        outbox: outbox.clone(),
        event,
        deliveries: vec![delivery.clone()],
    };

    let outbox_list = NotificationOutboxListResponse {
        items: vec![outbox.clone()],
        limit: 50,
        offset: 0,
    };
    let delivery_list = NotificationDeliveryListResponse {
        items: vec![delivery],
        limit: 50,
        offset: 0,
    };
    let retry = RetryNotificationOutboxResponse {
        outbox: NotificationOutboxItem {
            id: outbox.id,
            tenant_id: outbox.tenant_id,
            event_id: outbox.event_id,
            status: "retryable".to_string(),
            attempt_count: outbox.attempt_count,
            next_attempt_at: outbox.next_attempt_at,
            locked_at: None,
            locked_by: None,
            last_error: None,
            delivered_at: None,
            created_at: outbox.created_at,
            updated_at: outbox.updated_at,
        },
    };

    let payload = serde_json::to_string(&(outbox_list, detail, delivery_list, retry))
        .expect("serialize operations responses");

    assert!(payload.contains("\"delivery_failed\":1"));
    assert!(payload.contains("\"provider_status_code\":500"));
    assert!(payload.contains("\"outbox\""));
}
```

- [ ] **Step 2: Run tests to verify RED**

Run:

```bash
cargo test -p coin-listener-core notification_status_round_trips_outbox_counts --manifest-path backend/Cargo.toml
cargo test -p coin-listener-core notification_operations_queries_deserialize_filters --manifest-path backend/Cargo.toml
cargo test -p coin-listener-core notification_operations_responses_round_trip_provider_metadata --manifest-path backend/Cargo.toml
```

Expected: FAIL because the new DTOs and `NotificationStatus.outbox` do not exist.

- [ ] **Step 3: Add DTOs and extend `NotificationStatus`**

In `backend/crates/core/src/models.rs`, replace `NotificationStatus` with:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationStatus {
    pub last_24h_sent: i64,
    pub last_24h_skipped: i64,
    pub last_24h_failed: i64,
    pub unread_in_app: i64,
    pub outbox: OutboxStatusCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct OutboxStatusCounts {
    pub pending: i64,
    pub retryable: i64,
    pub processing: i64,
    pub failed: i64,
    pub stale_processing: i64,
    pub next_due_at: Option<DateTime<Utc>>,
}
```

Add these DTOs after `NotificationOutboxItem`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct NotificationOutboxQuery {
    pub status: Option<String>,
    pub event_id: Option<Uuid>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationOutboxListItem {
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
    pub event_type: Option<String>,
    pub direction: Option<String>,
    pub tx_hash: Option<String>,
    pub delivery_total: i64,
    pub delivery_sent: i64,
    pub delivery_failed: i64,
    pub delivery_skipped: i64,
    pub is_stale_processing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationOutboxListResponse {
    pub items: Vec<NotificationOutboxListItem>,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationOutboxDetail {
    pub outbox: NotificationOutboxListItem,
    pub event: AddressEvent,
    pub deliveries: Vec<NotificationDeliveryListItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NotificationDeliveryQuery {
    pub event_id: Option<Uuid>,
    pub status: Option<String>,
    pub channel_type: Option<String>,
    pub rule_id: Option<Uuid>,
    pub channel_id: Option<Uuid>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationDeliveryListItem {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub event_id: Uuid,
    pub rule_id: Option<Uuid>,
    pub channel_id: Option<Uuid>,
    pub channel_type: Option<String>,
    pub status: String,
    pub attempt_count: i32,
    pub last_error: Option<String>,
    pub sent_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub idempotency_key: Option<String>,
    pub provider_message_id: Option<String>,
    pub provider_status_code: Option<i32>,
    pub provider_response: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationDeliveryListResponse {
    pub items: Vec<NotificationDeliveryListItem>,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryNotificationOutboxResponse {
    pub outbox: NotificationOutboxItem,
}
```

Update `system_status_round_trips_as_json` so the `notifications` value includes outbox counts:

```rust
notifications: NotificationStatus {
    last_24h_sent: 20,
    last_24h_skipped: 2,
    last_24h_failed: 1,
    unread_in_app: 4,
    outbox: OutboxStatusCounts {
        pending: 0,
        retryable: 0,
        processing: 0,
        failed: 0,
        stale_processing: 0,
        next_due_at: None,
    },
},
```

- [ ] **Step 4: Run tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-core notification_status_round_trips_outbox_counts --manifest-path backend/Cargo.toml
cargo test -p coin-listener-core notification_operations_queries_deserialize_filters --manifest-path backend/Cargo.toml
cargo test -p coin-listener-core notification_operations_responses_round_trip_provider_metadata --manifest-path backend/Cargo.toml
cargo test -p coin-listener-core system_status_round_trips_as_json --manifest-path backend/Cargo.toml
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 5: Commit Task 1**

Run:

```bash
git status --short
git add backend/crates/core/src/models.rs
git commit -m "Add notification operations DTOs"
```

Expected: commit succeeds and only `backend/crates/core/src/models.rs` is staged.

---

### Task 2: Add notification operations indexes

**Files:**
- Create: `backend/crates/storage/migrations/0009_notification_ops_indexes.sql`
- Modify: `backend/crates/storage/src/repositories.rs:1071-1086`
- Test: `backend/crates/storage/src/repositories.rs` existing `#[cfg(test)] mod tests`

- [ ] **Step 1: Write failing migration string test**

In `backend/crates/storage/src/repositories.rs`, add this test after `notification_outbox_migration_defines_reliable_task_table`:

```rust
#[test]
fn notification_ops_indexes_migration_adds_outbox_and_delivery_indexes() {
    let migration = include_str!("../migrations/0009_notification_ops_indexes.sql");

    assert!(migration.contains("idx_notification_outbox_tenant_status_created"));
    assert!(migration.contains("ON notification_outbox(tenant_id, status, created_at DESC)"));
    assert!(migration.contains("idx_notification_outbox_tenant_next_attempt"));
    assert!(migration.contains("ON notification_outbox(tenant_id, next_attempt_at)"));
    assert!(migration.contains("idx_notification_deliveries_tenant_status_created"));
    assert!(migration.contains("ON notification_deliveries(tenant_id, status, created_at DESC)"));
    assert!(migration.contains("idx_notification_deliveries_tenant_channel_type_created"));
    assert!(migration.contains("ON notification_deliveries(tenant_id, channel_type, created_at DESC)"));
}
```

- [ ] **Step 2: Run test to verify RED**

Run:

```bash
cargo test -p coin-listener-storage notification_ops_indexes_migration_adds_outbox_and_delivery_indexes --manifest-path backend/Cargo.toml
```

Expected: FAIL because `0009_notification_ops_indexes.sql` does not exist.

- [ ] **Step 3: Create migration**

Create `backend/crates/storage/migrations/0009_notification_ops_indexes.sql` with exactly:

```sql
CREATE INDEX IF NOT EXISTS idx_notification_outbox_tenant_status_created
    ON notification_outbox(tenant_id, status, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_notification_outbox_tenant_next_attempt
    ON notification_outbox(tenant_id, next_attempt_at);

CREATE INDEX IF NOT EXISTS idx_notification_deliveries_tenant_status_created
    ON notification_deliveries(tenant_id, status, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_notification_deliveries_tenant_channel_type_created
    ON notification_deliveries(tenant_id, channel_type, created_at DESC);
```

- [ ] **Step 4: Run test to verify GREEN**

Run:

```bash
cargo test -p coin-listener-storage notification_ops_indexes_migration_adds_outbox_and_delivery_indexes --manifest-path backend/Cargo.toml
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 5: Commit Task 2**

Run:

```bash
git status --short
git add backend/crates/storage/src/repositories.rs backend/crates/storage/migrations/0009_notification_ops_indexes.sql
git commit -m "Add notification operations indexes"
```

Expected: commit succeeds and contains only the migration plus its string test.

---

### Task 3: Add outbox operations storage helpers

**Files:**
- Modify: `backend/crates/storage/src/repositories.rs:1-220`
- Modify: `backend/crates/storage/src/repositories.rs:631-706`
- Modify: `backend/crates/storage/src/repositories.rs:978-1233`
- Test: `backend/crates/storage/src/repositories.rs` existing `#[cfg(test)] mod tests`

- [ ] **Step 1: Write failing outbox helper tests**

In the `use coin_listener_core::models` import inside the `repositories.rs` test module, replace:

```rust
models::{AddressEvent, AddressEventDraft, NotificationOutboxItem},
```

with:

```rust
models::{
    AddressEvent, AddressEventDraft, NotificationOutboxDetail, NotificationOutboxItem,
    NotificationOutboxListItem, NotificationOutboxQuery, OutboxStatusCounts,
},
```

Add these constants to the `use super::{ ... }` list in the test module:

```rust
GET_NOTIFICATION_OUTBOX_ITEM_QUERY, LIST_NOTIFICATION_OUTBOX_QUERY,
MANUAL_RETRY_NOTIFICATION_OUTBOX_QUERY, NOTIFICATION_OUTBOX_STATUS_COUNTS_QUERY,
SELECT_NOTIFICATION_OUTBOX_STATUS_QUERY,
```

Add these tests after `notification_outbox_stale_release_only_matches_stale_processing_rows`:

```rust
#[test]
fn notification_outbox_ops_validates_status_and_retryability() {
    for status in ["pending", "processing", "retryable", "delivered", "failed"] {
        assert!(super::validate_notification_outbox_status(status).is_ok(), "{status}");
    }
    assert!(super::validate_notification_outbox_status("unknown").is_err());
    assert!(super::notification_outbox_status_allows_manual_retry("failed"));
    assert!(super::notification_outbox_status_allows_manual_retry("retryable"));
    assert!(!super::notification_outbox_status_allows_manual_retry("pending"));
    assert!(!super::notification_outbox_status_allows_manual_retry("processing"));
    assert!(!super::notification_outbox_status_allows_manual_retry("delivered"));
}

#[test]
fn notification_ops_pagination_defaults_and_clamps() {
    let default_query = NotificationOutboxQuery {
        status: None,
        event_id: None,
        limit: None,
        offset: None,
    };
    assert_eq!(super::notification_ops_limit(default_query.limit), 50);
    assert_eq!(super::notification_ops_offset(default_query.offset), 0);
    assert_eq!(super::notification_ops_limit(Some(0)), 1);
    assert_eq!(super::notification_ops_limit(Some(500)), 100);
    assert_eq!(super::notification_ops_offset(Some(-10)), 0);
    assert_eq!(super::notification_ops_offset(Some(25)), 25);
}

#[test]
fn notification_outbox_list_query_joins_events_and_delivery_counts() {
    assert!(LIST_NOTIFICATION_OUTBOX_QUERY.contains("FROM notification_outbox o"));
    assert!(LIST_NOTIFICATION_OUTBOX_QUERY.contains("LEFT JOIN address_events ae"));
    assert!(LIST_NOTIFICATION_OUTBOX_QUERY.contains("notification_deliveries"));
    assert!(LIST_NOTIFICATION_OUTBOX_QUERY.contains("COUNT(nd.id) AS delivery_total"));
    assert!(LIST_NOTIFICATION_OUTBOX_QUERY.contains("delivery_sent"));
    assert!(LIST_NOTIFICATION_OUTBOX_QUERY.contains("delivery_failed"));
    assert!(LIST_NOTIFICATION_OUTBOX_QUERY.contains("delivery_skipped"));
    assert!(LIST_NOTIFICATION_OUTBOX_QUERY.contains("$2::text IS NULL OR o.status = $2"));
    assert!(LIST_NOTIFICATION_OUTBOX_QUERY.contains("$6::uuid IS NULL OR o.event_id = $6"));
    assert!(LIST_NOTIFICATION_OUTBOX_QUERY.contains("LIMIT $3 OFFSET $4"));
    assert!(LIST_NOTIFICATION_OUTBOX_QUERY.contains("locked_at < $5"));
}

#[test]
fn notification_outbox_detail_and_retry_queries_are_scoped_and_safe() {
    assert!(GET_NOTIFICATION_OUTBOX_ITEM_QUERY.contains("WHERE o.tenant_id = $1"));
    assert!(GET_NOTIFICATION_OUTBOX_ITEM_QUERY.contains("AND o.id = $2"));
    assert!(SELECT_NOTIFICATION_OUTBOX_STATUS_QUERY.contains("WHERE id = $1"));
    assert!(SELECT_NOTIFICATION_OUTBOX_STATUS_QUERY.contains("AND tenant_id = $2"));
    assert!(MANUAL_RETRY_NOTIFICATION_OUTBOX_QUERY.contains("status = 'retryable'"));
    assert!(MANUAL_RETRY_NOTIFICATION_OUTBOX_QUERY.contains("next_attempt_at = $2"));
    assert!(MANUAL_RETRY_NOTIFICATION_OUTBOX_QUERY.contains("locked_at = NULL"));
    assert!(MANUAL_RETRY_NOTIFICATION_OUTBOX_QUERY.contains("locked_by = NULL"));
    assert!(MANUAL_RETRY_NOTIFICATION_OUTBOX_QUERY.contains("last_error = NULL"));
    assert!(!MANUAL_RETRY_NOTIFICATION_OUTBOX_QUERY.contains("attempt_count = 0"));
}

#[test]
fn notification_outbox_status_counts_query_counts_backlog_and_next_due() {
    assert!(NOTIFICATION_OUTBOX_STATUS_COUNTS_QUERY.contains("status = 'pending'"));
    assert!(NOTIFICATION_OUTBOX_STATUS_COUNTS_QUERY.contains("status = 'retryable'"));
    assert!(NOTIFICATION_OUTBOX_STATUS_COUNTS_QUERY.contains("status = 'processing'"));
    assert!(NOTIFICATION_OUTBOX_STATUS_COUNTS_QUERY.contains("status = 'failed'"));
    assert!(NOTIFICATION_OUTBOX_STATUS_COUNTS_QUERY.contains("locked_at < $2"));
    assert!(NOTIFICATION_OUTBOX_STATUS_COUNTS_QUERY.contains("MIN(next_attempt_at)"));
    assert!(NOTIFICATION_OUTBOX_STATUS_COUNTS_QUERY.contains("next_attempt_at <= $1"));
}

#[allow(dead_code)]
async fn assert_list_notification_outbox_signature(
    pool: &PgPool,
    query: NotificationOutboxQuery,
    stale_before: chrono::DateTime<Utc>,
) -> AppResult<Vec<NotificationOutboxListItem>> {
    super::list_notification_outbox(pool, query, stale_before).await
}

#[allow(dead_code)]
async fn assert_get_notification_outbox_detail_signature(
    pool: &PgPool,
    id: uuid::Uuid,
    stale_before: chrono::DateTime<Utc>,
) -> AppResult<NotificationOutboxDetail> {
    super::get_notification_outbox_detail(pool, id, stale_before).await
}

#[allow(dead_code)]
async fn assert_retry_notification_outbox_signature(
    pool: &PgPool,
    id: uuid::Uuid,
    now: chrono::DateTime<Utc>,
) -> AppResult<NotificationOutboxItem> {
    super::retry_notification_outbox(pool, id, now).await
}

#[allow(dead_code)]
async fn assert_notification_outbox_status_counts_signature(
    pool: &PgPool,
    now: chrono::DateTime<Utc>,
    stale_before: chrono::DateTime<Utc>,
) -> AppResult<OutboxStatusCounts> {
    super::notification_outbox_status_counts(pool, now, stale_before).await
}

#[test]
fn notification_outbox_ops_helper_signatures_are_stable() {
    let _ = assert_list_notification_outbox_signature;
    let _ = assert_get_notification_outbox_detail_signature;
    let _ = assert_retry_notification_outbox_signature;
    let _ = assert_notification_outbox_status_counts_signature;
}
```

- [ ] **Step 2: Run tests to verify RED**

Run:

```bash
cargo test -p coin-listener-storage notification_outbox_ops_validates_status_and_retryability --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage notification_ops_pagination_defaults_and_clamps --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage notification_outbox_list_query_joins_events_and_delivery_counts --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage notification_outbox_detail_and_retry_queries_are_scoped_and_safe --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage notification_outbox_status_counts_query_counts_backlog_and_next_due --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage notification_outbox_ops_helper_signatures_are_stable --manifest-path backend/Cargo.toml
```

Expected: FAIL because the constants, validation helpers, and storage functions do not exist.

- [ ] **Step 3: Add outbox query constants and validation helpers**

In `backend/crates/storage/src/repositories.rs`, update the model import list at the top to include the operations DTOs:

```rust
models::{
    AddressEvent, AddressEventDraft, Asset, BalanceSnapshot, Chain,
    CreateBalanceSnapshotRequest, CreateProviderRequest, CreateWatchedAddressRequest,
    EventQuery, NotificationOutboxDetail, NotificationOutboxItem, NotificationOutboxListItem,
    NotificationOutboxQuery, OutboxStatusCounts, Provider, ScanAddressCandidate,
    ScanAddressContext, ScanCursor, Tenant, User, WatchedAddress,
},
```

Add these constants after `RELEASE_STALE_NOTIFICATION_OUTBOX_QUERY`:

```rust
pub const LIST_NOTIFICATION_OUTBOX_QUERY: &str = r#"
WITH delivery_counts AS (
    SELECT tenant_id,
           event_id,
           COUNT(nd.id) AS delivery_total,
           COUNT(nd.id) FILTER (WHERE nd.status = 'sent') AS delivery_sent,
           COUNT(nd.id) FILTER (WHERE nd.status = 'failed') AS delivery_failed,
           COUNT(nd.id) FILTER (WHERE nd.status = 'skipped') AS delivery_skipped
    FROM notification_deliveries nd
    WHERE nd.tenant_id = $1
    GROUP BY tenant_id, event_id
)
SELECT o.id,
       o.tenant_id,
       o.event_id,
       o.status,
       o.attempt_count,
       o.next_attempt_at,
       o.locked_at,
       o.locked_by,
       o.last_error,
       o.delivered_at,
       o.created_at,
       o.updated_at,
       ae.event_type,
       ae.direction,
       ae.tx_hash,
       COALESCE(dc.delivery_total, 0) AS delivery_total,
       COALESCE(dc.delivery_sent, 0) AS delivery_sent,
       COALESCE(dc.delivery_failed, 0) AS delivery_failed,
       COALESCE(dc.delivery_skipped, 0) AS delivery_skipped,
       (o.status = 'processing' AND o.locked_at IS NOT NULL AND o.locked_at < $5) AS is_stale_processing
FROM notification_outbox o
LEFT JOIN address_events ae ON ae.id = o.event_id AND ae.tenant_id = o.tenant_id
LEFT JOIN delivery_counts dc ON dc.tenant_id = o.tenant_id AND dc.event_id = o.event_id
WHERE o.tenant_id = $1
  AND ($2::text IS NULL OR o.status = $2)
  AND ($6::uuid IS NULL OR o.event_id = $6)
ORDER BY o.created_at DESC
LIMIT $3 OFFSET $4
"#;

pub const GET_NOTIFICATION_OUTBOX_ITEM_QUERY: &str = r#"
WITH delivery_counts AS (
    SELECT tenant_id,
           event_id,
           COUNT(nd.id) AS delivery_total,
           COUNT(nd.id) FILTER (WHERE nd.status = 'sent') AS delivery_sent,
           COUNT(nd.id) FILTER (WHERE nd.status = 'failed') AS delivery_failed,
           COUNT(nd.id) FILTER (WHERE nd.status = 'skipped') AS delivery_skipped
    FROM notification_deliveries nd
    WHERE nd.tenant_id = $1
    GROUP BY tenant_id, event_id
)
SELECT o.id,
       o.tenant_id,
       o.event_id,
       o.status,
       o.attempt_count,
       o.next_attempt_at,
       o.locked_at,
       o.locked_by,
       o.last_error,
       o.delivered_at,
       o.created_at,
       o.updated_at,
       ae.event_type,
       ae.direction,
       ae.tx_hash,
       COALESCE(dc.delivery_total, 0) AS delivery_total,
       COALESCE(dc.delivery_sent, 0) AS delivery_sent,
       COALESCE(dc.delivery_failed, 0) AS delivery_failed,
       COALESCE(dc.delivery_skipped, 0) AS delivery_skipped,
       (o.status = 'processing' AND o.locked_at IS NOT NULL AND o.locked_at < $3) AS is_stale_processing
FROM notification_outbox o
LEFT JOIN address_events ae ON ae.id = o.event_id AND ae.tenant_id = o.tenant_id
LEFT JOIN delivery_counts dc ON dc.tenant_id = o.tenant_id AND dc.event_id = o.event_id
WHERE o.tenant_id = $1
  AND o.id = $2
"#;

pub const SELECT_NOTIFICATION_OUTBOX_STATUS_QUERY: &str = r#"
SELECT status
FROM notification_outbox
WHERE id = $1
  AND tenant_id = $2
"#;

pub const MANUAL_RETRY_NOTIFICATION_OUTBOX_QUERY: &str = r#"
UPDATE notification_outbox
SET status = 'retryable',
    next_attempt_at = $2,
    locked_at = NULL,
    locked_by = NULL,
    last_error = NULL,
    updated_at = NOW()
WHERE id = $1
  AND tenant_id = $3
RETURNING id, tenant_id, event_id, status, attempt_count,
          next_attempt_at, locked_at, locked_by, last_error,
          delivered_at, created_at, updated_at
"#;

pub const NOTIFICATION_OUTBOX_STATUS_COUNTS_QUERY: &str = r#"
SELECT COUNT(*) FILTER (WHERE status = 'pending') AS pending,
       COUNT(*) FILTER (WHERE status = 'retryable') AS retryable,
       COUNT(*) FILTER (WHERE status = 'processing') AS processing,
       COUNT(*) FILTER (WHERE status = 'failed') AS failed,
       COUNT(*) FILTER (
           WHERE status = 'processing'
             AND locked_at IS NOT NULL
             AND locked_at < $2
       ) AS stale_processing,
       MIN(next_attempt_at) FILTER (
           WHERE status IN ('pending', 'retryable')
             AND next_attempt_at <= $1
       ) AS next_due_at
FROM notification_outbox
WHERE tenant_id = $3
"#;
```

Add these helpers after the constants:

```rust
pub fn validate_notification_outbox_status(status: &str) -> AppResult<()> {
    if !matches!(
        status,
        "pending" | "processing" | "retryable" | "delivered" | "failed"
    ) {
        return Err(AppError::Validation(
            "outbox status must be pending, processing, retryable, delivered, or failed".to_string(),
        ));
    }
    Ok(())
}

pub fn notification_outbox_status_allows_manual_retry(status: &str) -> bool {
    matches!(status, "failed" | "retryable")
}

pub fn notification_ops_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(50).clamp(1, 100)
}

pub fn notification_ops_offset(offset: Option<i64>) -> i64 {
    offset.unwrap_or(0).max(0)
}
```

- [ ] **Step 4: Add outbox storage functions**

Add these functions after `release_stale_notification_outbox`:

```rust
pub async fn list_notification_outbox(
    pool: &PgPool,
    query: NotificationOutboxQuery,
    stale_before: DateTime<Utc>,
) -> AppResult<Vec<NotificationOutboxListItem>> {
    if let Some(status) = query.status.as_deref() {
        validate_notification_outbox_status(status)?;
    }

    sqlx::query_as::<_, NotificationOutboxListItem>(LIST_NOTIFICATION_OUTBOX_QUERY)
        .bind(DEFAULT_TENANT_ID)
        .bind(query.status)
        .bind(notification_ops_limit(query.limit))
        .bind(notification_ops_offset(query.offset))
        .bind(stale_before)
        .bind(query.event_id)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn get_notification_outbox_detail(
    pool: &PgPool,
    id: Uuid,
    stale_before: DateTime<Utc>,
) -> AppResult<NotificationOutboxDetail> {
    let outbox = sqlx::query_as::<_, NotificationOutboxListItem>(GET_NOTIFICATION_OUTBOX_ITEM_QUERY)
        .bind(DEFAULT_TENANT_ID)
        .bind(id)
        .bind(stale_before)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("notification outbox".to_string()))?;

    let event = get_address_event(pool, outbox.event_id, outbox.tenant_id).await?;
    let deliveries = crate::notifications::list_notification_deliveries_for_event(pool, outbox.event_id).await?;

    Ok(NotificationOutboxDetail {
        outbox,
        event,
        deliveries,
    })
}

pub async fn retry_notification_outbox(
    pool: &PgPool,
    id: Uuid,
    now: DateTime<Utc>,
) -> AppResult<NotificationOutboxItem> {
    let status = sqlx::query_scalar::<_, String>(SELECT_NOTIFICATION_OUTBOX_STATUS_QUERY)
        .bind(id)
        .bind(DEFAULT_TENANT_ID)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("notification outbox".to_string()))?;

    if !notification_outbox_status_allows_manual_retry(&status) {
        return Err(AppError::Validation(
            "only failed or retryable notification outbox rows can be retried".to_string(),
        ));
    }

    sqlx::query_as::<_, NotificationOutboxItem>(MANUAL_RETRY_NOTIFICATION_OUTBOX_QUERY)
        .bind(id)
        .bind(now)
        .bind(DEFAULT_TENANT_ID)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("notification outbox".to_string()))
}

pub async fn notification_outbox_status_counts(
    pool: &PgPool,
    now: DateTime<Utc>,
    stale_before: DateTime<Utc>,
) -> AppResult<OutboxStatusCounts> {
    sqlx::query_as::<_, OutboxStatusCounts>(NOTIFICATION_OUTBOX_STATUS_COUNTS_QUERY)
        .bind(now)
        .bind(stale_before)
        .bind(DEFAULT_TENANT_ID)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}
```

If this does not compile because `crate::notifications::list_notification_deliveries_for_event` does not exist yet, leave the call in place and continue to Task 4 before running full `cargo check`.

- [ ] **Step 5: Run focused tests to verify implemented pieces**

Run:

```bash
cargo test -p coin-listener-storage notification_outbox_ops_validates_status_and_retryability --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage notification_ops_pagination_defaults_and_clamps --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage notification_outbox_list_query_joins_events_and_delivery_counts --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage notification_outbox_detail_and_retry_queries_are_scoped_and_safe --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage notification_outbox_status_counts_query_counts_backlog_and_next_due --manifest-path backend/Cargo.toml
```

Expected: all focused string/helper tests exit 0. `notification_outbox_ops_helper_signatures_are_stable` may still fail until Task 4 adds the delivery function used by detail.

- [ ] **Step 6: Commit Task 3**

Run:

```bash
git status --short
git add backend/crates/storage/src/repositories.rs
git commit -m "Add notification outbox operations helpers"
```

Expected: commit succeeds and only `backend/crates/storage/src/repositories.rs` is staged.

---

### Task 4: Add delivery operations storage helpers

**Files:**
- Modify: `backend/crates/storage/src/notifications.rs:1-220`
- Modify: `backend/crates/storage/src/notifications.rs:750-903`
- Modify: `backend/crates/storage/src/notifications.rs:1057-1289`
- Test: `backend/crates/storage/src/notifications.rs` existing `#[cfg(test)] mod tests`
- Test: `backend/crates/storage/src/repositories.rs` existing `notification_outbox_ops_helper_signatures_are_stable`

- [ ] **Step 1: Write failing delivery helper tests**

In `backend/crates/storage/src/notifications.rs`, update the model import near the top from:

```rust
AddressEvent, CreateNotificationChannelRequest, CreateNotificationRuleRequest,
InAppNotification, InAppNotificationQuery, NotificationChannel, NotificationDelivery,
NotificationRule,
```

to:

```rust
AddressEvent, CreateNotificationChannelRequest, CreateNotificationRuleRequest,
InAppNotification, InAppNotificationQuery, NotificationChannel, NotificationDelivery,
NotificationDeliveryListItem, NotificationDeliveryQuery, NotificationRule,
```

Add these constants to the test module `use super::{ ... }` list:

```rust
LIST_NOTIFICATION_DELIVERIES_FOR_EVENT_QUERY, LIST_NOTIFICATION_DELIVERIES_QUERY,
```

Update the test module model import from:

```rust
models::{CreateNotificationChannelRequest, CreateNotificationRuleRequest},
```

to:

```rust
models::{
    CreateNotificationChannelRequest, CreateNotificationRuleRequest, NotificationDeliveryListItem,
    NotificationDeliveryQuery,
},
```

Add `AppResult` to the `coin_listener_core` import:

```rust
AppError, AppResult,
```

Add these tests before `rule_reference_consistency_rejects_address_chain_mismatch`:

```rust
#[test]
fn delivery_ops_validates_channel_type_filter() {
    for channel_type in ["in_app", "telegram", "webhook"] {
        assert!(super::validate_notification_delivery_channel_type(channel_type).is_ok());
    }
    assert!(super::validate_notification_delivery_channel_type("email").is_err());
}

#[test]
fn delivery_ops_pagination_defaults_and_clamps() {
    let default_query = NotificationDeliveryQuery {
        event_id: None,
        status: None,
        channel_type: None,
        rule_id: None,
        channel_id: None,
        limit: None,
        offset: None,
    };
    assert_eq!(super::notification_delivery_ops_limit(default_query.limit), 50);
    assert_eq!(super::notification_delivery_ops_offset(default_query.offset), 0);
    assert_eq!(super::notification_delivery_ops_limit(Some(0)), 1);
    assert_eq!(super::notification_delivery_ops_limit(Some(500)), 100);
    assert_eq!(super::notification_delivery_ops_offset(Some(-10)), 0);
    assert_eq!(super::notification_delivery_ops_offset(Some(25)), 25);
}

#[test]
fn delivery_ops_list_query_filters_metadata_and_orders_newest_first() {
    assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("FROM notification_deliveries"));
    assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("tenant_id = $1"));
    assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("$2::uuid IS NULL OR event_id = $2"));
    assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("$3::text IS NULL OR status = $3"));
    assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("$4::text IS NULL OR channel_type = $4"));
    assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("$5::uuid IS NULL OR rule_id = $5"));
    assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("$6::uuid IS NULL OR channel_id = $6"));
    assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("provider_message_id"));
    assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("provider_status_code"));
    assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("provider_response"));
    assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("ORDER BY created_at DESC"));
    assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("LIMIT $7 OFFSET $8"));
}

#[test]
fn delivery_ops_event_query_is_event_scoped() {
    assert!(LIST_NOTIFICATION_DELIVERIES_FOR_EVENT_QUERY.contains("tenant_id = $1"));
    assert!(LIST_NOTIFICATION_DELIVERIES_FOR_EVENT_QUERY.contains("event_id = $2"));
    assert!(LIST_NOTIFICATION_DELIVERIES_FOR_EVENT_QUERY.contains("ORDER BY created_at DESC"));
}

#[allow(dead_code)]
async fn assert_list_notification_deliveries_signature(
    pool: &PgPool,
    query: NotificationDeliveryQuery,
) -> AppResult<Vec<NotificationDeliveryListItem>> {
    super::list_notification_deliveries(pool, query).await
}

#[allow(dead_code)]
async fn assert_list_notification_deliveries_for_event_signature(
    pool: &PgPool,
    event_id: uuid::Uuid,
) -> AppResult<Vec<NotificationDeliveryListItem>> {
    super::list_notification_deliveries_for_event(pool, event_id).await
}

#[test]
fn delivery_ops_helper_signatures_are_stable() {
    let _ = assert_list_notification_deliveries_signature;
    let _ = assert_list_notification_deliveries_for_event_signature;
}
```

- [ ] **Step 2: Run tests to verify RED**

Run:

```bash
cargo test -p coin-listener-storage delivery_ops_validates_channel_type_filter --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage delivery_ops_pagination_defaults_and_clamps --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage delivery_ops_list_query_filters_metadata_and_orders_newest_first --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage delivery_ops_event_query_is_event_scoped --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage delivery_ops_helper_signatures_are_stable --manifest-path backend/Cargo.toml
```

Expected: FAIL because the delivery operations constants, helpers, and functions do not exist.

- [ ] **Step 3: Add delivery validation helpers and query constants**

Add these constants after `MARK_EXTERNAL_NOTIFICATION_DELIVERY_FAILED_QUERY`:

```rust
pub const LIST_NOTIFICATION_DELIVERIES_QUERY: &str = r#"
SELECT id,
       tenant_id,
       event_id,
       rule_id,
       channel_id,
       channel_type,
       status,
       attempt_count,
       last_error,
       sent_at,
       created_at,
       idempotency_key,
       provider_message_id,
       provider_status_code,
       provider_response
FROM notification_deliveries
WHERE tenant_id = $1
  AND ($2::uuid IS NULL OR event_id = $2)
  AND ($3::text IS NULL OR status = $3)
  AND ($4::text IS NULL OR channel_type = $4)
  AND ($5::uuid IS NULL OR rule_id = $5)
  AND ($6::uuid IS NULL OR channel_id = $6)
ORDER BY created_at DESC
LIMIT $7 OFFSET $8
"#;

pub const LIST_NOTIFICATION_DELIVERIES_FOR_EVENT_QUERY: &str = r#"
SELECT id,
       tenant_id,
       event_id,
       rule_id,
       channel_id,
       channel_type,
       status,
       attempt_count,
       last_error,
       sent_at,
       created_at,
       idempotency_key,
       provider_message_id,
       provider_status_code,
       provider_response
FROM notification_deliveries
WHERE tenant_id = $1
  AND event_id = $2
ORDER BY created_at DESC
"#;
```

Add these helpers after `validate_notification_delivery_status`:

```rust
pub fn validate_notification_delivery_channel_type(channel_type: &str) -> AppResult<()> {
    if !matches!(
        channel_type,
        CHANNEL_TYPE_IN_APP | CHANNEL_TYPE_TELEGRAM | CHANNEL_TYPE_WEBHOOK
    ) {
        return Err(AppError::Validation(
            "channel_type must be in_app, telegram, or webhook".to_string(),
        ));
    }
    Ok(())
}

pub fn notification_delivery_ops_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(50).clamp(1, 100)
}

pub fn notification_delivery_ops_offset(offset: Option<i64>) -> i64 {
    offset.unwrap_or(0).max(0)
}
```

- [ ] **Step 4: Add delivery list functions**

Add these functions after `mark_external_notification_delivery_failed`:

```rust
pub async fn list_notification_deliveries(
    pool: &PgPool,
    query: NotificationDeliveryQuery,
) -> AppResult<Vec<NotificationDeliveryListItem>> {
    if let Some(status) = query.status.as_deref() {
        validate_notification_delivery_status(status)?;
    }
    if let Some(channel_type) = query.channel_type.as_deref() {
        validate_notification_delivery_channel_type(channel_type)?;
    }

    sqlx::query_as::<_, NotificationDeliveryListItem>(LIST_NOTIFICATION_DELIVERIES_QUERY)
        .bind(DEFAULT_TENANT_ID)
        .bind(query.event_id)
        .bind(query.status)
        .bind(query.channel_type)
        .bind(query.rule_id)
        .bind(query.channel_id)
        .bind(notification_delivery_ops_limit(query.limit))
        .bind(notification_delivery_ops_offset(query.offset))
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn list_notification_deliveries_for_event(
    pool: &PgPool,
    event_id: Uuid,
) -> AppResult<Vec<NotificationDeliveryListItem>> {
    sqlx::query_as::<_, NotificationDeliveryListItem>(LIST_NOTIFICATION_DELIVERIES_FOR_EVENT_QUERY)
        .bind(DEFAULT_TENANT_ID)
        .bind(event_id)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}
```

- [ ] **Step 5: Run tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-storage delivery_ops_validates_channel_type_filter --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage delivery_ops_pagination_defaults_and_clamps --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage delivery_ops_list_query_filters_metadata_and_orders_newest_first --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage delivery_ops_event_query_is_event_scoped --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage delivery_ops_helper_signatures_are_stable --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage notification_outbox_ops_helper_signatures_are_stable --manifest-path backend/Cargo.toml
cargo check -p coin-listener-storage --manifest-path backend/Cargo.toml
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 6: Commit Task 4**

Run:

```bash
git status --short
git add backend/crates/storage/src/notifications.rs backend/crates/storage/src/repositories.rs
git commit -m "Add notification delivery operations helpers"
```

Expected: commit succeeds. `backend/crates/storage/src/repositories.rs` is staged only if Task 3 left compile-facing signature imports or formatting changes that became valid after this task.

---

### Task 5: Extend notification system status with outbox counts

**Files:**
- Modify: `backend/crates/storage/src/system_status.rs:1-166`
- Test: `backend/crates/storage/src/system_status.rs` existing `#[cfg(test)] mod tests`
- Test: `backend/crates/core/src/models.rs` existing `system_status_round_trips_as_json`

- [ ] **Step 1: Write failing system-status tests**

In `backend/crates/storage/src/system_status.rs`, update the test import list from:

```rust
use crate::system_status::{
    EVENT_STATUS_QUERY, NOTIFICATION_STATUS_QUERY, PROVIDER_CHAIN_STATUS_QUERY,
    PROVIDER_ITEMS_QUERY, SCAN_STATUS_QUERY,
};
```

to:

```rust
use crate::system_status::{
    EVENT_STATUS_QUERY, NOTIFICATION_STATUS_QUERY, NOTIFICATION_STATUS_STALE_MINUTES,
    PROVIDER_CHAIN_STATUS_QUERY, PROVIDER_ITEMS_QUERY, SCAN_STATUS_QUERY,
};
```

Replace `notification_status_query_counts_delivery_statuses_and_unread` with:

```rust
#[test]
fn notification_status_query_counts_delivery_statuses_and_unread() {
    assert!(NOTIFICATION_STATUS_QUERY.contains("status = 'sent'"));
    assert!(NOTIFICATION_STATUS_QUERY.contains("status = 'skipped'"));
    assert!(NOTIFICATION_STATUS_QUERY.contains("status = 'failed'"));
    assert!(NOTIFICATION_STATUS_QUERY.contains("read_at IS NULL"));
}

#[test]
fn notification_status_uses_fifteen_minute_stale_outbox_window() {
    assert_eq!(NOTIFICATION_STATUS_STALE_MINUTES, 15);
}
```

- [ ] **Step 2: Run tests to verify RED**

Run:

```bash
cargo test -p coin-listener-storage notification_status_uses_fifteen_minute_stale_outbox_window --manifest-path backend/Cargo.toml
cargo test -p coin-listener-core system_status_round_trips_as_json --manifest-path backend/Cargo.toml
```

Expected: the storage test FAILS because `NOTIFICATION_STATUS_STALE_MINUTES` does not exist. The core test may already pass from Task 1 and confirms `NotificationStatus` now serializes outbox counts.

- [ ] **Step 3: Call outbox count helper from system status**

In `backend/crates/storage/src/system_status.rs`, change the imports at the top to:

```rust
use chrono::{DateTime, Duration, Utc};
use coin_listener_core::{
    models::{
        EventStatus, NotificationStatus, ProviderChainStatus, ProviderStatus, ProviderStatusItem,
        ScanStatus,
    },
    AppError, AppResult,
};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::repositories;

pub const NOTIFICATION_STATUS_STALE_MINUTES: i64 = 15;
```

Leave `NOTIFICATION_STATUS_QUERY` focused on 24h delivery and unread in-app counts. Update `system_notification_status` to fetch outbox counts:

```rust
pub async fn system_notification_status(pool: &PgPool) -> AppResult<NotificationStatus> {
    let row = sqlx::query_as::<_, NotificationStatusRow>(NOTIFICATION_STATUS_QUERY)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let now = Utc::now();
    let outbox = repositories::notification_outbox_status_counts(
        pool,
        now,
        now - Duration::minutes(NOTIFICATION_STATUS_STALE_MINUTES),
    )
    .await?;

    Ok(NotificationStatus {
        last_24h_sent: row.last_24h_sent,
        last_24h_skipped: row.last_24h_skipped,
        last_24h_failed: row.last_24h_failed,
        unread_in_app: row.unread_in_app,
        outbox,
    })
}
```

- [ ] **Step 4: Run tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-storage notification_status_query_counts_delivery_statuses_and_unread --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage notification_status_uses_fifteen_minute_stale_outbox_window --manifest-path backend/Cargo.toml
cargo test -p coin-listener-core system_status_round_trips_as_json --manifest-path backend/Cargo.toml
cargo check -p coin-listener-storage --manifest-path backend/Cargo.toml
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 5: Commit Task 5**

Run:

```bash
git status --short
git add backend/crates/storage/src/system_status.rs backend/crates/core/src/models.rs
git commit -m "Add notification outbox status counts"
```

Expected: commit succeeds. `backend/crates/core/src/models.rs` is staged only if this task adjusted its test after Task 1.

---

### Task 6: Add notification operations API routes

**Files:**
- Modify: `backend/crates/api-server/src/routes.rs:1-529`
- Test: `backend/crates/api-server/src/routes.rs` existing `#[cfg(test)] mod tests`

- [ ] **Step 1: Write failing route tests**

In `backend/crates/api-server/src/routes.rs`, add this test after `router_exposes_notification_routes`:

```rust
#[tokio::test]
async fn router_exposes_notification_operations_routes() {
    let app = build_router(Arc::new(ApiState {
        postgres: PgPool::connect_lazy("postgres://postgres:postgres@localhost/coin_listener_test")
            .expect("valid postgres url"),
        redis: None,
        scan_queue_key: "scan:address:queue".to_string(),
        notify_queue_key: "notify:event:queue".to_string(),
        enable_dev_routes: true,
    }));

    for (method, uri, status) in [
        (
            Method::GET,
            "/api/notification-outbox?status=unknown",
            StatusCode::BAD_REQUEST,
        ),
        (
            Method::PUT,
            "/api/notification-outbox",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            Method::GET,
            "/api/notification-outbox/not-a-uuid",
            StatusCode::BAD_REQUEST,
        ),
        (
            Method::POST,
            "/api/notification-outbox/not-a-uuid/retry",
            StatusCode::BAD_REQUEST,
        ),
        (
            Method::POST,
            "/api/notification-deliveries",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            Method::GET,
            "/api/notification-deliveries?status=unknown",
            StatusCode::BAD_REQUEST,
        ),
        (
            Method::GET,
            "/api/notification-deliveries?channel_type=email",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), status, "{uri}");
    }
}
```

- [ ] **Step 2: Run test to verify RED**

Run:

```bash
cargo test -p api-server router_exposes_notification_operations_routes --manifest-path backend/Cargo.toml
```

Expected: FAIL because the new routes and handlers do not exist.

- [ ] **Step 3: Add imports, route registrations, and stale helper**

Update the `coin_listener_core::models` import in `routes.rs` to include the operations DTOs:

```rust
CreateNotificationChannelRequest, CreateNotificationRuleRequest, CreateProviderRequest,
CreateWatchedAddressRequest, EventQuery, InAppNotificationQuery, LoginRequest,
LoginResponse, NotificationDeliveryListResponse, NotificationDeliveryQuery,
NotificationOutboxListResponse, NotificationOutboxQuery, QueueStatus,
RetryNotificationOutboxResponse, SystemStatus, UserSummary,
```

Change the chrono import to:

```rust
use chrono::{Duration, Utc};
```

Add this helper near `queue_status`:

```rust
fn notification_ops_stale_before() -> chrono::DateTime<Utc> {
    Utc::now() - Duration::minutes(system_status::NOTIFICATION_STATUS_STALE_MINUTES)
}
```

Add these route registrations after `/api/in-app-notifications/:id/read`:

```rust
.route("/api/notification-outbox", get(list_notification_outbox))
.route("/api/notification-outbox/:id", get(get_notification_outbox))
.route(
    "/api/notification-outbox/:id/retry",
    post(retry_notification_outbox),
)
.route("/api/notification-deliveries", get(list_notification_deliveries))
```

- [ ] **Step 4: Add API handlers**

Add these handlers after `mark_in_app_notification_read`:

```rust
async fn list_notification_outbox(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<NotificationOutboxQuery>,
) -> Result<Response, ApiError> {
    let limit = repositories::notification_ops_limit(query.limit);
    let offset = repositories::notification_ops_offset(query.offset);
    let items = repositories::list_notification_outbox(
        &state.postgres,
        query,
        notification_ops_stale_before(),
    )
    .await?;

    Ok(Json(NotificationOutboxListResponse {
        items,
        limit,
        offset,
    })
    .into_response())
}

async fn get_notification_outbox(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let detail = repositories::get_notification_outbox_detail(
        &state.postgres,
        id,
        notification_ops_stale_before(),
    )
    .await?;
    Ok(Json(detail).into_response())
}

async fn retry_notification_outbox(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let outbox = repositories::retry_notification_outbox(&state.postgres, id, Utc::now()).await?;
    Ok(Json(RetryNotificationOutboxResponse { outbox }).into_response())
}

async fn list_notification_deliveries(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<NotificationDeliveryQuery>,
) -> Result<Response, ApiError> {
    let limit = notifications::notification_delivery_ops_limit(query.limit);
    let offset = notifications::notification_delivery_ops_offset(query.offset);
    let items = notifications::list_notification_deliveries(&state.postgres, query).await?;

    Ok(Json(NotificationDeliveryListResponse {
        items,
        limit,
        offset,
    })
    .into_response())
}
```

- [ ] **Step 5: Run tests to verify GREEN**

Run:

```bash
cargo test -p api-server router_exposes_notification_operations_routes --manifest-path backend/Cargo.toml
cargo test -p api-server router_exposes_notification_routes --manifest-path backend/Cargo.toml
cargo check -p api-server --manifest-path backend/Cargo.toml
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 6: Commit Task 6**

Run:

```bash
git status --short
git add backend/crates/api-server/src/routes.rs
git commit -m "Expose notification operations API routes"
```

Expected: commit succeeds and only `backend/crates/api-server/src/routes.rs` is staged.

---

### Task 7: Add frontend notification operations API types and client

**Files:**
- Modify: `frontend/src/api/types.ts:155-232`
- Modify: `frontend/src/api/client.ts:1-164`
- Test: TypeScript compile via `npm run build --prefix frontend`

- [ ] **Step 1: Write failing client contract**

In `frontend/src/api/client.ts`, update the import list to include types that do not exist yet:

```ts
NotificationDeliveryListResponse,
NotificationDeliveryQuery,
NotificationOutboxDetail,
NotificationOutboxListResponse,
NotificationOutboxQuery,
RetryNotificationOutboxResponse,
```

Add this query helper after `request`:

```ts
function buildQuery(filters: object): string {
  const params = new URLSearchParams();
  Object.entries(filters).forEach(([key, value]) => {
    if (value !== undefined && value !== null && value !== '') {
      params.set(key, String(value));
    }
  });
  const query = params.toString();
  return query ? `?${query}` : '';
}
```

Add these functions after `markInAppNotificationRead`:

```ts
export function listNotificationOutbox(filters: NotificationOutboxQuery = {}): Promise<NotificationOutboxListResponse> {
  return request<NotificationOutboxListResponse>(`/api/notification-outbox${buildQuery(filters)}`);
}

export function getNotificationOutbox(id: string): Promise<NotificationOutboxDetail> {
  return request<NotificationOutboxDetail>(`/api/notification-outbox/${id}`);
}

export function retryNotificationOutbox(id: string): Promise<RetryNotificationOutboxResponse> {
  return request<RetryNotificationOutboxResponse>(`/api/notification-outbox/${id}/retry`, {
    method: 'POST',
  });
}

export function listNotificationDeliveries(filters: NotificationDeliveryQuery = {}): Promise<NotificationDeliveryListResponse> {
  return request<NotificationDeliveryListResponse>(`/api/notification-deliveries${buildQuery(filters)}`);
}
```

- [ ] **Step 2: Run build to verify RED**

Run:

```bash
npm run build --prefix frontend
```

Expected: FAIL because the new notification operations TypeScript types are not exported from `frontend/src/api/types.ts`.

- [ ] **Step 3: Add frontend DTO types**

In `frontend/src/api/types.ts`, add these types after `InAppNotificationQuery`:

```ts
export type OutboxStatusCounts = {
  pending: number;
  retryable: number;
  processing: number;
  failed: number;
  stale_processing: number;
  next_due_at?: string | null;
};

export type NotificationOutboxQuery = {
  status?: string;
  event_id?: string;
  limit?: number;
  offset?: number;
};

export type NotificationOutboxListItem = {
  id: string;
  tenant_id: string;
  event_id: string;
  status: string;
  attempt_count: number;
  next_attempt_at: string;
  locked_at?: string | null;
  locked_by?: string | null;
  last_error?: string | null;
  delivered_at?: string | null;
  created_at: string;
  updated_at: string;
  event_type?: string | null;
  direction?: string | null;
  tx_hash?: string | null;
  delivery_total: number;
  delivery_sent: number;
  delivery_failed: number;
  delivery_skipped: number;
  is_stale_processing: boolean;
};

export type NotificationDeliveryQuery = {
  event_id?: string;
  status?: string;
  channel_type?: string;
  rule_id?: string;
  channel_id?: string;
  limit?: number;
  offset?: number;
};

export type NotificationDeliveryListItem = {
  id: string;
  tenant_id: string;
  event_id: string;
  rule_id?: string | null;
  channel_id?: string | null;
  channel_type?: string | null;
  status: string;
  attempt_count: number;
  last_error?: string | null;
  sent_at?: string | null;
  created_at: string;
  idempotency_key?: string | null;
  provider_message_id?: string | null;
  provider_status_code?: number | null;
  provider_response?: string | null;
};

export type NotificationOutboxListResponse = {
  items: NotificationOutboxListItem[];
  limit: number;
  offset: number;
};

export type NotificationOutboxDetail = {
  outbox: NotificationOutboxListItem;
  event: AddressEvent;
  deliveries: NotificationDeliveryListItem[];
};

export type NotificationDeliveryListResponse = {
  items: NotificationDeliveryListItem[];
  limit: number;
  offset: number;
};

export type RetryNotificationOutboxResponse = {
  outbox: NotificationOutboxListItem;
};
```

Update `NotificationStatus` to include outbox counts:

```ts
export type NotificationStatus = {
  last_24h_sent: number;
  last_24h_skipped: number;
  last_24h_failed: number;
  unread_in_app: number;
  outbox: OutboxStatusCounts;
};
```

- [ ] **Step 4: Deduplicate query builder usage**

Replace the manual query-string logic in `listEvents` with:

```ts
export function listEvents(filters: EventQuery = {}): Promise<AddressEvent[]> {
  return request<AddressEvent[]>(`/api/events${buildQuery(filters)}`);
}
```

Replace the manual query-string logic in `listInAppNotifications` with:

```ts
export function listInAppNotifications(filters: InAppNotificationQuery = {}): Promise<InAppNotification[]> {
  return request<InAppNotification[]>(`/api/in-app-notifications${buildQuery(filters)}`);
}
```

Keep `buildQuery` near the top of the file after `request` so all client methods can use it.

- [ ] **Step 5: Run build to verify GREEN**

Run:

```bash
npm run build --prefix frontend
```

Expected: command exits 0. Vite may print existing chunk-size or `lottie-web` eval warnings; those warnings do not fail the build.

- [ ] **Step 6: Commit Task 7**

Run:

```bash
git status --short
git add frontend/src/api/types.ts frontend/src/api/client.ts
git commit -m "Add notification operations frontend API client"
```

Expected: commit succeeds and does not stage `frontend/package-lock.json` unless this task intentionally changed dependencies, which it should not.

---

### Task 8: Add Notification Operations page

**Files:**
- Create: `frontend/src/pages/NotificationOperationsPage.tsx`
- Test: TypeScript compile via `npm run build --prefix frontend`

- [ ] **Step 1: Write failing page shell**

Create `frontend/src/pages/NotificationOperationsPage.tsx` with this minimal compile target:

```tsx
import { Card } from '@douyinfe/semi-ui';
import { listNotificationOutbox } from '../api/client';
import type { NotificationOutboxListItem } from '../api/types';

export function NotificationOperationsPage() {
  const row: NotificationOutboxListItem | null = null;
  void listNotificationOutbox;
  return <Card title="通知运维">{row?.status ?? 'loading'}</Card>;
}
```

- [ ] **Step 2: Run build to verify RED**

Run:

```bash
npm run build --prefix frontend
```

Expected: PASS if Task 7 is complete. If it fails, the failure should point to missing client/types from Task 7; fix Task 7 before continuing. Then replace the shell with the full page in the next step and use the build as the GREEN check for the real page.

- [ ] **Step 3: Replace shell with full Semi Design page**

Replace the whole file with:

```tsx
import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Banner, Button, Card, Col, Form, Modal, Row, Space, Table, Tag, Toast, Typography } from '@douyinfe/semi-ui';
import { getNotificationOutbox, getSystemStatus, listNotificationOutbox, retryNotificationOutbox } from '../api/client';
import type { NotificationDeliveryListItem, NotificationOutboxListItem, NotificationOutboxQuery } from '../api/types';

const { Text, Title } = Typography;

type FilterForm = {
  status?: string;
  event_id?: string;
};

const outboxStatusOptions = [
  { label: 'pending', value: 'pending' },
  { label: 'processing', value: 'processing' },
  { label: 'retryable', value: 'retryable' },
  { label: 'delivered', value: 'delivered' },
  { label: 'failed', value: 'failed' },
];

function formatTime(value?: string | null) {
  return value ? new Date(value).toLocaleString() : '-';
}

function truncate(value?: string | null, maxLength = 160) {
  if (!value) return '-';
  return value.length > maxLength ? `${value.slice(0, maxLength)}...` : value;
}

function outboxStatusColor(status: string): 'green' | 'red' | 'orange' | 'blue' | 'grey' {
  if (status === 'delivered') return 'green';
  if (status === 'failed') return 'red';
  if (status === 'retryable') return 'orange';
  if (status === 'processing') return 'blue';
  return 'grey';
}

function deliveryStatusColor(status: string): 'green' | 'red' | 'orange' | 'blue' | 'grey' {
  if (status === 'sent') return 'green';
  if (status === 'failed') return 'red';
  if (status === 'skipped') return 'orange';
  if (status === 'processing') return 'blue';
  return 'grey';
}

function retryableOutbox(status: string) {
  return status === 'failed' || status === 'retryable';
}

export function NotificationOperationsPage() {
  const [filters, setFilters] = useState<NotificationOutboxQuery>({ limit: 50, offset: 0 });
  const [selectedOutboxId, setSelectedOutboxId] = useState<string>();
  const queryClient = useQueryClient();

  const statusQuery = useQuery({
    queryKey: ['system-status'],
    queryFn: getSystemStatus,
    refetchInterval: 10_000,
  });

  const outboxQuery = useQuery({
    queryKey: ['notification-outbox', filters],
    queryFn: () => listNotificationOutbox(filters),
  });

  const detailQuery = useQuery({
    queryKey: ['notification-outbox-detail', selectedOutboxId],
    queryFn: () => getNotificationOutbox(selectedOutboxId ?? ''),
    enabled: Boolean(selectedOutboxId),
  });

  const retryMutation = useMutation({
    mutationFn: retryNotificationOutbox,
    onSuccess: () => {
      Toast.success('通知任务已重新进入重试队列');
      queryClient.invalidateQueries({ queryKey: ['notification-outbox'] });
      queryClient.invalidateQueries({ queryKey: ['notification-outbox-detail'] });
      queryClient.invalidateQueries({ queryKey: ['system-status'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '重试通知任务失败'),
  });

  function handleFilterSubmit(values: Record<string, unknown>) {
    const form = values as FilterForm;
    const eventId = form.event_id?.trim();
    setFilters({
      status: form.status || undefined,
      event_id: eventId || undefined,
      limit: 50,
      offset: 0,
    });
  }

  function resetFilters(formApi: { reset: () => void }) {
    formApi.reset();
    setFilters({ limit: 50, offset: 0 });
  }

  const outbox = statusQuery.data?.notifications.outbox;

  return (
    <Space vertical align="start" spacing={16} className="content-stack">
      {outboxQuery.isError ? (
        <Banner
          type="danger"
          title="通知任务加载失败"
          description={outboxQuery.error instanceof Error ? outboxQuery.error.message : '请求失败'}
        />
      ) : null}

      {detailQuery.isError ? (
        <Banner
          type="danger"
          title="通知任务详情加载失败"
          description={detailQuery.error instanceof Error ? detailQuery.error.message : '请求失败'}
        />
      ) : null}

      <Card title="通知任务积压总览" loading={statusQuery.isLoading}>
        <Row gutter={[16, 16]}>
          <Col span={8}>
            <Metric title="Pending" value={outbox?.pending ?? 0} hint="等待 notifier claim" />
          </Col>
          <Col span={8}>
            <Metric title="Retryable" value={outbox?.retryable ?? 0} hint="等待自动重试" />
          </Col>
          <Col span={8}>
            <Metric title="Processing" value={outbox?.processing ?? 0} hint={`stale ${outbox?.stale_processing ?? 0}`} />
          </Col>
          <Col span={8}>
            <Metric title="Failed" value={outbox?.failed ?? 0} hint="可人工重试" />
          </Col>
          <Col span={8}>
            <Metric title="Stale Processing" value={outbox?.stale_processing ?? 0} hint="locked_at 超过 15 分钟" />
          </Col>
          <Col span={8}>
            <Metric title="Next Due" value={formatTime(outbox?.next_due_at)} hint="pending/retryable due" />
          </Col>
        </Row>
      </Card>

      <Card title="Outbox 筛选" className="filter-card">
        <Form<FilterForm> layout="horizontal" onSubmit={handleFilterSubmit} labelPosition="left">
          {({ formApi }) => (
            <>
              <Form.Select field="status" label="状态" showClear placeholder="全部状态" optionList={outboxStatusOptions} />
              <Form.Input field="event_id" label="Event ID" placeholder="按 event UUID 查询" style={{ width: 360 }} />
              <Space>
                <Button htmlType="submit" type="primary">查询</Button>
                <Button onClick={() => resetFilters(formApi)}>重置</Button>
                <Button onClick={() => outboxQuery.refetch()}>刷新</Button>
              </Space>
            </>
          )}
        </Form>
      </Card>

      <Card title="Notification Outbox">
        <Table<NotificationOutboxListItem>
          loading={outboxQuery.isLoading}
          dataSource={outboxQuery.data?.items ?? []}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          scroll={{ x: 1700 }}
          columns={[
            { title: '创建时间', dataIndex: 'created_at', width: 180, render: value => formatTime(String(value)) },
            {
              title: '状态',
              dataIndex: 'status',
              width: 130,
              render: (_, row) => (
                <Space>
                  <Tag color={outboxStatusColor(row.status)}>{row.status}</Tag>
                  {row.is_stale_processing ? <Tag color="red">stale</Tag> : null}
                </Space>
              ),
            },
            { title: 'Event', dataIndex: 'event_id', width: 260, ellipsis: { showTitle: true } },
            { title: '事件类型', dataIndex: 'event_type', width: 130, render: value => value ? <Tag>{String(value)}</Tag> : '-' },
            { title: '方向', dataIndex: 'direction', width: 90, render: value => value ? String(value) : '-' },
            { title: '交易哈希', dataIndex: 'tx_hash', width: 240, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
            { title: 'Attempt', dataIndex: 'attempt_count', width: 100 },
            { title: 'Next Attempt', dataIndex: 'next_attempt_at', width: 180, render: value => formatTime(String(value)) },
            { title: 'Locked By', dataIndex: 'locked_by', width: 160, render: value => value ? String(value) : '-' },
            { title: 'Locked At', dataIndex: 'locked_at', width: 180, render: value => formatTime(value ? String(value) : null) },
            {
              title: 'Delivery',
              width: 170,
              render: (_, row) => `${row.delivery_sent}/${row.delivery_failed}/${row.delivery_skipped} / total ${row.delivery_total}`,
            },
            { title: 'Last Error', dataIndex: 'last_error', width: 280, ellipsis: { showTitle: true }, render: value => truncate(value ? String(value) : null) },
            {
              title: '操作',
              width: 170,
              fixed: 'right',
              render: (_, row) => (
                <Space>
                  <Button size="small" onClick={() => setSelectedOutboxId(row.id)}>详情</Button>
                  <Button
                    size="small"
                    type="primary"
                    disabled={!retryableOutbox(row.status)}
                    loading={retryMutation.isPending}
                    onClick={() => retryMutation.mutate(row.id)}
                  >
                    重试
                  </Button>
                </Space>
              ),
            },
          ]}
        />
      </Card>

      <OutboxDetailModal
        visible={Boolean(selectedOutboxId)}
        loading={detailQuery.isLoading}
        detail={detailQuery.data}
        onClose={() => setSelectedOutboxId(undefined)}
      />
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

function OutboxDetailModal({
  visible,
  loading,
  detail,
  onClose,
}: {
  visible: boolean;
  loading: boolean;
  detail?: {
    outbox: NotificationOutboxListItem;
    event: { id: string; event_type: string; direction: string; tx_hash?: string | null; metadata: Record<string, unknown> };
    deliveries: NotificationDeliveryListItem[];
  };
  onClose: () => void;
}) {
  return (
    <Modal title="通知任务详情" visible={visible} onCancel={onClose} footer={null} width={1100}>
      {loading ? <Text>正在加载详情...</Text> : null}
      {detail ? (
        <Space vertical align="start" spacing={16} style={{ width: '100%' }}>
          <Card title="Outbox">
            <Space vertical align="start">
              <Text>ID：{detail.outbox.id}</Text>
              <Text>Event：{detail.outbox.event_id}</Text>
              <Text>状态：<Tag color={outboxStatusColor(detail.outbox.status)}>{detail.outbox.status}</Tag></Text>
              <Text>Attempt：{detail.outbox.attempt_count}</Text>
              <Text>Next Attempt：{formatTime(detail.outbox.next_attempt_at)}</Text>
              <Text>Lock：{detail.outbox.locked_by ?? '-'} / {formatTime(detail.outbox.locked_at)}</Text>
              <Text>Last Error：{detail.outbox.last_error ?? '-'}</Text>
            </Space>
          </Card>

          <Card title="Event">
            <Space vertical align="start">
              <Text>类型：{detail.event.event_type}</Text>
              <Text>方向：{detail.event.direction}</Text>
              <Text>交易哈希：{detail.event.tx_hash ?? '-'}</Text>
              <pre style={{ maxWidth: 1000, whiteSpace: 'pre-wrap', wordBreak: 'break-word' }}>
                {JSON.stringify(detail.event.metadata, null, 2)}
              </pre>
            </Space>
          </Card>

          <Card title="Deliveries">
            <Table<NotificationDeliveryListItem>
              dataSource={detail.deliveries}
              rowKey="id"
              pagination={{ pageSize: 5 }}
              scroll={{ x: 1500 }}
              columns={[
                { title: '创建时间', dataIndex: 'created_at', width: 180, render: value => formatTime(String(value)) },
                { title: '渠道', dataIndex: 'channel_type', width: 110, render: value => value ? <Tag>{String(value)}</Tag> : '-' },
                { title: '状态', dataIndex: 'status', width: 110, render: value => <Tag color={deliveryStatusColor(String(value))}>{String(value)}</Tag> },
                { title: 'Attempt', dataIndex: 'attempt_count', width: 90 },
                { title: 'Rule ID', dataIndex: 'rule_id', width: 240, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
                { title: 'Channel ID', dataIndex: 'channel_id', width: 240, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
                { title: 'Idempotency Key', dataIndex: 'idempotency_key', width: 320, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
                { title: 'Provider Message', dataIndex: 'provider_message_id', width: 180, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
                { title: 'Provider Status', dataIndex: 'provider_status_code', width: 130, render: value => value ?? '-' },
                { title: 'Provider Response', dataIndex: 'provider_response', width: 260, ellipsis: { showTitle: true }, render: value => truncate(value ? String(value) : null, 120) },
                { title: 'Last Error', dataIndex: 'last_error', width: 260, ellipsis: { showTitle: true }, render: value => truncate(value ? String(value) : null, 120) },
              ]}
            />
          </Card>
        </Space>
      ) : null}
    </Modal>
  );
}
```

- [ ] **Step 4: Run build to verify GREEN**

Run:

```bash
npm run build --prefix frontend
```

Expected: command exits 0. Vite may print existing chunk-size or `lottie-web` eval warnings; those warnings do not fail the build.

- [ ] **Step 5: Commit Task 8**

Run:

```bash
git status --short
git add frontend/src/pages/NotificationOperationsPage.tsx
git commit -m "Add notification operations page"
```

Expected: commit succeeds and stages only the new page.

---

### Task 9: Wire notification operations navigation and status display

**Files:**
- Modify: `frontend/src/App.tsx:1-111`
- Modify: `frontend/src/pages/SystemStatusPage.tsx:47-83`
- Test: TypeScript compile via `npm run build --prefix frontend`

- [ ] **Step 1: Write failing navigation/status references**

In `frontend/src/App.tsx`, add this import:

```ts
import { NotificationOperationsPage } from './pages/NotificationOperationsPage';
```

Add `notification-operations` to the `PageKey` union:

```ts
  | 'notification-rules'
  | 'notification-operations'
  | 'in-app-notifications';
```

Add a nav item between `通知规则` and `站内通知`:

```tsx
{ itemKey: 'notification-operations', text: '通知运维', icon: <IconBell /> },
```

Add this route in `renderPage`:

```tsx
if (page === 'notification-operations') return <NotificationOperationsPage />;
```

In `frontend/src/pages/SystemStatusPage.tsx`, change the metric at the `24h 通知失败` card to reference `status.notifications.outbox.failed`:

```tsx
<Metric
  title="Outbox Failed"
  value={status?.notifications.outbox.failed ?? 0}
  hint={`24h delivery failed ${status?.notifications.last_24h_failed ?? 0}`}
/>
```

- [ ] **Step 2: Run build to verify RED or integration failures**

Run:

```bash
npm run build --prefix frontend
```

Expected: PASS if Tasks 7 and 8 are complete. If it fails, the failure should identify an incorrect import, page key, or `NotificationStatus.outbox` typing mismatch; fix the mismatch in the next step.

- [ ] **Step 3: Complete system status outbox display**

In `frontend/src/pages/SystemStatusPage.tsx`, update the notification summary text from:

```tsx
<Text>
  24h 通知：sent {status?.notifications.last_24h_sent ?? 0} / skipped {status?.notifications.last_24h_skipped ?? 0} / failed{' '}
  {status?.notifications.last_24h_failed ?? 0} / unread {status?.notifications.unread_in_app ?? 0}
</Text>
```

to:

```tsx
<Text>
  24h 通知：sent {status?.notifications.last_24h_sent ?? 0} / skipped {status?.notifications.last_24h_skipped ?? 0} / failed{' '}
  {status?.notifications.last_24h_failed ?? 0} / unread {status?.notifications.unread_in_app ?? 0}
</Text>
<Text>
  Outbox：pending {status?.notifications.outbox.pending ?? 0} / retryable {status?.notifications.outbox.retryable ?? 0} / processing{' '}
  {status?.notifications.outbox.processing ?? 0} / failed {status?.notifications.outbox.failed ?? 0} / stale{' '}
  {status?.notifications.outbox.stale_processing ?? 0} / next due {formatTime(status?.notifications.outbox.next_due_at)}
</Text>
```

Update the dashboard `Banner` description in `frontend/src/App.tsx` to mention notification operations:

```tsx
description="当前版本提供登录、链配置、资产配置、Provider 配置、监听地址管理、事件中心、通知规则、通知运维、站内通知和系统状态。"
```

- [ ] **Step 4: Run build to verify GREEN**

Run:

```bash
npm run build --prefix frontend
```

Expected: command exits 0. Vite may print existing chunk-size or `lottie-web` eval warnings; those warnings do not fail the build.

- [ ] **Step 5: Commit Task 9**

Run:

```bash
git status --short
git add frontend/src/App.tsx frontend/src/pages/SystemStatusPage.tsx
git commit -m "Wire notification operations navigation"
```

Expected: commit succeeds and does not stage `frontend/package-lock.json` unless the file was intentionally changed by this task, which it should not be.

---

### Task 10: Final verification

**Files:**
- Verify: backend workspace
- Verify: frontend build
- Verify: Docker Compose config

- [ ] **Step 1: Run backend formatting check**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: exits 0.

- [ ] **Step 2: Run backend workspace check**

Run:

```bash
cargo check --workspace --manifest-path backend/Cargo.toml
```

Expected: exits 0.

- [ ] **Step 3: Run backend workspace tests**

Run:

```bash
cargo test --workspace --manifest-path backend/Cargo.toml
```

Expected: exits 0 with all tests passing.

- [ ] **Step 4: Run frontend build**

Run:

```bash
npm run build --prefix frontend
```

Expected: exits 0. Existing Vite warnings about chunk size or `lottie-web` eval do not fail the build.

- [ ] **Step 5: Run Docker Compose config without preserving a local `.env`**

Run:

```bash
bash -lc 'if [ -e .env ]; then echo ".env already exists; not overwriting"; exit 1; fi; cp .env.example .env; docker compose config >/tmp/coin-listener-compose-config.txt; compose_status=$?; rm -f .env; exit $compose_status'
```

Expected: exits 0 and removes the temporary `.env` file.

- [ ] **Step 6: Confirm git state only contains intentional changes**

Run:

```bash
git status --short
```

Expected: clean working tree after all task commits, or only pre-existing unrelated files such as `frontend/package-lock.json` remain modified and unstaged.

- [ ] **Step 7: Commit final verification note if a tracked verification artifact was intentionally added**

No commit is required when Step 6 is clean or only unrelated unstaged files remain. If a task added a small tracked verification artifact, commit only that artifact:

```bash
git add <intentional-verification-artifact>
git commit -m "Verify notification operations console"
```

Expected: either no commit is needed, or the commit contains only the intentional verification artifact.

---

## Self-Review Checklist for Implementers

Before reporting the implementation complete, verify these requirements against the code:

- Outbox list supports `status`, `event_id`, `limit`, and `offset`.
- Outbox list returns event summary fields, delivery counts, and `is_stale_processing`.
- Outbox detail returns one outbox row, the related address event, and related deliveries.
- Delivery list supports `event_id`, `status`, `channel_type`, `rule_id`, `channel_id`, `limit`, and `offset`.
- Retry endpoint only accepts `failed` and `retryable` rows.
- Retry sets `status = 'retryable'`, `next_attempt_at = now`, clears lock fields and `last_error`, and does not reset `attempt_count`.
- System status includes pending, retryable, processing, failed, stale processing, and next due outbox values.
- Frontend page truncates long provider response and error text in table views.
- Navigation includes `通知运维`.
- `frontend/package-lock.json` is not staged unless dependency changes were intentionally made.
