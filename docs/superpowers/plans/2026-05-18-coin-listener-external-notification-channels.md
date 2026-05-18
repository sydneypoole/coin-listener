# Coin Listener External Notification Channels Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement real Telegram and Webhook notification channel sending with stable event/rule/channel idempotency records.

**Architecture:** Keep PostgreSQL `notification_outbox` as the reliable task source. Extend `notification_deliveries` with external delivery metadata, add idempotent delivery begin/update helpers, then route active `telegram` and `webhook` channels through a shared reusable HTTP sender while preserving existing `in_app` behavior.

**Tech Stack:** Rust 2021, SQLx, PostgreSQL migrations, Tokio, reqwest with rustls, serde/serde_json, hmac, sha2, chrono, uuid, existing Coin Listener core/storage/notifier crates.

---

## Scope and Constraints

Implement only the approved scope from `docs/superpowers/specs/2026-05-18-coin-listener-external-notification-channels-design.md`.

Do not implement frontend retry/dead-letter pages, a generic secret manager, Email/Discord/enterprise chat channels, WebSocket push, a notification template system, or historical replay of old skipped Telegram/Webhook deliveries.

Keep the existing `in_app` path working. Keep `notification_outbox` claim/retry/stale recovery as the only reliable notification task source.

---

## File Structure

Create:

```text
backend/crates/storage/migrations/0008_external_notification_deliveries.sql
backend/crates/notifier/src/external.rs
```

Modify:

```text
backend/crates/core/src/error.rs
backend/crates/core/src/models.rs
backend/crates/storage/src/notifications.rs
backend/crates/notifier/Cargo.toml
backend/crates/notifier/src/lib.rs
backend/crates/notifier/src/main.rs
```

Responsibilities:

- `backend/crates/storage/migrations/0008_external_notification_deliveries.sql`: additive delivery metadata columns and idempotency unique index.
- `backend/crates/core/src/models.rs`: shared `NotificationDelivery` row shape including external metadata fields.
- `backend/crates/core/src/error.rs`: external notification error variant used for transient provider failures that must make outbox retry.
- `backend/crates/storage/src/notifications.rs`: delivery status validation, existing delivery query projections, and external delivery begin/sent/failed repository helpers.
- `backend/crates/notifier/Cargo.toml`: notifier dependencies for HTTP, JSON, HMAC, SHA-256.
- `backend/crates/notifier/src/external.rs`: typed Telegram/Webhook config parsing, idempotency keys, redaction, webhook signing/payloads, response classification, and reusable HTTP sender.
- `backend/crates/notifier/src/lib.rs`: channel decision and delivery plan integration, external delivery processing, outbox retry propagation, and reusable sender injection.
- `backend/crates/notifier/src/main.rs`: create one `reqwest::Client` at notifier startup and pass one `ExternalNotificationSender` into the notifier loop.

---

### Task 1: Add external delivery metadata schema and model

**Files:**
- Create: `backend/crates/storage/migrations/0008_external_notification_deliveries.sql`
- Modify: `backend/crates/core/src/models.rs:327-339`
- Modify: `backend/crates/storage/src/notifications.rs:488-623`
- Test: `backend/crates/core/src/models.rs` existing `#[cfg(test)] mod tests`
- Test: `backend/crates/storage/src/notifications.rs` existing `#[cfg(test)] mod tests`

- [ ] **Step 1: Write failing model and migration tests**

In `backend/crates/core/src/models.rs`, add this test after `notification_outbox_item_round_trips_as_json`:

```rust
#[test]
fn notification_delivery_round_trips_external_metadata() {
    let delivery = NotificationDelivery {
        id: Uuid::from_u128(31),
        tenant_id: Uuid::from_u128(32),
        event_id: Uuid::from_u128(33),
        rule_id: Some(Uuid::from_u128(34)),
        channel_id: Some(Uuid::from_u128(35)),
        channel_type: Some("webhook".to_string()),
        status: "sent".to_string(),
        attempt_count: 2,
        idempotency_key: Some("notification:v1:tenant:event:rule:channel".to_string()),
        provider_message_id: Some("provider-123".to_string()),
        provider_status_code: Some(202),
        provider_response: Some("accepted".to_string()),
        last_error: None,
        sent_at: Some(Utc.with_ymd_and_hms(2026, 5, 18, 14, 0, 0).unwrap()),
        created_at: Utc.with_ymd_and_hms(2026, 5, 18, 13, 59, 0).unwrap(),
    };

    let payload = serde_json::to_string(&delivery).expect("serialize notification delivery");
    let decoded: NotificationDelivery =
        serde_json::from_str(&payload).expect("deserialize notification delivery");

    assert_eq!(decoded.channel_type.as_deref(), Some("webhook"));
    assert_eq!(
        decoded.idempotency_key.as_deref(),
        Some("notification:v1:tenant:event:rule:channel")
    );
    assert_eq!(decoded.provider_message_id.as_deref(), Some("provider-123"));
    assert_eq!(decoded.provider_status_code, Some(202));
    assert!(payload.contains("\"provider_response\":\"accepted\""));
}
```

Update the existing core test import list from:

```rust
use super::{
    CreateBalanceSnapshotRequest, EventStatus, NotificationOutboxItem, NotificationStatus,
    NotifyEventTask, ProviderChainStatus, ProviderStatus, ProviderStatusItem, QueueStatus,
    ScanAddressTask, ScanCursor, ScanStatus, SystemStatus,
};
```

to:

```rust
use super::{
    CreateBalanceSnapshotRequest, EventStatus, NotificationDelivery, NotificationOutboxItem,
    NotificationStatus, NotifyEventTask, ProviderChainStatus, ProviderStatus, ProviderStatusItem,
    QueueStatus, ScanAddressTask, ScanCursor, ScanStatus, SystemStatus,
};
```

In `backend/crates/storage/src/notifications.rs`, add this test inside the existing test module:

```rust
#[test]
fn external_delivery_migration_adds_metadata_and_idempotency_index() {
    let migration = include_str!("../migrations/0008_external_notification_deliveries.sql");

    assert!(migration.contains("ADD COLUMN IF NOT EXISTS channel_type TEXT"));
    assert!(migration.contains("ADD COLUMN IF NOT EXISTS idempotency_key TEXT"));
    assert!(migration.contains("ADD COLUMN IF NOT EXISTS provider_message_id TEXT"));
    assert!(migration.contains("ADD COLUMN IF NOT EXISTS provider_status_code INTEGER"));
    assert!(migration.contains("ADD COLUMN IF NOT EXISTS provider_response TEXT"));
    assert!(migration.contains("idx_notification_deliveries_idempotency"));
    assert!(migration.contains("WHERE idempotency_key IS NOT NULL"));
}
```

- [ ] **Step 2: Run tests to verify RED**

Run:

```bash
cargo test -p coin-listener-core notification_delivery_round_trips_external_metadata --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage external_delivery_migration_adds_metadata_and_idempotency_index --manifest-path backend/Cargo.toml
```

Expected: FAIL. The core test fails because `NotificationDelivery` lacks the new fields. The storage test fails because `0008_external_notification_deliveries.sql` does not exist.

- [ ] **Step 3: Create metadata migration**

Create `backend/crates/storage/migrations/0008_external_notification_deliveries.sql` with exactly this content:

```sql
ALTER TABLE notification_deliveries
    ADD COLUMN IF NOT EXISTS channel_type TEXT,
    ADD COLUMN IF NOT EXISTS idempotency_key TEXT,
    ADD COLUMN IF NOT EXISTS provider_message_id TEXT,
    ADD COLUMN IF NOT EXISTS provider_status_code INTEGER,
    ADD COLUMN IF NOT EXISTS provider_response TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_notification_deliveries_idempotency
    ON notification_deliveries(event_id, rule_id, channel_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
```

- [ ] **Step 4: Extend `NotificationDelivery` model**

In `backend/crates/core/src/models.rs`, replace `NotificationDelivery` with:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationDelivery {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub event_id: Uuid,
    pub rule_id: Option<Uuid>,
    pub channel_id: Option<Uuid>,
    pub channel_type: Option<String>,
    pub status: String,
    pub attempt_count: i32,
    pub idempotency_key: Option<String>,
    pub provider_message_id: Option<String>,
    pub provider_status_code: Option<i32>,
    pub provider_response: Option<String>,
    pub last_error: Option<String>,
    pub sent_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}
```

- [ ] **Step 5: Update existing delivery query projections**

In `backend/crates/storage/src/notifications.rs`, update every `RETURNING` projection for `NotificationDelivery` and every `SELECT` projection for `notification_deliveries` to include the new fields in this order:

```sql
RETURNING id, tenant_id, event_id, rule_id, channel_id, channel_type, status, attempt_count,
          idempotency_key, provider_message_id, provider_status_code, provider_response,
          last_error, sent_at, created_at
```

Apply this to:

```text
create_notification_delivery
create_notification_delivery_with_executor
update_notification_delivery_status
```

Do not add the new metadata fields to the existing `INSERT INTO notification_deliveries` columns in this task. Existing in-app and skipped deliveries should naturally return `NULL` for the new columns.

- [ ] **Step 6: Run tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-core notification_delivery_round_trips_external_metadata --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage external_delivery_migration_adds_metadata_and_idempotency_index --manifest-path backend/Cargo.toml
cargo check -p coin-listener-storage --manifest-path backend/Cargo.toml
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 7: Commit Task 1**

Run:

```bash
git status --short
git add backend/crates/core/src/models.rs backend/crates/storage/src/notifications.rs backend/crates/storage/migrations/0008_external_notification_deliveries.sql
git commit -m "Add external notification delivery metadata"
```

Expected: commit succeeds and contains only the migration/model/query projection changes.

---

### Task 2: Add idempotent external delivery repository helpers

**Files:**
- Modify: `backend/crates/storage/src/notifications.rs:20-690`
- Test: `backend/crates/storage/src/notifications.rs` existing `#[cfg(test)] mod tests`

- [ ] **Step 1: Write failing repository helper tests**

In `backend/crates/storage/src/notifications.rs`, add these tests to the existing test module:

```rust
#[test]
fn delivery_status_validation_accepts_processing_for_external_sends() {
    assert!(validate_notification_delivery_status("processing").is_ok());
}

#[test]
fn external_delivery_start_skips_already_sent_delivery() {
    let delivery_id = Uuid::from_u128(41);

    let start = external_delivery_start_from_status(delivery_id, "sent");

    assert_eq!(
        start,
        ExternalDeliveryStart::AlreadyComplete { delivery_id }
    );
}

#[test]
fn external_delivery_start_reuses_failed_delivery_for_retry() {
    let delivery_id = Uuid::from_u128(42);

    let start = external_delivery_start_from_status(delivery_id, "failed");

    assert_eq!(start, ExternalDeliveryStart::ReadyToSend { delivery_id });
}

#[test]
fn external_delivery_queries_use_idempotency_key_and_row_lock() {
    assert!(SELECT_EXTERNAL_NOTIFICATION_DELIVERY_FOR_UPDATE_QUERY.contains("FOR UPDATE"));
    assert!(SELECT_EXTERNAL_NOTIFICATION_DELIVERY_FOR_UPDATE_QUERY.contains("idempotency_key = $5"));
    assert!(INSERT_EXTERNAL_NOTIFICATION_DELIVERY_QUERY.contains("idempotency_key"));
    assert!(UPDATE_EXTERNAL_NOTIFICATION_DELIVERY_PROCESSING_QUERY.contains("status = 'processing'"));
    assert!(MARK_EXTERNAL_NOTIFICATION_DELIVERY_SENT_QUERY.contains("provider_message_id"));
    assert!(MARK_EXTERNAL_NOTIFICATION_DELIVERY_FAILED_QUERY.contains("provider_status_code"));
}
```

Update the test module import list to include:

```rust
ExternalDeliveryStart, INSERT_EXTERNAL_NOTIFICATION_DELIVERY_QUERY,
MARK_EXTERNAL_NOTIFICATION_DELIVERY_FAILED_QUERY, MARK_EXTERNAL_NOTIFICATION_DELIVERY_SENT_QUERY,
SELECT_EXTERNAL_NOTIFICATION_DELIVERY_FOR_UPDATE_QUERY,
UPDATE_EXTERNAL_NOTIFICATION_DELIVERY_PROCESSING_QUERY,
external_delivery_start_from_status,
```

- [ ] **Step 2: Run tests to verify RED**

Run:

```bash
cargo test -p coin-listener-storage delivery_status_validation_accepts_processing_for_external_sends --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage external_delivery_start_skips_already_sent_delivery --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage external_delivery_queries_use_idempotency_key_and_row_lock --manifest-path backend/Cargo.toml
```

Expected: FAIL because `processing` is rejected, `ExternalDeliveryStart` does not exist, and the query constants do not exist.

- [ ] **Step 3: Add query constants and start type**

In `backend/crates/storage/src/notifications.rs`, add these items after the existing constants:

```rust
pub const SELECT_EXTERNAL_NOTIFICATION_DELIVERY_FOR_UPDATE_QUERY: &str = r#"
        SELECT id, status
        FROM notification_deliveries
        WHERE tenant_id = $1
          AND event_id = $2
          AND rule_id = $3
          AND channel_id = $4
          AND idempotency_key = $5
        FOR UPDATE
        "#;

pub const INSERT_EXTERNAL_NOTIFICATION_DELIVERY_QUERY: &str = r#"
        INSERT INTO notification_deliveries (
            tenant_id, event_id, rule_id, channel_id, channel_type, status, attempt_count,
            idempotency_key, last_error
        )
        SELECT $1, $2, $3, $4, $5, 'processing', $6, $7, NULL
        WHERE EXISTS (
            SELECT 1 FROM address_events
            WHERE id = $2
              AND tenant_id = $1
        )
          AND EXISTS (
              SELECT 1 FROM notification_rules
              WHERE id = $3
                AND tenant_id = $1
          )
          AND EXISTS (
              SELECT 1 FROM notification_channels
              WHERE id = $4
                AND tenant_id = $1
          )
        RETURNING id, tenant_id, event_id, rule_id, channel_id, channel_type, status, attempt_count,
                  idempotency_key, provider_message_id, provider_status_code, provider_response,
                  last_error, sent_at, created_at
        "#;

pub const UPDATE_EXTERNAL_NOTIFICATION_DELIVERY_PROCESSING_QUERY: &str = r#"
        UPDATE notification_deliveries
        SET status = 'processing',
            attempt_count = $3,
            last_error = NULL,
            provider_message_id = NULL,
            provider_status_code = NULL,
            provider_response = NULL,
            sent_at = NULL
        WHERE id = $1
          AND tenant_id = $2
        RETURNING id, tenant_id, event_id, rule_id, channel_id, channel_type, status, attempt_count,
                  idempotency_key, provider_message_id, provider_status_code, provider_response,
                  last_error, sent_at, created_at
        "#;

pub const MARK_EXTERNAL_NOTIFICATION_DELIVERY_SENT_QUERY: &str = r#"
        UPDATE notification_deliveries
        SET status = 'sent',
            last_error = NULL,
            sent_at = $3,
            provider_message_id = $4,
            provider_status_code = $5,
            provider_response = $6
        WHERE id = $1
          AND tenant_id = $2
        RETURNING id, tenant_id, event_id, rule_id, channel_id, channel_type, status, attempt_count,
                  idempotency_key, provider_message_id, provider_status_code, provider_response,
                  last_error, sent_at, created_at
        "#;

pub const MARK_EXTERNAL_NOTIFICATION_DELIVERY_FAILED_QUERY: &str = r#"
        UPDATE notification_deliveries
        SET status = 'failed',
            last_error = $3,
            provider_status_code = $4,
            provider_response = $5
        WHERE id = $1
          AND tenant_id = $2
        RETURNING id, tenant_id, event_id, rule_id, channel_id, channel_type, status, attempt_count,
                  idempotency_key, provider_message_id, provider_status_code, provider_response,
                  last_error, sent_at, created_at
        "#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalDeliveryStart {
    AlreadyComplete { delivery_id: Uuid },
    ReadyToSend { delivery_id: Uuid },
}
```

- [ ] **Step 4: Add status validation and pure start helper**

Update `validate_notification_delivery_status` so it accepts processing:

```rust
pub fn validate_notification_delivery_status(status: &str) -> AppResult<()> {
    if !matches!(
        status,
        DELIVERY_STATUS_SENT | DELIVERY_STATUS_SKIPPED | DELIVERY_STATUS_FAILED | DELIVERY_STATUS_PROCESSING
    ) {
        return Err(AppError::Validation(
            "delivery status must be processing, sent, skipped, or failed".to_string(),
        ));
    }
    Ok(())
}
```

Add this helper after `validate_notification_delivery_status`:

```rust
pub fn external_delivery_start_from_status(
    delivery_id: Uuid,
    status: &str,
) -> ExternalDeliveryStart {
    match status {
        DELIVERY_STATUS_SENT | DELIVERY_STATUS_SKIPPED => {
            ExternalDeliveryStart::AlreadyComplete { delivery_id }
        }
        _ => ExternalDeliveryStart::ReadyToSend { delivery_id },
    }
}
```

Update the existing `delivery_status_validation_rejects_unknown_status` expected message to:

```rust
Err(AppError::Validation(message)) if message == "delivery status must be processing, sent, skipped, or failed"
```

- [ ] **Step 5: Add repository helpers**

Add these functions after `create_notification_delivery`:

```rust
pub async fn begin_external_notification_delivery(
    pool: &PgPool,
    tenant_id: Uuid,
    event_id: Uuid,
    rule_id: Uuid,
    channel_id: Uuid,
    channel_type: &str,
    idempotency_key: &str,
    attempt_count: i32,
) -> AppResult<ExternalDeliveryStart> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let existing = sqlx::query_as::<_, (Uuid, String)>(
        SELECT_EXTERNAL_NOTIFICATION_DELIVERY_FOR_UPDATE_QUERY,
    )
    .bind(tenant_id)
    .bind(event_id)
    .bind(rule_id)
    .bind(channel_id)
    .bind(idempotency_key)
    .fetch_optional(transaction.as_mut())
    .await
    .map_err(|error| AppError::Database(error.to_string()))?;

    let start = if let Some((delivery_id, status)) = existing {
        let start = external_delivery_start_from_status(delivery_id, &status);
        if matches!(start, ExternalDeliveryStart::ReadyToSend { .. }) {
            sqlx::query_as::<_, NotificationDelivery>(
                UPDATE_EXTERNAL_NOTIFICATION_DELIVERY_PROCESSING_QUERY,
            )
            .bind(delivery_id)
            .bind(tenant_id)
            .bind(attempt_count)
            .fetch_optional(transaction.as_mut())
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .ok_or_else(|| AppError::NotFound("external notification delivery".to_string()))?;
        }
        start
    } else {
        let delivery = sqlx::query_as::<_, NotificationDelivery>(
            INSERT_EXTERNAL_NOTIFICATION_DELIVERY_QUERY,
        )
        .bind(tenant_id)
        .bind(event_id)
        .bind(rule_id)
        .bind(channel_id)
        .bind(channel_type)
        .bind(attempt_count)
        .bind(idempotency_key)
        .fetch_optional(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("external notification delivery target".to_string()))?;

        ExternalDeliveryStart::ReadyToSend {
            delivery_id: delivery.id,
        }
    };

    transaction
        .commit()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    Ok(start)
}

pub async fn mark_external_notification_delivery_sent(
    pool: &PgPool,
    tenant_id: Uuid,
    delivery_id: Uuid,
    sent_at: DateTime<Utc>,
    provider_message_id: Option<&str>,
    provider_status_code: Option<i32>,
    provider_response: Option<&str>,
) -> AppResult<NotificationDelivery> {
    sqlx::query_as::<_, NotificationDelivery>(MARK_EXTERNAL_NOTIFICATION_DELIVERY_SENT_QUERY)
        .bind(delivery_id)
        .bind(tenant_id)
        .bind(sent_at)
        .bind(provider_message_id)
        .bind(provider_status_code)
        .bind(provider_response)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("external notification delivery".to_string()))
}

pub async fn mark_external_notification_delivery_failed(
    pool: &PgPool,
    tenant_id: Uuid,
    delivery_id: Uuid,
    last_error: &str,
    provider_status_code: Option<i32>,
    provider_response: Option<&str>,
) -> AppResult<NotificationDelivery> {
    sqlx::query_as::<_, NotificationDelivery>(MARK_EXTERNAL_NOTIFICATION_DELIVERY_FAILED_QUERY)
        .bind(delivery_id)
        .bind(tenant_id)
        .bind(last_error)
        .bind(provider_status_code)
        .bind(provider_response)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("external notification delivery".to_string()))
}
```

- [ ] **Step 6: Run tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-storage delivery_status_validation_accepts_processing_for_external_sends --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage external_delivery_start_skips_already_sent_delivery --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage external_delivery_start_reuses_failed_delivery_for_retry --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage external_delivery_queries_use_idempotency_key_and_row_lock --manifest-path backend/Cargo.toml
cargo check -p coin-listener-storage --manifest-path backend/Cargo.toml
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 7: Commit Task 2**

Run:

```bash
git status --short
git add backend/crates/storage/src/notifications.rs
git commit -m "Add idempotent external delivery helpers"
```

Expected: commit succeeds and contains only storage notification helper changes.

---

### Task 3: Add external notification config and idempotency module

**Files:**
- Create: `backend/crates/notifier/src/external.rs`
- Modify: `backend/crates/notifier/src/lib.rs:1-22`
- Modify: `backend/crates/notifier/Cargo.toml:7-18`
- Modify: `backend/crates/core/src/error.rs:5-19`
- Test: `backend/crates/notifier/src/external.rs` new `#[cfg(test)] mod tests`

- [ ] **Step 1: Write failing external config/idempotency tests**

Create `backend/crates/notifier/src/external.rs` with this test module first:

```rust
#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use crate::external::{
        notification_idempotency_key, redact_telegram_url, redact_webhook_url,
        TelegramChannelConfig, WebhookChannelConfig,
    };

    fn uuid(value: u128) -> Uuid {
        Uuid::from_u128(value)
    }

    #[test]
    fn telegram_channel_config_requires_token_env_and_chat_id() {
        let missing_token = TelegramChannelConfig::parse(&json!({"chat_id": "123"}))
            .expect_err("missing bot_token_env should fail");
        let missing_chat = TelegramChannelConfig::parse(&json!({"bot_token_env": "TELEGRAM_BOT_TOKEN"}))
            .expect_err("missing chat_id should fail");

        assert_eq!(missing_token.message, "telegram bot_token_env is required");
        assert_eq!(missing_chat.message, "telegram chat_id is required");
    }

    #[test]
    fn webhook_channel_config_requires_http_url() {
        let missing_url = WebhookChannelConfig::parse(&json!({}))
            .expect_err("missing url should fail");
        let invalid_scheme = WebhookChannelConfig::parse(&json!({"url": "ftp://example.com"}))
            .expect_err("non-http url should fail");

        assert_eq!(missing_url.message, "webhook url is required");
        assert_eq!(invalid_scheme.message, "webhook url must use http or https");
    }

    #[test]
    fn webhook_channel_config_defaults_timeout() {
        let config = WebhookChannelConfig::parse(&json!({
            "url": "https://example.com/hook"
        }))
        .expect("valid webhook config");

        assert_eq!(config.timeout_ms, 5000);
        assert_eq!(config.secret_env, None);
    }

    #[test]
    fn notification_idempotency_key_is_stable_for_same_rule_channel() {
        let key = notification_idempotency_key(uuid(1), uuid(2), uuid(3), uuid(4));
        let same_key = notification_idempotency_key(uuid(1), uuid(2), uuid(3), uuid(4));

        assert_eq!(key, same_key);
        assert_eq!(
            key,
            "notification:v1:00000000-0000-0000-0000-000000000001:00000000-0000-0000-0000-000000000002:00000000-0000-0000-0000-000000000003:00000000-0000-0000-0000-000000000004"
        );
    }

    #[test]
    fn notification_idempotency_key_changes_for_different_channel() {
        let first = notification_idempotency_key(uuid(1), uuid(2), uuid(3), uuid(4));
        let second = notification_idempotency_key(uuid(1), uuid(2), uuid(3), uuid(5));

        assert_ne!(first, second);
    }

    #[test]
    fn redaction_removes_token_and_webhook_query() {
        assert_eq!(
            redact_telegram_url("https://api.telegram.org/bot123:secret/sendMessage"),
            "https://api.telegram.org/bot<redacted>/sendMessage"
        );
        assert_eq!(
            redact_webhook_url("https://example.com/hook?token=secret"),
            "https://example.com/hook"
        );
    }
}
```

- [ ] **Step 2: Wire module and dependencies, then verify RED**

In `backend/crates/notifier/src/lib.rs`, add this near the top:

```rust
pub mod external;
```

In `backend/crates/notifier/Cargo.toml`, add dependencies under `[dependencies]`:

```toml
hmac = "0.12"
reqwest.workspace = true
serde.workspace = true
serde_json.workspace = true
sha2 = "0.10"
```

In `backend/crates/core/src/error.rs`, add this enum variant after `Redis`:

```rust
#[error("external notification error: {0}")]
ExternalNotification(String),
```

Run:

```bash
cargo test -p notifier telegram_channel_config_requires_token_env_and_chat_id --manifest-path backend/Cargo.toml
```

Expected: FAIL because the external config/idempotency types and functions do not exist.

- [ ] **Step 3: Implement config parsing and redaction**

Replace `backend/crates/notifier/src/external.rs` with:

```rust
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalChannelType {
    Telegram,
    Webhook,
}

impl ExternalChannelType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Telegram => "telegram",
            Self::Webhook => "webhook",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalConfigError {
    pub message: String,
}

impl ExternalConfigError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramChannelConfig {
    pub bot_token_env: String,
    pub chat_id: String,
}

impl TelegramChannelConfig {
    pub fn parse(value: &Value) -> Result<Self, ExternalConfigError> {
        let bot_token_env = required_string(value, "bot_token_env", "telegram bot_token_env is required")?;
        let chat_id = required_string(value, "chat_id", "telegram chat_id is required")?;
        Ok(Self {
            bot_token_env,
            chat_id,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookChannelConfig {
    pub url: String,
    pub secret_env: Option<String>,
    pub timeout_ms: u64,
}

impl WebhookChannelConfig {
    pub fn parse(value: &Value) -> Result<Self, ExternalConfigError> {
        let url = required_string(value, "url", "webhook url is required")?;
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(ExternalConfigError::new("webhook url must use http or https"));
        }
        let secret_env = optional_string(value, "secret_env");
        let timeout_ms = value
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(5000)
            .clamp(1000, 30000);
        Ok(Self {
            url,
            secret_env,
            timeout_ms,
        })
    }
}

pub fn notification_idempotency_key(
    tenant_id: Uuid,
    event_id: Uuid,
    rule_id: Uuid,
    channel_id: Uuid,
) -> String {
    format!("notification:v1:{tenant_id}:{event_id}:{rule_id}:{channel_id}")
}

pub fn redact_telegram_url(url: &str) -> String {
    let Some(bot_index) = url.find("/bot") else {
        return url.to_string();
    };
    let token_start = bot_index + 4;
    let Some(relative_end) = url[token_start..].find('/') else {
        return format!("{}<redacted>", &url[..token_start]);
    };
    let token_end = token_start + relative_end;
    format!("{}<redacted>{}", &url[..token_start], &url[token_end..])
}

pub fn redact_webhook_url(url: &str) -> String {
    url.split('?').next().unwrap_or(url).to_string()
}

fn required_string(
    value: &Value,
    key: &str,
    message: &'static str,
) -> Result<String, ExternalConfigError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ExternalConfigError::new(message))
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}
```

Keep the test module from Step 1 at the bottom of the file.

- [ ] **Step 4: Run tests to verify GREEN**

Run:

```bash
cargo test -p notifier telegram_channel_config_requires_token_env_and_chat_id --manifest-path backend/Cargo.toml
cargo test -p notifier webhook_channel_config_requires_http_url --manifest-path backend/Cargo.toml
cargo test -p notifier webhook_channel_config_defaults_timeout --manifest-path backend/Cargo.toml
cargo test -p notifier notification_idempotency_key_is_stable_for_same_rule_channel --manifest-path backend/Cargo.toml
cargo test -p notifier notification_idempotency_key_changes_for_different_channel --manifest-path backend/Cargo.toml
cargo test -p notifier redaction_removes_token_and_webhook_query --manifest-path backend/Cargo.toml
cargo check -p notifier --manifest-path backend/Cargo.toml
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 5: Commit Task 3**

Run:

```bash
git status --short
git add backend/crates/notifier/Cargo.toml backend/crates/notifier/src/lib.rs backend/crates/notifier/src/external.rs backend/crates/core/src/error.rs
git commit -m "Add external notification config parsing"
```

Expected: commit succeeds and includes the new module, dependency updates, and core error variant.

---

### Task 4: Add Webhook payload, signature, and response classification

**Files:**
- Modify: `backend/crates/notifier/src/external.rs`
- Test: `backend/crates/notifier/src/external.rs` existing `#[cfg(test)] mod tests`

- [ ] **Step 1: Write failing Webhook tests**

Add these imports to the external test module:

```rust
use chrono::{TimeZone, Utc};
use coin_listener_core::models::AddressEvent;
```

Add these helper/test functions to the external test module:

```rust
fn event() -> AddressEvent {
    AddressEvent {
        id: uuid(11),
        tenant_id: uuid(12),
        chain_id: uuid(13),
        address_id: uuid(14),
        asset_id: uuid(15),
        event_type: "transfer".to_string(),
        direction: "in".to_string(),
        is_transfer: true,
        tx_hash: Some("0xabc".to_string()),
        log_index: Some(0),
        block_number: Some(123),
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
        detected_at: Utc.with_ymd_and_hms(2026, 5, 18, 15, 0, 0).unwrap(),
        created_at: Utc.with_ymd_and_hms(2026, 5, 18, 15, 0, 1).unwrap(),
    }
}

#[test]
fn webhook_sender_includes_idempotency_headers() {
    let request = build_webhook_request_parts(
        &WebhookChannelConfig {
            url: "https://example.com/hook".to_string(),
            secret_env: None,
            timeout_ms: 5000,
        },
        &event(),
        "notification:key",
        None,
    )
    .expect("build webhook request");

    assert_eq!(request.url, "https://example.com/hook");
    assert_eq!(
        request.headers.get("X-Coin-Listener-Event-Id").map(String::as_str),
        Some("00000000-0000-0000-0000-00000000000b")
    );
    assert_eq!(
        request
            .headers
            .get("X-Coin-Listener-Idempotency-Key")
            .map(String::as_str),
        Some("notification:key")
    );
    assert!(request.body.contains("\"idempotency_key\":\"notification:key\""));
}

#[test]
fn webhook_sender_signs_payload_when_secret_env_is_set() {
    let request = build_webhook_request_parts(
        &WebhookChannelConfig {
            url: "https://example.com/hook".to_string(),
            secret_env: Some("WEBHOOK_SECRET".to_string()),
            timeout_ms: 5000,
        },
        &event(),
        "notification:key",
        Some("secret-value"),
    )
    .expect("build signed webhook request");

    let signature = request
        .headers
        .get("X-Coin-Listener-Signature")
        .expect("signature header");
    assert_eq!(signature.len(), 64);
    assert!(signature.chars().all(|character| character.is_ascii_hexdigit()));
}

#[test]
fn webhook_status_classification_distinguishes_retryable_and_permanent_failures() {
    assert!(classify_webhook_response(202, "accepted").is_sent());
    assert!(classify_webhook_response(429, "rate limited").is_transient_failure());
    assert!(classify_webhook_response(500, "server error").is_transient_failure());
    assert!(classify_webhook_response(401, "unauthorized").is_permanent_failure());
}

#[test]
fn webhook_sender_redacts_query_string_from_errors() {
    let message = webhook_network_error_message(
        "https://example.com/hook?token=secret",
        "connection reset",
    );

    assert!(message.contains("https://example.com/hook"));
    assert!(!message.contains("token=secret"));
}
```

Update the external test module import list to include:

```rust
build_webhook_request_parts, classify_webhook_response, webhook_network_error_message,
```

- [ ] **Step 2: Run tests to verify RED**

Run:

```bash
cargo test -p notifier webhook_sender_includes_idempotency_headers --manifest-path backend/Cargo.toml
cargo test -p notifier webhook_sender_signs_payload_when_secret_env_is_set --manifest-path backend/Cargo.toml
cargo test -p notifier webhook_status_classification_distinguishes_retryable_and_permanent_failures --manifest-path backend/Cargo.toml
cargo test -p notifier webhook_sender_redacts_query_string_from_errors --manifest-path backend/Cargo.toml
```

Expected: FAIL because Webhook request/signature/classification helpers do not exist.

- [ ] **Step 3: Implement Webhook helpers**

In `backend/crates/notifier/src/external.rs`, add these imports at the top:

```rust
use coin_listener_core::models::AddressEvent;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::{collections::BTreeMap, time::Duration};
```

Add these structs and helpers before the test module:

```rust
type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalSendMetadata {
    pub last_error: Option<String>,
    pub provider_message_id: Option<String>,
    pub provider_status_code: Option<i32>,
    pub provider_response: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalSendOutcome {
    Sent(ExternalSendMetadata),
    PermanentFailure(ExternalSendMetadata),
    TransientFailure(ExternalSendMetadata),
}

impl ExternalSendOutcome {
    pub fn is_sent(&self) -> bool {
        matches!(self, Self::Sent(_))
    }

    pub fn is_permanent_failure(&self) -> bool {
        matches!(self, Self::PermanentFailure(_))
    }

    pub fn is_transient_failure(&self) -> bool {
        matches!(self, Self::TransientFailure(_))
    }

    pub fn metadata(&self) -> &ExternalSendMetadata {
        match self {
            Self::Sent(metadata)
            | Self::PermanentFailure(metadata)
            | Self::TransientFailure(metadata) => metadata,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookRequestParts {
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: String,
    pub timeout: Duration,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebhookPayload {
    pub idempotency_key: String,
    pub event_id: Uuid,
    pub tenant_id: Uuid,
    pub chain_id: Uuid,
    pub address_id: Uuid,
    pub asset_id: Uuid,
    pub event_type: String,
    pub direction: String,
    pub is_transfer: bool,
    pub tx_hash: Option<String>,
    pub block_number: Option<i64>,
    pub from_address: Option<String>,
    pub to_address: Option<String>,
    pub amount_raw: Option<String>,
    pub amount_decimal: Option<String>,
    pub detected_at: String,
}

pub fn build_webhook_payload(event: &AddressEvent, idempotency_key: &str) -> WebhookPayload {
    WebhookPayload {
        idempotency_key: idempotency_key.to_string(),
        event_id: event.id,
        tenant_id: event.tenant_id,
        chain_id: event.chain_id,
        address_id: event.address_id,
        asset_id: event.asset_id,
        event_type: event.event_type.clone(),
        direction: event.direction.clone(),
        is_transfer: event.is_transfer,
        tx_hash: event.tx_hash.clone(),
        block_number: event.block_number,
        from_address: event.from_address.clone(),
        to_address: event.to_address.clone(),
        amount_raw: event.amount_raw.clone(),
        amount_decimal: event.amount_decimal.clone(),
        detected_at: event.detected_at.to_rfc3339(),
    }
}

pub fn build_webhook_request_parts(
    config: &WebhookChannelConfig,
    event: &AddressEvent,
    idempotency_key: &str,
    secret: Option<&str>,
) -> Result<WebhookRequestParts, ExternalConfigError> {
    let payload = build_webhook_payload(event, idempotency_key);
    let body = serde_json::to_string(&payload)
        .map_err(|error| ExternalConfigError::new(format!("webhook payload serialization failed: {error}")))?;
    let mut headers = BTreeMap::new();
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    headers.insert(
        "X-Coin-Listener-Event-Id".to_string(),
        event.id.to_string(),
    );
    headers.insert(
        "X-Coin-Listener-Idempotency-Key".to_string(),
        idempotency_key.to_string(),
    );
    if let Some(secret) = secret {
        headers.insert(
            "X-Coin-Listener-Signature".to_string(),
            webhook_signature(secret, body.as_bytes()),
        );
    }
    Ok(WebhookRequestParts {
        url: config.url.clone(),
        headers,
        body,
        timeout: Duration::from_millis(config.timeout_ms),
    })
}

pub fn webhook_signature(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts keys of any size");
    mac.update(body);
    bytes_to_lower_hex(&mac.finalize().into_bytes())
}

pub fn bytes_to_lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

pub fn classify_webhook_response(status_code: u16, body: &str) -> ExternalSendOutcome {
    let metadata = ExternalSendMetadata {
        last_error: None,
        provider_message_id: None,
        provider_status_code: Some(status_code as i32),
        provider_response: Some(truncate_provider_response(body)),
    };
    match status_code {
        200..=299 => ExternalSendOutcome::Sent(metadata),
        408 | 429 | 500..=599 => ExternalSendOutcome::TransientFailure(ExternalSendMetadata {
            last_error: Some(format!("webhook returned retryable status {status_code}")),
            ..metadata
        }),
        _ => ExternalSendOutcome::PermanentFailure(ExternalSendMetadata {
            last_error: Some(format!("webhook returned permanent status {status_code}")),
            ..metadata
        }),
    }
}

pub fn webhook_network_error_message(url: &str, error: &str) -> String {
    format!("webhook {} failed: {error}", redact_webhook_url(url))
}

pub fn truncate_provider_response(body: &str) -> String {
    body.chars().take(2048).collect()
}
```

- [ ] **Step 4: Run tests to verify GREEN**

Run:

```bash
cargo test -p notifier webhook_sender_includes_idempotency_headers --manifest-path backend/Cargo.toml
cargo test -p notifier webhook_sender_signs_payload_when_secret_env_is_set --manifest-path backend/Cargo.toml
cargo test -p notifier webhook_status_classification_distinguishes_retryable_and_permanent_failures --manifest-path backend/Cargo.toml
cargo test -p notifier webhook_sender_redacts_query_string_from_errors --manifest-path backend/Cargo.toml
cargo check -p notifier --manifest-path backend/Cargo.toml
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 5: Commit Task 4**

Run:

```bash
git status --short
git add backend/crates/notifier/src/external.rs
git commit -m "Add webhook notification send helpers"
```

Expected: commit succeeds and contains only Webhook helper changes.

---

### Task 5: Add Telegram response classification and reusable sender

**Files:**
- Modify: `backend/crates/notifier/src/external.rs`
- Test: `backend/crates/notifier/src/external.rs` existing `#[cfg(test)] mod tests`

- [ ] **Step 1: Write failing Telegram sender tests**

Add these tests to `backend/crates/notifier/src/external.rs` test module:

```rust
#[test]
fn telegram_sender_classifies_success_as_sent() {
    let outcome = classify_telegram_response(
        200,
        r#"{"ok":true,"result":{"message_id":123}}"#,
    );

    assert!(outcome.is_sent());
    assert_eq!(
        outcome.metadata().provider_message_id.as_deref(),
        Some("123")
    );
    assert_eq!(outcome.metadata().provider_status_code, Some(200));
}

#[test]
fn telegram_sender_classifies_rate_limit_as_retryable() {
    let outcome = classify_telegram_response(
        429,
        r#"{"ok":false,"description":"Too Many Requests"}"#,
    );

    assert!(outcome.is_transient_failure());
    assert!(outcome
        .metadata()
        .last_error
        .as_deref()
        .unwrap_or("")
        .contains("retryable status 429"));
}

#[test]
fn telegram_sender_classifies_auth_error_as_permanent() {
    let outcome = classify_telegram_response(
        401,
        r#"{"ok":false,"description":"Unauthorized"}"#,
    );

    assert!(outcome.is_permanent_failure());
    assert!(outcome
        .metadata()
        .last_error
        .as_deref()
        .unwrap_or("")
        .contains("permanent status 401"));
}

#[test]
fn telegram_request_url_redacts_token_from_error_message() {
    let error = telegram_network_error_message(
        "https://api.telegram.org/bot123:secret/sendMessage",
        "timeout",
    );

    assert!(error.contains("bot<redacted>"));
    assert!(!error.contains("123:secret"));
}
```

Update the external test module import list to include:

```rust
classify_telegram_response, telegram_network_error_message,
```

- [ ] **Step 2: Run tests to verify RED**

Run:

```bash
cargo test -p notifier telegram_sender_classifies_success_as_sent --manifest-path backend/Cargo.toml
cargo test -p notifier telegram_sender_classifies_rate_limit_as_retryable --manifest-path backend/Cargo.toml
cargo test -p notifier telegram_sender_classifies_auth_error_as_permanent --manifest-path backend/Cargo.toml
cargo test -p notifier telegram_request_url_redacts_token_from_error_message --manifest-path backend/Cargo.toml
```

Expected: FAIL because Telegram response helpers do not exist.

- [ ] **Step 3: Implement Telegram classification and sender struct**

In `backend/crates/notifier/src/external.rs`, add these imports:

```rust
use reqwest::Client;
```

Add this sender and Telegram logic before the test module:

```rust
#[derive(Debug, Clone)]
pub struct ExternalNotificationSender {
    client: Client,
    telegram_api_base_url: String,
}

impl ExternalNotificationSender {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            telegram_api_base_url: "https://api.telegram.org".to_string(),
        }
    }

    #[cfg(test)]
    pub fn with_telegram_api_base_url(client: Client, telegram_api_base_url: String) -> Self {
        Self {
            client,
            telegram_api_base_url,
        }
    }

    pub async fn send_telegram(
        &self,
        config: &TelegramChannelConfig,
        bot_token: &str,
        text: &str,
    ) -> ExternalSendOutcome {
        let url = format!(
            "{}/bot{}/sendMessage",
            self.telegram_api_base_url.trim_end_matches('/'),
            bot_token
        );
        let response = self
            .client
            .post(&url)
            .json(&serde_json::json!({
                "chat_id": config.chat_id,
                "text": text,
            }))
            .send()
            .await;

        match response {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = response.text().await.unwrap_or_default();
                classify_telegram_response(status, &body)
            }
            Err(error) => ExternalSendOutcome::TransientFailure(ExternalSendMetadata {
                last_error: Some(telegram_network_error_message(&url, &error.to_string())),
                provider_message_id: None,
                provider_status_code: None,
                provider_response: None,
            }),
        }
    }

    pub async fn send_webhook(
        &self,
        parts: WebhookRequestParts,
    ) -> ExternalSendOutcome {
        let mut request = self
            .client
            .post(&parts.url)
            .timeout(parts.timeout)
            .body(parts.body);
        for (name, value) in parts.headers {
            request = request.header(name, value);
        }
        match request.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = response.text().await.unwrap_or_default();
                classify_webhook_response(status, &body)
            }
            Err(error) => ExternalSendOutcome::TransientFailure(ExternalSendMetadata {
                last_error: Some(webhook_network_error_message(&parts.url, &error.to_string())),
                provider_message_id: None,
                provider_status_code: None,
                provider_response: None,
            }),
        }
    }
}

pub fn classify_telegram_response(status_code: u16, body: &str) -> ExternalSendOutcome {
    let provider_message_id = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("result").cloned())
        .and_then(|result| result.get("message_id").cloned())
        .and_then(|message_id| message_id.as_i64().map(|value| value.to_string()));

    let metadata = ExternalSendMetadata {
        last_error: None,
        provider_message_id,
        provider_status_code: Some(status_code as i32),
        provider_response: Some(truncate_provider_response(body)),
    };
    match status_code {
        200..=299 => ExternalSendOutcome::Sent(metadata),
        408 | 429 | 500..=599 => ExternalSendOutcome::TransientFailure(ExternalSendMetadata {
            last_error: Some(format!("telegram returned retryable status {status_code}")),
            ..metadata
        }),
        _ => ExternalSendOutcome::PermanentFailure(ExternalSendMetadata {
            last_error: Some(format!("telegram returned permanent status {status_code}")),
            ..metadata
        }),
    }
}

pub fn telegram_network_error_message(url: &str, error: &str) -> String {
    format!("telegram {} failed: {error}", redact_telegram_url(url))
}

pub fn render_external_notification_text(event: &AddressEvent) -> String {
    let amount = event
        .amount_decimal
        .as_deref()
        .or(event.amount_raw.as_deref())
        .unwrap_or("-");
    let tx_hash = event.tx_hash.as_deref().unwrap_or("-");
    format!(
        "{} {}\naddress: {}\nasset: {}\namount: {}\ntx: {}",
        event.event_type, event.direction, event.address_id, event.asset_id, amount, tx_hash
    )
}
```

- [ ] **Step 4: Run tests to verify GREEN**

Run:

```bash
cargo test -p notifier telegram_sender_classifies_success_as_sent --manifest-path backend/Cargo.toml
cargo test -p notifier telegram_sender_classifies_rate_limit_as_retryable --manifest-path backend/Cargo.toml
cargo test -p notifier telegram_sender_classifies_auth_error_as_permanent --manifest-path backend/Cargo.toml
cargo test -p notifier telegram_request_url_redacts_token_from_error_message --manifest-path backend/Cargo.toml
cargo check -p notifier --manifest-path backend/Cargo.toml
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 5: Commit Task 5**

Run:

```bash
git status --short
git add backend/crates/notifier/src/external.rs
git commit -m "Add telegram notification send helpers"
```

Expected: commit succeeds and contains only Telegram/sender changes.

---

### Task 6: Integrate Telegram and Webhook into notifier processing

**Files:**
- Modify: `backend/crates/notifier/src/lib.rs:1-535`
- Modify: `backend/crates/notifier/src/main.rs:20-42`
- Test: `backend/crates/notifier/src/lib.rs` existing `#[cfg(test)] mod tests`

- [ ] **Step 1: Write failing notifier integration tests**

In `backend/crates/notifier/src/lib.rs`, update the test import list to include:

```rust
crate::external::{ExternalChannelType, ExternalSendMetadata, ExternalSendOutcome};
```

Also include `DELIVERY_STATUS_PROCESSING` and `external_send_outcome_result` in the existing `use crate::{ ... }` test import list.

Add these tests to the existing test module:

```rust
#[test]
fn notifier_treats_telegram_and_webhook_as_sendable_channels() {
    assert_eq!(
        notify_channel_decision("telegram"),
        NotifyChannelDecision::External {
            channel_type: ExternalChannelType::Telegram
        }
    );
    assert_eq!(
        notify_channel_decision("webhook"),
        NotifyChannelDecision::External {
            channel_type: ExternalChannelType::Webhook
        }
    );
}

#[test]
fn external_channel_delivery_plan_records_sendable_channel_type() {
    let telegram = build_delivery_plan(&channel("telegram"));
    let webhook = build_delivery_plan(&channel("webhook"));

    assert_eq!(telegram.status, DELIVERY_STATUS_PROCESSING);
    assert_eq!(telegram.external_channel_type, Some(ExternalChannelType::Telegram));
    assert!(!telegram.create_in_app);
    assert_eq!(webhook.status, DELIVERY_STATUS_PROCESSING);
    assert_eq!(webhook.external_channel_type, Some(ExternalChannelType::Webhook));
    assert!(!webhook.create_in_app);
}

#[test]
fn transient_external_send_error_keeps_outbox_retryable() {
    let outcome = ExternalSendOutcome::TransientFailure(ExternalSendMetadata {
        last_error: Some("webhook returned retryable status 429".to_string()),
        provider_message_id: None,
        provider_status_code: Some(429),
        provider_response: Some("rate limited".to_string()),
    });

    let result = external_send_outcome_result(&outcome);

    assert!(matches!(
        result,
        Err(AppError::ExternalNotification(message)) if message.contains("retryable status 429")
    ));
}

#[test]
fn permanent_external_send_error_does_not_trigger_outbox_retry() {
    let outcome = ExternalSendOutcome::PermanentFailure(ExternalSendMetadata {
        last_error: Some("webhook returned permanent status 401".to_string()),
        provider_message_id: None,
        provider_status_code: Some(401),
        provider_response: Some("unauthorized".to_string()),
    });

    assert!(external_send_outcome_result(&outcome).is_ok());
}
```

- [ ] **Step 2: Run tests to verify RED**

Run:

```bash
cargo test -p notifier notifier_treats_telegram_and_webhook_as_sendable_channels --manifest-path backend/Cargo.toml
cargo test -p notifier external_channel_delivery_plan_records_sendable_channel_type --manifest-path backend/Cargo.toml
cargo test -p notifier transient_external_send_error_keeps_outbox_retryable --manifest-path backend/Cargo.toml
cargo test -p notifier permanent_external_send_error_does_not_trigger_outbox_retry --manifest-path backend/Cargo.toml
```

Expected: FAIL. Telegram/Webhook are still skipped, delivery plans lack `external_channel_type`, and `external_send_outcome_result` does not exist.

- [ ] **Step 3: Extend channel decision and delivery plan types**

In `backend/crates/notifier/src/lib.rs`, add these imports near the top:

```rust
use crate::external::{
    build_webhook_request_parts, notification_idempotency_key, render_external_notification_text,
    ExternalChannelType, ExternalNotificationSender, ExternalSendOutcome, TelegramChannelConfig,
    WebhookChannelConfig,
};
use coin_listener_core::AppError;
use coin_listener_storage::notifications::ExternalDeliveryStart;
```

Replace `NotifyChannelDecision` with:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyChannelDecision {
    InApp,
    External { channel_type: ExternalChannelType },
    Skipped { last_error: &'static str },
}
```

Add this constant near the existing delivery status constants:

```rust
pub const DELIVERY_STATUS_PROCESSING: &str = "processing";
```

Replace `NotifyDeliveryPlan` with:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyDeliveryPlan {
    pub channel_id: uuid::Uuid,
    pub channel_type: Option<String>,
    pub status: &'static str,
    pub last_error: Option<&'static str>,
    pub create_in_app: bool,
    pub external_channel_type: Option<ExternalChannelType>,
}
```

Update `notify_channel_decision`:

```rust
pub fn notify_channel_decision(channel_type: &str) -> NotifyChannelDecision {
    match channel_type {
        "in_app" => NotifyChannelDecision::InApp,
        "telegram" => NotifyChannelDecision::External {
            channel_type: ExternalChannelType::Telegram,
        },
        "webhook" => NotifyChannelDecision::External {
            channel_type: ExternalChannelType::Webhook,
        },
        _ => NotifyChannelDecision::Skipped {
            last_error: NOT_IMPLEMENTED_CHANNEL_ERROR,
        },
    }
}
```

Update `build_delivery_plan` so each branch fills `external_channel_type`:

```rust
pub fn build_delivery_plan(channel: &NotificationChannel) -> NotifyDeliveryPlan {
    match notify_channel_decision(&channel.channel_type) {
        NotifyChannelDecision::InApp => NotifyDeliveryPlan {
            channel_id: channel.id,
            channel_type: Some(channel.channel_type.clone()),
            status: DELIVERY_STATUS_SENT,
            last_error: None,
            create_in_app: true,
            external_channel_type: None,
        },
        NotifyChannelDecision::External { channel_type } => NotifyDeliveryPlan {
            channel_id: channel.id,
            channel_type: Some(channel.channel_type.clone()),
            status: DELIVERY_STATUS_PROCESSING,
            last_error: None,
            create_in_app: false,
            external_channel_type: Some(channel_type),
        },
        NotifyChannelDecision::Skipped { last_error } => NotifyDeliveryPlan {
            channel_id: channel.id,
            channel_type: Some(channel.channel_type.clone()),
            status: DELIVERY_STATUS_SKIPPED,
            last_error: Some(last_error),
            create_in_app: false,
            external_channel_type: None,
        },
    }
}
```

Update `build_unavailable_channel_delivery_plan` to set:

```rust
external_channel_type: None,
```

- [ ] **Step 4: Add external outcome helper**

Add this function before `sent_in_app_delivery_result`:

```rust
pub fn external_send_outcome_result(outcome: &ExternalSendOutcome) -> AppResult<()> {
    match outcome {
        ExternalSendOutcome::Sent(_) | ExternalSendOutcome::PermanentFailure(_) => Ok(()),
        ExternalSendOutcome::TransientFailure(metadata) => Err(AppError::ExternalNotification(
            metadata
                .last_error
                .clone()
                .unwrap_or_else(|| "external notification transient failure".to_string()),
        )),
    }
}
```

- [ ] **Step 5: Inject sender through notifier processing signatures**

Change signatures:

```rust
pub async fn process_notify_task(
    pool: &PgPool,
    task: NotifyEventTask,
    now: DateTime<Utc>,
    sender: &ExternalNotificationSender,
) -> AppResult<usize>
```

```rust
pub async fn process_notification_outbox_item(
    pool: &PgPool,
    item: NotificationOutboxItem,
    now: DateTime<Utc>,
    sender: &ExternalNotificationSender,
) -> AppResult<usize>
```

```rust
pub async fn process_notification_outbox_batch(
    pool: &PgPool,
    worker_id: &str,
    config: &NotificationOutboxDispatcherConfig,
    sender: &ExternalNotificationSender,
    now: DateTime<Utc>,
) -> AppResult<usize>
```

Update calls so `sender` is passed from batch to item to task to channel processing.

Update `process_resolved_channel` and `process_channel_delivery` signatures to receive:

```rust
sender: &ExternalNotificationSender,
```

Pass `sender` into the active-channel branch only; inactive and missing channels still create skipped deliveries without HTTP calls.

- [ ] **Step 6: Implement external channel processing**

In `process_channel_delivery`, after the in-app branch and before the skipped delivery insert, add:

```rust
if let Some(external_channel_type) = plan.external_channel_type {
    return process_external_channel_delivery(
        pool,
        sender,
        task,
        event,
        rule,
        plan.channel_id,
        external_channel_type,
        now,
    )
    .await;
}
```

Add this function before `process_channel_delivery`:

```rust
async fn process_external_channel_delivery(
    pool: &PgPool,
    sender: &ExternalNotificationSender,
    task: &NotifyEventTask,
    event: &AddressEvent,
    rule: &NotificationRule,
    channel_id: uuid::Uuid,
    channel_type: ExternalChannelType,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let idempotency_key = notification_idempotency_key(
        task.tenant_id,
        task.event_id,
        rule.id,
        channel_id,
    );
    let start = notifications::begin_external_notification_delivery(
        pool,
        task.tenant_id,
        task.event_id,
        rule.id,
        channel_id,
        channel_type.as_str(),
        &idempotency_key,
        task.attempt as i32,
    )
    .await?;

    let ExternalDeliveryStart::ReadyToSend { delivery_id } = start else {
        return Ok(());
    };

    let channel = notifications::list_channels_by_ids(pool, task.tenant_id, &[channel_id])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| AppError::NotFound("external notification channel".to_string()))?;

    let outcome = match channel_type {
        ExternalChannelType::Telegram => {
            let config = match TelegramChannelConfig::parse(&channel.config) {
                Ok(config) => config,
                Err(error) => {
                    notifications::mark_external_notification_delivery_failed(
                        pool,
                        task.tenant_id,
                        delivery_id,
                        &error.message,
                        None,
                        None,
                    )
                    .await?;
                    return Ok(());
                }
            };
            let bot_token = match std::env::var(&config.bot_token_env) {
                Ok(token) => token,
                Err(_) => {
                    let message = format!("telegram token env {} is not set", config.bot_token_env);
                    notifications::mark_external_notification_delivery_failed(
                        pool,
                        task.tenant_id,
                        delivery_id,
                        &message,
                        None,
                        None,
                    )
                    .await?;
                    return Ok(());
                }
            };
            sender
                .send_telegram(&config, &bot_token, &render_external_notification_text(event))
                .await
        }
        ExternalChannelType::Webhook => {
            let config = match WebhookChannelConfig::parse(&channel.config) {
                Ok(config) => config,
                Err(error) => {
                    notifications::mark_external_notification_delivery_failed(
                        pool,
                        task.tenant_id,
                        delivery_id,
                        &error.message,
                        None,
                        None,
                    )
                    .await?;
                    return Ok(());
                }
            };
            let secret = config
                .secret_env
                .as_deref()
                .map(std::env::var)
                .transpose()
                .map_err(|_| AppError::ExternalNotification("webhook secret env is not set".to_string()))?;
            let parts = build_webhook_request_parts(
                &config,
                event,
                &idempotency_key,
                secret.as_deref(),
            )
            .map_err(|error| AppError::ExternalNotification(error.message))?;
            sender.send_webhook(parts).await
        }
    };

    let metadata = outcome.metadata();
    match &outcome {
        ExternalSendOutcome::Sent(_) => {
            notifications::mark_external_notification_delivery_sent(
                pool,
                task.tenant_id,
                delivery_id,
                now,
                metadata.provider_message_id.as_deref(),
                metadata.provider_status_code,
                metadata.provider_response.as_deref(),
            )
            .await?;
        }
        ExternalSendOutcome::PermanentFailure(_) | ExternalSendOutcome::TransientFailure(_) => {
            notifications::mark_external_notification_delivery_failed(
                pool,
                task.tenant_id,
                delivery_id,
                metadata
                    .last_error
                    .as_deref()
                    .unwrap_or("external notification failed"),
                metadata.provider_status_code,
                metadata.provider_response.as_deref(),
            )
            .await?;
        }
    }

    external_send_outcome_result(&outcome)
}
```

- [ ] **Step 7: Reuse one HTTP client from notifier main loop**

In `backend/crates/notifier/src/main.rs`, add:

```rust
use notifier::external::ExternalNotificationSender;
```

Before `run_notifier`, create one sender:

```rust
let http_client = reqwest::Client::new();
let external_sender = ExternalNotificationSender::new(http_client);
```

Change the final call to:

```rust
run_notifier(postgres, dispatcher_config, external_sender, shutdown).await?;
```

In `backend/crates/notifier/src/lib.rs`, change `run_notifier` signature:

```rust
pub async fn run_notifier(
    pool: PgPool,
    config: NotificationOutboxDispatcherConfig,
    sender: ExternalNotificationSender,
    shutdown: Arc<AtomicBool>,
) -> AppResult<()>
```

Inside the loop, call:

```rust
process_notification_outbox_batch(&pool, &worker_id, &config, &sender, Utc::now()).await
```

- [ ] **Step 8: Update existing tests for new fields/signatures**

Update existing notifier tests:

- `unsupported_channel_is_skipped`: change input from `telegram` to `email`.
- `unsupported_channel_delivery_plan_is_skipped_without_in_app`: change channel type from `telegram` to `email`.
- Any direct `NotifyDeliveryPlan` construction must include `external_channel_type: None`.
- Any `NotifyChannelDecision::Skipped` expectation for Telegram/Webhook must be replaced by the new sendable expectations from Step 1.

- [ ] **Step 9: Run tests to verify GREEN**

Run:

```bash
cargo test -p notifier notifier_treats_telegram_and_webhook_as_sendable_channels --manifest-path backend/Cargo.toml
cargo test -p notifier external_channel_delivery_plan_records_sendable_channel_type --manifest-path backend/Cargo.toml
cargo test -p notifier transient_external_send_error_keeps_outbox_retryable --manifest-path backend/Cargo.toml
cargo test -p notifier permanent_external_send_error_does_not_trigger_outbox_retry --manifest-path backend/Cargo.toml
cargo test -p notifier unsupported_channel_is_skipped --manifest-path backend/Cargo.toml
cargo check -p notifier --manifest-path backend/Cargo.toml
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 10: Commit Task 6**

Run:

```bash
git status --short
git add backend/crates/notifier/src/lib.rs backend/crates/notifier/src/main.rs
git commit -m "Send external notification channels from notifier"
```

Expected: commit succeeds and contains only notifier integration changes.

---

### Task 7: Final verification

**Files:**
- Verify: full backend workspace, frontend build, docker compose config

- [ ] **Step 1: Run full backend formatting check**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: exits 0.

- [ ] **Step 2: Run full backend check**

Run:

```bash
cargo check --workspace --manifest-path backend/Cargo.toml
```

Expected: exits 0. The existing `sqlx-postgres` future-incompatibility warning may appear and is acceptable.

- [ ] **Step 3: Run full backend tests**

Run:

```bash
cargo test --workspace --manifest-path backend/Cargo.toml
```

Expected: exits 0. The existing `sqlx-postgres` future-incompatibility warning may appear and is acceptable.

- [ ] **Step 4: Run frontend build**

Run:

```bash
npm run build --prefix frontend
```

Expected: exits 0. Existing Vite warnings about `lottie-web` direct eval and large chunks may appear and are acceptable.

- [ ] **Step 5: Run docker compose config without overwriting `.env`**

Run:

```bash
if [ -f .env ]; then docker compose -f docker-compose.yml config >/tmp/coin-listener-compose-config.txt; else touch .env && rc=0; docker compose -f docker-compose.yml config >/tmp/coin-listener-compose-config.txt || rc=$?; rm .env; exit $rc; fi
```

Expected: exits 0 and leaves no new `.env` file behind when `.env` was absent.

- [ ] **Step 6: Confirm working tree and commit verification marker if needed**

Run:

```bash
git status --short
```

Expected: no uncommitted source changes. If verification required formatting changes, commit those exact formatting changes with:

```bash
git add backend/crates/core/src/error.rs backend/crates/core/src/models.rs backend/crates/storage/src/notifications.rs backend/crates/notifier/Cargo.toml backend/crates/notifier/src/external.rs backend/crates/notifier/src/lib.rs backend/crates/notifier/src/main.rs
git commit -m "Verify external notification channels"
```

Do not create an empty commit if `git status --short` is clean.

---

## Self-Review Checklist

Spec coverage:

- Telegram real sending: Task 5 adds Telegram classification and HTTP sender; Task 6 wires it into notifier.
- Webhook real sending: Task 4 adds payload/signature/classification; Task 5 adds shared sender; Task 6 wires it into notifier.
- Channel-level idempotency: Task 1 adds metadata/index; Task 2 adds begin/reuse helpers; Task 6 calls them before sending.
- Delivery metadata: Task 1 extends model/schema; Task 2 writes sent/failed metadata.
- Error classification: Task 4 and Task 5 classify provider responses; Task 6 maps transient failures to `AppError::ExternalNotification` for outbox retry and permanent failures to completed channel processing.
- Reusable HTTP client: Task 5 defines `ExternalNotificationSender`; Task 6 creates one client in `main.rs` and passes it through the notifier loop.
- Verification: Task 7 runs backend fmt/check/test, frontend build, and compose config.

Type consistency:

- `ExternalChannelType`, `ExternalNotificationSender`, `ExternalSendOutcome`, and `ExternalSendMetadata` are defined in `notifier::external` before `lib.rs` imports them.
- `ExternalDeliveryStart` is defined in `coin_listener_storage::notifications` before notifier integration uses it.
- `NotificationDelivery` model fields match every storage `RETURNING` projection after Task 1.
- `run_notifier` receives `ExternalNotificationSender` by value and passes it by shared reference inside the loop.

Implementation constraints:

- No frontend UI is added.
- No generic secret manager is added.
- Telegram provider-side exactly-once is not claimed; system-level idempotency only prevents duplicate sends after a `sent` delivery is recorded.
