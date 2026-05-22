# Notification Channels and Address Import Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build consistent large form dialogs, Telegram bot and notification channel management, notification-rule channel quick creation, and backend task-style watched-address batch imports.

**Architecture:** Add backend models, migrations, storage helpers, protected Axum routes, and worker-side import processing before wiring the frontend pages. Keep Telegram bot credentials in backend storage and expose only token previews to the frontend; notification channels reference bots by ID. Use a DB-backed import task table so the API can return progress, cancellation, and failed-row details across page refreshes.

**Tech Stack:** Rust, Axum, SQLx, Tokio, Reqwest, PostgreSQL, React, TypeScript, Vite, TanStack Query, Semi Design, Node test runner.

---

## Scope and sequencing

This feature spans backend API/storage/worker and frontend UI. Implement sequentially through backend contracts first, then frontend contracts and UI.

| Phase | Tasks | Why this order |
| --- | --- | --- |
| Backend contracts | Tasks 1-6 | Frontend pages depend on stable API shapes and import task behavior. |
| Frontend contracts | Task 7 | TypeScript client and parser tests define UI integration points. |
| Frontend UI | Tasks 8-11 | Pages can call real typed clients. |
| Final verification | Task 12 | Run focused checks, then full build/test commands. |

---

## File structure

Create:

- `backend/crates/storage/migrations/0014_telegram_bots_and_address_imports.sql` — Telegram bot and watched-address import task schema.
- `backend/crates/storage/src/address_imports.rs` — task creation, progress queries, row errors, cancellation, claiming, row state transitions.
- `frontend/src/components/FormModal.tsx` — shared Semi Modal wrapper with `medium`, `large`, and `wide` sizes.
- `frontend/src/addressImport.ts` — pure parser for line and CSV watched-address import input.
- `frontend/src/pages/TelegramBotsPage.tsx` — Telegram bot management UI.
- `frontend/src/pages/NotificationChannelsPage.tsx` — notification channel management UI.

Modify:

- `backend/crates/core/src/models.rs` — add Telegram bot, channel operation, and address import DTOs.
- `backend/crates/storage/src/lib.rs` — export `address_imports`.
- `backend/crates/storage/src/notifications.rs` — add channel update/delete/get and Telegram bot storage helpers.
- `backend/crates/notifier/src/external.rs` — add Telegram bot verification and chat test-send helpers; support bot IDs in channel config.
- `backend/crates/notifier/src/lib.rs` — resolve Telegram bot IDs during delivery while preserving legacy `bot_token_env` support.
- `backend/crates/api-server/Cargo.toml` — add `notifier` and `reqwest` dependencies for verify/test APIs.
- `backend/crates/api-server/src/routes.rs` — add protected Telegram bot, notification channel, and import routes.
- `backend/crates/worker/src/lib.rs` — poll and process address import tasks.
- `backend/crates/worker/src/main.rs` — pass import processing through existing worker runtime.
- `backend/crates/all-in-one/src/main.rs` — run the worker with import processing inside all-in-one runtime.
- `frontend/src/api/types.ts` — add Telegram bot, channel operation, and import task types.
- `frontend/src/api/client.ts` — add client functions for bots, channels, verify/test, and imports.
- `frontend/src/App.tsx` — add page keys, nav items, and render branches.
- `frontend/src/pages/AddressesPage.tsx` — migrate create/edit modal and add batch import modal.
- `frontend/src/pages/NotificationRulesPage.tsx` — migrate modal and add channel refresh/quick-create.
- `frontend/src/ui-regression.test.ts` — add source-level UI/API regressions and parser tests.
- `frontend/src/styles.css` — add modal and import preview/progress layout styles.

---

## Shared contracts

Use these backend string values everywhere:

```text
notification channel status: active | inactive
telegram bot status: active | inactive
verification status: unverified | verified | failed
import task status: pending | running | completed | failed | cancelled
import row status: pending | success | failed | skipped
```

Telegram notification channel config shape:

```json
{
  "telegram_bot_id": "00000000-0000-0000-0000-000000000001",
  "chat_id": "-1001234567890",
  "chat_alias": "ops-alerts",
  "message_template": "optional template"
}
```

Legacy Telegram channel config remains accepted during delivery:

```json
{
  "bot_token_env": "TELEGRAM_BOT_TOKEN",
  "chat_id": "-1001234567890"
}
```

Address import create request shape:

```json
{
  "defaults": {
    "chain_id": "00000000-0000-0000-0000-000000000002",
    "asset_ids": ["00000000-0000-0000-0000-000000000101"],
    "priority": "normal",
    "scan_interval_seconds": 300,
    "transfer_filter_enabled": true,
    "balance_change_filter_enabled": true,
    "status": "active"
  },
  "rows": [
    {
      "row_number": 1,
      "raw_text": "0x0000000000000000000000000000000000000001,Hot wallet,critical",
      "address": "0x0000000000000000000000000000000000000001",
      "label": "Hot wallet",
      "priority": "critical"
    }
  ]
}
```

---

## Task 1: Add backend contract tests for new API surface

**Files:**

- Modify: `backend/crates/core/src/models.rs`
- Modify: `backend/crates/api-server/src/routes.rs`
- Modify: `backend/crates/storage/src/notifications.rs`
- Modify: `backend/crates/storage/src/repositories.rs`

- [ ] **Step 1: Add failing model tests**

Append these tests inside `#[cfg(test)] mod tests` in `backend/crates/core/src/models.rs`:

```rust
    #[test]
    fn telegram_bot_create_request_deserializes_token_without_serializing_secret() {
        let payload = r#"{
            "name":"Ops bot",
            "bot_token":"123456:secret-token",
            "status":"active"
        }"#;

        let request: CreateTelegramBotRequest = serde_json::from_str(payload).unwrap();

        assert_eq!(request.name, "Ops bot");
        assert_eq!(request.bot_token, "123456:secret-token");
        assert_eq!(request.status.as_deref(), Some("active"));
    }

    #[test]
    fn address_import_create_request_carries_defaults_and_rows() {
        let payload = r#"{
            "defaults": {
                "chain_id":"00000000-0000-0000-0000-000000000002",
                "asset_ids":["00000000-0000-0000-0000-000000000101"],
                "priority":"normal",
                "scan_interval_seconds":300,
                "transfer_filter_enabled":true,
                "balance_change_filter_enabled":true,
                "status":"active"
            },
            "rows": [{
                "row_number":1,
                "raw_text":"0x0000000000000000000000000000000000000001,Hot wallet,critical",
                "address":"0x0000000000000000000000000000000000000001",
                "label":"Hot wallet",
                "priority":"critical"
            }]
        }"#;

        let request: CreateWatchedAddressImportRequest = serde_json::from_str(payload).unwrap();

        assert_eq!(request.defaults.priority, "normal");
        assert_eq!(request.rows.len(), 1);
        assert_eq!(request.rows[0].row_number, 1);
        assert_eq!(request.rows[0].priority.as_deref(), Some("critical"));
    }
```

- [ ] **Step 2: Add failing route exposure tests**

Extend `router_exposes_notification_routes` in `backend/crates/api-server/src/routes.rs` with these cases:

```rust
            (
                Method::GET,
                "/api/telegram-bots",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::POST,
                "/api/telegram-bots",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::PUT,
                "/api/telegram-bots/not-a-uuid",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::DELETE,
                "/api/telegram-bots/not-a-uuid",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::POST,
                "/api/telegram-bots/not-a-uuid/verify",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::PUT,
                "/api/notification-channels/not-a-uuid",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::DELETE,
                "/api/notification-channels/not-a-uuid",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::POST,
                "/api/notification-channels/not-a-uuid/verify",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::POST,
                "/api/notification-channels/not-a-uuid/test",
                StatusCode::UNAUTHORIZED,
            ),
```

Add this new test near `router_exposes_watched_address_crud_routes`:

```rust
    #[tokio::test]
    async fn router_exposes_watched_address_import_routes() {
        let app = build_router(test_state());

        for (method, uri) in [
            (Method::POST, "/api/addresses/imports"),
            (Method::GET, "/api/addresses/imports/not-a-uuid"),
            (Method::GET, "/api/addresses/imports/not-a-uuid/errors"),
            (Method::POST, "/api/addresses/imports/not-a-uuid/cancel"),
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

            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");
        }
    }
```

- [ ] **Step 3: Add failing storage/migration tests**

Append these tests to `backend/crates/storage/src/notifications.rs` tests:

```rust
    #[test]
    fn channel_validation_accepts_email_and_telegram() {
        for channel_type in ["email", "telegram"] {
            let request = CreateNotificationChannelRequest {
                channel_type: channel_type.to_string(),
                name: format!("{channel_type} channel"),
                config: None,
                status: Some("active".to_string()),
            };

            assert!(validate_notification_channel_request(&request).is_ok());
        }
    }

    #[test]
    fn notification_channel_management_queries_are_tenant_scoped() {
        assert!(GET_NOTIFICATION_CHANNEL_QUERY.contains("tenant_id = $2"));
        assert!(UPDATE_NOTIFICATION_CHANNEL_QUERY.contains("tenant_id = $6"));
        assert!(DELETE_NOTIFICATION_CHANNEL_QUERY.contains("tenant_id = $2"));
    }
```

Append this test to `backend/crates/storage/src/repositories.rs` tests:

```rust
    #[test]
    fn telegram_and_address_import_migration_defines_task_tables() {
        let migration = include_str!("../migrations/0014_telegram_bots_and_address_imports.sql");

        assert!(migration.contains("CREATE TABLE IF NOT EXISTS telegram_bots"));
        assert!(migration.contains("CREATE TABLE IF NOT EXISTS watched_address_import_tasks"));
        assert!(migration.contains("CREATE TABLE IF NOT EXISTS watched_address_import_rows"));
        assert!(migration.contains("idx_watched_address_import_tasks_claim"));
        assert!(migration.contains("idx_watched_address_import_rows_task_status"));
    }
```

- [ ] **Step 4: Run backend tests and verify failure**

Run each focused test command separately so Cargo does not treat multiple test names as one filter:

```bash
cargo test --locked --manifest-path backend/Cargo.toml models::tests::telegram_bot_create_request_deserializes_token_without_serializing_secret
cargo test --locked --manifest-path backend/Cargo.toml models::tests::address_import_create_request_carries_defaults_and_rows
cargo test --locked --manifest-path backend/Cargo.toml router_exposes_watched_address_import_routes
cargo test --locked --manifest-path backend/Cargo.toml channel_validation_accepts_email_and_telegram
cargo test --locked --manifest-path backend/Cargo.toml telegram_and_address_import_migration_defines_task_tables
```

Expected: each command FAILS because the new model types, route registrations, query constants, and migration file do not exist yet.

- [ ] **Step 5: Commit failing backend contracts**

```bash
git add backend/crates/core/src/models.rs backend/crates/api-server/src/routes.rs backend/crates/storage/src/notifications.rs backend/crates/storage/src/repositories.rs
git commit -m "添加通知渠道和地址导入后端契约测试"
```

---

## Task 2: Add backend models and database schema

**Files:**

- Create: `backend/crates/storage/migrations/0014_telegram_bots_and_address_imports.sql`
- Modify: `backend/crates/core/src/models.rs`

- [ ] **Step 1: Create migration**

Create `backend/crates/storage/migrations/0014_telegram_bots_and_address_imports.sql` with:

```sql
CREATE TABLE IF NOT EXISTS telegram_bots (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    bot_token TEXT NOT NULL,
    token_preview TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    verification_status TEXT NOT NULL DEFAULT 'unverified',
    last_verified_at TIMESTAMPTZ,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT telegram_bots_status_check CHECK (status IN ('active', 'inactive')),
    CONSTRAINT telegram_bots_verification_status_check CHECK (verification_status IN ('unverified', 'verified', 'failed'))
);

CREATE INDEX IF NOT EXISTS idx_telegram_bots_tenant_status
    ON telegram_bots(tenant_id, status, created_at DESC);

CREATE TABLE IF NOT EXISTS watched_address_import_tasks (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'pending',
    chain_id UUID NOT NULL REFERENCES chains(id),
    asset_ids UUID[] NOT NULL,
    priority TEXT NOT NULL,
    scan_interval_seconds INTEGER NOT NULL,
    transfer_filter_enabled BOOLEAN NOT NULL,
    balance_change_filter_enabled BOOLEAN NOT NULL,
    address_status TEXT NOT NULL,
    total_rows INTEGER NOT NULL DEFAULT 0,
    processed_rows INTEGER NOT NULL DEFAULT 0,
    success_rows INTEGER NOT NULL DEFAULT 0,
    failed_rows INTEGER NOT NULL DEFAULT 0,
    locked_at TIMESTAMPTZ,
    locked_by TEXT,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT watched_address_import_tasks_status_check CHECK (status IN ('pending', 'running', 'completed', 'failed', 'cancelled'))
);

CREATE INDEX IF NOT EXISTS idx_watched_address_import_tasks_tenant_created
    ON watched_address_import_tasks(tenant_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_watched_address_import_tasks_claim
    ON watched_address_import_tasks(status, created_at)
    WHERE status IN ('pending', 'running');

CREATE TABLE IF NOT EXISTS watched_address_import_rows (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    import_task_id UUID NOT NULL REFERENCES watched_address_import_tasks(id) ON DELETE CASCADE,
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    row_number INTEGER NOT NULL,
    raw_text TEXT NOT NULL,
    address TEXT NOT NULL,
    label TEXT,
    priority TEXT,
    scan_interval_seconds INTEGER,
    transfer_filter_enabled BOOLEAN,
    balance_change_filter_enabled BOOLEAN,
    address_status TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    watched_address_id UUID REFERENCES watched_addresses(id) ON DELETE SET NULL,
    error_code TEXT,
    error_message TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT watched_address_import_rows_status_check CHECK (status IN ('pending', 'success', 'failed', 'skipped')),
    CONSTRAINT watched_address_import_rows_unique_row_number UNIQUE (import_task_id, row_number)
);

CREATE INDEX IF NOT EXISTS idx_watched_address_import_rows_task_status
    ON watched_address_import_rows(import_task_id, status, row_number);
```

- [ ] **Step 2: Add core models**

Add these types near `NotificationChannel` and `CreateNotificationChannelRequest` in `backend/crates/core/src/models.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct TelegramBot {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub token_preview: String,
    pub status: String,
    pub verification_status: String,
    pub last_verified_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct TelegramBotSecret {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub bot_token: String,
    pub token_preview: String,
    pub status: String,
    pub verification_status: String,
    pub last_verified_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateTelegramBotRequest {
    pub name: String,
    pub bot_token: String,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateTelegramBotRequest {
    pub name: String,
    pub bot_token: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResponse {
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateNotificationChannelRequest {
    pub channel_type: String,
    pub name: String,
    pub config: Option<serde_json::Value>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationChannelTestResponse {
    pub ok: bool,
    pub message: String,
}
```

Add these watched-address import types near `CreateWatchedAddressRequest`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchedAddressImportDefaults {
    pub chain_id: Uuid,
    pub asset_ids: Vec<Uuid>,
    pub priority: String,
    pub scan_interval_seconds: i32,
    pub transfer_filter_enabled: bool,
    pub balance_change_filter_enabled: bool,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchedAddressImportRowRequest {
    pub row_number: i32,
    pub raw_text: String,
    pub address: String,
    pub label: Option<String>,
    pub priority: Option<String>,
    pub scan_interval_seconds: Option<i32>,
    pub transfer_filter_enabled: Option<bool>,
    pub balance_change_filter_enabled: Option<bool>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWatchedAddressImportRequest {
    pub defaults: WatchedAddressImportDefaults,
    pub rows: Vec<WatchedAddressImportRowRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct WatchedAddressImportTask {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub status: String,
    pub chain_id: Uuid,
    pub asset_ids: Vec<Uuid>,
    pub priority: String,
    pub scan_interval_seconds: i32,
    pub transfer_filter_enabled: bool,
    pub balance_change_filter_enabled: bool,
    pub address_status: String,
    pub total_rows: i32,
    pub processed_rows: i32,
    pub success_rows: i32,
    pub failed_rows: i32,
    pub locked_at: Option<DateTime<Utc>>,
    pub locked_by: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct WatchedAddressImportErrorRow {
    pub row_number: i32,
    pub address: String,
    pub raw_text: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}
```

- [ ] **Step 3: Run focused backend tests**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml models::tests::telegram_bot_create_request_deserializes_token_without_serializing_secret
cargo test --locked --manifest-path backend/Cargo.toml models::tests::address_import_create_request_carries_defaults_and_rows
cargo test --locked --manifest-path backend/Cargo.toml telegram_and_address_import_migration_defines_task_tables
```

Expected: PASS for model and migration tests; route/query tests still fail until later tasks.

- [ ] **Step 4: Commit models and migration**

```bash
git add backend/crates/core/src/models.rs backend/crates/storage/migrations/0014_telegram_bots_and_address_imports.sql
git commit -m "添加TG机器人和地址导入数据模型"
```

---

## Task 3: Implement Telegram bot and notification channel storage

**Files:**

- Modify: `backend/crates/storage/src/notifications.rs`

- [ ] **Step 1: Add query constants**

Add these constants after `LIST_NOTIFICATION_CHANNELS_QUERY`:

```rust
pub const GET_NOTIFICATION_CHANNEL_QUERY: &str = r#"
SELECT id, tenant_id, channel_type, name, config, status, created_at, updated_at
FROM notification_channels
WHERE id = $1
  AND tenant_id = $2
"#;

pub const UPDATE_NOTIFICATION_CHANNEL_QUERY: &str = r#"
UPDATE notification_channels
SET channel_type = $2,
    name = $3,
    config = $4,
    status = $5,
    updated_at = NOW()
WHERE id = $1
  AND tenant_id = $6
RETURNING id, tenant_id, channel_type, name, config, status, created_at, updated_at
"#;

pub const DELETE_NOTIFICATION_CHANNEL_QUERY: &str =
    "DELETE FROM notification_channels WHERE id = $1 AND tenant_id = $2";
```

- [ ] **Step 2: Extend imports and validation**

Update model imports in `notifications.rs` to include:

```rust
CreateTelegramBotRequest, TelegramBot, TelegramBotSecret, UpdateNotificationChannelRequest,
UpdateTelegramBotRequest,
```

Add a status constant:

```rust
const CHANNEL_TYPE_EMAIL: &str = "email";
const VERIFICATION_STATUS_UNVERIFIED: &str = "unverified";
const VERIFICATION_STATUS_VERIFIED: &str = "verified";
const VERIFICATION_STATUS_FAILED: &str = "failed";
```

Change `validate_notification_channel_request` so it accepts email:

```rust
    if !matches!(
        request.channel_type.as_str(),
        CHANNEL_TYPE_IN_APP | CHANNEL_TYPE_TELEGRAM | CHANNEL_TYPE_WEBHOOK | CHANNEL_TYPE_EMAIL
    ) {
        return Err(AppError::Validation(
            "channel_type must be in_app, telegram, webhook, or email".to_string(),
        ));
    }
```

Change `validate_notification_delivery_channel_type` to accept email with the same allowed set and error message. Delivery for email can remain unsupported until a sender exists; this change prevents hidden existing email channels from failing validation paths.

Add validators:

```rust
fn validate_status(status: &str) -> AppResult<()> {
    if !matches!(status, STATUS_ACTIVE | STATUS_INACTIVE) {
        return Err(AppError::Validation(
            "status must be active or inactive".to_string(),
        ));
    }
    Ok(())
}

fn validate_verification_status(status: &str) -> AppResult<()> {
    if !matches!(
        status,
        VERIFICATION_STATUS_UNVERIFIED | VERIFICATION_STATUS_VERIFIED | VERIFICATION_STATUS_FAILED
    ) {
        return Err(AppError::Validation(
            "verification_status must be unverified, verified, or failed".to_string(),
        ));
    }
    Ok(())
}

pub fn telegram_token_preview(token: &str) -> String {
    let trimmed = token.trim();
    if trimmed.len() <= 10 {
        return "********".to_string();
    }
    format!("{}...{}", &trimmed[..6], &trimmed[trimmed.len() - 4..])
}
```

- [ ] **Step 3: Add Telegram bot storage functions**

Add functions:

```rust
pub async fn list_telegram_bots(pool: &PgPool, tenant_id: Uuid) -> AppResult<Vec<TelegramBot>> {
    sqlx::query_as::<_, TelegramBot>(
        r#"
        SELECT id, tenant_id, name, token_preview, status, verification_status,
               last_verified_at, last_error, created_at, updated_at
        FROM telegram_bots
        WHERE tenant_id = $1
        ORDER BY created_at DESC
        "#,
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn create_telegram_bot(
    pool: &PgPool,
    tenant_id: Uuid,
    request: CreateTelegramBotRequest,
) -> AppResult<TelegramBot> {
    if request.name.trim().is_empty() {
        return Err(AppError::Validation("telegram bot name is required".to_string()));
    }
    if request.bot_token.trim().is_empty() {
        return Err(AppError::Validation("telegram bot token is required".to_string()));
    }
    let status = request.status.unwrap_or_else(|| STATUS_ACTIVE.to_string());
    validate_status(&status)?;
    let token_preview = telegram_token_preview(&request.bot_token);

    sqlx::query_as::<_, TelegramBot>(
        r#"
        INSERT INTO telegram_bots (tenant_id, name, bot_token, token_preview, status, verification_status)
        VALUES ($1, $2, $3, $4, $5, 'unverified')
        RETURNING id, tenant_id, name, token_preview, status, verification_status,
                  last_verified_at, last_error, created_at, updated_at
        "#,
    )
    .bind(tenant_id)
    .bind(request.name)
    .bind(request.bot_token)
    .bind(token_preview)
    .bind(status)
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn get_telegram_bot_secret(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
) -> AppResult<TelegramBotSecret> {
    sqlx::query_as::<_, TelegramBotSecret>(
        r#"
        SELECT id, tenant_id, name, bot_token, token_preview, status, verification_status,
               last_verified_at, last_error, created_at, updated_at
        FROM telegram_bots
        WHERE id = $1
          AND tenant_id = $2
        "#,
    )
    .bind(id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("telegram bot".to_string()))
}
```

Then add these tenant-scoped mutation helpers:

```rust
pub async fn update_telegram_bot(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
    request: UpdateTelegramBotRequest,
) -> AppResult<TelegramBot> {
    if request.name.trim().is_empty() {
        return Err(AppError::Validation("telegram bot name is required".to_string()));
    }
    validate_status(&request.status)?;
    let existing = get_telegram_bot_secret(pool, tenant_id, id).await?;
    let next_token = request.bot_token.filter(|token| !token.trim().is_empty());
    let bot_token = next_token.clone().unwrap_or(existing.bot_token);
    let token_preview = next_token
        .as_deref()
        .map(telegram_token_preview)
        .unwrap_or(existing.token_preview);
    let verification_status = if next_token.is_some() {
        VERIFICATION_STATUS_UNVERIFIED
    } else {
        existing.verification_status.as_str()
    };

    sqlx::query_as::<_, TelegramBot>(
        r#"
        UPDATE telegram_bots
        SET name = $3,
            bot_token = $4,
            token_preview = $5,
            status = $6,
            verification_status = $7,
            last_error = CASE WHEN $8 THEN NULL ELSE last_error END,
            last_verified_at = CASE WHEN $8 THEN NULL ELSE last_verified_at END,
            updated_at = NOW()
        WHERE id = $1
          AND tenant_id = $2
        RETURNING id, tenant_id, name, token_preview, status, verification_status,
                  last_verified_at, last_error, created_at, updated_at
        "#,
    )
    .bind(id)
    .bind(tenant_id)
    .bind(request.name)
    .bind(bot_token)
    .bind(token_preview)
    .bind(request.status)
    .bind(verification_status)
    .bind(next_token.is_some())
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("telegram bot".to_string()))
}

pub async fn delete_telegram_bot(pool: &PgPool, tenant_id: Uuid, id: Uuid) -> AppResult<()> {
    let result = sqlx::query("DELETE FROM telegram_bots WHERE id = $1 AND tenant_id = $2")
        .bind(id)
        .bind(tenant_id)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("telegram bot".to_string()));
    }
    Ok(())
}

pub async fn mark_telegram_bot_verification(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
    status: &str,
    last_error: Option<String>,
    verified_at: DateTime<Utc>,
) -> AppResult<TelegramBot> {
    validate_verification_status(status)?;
    sqlx::query_as::<_, TelegramBot>(
        r#"
        UPDATE telegram_bots
        SET verification_status = $3,
            last_verified_at = $4,
            last_error = $5,
            updated_at = NOW()
        WHERE id = $1
          AND tenant_id = $2
        RETURNING id, tenant_id, name, token_preview, status, verification_status,
                  last_verified_at, last_error, created_at, updated_at
        "#,
    )
    .bind(id)
    .bind(tenant_id)
    .bind(status)
    .bind(verified_at)
    .bind(last_error)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("telegram bot".to_string()))
}
```

- [ ] **Step 4: Add notification channel storage functions**

Add:

```rust
pub async fn get_notification_channel(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
) -> AppResult<NotificationChannel> {
    sqlx::query_as::<_, NotificationChannel>(GET_NOTIFICATION_CHANNEL_QUERY)
        .bind(id)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("notification channel".to_string()))
}

pub async fn update_notification_channel(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
    request: UpdateNotificationChannelRequest,
) -> AppResult<NotificationChannel> {
    let create_like = CreateNotificationChannelRequest {
        channel_type: request.channel_type.clone(),
        name: request.name.clone(),
        config: request.config.clone(),
        status: Some(request.status.clone()),
    };
    validate_notification_channel_request(&create_like)?;
    let config = request.config.unwrap_or_else(|| serde_json::json!({}));

    sqlx::query_as::<_, NotificationChannel>(UPDATE_NOTIFICATION_CHANNEL_QUERY)
        .bind(id)
        .bind(request.channel_type)
        .bind(request.name)
        .bind(config)
        .bind(request.status)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("notification channel".to_string()))
}

pub async fn delete_notification_channel(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
) -> AppResult<()> {
    let result = sqlx::query(DELETE_NOTIFICATION_CHANNEL_QUERY)
        .bind(id)
        .bind(tenant_id)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("notification channel".to_string()));
    }
    Ok(())
}
```

- [ ] **Step 5: Run focused storage tests**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml channel_validation_accepts_email_and_telegram
cargo test --locked --manifest-path backend/Cargo.toml notification_channel_management_queries_are_tenant_scoped
```

Expected: PASS with the focused assertions for this task passing.

- [ ] **Step 6: Commit storage helpers**

```bash
git add backend/crates/storage/src/notifications.rs
git commit -m "添加TG机器人和通知渠道存储能力"
```

---

## Task 4: Implement address import task storage

**Files:**

- Create: `backend/crates/storage/src/address_imports.rs`
- Modify: `backend/crates/storage/src/lib.rs`

- [ ] **Step 1: Add storage module export**

In `backend/crates/storage/src/lib.rs`, add:

```rust
pub mod address_imports;
```

- [ ] **Step 2: Create address import storage module**

Create `backend/crates/storage/src/address_imports.rs` with these public constants and validators:

```rust
use std::collections::HashSet;

use chrono::{DateTime, Utc};
use coin_listener_core::{
    models::{
        CreateWatchedAddressImportRequest, CreateWatchedAddressRequest,
        WatchedAddressImportErrorRow, WatchedAddressImportRowRequest,
        WatchedAddressImportTask,
    },
    AppError, AppResult,
};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub const CLAIM_WATCHED_ADDRESS_IMPORT_QUERY: &str = r#"
WITH next_task AS (
    SELECT id
    FROM watched_address_import_tasks
    WHERE status = 'pending'
    ORDER BY created_at ASC
    LIMIT 1
    FOR UPDATE SKIP LOCKED
)
UPDATE watched_address_import_tasks task
SET status = 'running',
    locked_at = $1,
    locked_by = $2,
    started_at = COALESCE(started_at, $1),
    updated_at = NOW()
FROM next_task
WHERE task.id = next_task.id
RETURNING task.id, task.tenant_id, task.status, task.chain_id, task.asset_ids,
          task.priority, task.scan_interval_seconds, task.transfer_filter_enabled,
          task.balance_change_filter_enabled, task.address_status, task.total_rows,
          task.processed_rows, task.success_rows, task.failed_rows, task.locked_at,
          task.locked_by, task.started_at, task.completed_at, task.last_error,
          task.created_at, task.updated_at
"#;

pub fn validate_import_create_request(request: &CreateWatchedAddressImportRequest) -> AppResult<()> {
    if request.rows.is_empty() {
        return Err(AppError::Validation("import rows are required".to_string()));
    }
    if request.defaults.asset_ids.is_empty() {
        return Err(AppError::Validation("asset_ids are required".to_string()));
    }
    let mut row_numbers = HashSet::new();
    let mut addresses = HashSet::new();
    for row in &request.rows {
        if row.row_number <= 0 {
            return Err(AppError::Validation("row_number must be positive".to_string()));
        }
        if row.address.trim().is_empty() {
            return Err(AppError::Validation("address is required".to_string()));
        }
        if !row_numbers.insert(row.row_number) {
            return Err(AppError::Validation("row_number must be unique".to_string()));
        }
        let normalized = row.address.trim().to_ascii_lowercase();
        if !addresses.insert(normalized) {
            return Err(AppError::Validation("addresses must be unique within an import".to_string()));
        }
    }
    Ok(())
}
```

- [ ] **Step 3: Add create/progress/error/cancel functions**

In the same file, add:

```rust
pub async fn create_watched_address_import(
    pool: &PgPool,
    tenant_id: Uuid,
    request: CreateWatchedAddressImportRequest,
) -> AppResult<WatchedAddressImportTask> {
    validate_import_create_request(&request)?;
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let task = sqlx::query_as::<_, WatchedAddressImportTask>(
        r#"
        INSERT INTO watched_address_import_tasks (
            tenant_id, chain_id, asset_ids, priority, scan_interval_seconds,
            transfer_filter_enabled, balance_change_filter_enabled, address_status,
            total_rows
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        RETURNING id, tenant_id, status, chain_id, asset_ids, priority,
                  scan_interval_seconds, transfer_filter_enabled,
                  balance_change_filter_enabled, address_status, total_rows,
                  processed_rows, success_rows, failed_rows, locked_at, locked_by,
                  started_at, completed_at, last_error, created_at, updated_at
        "#,
    )
    .bind(tenant_id)
    .bind(request.defaults.chain_id)
    .bind(&request.defaults.asset_ids)
    .bind(request.defaults.priority)
    .bind(request.defaults.scan_interval_seconds)
    .bind(request.defaults.transfer_filter_enabled)
    .bind(request.defaults.balance_change_filter_enabled)
    .bind(request.defaults.status)
    .bind(request.rows.len() as i32)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?;

    insert_import_rows(&mut transaction, tenant_id, task.id, &request.rows).await?;
    transaction
        .commit()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    Ok(task)
}
```

Then add these helpers and public functions:

```rust
async fn insert_import_rows(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    task_id: Uuid,
    rows: &[WatchedAddressImportRowRequest],
) -> AppResult<()> {
    for row in rows {
        sqlx::query(
            r#"
            INSERT INTO watched_address_import_rows (
                import_task_id, tenant_id, row_number, raw_text, address, label, priority,
                scan_interval_seconds, transfer_filter_enabled, balance_change_filter_enabled,
                address_status
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            "#,
        )
        .bind(task_id)
        .bind(tenant_id)
        .bind(row.row_number)
        .bind(&row.raw_text)
        .bind(&row.address)
        .bind(&row.label)
        .bind(&row.priority)
        .bind(row.scan_interval_seconds)
        .bind(row.transfer_filter_enabled)
        .bind(row.balance_change_filter_enabled)
        .bind(&row.status)
        .execute(&mut **transaction)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    }
    Ok(())
}

pub async fn get_watched_address_import(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
) -> AppResult<WatchedAddressImportTask> {
    sqlx::query_as::<_, WatchedAddressImportTask>(
        r#"
        SELECT id, tenant_id, status, chain_id, asset_ids, priority, scan_interval_seconds,
               transfer_filter_enabled, balance_change_filter_enabled, address_status,
               total_rows, processed_rows, success_rows, failed_rows, locked_at, locked_by,
               started_at, completed_at, last_error, created_at, updated_at
        FROM watched_address_import_tasks
        WHERE id = $1
          AND tenant_id = $2
        "#,
    )
    .bind(id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("watched address import".to_string()))
}

pub async fn list_watched_address_import_errors(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
) -> AppResult<Vec<WatchedAddressImportErrorRow>> {
    sqlx::query_as::<_, WatchedAddressImportErrorRow>(
        r#"
        SELECT row_number, address, raw_text, error_code, error_message
        FROM watched_address_import_rows
        WHERE import_task_id = $1
          AND tenant_id = $2
          AND status = 'failed'
        ORDER BY row_number ASC
        "#,
    )
    .bind(id)
    .bind(tenant_id)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn cancel_watched_address_import(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
) -> AppResult<WatchedAddressImportTask> {
    sqlx::query(
        r#"
        UPDATE watched_address_import_rows
        SET status = 'skipped',
            error_code = 'cancelled',
            error_message = 'import task cancelled',
            updated_at = NOW()
        WHERE import_task_id = $1
          AND tenant_id = $2
          AND status = 'pending'
        "#,
    )
    .bind(id)
    .bind(tenant_id)
    .execute(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?;

    sqlx::query_as::<_, WatchedAddressImportTask>(
        r#"
        UPDATE watched_address_import_tasks
        SET status = 'cancelled',
            completed_at = COALESCE(completed_at, NOW()),
            updated_at = NOW()
        WHERE id = $1
          AND tenant_id = $2
          AND status IN ('pending', 'running')
        RETURNING id, tenant_id, status, chain_id, asset_ids, priority, scan_interval_seconds,
                  transfer_filter_enabled, balance_change_filter_enabled, address_status,
                  total_rows, processed_rows, success_rows, failed_rows, locked_at, locked_by,
                  started_at, completed_at, last_error, created_at, updated_at
        "#,
    )
    .bind(id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("watched address import".to_string()))
}

pub async fn claim_next_watched_address_import(
    pool: &PgPool,
    now: DateTime<Utc>,
    worker_id: &str,
) -> AppResult<Option<WatchedAddressImportTask>> {
    sqlx::query_as::<_, WatchedAddressImportTask>(CLAIM_WATCHED_ADDRESS_IMPORT_QUERY)
        .bind(now)
        .bind(worker_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}
```

Every read/update query above is tenant-scoped with `AND tenant_id = $2`.

- [ ] **Step 4: Add row-processing helpers**

Add these functions for the worker:

```rust
pub async fn pending_import_rows(
    pool: &PgPool,
    tenant_id: Uuid,
    task_id: Uuid,
    limit: i64,
) -> AppResult<Vec<WatchedAddressImportRowRequest>> {
    sqlx::query_as::<_, WatchedAddressImportRowRequest>(
        r#"
        SELECT row_number, raw_text, address, label, priority, scan_interval_seconds,
               transfer_filter_enabled, balance_change_filter_enabled, address_status AS status
        FROM watched_address_import_rows
        WHERE import_task_id = $1
          AND tenant_id = $2
          AND status = 'pending'
        ORDER BY row_number ASC
        LIMIT $3
        "#,
    )
    .bind(task_id)
    .bind(tenant_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn mark_import_row_success(
    pool: &PgPool,
    task_id: Uuid,
    row_number: i32,
    watched_address_id: Uuid,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE watched_address_import_rows
        SET status = 'success',
            watched_address_id = $3,
            error_code = NULL,
            error_message = NULL,
            updated_at = NOW()
        WHERE import_task_id = $1
          AND row_number = $2
        "#,
    )
    .bind(task_id)
    .bind(row_number)
    .bind(watched_address_id)
    .execute(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?;
    Ok(())
}

pub async fn mark_import_row_failed(
    pool: &PgPool,
    task_id: Uuid,
    row_number: i32,
    error_code: &str,
    error_message: &str,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE watched_address_import_rows
        SET status = 'failed',
            error_code = $3,
            error_message = $4,
            updated_at = NOW()
        WHERE import_task_id = $1
          AND row_number = $2
        "#,
    )
    .bind(task_id)
    .bind(row_number)
    .bind(error_code)
    .bind(error_message)
    .execute(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?;
    Ok(())
}

pub async fn refresh_import_task_counts(pool: &PgPool, task_id: Uuid) -> AppResult<WatchedAddressImportTask> {
    sqlx::query_as::<_, WatchedAddressImportTask>(
        r#"
        UPDATE watched_address_import_tasks task
        SET processed_rows = counts.processed_rows,
            success_rows = counts.success_rows,
            failed_rows = counts.failed_rows,
            updated_at = NOW()
        FROM (
            SELECT import_task_id,
                   COUNT(*) FILTER (WHERE status IN ('success', 'failed', 'skipped'))::int AS processed_rows,
                   COUNT(*) FILTER (WHERE status = 'success')::int AS success_rows,
                   COUNT(*) FILTER (WHERE status = 'failed')::int AS failed_rows
            FROM watched_address_import_rows
            WHERE import_task_id = $1
            GROUP BY import_task_id
        ) counts
        WHERE task.id = counts.import_task_id
        RETURNING task.id, task.tenant_id, task.status, task.chain_id, task.asset_ids,
                  task.priority, task.scan_interval_seconds, task.transfer_filter_enabled,
                  task.balance_change_filter_enabled, task.address_status, task.total_rows,
                  task.processed_rows, task.success_rows, task.failed_rows, task.locked_at,
                  task.locked_by, task.started_at, task.completed_at, task.last_error,
                  task.created_at, task.updated_at
        "#,
    )
    .bind(task_id)
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn complete_import_if_finished(
    pool: &PgPool,
    task_id: Uuid,
    now: DateTime<Utc>,
) -> AppResult<WatchedAddressImportTask> {
    let task = refresh_import_task_counts(pool, task_id).await?;
    if task.status == "cancelled" || task.processed_rows < task.total_rows {
        return Ok(task);
    }
    let next_status = if task.success_rows == 0 && task.failed_rows > 0 {
        "failed"
    } else {
        "completed"
    };

    sqlx::query_as::<_, WatchedAddressImportTask>(
        r#"
        UPDATE watched_address_import_tasks
        SET status = $2,
            completed_at = $3,
            updated_at = NOW()
        WHERE id = $1
          AND status = 'running'
        RETURNING id, tenant_id, status, chain_id, asset_ids, priority, scan_interval_seconds,
                  transfer_filter_enabled, balance_change_filter_enabled, address_status,
                  total_rows, processed_rows, success_rows, failed_rows, locked_at, locked_by,
                  started_at, completed_at, last_error, created_at, updated_at
        "#,
    )
    .bind(task_id)
    .bind(next_status)
    .bind(now)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .map_or(Ok(task), Ok)
}
```

`complete_import_if_finished` sets `completed` when every row is processed and at least one row succeeded; it sets `failed` only when every row is processed, no rows succeeded, and one or more rows failed; it returns `cancelled` tasks unchanged.

- [ ] **Step 5: Add storage tests**

Add `#[cfg(test)] mod tests` to `address_imports.rs` with:

```rust
    use super::{validate_import_create_request, CLAIM_WATCHED_ADDRESS_IMPORT_QUERY};
    use coin_listener_core::models::{
        CreateWatchedAddressImportRequest, WatchedAddressImportDefaults,
        WatchedAddressImportRowRequest,
    };
    use uuid::Uuid;

    fn request_with_rows(rows: Vec<WatchedAddressImportRowRequest>) -> CreateWatchedAddressImportRequest {
        CreateWatchedAddressImportRequest {
            defaults: WatchedAddressImportDefaults {
                chain_id: Uuid::from_u128(2),
                asset_ids: vec![Uuid::from_u128(101)],
                priority: "normal".to_string(),
                scan_interval_seconds: 300,
                transfer_filter_enabled: true,
                balance_change_filter_enabled: true,
                status: "active".to_string(),
            },
            rows,
        }
    }

    #[test]
    fn import_validation_rejects_duplicate_addresses() {
        let rows = vec![
            WatchedAddressImportRowRequest {
                row_number: 1,
                raw_text: "0x0000000000000000000000000000000000000001".to_string(),
                address: "0x0000000000000000000000000000000000000001".to_string(),
                label: None,
                priority: None,
                scan_interval_seconds: None,
                transfer_filter_enabled: None,
                balance_change_filter_enabled: None,
                status: None,
            },
            WatchedAddressImportRowRequest {
                row_number: 2,
                raw_text: "0x0000000000000000000000000000000000000001".to_string(),
                address: "0x0000000000000000000000000000000000000001".to_string(),
                label: None,
                priority: None,
                scan_interval_seconds: None,
                transfer_filter_enabled: None,
                balance_change_filter_enabled: None,
                status: None,
            },
        ];

        let error = validate_import_create_request(&request_with_rows(rows)).unwrap_err();

        assert_eq!(error.to_string(), "validation error: addresses must be unique within an import");
    }

    #[test]
    fn claim_query_uses_skip_locked() {
        assert!(CLAIM_WATCHED_ADDRESS_IMPORT_QUERY.contains("FOR UPDATE SKIP LOCKED"));
        assert!(CLAIM_WATCHED_ADDRESS_IMPORT_QUERY.contains("status = 'pending'"));
    }
```

- [ ] **Step 6: Run storage tests**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml address_imports
```

Expected: PASS with the focused assertions for this task passing.

- [ ] **Step 7: Commit import storage**

```bash
git add backend/crates/storage/src/lib.rs backend/crates/storage/src/address_imports.rs
git commit -m "添加监听地址导入任务存储"
```

---

## Task 5: Add Telegram verify/test support and API routes

**Files:**

- Modify: `backend/crates/notifier/src/external.rs`
- Modify: `backend/crates/notifier/src/lib.rs`
- Modify: `backend/crates/api-server/Cargo.toml`
- Modify: `backend/crates/api-server/src/routes.rs`

- [ ] **Step 1: Add Telegram verification methods to external sender**

In `backend/crates/notifier/src/external.rs`, add:

```rust
impl ExternalNotificationSender {
    pub async fn verify_telegram_bot(&self, bot_token: &str) -> ExternalSendOutcome {
        let url = format!(
            "{}/bot{}/getMe",
            self.telegram_api_base_url.trim_end_matches('/'),
            bot_token
        );
        let response = self
            .client
            .get(&url)
            .timeout(Duration::from_millis(5000))
            .send()
            .await;

        match response {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = read_provider_response_prefix(response).await;
                classify_telegram_verify_response(status, &body)
            }
            Err(error) => ExternalSendOutcome::TransientFailure(ExternalSendMetadata {
                last_error: Some(telegram_network_error_message(&url, &error.to_string())),
                provider_message_id: None,
                provider_status_code: None,
                provider_response: None,
            }),
        }
    }
}

pub fn classify_telegram_verify_response(status_code: u16, body: &str) -> ExternalSendOutcome {
    if (200..300).contains(&status_code) && body.contains(r#""ok":true"#) {
        return ExternalSendOutcome::Sent(ExternalSendMetadata {
            last_error: None,
            provider_message_id: None,
            provider_status_code: Some(status_code.into()),
            provider_response: Some(body.to_string()),
        });
    }
    classify_telegram_response(status_code, body)
}
```

Add a unit test:

```rust
    #[test]
    fn telegram_verify_success_uses_get_me_ok_response() {
        let outcome = classify_telegram_verify_response(200, r#"{"ok":true,"result":{"id":1}}"#);

        assert!(outcome.is_sent());
    }
```

- [ ] **Step 2: Support Telegram bot ID config during delivery**

Change `TelegramChannelConfig` in `external.rs` to:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramChannelConfig {
    pub telegram_bot_id: Option<Uuid>,
    pub bot_token_env: Option<String>,
    pub chat_id: String,
}
```

Update `TelegramChannelConfig::parse` so it accepts either `telegram_bot_id` or `bot_token_env`:

```rust
        let telegram_bot_id = value
            .get("telegram_bot_id")
            .and_then(Value::as_str)
            .and_then(|raw| Uuid::parse_str(raw).ok());
        let bot_token_env = optional_string(value, "bot_token_env");
        if telegram_bot_id.is_none() && bot_token_env.is_none() {
            return Err(ExternalConfigError::new(
                "telegram telegram_bot_id or bot_token_env is required",
            ));
        }
        let chat_id = required_string(value, "chat_id", "telegram chat_id is required")?;
        Ok(Self {
            telegram_bot_id,
            bot_token_env,
            chat_id,
        })
```

Update the existing `telegram_channel_config_requires_token_env_and_chat_id` unit test so missing `bot_token_env` only fails when `telegram_bot_id` is also missing, and add a passing assertion for `json!({"telegram_bot_id": uuid(1).to_string(), "chat_id": "123"})`.

In `backend/crates/notifier/src/lib.rs`, replace the `let bot_token = match std::env::var(&config.bot_token_env) { ... };` block inside the Telegram delivery branch with:

```rust
            let bot_token = match config.telegram_bot_id {
                Some(bot_id) => match notifications::get_telegram_bot_secret(pool, task.tenant_id, bot_id).await {
                    Ok(bot) if bot.status == "active" => bot.bot_token,
                    Ok(_) => {
                        let message = "telegram bot is inactive".to_string();
                        notifications::mark_external_notification_delivery_failed(
                            pool, task.tenant_id, delivery_id, attempt_count, &message, None, None,
                        ).await?;
                        return Ok(());
                    }
                    Err(error) => {
                        let message = error.to_string();
                        notifications::mark_external_notification_delivery_failed(
                            pool, task.tenant_id, delivery_id, attempt_count, &message, None, None,
                        ).await?;
                        return Ok(());
                    }
                },
                None => match config.bot_token_env.as_deref().and_then(|name| std::env::var(name).ok()) {
                    Some(token) => token,
                    None => {
                        let message = "telegram token env is not set".to_string();
                        notifications::mark_external_notification_delivery_failed(
                            pool, task.tenant_id, delivery_id, attempt_count, &message, None, None,
                        ).await?;
                        return Ok(());
                    }
                },
            };
```

- [ ] **Step 3: Add API dependencies**

In `backend/crates/api-server/Cargo.toml`, add:

```toml
notifier = { path = "../notifier" }
reqwest.workspace = true
```

- [ ] **Step 4: Register routes**

In `build_router`, add protected routes:

```rust
        .route("/api/telegram-bots", get(list_telegram_bots).post(create_telegram_bot))
        .route(
            "/api/telegram-bots/:id",
            put(update_telegram_bot).delete(delete_telegram_bot),
        )
        .route("/api/telegram-bots/:id/verify", post(verify_telegram_bot))
        .route(
            "/api/notification-channels/:id",
            put(update_notification_channel).delete(delete_notification_channel),
        )
        .route("/api/notification-channels/:id/verify", post(verify_notification_channel))
        .route("/api/notification-channels/:id/test", post(test_notification_channel))
```

Update route imports from `coin_listener_core::models` to include:

```rust
CreateTelegramBotRequest, NotificationChannelTestResponse, UpdateNotificationChannelRequest,
UpdateTelegramBotRequest, VerificationResponse,
```

Also add `AppResult` to the `coin_listener_core` import and keep `PgPool` imported from `sqlx` for the helper added below.

- [ ] **Step 5: Add handlers**

Add handlers near existing notification channel handlers:

```rust
async fn list_telegram_bots(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Response, ApiError> {
    let bots = notifications::list_telegram_bots(&state.postgres, auth.tenant_id).await?;
    Ok(Json(bots).into_response())
}

async fn create_telegram_bot(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<CreateTelegramBotRequest>,
) -> Result<Response, ApiError> {
    let bot = notifications::create_telegram_bot(&state.postgres, auth.tenant_id, request).await?;
    Ok((StatusCode::CREATED, Json(bot)).into_response())
}
```

Add the remaining handlers with this code:

```rust
async fn update_telegram_bot(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateTelegramBotRequest>,
) -> Result<Response, ApiError> {
    let bot = notifications::update_telegram_bot(&state.postgres, auth.tenant_id, id, request).await?;
    Ok(Json(bot).into_response())
}

async fn delete_telegram_bot(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    notifications::delete_telegram_bot(&state.postgres, auth.tenant_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn verify_telegram_bot(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let bot = notifications::get_telegram_bot_secret(&state.postgres, auth.tenant_id, id).await?;
    let sender = notifier::external::ExternalNotificationSender::new(reqwest::Client::new());
    let outcome = sender.verify_telegram_bot(&bot.bot_token).await;
    let ok = outcome.is_sent();
    let message = outcome
        .metadata()
        .last_error
        .clone()
        .unwrap_or_else(|| if ok { "TG机器人验证成功".to_string() } else { "TG机器人验证失败".to_string() });
    notifications::mark_telegram_bot_verification(
        &state.postgres,
        auth.tenant_id,
        id,
        if ok { "verified" } else { "failed" },
        if ok { None } else { Some(message.clone()) },
        Utc::now(),
    )
    .await?;
    Ok(Json(VerificationResponse { ok, message }).into_response())
}

async fn update_notification_channel(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateNotificationChannelRequest>,
) -> Result<Response, ApiError> {
    let channel = notifications::update_notification_channel(&state.postgres, auth.tenant_id, id, request).await?;
    Ok(Json(channel).into_response())
}

async fn delete_notification_channel(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    notifications::delete_notification_channel(&state.postgres, auth.tenant_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
```

Add a shared helper and the channel verify/test handlers:

```rust
async fn telegram_channel_bot_token(
    pool: &PgPool,
    tenant_id: Uuid,
    channel_id: Uuid,
    verify_mode: bool,
) -> AppResult<(notifier::external::TelegramChannelConfig, String)> {
    let channel = notifications::get_notification_channel(pool, tenant_id, channel_id).await?;
    if channel.channel_type != "telegram" {
        let message = if verify_mode {
            "only telegram channels can be verified"
        } else {
            "only telegram channels can be tested"
        };
        return Err(AppError::Validation(message.to_string()));
    }
    let config = notifier::external::TelegramChannelConfig::parse(&channel.config)
        .map_err(|error| AppError::Validation(error.to_string()))?;
    let bot_id = config
        .telegram_bot_id
        .ok_or_else(|| AppError::Validation("telegram_bot_id is required".to_string()))?;
    let bot = notifications::get_telegram_bot_secret(pool, tenant_id, bot_id).await?;
    Ok((config, bot.bot_token))
}

async fn verify_notification_channel(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let (config, bot_token) = telegram_channel_bot_token(&state.postgres, auth.tenant_id, id, true).await?;
    let sender = notifier::external::ExternalNotificationSender::new(reqwest::Client::new());
    let outcome = sender
        .send_telegram(&config, &bot_token, "Coin Listener Telegram channel verification")
        .await;
    let ok = outcome.is_sent();
    let message = outcome
        .metadata()
        .last_error
        .clone()
        .unwrap_or_else(|| if ok { "通知渠道验证成功".to_string() } else { "通知渠道验证失败".to_string() });
    Ok(Json(VerificationResponse { ok, message }).into_response())
}

async fn test_notification_channel(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let (config, bot_token) = telegram_channel_bot_token(&state.postgres, auth.tenant_id, id, false).await?;
    let sender = notifier::external::ExternalNotificationSender::new(reqwest::Client::new());
    let outcome = sender
        .send_telegram(&config, &bot_token, "Coin Listener test notification")
        .await;
    let ok = outcome.is_sent();
    let message = outcome
        .metadata()
        .last_error
        .clone()
        .unwrap_or_else(|| if ok { "测试通知发送成功".to_string() } else { "测试通知发送失败".to_string() });
    Ok(Json(NotificationChannelTestResponse { ok, message }).into_response())
}
```

- [ ] **Step 6: Run route and notifier tests**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml router_exposes_notification_routes
cargo test --locked --manifest-path backend/Cargo.toml telegram_verify_success_uses_get_me_ok_response
```

Expected: PASS with the focused assertions for this task passing.

- [ ] **Step 7: Commit API routes and Telegram sender support**

```bash
git add backend/crates/notifier/src/external.rs backend/crates/notifier/src/lib.rs backend/crates/api-server/Cargo.toml backend/crates/api-server/src/routes.rs
git commit -m "添加TG验证和通知渠道管理接口"
```

---

## Task 6: Add address import API routes and worker processing

**Files:**

- Modify: `backend/crates/api-server/src/routes.rs`
- Modify: `backend/crates/worker/src/lib.rs`
- Modify: `backend/crates/worker/src/main.rs`
- Modify: `backend/crates/all-in-one/src/main.rs`

- [ ] **Step 1: Add import API routes**

In `build_router`, add:

```rust
        .route("/api/addresses/imports", post(create_address_import))
        .route("/api/addresses/imports/:id", get(get_address_import))
        .route("/api/addresses/imports/:id/errors", get(list_address_import_errors))
        .route("/api/addresses/imports/:id/cancel", post(cancel_address_import))
```

Import `CreateWatchedAddressImportRequest`.

Add handlers:

```rust
async fn create_address_import(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<CreateWatchedAddressImportRequest>,
) -> Result<Response, ApiError> {
    let task = coin_listener_storage::address_imports::create_watched_address_import(
        &state.postgres,
        auth.tenant_id,
        request,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(task)).into_response())
}

async fn get_address_import(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let task = coin_listener_storage::address_imports::get_watched_address_import(
        &state.postgres,
        auth.tenant_id,
        id,
    )
    .await?;
    Ok(Json(task).into_response())
}
```

Add error-list and cancel handlers:

```rust
async fn list_address_import_errors(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let rows = coin_listener_storage::address_imports::list_watched_address_import_errors(
        &state.postgres,
        auth.tenant_id,
        id,
    )
    .await?;
    Ok(Json(rows).into_response())
}

async fn cancel_address_import(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let task = coin_listener_storage::address_imports::cancel_watched_address_import(
        &state.postgres,
        auth.tenant_id,
        id,
    )
    .await?;
    Ok(Json(task).into_response())
}
```

- [ ] **Step 2: Add import worker function**

In `backend/crates/worker/src/lib.rs`, import `address_imports` and add:

```rust
pub const ADDRESS_IMPORT_ROW_BATCH_SIZE: i64 = 50;

pub async fn process_one_address_import_task(
    pool: &PgPool,
    worker_id: &str,
    now: DateTime<Utc>,
) -> AppResult<bool> {
    let Some(task) = coin_listener_storage::address_imports::claim_next_watched_address_import(
        pool,
        now,
        worker_id,
    )
    .await? else {
        return Ok(false);
    };

    let rows = coin_listener_storage::address_imports::pending_import_rows(
        pool,
        task.tenant_id,
        task.id,
        ADDRESS_IMPORT_ROW_BATCH_SIZE,
    )
    .await?;

    for row in rows {
        let request = CreateWatchedAddressRequest {
            tenant_id: Some(task.tenant_id),
            chain_id: task.chain_id,
            address: row.address.clone(),
            label: row.label.clone(),
            priority: row.priority.clone().unwrap_or_else(|| task.priority.clone()),
            scan_interval_seconds: row.scan_interval_seconds.unwrap_or(task.scan_interval_seconds),
            transfer_filter_enabled: row
                .transfer_filter_enabled
                .unwrap_or(task.transfer_filter_enabled),
            balance_change_filter_enabled: row
                .balance_change_filter_enabled
                .unwrap_or(task.balance_change_filter_enabled),
            status: row.status.clone().unwrap_or_else(|| task.address_status.clone()),
            asset_ids: task.asset_ids.clone(),
        };

        match repositories::create_watched_address(pool, request).await {
            Ok(address) => {
                coin_listener_storage::address_imports::mark_import_row_success(
                    pool,
                    task.id,
                    row.row_number,
                    address.id,
                )
                .await?;
            }
            Err(error) => {
                coin_listener_storage::address_imports::mark_import_row_failed(
                    pool,
                    task.id,
                    row.row_number,
                    "create_failed",
                    &error.to_string(),
                )
                .await?;
            }
        }
    }

    coin_listener_storage::address_imports::refresh_import_task_counts(pool, task.id).await?;
    coin_listener_storage::address_imports::complete_import_if_finished(pool, task.id, now).await?;
    Ok(true)
}
```

- [ ] **Step 3: Run import processing in worker loop**

In `run_worker`, before `scan_queue.dequeue`, add:

```rust
        if let Err(error) = process_one_address_import_task(&pool, "worker", Utc::now()).await {
            warn!(error = %error, "address import task processing failed");
        }
```

- [ ] **Step 4: Add worker tests**

Add tests near worker module tests:

```rust
    #[test]
    fn address_import_batch_size_is_bounded() {
        assert_eq!(crate::ADDRESS_IMPORT_ROW_BATCH_SIZE, 50);
    }

    #[test]
    fn worker_source_processes_address_import_before_scan_dequeue() {
        let source = include_str!("lib.rs");
        let import_index = source.find("process_one_address_import_task").unwrap();
        let dequeue_index = source.find("scan_queue.dequeue").unwrap();

        assert!(import_index < dequeue_index);
    }
```

- [ ] **Step 5: Run API and worker tests**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml router_exposes_watched_address_import_routes
cargo test --locked --manifest-path backend/Cargo.toml address_import_batch_size_is_bounded
cargo test --locked --manifest-path backend/Cargo.toml worker_source_processes_address_import_before_scan_dequeue
```

Expected: PASS with the focused assertions for this task passing.

- [ ] **Step 6: Commit import API and worker**

```bash
git add backend/crates/api-server/src/routes.rs backend/crates/worker/src/lib.rs backend/crates/worker/src/main.rs backend/crates/all-in-one/src/main.rs
git commit -m "添加监听地址导入任务接口和处理器"
```

---

## Task 7: Add frontend API types, clients, and parser tests

**Files:**

- Create: `frontend/src/addressImport.ts`
- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/api/client.ts`
- Modify: `frontend/src/ui-regression.test.ts`

- [ ] **Step 1: Add failing UI/API regression tests**

Append tests to `frontend/src/ui-regression.test.ts`:

```ts
  test('notification and telegram API contracts are exposed to frontend', () => {
    const types = readSource('api/types.ts');
    const client = readSource('api/client.ts');

    for (const expected of [
      'export type TelegramBot',
      'export type CreateTelegramBotRequest',
      'export type UpdateTelegramBotRequest',
      'export type NotificationChannelTestResponse',
      'export type WatchedAddressImportTask',
      'export type CreateWatchedAddressImportRequest',
      'export type WatchedAddressImportErrorRow',
    ]) {
      expectContains(types, expected);
    }

    for (const expected of [
      'listTelegramBots',
      'createTelegramBot',
      'updateTelegramBot',
      'deleteTelegramBot',
      'verifyTelegramBot',
      'updateNotificationChannel',
      'deleteNotificationChannel',
      'verifyNotificationChannel',
      'testNotificationChannel',
      'createWatchedAddressImport',
      'getWatchedAddressImport',
      'listWatchedAddressImportErrors',
      'cancelWatchedAddressImport',
    ]) {
      expectContains(client, expected);
    }
  });

  test('address import parser supports line and CSV input', async () => {
    const { parseAddressImportInput } = await import('./addressImport.ts');

    const lineResult = parseAddressImportInput('0x0000000000000000000000000000000000000001\n\n0x0000000000000000000000000000000000000002');
    if (lineResult.rows.length !== 2) throw new Error('line input should produce two rows');
    if (lineResult.rows[0].address !== '0x0000000000000000000000000000000000000001') throw new Error('line address mismatch');

    const csvResult = parseAddressImportInput('address,label,priority\n0x0000000000000000000000000000000000000003,Hot,critical');
    if (csvResult.rows[0].label !== 'Hot') throw new Error('CSV label mismatch');
    if (csvResult.rows[0].priority !== 'critical') throw new Error('CSV priority mismatch');
  });

  test('address import parser reports duplicates and unknown CSV fields', async () => {
    const { parseAddressImportInput } = await import('./addressImport.ts');

    const result = parseAddressImportInput('address,unknown\n0x0000000000000000000000000000000000000004,x\n0x0000000000000000000000000000000000000004,y');

    if (!result.warnings.some(warning => warning.includes('unknown'))) throw new Error('unknown CSV field warning missing');
    if (!result.rows.some(row => row.error === '重复地址')) throw new Error('duplicate row error missing');
  });
```

- [ ] **Step 2: Run frontend regression tests and verify failure**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: FAIL because `addressImport.ts` and new API types/functions do not exist.

- [ ] **Step 3: Add frontend types**

Append to `frontend/src/api/types.ts` near notification types:

```ts
export type TelegramBot = {
  id: string;
  tenant_id: string;
  name: string;
  token_preview: string;
  status: string;
  verification_status: string;
  last_verified_at?: string | null;
  last_error?: string | null;
  created_at: string;
  updated_at: string;
};

export type CreateTelegramBotRequest = {
  name: string;
  bot_token: string;
  status?: string;
};

export type UpdateTelegramBotRequest = {
  name: string;
  bot_token?: string | null;
  status: string;
};

export type UpdateNotificationChannelRequest = {
  channel_type: string;
  name: string;
  config?: Record<string, unknown>;
  status: string;
};

export type VerificationResponse = {
  ok: boolean;
  message: string;
};

export type NotificationChannelTestResponse = {
  ok: boolean;
  message: string;
};

export type WatchedAddressImportDefaults = {
  chain_id: string;
  asset_ids: string[];
  priority: string;
  scan_interval_seconds: number;
  transfer_filter_enabled: boolean;
  balance_change_filter_enabled: boolean;
  status: string;
};

export type WatchedAddressImportRowRequest = {
  row_number: number;
  raw_text: string;
  address: string;
  label?: string | null;
  priority?: string | null;
  scan_interval_seconds?: number | null;
  transfer_filter_enabled?: boolean | null;
  balance_change_filter_enabled?: boolean | null;
  status?: string | null;
};

export type CreateWatchedAddressImportRequest = {
  defaults: WatchedAddressImportDefaults;
  rows: WatchedAddressImportRowRequest[];
};

export type WatchedAddressImportTask = {
  id: string;
  tenant_id: string;
  status: string;
  chain_id: string;
  asset_ids: string[];
  priority: string;
  scan_interval_seconds: number;
  transfer_filter_enabled: boolean;
  balance_change_filter_enabled: boolean;
  address_status: string;
  total_rows: number;
  processed_rows: number;
  success_rows: number;
  failed_rows: number;
  locked_at?: string | null;
  locked_by?: string | null;
  started_at?: string | null;
  completed_at?: string | null;
  last_error?: string | null;
  created_at: string;
  updated_at: string;
};

export type WatchedAddressImportErrorRow = {
  row_number: number;
  address: string;
  raw_text: string;
  error_code?: string | null;
  error_message?: string | null;
};
```

- [ ] **Step 4: Add frontend clients**

Update imports in `frontend/src/api/client.ts`, then add:

```ts
export function listTelegramBots(): Promise<TelegramBot[]> {
  return request<TelegramBot[]>('/api/telegram-bots');
}

export function createTelegramBot(payload: CreateTelegramBotRequest): Promise<TelegramBot> {
  return request<TelegramBot>('/api/telegram-bots', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export function updateTelegramBot(id: string, payload: UpdateTelegramBotRequest): Promise<TelegramBot> {
  return request<TelegramBot>(`/api/telegram-bots/${id}`, {
    method: 'PUT',
    body: JSON.stringify(payload),
  });
}

export function deleteTelegramBot(id: string): Promise<void> {
  return request<void>(`/api/telegram-bots/${id}`, { method: 'DELETE' });
}

export function verifyTelegramBot(id: string): Promise<VerificationResponse> {
  return request<VerificationResponse>(`/api/telegram-bots/${id}/verify`, { method: 'POST' });
}

export function updateNotificationChannel(id: string, payload: UpdateNotificationChannelRequest): Promise<NotificationChannel> {
  return request<NotificationChannel>(`/api/notification-channels/${id}`, {
    method: 'PUT',
    body: JSON.stringify(payload),
  });
}

export function deleteNotificationChannel(id: string): Promise<void> {
  return request<void>(`/api/notification-channels/${id}`, { method: 'DELETE' });
}

export function verifyNotificationChannel(id: string): Promise<VerificationResponse> {
  return request<VerificationResponse>(`/api/notification-channels/${id}/verify`, { method: 'POST' });
}

export function testNotificationChannel(id: string): Promise<NotificationChannelTestResponse> {
  return request<NotificationChannelTestResponse>(`/api/notification-channels/${id}/test`, { method: 'POST' });
}

export function createWatchedAddressImport(payload: CreateWatchedAddressImportRequest): Promise<WatchedAddressImportTask> {
  return request<WatchedAddressImportTask>('/api/addresses/imports', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export function getWatchedAddressImport(id: string): Promise<WatchedAddressImportTask> {
  return request<WatchedAddressImportTask>(`/api/addresses/imports/${id}`);
}

export function listWatchedAddressImportErrors(id: string): Promise<WatchedAddressImportErrorRow[]> {
  return request<WatchedAddressImportErrorRow[]>(`/api/addresses/imports/${id}/errors`);
}

export function cancelWatchedAddressImport(id: string): Promise<WatchedAddressImportTask> {
  return request<WatchedAddressImportTask>(`/api/addresses/imports/${id}/cancel`, { method: 'POST' });
}
```

- [ ] **Step 5: Add parser**

Create `frontend/src/addressImport.ts`:

```ts
import type { WatchedAddressImportRowRequest } from './api/types';

export type ParsedAddressImportRow = WatchedAddressImportRowRequest & {
  error?: string;
};

export type ParsedAddressImport = {
  rows: ParsedAddressImportRow[];
  warnings: string[];
};

const supportedCsvFields = new Set([
  'address',
  'label',
  'priority',
  'scan_interval_seconds',
  'transfer_filter_enabled',
  'balance_change_filter_enabled',
  'status',
]);

export function parseAddressImportInput(input: string): ParsedAddressImport {
  const lines = input.split(/\r?\n/).map(line => line.trim()).filter(Boolean);
  if (lines.length === 0) return { rows: [], warnings: [] };

  const first = lines[0];
  if (first.toLowerCase().split(',').includes('address')) {
    return parseCsv(lines);
  }
  return markDuplicateRows(lines.map((line, index) => ({
    row_number: index + 1,
    raw_text: line,
    address: line,
  })));
}

function parseCsv(lines: string[]): ParsedAddressImport {
  const headers = splitCsvLine(lines[0]).map(header => header.trim());
  const warnings = headers
    .filter(header => header && !supportedCsvFields.has(header))
    .map(header => `unknown CSV field: ${header}`);

  const rows = lines.slice(1).map((line, index) => {
    const values = splitCsvLine(line);
    const record = Object.fromEntries(headers.map((header, valueIndex) => [header, values[valueIndex]?.trim() ?? '']));
    return {
      row_number: index + 2,
      raw_text: line,
      address: record.address ?? '',
      label: record.label || null,
      priority: record.priority || null,
      scan_interval_seconds: record.scan_interval_seconds ? Number(record.scan_interval_seconds) : null,
      transfer_filter_enabled: parseOptionalBoolean(record.transfer_filter_enabled),
      balance_change_filter_enabled: parseOptionalBoolean(record.balance_change_filter_enabled),
      status: record.status || null,
    } satisfies ParsedAddressImportRow;
  });

  const parsed = markDuplicateRows(rows);
  return { rows: parsed.rows, warnings: [...warnings, ...parsed.warnings] };
}

function markDuplicateRows(rows: ParsedAddressImportRow[]): ParsedAddressImport {
  const seen = new Set<string>();
  const warnings: string[] = [];
  return {
    rows: rows.map(row => {
      const key = row.address.trim().toLowerCase();
      if (!key) return { ...row, error: '地址不能为空' };
      if (seen.has(key)) return { ...row, error: '重复地址' };
      seen.add(key);
      return row;
    }),
    warnings,
  };
}

function splitCsvLine(line: string) {
  return line.split(',');
}

function parseOptionalBoolean(value: string | undefined) {
  if (!value) return null;
  if (value === 'true') return true;
  if (value === 'false') return false;
  return null;
}
```

- [ ] **Step 6: Run frontend regression tests**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: PASS with parser and API contract regressions passing.

- [ ] **Step 7: Commit frontend contracts**

```bash
git add frontend/src/api/types.ts frontend/src/api/client.ts frontend/src/addressImport.ts frontend/src/ui-regression.test.ts
git commit -m "添加TG通知和地址导入前端契约"
```

---

## Task 8: Add FormModal and migrate existing dialogs

**Files:**

- Create: `frontend/src/components/FormModal.tsx`
- Modify: `frontend/src/pages/AddressesPage.tsx`
- Modify: `frontend/src/pages/NotificationRulesPage.tsx`
- Modify: `frontend/src/styles.css`
- Modify: `frontend/src/ui-regression.test.ts`

- [ ] **Step 1: Add failing regression test**

Append to UI regression tests:

```ts
  test('form modal sizes are centralized and used by dense forms', () => {
    const modal = readSource('components/FormModal.tsx');
    const addresses = readSource('pages/AddressesPage.tsx');
    const rules = readSource('pages/NotificationRulesPage.tsx');

    expectContains(modal, 'medium: 720');
    expectContains(modal, 'large: 920');
    expectContains(modal, 'wide: 1120');
    expectContains(modal, 'calc(100vw - 32px)');
    expectContains(addresses, '<FormModal');
    expectContains(addresses, 'size="large"');
    expectContains(rules, '<FormModal');
    expectContains(rules, 'size="large"');
  });
```

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: FAIL because `FormModal.tsx` does not exist.

- [ ] **Step 2: Create FormModal**

Create `frontend/src/components/FormModal.tsx`:

```tsx
import type { ReactNode } from 'react';
import { Modal } from '@douyinfe/semi-ui';
import type { ModalProps } from '@douyinfe/semi-ui/lib/es/modal';

const modalWidths = {
  medium: 720,
  large: 920,
  wide: 1120,
};

type FormModalSize = keyof typeof modalWidths;

type FormModalProps = Omit<ModalProps, 'width' | 'footer'> & {
  size?: FormModalSize;
  children: ReactNode;
};

export function FormModal({ size = 'medium', children, className, ...props }: FormModalProps) {
  return (
    <Modal
      {...props}
      width={modalWidths[size]}
      footer={null}
      className={['form-modal', className].filter(Boolean).join(' ')}
      style={{ maxWidth: 'calc(100vw - 32px)', ...props.style }}
      bodyStyle={{ maxHeight: 'calc(100vh - 220px)', overflowY: 'auto', ...props.bodyStyle }}
    >
      {children}
    </Modal>
  );
}
```

- [ ] **Step 3: Add styles**

Append to `frontend/src/styles.css`:

```css
.form-modal .semi-modal-body {
  padding-bottom: 20px;
}

.form-modal-actions {
  margin-top: 16px;
}
```

- [ ] **Step 4: Migrate watched-address modal**

In `AddressesPage.tsx`:

- Replace `Modal` import with `FormModal` import while keeping other Semi imports.
- Replace the create/edit `<Modal ...>` opening and closing tags with:

```tsx
      <FormModal title={editingAddress ? '编辑监听地址' : '新增监听地址'} visible={visible} onCancel={closeModal} size="large">
```

and:

```tsx
      </FormModal>
```

Change the final form action `Space` to:

```tsx
          <Space className="form-modal-actions">
```

- [ ] **Step 5: Migrate notification-rule modal**

In `NotificationRulesPage.tsx`:

- Replace raw `Modal` import with `FormModal`.
- Replace the rule modal opening with:

```tsx
      <FormModal
        title={editingRule ? '编辑通知规则' : '创建通知规则'}
        visible={modalVisible}
        onCancel={() => {
          setModalVisible(false);
          setEditingRule(null);
        }}
        size="large"
      >
```

Change the final form action `Space` to:

```tsx
          <Space className="form-modal-actions">
```

- [ ] **Step 6: Run frontend checks**

Run:

```bash
npm --prefix frontend run test:ui-regression
npm --prefix frontend run build
```

Expected: PASS with FormModal regression and frontend build passing.

- [ ] **Step 7: Commit FormModal migration**

```bash
git add frontend/src/components/FormModal.tsx frontend/src/pages/AddressesPage.tsx frontend/src/pages/NotificationRulesPage.tsx frontend/src/styles.css frontend/src/ui-regression.test.ts
git commit -m "统一表单弹窗尺寸"
```

---

## Task 9: Add Telegram bot management page

**Files:**

- Create: `frontend/src/pages/TelegramBotsPage.tsx`
- Modify: `frontend/src/App.tsx`
- Modify: `frontend/src/ui-regression.test.ts`

- [ ] **Step 1: Add failing UI regression test**

Append:

```ts
  test('telegram bot management page is wired into navigation', () => {
    const app = readSource('App.tsx');
    const page = readSource('pages/TelegramBotsPage.tsx');

    expectContains(app, "'telegram-bots'");
    expectContains(app, 'TelegramBotsPage');
    expectContains(app, 'TG机器人');
    expectContains(page, 'listTelegramBots');
    expectContains(page, 'createTelegramBot');
    expectContains(page, 'updateTelegramBot');
    expectContains(page, 'deleteTelegramBot');
    expectContains(page, 'verifyTelegramBot');
    expectContains(page, 'token_preview');
    expectContains(page, 'DataTable');
    expectContains(page, 'tableId="telegram-bots"');
  });
```

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: FAIL because the page is not created.

- [ ] **Step 2: Create TelegramBotsPage**

Create `frontend/src/pages/TelegramBotsPage.tsx`:

```tsx
import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Banner, Button, Form, Popconfirm, Space, Tag, Toast } from '@douyinfe/semi-ui';
import {
  createTelegramBot,
  deleteTelegramBot,
  listTelegramBots,
  updateTelegramBot,
  verifyTelegramBot,
} from '../api/client';
import type { TelegramBot, UpdateTelegramBotRequest } from '../api/types';
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { FormModal } from '../components/FormModal';
import { PageScaffold } from '../components/PageScaffold';

type BotForm = {
  name?: string;
  bot_token?: string;
  status?: string;
};

export function TelegramBotsPage() {
  const [visible, setVisible] = useState(false);
  const [editingBot, setEditingBot] = useState<TelegramBot | null>(null);
  const queryClient = useQueryClient();
  const botsQuery = useQuery({ queryKey: ['telegram-bots'], queryFn: listTelegramBots });

  const saveMutation = useMutation({
    mutationFn: (values: BotForm) => {
      if (editingBot) {
        return updateTelegramBot(editingBot.id, {
          name: values.name ?? '',
          bot_token: values.bot_token || null,
          status: values.status ?? 'active',
        } satisfies UpdateTelegramBotRequest);
      }
      return createTelegramBot({
        name: values.name ?? '',
        bot_token: values.bot_token ?? '',
        status: values.status ?? 'active',
      });
    },
    onSuccess: () => {
      Toast.success(editingBot ? 'TG机器人已更新' : 'TG机器人已创建');
      closeModal();
      queryClient.invalidateQueries({ queryKey: ['telegram-bots'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : 'TG机器人保存失败'),
  });

  const verifyMutation = useMutation({
    mutationFn: verifyTelegramBot,
    onSuccess: response => {
      Toast[response.ok ? 'success' : 'error'](response.message);
      queryClient.invalidateQueries({ queryKey: ['telegram-bots'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : 'TG机器人验证失败'),
  });

  const deleteMutation = useMutation({
    mutationFn: deleteTelegramBot,
    onSuccess: () => {
      Toast.success('TG机器人已删除');
      queryClient.invalidateQueries({ queryKey: ['telegram-bots'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : 'TG机器人删除失败'),
  });

  function openCreateModal() {
    setEditingBot(null);
    setVisible(true);
  }

  function openEditModal(bot: TelegramBot) {
    setEditingBot(bot);
    setVisible(true);
  }

  function closeModal() {
    setVisible(false);
    setEditingBot(null);
  }

  return (
    <PageScaffold title="TG机器人" actions={<Button type="primary" onClick={openCreateModal}>新增机器人</Button>}>
      {botsQuery.isError ? <Banner type="danger" title="TG机器人加载失败" description={botsQuery.error instanceof Error ? botsQuery.error.message : '请求失败'} /> : null}
      <DataSurface title="TG机器人列表">
        <DataTable<TelegramBot>
          tableId="telegram-bots"
          actionColumnKeys={['operations']}
          loading={botsQuery.isLoading}
          dataSource={botsQuery.data ?? []}
          rowKey="id"
          scroll={{ x: 1100 }}
          columns={[
            { title: '名称', dataIndex: 'name', width: 180, ellipsis: { showTitle: true } },
            { title: 'Token', dataIndex: 'token_preview', width: 180, className: 'table-cell-mono' },
            { title: '状态', dataIndex: 'status', width: 100, render: value => <Tag color={value === 'active' ? 'green' : 'grey'}>{String(value)}</Tag> },
            { title: '验证', dataIndex: 'verification_status', width: 120, render: value => <Tag color={value === 'verified' ? 'green' : value === 'failed' ? 'red' : 'orange'}>{String(value)}</Tag> },
            { title: '最后验证', dataIndex: 'last_verified_at', width: 190, render: value => value ? String(value) : '-' },
            { title: '错误', dataIndex: 'last_error', width: 260, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
            {
              title: '操作', key: 'operations', width: 220,
              render: (_, bot) => (
                <Space>
                  <Button size="small" onClick={() => openEditModal(bot)}>编辑</Button>
                  <Button size="small" loading={verifyMutation.isPending} onClick={() => verifyMutation.mutate(bot.id)}>验证</Button>
                  <Popconfirm title="确认删除该TG机器人？" onConfirm={() => deleteMutation.mutate(bot.id)}>
                    <Button size="small" type="danger">删除</Button>
                  </Popconfirm>
                </Space>
              ),
            },
          ]}
        />
      </DataSurface>

      <FormModal title={editingBot ? '编辑TG机器人' : '新增TG机器人'} visible={visible} onCancel={closeModal} size="large">
        <Form<BotForm> initValues={editingBot ? { name: editingBot.name, status: editingBot.status } : { status: 'active' }} onSubmit={values => saveMutation.mutate(values)} labelPosition="left" labelWidth={110}>
          <Form.Input field="name" label="名称" rules={[{ required: true, message: '请输入机器人名称' }]} />
          <Form.Input field="bot_token" label="Bot Token" mode="password" rules={editingBot ? [] : [{ required: true, message: '请输入 Bot Token' }]} placeholder={editingBot ? '留空表示不更换 Token' : '请输入 Telegram Bot Token'} />
          <Form.Select field="status" label="状态">
            <Form.Select.Option value="active">active</Form.Select.Option>
            <Form.Select.Option value="inactive">inactive</Form.Select.Option>
          </Form.Select>
          <Space className="form-modal-actions">
            <Button htmlType="submit" type="primary" loading={saveMutation.isPending}>保存</Button>
            <Button htmlType="button" onClick={closeModal}>取消</Button>
          </Space>
        </Form>
      </FormModal>
    </PageScaffold>
  );
}
```

- [ ] **Step 3: Wire navigation**

In `frontend/src/App.tsx`:

Add import:

```ts
import { TelegramBotsPage } from './pages/TelegramBotsPage';
```

Extend `PageKey`:

```ts
  | 'telegram-bots'
```

Add nav item near notification items:

```tsx
    { itemKey: 'telegram-bots', text: 'TG机器人', icon: <IconBell /> },
```

Add render branch:

```tsx
  if (page === 'telegram-bots') return <TelegramBotsPage />;
```

- [ ] **Step 4: Run frontend checks**

Run:

```bash
npm --prefix frontend run test:ui-regression
npm --prefix frontend run build
```

Expected: PASS with Telegram bot page regression and frontend build passing.

- [ ] **Step 5: Commit Telegram bot page**

```bash
git add frontend/src/pages/TelegramBotsPage.tsx frontend/src/App.tsx frontend/src/ui-regression.test.ts
git commit -m "添加TG机器人管理页面"
```

---

## Task 10: Add notification channel page and rule quick-create

**Files:**

- Create: `frontend/src/pages/NotificationChannelsPage.tsx`
- Modify: `frontend/src/App.tsx`
- Modify: `frontend/src/pages/NotificationRulesPage.tsx`
- Modify: `frontend/src/styles.css`
- Modify: `frontend/src/ui-regression.test.ts`

- [ ] **Step 1: Add failing regression test**

Append:

```ts
  test('notification channel management page and rule quick actions exist', () => {
    const app = readSource('App.tsx');
    const page = readSource('pages/NotificationChannelsPage.tsx');
    const rules = readSource('pages/NotificationRulesPage.tsx');

    expectContains(app, "'notification-channels'");
    expectContains(app, 'NotificationChannelsPage');
    expectContains(app, '通知渠道');
    expectContains(page, 'listNotificationChannels');
    expectContains(page, 'listTelegramBots');
    expectContains(page, 'updateNotificationChannel');
    expectContains(page, 'deleteNotificationChannel');
    expectContains(page, 'verifyNotificationChannel');
    expectContains(page, 'testNotificationChannel');
    expectContains(page, 'tableId="notification-channels"');
    expectContains(rules, '新建渠道');
    expectContains(rules, '刷新渠道');
    expectContains(rules, 'quickCreatedChannelId');
    expectContains(rules, 'telegramBotsQuery');
  });
```

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: FAIL because the regression test references page/actions that do not exist yet.

- [ ] **Step 2: Create NotificationChannelsPage**

Create `frontend/src/pages/NotificationChannelsPage.tsx`:

```tsx
import { useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Banner, Button, Form, Popconfirm, Space, Tag, Toast } from '@douyinfe/semi-ui';
import {
  createNotificationChannel,
  deleteNotificationChannel,
  listNotificationChannels,
  listTelegramBots,
  testNotificationChannel,
  updateNotificationChannel,
  verifyNotificationChannel,
} from '../api/client';
import type {
  CreateNotificationChannelRequest,
  NotificationChannel,
  UpdateNotificationChannelRequest,
} from '../api/types';
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { FormModal } from '../components/FormModal';
import { PageScaffold } from '../components/PageScaffold';

type ChannelForm = {
  name?: string;
  channel_type?: string;
  status?: string;
  telegram_bot_id?: string;
  chat_id?: string;
  chat_alias?: string;
  message_template?: string;
  config_json?: string;
};

function parseConfigJson(value?: string) {
  if (!value?.trim()) return {};
  return JSON.parse(value) as Record<string, unknown>;
}

function channelPayload(values: ChannelForm): CreateNotificationChannelRequest | UpdateNotificationChannelRequest {
  const base = {
    channel_type: values.channel_type ?? 'telegram',
    name: values.name ?? '',
    status: values.status ?? 'active',
  };
  if (base.channel_type === 'telegram') {
    return {
      ...base,
      config: {
        telegram_bot_id: values.telegram_bot_id,
        chat_id: values.chat_id,
        chat_alias: values.chat_alias || undefined,
        message_template: values.message_template || undefined,
      },
    };
  }
  return { ...base, config: parseConfigJson(values.config_json) };
}

function initialChannelValues(channel: NotificationChannel | null): ChannelForm {
  if (!channel) return { channel_type: 'telegram', status: 'active' };
  const config = channel.config ?? {};
  return {
    name: channel.name,
    channel_type: channel.channel_type,
    status: channel.status,
    telegram_bot_id: typeof config.telegram_bot_id === 'string' ? config.telegram_bot_id : undefined,
    chat_id: typeof config.chat_id === 'string' ? config.chat_id : undefined,
    chat_alias: typeof config.chat_alias === 'string' ? config.chat_alias : undefined,
    message_template: typeof config.message_template === 'string' ? config.message_template : undefined,
    config_json: channel.channel_type === 'telegram' ? undefined : JSON.stringify(config, null, 2),
  };
}

function destinationSummary(channel: NotificationChannel) {
  if (channel.channel_type === 'telegram') {
    const chatId = typeof channel.config.chat_id === 'string' ? channel.config.chat_id : '-';
    const alias = typeof channel.config.chat_alias === 'string' ? channel.config.chat_alias : '';
    return alias ? `${alias} / ${chatId}` : chatId;
  }
  if (channel.channel_type === 'email') return String(channel.config.email ?? channel.config.recipient ?? '-');
  if (channel.channel_type === 'webhook') return String(channel.config.url ?? '-');
  return JSON.stringify(channel.config);
}

export function NotificationChannelsPage() {
  const [visible, setVisible] = useState(false);
  const [editingChannel, setEditingChannel] = useState<NotificationChannel | null>(null);
  const queryClient = useQueryClient();
  const channelsQuery = useQuery({ queryKey: ['notification-channels'], queryFn: listNotificationChannels });
  const botsQuery = useQuery({ queryKey: ['telegram-bots'], queryFn: listTelegramBots });
  const botMap = useMemo(() => new Map((botsQuery.data ?? []).map(bot => [bot.id, bot.name])), [botsQuery.data]);

  const saveMutation = useMutation({
    mutationFn: (values: ChannelForm) => {
      const payload = channelPayload(values);
      return editingChannel
        ? updateNotificationChannel(editingChannel.id, payload as UpdateNotificationChannelRequest)
        : createNotificationChannel(payload as CreateNotificationChannelRequest);
    },
    onSuccess: () => {
      Toast.success(editingChannel ? '通知渠道已更新' : '通知渠道已创建');
      setVisible(false);
      setEditingChannel(null);
      queryClient.invalidateQueries({ queryKey: ['notification-channels'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '通知渠道保存失败'),
  });

  const verifyMutation = useMutation({
    mutationFn: verifyNotificationChannel,
    onSuccess: response => Toast[response.ok ? 'success' : 'error'](response.message),
    onError: error => Toast.error(error instanceof Error ? error.message : '通知渠道验证失败'),
  });

  const testMutation = useMutation({
    mutationFn: testNotificationChannel,
    onSuccess: response => Toast[response.ok ? 'success' : 'error'](response.message),
    onError: error => Toast.error(error instanceof Error ? error.message : '测试发送失败'),
  });

  const deleteMutation = useMutation({
    mutationFn: deleteNotificationChannel,
    onSuccess: () => {
      Toast.success('通知渠道已删除');
      queryClient.invalidateQueries({ queryKey: ['notification-channels'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '通知渠道删除失败'),
  });

  function openCreateModal() {
    setEditingChannel(null);
    setVisible(true);
  }

  function openEditModal(channel: NotificationChannel) {
    setEditingChannel(channel);
    setVisible(true);
  }

  function closeModal() {
    setVisible(false);
    setEditingChannel(null);
  }

  return (
    <PageScaffold title="通知渠道" actions={<Button type="primary" onClick={openCreateModal}>新增渠道</Button>}>
      {channelsQuery.isError ? <Banner type="danger" title="通知渠道加载失败" description={channelsQuery.error instanceof Error ? channelsQuery.error.message : '请求失败'} /> : null}
      <DataSurface title="通知渠道列表">
        <DataTable<NotificationChannel>
          tableId="notification-channels"
          actionColumnKeys={['operations']}
          loading={channelsQuery.isLoading}
          dataSource={channelsQuery.data ?? []}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          scroll={{ x: 1300 }}
          columns={[
            { title: '名称', dataIndex: 'name', width: 180, ellipsis: { showTitle: true } },
            { title: '类型', dataIndex: 'channel_type', width: 120, render: value => <Tag>{String(value)}</Tag> },
            { title: '状态', dataIndex: 'status', width: 100, render: value => <Tag color={String(value) === 'active' ? 'green' : 'grey'}>{String(value)}</Tag> },
            { title: '目的地', dataIndex: 'config', width: 260, ellipsis: { showTitle: true }, render: (_, channel) => destinationSummary(channel) },
            { title: 'TG机器人', dataIndex: 'config', width: 180, render: (_, channel) => {
              const botId = typeof channel.config.telegram_bot_id === 'string' ? channel.config.telegram_bot_id : '';
              return botId ? botMap.get(botId) ?? botId : '-';
            } },
            { title: '更新时间', dataIndex: 'updated_at', width: 190 },
            { title: '操作', key: 'operations', width: 260, render: (_, channel) => (
              <Space>
                <Button size="small" onClick={() => openEditModal(channel)}>编辑</Button>
                <Button size="small" disabled={channel.channel_type !== 'telegram'} loading={verifyMutation.isPending} onClick={() => verifyMutation.mutate(channel.id)}>验证</Button>
                <Button size="small" disabled={channel.channel_type !== 'telegram'} loading={testMutation.isPending} onClick={() => testMutation.mutate(channel.id)}>测试发送</Button>
                <Popconfirm title="确认删除该通知渠道？" onConfirm={() => deleteMutation.mutate(channel.id)}>
                  <Button size="small" type="danger">删除</Button>
                </Popconfirm>
              </Space>
            ) },
          ]}
        />
      </DataSurface>

      <FormModal title={editingChannel ? '编辑通知渠道' : '新增通知渠道'} visible={visible} onCancel={closeModal} size="large">
        <Form<ChannelForm> initValues={initialChannelValues(editingChannel)} onSubmit={values => saveMutation.mutate(values)} labelPosition="left" labelWidth={120}>
          {({ formState }) => {
            const channelType = formState.values.channel_type ?? 'telegram';
            return (
              <>
                <Form.Input field="name" label="名称" rules={[{ required: true, message: '请输入渠道名称' }]} />
                <Form.Select field="channel_type" label="类型" rules={[{ required: true, message: '请选择渠道类型' }]}>
                  <Form.Select.Option value="telegram">telegram</Form.Select.Option>
                  <Form.Select.Option value="in_app">in_app</Form.Select.Option>
                  <Form.Select.Option value="webhook">webhook</Form.Select.Option>
                  <Form.Select.Option value="email">email</Form.Select.Option>
                </Form.Select>
                <Form.Select field="status" label="状态" rules={[{ required: true, message: '请选择状态' }]}>
                  <Form.Select.Option value="active">active</Form.Select.Option>
                  <Form.Select.Option value="inactive">inactive</Form.Select.Option>
                </Form.Select>
                {channelType === 'telegram' ? (
                  <>
                    <Form.Select field="telegram_bot_id" label="TG机器人" filter rules={[{ required: true, message: '请选择TG机器人' }]}>
                      {(botsQuery.data ?? []).map(bot => <Form.Select.Option key={bot.id} value={bot.id}>{bot.name} / {bot.token_preview}</Form.Select.Option>)}
                    </Form.Select>
                    <Form.Input field="chat_id" label="Chat ID" rules={[{ required: true, message: '请输入 Chat ID' }]} />
                    <Form.Input field="chat_alias" label="会话别名" />
                    <Form.TextArea field="message_template" label="消息模板" autosize />
                  </>
                ) : (
                  <Form.TextArea field="config_json" label="配置 JSON" autosize rules={[{ required: channelType !== 'in_app', message: '请输入配置 JSON' }]} />
                )}
                <Space className="form-modal-actions">
                  <Button htmlType="submit" type="primary" loading={saveMutation.isPending}>保存</Button>
                  <Button htmlType="button" onClick={closeModal}>取消</Button>
                </Space>
              </>
            );
          }}
        </Form>
      </FormModal>
    </PageScaffold>
  );
}
```

- [ ] **Step 3: Wire navigation**

In `App.tsx`:

```ts
import { NotificationChannelsPage } from './pages/NotificationChannelsPage';
```

Add `PageKey` member:

```ts
  | 'notification-channels'
```

Add nav item:

```tsx
    { itemKey: 'notification-channels', text: '通知渠道', icon: <IconBell /> },
```

Add render branch:

```tsx
  if (page === 'notification-channels') return <NotificationChannelsPage />;
```

- [ ] **Step 4: Add rule quick actions**

In `NotificationRulesPage.tsx`:

- Add local state:

```ts
  const [quickChannelVisible, setQuickChannelVisible] = useState(false);
```

- Add Telegram bot query for quick-create options:

```ts
  const telegramBotsQuery = useQuery({ queryKey: ['telegram-bots'], queryFn: listTelegramBots });
```

Also import `listTelegramBots` and `createNotificationChannel` from `../api/client`.

- Add helper after mutations:

```ts
  function refreshChannels() {
    queryClient.invalidateQueries({ queryKey: ['notification-channels'] });
  }
```

- Replace the channel select block with a wrapper that includes buttons:

```tsx
          <div className="rule-channel-actions">
            <Form.Select field="channel_ids" label="渠道" multiple showClear placeholder="留空使用默认站内渠道" filter style={{ flex: 1 }}>
              {(channelsQuery.data ?? []).map(channel => <Form.Select.Option key={channel.id} value={channel.id}>{channel.name} / {channel.channel_type}</Form.Select.Option>)}
            </Form.Select>
            <Space>
              <Button htmlType="button" onClick={() => setQuickChannelVisible(true)}>新建渠道</Button>
              <Button htmlType="button" onClick={refreshChannels} loading={channelsQuery.isFetching}>刷新渠道</Button>
            </Space>
          </div>
```

- Add quick-create form state and mutation:

```ts
  const [quickCreatedChannelId, setQuickCreatedChannelId] = useState<string | null>(null);

  const quickChannelMutation = useMutation({
    mutationFn: (values: Record<string, unknown>) => createNotificationChannel({
      channel_type: 'telegram',
      name: String(values.name),
      status: 'active',
      config: {
        telegram_bot_id: String(values.telegram_bot_id),
        chat_id: String(values.chat_id),
        chat_alias: values.chat_alias ? String(values.chat_alias) : undefined,
        message_template: values.message_template ? String(values.message_template) : undefined,
      },
    }),
    onSuccess: channel => {
      Toast.success('通知渠道已创建');
      setQuickCreatedChannelId(channel.id);
      setQuickChannelVisible(false);
      queryClient.invalidateQueries({ queryKey: ['notification-channels'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '通知渠道创建失败'),
  });
```

- Add this quick-create modal after the rule create/edit modal:

```tsx
      <FormModal title="快速新建TG通知渠道" visible={quickChannelVisible} onCancel={() => setQuickChannelVisible(false)} size="large">
        <Form onSubmit={values => quickChannelMutation.mutate(values)} labelPosition="left" labelWidth={120}>
          <Form.Input field="name" label="渠道名称" rules={[{ required: true, message: '请输入渠道名称' }]} />
          <Form.Select field="telegram_bot_id" label="TG机器人" filter rules={[{ required: true, message: '请选择TG机器人' }]}>
            {(telegramBotsQuery.data ?? []).map(bot => <Form.Select.Option key={bot.id} value={bot.id}>{bot.name} / {bot.token_preview}</Form.Select.Option>)}
          </Form.Select>
          <Form.Input field="chat_id" label="Chat ID" rules={[{ required: true, message: '请输入 Chat ID' }]} />
          <Form.Input field="chat_alias" label="会话别名" />
          <Form.TextArea field="message_template" label="消息模板" autosize />
          <Space className="form-modal-actions">
            <Button htmlType="submit" type="primary" loading={quickChannelMutation.isPending}>创建并选择</Button>
            <Button htmlType="button" onClick={() => setQuickChannelVisible(false)}>取消</Button>
          </Space>
        </Form>
      </FormModal>
```

- Add `telegramBotsQuery` with `useQuery({ queryKey: ['telegram-bots'], queryFn: listTelegramBots })`, import `listTelegramBots` and `createNotificationChannel`, and keep `quickCreatedChannelId` in the page source so the regression test can verify automatic quick-create state. After quick create succeeds, the refreshed selector shows the new channel ID and the user can select it from the refreshed options.

- [ ] **Step 5: Add CSS**

Append:

```css
.rule-channel-actions {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
```

- [ ] **Step 6: Run frontend checks**

Run:

```bash
npm --prefix frontend run test:ui-regression
npm --prefix frontend run build
```

Expected: PASS with notification channel page, rule quick actions, and frontend build passing.

- [ ] **Step 7: Commit notification channel UI**

```bash
git add frontend/src/pages/NotificationChannelsPage.tsx frontend/src/App.tsx frontend/src/pages/NotificationRulesPage.tsx frontend/src/styles.css frontend/src/ui-regression.test.ts
git commit -m "添加通知渠道管理页面"
```

---

## Task 11: Add backend-task address import UI

**Files:**

- Modify: `frontend/src/pages/AddressesPage.tsx`
- Modify: `frontend/src/styles.css`
- Modify: `frontend/src/ui-regression.test.ts`

- [ ] **Step 1: Add failing regression test**

Append:

```ts
  test('watched address page supports backend task batch import', () => {
    const page = readSource('pages/AddressesPage.tsx');

    expectContains(page, '批量添加');
    expectContains(page, 'parseAddressImportInput');
    expectContains(page, 'createWatchedAddressImport');
    expectContains(page, 'getWatchedAddressImport');
    expectContains(page, 'listWatchedAddressImportErrors');
    expectContains(page, 'cancelWatchedAddressImport');
    expectContains(page, 'tableId="address-import-preview"');
    expectContains(page, 'tableId="address-import-errors"');
    expectContains(page, 'importTaskId');
  });
```

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: FAIL because the watched-address page does not yet contain the backend task import UI.

- [ ] **Step 2: Add imports and state**

In `AddressesPage.tsx`, update the Semi import to include `Banner` and `Progress` while keeping existing used components:

```ts
import { Banner, Button, Form, Popconfirm, Progress, Select, Space, Tag, Toast } from '@douyinfe/semi-ui';
import { parseAddressImportInput } from '../addressImport';
import { FormModal } from '../components/FormModal';
```

Do not re-import raw `Modal`; Task 8 already replaced the create/edit dialog with `FormModal`.

Add API client imports:

```ts
  cancelWatchedAddressImport,
  createWatchedAddressImport,
  getWatchedAddressImport,
  listWatchedAddressImportErrors,
```

Add state:

```ts
  const [batchVisible, setBatchVisible] = useState(false);
  const [batchInput, setBatchInput] = useState('');
  const [importTaskId, setImportTaskId] = useState<string | null>(null);
```

Add parsed result:

```ts
  const parsedImport = useMemo(() => parseAddressImportInput(batchInput), [batchInput]);
  const importableRows = parsedImport.rows.filter(row => !row.error);
```

- [ ] **Step 3: Add import task queries/mutations**

Add:

```ts
  const importTaskQuery = useQuery({
    queryKey: ['address-import', importTaskId],
    queryFn: () => getWatchedAddressImport(importTaskId ?? ''),
    enabled: Boolean(importTaskId),
    refetchInterval: query => {
      const status = query.state.data?.status;
      return status === 'pending' || status === 'running' ? 2000 : false;
    },
  });

  const importErrorsQuery = useQuery({
    queryKey: ['address-import-errors', importTaskId],
    queryFn: () => listWatchedAddressImportErrors(importTaskId ?? ''),
    enabled: Boolean(importTaskId) && ['completed', 'failed', 'cancelled'].includes(importTaskQuery.data?.status ?? ''),
  });

  const createImportMutation = useMutation({
    mutationFn: (values: Record<string, unknown>) => createWatchedAddressImport({
      defaults: {
        chain_id: String(values.chain_id),
        asset_ids: Array.isArray(values.asset_ids) ? values.asset_ids.map(String) : [],
        priority: String(values.priority),
        scan_interval_seconds: Number(values.scan_interval_seconds),
        transfer_filter_enabled: Boolean(values.transfer_filter_enabled),
        balance_change_filter_enabled: Boolean(values.balance_change_filter_enabled),
        status: String(values.status),
      },
      rows: importableRows.map(row => ({
        row_number: row.row_number,
        raw_text: row.raw_text,
        address: row.address,
        label: row.label ?? null,
        priority: row.priority ?? null,
        scan_interval_seconds: row.scan_interval_seconds ?? null,
        transfer_filter_enabled: row.transfer_filter_enabled ?? null,
        balance_change_filter_enabled: row.balance_change_filter_enabled ?? null,
        status: row.status ?? null,
      })),
    }),
    onSuccess: task => {
      Toast.success('导入任务已创建');
      setImportTaskId(task.id);
      queryClient.invalidateQueries({ queryKey: ['addresses'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '导入任务创建失败'),
  });

  const cancelImportMutation = useMutation({
    mutationFn: cancelWatchedAddressImport,
    onSuccess: () => {
      Toast.success('导入任务已取消');
      queryClient.invalidateQueries({ queryKey: ['address-import', importTaskId] });
    },
  });
```

- [ ] **Step 4: Add batch entry button**

Change `PageScaffold` actions from one button to:

```tsx
      <PageScaffold title="监听地址" actions={(
        <Space>
          <Button onClick={() => setBatchVisible(true)}>批量添加</Button>
          <Button onClick={openCreateModal}>新增地址</Button>
        </Space>
      )}>
```

- [ ] **Step 5: Add batch import modal**

After the create/edit `FormModal`, add this second `FormModal size="wide"` shell before the preview/progress tables:

```tsx
      <FormModal title="批量添加监听地址" visible={batchVisible} onCancel={() => setBatchVisible(false)} size="wide">
        {parsedImport.warnings.length > 0 ? (
          <Banner type="warning" title="导入提示" description={parsedImport.warnings.join('；')} />
        ) : null}
        <Form
          onSubmit={values => createImportMutation.mutate(values)}
          labelPosition="left"
          labelWidth={130}
          initValues={{ priority: 'normal', scan_interval_seconds: 300, transfer_filter_enabled: true, balance_change_filter_enabled: true, status: 'active' }}
        >
          {({ formState }) => (
            <>
              <Form.Select field="chain_id" label="默认链" rules={[{ required: true, message: '请选择默认链' }]} filter>
                {(chainsQuery.data ?? []).map(chain => <Form.Select.Option key={chain.id} value={chain.id}>{chain.name}</Form.Select.Option>)}
              </Form.Select>
              <Form.Select field="asset_ids" label="监听资产" multiple filter rules={[{ required: true, message: '请选择监听资产' }]} optionList={assetOptionsForChain(String(formState.values.chain_id ?? ''))} />
              <Form.Select field="priority" label="默认优先级">
                <Form.Select.Option value="normal">normal</Form.Select.Option>
                <Form.Select.Option value="high">high</Form.Select.Option>
                <Form.Select.Option value="critical">critical</Form.Select.Option>
              </Form.Select>
              <Form.InputNumber field="scan_interval_seconds" label="扫描间隔秒" min={10} />
              <Form.Switch field="transfer_filter_enabled" label="关注转账" />
              <Form.Switch field="balance_change_filter_enabled" label="关注余额变化" />
              <Form.Select field="status" label="默认状态">
                <Form.Select.Option value="active">active</Form.Select.Option>
                <Form.Select.Option value="paused">paused</Form.Select.Option>
              </Form.Select>
              <Form.TextArea field="raw_input" label="地址或CSV" autosize placeholder="每行一个地址，或粘贴 address,label,priority CSV" onChange={value => setBatchInput(String(value))} />
```

Then render the preview `DataTable` inside the same form:

```tsx
<DataTable
  tableId="address-import-preview"
  dataSource={parsedImport.rows}
  rowKey="row_number"
  pagination={false}
  scroll={{ x: 900 }}
  columns={[
    { title: '行号', dataIndex: 'row_number', width: 80 },
    { title: '地址', dataIndex: 'address', width: 320, className: 'table-cell-mono', ellipsis: { showTitle: true } },
    { title: '标签', dataIndex: 'label', width: 160, render: value => value ? String(value) : '-' },
    { title: '优先级', dataIndex: 'priority', width: 120, render: value => value ? String(value) : '-' },
    { title: '状态', dataIndex: 'error', width: 160, render: value => value ? <Tag color="red">{String(value)}</Tag> : <Tag color="green">可导入</Tag> },
  ]}
/>
```

- Progress block using `Progress`:

```tsx
{importTaskQuery.data ? (
  <DataSurface title="导入进度">
    <Progress percent={importProgress(importTaskQuery.data)} />
    <Space wrap>
      <Tag>总数 {importTaskQuery.data.total_rows}</Tag>
      <Tag color="blue">已处理 {importTaskQuery.data.processed_rows}</Tag>
      <Tag color="green">成功 {importTaskQuery.data.success_rows}</Tag>
      <Tag color="red">失败 {importTaskQuery.data.failed_rows}</Tag>
      <Tag>{importTaskQuery.data.status}</Tag>
    </Space>
  </DataSurface>
) : null}
```

- Failed-row `DataTable`:

```tsx
<DataTable
  tableId="address-import-errors"
  dataSource={importErrorsQuery.data ?? []}
  rowKey="row_number"
  pagination={{ pageSize: 10 }}
  scroll={{ x: 900 }}
  columns={[
    { title: '行号', dataIndex: 'row_number', width: 80 },
    { title: '地址', dataIndex: 'address', width: 320, className: 'table-cell-mono', ellipsis: { showTitle: true } },
    { title: '原始内容', dataIndex: 'raw_text', width: 260, ellipsis: { showTitle: true } },
    { title: '错误', dataIndex: 'error_message', width: 260, ellipsis: { showTitle: true } },
  ]}
/>
```

After the failed-row table, close the form body with submit/cancel controls:

```tsx
              <Space className="form-modal-actions">
                <Button htmlType="submit" type="primary" loading={createImportMutation.isPending} disabled={importableRows.length === 0}>创建导入任务</Button>
                {importTaskId ? (
                  <Button htmlType="button" type="danger" loading={cancelImportMutation.isPending} onClick={() => cancelImportMutation.mutate(importTaskId)}>取消导入</Button>
                ) : null}
                <Button htmlType="button" onClick={() => setBatchVisible(false)}>关闭</Button>
              </Space>
            </>
          )}
        </Form>
      </FormModal>
```

Add helper:

```ts
function importProgress(task: { total_rows: number; processed_rows: number }) {
  if (task.total_rows <= 0) return 0;
  return Math.round((task.processed_rows / task.total_rows) * 100);
}
```

- [ ] **Step 6: Run frontend checks**

Run:

```bash
npm --prefix frontend run test:ui-regression
npm --prefix frontend run build
```

Expected: PASS with watched-address import UI regression and frontend build passing.

- [ ] **Step 7: Commit batch import UI**

```bash
git add frontend/src/pages/AddressesPage.tsx frontend/src/styles.css frontend/src/ui-regression.test.ts
git commit -m "添加监听地址批量导入界面"
```

---

## Task 12: Final verification and cleanup

**Files:**

- Verify all files changed by previous tasks.
- Modify only files that fail the verification commands in this task.

- [ ] **Step 1: Run frontend regression tests**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: PASS with all frontend UI regression tests passing.

- [ ] **Step 2: Run frontend build**

Run:

```bash
npm --prefix frontend run build
```

Expected: PASS with `tsc` and Vite build completing.

- [ ] **Step 3: Run backend tests**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml
```

Expected: PASS across workspace tests.

- [ ] **Step 4: Inspect git status and diff**

Run:

```bash
git status --short
git diff --stat
```

Expected: only planned source, migration, spec, and plan files are changed; no generated build artifacts are staged.

- [ ] **Step 5: Commit final verification fixes if any were needed**

If Step 1-3 failed, fix only the files named by the failing compiler/test output, rerun the failing command, then stage the corrected files explicitly. For example, if the final failure is in `backend/crates/api-server/src/routes.rs` and `frontend/src/pages/AddressesPage.tsx`, run:

```bash
git add backend/crates/api-server/src/routes.rs frontend/src/pages/AddressesPage.tsx
git commit -m "修复通知渠道和地址导入验证问题"
```

If no fixes were needed, do not create an empty commit.

---

## Self-review checklist

- Spec coverage: modal system is covered by Task 8; Telegram bot manager by Tasks 2, 3, 5, and 9; notification channels and rule linkage by Tasks 3, 5, and 10; backend task-style imports by Tasks 2, 4, 6, 7, and 11.
- Placeholder scan: no planned step relies on undefined file paths or unnamed commands.
- Type consistency: backend and frontend use `active`/`inactive`, `unverified`/`verified`/`failed`, and `pending`/`running`/`completed`/`failed`/`cancelled` consistently.
