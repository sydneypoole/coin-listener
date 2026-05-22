# Telegram Proxy, Light Theme, and RPC Range Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add configurable Telegram proxy support, fix light-mode readability, and prevent EVM `eth_getLogs` requests from exceeding free-tier block range limits.

**Architecture:** Store a tenant-level Telegram proxy and optional bot-level override, resolve the final proxy at every Telegram call site, and route Telegram HTTP through a proxy-aware client resolver. Fix light mode by moving main content surfaces to app theme tokens while keeping the dark brand sidebar. Split EVM ERC20 log scans into bounded block chunks before making RPC calls.

**Tech Stack:** Rust, Axum, SQLx/PostgreSQL migrations, reqwest, Tokio, React, TypeScript, TanStack Query, Semi Design, TailwindCSS, Vite.

---

## File Structure

- Create `backend/crates/storage/migrations/0017_telegram_proxy_settings.sql` — adds `telegram_settings.proxy_url` and `telegram_bots.proxy_url`.
- Create `backend/crates/core/src/proxy.rs` — shared proxy URL normalization, masking, and source selection helpers.
- Modify `backend/crates/core/src/lib.rs` — exports the proxy helper module.
- Modify `backend/crates/core/src/models.rs` — adds public settings models, proxy fields, and update request types.
- Create `backend/crates/storage/src/telegram_settings.rs` — tenant-level Telegram proxy get/update repository.
- Modify `backend/crates/storage/src/lib.rs` — exports `telegram_settings`.
- Modify `backend/crates/storage/src/notifications.rs` — persists bot-level proxy, computes public proxy previews, and returns effective proxy for secrets.
- Modify `backend/crates/notifier/Cargo.toml` and `backend/Cargo.toml` — enables reqwest SOCKS proxy support.
- Modify `backend/crates/notifier/src/external.rs` — adds proxy-aware Telegram client resolver and proxy parameters on Telegram methods.
- Modify `backend/crates/notifier/src/lib.rs` — passes final proxy URL to notification sends, binding confirmations, and `getUpdates` polling.
- Modify `backend/crates/api-server/src/routes.rs` — adds Telegram settings API and passes proxy URLs through verification/webhook/test paths.
- Modify `backend/crates/api-server/src/main.rs` and `backend/crates/all-in-one/src/main.rs` — keep runtime sender construction compatible with the resolver.
- Modify `backend/crates/worker/src/lib.rs` — chunks EVM ERC20 log ranges and advances cursors per successful chunk.
- Modify `frontend/src/api/types.ts` — adds Telegram settings and proxy fields.
- Modify `frontend/src/api/client.ts` — adds Telegram settings API functions.
- Modify `frontend/src/pages/TelegramBotsPage.tsx` — adds global proxy card, bot proxy form mode, and proxy source column.
- Modify `frontend/src/styles.css` — adds app-level light/dark tokens and replaces unreadable light-mode hard-coded colors.
- Modify `frontend/src/ui-regression.test.ts` — locks proxy UI/API and light-mode token usage.

---

### Task 1: Core proxy helpers and migration

**Files:**
- Create: `backend/crates/storage/migrations/0017_telegram_proxy_settings.sql`
- Create: `backend/crates/core/src/proxy.rs`
- Modify: `backend/crates/core/src/lib.rs`
- Modify: `backend/crates/core/src/models.rs:397-440`
- Test: `backend/crates/core/src/proxy.rs`

- [ ] **Step 1: Write the failing proxy helper tests**

Create `backend/crates/core/src/proxy.rs` with this test-first content, then add `pub mod proxy;` to `backend/crates/core/src/lib.rs` so Cargo compiles the new module:

```rust
use crate::{AppError, AppResult};

pub const TELEGRAM_PROXY_SOURCE_BOT: &str = "bot";
pub const TELEGRAM_PROXY_SOURCE_GLOBAL: &str = "global";
pub const TELEGRAM_PROXY_SOURCE_DIRECT: &str = "direct";

pub fn normalize_proxy_url(_value: Option<&str>) -> AppResult<Option<String>> {
    Err(AppError::Validation("proxy helper not implemented".to_string()))
}

pub fn mask_proxy_url(_url: &str) -> String {
    "proxy helper not implemented".to_string()
}

pub fn telegram_proxy_source(_bot_proxy_url: Option<&str>, _global_proxy_url: Option<&str>) -> &'static str {
    TELEGRAM_PROXY_SOURCE_DIRECT
}

#[cfg(test)]
mod tests {
    use super::{mask_proxy_url, normalize_proxy_url, telegram_proxy_source};

    #[test]
    fn normalize_proxy_url_accepts_supported_schemes() {
        assert_eq!(
            normalize_proxy_url(Some(" http://user:pass@127.0.0.1:7890 ")).unwrap(),
            Some("http://user:pass@127.0.0.1:7890".to_string())
        );
        assert_eq!(
            normalize_proxy_url(Some("https://proxy.example.com:443")).unwrap(),
            Some("https://proxy.example.com:443".to_string())
        );
        assert_eq!(
            normalize_proxy_url(Some("socks5://127.0.0.1:1080")).unwrap(),
            Some("socks5://127.0.0.1:1080".to_string())
        );
        assert_eq!(normalize_proxy_url(Some("   ")).unwrap(), None);
        assert_eq!(normalize_proxy_url(None).unwrap(), None);
    }

    #[test]
    fn normalize_proxy_url_rejects_unsupported_or_incomplete_urls() {
        for value in [
            "ftp://proxy.example.com:21",
            "http:///missing-host",
            "https://",
            "socks5:// user:pass@host:1080",
            "proxy.example.com:7890",
        ] {
            let error = normalize_proxy_url(Some(value)).unwrap_err().to_string();
            assert!(error.contains("proxy_url must use http, https, or socks5"));
        }
    }

    #[test]
    fn mask_proxy_url_redacts_credentials() {
        assert_eq!(
            mask_proxy_url("http://alice:secret@proxy.example.com:7890"),
            "http://alice:***@proxy.example.com:7890"
        );
        assert_eq!(
            mask_proxy_url("socks5://proxy.example.com:1080"),
            "socks5://proxy.example.com:1080"
        );
    }

    #[test]
    fn telegram_proxy_source_prefers_bot_then_global_then_direct() {
        assert_eq!(telegram_proxy_source(Some("http://bot:7890"), Some("http://global:7890")), "bot");
        assert_eq!(telegram_proxy_source(None, Some("http://global:7890")), "global");
        assert_eq!(telegram_proxy_source(None, None), "direct");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml -p coin-listener-core proxy::tests -- --nocapture
```

Expected: FAIL because `normalize_proxy_url_accepts_supported_schemes` returns `proxy helper not implemented`.

- [ ] **Step 3: Implement proxy helpers**

Replace the non-test functions in `backend/crates/core/src/proxy.rs` with this implementation and keep the tests from Step 1:

```rust
use crate::{AppError, AppResult};

pub const TELEGRAM_PROXY_SOURCE_BOT: &str = "bot";
pub const TELEGRAM_PROXY_SOURCE_GLOBAL: &str = "global";
pub const TELEGRAM_PROXY_SOURCE_DIRECT: &str = "direct";

pub fn normalize_proxy_url(value: Option<&str>) -> AppResult<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if !valid_proxy_url(value) {
        return Err(AppError::Validation(
            "proxy_url must use http, https, or socks5 with a host".to_string(),
        ));
    }
    Ok(Some(value.to_string()))
}

pub fn mask_proxy_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let path = &rest[authority_end..];
    let Some((userinfo, host)) = authority.rsplit_once('@') else {
        return url.to_string();
    };
    let username = userinfo.split(':').next().unwrap_or("");
    if username.is_empty() || host.is_empty() {
        return url.to_string();
    }
    format!("{scheme}://{username}:***@{host}{path}")
}

pub fn telegram_proxy_source(
    bot_proxy_url: Option<&str>,
    global_proxy_url: Option<&str>,
) -> &'static str {
    if bot_proxy_url.is_some_and(|value| !value.trim().is_empty()) {
        TELEGRAM_PROXY_SOURCE_BOT
    } else if global_proxy_url.is_some_and(|value| !value.trim().is_empty()) {
        TELEGRAM_PROXY_SOURCE_GLOBAL
    } else {
        TELEGRAM_PROXY_SOURCE_DIRECT
    }
}

fn valid_proxy_url(value: &str) -> bool {
    if value.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((scheme, rest)) = value.split_once("://") else {
        return false;
    };
    if !matches!(scheme, "http" | "https" | "socks5") {
        return false;
    }
    if rest.is_empty() || rest.starts_with('/') {
        return false;
    }
    let authority = rest.split('/').next().unwrap_or(rest);
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    !host_port.is_empty() && !host_port.starts_with(':')
}
```

- [ ] **Step 4: Export the proxy module**

Modify `backend/crates/core/src/lib.rs` to include the proxy module:

```rust
pub mod config;
pub mod error;
pub mod models;
pub mod proxy;

pub use config::{
    AppConfig, AuthConfig, NotifyConfig, PostgresConfig, RedisConfig, ScanConfig, ServerConfig,
};
pub use error::{AppError, AppResult};
```

- [ ] **Step 5: Add models for proxy settings and bot proxy fields**

Modify the Telegram bot section in `backend/crates/core/src/models.rs` so the structs become:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct TelegramBot {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub username: Option<String>,
    pub token_preview: String,
    pub proxy_source: String,
    pub proxy_url_preview: Option<String>,
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
    pub username: Option<String>,
    pub bot_token: String,
    pub token_preview: String,
    pub bot_proxy_url: Option<String>,
    pub effective_proxy_url: Option<String>,
    pub status: String,
    pub verification_status: String,
    pub last_verified_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct TelegramSettings {
    pub tenant_id: Uuid,
    pub proxy_url_preview: Option<String>,
    pub has_proxy: bool,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateTelegramSettingsRequest {
    #[serde(default)]
    pub proxy_url: Option<Option<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateTelegramBotRequest {
    pub name: String,
    pub bot_token: String,
    pub status: Option<String>,
    pub proxy_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateTelegramBotRequest {
    pub name: String,
    pub bot_token: Option<String>,
    pub status: String,
    #[serde(default)]
    pub proxy_url: Option<Option<String>>,
}
```

- [ ] **Step 6: Update request deserialization tests**

In the existing tests in `backend/crates/core/src/models.rs`, update `telegram_bot_create_request_deserializes_token_without_serializing_secret` to assert proxy fields:

```rust
#[test]
fn telegram_bot_create_request_deserializes_token_without_serializing_secret() {
    let payload = r#"{
        "name":"Ops bot",
        "bot_token":"123456:secret-token",
        "status":"active",
        "proxy_url":"socks5://127.0.0.1:1080"
    }"#;

    let request: CreateTelegramBotRequest = serde_json::from_str(payload).unwrap();

    assert_eq!(request.name, "Ops bot");
    assert_eq!(request.bot_token, "123456:secret-token");
    assert_eq!(request.status.as_deref(), Some("active"));
    assert_eq!(request.proxy_url.as_deref(), Some("socks5://127.0.0.1:1080"));
}

#[test]
fn update_telegram_bot_request_distinguishes_proxy_omitted_null_and_value() {
    let omitted: UpdateTelegramBotRequest = serde_json::from_str(
        r#"{"name":"Ops bot","status":"active"}"#,
    )
    .unwrap();
    assert_eq!(omitted.proxy_url, None);

    let cleared: UpdateTelegramBotRequest = serde_json::from_str(
        r#"{"name":"Ops bot","status":"active","proxy_url":null}"#,
    )
    .unwrap();
    assert_eq!(cleared.proxy_url, Some(None));

    let updated: UpdateTelegramBotRequest = serde_json::from_str(
        r#"{"name":"Ops bot","status":"active","proxy_url":"http://proxy:7890"}"#,
    )
    .unwrap();
    assert_eq!(updated.proxy_url, Some(Some("http://proxy:7890".to_string())));
}
```

- [ ] **Step 7: Add migration**

Create `backend/crates/storage/migrations/0017_telegram_proxy_settings.sql`:

```sql
ALTER TABLE telegram_bots
    ADD COLUMN IF NOT EXISTS proxy_url TEXT;

CREATE TABLE IF NOT EXISTS telegram_settings (
    tenant_id UUID PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    proxy_url TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT telegram_settings_proxy_url_not_blank CHECK (
        proxy_url IS NULL OR btrim(proxy_url) <> ''
    )
);
```

- [ ] **Step 8: Run tests to verify this task passes**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml -p coin-listener-core -- --nocapture
```

Expected: PASS for proxy helper tests and Telegram bot request deserialization tests.

- [ ] **Step 9: Commit**

```bash
git add backend/crates/core/src/lib.rs backend/crates/core/src/proxy.rs backend/crates/core/src/models.rs backend/crates/storage/migrations/0017_telegram_proxy_settings.sql
git commit -m "$(cat <<'EOF'
添加Telegram代理基础模型
EOF
)"
```

---

### Task 2: Telegram settings repository and API

**Files:**
- Create: `backend/crates/storage/src/telegram_settings.rs`
- Modify: `backend/crates/storage/src/lib.rs`
- Modify: `backend/crates/api-server/src/routes.rs:15-24,40-50,70-110,486-523`
- Test: `backend/crates/storage/src/telegram_settings.rs`, `backend/crates/api-server/src/routes.rs`

- [ ] **Step 1: Write failing storage tests**

Create `backend/crates/storage/src/telegram_settings.rs` with this test-first content, then add `pub mod telegram_settings;` to `backend/crates/storage/src/lib.rs` so Cargo compiles the new module:

```rust
use coin_listener_core::{models::TelegramSettings, AppResult};
use sqlx::PgPool;
use uuid::Uuid;

pub async fn get_telegram_settings(_pool: &PgPool, tenant_id: Uuid) -> AppResult<TelegramSettings> {
    Ok(TelegramSettings {
        tenant_id,
        proxy_url_preview: None,
        has_proxy: false,
        created_at: None,
        updated_at: None,
    })
}

#[cfg(test)]
mod tests {
    use super::{GET_TELEGRAM_SETTINGS_QUERY, UPSERT_TELEGRAM_SETTINGS_QUERY};

    #[test]
    fn telegram_settings_queries_are_tenant_scoped() {
        assert!(GET_TELEGRAM_SETTINGS_QUERY.contains("WHERE tenant_id = $1"));
        assert!(UPSERT_TELEGRAM_SETTINGS_QUERY.contains("ON CONFLICT (tenant_id)"));
        assert!(UPSERT_TELEGRAM_SETTINGS_QUERY.contains("proxy_url = EXCLUDED.proxy_url"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml -p coin-listener-storage telegram_settings -- --nocapture
```

Expected: FAIL because `GET_TELEGRAM_SETTINGS_QUERY` and `UPSERT_TELEGRAM_SETTINGS_QUERY` are missing.

- [ ] **Step 3: Implement the repository**

Replace `backend/crates/storage/src/telegram_settings.rs` with:

```rust
use coin_listener_core::{
    models::{TelegramSettings, UpdateTelegramSettingsRequest},
    proxy::{mask_proxy_url, normalize_proxy_url},
    AppError, AppResult,
};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

pub const GET_TELEGRAM_SETTINGS_QUERY: &str = r#"
    SELECT tenant_id, proxy_url, created_at, updated_at
    FROM telegram_settings
    WHERE tenant_id = $1
    "#;

pub const UPSERT_TELEGRAM_SETTINGS_QUERY: &str = r#"
    INSERT INTO telegram_settings (tenant_id, proxy_url)
    VALUES ($1, $2)
    ON CONFLICT (tenant_id)
    DO UPDATE SET proxy_url = EXCLUDED.proxy_url,
                  updated_at = NOW()
    RETURNING tenant_id, proxy_url, created_at, updated_at
    "#;

#[derive(Debug, Clone, sqlx::FromRow)]
struct TelegramSettingsRow {
    tenant_id: Uuid,
    proxy_url: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TelegramSettingsRow {
    fn into_public(self) -> TelegramSettings {
        TelegramSettings {
            tenant_id: self.tenant_id,
            proxy_url_preview: self.proxy_url.as_deref().map(mask_proxy_url),
            has_proxy: self.proxy_url.is_some(),
            created_at: Some(self.created_at),
            updated_at: Some(self.updated_at),
        }
    }
}

pub async fn get_telegram_settings(pool: &PgPool, tenant_id: Uuid) -> AppResult<TelegramSettings> {
    let row = sqlx::query_as::<_, TelegramSettingsRow>(GET_TELEGRAM_SETTINGS_QUERY)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    Ok(match row {
        Some(row) => row.into_public(),
        None => TelegramSettings {
            tenant_id,
            proxy_url_preview: None,
            has_proxy: false,
            created_at: None,
            updated_at: None,
        },
    })
}

pub async fn get_telegram_proxy_url(pool: &PgPool, tenant_id: Uuid) -> AppResult<Option<String>> {
    let row = sqlx::query_scalar::<_, Option<String>>(
        "SELECT proxy_url FROM telegram_settings WHERE tenant_id = $1",
    )
    .bind(tenant_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?;

    Ok(row.flatten())
}

pub async fn update_telegram_settings(
    pool: &PgPool,
    tenant_id: Uuid,
    request: UpdateTelegramSettingsRequest,
) -> AppResult<TelegramSettings> {
    let proxy_url = match request.proxy_url {
        Some(value) => normalize_proxy_url(value.as_deref())?,
        None => get_telegram_proxy_url(pool, tenant_id).await?,
    };

    sqlx::query_as::<_, TelegramSettingsRow>(UPSERT_TELEGRAM_SETTINGS_QUERY)
        .bind(tenant_id)
        .bind(proxy_url)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
        .map(TelegramSettingsRow::into_public)
}

#[cfg(test)]
mod tests {
    use super::{GET_TELEGRAM_SETTINGS_QUERY, UPSERT_TELEGRAM_SETTINGS_QUERY};

    #[test]
    fn telegram_settings_queries_are_tenant_scoped() {
        assert!(GET_TELEGRAM_SETTINGS_QUERY.contains("WHERE tenant_id = $1"));
        assert!(UPSERT_TELEGRAM_SETTINGS_QUERY.contains("ON CONFLICT (tenant_id)"));
        assert!(UPSERT_TELEGRAM_SETTINGS_QUERY.contains("proxy_url = EXCLUDED.proxy_url"));
    }
}
```

- [ ] **Step 4: Export storage module**

Modify `backend/crates/storage/src/lib.rs`:

```rust
pub mod address_imports;
pub mod notifications;
pub mod notify_queue;
pub mod postgres;
pub mod provider_health;
pub mod redis;
pub mod repositories;
pub mod scan_queue;
pub mod service_heartbeats;
pub mod system_status;
pub mod telegram_bindings;
pub mod telegram_settings;

pub use postgres::{connect_postgres, run_migrations};
pub use redis::connect_redis;
```

- [ ] **Step 5: Write failing API route source test**

In `backend/crates/api-server/src/routes.rs` test module, add:

```rust
#[test]
fn telegram_settings_routes_are_protected() {
    let source = production_source();
    let settings_index = source
        .find("/api/telegram-settings")
        .expect("telegram settings route exists");
    let auth_layer_index = source
        .find("route_layer(middleware::from_fn_with_state")
        .expect("protected auth layer exists");

    assert!(settings_index < auth_layer_index);
    assert!(source.contains("get(get_telegram_settings).put(update_telegram_settings)"));
}
```

- [ ] **Step 6: Run test to verify it fails**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml -p api-server telegram_settings_routes_are_protected -- --nocapture
```

Expected: FAIL because `/api/telegram-settings` is not routed.

- [ ] **Step 7: Add API imports and routes**

Modify the imports in `backend/crates/api-server/src/routes.rs` so the model import list includes `UpdateTelegramSettingsRequest`:

```rust
UpdateNotificationChannelRequest, UpdateTelegramBotRequest, UpdateTelegramSettingsRequest,
UserSummary, VerificationResponse,
```

Modify the protected router near the Telegram bot routes:

```rust
.route(
    "/api/telegram-settings",
    get(get_telegram_settings).put(update_telegram_settings),
)
.route(
    "/api/telegram-bots",
    get(list_telegram_bots).post(create_telegram_bot),
)
```

Add handlers before `list_telegram_bots`:

```rust
async fn get_telegram_settings(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Response, ApiError> {
    let settings = coin_listener_storage::telegram_settings::get_telegram_settings(
        &state.postgres,
        auth.tenant_id,
    )
    .await?;
    Ok(Json(settings).into_response())
}

async fn update_telegram_settings(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<UpdateTelegramSettingsRequest>,
) -> Result<Response, ApiError> {
    let settings = coin_listener_storage::telegram_settings::update_telegram_settings(
        &state.postgres,
        auth.tenant_id,
        request,
    )
    .await?;
    Ok(Json(settings).into_response())
}
```

- [ ] **Step 8: Run tests to verify this task passes**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml -p coin-listener-storage telegram_settings -- --nocapture
cargo test --locked --manifest-path backend/Cargo.toml -p api-server telegram_settings_routes_are_protected -- --nocapture
```

Expected: both commands PASS.

- [ ] **Step 9: Commit**

```bash
git add backend/crates/storage/src/telegram_settings.rs backend/crates/storage/src/lib.rs backend/crates/api-server/src/routes.rs
git commit -m "$(cat <<'EOF'
添加Telegram全局代理设置接口
EOF
)"
```

---

### Task 3: Telegram bot proxy storage and API contract

**Files:**
- Modify: `backend/crates/storage/src/notifications.rs:63-150,666-808`
- Test: `backend/crates/storage/src/notifications.rs`

- [ ] **Step 1: Write failing storage query tests**

In `backend/crates/storage/src/notifications.rs` test module, add the Telegram bot query constants to the existing `use super::{...}` list, then add:

```rust
#[test]
fn telegram_bot_queries_include_proxy_columns_and_global_join() {
    assert!(LIST_TELEGRAM_BOTS_QUERY.contains("LEFT JOIN telegram_settings"));
    assert!(LIST_TELEGRAM_BOTS_QUERY.contains("proxy_url_preview"));
    assert!(LIST_TELEGRAM_BOTS_QUERY.contains("proxy_source"));
    assert!(GET_TELEGRAM_BOT_SECRET_QUERY.contains("effective_proxy_url"));
    assert!(GET_TELEGRAM_BOT_SECRET_BY_ID_ANY_TENANT_QUERY.contains("effective_proxy_url"));
    assert!(LIST_ACTIVE_VERIFIED_TELEGRAM_BOT_SECRETS_QUERY.contains("effective_proxy_url"));
    assert!(CREATE_TELEGRAM_BOT_QUERY.contains("proxy_url"));
    assert!(UPDATE_TELEGRAM_BOT_QUERY.contains("CASE WHEN $7::boolean"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml -p coin-listener-storage telegram_bot_queries_include_proxy_columns_and_global_join -- --nocapture
```

Expected: FAIL because the queries do not include proxy fields.

- [ ] **Step 3: Update Telegram bot SQL queries**

In `backend/crates/storage/src/notifications.rs`, replace the Telegram bot query constants with these versions:

```rust
const LIST_TELEGRAM_BOTS_QUERY: &str = r#"
        SELECT b.id, b.tenant_id, b.name, b.username, b.token_preview,
               CASE
                   WHEN b.proxy_url IS NOT NULL THEN 'bot'
                   WHEN s.proxy_url IS NOT NULL THEN 'global'
                   ELSE 'direct'
               END AS proxy_source,
               CASE
                   WHEN b.proxy_url IS NOT NULL THEN b.proxy_url
                   WHEN s.proxy_url IS NOT NULL THEN s.proxy_url
                   ELSE NULL
               END AS proxy_url_preview,
               b.status, b.verification_status,
               b.last_verified_at, b.last_error, b.created_at, b.updated_at
        FROM telegram_bots b
        LEFT JOIN telegram_settings s ON s.tenant_id = b.tenant_id
        WHERE b.tenant_id = $1
        ORDER BY b.created_at DESC
        "#;

const CREATE_TELEGRAM_BOT_QUERY: &str = r#"
        INSERT INTO telegram_bots (tenant_id, name, bot_token, token_preview, status, proxy_url)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, tenant_id, name, username, token_preview,
                  CASE WHEN proxy_url IS NULL THEN 'direct' ELSE 'bot' END AS proxy_source,
                  proxy_url AS proxy_url_preview,
                  status, verification_status, last_verified_at, last_error, created_at, updated_at
        "#;

const GET_TELEGRAM_BOT_SECRET_QUERY: &str = r#"
        SELECT b.id, b.tenant_id, b.name, b.username, b.bot_token, b.token_preview,
               b.proxy_url AS bot_proxy_url,
               COALESCE(b.proxy_url, s.proxy_url) AS effective_proxy_url,
               b.status, b.verification_status, b.last_verified_at, b.last_error, b.created_at, b.updated_at
        FROM telegram_bots b
        LEFT JOIN telegram_settings s ON s.tenant_id = b.tenant_id
        WHERE b.id = $1
          AND b.tenant_id = $2
        "#;

const GET_TELEGRAM_BOT_SECRET_BY_ID_ANY_TENANT_QUERY: &str = r#"
        SELECT b.id, b.tenant_id, b.name, b.username, b.bot_token, b.token_preview,
               b.proxy_url AS bot_proxy_url,
               COALESCE(b.proxy_url, s.proxy_url) AS effective_proxy_url,
               b.status, b.verification_status, b.last_verified_at, b.last_error, b.created_at, b.updated_at
        FROM telegram_bots b
        LEFT JOIN telegram_settings s ON s.tenant_id = b.tenant_id
        WHERE b.id = $1
          AND b.status = 'active'
          AND b.verification_status = 'verified'
        "#;

const LIST_ACTIVE_VERIFIED_TELEGRAM_BOT_SECRETS_QUERY: &str = r#"
        SELECT b.id, b.tenant_id, b.name, b.username, b.bot_token, b.token_preview,
               b.proxy_url AS bot_proxy_url,
               COALESCE(b.proxy_url, s.proxy_url) AS effective_proxy_url,
               b.status, b.verification_status, b.last_verified_at, b.last_error, b.created_at, b.updated_at
        FROM telegram_bots b
        LEFT JOIN telegram_settings s ON s.tenant_id = b.tenant_id
        WHERE b.status = 'active'
          AND b.verification_status = 'verified'
        ORDER BY b.created_at ASC
        "#;

const UPDATE_TELEGRAM_BOT_QUERY: &str = r#"
        UPDATE telegram_bots
        SET name = $3,
            bot_token = COALESCE($4, bot_token),
            token_preview = COALESCE($5, token_preview),
            status = $6,
            proxy_url = CASE WHEN $7::boolean THEN $8 ELSE proxy_url END,
            username = CASE
                WHEN $4::text IS NULL THEN username
                ELSE NULL
            END,
            verification_status = CASE
                WHEN $4::text IS NULL THEN verification_status
                ELSE 'unverified'
            END,
            last_verified_at = CASE
                WHEN $4::text IS NULL THEN last_verified_at
                ELSE NULL
            END,
            last_error = CASE
                WHEN $4::text IS NULL THEN last_error
                ELSE NULL
            END,
            updated_at = NOW()
        WHERE id = $1
          AND tenant_id = $2
        RETURNING id, tenant_id, name, username, token_preview,
                  CASE WHEN proxy_url IS NULL THEN 'direct' ELSE 'bot' END AS proxy_source,
                  proxy_url AS proxy_url_preview,
                  status, verification_status, last_verified_at, last_error, created_at, updated_at
        "#;
```

- [ ] **Step 4: Mask public proxy previews after query**

In `backend/crates/storage/src/notifications.rs`, add this row type near the Telegram query constants:

```rust
#[derive(Debug, Clone, sqlx::FromRow)]
struct TelegramBotRow {
    id: Uuid,
    tenant_id: Uuid,
    name: String,
    username: Option<String>,
    token_preview: String,
    proxy_source: String,
    proxy_url_preview: Option<String>,
    status: String,
    verification_status: String,
    last_verified_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TelegramBotRow {
    fn into_public(self) -> TelegramBot {
        TelegramBot {
            id: self.id,
            tenant_id: self.tenant_id,
            name: self.name,
            username: self.username,
            token_preview: self.token_preview,
            proxy_source: self.proxy_source,
            proxy_url_preview: self
                .proxy_url_preview
                .as_deref()
                .map(coin_listener_core::proxy::mask_proxy_url),
            status: self.status,
            verification_status: self.verification_status,
            last_verified_at: self.last_verified_at,
            last_error: self.last_error,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}
```

Update `list_telegram_bots`, `create_telegram_bot`, `update_telegram_bot`, and `mark_telegram_bot_verification` to query `TelegramBotRow` and call `into_public()`. For `mark_telegram_bot_verification`, update the `MARK_TELEGRAM_BOT_VERIFICATION_QUERY` return list to include `proxy_source` and `proxy_url_preview`:

```rust
RETURNING id, tenant_id, name, username, token_preview,
          CASE WHEN proxy_url IS NULL THEN 'direct' ELSE 'bot' END AS proxy_source,
          proxy_url AS proxy_url_preview,
          status, verification_status, last_verified_at, last_error, created_at, updated_at
```

- [ ] **Step 5: Validate and bind proxy URL on create/update**

Update `create_telegram_bot`:

```rust
let proxy_url = normalize_proxy_url(request.proxy_url.as_deref())?;

sqlx::query_as::<_, TelegramBotRow>(CREATE_TELEGRAM_BOT_QUERY)
    .bind(tenant_id)
    .bind(request.name)
    .bind(bot_token)
    .bind(token_preview)
    .bind(status)
    .bind(proxy_url)
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
    .map(TelegramBotRow::into_public)
```

Update `update_telegram_bot` before the SQL call:

```rust
let proxy_url_was_provided = request.proxy_url.is_some();
let proxy_url = match request.proxy_url {
    Some(value) => normalize_proxy_url(value.as_deref())?,
    None => None,
};
```

Bind the two new values after `request.status`:

```rust
.bind(request.status)
.bind(proxy_url_was_provided)
.bind(proxy_url)
```

- [ ] **Step 6: Update imports**

Add `normalize_proxy_url` to the imports in `backend/crates/storage/src/notifications.rs`:

```rust
use coin_listener_core::{
    models::{
        AddressEvent, CreateNotificationChannelRequest, CreateNotificationRuleRequest,
        CreateTelegramBotRequest, InAppNotification, InAppNotificationQuery, NotificationChannel,
        NotificationDelivery, NotificationDeliveryListItem, NotificationDeliveryQuery,
        NotificationRule, TelegramBot, TelegramBotSecret, UpdateNotificationChannelRequest,
        UpdateTelegramBotRequest,
    },
    proxy::normalize_proxy_url,
    AppError, AppResult,
};
```

- [ ] **Step 7: Run tests to verify this task passes**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml -p coin-listener-storage telegram_bot_queries_include_proxy_columns_and_global_join -- --nocapture
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add backend/crates/storage/src/notifications.rs
git commit -m "$(cat <<'EOF'
接入TG机器人代理存储
EOF
)"
```

---

### Task 4: Proxy-aware Telegram HTTP sender and runtime integration

**Files:**
- Modify: `backend/Cargo.toml:27`
- Modify: `backend/crates/notifier/src/external.rs:3-280`
- Modify: `backend/crates/notifier/src/lib.rs:101-144,620-698,906-985`
- Modify: `backend/crates/api-server/src/routes.rs:523-745`
- Modify: `backend/crates/api-server/src/main.rs:56-59,146-153`
- Modify: `backend/crates/all-in-one/src/main.rs:67-108,392-404`
- Test: `backend/crates/notifier/src/external.rs`, `backend/crates/notifier/src/lib.rs`, `backend/crates/api-server/src/routes.rs`

- [ ] **Step 1: Write failing notifier tests**

In `backend/crates/notifier/src/external.rs` tests, add:

```rust
#[test]
fn external_sender_source_contains_proxy_resolver() {
    let source = include_str!("external.rs");
    let production_source = source
        .split("#[cfg(test)]")
        .next()
        .expect("production source exists");

    assert!(production_source.contains("TelegramClientResolver"));
    assert!(production_source.contains("client_for"));
    assert!(production_source.contains("reqwest::Proxy::all"));
    assert!(production_source.contains("proxy_clients"));
}

#[test]
fn telegram_methods_accept_proxy_url() {
    let source = include_str!("external.rs");
    let production_source = source
        .split("#[cfg(test)]")
        .next()
        .expect("production source exists");

    assert!(production_source.contains("pub async fn send_telegram("));
    assert!(production_source.contains("proxy_url: Option<&str>"));
    assert!(production_source.contains("pub async fn verify_telegram_bot("));
    assert!(production_source.contains("pub async fn get_telegram_updates("));
}
```

In `backend/crates/notifier/src/lib.rs` tests, add:

```rust
#[test]
fn telegram_runtime_passes_effective_proxy_url_to_sender() {
    let source = include_str!("lib.rs");
    let production_source = source
        .split("#[cfg(test)]")
        .next()
        .expect("production source exists");

    assert!(production_source.contains("effective_proxy_url.as_deref()"));
    assert!(production_source.contains("get_telegram_proxy_url(pool, task.tenant_id)"));
    assert!(production_source.contains("process_telegram_binding_update("));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml -p notifier external_sender_source_contains_proxy_resolver -- --nocapture
cargo test --locked --manifest-path backend/Cargo.toml -p notifier telegram_methods_accept_proxy_url -- --nocapture
cargo test --locked --manifest-path backend/Cargo.toml -p notifier telegram_runtime_passes_effective_proxy_url_to_sender -- --nocapture
```

Expected: FAIL because the sender and runtime do not pass proxy URLs.

- [ ] **Step 3: Enable reqwest SOCKS proxy support**

Modify the workspace reqwest dependency in `backend/Cargo.toml`:

```toml
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "socks"] }
```

- [ ] **Step 4: Add proxy resolver to `external.rs`**

Modify imports in `backend/crates/notifier/src/external.rs`:

```rust
use reqwest::Client;
use serde::Serialize;
use serde_json::Value;
use sha2::Sha256;
use std::{collections::BTreeMap, collections::HashMap, sync::{Arc, RwLock}, time::Duration};
use uuid::Uuid;
```

Add resolver types before `ExternalNotificationSender`:

```rust
#[derive(Debug, Clone)]
pub struct TelegramClientResolver {
    direct_client: Client,
    proxy_clients: Arc<RwLock<HashMap<String, Client>>>,
}

impl TelegramClientResolver {
    pub fn new(direct_client: Client) -> Self {
        Self {
            direct_client,
            proxy_clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn client_for(&self, proxy_url: Option<&str>) -> Result<Client, ExternalConfigError> {
        let proxy_url = match coin_listener_core::proxy::normalize_proxy_url(proxy_url)
            .map_err(|error| ExternalConfigError::new(error.to_string()))?
        {
            Some(proxy_url) => proxy_url,
            None => return Ok(self.direct_client.clone()),
        };

        if let Some(client) = self
            .proxy_clients
            .read()
            .expect("telegram proxy client cache lock poisoned")
            .get(&proxy_url)
            .cloned()
        {
            return Ok(client);
        }

        let proxy = reqwest::Proxy::all(&proxy_url).map_err(|error| {
            ExternalConfigError::new(format!("telegram proxy_url is invalid: {error}"))
        })?;
        let client = Client::builder()
            .proxy(proxy)
            .build()
            .map_err(|error| ExternalConfigError::new(format!("telegram proxy client failed: {error}")))?;

        self.proxy_clients
            .write()
            .expect("telegram proxy client cache lock poisoned")
            .insert(proxy_url, client.clone());
        Ok(client)
    }
}
```

Change `ExternalNotificationSender` to store the resolver:

```rust
#[derive(Debug, Clone)]
pub struct ExternalNotificationSender {
    client: Client,
    telegram_clients: TelegramClientResolver,
    telegram_api_base_url: String,
}

impl ExternalNotificationSender {
    pub fn new(client: Client) -> Self {
        Self {
            client: client.clone(),
            telegram_clients: TelegramClientResolver::new(client),
            telegram_api_base_url: "https://api.telegram.org".to_string(),
        }
    }
```

Update the test-only constructor:

```rust
#[cfg(test)]
fn with_telegram_api_base_url(client: Client, telegram_api_base_url: String) -> Self {
    Self {
        client: client.clone(),
        telegram_clients: TelegramClientResolver::new(client),
        telegram_api_base_url,
    }
}
```

- [ ] **Step 5: Add proxy URL parameters to Telegram methods**

Change `send_telegram`, `verify_telegram_bot`, and `get_telegram_updates` signatures:

```rust
pub async fn send_telegram(
    &self,
    config: &TelegramChannelConfig,
    bot_token: &str,
    text: &str,
    proxy_url: Option<&str>,
) -> ExternalSendOutcome {
```

```rust
pub async fn verify_telegram_bot(
    &self,
    bot_token: &str,
    proxy_url: Option<&str>,
) -> ExternalSendOutcome {
```

```rust
pub async fn get_telegram_updates(
    &self,
    bot_token: &str,
    last_update_id: i64,
    proxy_url: Option<&str>,
) -> Result<Vec<crate::telegram_updates::TelegramUpdate>, ExternalSendOutcome> {
```

At the start of each method, get the client:

```rust
let client = match self.telegram_clients.client_for(proxy_url) {
    Ok(client) => client,
    Err(error) => {
        return ExternalSendOutcome::TransientFailure(ExternalSendMetadata {
            last_error: Some(error.message),
            provider_message_id: None,
            provider_status_code: None,
            provider_response: None,
        });
    }
};
```

For `get_telegram_updates`, return `Err(ExternalSendOutcome::TransientFailure(...))` for resolver errors. Replace `.client.get` and `.client.post` with `client.get` and `client.post` inside Telegram methods only. Keep webhook sending on `self.client`.

- [ ] **Step 6: Pass proxy URL in binding processor and poller**

Change `process_telegram_binding_update` signature in `backend/crates/notifier/src/lib.rs`:

```rust
pub async fn process_telegram_binding_update(
    pool: &PgPool,
    sender: &external::ExternalNotificationSender,
    telegram_bot_id: uuid::Uuid,
    bot_token: &str,
    proxy_url: Option<&str>,
    update: &crate::telegram_updates::TelegramUpdate,
    now: DateTime<Utc>,
) -> AppResult<Option<TelegramBindingRequest>> {
```

Pass proxy to confirmation:

```rust
let outcome = sender
    .send_telegram(
        &external::TelegramChannelConfig {
            telegram_bot_id: Some(telegram_bot_id),
            bot_token_env: None,
            chat_id: binding
                .chat_id
                .clone()
                .unwrap_or_else(|| candidate.chat.chat_id.clone()),
        },
        bot_token,
        &external::telegram_binding_confirmation_text(chat_name),
        proxy_url,
    )
    .await;
```

In `run_telegram_update_poller`, pass effective proxy to `get_telegram_updates` and `process_telegram_binding_update`:

```rust
let updates = match sender
    .get_telegram_updates(
        &bot.bot_token,
        offset_claim.last_update_id,
        bot.effective_proxy_url.as_deref(),
    )
    .await
```

```rust
process_telegram_binding_update(
    &pool,
    &sender,
    bot.id,
    &bot.bot_token,
    bot.effective_proxy_url.as_deref(),
    &update,
    Utc::now(),
)
.await?;
```

- [ ] **Step 7: Pass proxy URL in notification delivery**

In `process_external_channel_delivery`, replace the `bot_token` local with a tuple:

```rust
let (bot_token, proxy_url) = if let Some(bot_id) = config.telegram_bot_id {
    match notifications::get_telegram_bot_secret(pool, task.tenant_id, bot_id).await {
        Ok(bot) if bot.status == "active" => (bot.bot_token, bot.effective_proxy_url),
        Ok(_) => {
            notifications::mark_external_notification_delivery_failed(
                pool,
                task.tenant_id,
                delivery_id,
                attempt_count,
                "telegram bot is inactive",
                None,
                None,
            )
            .await?;
            return Ok(());
        }
        Err(error) => {
            let message = error.to_string();
            notifications::mark_external_notification_delivery_failed(
                pool,
                task.tenant_id,
                delivery_id,
                attempt_count,
                &message,
                None,
                None,
            )
            .await?;
            return Ok(());
        }
    }
} else {
    let proxy_url = coin_listener_storage::telegram_settings::get_telegram_proxy_url(
        pool,
        task.tenant_id,
    )
    .await?;
    match config
        .bot_token_env
        .as_deref()
        .and_then(|name| std::env::var(name).ok())
    {
        Some(token) => (token, proxy_url),
        None => {
            notifications::mark_external_notification_delivery_failed(
                pool,
                task.tenant_id,
                delivery_id,
                attempt_count,
                "telegram token env is not set",
                None,
                None,
            )
            .await?;
            return Ok(());
        }
    }
};
```

Pass it to send:

```rust
sender
    .send_telegram(
        &config,
        &bot_token,
        &render_external_notification_text(event),
        proxy_url.as_deref(),
    )
    .await
```

- [ ] **Step 8: Write failing API source test for proxy-aware call sites**

In `backend/crates/api-server/src/routes.rs` test module, add:

```rust
#[test]
fn telegram_api_uses_effective_proxy_url() {
    let source = production_source();
    assert!(source.contains("bot.effective_proxy_url.as_deref()"));
    assert!(source.contains("telegram_channel_bot_token"));
    assert!(source.contains("proxy_url"));
}
```

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml -p api-server telegram_api_uses_effective_proxy_url -- --nocapture
```

Expected: FAIL because API routes do not pass effective proxy URLs.

- [ ] **Step 9: Pass proxy URL in API routes**

Update `verify_telegram_bot`:

```rust
let outcome = notifier::external::ExternalNotificationSender::new(reqwest::Client::new())
    .verify_telegram_bot(&bot.bot_token, bot.effective_proxy_url.as_deref())
    .await;
```

Update `telegram_webhook`:

```rust
notifier::process_telegram_binding_update(
    &state.postgres,
    &sender,
    bot_id,
    &bot.bot_token,
    bot.effective_proxy_url.as_deref(),
    &update,
    Utc::now(),
)
.await?;
```

Change `telegram_channel_bot_token` return type:

```rust
) -> AppResult<(notifier::external::TelegramChannelConfig, String, Option<String>)> {
```

Return proxy from bot:

```rust
Ok((config, bot.bot_token, bot.effective_proxy_url))
```

Update channel verify/test:

```rust
let (config, bot_token, proxy_url) =
    telegram_channel_bot_token(&state.postgres, auth.tenant_id, id, true).await?;
let outcome = notifier::external::ExternalNotificationSender::new(reqwest::Client::new())
    .send_telegram(
        &config,
        &bot_token,
        "Coin Listener Telegram channel verification",
        proxy_url.as_deref(),
    )
    .await;
```

Use the same pattern for `test_notification_channel` with `"Coin Listener test notification"`.

- [ ] **Step 10: Update runtime source tests that mention sender construction**

Existing tests in `backend/crates/api-server/src/main.rs` and `backend/crates/all-in-one/src/main.rs` assert `ExternalNotificationSender::new(reqwest::Client::new())`. Keep that string valid because the constructor still exists. Add one source assertion in each test that Telegram poller starts with `ExternalNotificationSender::new`.

- [ ] **Step 11: Run tests to verify this task passes**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml -p notifier external_sender_source_contains_proxy_resolver -- --nocapture
cargo test --locked --manifest-path backend/Cargo.toml -p notifier telegram_methods_accept_proxy_url -- --nocapture
cargo test --locked --manifest-path backend/Cargo.toml -p notifier telegram_runtime_passes_effective_proxy_url_to_sender -- --nocapture
cargo test --locked --manifest-path backend/Cargo.toml -p api-server telegram_api_uses_effective_proxy_url -- --nocapture
```

Expected: PASS.

- [ ] **Step 12: Commit**

```bash
git add backend/Cargo.toml backend/Cargo.lock backend/crates/notifier/Cargo.toml backend/crates/notifier/src/external.rs backend/crates/notifier/src/lib.rs backend/crates/api-server/src/routes.rs backend/crates/api-server/src/main.rs backend/crates/all-in-one/src/main.rs
git commit -m "$(cat <<'EOF'
让Telegram调用支持代理解析
EOF
)"
```

---

### Task 5: EVM `eth_getLogs` block range chunking

**Files:**
- Modify: `backend/crates/worker/src/lib.rs:38-106,332-404,1326-1372`
- Test: `backend/crates/worker/src/lib.rs`

- [ ] **Step 1: Write failing range chunk tests**

In `backend/crates/worker/src/lib.rs`, update the imports inside `mod evm_transfer_ranges`:

```rust
use crate::{
    bounded_block_ranges, evm_transfer_scan_range, BlockRange, EVM_ERC20_TRANSFER_CURSOR,
    EVM_LOG_MAX_BLOCK_SPAN,
};
```

Add tests inside `mod evm_transfer_ranges`:

```rust
#[test]
fn bounded_block_ranges_split_large_inclusive_ranges() {
    let ranges = bounded_block_ranges(1, 25_000, 10_000).unwrap();

    assert_eq!(
        ranges,
        vec![
            BlockRange { from_block: 1, to_block: 10_000 },
            BlockRange { from_block: 10_001, to_block: 20_000 },
            BlockRange { from_block: 20_001, to_block: 25_000 },
        ]
    );
}

#[test]
fn bounded_block_ranges_keep_exact_limit_as_one_range() {
    let ranges = bounded_block_ranges(5, 10_004, EVM_LOG_MAX_BLOCK_SPAN).unwrap();

    assert_eq!(
        ranges,
        vec![BlockRange { from_block: 5, to_block: 10_004 }]
    );
}

#[test]
fn bounded_block_ranges_reject_non_positive_span() {
    let error = bounded_block_ranges(1, 10, 0).unwrap_err();

    assert!(error.to_string().contains("max block span must be positive"));
}
```

Add a source-level test in the outer worker tests module:

```rust
#[test]
fn evm_erc20_scan_chunks_logs_before_updating_cursor() {
    let source = include_str!("lib.rs");
    let start = source
        .find("pub async fn scan_evm_erc20_transfers")
        .expect("erc20 scanner exists");
    let end = source[start..]
        .find("pub async fn scan_evm_address_with_provider")
        .expect("next function exists")
        + start;
    let scanner = &source[start..end];

    assert!(scanner.contains("bounded_block_ranges(from_block, to_block, EVM_LOG_MAX_BLOCK_SPAN)?"));
    assert!(scanner.contains("for range in ranges"));
    assert!(scanner.contains("range.from_block"));
    assert!(scanner.contains("range.to_block"));
    assert!(scanner.contains("last_successful_block = Some(range.to_block)"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml -p worker bounded_block_ranges -- --nocapture
cargo test --locked --manifest-path backend/Cargo.toml -p worker evm_erc20_scan_chunks_logs_before_updating_cursor -- --nocapture
```

Expected: FAIL because `BlockRange`, `EVM_LOG_MAX_BLOCK_SPAN`, and `bounded_block_ranges` do not exist.

- [ ] **Step 3: Add block range helper**

Near `EVM_TRANSFER_INITIAL_WINDOW_BLOCKS`, add:

```rust
pub const EVM_LOG_MAX_BLOCK_SPAN: i64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockRange {
    pub from_block: i64,
    pub to_block: i64,
}

pub fn bounded_block_ranges(
    from_block: i64,
    to_block: i64,
    max_block_span: i64,
) -> AppResult<Vec<BlockRange>> {
    if max_block_span <= 0 {
        return Err(AppError::Validation(
            "max block span must be positive".to_string(),
        ));
    }
    if to_block < from_block {
        return Ok(Vec::new());
    }

    let mut ranges = Vec::new();
    let mut current_from = from_block;
    while current_from <= to_block {
        let current_to = current_from
            .saturating_add(max_block_span - 1)
            .min(to_block);
        ranges.push(BlockRange {
            from_block: current_from,
            to_block: current_to,
        });
        current_from = current_to.saturating_add(1);
    }
    Ok(ranges)
}
```

- [ ] **Step 4: Chunk `scan_evm_erc20_transfers`**

Replace the body after the scan range calculation in `scan_evm_erc20_transfers` with chunk-aware logic:

```rust
let assets = selected_assets_by_type(selected_assets, "erc20");
if assets.is_empty() {
    return Ok(Vec::new());
}

let watched_topic = address_to_topic(&context.address)?;
let mut events = Vec::new();
let ranges = bounded_block_ranges(from_block, to_block, EVM_LOG_MAX_BLOCK_SPAN)?;
let mut last_successful_block = None;

for range in ranges {
    for asset in &assets {
        let Some(contract_address) = asset.contract_address.clone() else {
            continue;
        };
        let incoming = EvmLogFilter {
            address: contract_address.clone(),
            from_block: range.from_block,
            to_block: range.to_block,
            topics: vec![
                Some(TRANSFER_TOPIC0.to_string()),
                None,
                Some(watched_topic.clone()),
            ],
        };
        let outgoing = EvmLogFilter {
            address: contract_address,
            from_block: range.from_block,
            to_block: range.to_block,
            topics: vec![
                Some(TRANSFER_TOPIC0.to_string()),
                Some(watched_topic.clone()),
                None,
            ],
        };

        for filter in [incoming, outgoing] {
            let logs = rpc.eth_get_logs(filter).await?;
            for log in logs {
                let transfer = evm::decode_erc20_transfer_log(&log, asset.decimals)?;
                let draft = transfer_event_draft(context, asset, transfer);
                if let Some(event) =
                    repositories::insert_event_and_outbox_if_not_exists(pool, draft).await?
                {
                    events.push(event);
                }
            }
        }
    }

    repositories::upsert_scan_cursor(
        pool,
        context.tenant_id,
        context.chain_id,
        context.id,
        EVM_ERC20_TRANSFER_CURSOR,
        range.to_block,
    )
    .await?;
    last_successful_block = Some(range.to_block);
}

if last_successful_block.is_none() {
    return Ok(events);
}

Ok(events)
```

Remove the old single `repositories::upsert_scan_cursor(..., to_block)` block at the end of the function.

- [ ] **Step 5: Run tests to verify this task passes**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml -p worker bounded_block_ranges -- --nocapture
cargo test --locked --manifest-path backend/Cargo.toml -p worker evm_erc20_scan_chunks_logs_before_updating_cursor -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add backend/crates/worker/src/lib.rs
git commit -m "$(cat <<'EOF'
限制EVM日志扫描区块范围
EOF
)"
```

---

### Task 6: Frontend Telegram proxy API and UI

**Files:**
- Modify: `frontend/src/api/types.ts:115-139`
- Modify: `frontend/src/api/client.ts:183-211`
- Modify: `frontend/src/pages/TelegramBotsPage.tsx:1-166`
- Modify: `frontend/src/ui-regression.test.ts`
- Test: `frontend/src/ui-regression.test.ts`

- [ ] **Step 1: Write failing frontend regression test**

In `frontend/src/ui-regression.test.ts`, add:

```ts
test('telegram bot management exposes global and bot proxy configuration', () => {
  const types = readSource('api/types.ts');
  const client = readSource('api/client.ts');
  const page = readSource('pages/TelegramBotsPage.tsx');

  for (const expected of [
    'export type TelegramSettings',
    'proxy_url_preview?: string | null',
    'proxy_source: string',
    'getTelegramSettings',
    'updateTelegramSettings',
    'Telegram 全局代理',
    '代理来源',
    'proxy_mode',
    'proxy_url',
    '使用全局代理',
    '此机器人单独配置代理',
  ]) {
    expectContains(`${types}\n${client}\n${page}`, expected);
  }
});
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: FAIL because the proxy fields and UI are missing.

- [ ] **Step 3: Update API types**

Modify `frontend/src/api/types.ts` Telegram section:

```ts
export type TelegramSettings = {
  tenant_id: string;
  proxy_url_preview?: string | null;
  has_proxy: boolean;
  created_at?: string | null;
  updated_at?: string | null;
};

export type UpdateTelegramSettingsRequest = {
  proxy_url?: string | null;
};

export type TelegramBot = {
  id: string;
  tenant_id: string;
  name: string;
  username?: string | null;
  token_preview: string;
  proxy_source: string;
  proxy_url_preview?: string | null;
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
  proxy_url?: string | null;
};

export type UpdateTelegramBotRequest = {
  name: string;
  bot_token?: string | null;
  status: string;
  proxy_url?: string | null;
};
```

- [ ] **Step 4: Update API client**

Add `TelegramSettings` and `UpdateTelegramSettingsRequest` to the type imports in `frontend/src/api/client.ts`, then add functions before `listTelegramBots`:

```ts
export function getTelegramSettings(): Promise<TelegramSettings> {
  return request<TelegramSettings>('/api/telegram-settings');
}

export function updateTelegramSettings(payload: UpdateTelegramSettingsRequest): Promise<TelegramSettings> {
  return request<TelegramSettings>('/api/telegram-settings', {
    method: 'PUT',
    body: JSON.stringify(payload),
  });
}
```

- [ ] **Step 5: Update Telegram bot page imports and form type**

Modify imports in `frontend/src/pages/TelegramBotsPage.tsx`:

```ts
import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Banner, Button, Card, Form, Popconfirm, Space, Tag, Toast, Typography } from '@douyinfe/semi-ui';
import {
  createTelegramBot,
  deleteTelegramBot,
  getTelegramSettings,
  listTelegramBots,
  updateTelegramBot,
  updateTelegramSettings,
  verifyTelegramBot,
} from '../api/client';
import type { TelegramBot, UpdateTelegramBotRequest } from '../api/types';
```

Update `BotForm`:

```ts
type BotForm = {
  name?: string;
  bot_token?: string;
  status?: string;
  proxy_mode?: 'global' | 'bot';
  proxy_url?: string;
};

type TelegramSettingsForm = {
  proxy_url?: string;
};
```

Add helpers above the component:

```ts
function proxySourceLabel(source: string) {
  if (source === 'bot') {
    return '机器人代理';
  }
  if (source === 'global') {
    return '全局代理';
  }
  return '直连';
}

function proxySourceColor(source: string) {
  if (source === 'bot') {
    return 'blue';
  }
  if (source === 'global') {
    return 'cyan';
  }
  return 'grey';
}

function buildProxyUrlPayload(values: BotForm, editingBot: TelegramBot | null) {
  if (values.proxy_mode === 'global') {
    return null;
  }
  const proxyUrl = values.proxy_url?.trim();
  if (proxyUrl) {
    return proxyUrl;
  }
  return editingBot ? undefined : null;
}
```

- [ ] **Step 6: Add settings query and mutation**

Inside `TelegramBotsPage`, after `botsQuery`, add:

```ts
const settingsQuery = useQuery({ queryKey: ['telegram-settings'], queryFn: getTelegramSettings });

const settingsMutation = useMutation({
  mutationFn: (values: TelegramSettingsForm) => updateTelegramSettings({ proxy_url: values.proxy_url?.trim() || null }),
  onSuccess: () => {
    Toast.success('Telegram 全局代理已更新');
    queryClient.invalidateQueries({ queryKey: ['telegram-settings'] });
    queryClient.invalidateQueries({ queryKey: ['telegram-bots'] });
  },
  onError: error => Toast.error(error instanceof Error ? error.message : 'Telegram 全局代理保存失败'),
});
```

- [ ] **Step 7: Include proxy payload in create/update**

In `saveMutation`, compute proxy before the `if`:

```ts
const proxy_url = buildProxyUrlPayload(values, editingBot);
```

Update update payload:

```ts
const payload: UpdateTelegramBotRequest = {
  name: values.name ?? '',
  bot_token: values.bot_token || null,
  status: values.status ?? 'active',
};
if (proxy_url !== undefined) {
  payload.proxy_url = proxy_url;
}
return updateTelegramBot(editingBot.id, payload);
```

Update create payload:

```ts
return createTelegramBot({
  name: values.name ?? '',
  bot_token: values.bot_token ?? '',
  status: values.status ?? 'active',
  proxy_url: proxy_url ?? null,
});
```

- [ ] **Step 8: Add global proxy card above the list**

Before `<DataSurface title="TG机器人列表">`, add:

```tsx
<Card className="filter-card" title="Telegram 全局代理">
  <Typography.Text type="tertiary">
    未配置机器人专属代理时，验证、通知发送、绑定确认和 getUpdates 会使用这里的代理。
  </Typography.Text>
  {settingsQuery.data?.proxy_url_preview ? (
    <div className="telegram-proxy-preview">当前代理：{settingsQuery.data.proxy_url_preview}</div>
  ) : (
    <div className="telegram-proxy-preview">当前代理：直连</div>
  )}
  <Form<TelegramSettingsForm>
    initValues={{ proxy_url: '' }}
    onSubmit={values => settingsMutation.mutate(values)}
    labelPosition="left"
    labelWidth={110}
  >
    <Form.Input
      field="proxy_url"
      label="代理 URL"
      placeholder="留空清空全局代理，例如 socks5://127.0.0.1:1080"
    />
    <Space className="form-modal-actions">
      <Button htmlType="submit" type="primary" loading={settingsMutation.isPending}>保存全局代理</Button>
    </Space>
  </Form>
</Card>
```

- [ ] **Step 9: Add proxy columns and form fields**

Add column before `最后验证`:

```tsx
{
  title: '代理来源',
  dataIndex: 'proxy_source',
  width: 130,
  render: value => <Tag color={proxySourceColor(String(value))}>{proxySourceLabel(String(value))}</Tag>,
},
{
  title: '代理',
  dataIndex: 'proxy_url_preview',
  width: 220,
  ellipsis: { showTitle: true },
  render: value => value ? String(value) : '-',
},
```

Update `scroll` to `scroll={{ x: 1450 }}`.

Update form `initValues`:

```tsx
initValues={editingBot ? {
  name: editingBot.name,
  status: editingBot.status,
  proxy_mode: editingBot.proxy_source === 'bot' ? 'bot' : 'global',
} : { status: 'active', proxy_mode: 'global' }}
```

Add proxy form fields after status:

```tsx
<Form.Select field="proxy_mode" label="代理模式">
  <Form.Select.Option value="global">使用全局代理</Form.Select.Option>
  <Form.Select.Option value="bot">此机器人单独配置代理</Form.Select.Option>
</Form.Select>
<Form.Input
  field="proxy_url"
  label="代理 URL"
  placeholder={editingBot?.proxy_source === 'bot' && editingBot.proxy_url_preview ? `当前：${editingBot.proxy_url_preview}，留空表示不更换` : '例如 socks5://127.0.0.1:1080'}
/>
```

- [ ] **Step 10: Run frontend regression test**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: PASS.

- [ ] **Step 11: Commit**

```bash
git add frontend/src/api/types.ts frontend/src/api/client.ts frontend/src/pages/TelegramBotsPage.tsx frontend/src/ui-regression.test.ts
git commit -m "$(cat <<'EOF'
添加TG机器人代理配置界面
EOF
)"
```

---

### Task 7: Light-mode app tokens and readability cleanup

**Files:**
- Modify: `frontend/src/styles.css:1-512`
- Modify: `frontend/src/ui-regression.test.ts`
- Test: `frontend/src/ui-regression.test.ts`

- [ ] **Step 1: Write failing light-mode regression test**

In `frontend/src/ui-regression.test.ts`, add:

```ts
test('light mode uses app tokens for readable main content while keeping brand sidebar dark', () => {
  const styles = readSource('styles.css');

  for (const expected of [
    '--app-shell-sidebar-bg',
    '--app-content-bg',
    '--app-card-bg',
    '--app-text-primary',
    '--app-text-secondary',
    '--app-border-subtle',
    'body[theme-mode=\'dark\']',
    'background: var(--app-content-bg)',
    'color: var(--app-text-primary)',
    '.app-sider',
  ]) {
    expectContains(styles, expected);
  }

  const mainContentSelectors = [
    '.app-content',
    '.filter-panel',
    '.data-surface',
    '.notification-detail-card',
    '.detail-json',
  ];
  for (const selector of mainContentSelectors) {
    const selectorIndex = styles.indexOf(selector);
    if (selectorIndex === -1) {
      throw new Error(`Missing selector ${selector}`);
    }
    const block = styles.slice(selectorIndex, styles.indexOf('}', selectorIndex));
    expectNotContains(block, '#e5f7ff');
    expectNotContains(block, '#f8fbff');
    expectNotContains(block, 'rgba(226, 232, 240');
  }
});
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: FAIL because the app tokens do not exist.

- [ ] **Step 3: Add app theme tokens**

At the top of `frontend/src/styles.css`, after the `html, body, #root` block, add:

```css
:root {
  --app-shell-sidebar-bg: linear-gradient(180deg, #06111f 0%, #0b1728 54%, #101827 100%);
  --app-shell-sidebar-border: rgba(148, 163, 184, 0.18);
  --app-sidebar-text-primary: #e5f7ff;
  --app-sidebar-text-secondary: rgba(226, 232, 240, 0.78);
  --app-sidebar-selected-bg: rgba(14, 165, 233, 0.18);
  --app-sidebar-selected-text: #e0f2fe;
  --app-content-bg: var(--semi-color-bg-0);
  --app-content-bg-accent-a: rgba(20, 184, 166, 0.12);
  --app-content-bg-accent-b: rgba(59, 130, 246, 0.14);
  --app-card-bg: color-mix(in srgb, var(--semi-color-bg-1) 94%, transparent);
  --app-card-bg-strong: var(--semi-color-bg-1);
  --app-text-primary: var(--semi-color-text-0);
  --app-text-secondary: var(--semi-color-text-1);
  --app-text-tertiary: var(--semi-color-text-2);
  --app-border-subtle: var(--semi-color-border);
  --app-shadow-soft: 0 20px 60px rgba(15, 23, 42, 0.06);
  --app-login-panel-bg: rgba(8, 18, 32, 0.72);
  --app-login-text-primary: #e5f7ff;
  --app-login-text-secondary: rgba(226, 232, 240, 0.78);
}

body[theme-mode='dark'] {
  --app-content-bg: var(--semi-color-bg-0);
  --app-content-bg-accent-a: rgba(20, 184, 166, 0.16);
  --app-content-bg-accent-b: rgba(59, 130, 246, 0.18);
  --app-card-bg: color-mix(in srgb, var(--semi-color-bg-1) 90%, transparent);
  --app-card-bg-strong: var(--semi-color-bg-1);
  --app-shadow-soft: 0 20px 60px rgba(0, 0, 0, 0.22);
}
```

- [ ] **Step 4: Replace shell and main content colors**

Update the existing shell and main content selectors:

```css
body {
  color: var(--app-text-primary);
  background:
    radial-gradient(circle at 12% 0%, var(--app-content-bg-accent-a), transparent 32%),
    radial-gradient(circle at 88% 10%, var(--app-content-bg-accent-b), transparent 30%),
    var(--app-content-bg);
}

.app-sider {
  min-height: 100vh;
  background: var(--app-shell-sidebar-bg);
  border-right: 1px solid var(--app-shell-sidebar-border);
}

.brand {
  height: 64px;
  display: flex;
  align-items: center;
  padding: 0 20px;
  color: var(--app-sidebar-text-primary);
  font-weight: 800;
  font-size: 18px;
  letter-spacing: 0.02em;
}

.app-header {
  height: 64px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 0 24px;
  color: var(--app-text-primary);
  background: color-mix(in srgb, var(--app-content-bg) 88%, transparent);
  border-bottom: 1px solid var(--app-border-subtle);
  backdrop-filter: blur(16px);
}

.app-content {
  min-width: 0;
  height: calc(100vh - 64px);
  padding: 24px;
  overflow: auto;
  color: var(--app-text-primary);
  background: var(--app-content-bg);
}
```

- [ ] **Step 5: Replace card/table/detail colors**

Update these selectors:

```css
.form-help-text {
  margin-top: 0;
  color: var(--app-text-tertiary);
}

.address-import-progress {
  margin-top: 16px;
  padding: 14px;
  border: 1px solid var(--app-border-subtle);
  border-radius: 12px;
  color: var(--app-text-primary);
  background: var(--app-card-bg-strong);
}

.filter-panel,
.data-surface {
  width: 100%;
  min-width: 0;
  border: 1px solid var(--app-border-subtle);
  color: var(--app-text-primary);
  background: var(--app-card-bg);
  box-shadow: var(--app-shadow-soft);
}

.metric-card {
  border: 1px solid var(--app-border-subtle);
  color: var(--app-text-primary);
  background: linear-gradient(180deg, var(--app-card-bg-strong), var(--app-content-bg));
}

.data-table .semi-table-thead > .semi-table-row > .semi-table-row-head {
  color: var(--app-text-secondary);
  background: color-mix(in srgb, var(--semi-color-bg-2) 86%, transparent);
  font-size: 12px;
  font-weight: 700;
}

.dashboard-health-title,
.dashboard-step-title,
.status-summary-item div,
.detail-value,
.detail-json {
  color: var(--app-text-primary);
}

.dashboard-chain-step,
.notification-detail-card {
  border: 1px solid var(--app-border-subtle);
  color: var(--app-text-primary);
  background: var(--app-card-bg);
}

.detail-json {
  max-height: 260px;
  margin: 10px 0 0;
  padding: 14px;
  overflow: auto;
  border: 1px solid var(--app-border-subtle);
  border-radius: 14px;
  background: var(--app-content-bg);
  white-space: pre-wrap;
  word-break: break-word;
}
```

- [ ] **Step 6: Keep dark surfaces intentionally dark**

Update sidebar and login selectors to use dedicated dark-surface tokens:

```css
.login-hero-panel {
  padding: 38px;
  color: var(--app-login-text-primary);
}

.login-brand-row {
  display: flex;
  align-items: center;
  gap: 10px;
  color: #c8f7ff;
}

.login-hero-title.semi-typography {
  max-width: 520px;
  margin: 48px 0 18px;
  color: var(--app-login-text-primary);
  font-size: clamp(40px, 6vw, 76px);
  line-height: 0.92;
  letter-spacing: -0.05em;
}

.login-hero-copy.semi-typography,
.login-card .semi-typography-tertiary,
.login-form .semi-form-field-label-text {
  color: var(--app-login-text-secondary);
}

.login-card .semi-typography {
  color: var(--app-login-text-primary);
}

.chain-nav .semi-navigation-item {
  color: var(--app-sidebar-text-secondary);
}

.chain-nav .semi-navigation-item-selected {
  color: var(--app-sidebar-selected-text);
  background: var(--app-sidebar-selected-bg);
}
```

- [ ] **Step 7: Run frontend regression test**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/styles.css frontend/src/ui-regression.test.ts
git commit -m "$(cat <<'EOF'
修复前端白天模式文字可读性
EOF
)"
```

---

### Task 8: End-to-end verification and cleanup

**Files:**
- Modify only if verification exposes a concrete issue: files changed by Tasks 1-7
- Test: full backend and frontend verification commands

- [ ] **Step 1: Run full backend tests**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml
```

Expected: all backend crate tests and doctests PASS.

- [ ] **Step 2: Run frontend source regression**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: all frontend UI regression tests PASS.

- [ ] **Step 3: Run frontend production build**

Run:

```bash
npm --prefix frontend run build
```

Expected: build exits 0. Existing Vite warnings about `lottie-web` eval or chunk size are acceptable if unchanged.

- [ ] **Step 4: Inspect final diff for scope control**

Run:

```bash
git status --short
git diff --stat HEAD
```

Expected: changes are limited to Telegram proxy support, EVM RPC range chunking, and light-mode readability. No unrelated files are modified.

- [ ] **Step 5: Commit verification fixes if any were needed**

If Step 1, Step 2, or Step 3 required code fixes, commit those fixes:

```bash
git add backend frontend
git commit -m "$(cat <<'EOF'
完善TG代理与RPC范围验证
EOF
)"
```

If no fixes were needed, do not create an empty commit.

---

## Self-Review Checklist

- Spec coverage:
  - Telegram global proxy: Tasks 1, 2, 4, 6.
  - Telegram per-bot proxy override: Tasks 1, 3, 4, 6.
  - Proxy URL-only input and validation: Tasks 1, 2, 3, 6.
  - Proxy-aware verification/send/binding/getUpdates: Task 4.
  - Light-mode readable mixed brand style: Task 7.
  - EVM `eth_getLogs` chunking and cursor safety: Task 5.
  - Verification commands: Task 8.
- Type consistency:
  - Backend public settings type: `TelegramSettings`.
  - Frontend settings type: `TelegramSettings`.
  - Bot public proxy fields: `proxy_source`, `proxy_url_preview`.
  - Secret effective proxy field: `effective_proxy_url`.
  - Update request proxy field: `proxy_url` with omitted/null/value semantics.
- Placeholder scan: no placeholder markers or unspecified implementation steps.
