# Telegram Channel Binding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace manual Telegram Chat ID entry with a verification-code binding flow that supports private chats and group chats.

**Architecture:** Add a backend Telegram binding module that stores pending binding requests, processes Telegram updates from both webhook and `getUpdates`, and writes trusted chat metadata. Keep notification channel config compatible with the current `telegram_bot_id + chat_id + chat_alias + message_template` shape so existing delivery logic continues to work. Frontend forms use a reusable binding panel instead of manual Chat ID fields.

**Tech Stack:** Rust, Axum, SQLx, PostgreSQL, Reqwest, Tokio, React, TypeScript, TanStack Query, Semi Design, node:test source-level UI regressions.

---

## File Structure

**Create**

- `backend/crates/storage/migrations/0015_telegram_channel_bindings.sql` — DB tables for binding requests and Telegram polling offsets.
- `backend/crates/storage/src/telegram_bindings.rs` — storage repository and pure helpers for binding request lifecycle.
- `backend/crates/notifier/src/telegram_updates.rs` — Telegram update structs, code extraction, and provider payload helpers.
- `frontend/src/components/TelegramBindingPanel.tsx` — reusable Semi UI panel for creating and polling binding requests.

**Modify**

- `backend/crates/storage/src/lib.rs` — export `telegram_bindings` module.
- `backend/crates/core/src/models.rs` — shared DTOs for binding APIs and Telegram update payloads.
- `backend/crates/notifier/src/external.rs` — add Telegram `getUpdates` and binding confirmation send helpers.
- `backend/crates/notifier/src/lib.rs` — add shared binding processor and polling runner.
- `backend/crates/api-server/src/routes.rs` — add binding APIs and webhook route.
- `backend/crates/api-server/src/main.rs` — run the Telegram update poller for standalone API server.
- `backend/crates/all-in-one/src/main.rs` — run the Telegram update poller in all-in-one mode.
- `frontend/src/api/types.ts` — binding API response/request types.
- `frontend/src/api/client.ts` — binding API client functions.
- `frontend/src/pages/NotificationChannelsPage.tsx` — replace manual Chat ID input with `TelegramBindingPanel`.
- `frontend/src/pages/NotificationRulesPage.tsx` — reuse `TelegramBindingPanel` in quick-create TG channel modal.
- `frontend/src/ui-regression.test.ts` — frontend source regressions.

---

### Task 1: Backend models and migration

**Files:**
- Create: `backend/crates/storage/migrations/0015_telegram_channel_bindings.sql`
- Modify: `backend/crates/core/src/models.rs`

- [ ] **Step 1: Write the failing model/migration tests**

Append these tests inside the existing `#[cfg(test)] mod tests` in `backend/crates/core/src/models.rs`:

```rust
#[test]
fn telegram_binding_response_carries_private_and_group_binding_fields() {
    let now = Utc.with_ymd_and_hms(2026, 5, 22, 12, 0, 0).unwrap();
    let response = TelegramBindingRequest {
        id: Uuid::from_u128(1),
        tenant_id: Uuid::from_u128(2),
        telegram_bot_id: Uuid::from_u128(3),
        status: "pending".to_string(),
        bind_token: "bind_abc".to_string(),
        short_code: "CL-7K2P9Q".to_string(),
        deep_link_url: Some("https://t.me/demo_bot?start=bind_abc".to_string()),
        chat_id: None,
        chat_type: None,
        chat_title: None,
        chat_username: None,
        confirmation_error: None,
        expires_at: now,
        bound_at: None,
        created_at: now,
        updated_at: now,
    };

    assert_eq!(response.bind_token, "bind_abc");
    assert_eq!(response.short_code, "CL-7K2P9Q");
    assert_eq!(response.deep_link_url.as_deref(), Some("https://t.me/demo_bot?start=bind_abc"));
}

#[test]
fn create_telegram_binding_request_names_target_bot() {
    let request = CreateTelegramBindingRequest {
        telegram_bot_id: Uuid::from_u128(9),
    };

    assert_eq!(request.telegram_bot_id, Uuid::from_u128(9));
}
```

Also add this migration test near the model tests or a small helper test in the same module:

```rust
#[test]
fn telegram_binding_migration_defines_request_and_offset_tables() {
    let migration = include_str!("../../../storage/migrations/0015_telegram_channel_bindings.sql");

    assert!(migration.contains("CREATE TABLE IF NOT EXISTS telegram_binding_requests"));
    assert!(migration.contains("CREATE TABLE IF NOT EXISTS telegram_bot_update_offsets"));
    assert!(migration.contains("short_code TEXT NOT NULL"));
    assert!(migration.contains("last_update_id BIGINT NOT NULL DEFAULT 0"));
    assert!(migration.contains("status IN ('pending', 'bound', 'expired', 'cancelled')"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml telegram_binding -- --nocapture
```

Expected: FAIL because `TelegramBindingRequest`, `CreateTelegramBindingRequest`, and migration file do not exist.

- [ ] **Step 3: Add models**

In `backend/crates/core/src/models.rs`, add these imports if not already in scope inside tests: `chrono::TimeZone` is already used elsewhere; ensure tests import it.

Add these model types near the Telegram bot and notification channel types:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct CreateTelegramBindingRequest {
    pub telegram_bot_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct TelegramBindingRequest {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub telegram_bot_id: Uuid,
    pub status: String,
    pub bind_token: String,
    pub short_code: String,
    pub deep_link_url: Option<String>,
    pub chat_id: Option<String>,
    pub chat_type: Option<String>,
    pub chat_title: Option<String>,
    pub chat_username: Option<String>,
    pub confirmation_error: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub bound_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelegramChatBinding {
    pub chat_id: String,
    pub chat_type: String,
    pub chat_title: Option<String>,
    pub chat_username: Option<String>,
}
```

- [ ] **Step 4: Add migration**

Create `backend/crates/storage/migrations/0015_telegram_channel_bindings.sql`:

```sql
CREATE TABLE IF NOT EXISTS telegram_binding_requests (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    telegram_bot_id UUID NOT NULL REFERENCES telegram_bots(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'pending',
    bind_token TEXT NOT NULL,
    short_code TEXT NOT NULL,
    deep_link_url TEXT,
    chat_id TEXT,
    chat_type TEXT,
    chat_title TEXT,
    chat_username TEXT,
    confirmation_error TEXT,
    expires_at TIMESTAMPTZ NOT NULL,
    bound_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT telegram_binding_requests_status_check CHECK (status IN ('pending', 'bound', 'expired', 'cancelled')),
    CONSTRAINT telegram_binding_requests_pending_chat_check CHECK (
        status <> 'bound' OR chat_id IS NOT NULL
    )
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_telegram_binding_requests_bind_token
    ON telegram_binding_requests(bind_token);

CREATE UNIQUE INDEX IF NOT EXISTS idx_telegram_binding_requests_pending_short_code
    ON telegram_binding_requests(tenant_id, short_code)
    WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS idx_telegram_binding_requests_bot_status
    ON telegram_binding_requests(telegram_bot_id, status, expires_at);

CREATE TABLE IF NOT EXISTS telegram_bot_update_offsets (
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    telegram_bot_id UUID NOT NULL REFERENCES telegram_bots(id) ON DELETE CASCADE,
    last_update_id BIGINT NOT NULL DEFAULT 0,
    locked_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, telegram_bot_id)
);
```

- [ ] **Step 5: Run tests to verify they pass**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml telegram_binding -- --nocapture
```

Expected: PASS for the new model and migration tests.

- [ ] **Step 6: Commit**

```bash
git add backend/crates/core/src/models.rs backend/crates/storage/migrations/0015_telegram_channel_bindings.sql
git commit -m "添加TG渠道绑定数据模型"
```

---

### Task 2: Binding repository lifecycle

**Files:**
- Create: `backend/crates/storage/src/telegram_bindings.rs`
- Modify: `backend/crates/storage/src/lib.rs`
- Test: `backend/crates/storage/src/telegram_bindings.rs`

- [ ] **Step 1: Write failing repository tests**

Create `backend/crates/storage/src/telegram_bindings.rs` with only constants, function signatures, and tests first:

```rust
use chrono::{DateTime, Duration, Utc};
use coin_listener_core::{models::{TelegramBindingRequest, TelegramChatBinding}, AppError, AppResult};
use sqlx::PgPool;
use uuid::Uuid;

pub const BINDING_EXPIRY_MINUTES: i64 = 15;
pub const BINDING_STATUS_PENDING: &str = "pending";
pub const BINDING_STATUS_BOUND: &str = "bound";
pub const BINDING_STATUS_CANCELLED: &str = "cancelled";
pub const BINDING_STATUS_EXPIRED: &str = "expired";

pub fn normalize_short_code(value: &str) -> String {
    value.trim().to_ascii_uppercase()
}

pub fn validate_binding_status(status: &str) -> AppResult<()> {
    match status {
        BINDING_STATUS_PENDING | BINDING_STATUS_BOUND | BINDING_STATUS_CANCELLED | BINDING_STATUS_EXPIRED => Ok(()),
        _ => Err(AppError::Validation("telegram binding status must be pending, bound, expired, or cancelled".to_string())),
    }
}

pub async fn create_binding_request(
    _pool: &PgPool,
    _tenant_id: Uuid,
    _telegram_bot_id: Uuid,
    _bind_token: String,
    _short_code: String,
    _deep_link_url: Option<String>,
    _now: DateTime<Utc>,
) -> AppResult<TelegramBindingRequest> {
    unimplemented!("implemented after failing tests")
}

pub async fn get_binding_request(
    _pool: &PgPool,
    _tenant_id: Uuid,
    _id: Uuid,
) -> AppResult<TelegramBindingRequest> {
    unimplemented!("implemented after failing tests")
}

pub async fn cancel_binding_request(
    _pool: &PgPool,
    _tenant_id: Uuid,
    _id: Uuid,
) -> AppResult<TelegramBindingRequest> {
    unimplemented!("implemented after failing tests")
}

pub async fn bind_pending_request(
    _pool: &PgPool,
    _telegram_bot_id: Uuid,
    _code: &str,
    _chat: TelegramChatBinding,
    _now: DateTime<Utc>,
) -> AppResult<Option<TelegramBindingRequest>> {
    unimplemented!("implemented after failing tests")
}

#[cfg(test)]
mod tests {
    use super::*;
    use coin_listener_core::AppError;

    #[test]
    fn normalizes_short_codes_for_matching() {
        assert_eq!(normalize_short_code(" cl-7k2p9q "), "CL-7K2P9Q");
    }

    #[test]
    fn validates_known_binding_statuses() {
        for status in [BINDING_STATUS_PENDING, BINDING_STATUS_BOUND, BINDING_STATUS_CANCELLED, BINDING_STATUS_EXPIRED] {
            validate_binding_status(status).expect("known status");
        }
        assert!(matches!(validate_binding_status("failed"), Err(AppError::Validation(_))));
    }

    #[test]
    fn create_binding_query_requires_verified_active_bot() {
        assert!(CREATE_BINDING_REQUEST_QUERY.contains("verification_status = 'verified'"));
        assert!(CREATE_BINDING_REQUEST_QUERY.contains("status = 'active'"));
    }

    #[test]
    fn bind_query_only_binds_pending_non_expired_request_once() {
        assert!(BIND_PENDING_REQUEST_QUERY.contains("status = 'pending'"));
        assert!(BIND_PENDING_REQUEST_QUERY.contains("expires_at > $5"));
        assert!(BIND_PENDING_REQUEST_QUERY.contains("FOR UPDATE"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml telegram_bindings -- --nocapture
```

Expected: FAIL because query constants are not defined and module is not exported.

- [ ] **Step 3: Export module**

In `backend/crates/storage/src/lib.rs`, add:

```rust
pub mod telegram_bindings;
```

- [ ] **Step 4: Implement query constants and functions**

Replace the placeholder repository content with implementations using these constants:

```rust
pub const CREATE_BINDING_REQUEST_QUERY: &str = r#"
    INSERT INTO telegram_binding_requests (
        tenant_id, telegram_bot_id, bind_token, short_code, deep_link_url, expires_at
    )
    SELECT $1, $2, $3, $4, $5, $6
    WHERE EXISTS (
        SELECT 1 FROM telegram_bots
        WHERE id = $2
          AND tenant_id = $1
          AND status = 'active'
          AND verification_status = 'verified'
    )
    RETURNING id, tenant_id, telegram_bot_id, status, bind_token, short_code, deep_link_url,
              chat_id, chat_type, chat_title, chat_username, confirmation_error,
              expires_at, bound_at, created_at, updated_at
    "#;

pub const GET_BINDING_REQUEST_QUERY: &str = r#"
    SELECT id, tenant_id, telegram_bot_id, status, bind_token, short_code, deep_link_url,
           chat_id, chat_type, chat_title, chat_username, confirmation_error,
           expires_at, bound_at, created_at, updated_at
    FROM telegram_binding_requests
    WHERE id = $1
      AND tenant_id = $2
    "#;

pub const CANCEL_BINDING_REQUEST_QUERY: &str = r#"
    UPDATE telegram_binding_requests
    SET status = 'cancelled', updated_at = NOW()
    WHERE id = $1
      AND tenant_id = $2
      AND status = 'pending'
    RETURNING id, tenant_id, telegram_bot_id, status, bind_token, short_code, deep_link_url,
              chat_id, chat_type, chat_title, chat_username, confirmation_error,
              expires_at, bound_at, created_at, updated_at
    "#;

pub const BIND_PENDING_REQUEST_QUERY: &str = r#"
    UPDATE telegram_binding_requests
    SET status = 'bound',
        chat_id = $2,
        chat_type = $3,
        chat_title = $4,
        chat_username = $6,
        bound_at = $5,
        updated_at = NOW()
    WHERE id = (
        SELECT id
        FROM telegram_binding_requests
        WHERE telegram_bot_id = $1
          AND status = 'pending'
          AND expires_at > $5
          AND (bind_token = $7 OR short_code = $8)
        ORDER BY created_at ASC
        LIMIT 1
        FOR UPDATE
    )
    RETURNING id, tenant_id, telegram_bot_id, status, bind_token, short_code, deep_link_url,
              chat_id, chat_type, chat_title, chat_username, confirmation_error,
              expires_at, bound_at, created_at, updated_at
    "#;
```

Implementation details:

```rust
pub async fn create_binding_request(
    pool: &PgPool,
    tenant_id: Uuid,
    telegram_bot_id: Uuid,
    bind_token: String,
    short_code: String,
    deep_link_url: Option<String>,
    now: DateTime<Utc>,
) -> AppResult<TelegramBindingRequest> {
    let expires_at = now + Duration::minutes(BINDING_EXPIRY_MINUTES);
    sqlx::query_as::<_, TelegramBindingRequest>(CREATE_BINDING_REQUEST_QUERY)
        .bind(tenant_id)
        .bind(telegram_bot_id)
        .bind(bind_token)
        .bind(normalize_short_code(&short_code))
        .bind(deep_link_url)
        .bind(expires_at)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::Validation("telegram bot must be active and verified".to_string()))
}

pub async fn get_binding_request(pool: &PgPool, tenant_id: Uuid, id: Uuid) -> AppResult<TelegramBindingRequest> {
    sqlx::query_as::<_, TelegramBindingRequest>(GET_BINDING_REQUEST_QUERY)
        .bind(id)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("telegram binding request".to_string()))
}

pub async fn cancel_binding_request(pool: &PgPool, tenant_id: Uuid, id: Uuid) -> AppResult<TelegramBindingRequest> {
    sqlx::query_as::<_, TelegramBindingRequest>(CANCEL_BINDING_REQUEST_QUERY)
        .bind(id)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("pending telegram binding request".to_string()))
}

pub async fn bind_pending_request(
    pool: &PgPool,
    telegram_bot_id: Uuid,
    code: &str,
    chat: TelegramChatBinding,
    now: DateTime<Utc>,
) -> AppResult<Option<TelegramBindingRequest>> {
    sqlx::query_as::<_, TelegramBindingRequest>(BIND_PENDING_REQUEST_QUERY)
        .bind(telegram_bot_id)
        .bind(chat.chat_id)
        .bind(chat.chat_type)
        .bind(chat.chat_title)
        .bind(now)
        .bind(chat.chat_username)
        .bind(code)
        .bind(normalize_short_code(code))
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml telegram_bindings -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add backend/crates/storage/src/lib.rs backend/crates/storage/src/telegram_bindings.rs
git commit -m "添加TG绑定请求仓储"
```

---

### Task 3: Telegram update parsing and provider methods

**Files:**
- Create: `backend/crates/notifier/src/telegram_updates.rs`
- Modify: `backend/crates/notifier/src/lib.rs`
- Modify: `backend/crates/notifier/src/external.rs`

- [ ] **Step 1: Write failing parser tests**

Create `backend/crates/notifier/src/telegram_updates.rs`:

```rust
use coin_listener_core::models::TelegramChatBinding;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub message: Option<TelegramMessage>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TelegramMessage {
    pub text: Option<String>,
    pub chat: TelegramChat,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TelegramChat {
    pub id: serde_json::Value,
    #[serde(rename = "type")]
    pub chat_type: String,
    pub title: Option<String>,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

pub fn extract_binding_code(_text: &str) -> Option<String> {
    unimplemented!("implemented after failing tests")
}

pub fn chat_binding_from_update(_update: &TelegramUpdate) -> Option<TelegramChatBinding> {
    unimplemented!("implemented after failing tests")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_private_start_bind_token() {
        assert_eq!(extract_binding_code("/start bind_abc123"), Some("bind_abc123".to_string()));
    }

    #[test]
    fn extracts_group_short_code_with_or_without_bot_mention() {
        assert_eq!(extract_binding_code("@coin_listener_bot CL-7K2P9Q"), Some("CL-7K2P9Q".to_string()));
        assert_eq!(extract_binding_code("please bind CL-7K2P9Q now"), Some("CL-7K2P9Q".to_string()));
    }

    #[test]
    fn ignores_messages_without_binding_code() {
        assert_eq!(extract_binding_code("hello bot"), None);
    }

    #[test]
    fn maps_group_chat_to_binding_metadata() {
        let update = TelegramUpdate {
            update_id: 99,
            message: Some(TelegramMessage {
                text: Some("CL-7K2P9Q".to_string()),
                chat: TelegramChat {
                    id: json!(-1001234567890_i64),
                    chat_type: "supergroup".to_string(),
                    title: Some("Ops Alerts".to_string()),
                    username: None,
                    first_name: None,
                    last_name: None,
                },
            }),
        };

        let binding = chat_binding_from_update(&update).expect("chat binding");

        assert_eq!(binding.chat_id, "-1001234567890");
        assert_eq!(binding.chat_type, "supergroup");
        assert_eq!(binding.chat_title.as_deref(), Some("Ops Alerts"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml telegram_updates -- --nocapture
```

Expected: FAIL because functions are unimplemented and module is not exported.

- [ ] **Step 3: Export module**

In `backend/crates/notifier/src/lib.rs`, add near other module declarations:

```rust
pub mod telegram_updates;
```

- [ ] **Step 4: Implement parsing helpers**

In `backend/crates/notifier/src/telegram_updates.rs`, implement:

```rust
pub fn extract_binding_code(text: &str) -> Option<String> {
    text.split_whitespace()
        .find_map(|part| {
            let token = part.trim_matches(|ch: char| ch == ',' || ch == '.' || ch == ':' || ch == ';');
            if token.starts_with("bind_") || token.starts_with("CL-") {
                return Some(token.to_string());
            }
            None
        })
}

pub fn chat_binding_from_update(update: &TelegramUpdate) -> Option<TelegramChatBinding> {
    let chat = &update.message.as_ref()?.chat;
    let chat_id = match &chat.id {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Number(value) => value.to_string(),
        _ => return None,
    };
    let chat_title = chat.title.clone().or_else(|| {
        let name = [chat.first_name.as_deref(), chat.last_name.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        (!name.is_empty()).then_some(name)
    });
    Some(TelegramChatBinding {
        chat_id,
        chat_type: chat.chat_type.clone(),
        chat_title,
        chat_username: chat.username.clone(),
    })
}
```

- [ ] **Step 5: Write failing provider method tests**

In `backend/crates/notifier/src/external.rs`, add tests to the existing test module:

```rust
#[test]
fn telegram_get_updates_url_includes_offset() {
    let sender = crate::external::ExternalNotificationSender::with_telegram_api_base_url(
        reqwest::Client::new(),
        "https://telegram.test".to_string(),
    );

    let url = sender.telegram_get_updates_url_for_test("123:token", 42);

    assert_eq!(url, "https://telegram.test/bot123:token/getUpdates?offset=43&timeout=0");
}

#[test]
fn telegram_confirmation_text_names_bound_chat() {
    assert_eq!(
        crate::external::telegram_binding_confirmation_text("Ops Alerts"),
        "Coin Listener 通知渠道绑定成功：Ops Alerts"
    );
}
```

- [ ] **Step 6: Run tests to verify they fail**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml telegram_get_updates_url_includes_offset telegram_confirmation_text_names_bound_chat -- --nocapture
```

Expected: FAIL because the test helper and confirmation text function do not exist.

- [ ] **Step 7: Implement provider helpers**

In `backend/crates/notifier/src/external.rs`, add:

```rust
pub fn telegram_binding_confirmation_text(chat_name: &str) -> String {
    format!("Coin Listener 通知渠道绑定成功：{chat_name}")
}
```

Inside `impl ExternalNotificationSender`, add:

```rust
#[cfg(test)]
pub fn telegram_get_updates_url_for_test(&self, bot_token: &str, last_update_id: i64) -> String {
    self.telegram_get_updates_url(bot_token, last_update_id)
}

fn telegram_get_updates_url(&self, bot_token: &str, last_update_id: i64) -> String {
    format!(
        "{}?offset={}&timeout=0",
        self.telegram_bot_method_url(bot_token, "getUpdates"),
        last_update_id + 1,
    )
}
```

Then add runtime methods:

```rust
pub async fn get_telegram_updates(
    &self,
    bot_token: &str,
    last_update_id: i64,
) -> Result<Vec<crate::telegram_updates::TelegramUpdate>, ExternalSendOutcome> {
    let url = self.telegram_get_updates_url(bot_token, last_update_id);
    let response = self.client.get(&url).timeout(Duration::from_millis(5000)).send().await;
    match response {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = read_provider_response_prefix(response).await;
            if !matches!(status, 200..=299) {
                return Err(classify_telegram_response(status, &body));
            }
            let value: Value = serde_json::from_str(&body).map_err(|_| ExternalSendOutcome::PermanentFailure(ExternalSendMetadata {
                last_error: Some("telegram getUpdates returned invalid JSON".to_string()),
                provider_message_id: None,
                provider_status_code: Some(status as i32),
                provider_response: Some(truncate_provider_response(&body)),
            }))?;
            if value.get("ok").and_then(Value::as_bool) != Some(true) {
                return Err(classify_telegram_response(status, &body));
            }
            serde_json::from_value(value.get("result").cloned().unwrap_or_else(|| Value::Array(vec![])))
                .map_err(|_| ExternalSendOutcome::PermanentFailure(ExternalSendMetadata {
                    last_error: Some("telegram getUpdates result is invalid".to_string()),
                    provider_message_id: None,
                    provider_status_code: Some(status as i32),
                    provider_response: Some(truncate_provider_response(&body)),
                }))
        }
        Err(error) => Err(ExternalSendOutcome::TransientFailure(ExternalSendMetadata {
            last_error: Some(telegram_network_error_message(&url, &error.to_string())),
            provider_message_id: None,
            provider_status_code: None,
            provider_response: None,
        })),
    }
}
```

- [ ] **Step 8: Run tests to verify they pass**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml telegram_updates telegram_get_updates_url_includes_offset telegram_confirmation_text_names_bound_chat -- --nocapture
```

Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add backend/crates/notifier/src/lib.rs backend/crates/notifier/src/telegram_updates.rs backend/crates/notifier/src/external.rs
git commit -m "添加TG更新解析工具"
```

---

### Task 4: Shared binding processor and polling offsets

**Files:**
- Modify: `backend/crates/storage/src/telegram_bindings.rs`
- Modify: `backend/crates/notifier/src/lib.rs`

- [ ] **Step 1: Write failing offset repository tests**

Add these tests to `backend/crates/storage/src/telegram_bindings.rs`:

```rust
#[test]
fn offset_queries_are_scoped_to_bot_and_tenant() {
    assert!(GET_TELEGRAM_UPDATE_OFFSET_QUERY.contains("tenant_id = $1"));
    assert!(GET_TELEGRAM_UPDATE_OFFSET_QUERY.contains("telegram_bot_id = $2"));
    assert!(UPSERT_TELEGRAM_UPDATE_OFFSET_QUERY.contains("ON CONFLICT (tenant_id, telegram_bot_id)"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml offset_queries_are_scoped_to_bot_and_tenant -- --nocapture
```

Expected: FAIL because offset query constants do not exist.

- [ ] **Step 3: Add offset repository constants and functions**

In `backend/crates/storage/src/telegram_bindings.rs`, add:

```rust
pub const GET_TELEGRAM_UPDATE_OFFSET_QUERY: &str = r#"
    SELECT last_update_id
    FROM telegram_bot_update_offsets
    WHERE tenant_id = $1
      AND telegram_bot_id = $2
    "#;

pub const UPSERT_TELEGRAM_UPDATE_OFFSET_QUERY: &str = r#"
    INSERT INTO telegram_bot_update_offsets (tenant_id, telegram_bot_id, last_update_id)
    VALUES ($1, $2, $3)
    ON CONFLICT (tenant_id, telegram_bot_id)
    DO UPDATE SET last_update_id = GREATEST(telegram_bot_update_offsets.last_update_id, EXCLUDED.last_update_id),
                  updated_at = NOW()
    "#;

pub async fn get_telegram_update_offset(pool: &PgPool, tenant_id: Uuid, telegram_bot_id: Uuid) -> AppResult<i64> {
    let value = sqlx::query_scalar::<_, i64>(GET_TELEGRAM_UPDATE_OFFSET_QUERY)
        .bind(tenant_id)
        .bind(telegram_bot_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    Ok(value.unwrap_or(0))
}

pub async fn upsert_telegram_update_offset(pool: &PgPool, tenant_id: Uuid, telegram_bot_id: Uuid, update_id: i64) -> AppResult<()> {
    sqlx::query(UPSERT_TELEGRAM_UPDATE_OFFSET_QUERY)
        .bind(tenant_id)
        .bind(telegram_bot_id)
        .bind(update_id)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    Ok(())
}
```

- [ ] **Step 4: Write failing processor tests**

In `backend/crates/notifier/src/lib.rs`, add tests to the existing test module or a new `telegram_binding_tests` module:

```rust
#[test]
fn binding_processor_ignores_update_without_code() {
    let update = crate::telegram_updates::TelegramUpdate {
        update_id: 1,
        message: Some(crate::telegram_updates::TelegramMessage {
            text: Some("hello".to_string()),
            chat: crate::telegram_updates::TelegramChat {
                id: serde_json::json!(12345),
                chat_type: "private".to_string(),
                title: None,
                username: Some("alice".to_string()),
                first_name: Some("Alice".to_string()),
                last_name: None,
            },
        }),
    };

    assert_eq!(telegram_binding_candidate(&update), None);
}

#[test]
fn binding_processor_extracts_code_and_chat_candidate() {
    let update = crate::telegram_updates::TelegramUpdate {
        update_id: 2,
        message: Some(crate::telegram_updates::TelegramMessage {
            text: Some("/start bind_abc".to_string()),
            chat: crate::telegram_updates::TelegramChat {
                id: serde_json::json!(12345),
                chat_type: "private".to_string(),
                title: None,
                username: Some("alice".to_string()),
                first_name: Some("Alice".to_string()),
                last_name: None,
            },
        }),
    };

    let candidate = telegram_binding_candidate(&update).expect("candidate");

    assert_eq!(candidate.code, "bind_abc");
    assert_eq!(candidate.chat.chat_id, "12345");
}
```

- [ ] **Step 5: Run tests to verify they fail**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml binding_processor_ -- --nocapture
```

Expected: FAIL because `telegram_binding_candidate` does not exist.

- [ ] **Step 6: Implement candidate helper and runtime processor**

In `backend/crates/notifier/src/lib.rs`, add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramBindingCandidate {
    pub code: String,
    pub chat: coin_listener_core::models::TelegramChatBinding,
}

pub fn telegram_binding_candidate(update: &crate::telegram_updates::TelegramUpdate) -> Option<TelegramBindingCandidate> {
    let text = update.message.as_ref()?.text.as_deref()?;
    let code = crate::telegram_updates::extract_binding_code(text)?;
    let chat = crate::telegram_updates::chat_binding_from_update(update)?;
    Some(TelegramBindingCandidate { code, chat })
}

pub async fn process_telegram_binding_update(
    pool: &sqlx::PgPool,
    sender: &external::ExternalNotificationSender,
    telegram_bot_id: uuid::Uuid,
    bot_token: &str,
    update: &crate::telegram_updates::TelegramUpdate,
    now: chrono::DateTime<chrono::Utc>,
) -> coin_listener_core::AppResult<Option<coin_listener_core::models::TelegramBindingRequest>> {
    let Some(candidate) = telegram_binding_candidate(update) else {
        return Ok(None);
    };
    let Some(binding) = coin_listener_storage::telegram_bindings::bind_pending_request(
        pool,
        telegram_bot_id,
        &candidate.code,
        candidate.chat.clone(),
        now,
    ).await? else {
        return Ok(None);
    };
    let chat_name = binding.chat_title.as_deref()
        .or(binding.chat_username.as_deref())
        .or(binding.chat_id.as_deref())
        .unwrap_or("Telegram 会话");
    let config = external::TelegramChannelConfig {
        telegram_bot_id: Some(telegram_bot_id),
        bot_token_env: None,
        chat_id: binding.chat_id.clone().unwrap_or_else(|| candidate.chat.chat_id.clone()),
    };
    let outcome = sender.send_telegram(
        &config,
        bot_token,
        &external::telegram_binding_confirmation_text(chat_name),
    ).await;
    if !outcome.is_sent() {
        tracing::warn!(telegram_bot_id = %telegram_bot_id, update_id = update.update_id, "telegram binding confirmation failed");
    }
    Ok(Some(binding))
}
```

- [ ] **Step 7: Run tests to verify they pass**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml offset_queries_are_scoped_to_bot_and_tenant binding_processor_ -- --nocapture
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add backend/crates/storage/src/telegram_bindings.rs backend/crates/notifier/src/lib.rs
git commit -m "添加TG绑定处理器"
```

---

### Task 5: Backend binding APIs and webhook route

**Files:**
- Modify: `backend/crates/api-server/src/routes.rs`

- [ ] **Step 1: Write failing route tests**

Add tests inside `#[cfg(test)] mod tests` in `backend/crates/api-server/src/routes.rs`:

```rust
#[test]
fn telegram_binding_routes_are_registered() {
    let routes = include_str!("routes.rs");

    assert!(routes.contains("/api/telegram-bindings"));
    assert!(routes.contains("/api/telegram-bindings/:id"));
    assert!(routes.contains("/api/telegram-bindings/:id/cancel"));
    assert!(routes.contains("/api/telegram/webhook/:bot_id"));
}

#[test]
fn telegram_webhook_route_is_not_under_auth_layer() {
    let routes = include_str!("routes.rs");
    let webhook_index = routes.find("/api/telegram/webhook/:bot_id").expect("webhook route");
    let auth_layer_index = routes.find("route_layer(middleware::from_fn_with_state").expect("auth layer");

    assert!(webhook_index > auth_layer_index, "webhook route should be added outside protected router");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml telegram_binding_routes_are_registered telegram_webhook_route_is_not_under_auth_layer -- --nocapture
```

Expected: FAIL because routes are not registered.

- [ ] **Step 3: Register routes**

In `backend/crates/api-server/src/routes.rs`, add imports:

```rust
CreateTelegramBindingRequest,
```

Add protected routes before notification rules:

```rust
.route("/api/telegram-bindings", post(create_telegram_binding))
.route("/api/telegram-bindings/:id", get(get_telegram_binding))
.route("/api/telegram-bindings/:id/cancel", post(cancel_telegram_binding))
```

After applying auth layer, add public webhook route to the final router:

```rust
Router::new()
    .route("/health", get(health))
    .route("/api/auth/login", post(login))
    .route("/api/realtime/notifications", get(realtime_notifications))
    .route("/api/telegram/webhook/:bot_id", post(telegram_webhook))
    .merge(protected)
    .with_state(state)
```

- [ ] **Step 4: Implement handlers**

Add handlers:

```rust
async fn create_telegram_binding(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<CreateTelegramBindingRequest>,
) -> Result<Response, ApiError> {
    let bot = notifications::get_telegram_bot_secret(&state.postgres, auth.tenant_id, request.telegram_bot_id).await?;
    let bind_token = format!("bind_{}", Uuid::new_v4().simple());
    let short_code = format!("CL-{}", Uuid::new_v4().simple().to_string()[..6].to_ascii_uppercase());
    let deep_link_url = telegram_deep_link(&bot.name, &bind_token);
    let binding = coin_listener_storage::telegram_bindings::create_binding_request(
        &state.postgres,
        auth.tenant_id,
        request.telegram_bot_id,
        bind_token,
        short_code,
        deep_link_url,
        Utc::now(),
    ).await?;
    Ok((StatusCode::CREATED, Json(binding)).into_response())
}

async fn get_telegram_binding(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let binding = coin_listener_storage::telegram_bindings::get_binding_request(&state.postgres, auth.tenant_id, id).await?;
    Ok(Json(binding).into_response())
}

async fn cancel_telegram_binding(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let binding = coin_listener_storage::telegram_bindings::cancel_binding_request(&state.postgres, auth.tenant_id, id).await?;
    Ok(Json(binding).into_response())
}

async fn telegram_webhook(
    State(state): State<Arc<ApiState>>,
    Path(bot_id): Path<Uuid>,
    Json(update): Json<notifier::telegram_updates::TelegramUpdate>,
) -> Result<Response, ApiError> {
    let bot = notifications::telegram_bot_secret_by_id_any_tenant(&state.postgres, bot_id).await?;
    let sender = notifier::external::ExternalNotificationSender::new(reqwest::Client::new());
    notifier::process_telegram_binding_update(
        &state.postgres,
        &sender,
        bot_id,
        &bot.bot_token,
        &update,
        Utc::now(),
    ).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

fn telegram_deep_link(bot_name: &str, bind_token: &str) -> Option<String> {
    let username = bot_name.trim().trim_start_matches('@');
    (!username.is_empty()).then(|| format!("https://t.me/{username}?start={bind_token}"))
}
```

Also add this function to `backend/crates/storage/src/notifications.rs` so the unauthenticated Telegram webhook can resolve a bot from the public `bot_id` path without receiving a token:

```rust
pub async fn telegram_bot_secret_by_id_any_tenant(pool: &PgPool, id: Uuid) -> AppResult<TelegramBotSecret> {
    sqlx::query_as::<_, TelegramBotSecret>(r#"
        SELECT id, tenant_id, name, bot_token, token_preview, status, verification_status,
               last_verified_at, last_error, created_at, updated_at
        FROM telegram_bots
        WHERE id = $1
    "#)
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("telegram bot".to_string()))
}
```

Add this route source regression in `routes.rs` tests:

```rust
#[test]
fn telegram_webhook_uses_bot_id_without_token_path() {
    let routes = include_str!("routes.rs");

    assert!(routes.contains("telegram_bot_secret_by_id_any_tenant"));
    assert!(routes.contains("/api/telegram/webhook/:bot_id"));
    assert!(!routes.contains("/api/telegram/webhook/:bot_token"));
}
```

- [ ] **Step 5: Run tests**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml telegram_binding_routes_are_registered telegram_webhook_route_is_not_under_auth_layer -- --nocapture
```

Expected: PASS.

Then run compile check:

```bash
cargo test --locked --manifest-path backend/Cargo.toml -p api-server --no-run
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add backend/crates/api-server/src/routes.rs backend/crates/storage/src/notifications.rs
git commit -m "添加TG绑定API"
```

---

### Task 6: Telegram getUpdates poller runtime

**Files:**
- Modify: `backend/crates/notifier/src/lib.rs`
- Modify: `backend/crates/storage/src/notifications.rs`
- Modify: `backend/crates/api-server/src/main.rs`
- Modify: `backend/crates/all-in-one/src/main.rs`

- [ ] **Step 1: Write failing tests for poller wiring**

In `backend/crates/notifier/src/lib.rs`, add:

```rust
#[test]
fn telegram_update_poller_uses_binding_processor_and_offsets() {
    let source = include_str!("lib.rs");

    assert!(source.contains("run_telegram_update_poller"));
    assert!(source.contains("get_telegram_update_offset"));
    assert!(source.contains("upsert_telegram_update_offset"));
    assert!(source.contains("process_telegram_binding_update"));
}
```

In `backend/crates/api-server/src/main.rs`, add this test module at the end of the file:

```rust
#[cfg(test)]
mod telegram_binding_runtime_tests {
    #[test]
    fn api_server_starts_telegram_update_poller() {
        let source = include_str!("main.rs");
        assert!(source.contains("run_telegram_update_poller"));
    }
}
```

In `backend/crates/all-in-one/src/main.rs`, add:

```rust
#[cfg(test)]
mod telegram_binding_runtime_tests {
    #[test]
    fn all_in_one_starts_telegram_update_poller() {
        let source = include_str!("main.rs");
        assert!(source.contains("run_telegram_update_poller"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml telegram_update_poller_uses_binding_processor_and_offsets api_server_starts_telegram_update_poller all_in_one_starts_telegram_update_poller -- --nocapture
```

Expected: FAIL because poller is not implemented or wired.

- [ ] **Step 3: Add active verified bot query**

In `backend/crates/storage/src/notifications.rs`, add:

```rust
pub async fn list_active_verified_telegram_bot_secrets(pool: &PgPool) -> AppResult<Vec<TelegramBotSecret>> {
    sqlx::query_as::<_, TelegramBotSecret>(r#"
        SELECT id, tenant_id, name, bot_token, token_preview, status, verification_status,
               last_verified_at, last_error, created_at, updated_at
        FROM telegram_bots
        WHERE status = 'active'
          AND verification_status = 'verified'
        ORDER BY created_at ASC
    "#)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}
```

- [ ] **Step 4: Implement poller**

In `backend/crates/notifier/src/lib.rs`, add:

```rust
pub async fn run_telegram_update_poller(
    pool: sqlx::PgPool,
    sender: external::ExternalNotificationSender,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> coin_listener_core::AppResult<()> {
    while !shutdown.load(std::sync::atomic::Ordering::Relaxed) {
        let bots = coin_listener_storage::notifications::list_active_verified_telegram_bot_secrets(&pool).await?;
        for bot in bots {
            let offset = coin_listener_storage::telegram_bindings::get_telegram_update_offset(&pool, bot.tenant_id, bot.id).await?;
            match sender.get_telegram_updates(&bot.bot_token, offset).await {
                Ok(updates) => {
                    for update in updates {
                        process_telegram_binding_update(&pool, &sender, bot.id, &bot.bot_token, &update, chrono::Utc::now()).await?;
                        coin_listener_storage::telegram_bindings::upsert_telegram_update_offset(&pool, bot.tenant_id, bot.id, update.update_id).await?;
                    }
                }
                Err(outcome) => {
                    tracing::warn!(telegram_bot_id = %bot.id, error = ?outcome.metadata().last_error, "telegram getUpdates failed");
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
    Ok(())
}
```

- [ ] **Step 5: Wire poller in API server and all-in-one**

In `backend/crates/api-server/src/main.rs`, after realtime listener spawn:

```rust
let telegram_poller_shutdown = Arc::clone(&shutdown);
tokio::spawn(notifier::run_telegram_update_poller(
    postgres.clone(),
    notifier::external::ExternalNotificationSender::new(reqwest::Client::new()),
    telegram_poller_shutdown,
));
```

In `backend/crates/all-in-one/src/main.rs`, add the poller after `notifier_handle` is created:

```rust
let mut telegram_poller_handle = tokio::spawn(notifier::run_telegram_update_poller(
    postgres.clone(),
    ExternalNotificationSender::new(reqwest::Client::new()),
    Arc::clone(&shutdown),
));
```

Add it to the existing `tokio::select!`:

```rust
result = &mut telegram_poller_handle => RuntimeEvent::TelegramPoller(log_service_result("telegram-poller", service_task_result("telegram-poller", result))),
```

Add a `TelegramPoller(anyhow::Result<()>)` variant to `RuntimeEvent`. In each non-shutdown branch that currently waits for scheduler, worker, notifier, and realtime, also call:

```rust
wait_for_service_shutdown("telegram-poller", telegram_poller_handle).await
```

For the `RuntimeEvent::TelegramPoller(result)` match arm, preserve `result` as the primary result and wait for server, scheduler, worker, notifier, and realtime as secondary shutdown work.

- [ ] **Step 6: Run tests**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml telegram_update_poller_uses_binding_processor_and_offsets api_server_starts_telegram_update_poller all_in_one_starts_telegram_update_poller -- --nocapture
cargo test --locked --manifest-path backend/Cargo.toml -p notifier --no-run
cargo test --locked --manifest-path backend/Cargo.toml -p api-server --no-run
cargo test --locked --manifest-path backend/Cargo.toml -p all-in-one --no-run
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add backend/crates/notifier/src/lib.rs backend/crates/storage/src/notifications.rs backend/crates/api-server/src/main.rs backend/crates/all-in-one/src/main.rs
git commit -m "接入TG更新轮询"
```

---

### Task 7: Frontend API contracts and reusable binding panel

**Files:**
- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/api/client.ts`
- Create: `frontend/src/components/TelegramBindingPanel.tsx`
- Modify: `frontend/src/ui-regression.test.ts`

- [ ] **Step 1: Write failing frontend regression tests**

In `frontend/src/ui-regression.test.ts`, add a test near the notification API test:

```ts
test('telegram binding API and panel are exposed to frontend', () => {
  const types = readSource('api/types.ts');
  const client = readSource('api/client.ts');
  const panel = readSource('components/TelegramBindingPanel.tsx');

  for (const expected of [
    'export type TelegramBindingRequest',
    'export type CreateTelegramBindingRequest',
    'createTelegramBinding',
    'getTelegramBinding',
    'cancelTelegramBinding',
    'TelegramBindingPanel',
    '生成绑定码',
    '/start',
    '群聊',
    'short_code',
    'deep_link_url',
  ]) {
    expectContains(`${types}\n${client}\n${panel}`, expected);
  }
});
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: FAIL because `TelegramBindingPanel.tsx` and binding API contracts do not exist.

- [ ] **Step 3: Add frontend types**

In `frontend/src/api/types.ts`, add:

```ts
export type TelegramBindingRequest = {
  id: string;
  tenant_id: string;
  telegram_bot_id: string;
  status: 'pending' | 'bound' | 'expired' | 'cancelled' | string;
  bind_token: string;
  short_code: string;
  deep_link_url?: string | null;
  chat_id?: string | null;
  chat_type?: string | null;
  chat_title?: string | null;
  chat_username?: string | null;
  confirmation_error?: string | null;
  expires_at: string;
  bound_at?: string | null;
  created_at: string;
  updated_at: string;
};

export type CreateTelegramBindingRequest = {
  telegram_bot_id: string;
};
```

- [ ] **Step 4: Add API client functions**

In `frontend/src/api/client.ts`, import the new types and add:

```ts
export function createTelegramBinding(payload: CreateTelegramBindingRequest): Promise<TelegramBindingRequest> {
  return request<TelegramBindingRequest>('/api/telegram-bindings', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export function getTelegramBinding(id: string): Promise<TelegramBindingRequest> {
  return request<TelegramBindingRequest>(`/api/telegram-bindings/${id}`);
}

export function cancelTelegramBinding(id: string): Promise<TelegramBindingRequest> {
  return request<TelegramBindingRequest>(`/api/telegram-bindings/${id}/cancel`, {
    method: 'POST',
  });
}
```

- [ ] **Step 5: Create reusable panel**

Create `frontend/src/components/TelegramBindingPanel.tsx`:

```tsx
import { useEffect, useMemo, useState } from 'react';
import { useMutation, useQuery } from '@tanstack/react-query';
import { Banner, Button, Space, Tag, Toast, Typography } from '@douyinfe/semi-ui';
import { cancelTelegramBinding, createTelegramBinding, getTelegramBinding } from '../api/client';
import type { TelegramBindingRequest } from '../api/types';

type TelegramBindingPanelProps = {
  telegramBotId?: string;
  onBound: (binding: TelegramBindingRequest) => void;
};

function isPending(binding?: TelegramBindingRequest) {
  return binding?.status === 'pending';
}

function chatAlias(binding: TelegramBindingRequest) {
  return binding.chat_title || binding.chat_username || binding.chat_id || 'Telegram 会话';
}

export function TelegramBindingPanel({ telegramBotId, onBound }: TelegramBindingPanelProps) {
  const [bindingId, setBindingId] = useState<string | null>(null);
  const createMutation = useMutation({
    mutationFn: () => createTelegramBinding({ telegram_bot_id: telegramBotId ?? '' }),
    onSuccess: binding => setBindingId(binding.id),
    onError: error => Toast.error(error instanceof Error ? error.message : '生成绑定码失败'),
  });
  const cancelMutation = useMutation({
    mutationFn: cancelTelegramBinding,
    onError: error => Toast.error(error instanceof Error ? error.message : '取消绑定失败'),
  });
  const bindingQuery = useQuery({
    queryKey: ['telegram-binding', bindingId],
    queryFn: () => getTelegramBinding(bindingId ?? ''),
    enabled: Boolean(bindingId),
    refetchInterval: query => isPending(query.state.data) ? 2000 : false,
  });
  const binding = bindingQuery.data;
  const command = useMemo(() => binding ? `/start ${binding.bind_token}` : '', [binding]);

  useEffect(() => {
    if (binding?.status === 'bound' && binding.chat_id) {
      onBound(binding);
    }
  }, [binding, onBound]);

  return (
    <div className="telegram-binding-panel">
      <Banner type="info" title="通过验证码绑定 Telegram 会话" description="Chat ID 只能由 Telegram 消息校验获得，不再手动填写。" />
      <Space vertical align="start" style={{ width: '100%' }}>
        <Button
          htmlType="button"
          type="primary"
          disabled={!telegramBotId}
          loading={createMutation.isPending}
          onClick={() => createMutation.mutate()}
        >
          生成绑定码
        </Button>
        {binding ? (
          <div className="telegram-binding-instructions">
            <Typography.Text strong>个人通知</Typography.Text>
            <div>{binding.deep_link_url ? <a href={binding.deep_link_url} target="_blank" rel="noreferrer">打开 Telegram 私聊绑定</a> : command}</div>
            <Typography.Text code>{command}</Typography.Text>
            <Typography.Text strong>群聊通知</Typography.Text>
            <div>把机器人加入群聊，然后在群聊发送短码：</div>
            <Typography.Text code>{binding.short_code}</Typography.Text>
            <div>状态：<Tag>{binding.status}</Tag></div>
            {binding.status === 'bound' ? <Banner type="success" title="Telegram 会话已绑定" description={`${chatAlias(binding)} / ${binding.chat_id}`} /> : null}
            {binding.status === 'pending' ? <Button htmlType="button" onClick={() => cancelMutation.mutate(binding.id)}>取消绑定</Button> : null}
          </div>
        ) : null}
      </Space>
    </div>
  );
}
```

- [ ] **Step 6: Run frontend regression**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: PASS for the new API/panel regression.

- [ ] **Step 7: Run frontend build**

Run:

```bash
npm --prefix frontend run build
```

Expected: PASS. Existing Vite warnings about `lottie-web` eval or large chunks are acceptable if exit code is 0.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/api/types.ts frontend/src/api/client.ts frontend/src/components/TelegramBindingPanel.tsx frontend/src/ui-regression.test.ts
git commit -m "添加TG绑定前端组件"
```

---

### Task 8: Replace manual Chat ID fields in channel forms

**Files:**
- Modify: `frontend/src/pages/NotificationChannelsPage.tsx`
- Modify: `frontend/src/pages/NotificationRulesPage.tsx`
- Modify: `frontend/src/ui-regression.test.ts`

- [ ] **Step 1: Write failing UI regression tests**

Update the existing `notification channel management page and rule quick actions exist` test in `frontend/src/ui-regression.test.ts`. Add these assertions:

```ts
expectContains(page, 'TelegramBindingPanel');
expectContains(rules, 'TelegramBindingPanel');
expectContains(page, 'handleTelegramBound');
expectContains(rules, 'handleQuickTelegramBound');
expectNotContains(page, 'label="Chat ID"');
expectNotContains(rules, 'label="Chat ID"');
```

- [ ] **Step 2: Run regression to verify it fails**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: FAIL because pages still contain manual Chat ID fields and do not use `TelegramBindingPanel`.

- [ ] **Step 3: Update `NotificationChannelsPage`**

Import:

```ts
import { TelegramBindingPanel } from '../components/TelegramBindingPanel';
import type { TelegramBindingRequest } from '../api/types';
```

Add form API state:

```ts
import type { FormApi } from '@douyinfe/semi-ui/lib/es/form/interface';

type ChannelFormApi = FormApi<ChannelForm>;
const [channelFormApi, setChannelFormApi] = useState<ChannelFormApi | null>(null);
```

Add handler inside component:

```ts
function handleTelegramBound(binding: TelegramBindingRequest) {
  const alias = binding.chat_title || binding.chat_username || binding.chat_id || '';
  channelFormApi?.setValue('chat_id', binding.chat_id ?? '');
  channelFormApi?.setValue('chat_alias', alias);
}
```

On `<Form<ChannelForm>>`, add:

```tsx
getFormApi={setChannelFormApi}
```

Replace manual Chat ID input block:

```tsx
<Form.Input field="chat_id" label="Chat ID" rules={[{ required: true, message: '请输入 Chat ID' }]} />
<Form.Input field="chat_alias" label="会话别名" />
```

with:

```tsx
<TelegramBindingPanel telegramBotId={String(formState.values?.telegram_bot_id ?? '')} onBound={handleTelegramBound} />
<Form.Input field="chat_alias" label="已绑定会话" disabled rules={[{ required: true, message: '请先完成 Telegram 会话绑定' }]} />
<Form.Input field="chat_id" noLabel style={{ display: 'none' }} rules={[{ required: true, message: '请先完成 Telegram 会话绑定' }]} />
```

Keep `message_template` unchanged.

- [ ] **Step 4: Update `NotificationRulesPage` quick-create**

Import the same panel and types. Add quick form API state:

```ts
import type { TelegramBindingRequest } from '../api/types';

type QuickChannelFormApi = FormApi<Record<string, unknown>>;
const [quickChannelFormApi, setQuickChannelFormApi] = useState<QuickChannelFormApi | null>(null);

function handleQuickTelegramBound(binding: TelegramBindingRequest) {
  const alias = binding.chat_title || binding.chat_username || binding.chat_id || '';
  quickChannelFormApi?.setValue('chat_id', binding.chat_id ?? '');
  quickChannelFormApi?.setValue('chat_alias', alias);
}
```

On the quick-create `<Form>`, add:

```tsx
getFormApi={setQuickChannelFormApi}
```

Replace manual Chat ID and alias inputs with:

```tsx
<TelegramBindingPanel telegramBotId={String(formState.values?.telegram_bot_id ?? '')} onBound={handleQuickTelegramBound} />
<Form.Input field="chat_alias" label="已绑定会话" disabled rules={[{ required: true, message: '请先完成 Telegram 会话绑定' }]} />
<Form.Input field="chat_id" noLabel style={{ display: 'none' }} rules={[{ required: true, message: '请先完成 Telegram 会话绑定' }]} />
```

Because this form now needs `formState`, wrap fields with render props:

```tsx
<Form onSubmit={values => quickChannelMutation.mutate(values)} labelPosition="left" labelWidth={120} getFormApi={setQuickChannelFormApi}>
  {({ formState }) => (
    <>
      ...fields...
    </>
  )}
</Form>
```

- [ ] **Step 5: Run UI regression**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: PASS.

- [ ] **Step 6: Run frontend build**

Run:

```bash
npm --prefix frontend run build
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/pages/NotificationChannelsPage.tsx frontend/src/pages/NotificationRulesPage.tsx frontend/src/ui-regression.test.ts
git commit -m "替换TG渠道Chat ID手填流程"
```

---

### Task 9: End-to-end verification and cleanup

**Files:**
- Verify all modified files.

- [ ] **Step 1: Run backend tests**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 2: Run frontend regression**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: PASS.

- [ ] **Step 3: Run frontend build**

Run:

```bash
npm --prefix frontend run build
```

Expected: PASS. Existing Vite warnings are acceptable only if exit code is 0.

- [ ] **Step 4: Review git diff for scope**

Run:

```bash
git diff --stat
```

Expected: only files listed in this plan changed, except for generated lockfile changes if dependency versions were intentionally updated. This plan should not require dependency updates.

- [ ] **Step 5: Commit verification-only cleanup if required**

If verification changes any files, inspect them with `git diff --stat` and stage only the changed files from this plan. Use this commit command with the concrete file paths that changed:

```bash
git add backend/crates/core/src/models.rs backend/crates/storage/src/telegram_bindings.rs backend/crates/notifier/src/lib.rs backend/crates/notifier/src/external.rs backend/crates/notifier/src/telegram_updates.rs backend/crates/api-server/src/routes.rs backend/crates/api-server/src/main.rs backend/crates/all-in-one/src/main.rs frontend/src/api/types.ts frontend/src/api/client.ts frontend/src/components/TelegramBindingPanel.tsx frontend/src/pages/NotificationChannelsPage.tsx frontend/src/pages/NotificationRulesPage.tsx frontend/src/ui-regression.test.ts
git commit -m "完善TG渠道绑定验证"
```

Do not create this commit if `git diff --stat` is empty.

---

## Self-review checklist

- Spec coverage: backend binding requests, webhook, polling, shared update processor, frontend binding panel, manual Chat ID removal, and confirmation message are covered.
- No placeholders: every task contains concrete files, code, commands, and expected outcomes.
- Type consistency: frontend `TelegramBindingRequest` mirrors Rust `TelegramBindingRequest`; saved channel config remains `telegram_bot_id`, `chat_id`, `chat_alias`, `message_template`.
