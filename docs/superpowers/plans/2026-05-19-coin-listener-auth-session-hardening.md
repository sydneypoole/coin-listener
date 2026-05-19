# Coin Listener Auth Session Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace demo login with hashed-password verification, signed bearer tokens, protected API routes, authenticated tenant context, and frontend session persistence/logout.

**Architecture:** Add a small backend auth boundary in `api-server` for Argon2id password verification, JWT issuing/validation, and `AuthContext` extraction. Keep `/health` and `/api/auth/login` public, protect all other `/api/*` routes with Axum middleware, and centralize frontend session/token handling in one `frontend/src/auth/session.ts` module.

**Tech Stack:** Rust, Axum 0.7 middleware/extractors, Argon2id (`argon2` crate), JWT (`jsonwebtoken` crate), PostgreSQL migrations via `sqlx`, React/Vite/TypeScript, Semi Design.

---

## File structure

- Create `backend/crates/api-server/src/auth.rs`: password verification, token claims, token issuer/validator, bearer parsing, `AuthContext`, and auth middleware tests.
- Modify `backend/crates/api-server/src/lib.rs`: expose the new auth module where tests and routes need it.
- Modify `backend/crates/api-server/src/routes.rs`: add auth config/state fields, protect API routes, use `AuthContext` in tenant-scoped handlers, map forbidden errors, and update login.
- Modify `backend/crates/api-server/src/main.rs`: load auth config before building `ApiState`.
- Modify `backend/crates/all-in-one/src/main.rs`: load auth config before building API state.
- Modify `backend/crates/core/src/config.rs`: add `AuthConfig` and environment parsing for token secret/TTL.
- Modify `backend/crates/core/src/error.rs`: add `Forbidden` for authenticated-but-not-allowed responses.
- Modify `backend/Cargo.toml`: add workspace dependencies for `argon2` and `jsonwebtoken`.
- Modify `backend/crates/api-server/Cargo.toml`: use the new auth dependencies.
- Create `backend/crates/storage/migrations/0012_auth_session_baseline.sql`: replace only the legacy seeded admin plaintext password with a precomputed Argon2id hash.
- Modify `backend/crates/storage/src/repositories.rs`: add active user/tenant membership helpers needed by middleware and tests.
- Modify `.env.example`: document `AUTH_TOKEN_SECRET` and `AUTH_TOKEN_TTL_SECONDS`.
- Create `frontend/src/auth/session.ts`: single browser-storage session boundary.
- Modify `frontend/src/api/client.ts`: attach bearer tokens for non-login requests and clear session on `401`.
- Modify `frontend/src/App.tsx`: initialize from storage, save login session, and add logout.
- Modify `frontend/src/pages/LoginPage.tsx`: remove displayed/default password and keep login behavior.

## Task 1: Backend auth primitives and configuration

**Files:**
- Create: `backend/crates/api-server/src/auth.rs`
- Modify: `backend/crates/api-server/src/lib.rs`
- Modify: `backend/crates/api-server/Cargo.toml`
- Modify: `backend/Cargo.toml`
- Modify: `backend/crates/core/src/config.rs`
- Modify: `backend/crates/core/src/error.rs`

- [ ] **Step 1: Add failing tests for password verification, token validation, and auth config parsing**

Create `backend/crates/api-server/src/auth.rs` with these tests first.

```rust
#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use uuid::Uuid;

    #[test]
    fn verifies_argon2id_password_hash() {
        let hash = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXJhbmRvbXNhbHQ$laqOUbdkJho4NACYmDwyLQdS/qq83rIuReZa+IyST2I";

        assert!(super::verify_password("admin", hash).expect("password verifies"));
        assert!(!super::verify_password("wrong", hash).expect("password rejects"));
    }

    #[test]
    fn rejects_plaintext_password_hashes() {
        assert!(!super::verify_password("admin", "admin").expect("plaintext rejected"));
    }

    #[test]
    fn token_round_trips_claims() {
        let user_id = Uuid::from_u128(7);
        let tenant_id = Uuid::from_u128(9);
        let settings = super::TokenSettings {
            secret: "test-secret-with-enough-entropy".to_string(),
            ttl: Duration::seconds(3600),
        };

        let token = super::issue_token(&settings, user_id, tenant_id, "admin@example.com")
            .expect("token issued");
        let claims = super::validate_token(&settings, &token).expect("token validates");

        assert_eq!(claims.subject_uuid().unwrap(), user_id);
        assert_eq!(claims.tenant_uuid().unwrap(), tenant_id);
        assert_eq!(claims.email, "admin@example.com");
        assert!(claims.exp > Utc::now().timestamp());
    }

    #[test]
    fn rejects_tampered_tokens() {
        let settings = super::TokenSettings {
            secret: "test-secret-with-enough-entropy".to_string(),
            ttl: Duration::seconds(3600),
        };
        let token = super::issue_token(
            &settings,
            Uuid::from_u128(7),
            Uuid::from_u128(9),
            "admin@example.com",
        )
        .expect("token issued");
        let tampered = format!("{}x", token);

        assert!(super::validate_token(&settings, &tampered).is_err());
    }

    #[test]
    fn rejects_expired_tokens() {
        let settings = super::TokenSettings {
            secret: "test-secret-with-enough-entropy".to_string(),
            ttl: Duration::seconds(-1),
        };
        let token = super::issue_token(
            &settings,
            Uuid::from_u128(7),
            Uuid::from_u128(9),
            "admin@example.com",
        )
        .expect("token issued");

        assert!(super::validate_token(&settings, &token).is_err());
    }
}
```

Also add this config test to `backend/crates/core/src/config.rs` inside the existing `#[cfg(test)] mod tests`:

```rust
use crate::config::AuthConfig;

#[test]
fn auth_config_carries_token_runtime_settings() {
    let config = AuthConfig {
        token_secret: "test-secret-with-enough-entropy".to_string(),
        token_ttl_seconds: 43_200,
    };

    assert_eq!(config.token_secret, "test-secret-with-enough-entropy");
    assert_eq!(config.token_ttl_seconds, 43_200);
}
```

Add this error mapping test to `backend/crates/api-server/src/routes.rs` inside the existing tests module:

```rust
#[test]
fn forbidden_errors_map_to_http_403() {
    let response = super::ApiError::from(coin_listener_core::AppError::Forbidden).into_response();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p api-server --manifest-path backend/Cargo.toml auth::tests -- --nocapture
cargo test -p coin-listener-core --manifest-path backend/Cargo.toml auth_config_carries_token_runtime_settings -- --nocapture
cargo test -p api-server --manifest-path backend/Cargo.toml forbidden_errors_map_to_http_403 -- --nocapture
```

Expected: FAIL. The failures should mention missing `verify_password`, `TokenSettings`, `issue_token`, `validate_token`, `AuthConfig`, or `AppError::Forbidden`.

- [ ] **Step 3: Add backend dependencies**

In `backend/Cargo.toml`, add these workspace dependencies under `[workspace.dependencies]`:

```toml
argon2 = "0.5"
jsonwebtoken = "9"
```

In `backend/crates/api-server/Cargo.toml`, add these dependencies under `[dependencies]`:

```toml
argon2.workspace = true
jsonwebtoken.workspace = true
```

- [ ] **Step 4: Implement config and forbidden error**

In `backend/crates/core/src/config.rs`, change `AppConfig` and add `AuthConfig`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub postgres: PostgresConfig,
    pub redis: RedisConfig,
    pub scan: ScanConfig,
    pub notify: NotifyConfig,
    pub auth: AuthConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    pub token_secret: String,
    pub token_ttl_seconds: i64,
}
```

In `AppConfig::from_env()`, add these merges before `.extract()`:

```rust
.merge((
    "auth.token_secret",
    env::var("AUTH_TOKEN_SECRET").unwrap_or_default(),
))
.merge((
    "auth.token_ttl_seconds",
    env::var("AUTH_TOKEN_TTL_SECONDS").unwrap_or_else(|_| "43200".to_string()),
))
```

In `backend/crates/core/src/lib.rs`, update the config re-export:

```rust
pub use config::{
    AppConfig, AuthConfig, NotifyConfig, PostgresConfig, RedisConfig, ScanConfig, ServerConfig,
};
```

In `backend/crates/core/src/error.rs`, add a forbidden variant:

```rust
#[error("forbidden")]
Forbidden,
```

In `backend/crates/api-server/src/routes.rs`, make `ApiError` public and update the status match:

```rust
pub struct ApiError(AppError);
```

```rust
AppError::Unauthorized => StatusCode::UNAUTHORIZED,
AppError::Forbidden => StatusCode::FORBIDDEN,
AppError::NotFound(_) => StatusCode::NOT_FOUND,
```

- [ ] **Step 5: Implement auth primitives**

Replace the empty `backend/crates/api-server/src/auth.rs` with:

```rust
use argon2::{
    password_hash::{PasswordHash, PasswordVerifier},
    Argon2,
};
use chrono::{Duration, Utc};
use coin_listener_core::{AppError, AppResult};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct TokenSettings {
    pub secret: String,
    pub ttl: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthClaims {
    pub sub: String,
    pub tenant_id: String,
    pub email: String,
    pub iat: i64,
    pub exp: i64,
}

impl AuthClaims {
    pub fn subject_uuid(&self) -> AppResult<Uuid> {
        Uuid::parse_str(&self.sub).map_err(|error| AppError::Validation(error.to_string()))
    }

    pub fn tenant_uuid(&self) -> AppResult<Uuid> {
        Uuid::parse_str(&self.tenant_id).map_err(|error| AppError::Validation(error.to_string()))
    }
}

pub fn token_settings(secret: String, ttl_seconds: i64) -> AppResult<TokenSettings> {
    if secret.trim().is_empty() {
        return Err(AppError::Config("AUTH_TOKEN_SECRET is required".to_string()));
    }

    Ok(TokenSettings {
        secret,
        ttl: Duration::seconds(ttl_seconds),
    })
}

pub fn verify_password(password: &str, password_hash: &str) -> AppResult<bool> {
    let parsed_hash = match PasswordHash::new(password_hash) {
        Ok(hash) => hash,
        Err(_) => return Ok(false),
    };

    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok())
}

pub fn issue_token(
    settings: &TokenSettings,
    user_id: Uuid,
    tenant_id: Uuid,
    email: &str,
) -> AppResult<String> {
    let issued_at = Utc::now();
    let expires_at = issued_at + settings.ttl;
    let claims = AuthClaims {
        sub: user_id.to_string(),
        tenant_id: tenant_id.to_string(),
        email: email.to_string(),
        iat: issued_at.timestamp(),
        exp: expires_at.timestamp(),
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(settings.secret.as_bytes()),
    )
    .map_err(|error| AppError::Config(error.to_string()))
}

pub fn validate_token(settings: &TokenSettings, token: &str) -> AppResult<AuthClaims> {
    decode::<AuthClaims>(
        token,
        &DecodingKey::from_secret(settings.secret.as_bytes()),
        &Validation::default(),
    )
    .map(|data| data.claims)
    .map_err(|_| AppError::Unauthorized)
}
```

In `backend/crates/api-server/src/lib.rs`, expose the module:

```rust
pub mod auth;
mod routes;

pub use routes::{build_router, ApiError, ApiState, HealthResponse};
```

- [ ] **Step 6: Run tests to verify they pass**

Run:

```bash
cargo test -p api-server --manifest-path backend/Cargo.toml auth::tests -- --nocapture
cargo test -p coin-listener-core --manifest-path backend/Cargo.toml auth_config_carries_token_runtime_settings -- --nocapture
cargo test -p api-server --manifest-path backend/Cargo.toml forbidden_errors_map_to_http_403 -- --nocapture
```

Expected: PASS.

- [ ] **Step 7: Commit Task 1**

Run:

```bash
git add backend/Cargo.toml backend/crates/api-server/Cargo.toml backend/crates/api-server/src/auth.rs backend/crates/api-server/src/lib.rs backend/crates/api-server/src/routes.rs backend/crates/core/src/config.rs backend/crates/core/src/error.rs backend/crates/core/src/lib.rs
git commit -m "Add auth token primitives"
```

## Task 2: Seed migration and tenant membership repository helpers

**Files:**
- Create: `backend/crates/storage/migrations/0012_auth_session_baseline.sql`
- Modify: `backend/crates/storage/src/repositories.rs`

- [ ] **Step 1: Add failing repository and migration tests**

In `backend/crates/storage/src/repositories.rs`, add these tests inside the existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn auth_baseline_migration_hashes_only_legacy_admin_password() {
    let migration = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("migrations/0012_auth_session_baseline.sql"),
    )
    .expect("migration readable");

    assert!(migration.contains("WHERE email = 'admin@example.com'"));
    assert!(migration.contains("password_hash = 'admin'"));
    assert!(migration.contains("$argon2id$"));
    assert!(!migration.contains("UPDATE users SET password_hash"));
}

#[test]
fn active_tenant_membership_query_checks_user_and_tenant_status() {
    assert!(ACTIVE_TENANT_MEMBERSHIP_QUERY.contains("u.status = 'active'"));
    assert!(ACTIVE_TENANT_MEMBERSHIP_QUERY.contains("t.status = 'active'"));
    assert!(ACTIVE_TENANT_MEMBERSHIP_QUERY.contains("tm.user_id = $1"));
    assert!(ACTIVE_TENANT_MEMBERSHIP_QUERY.contains("tm.tenant_id = $2"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p coin-listener-storage --manifest-path backend/Cargo.toml auth_baseline_migration_hashes_only_legacy_admin_password -- --nocapture
cargo test -p coin-listener-storage --manifest-path backend/Cargo.toml active_tenant_membership_query_checks_user_and_tenant_status -- --nocapture
```

Expected: FAIL because the migration file and `ACTIVE_TENANT_MEMBERSHIP_QUERY` do not exist yet.

- [ ] **Step 3: Create migration**

Create `backend/crates/storage/migrations/0012_auth_session_baseline.sql`:

```sql
UPDATE users
SET password_hash = '$argon2id$v=19$m=19456,t=2,p=1$c29tZXJhbmRvbXNhbHQ$laqOUbdkJho4NACYmDwyLQdS/qq83rIuReZa+IyST2I',
    updated_at = NOW()
WHERE email = 'admin@example.com'
  AND password_hash = 'admin';

INSERT INTO schema_migrations_marker (name)
VALUES ('0012_auth_session_baseline')
ON CONFLICT (name) DO NOTHING;
```

This hash is for the bootstrap password `admin`. It is used only to migrate the existing local seed away from plaintext; do not add new plaintext-password fallbacks.

- [ ] **Step 4: Add active tenant membership helper**

In `backend/crates/storage/src/repositories.rs`, near the existing login queries, add:

```rust
pub const ACTIVE_TENANT_MEMBERSHIP_QUERY: &str = r#"
SELECT t.id, t.name, t.status
FROM tenants t
INNER JOIN tenant_members tm ON tm.tenant_id = t.id
INNER JOIN users u ON u.id = tm.user_id
WHERE tm.user_id = $1
  AND tm.tenant_id = $2
  AND u.status = 'active'
  AND t.status = 'active'
LIMIT 1
"#;

pub async fn active_tenant_membership(
    pool: &PgPool,
    user_id: Uuid,
    tenant_id: Uuid,
) -> AppResult<Tenant> {
    sqlx::query_as::<_, Tenant>(ACTIVE_TENANT_MEMBERSHIP_QUERY)
        .bind(user_id)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or(AppError::Forbidden)
}
```

Update `default_tenant_for_user` so login only selects active users and active tenants:

```rust
pub async fn default_tenant_for_user(pool: &PgPool, user_id: Uuid) -> AppResult<Tenant> {
    sqlx::query_as::<_, Tenant>(
        r#"
        SELECT t.id, t.name, t.status
        FROM tenants t
        INNER JOIN tenant_members tm ON tm.tenant_id = t.id
        INNER JOIN users u ON u.id = tm.user_id
        WHERE tm.user_id = $1
          AND u.status = 'active'
          AND t.status = 'active'
        ORDER BY tm.created_at ASC
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or(AppError::Forbidden)
}
```

Update `find_user_by_email` so inactive users cannot log in:

```rust
pub async fn find_user_by_email(pool: &PgPool, email: &str) -> AppResult<User> {
    sqlx::query_as::<_, User>(
        "SELECT id, email, password_hash, display_name, status FROM users WHERE email = $1 AND status = 'active'",
    )
    .bind(email)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or(AppError::Unauthorized)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run:

```bash
cargo test -p coin-listener-storage --manifest-path backend/Cargo.toml auth_baseline_migration_hashes_only_legacy_admin_password -- --nocapture
cargo test -p coin-listener-storage --manifest-path backend/Cargo.toml active_tenant_membership_query_checks_user_and_tenant_status -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit Task 2**

Run:

```bash
git add backend/crates/storage/migrations/0012_auth_session_baseline.sql backend/crates/storage/src/repositories.rs
git commit -m "Hash seeded admin password"
```

## Task 3: Protect API routes and update login token issuance

**Files:**
- Modify: `backend/crates/api-server/src/auth.rs`
- Modify: `backend/crates/api-server/src/routes.rs`
- Modify: `backend/crates/api-server/src/main.rs`
- Modify: `backend/crates/all-in-one/src/main.rs`
- Modify: `.env.example`

- [ ] **Step 1: Add failing route protection tests**

In `backend/crates/api-server/src/routes.rs`, update the test imports:

```rust
use super::{build_router, ApiState};
use crate::auth::TokenSettings;
use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    response::IntoResponse,
};
use chrono::Duration;
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;
```

Add this helper to the tests module:

```rust
fn test_state() -> Arc<ApiState> {
    Arc::new(ApiState {
        postgres: PgPool::connect_lazy("postgres://postgres:postgres@localhost/coin_listener_test")
            .expect("valid postgres url"),
        redis: None,
        scan_queue_key: "scan:address:queue".to_string(),
        notify_queue_key: "notify:event:queue".to_string(),
        enable_dev_routes: true,
        auth: TokenSettings {
            secret: "test-secret-with-enough-entropy".to_string(),
            ttl: Duration::seconds(3600),
        },
    })
}
```

Add these tests:

```rust
#[tokio::test]
async fn protected_api_route_rejects_missing_token() {
    let app = build_router(test_state());

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/chains")
                .body(Body::empty())
                .expect("valid request"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn health_route_remains_public() {
    let app = build_router(test_state());

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/health")
                .body(Body::empty())
                .expect("valid request"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn login_route_remains_public() {
    let app = build_router(test_state());

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"email":"missing@example.com","password":"wrong"}"#))
                .expect("valid request"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
```

Then update existing route-shape tests to call `test_state()`. For existing protected `/api/*` route-shape tests that do not set up database-backed authenticated users, change their expected status to `StatusCode::UNAUTHORIZED`; those tests now assert route protection rather than downstream query validation. Keep the new `health_route_remains_public` and `login_route_remains_public` tests as the explicit public-route checks.

- [ ] **Step 2: Run route tests to verify they fail**

Run:

```bash
cargo test -p api-server --manifest-path backend/Cargo.toml protected_api_route_rejects_missing_token -- --nocapture
cargo test -p api-server --manifest-path backend/Cargo.toml health_route_remains_public -- --nocapture
cargo test -p api-server --manifest-path backend/Cargo.toml login_route_remains_public -- --nocapture
```

Expected: FAIL because `ApiState.auth`, auth middleware, and route protection are not wired yet.

- [ ] **Step 3: Implement auth context and middleware**

Append this to `backend/crates/api-server/src/auth.rs`:

```rust
use axum::{
    extract::{Request, State},
    http::header,
    middleware::Next,
    response::Response,
};
use coin_listener_storage::repositories;

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub user_id: Uuid,
    pub tenant_id: Uuid,
    pub email: String,
}

pub fn bearer_token(headers: &axum::http::HeaderMap) -> AppResult<&str> {
    let value = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or(AppError::Unauthorized)?;

    value
        .strip_prefix("Bearer ")
        .filter(|token| !token.trim().is_empty())
        .ok_or(AppError::Unauthorized)
}

pub async fn require_auth(
    State(state): State<Arc<crate::ApiState>>,
    mut request: Request,
    next: Next,
) -> Result<Response, crate::ApiError> {
    let token = bearer_token(request.headers())?;
    let claims = validate_token(&state.auth, token)?;
    let user_id = claims.subject_uuid()?;
    let tenant_id = claims.tenant_uuid()?;
    repositories::active_tenant_membership(&state.postgres, user_id, tenant_id).await?;

    request.extensions_mut().insert(AuthContext {
        user_id,
        tenant_id,
        email: claims.email,
    });

    Ok(next.run(request).await)
}
```

Add missing imports at the top of `auth.rs`:

```rust
use std::sync::Arc;
```

- [ ] **Step 4: Wire public and protected routers**

In `backend/crates/api-server/src/routes.rs`, update imports:

```rust
use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use crate::auth::{self, AuthContext, TokenSettings};
```

Update `ApiState`:

```rust
#[derive(Clone)]
pub struct ApiState {
    pub postgres: PgPool,
    pub redis: Option<redis::Client>,
    pub scan_queue_key: String,
    pub notify_queue_key: String,
    pub enable_dev_routes: bool,
    pub auth: TokenSettings,
}
```

Replace `build_router` with a public router plus protected router:

```rust
pub fn build_router(state: Arc<ApiState>) -> Router {
    let protected = Router::new()
        .route("/api/system/status", get(system_status_handler))
        .route("/api/chains", get(list_chains))
        .route("/api/assets", get(list_assets))
        .route("/api/providers", get(list_providers).post(create_provider))
        .route("/api/addresses", get(list_addresses).post(create_address))
        .route(
            "/api/addresses/:id",
            put(update_address).delete(delete_address),
        )
        .route("/api/events", get(list_events))
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
        .route("/api/notification-outbox", get(list_notification_outbox))
        .route("/api/notification-outbox/:id", get(get_notification_outbox))
        .route(
            "/api/notification-outbox/:id/retry",
            post(retry_notification_outbox),
        )
        .route(
            "/api/notification-deliveries",
            get(list_notification_deliveries),
        );

    let protected = if state.enable_dev_routes {
        protected.route("/api/dev/scan-address/:id", post(scan_address))
    } else {
        protected
    }
    .route_layer(middleware::from_fn_with_state(
        Arc::clone(&state),
        auth::require_auth,
    ));

    Router::new()
        .route("/health", get(health))
        .route("/api/auth/login", post(login))
        .merge(protected)
        .with_state(state)
}
```

- [ ] **Step 5: Update login to verify hash and issue token**

In `backend/crates/api-server/src/routes.rs`, replace the login password/token logic:

```rust
let user = repositories::find_user_by_email(&state.postgres, &request.email).await?;
if !auth::verify_password(&request.password, &user.password_hash)? {
    return Err(AppError::Unauthorized.into());
}

let tenant = repositories::default_tenant_for_user(&state.postgres, user.id).await?;
let token = auth::issue_token(&state.auth, user.id, tenant.id, &user.email)?;
```

- [ ] **Step 6: Enforce authenticated tenant for address creation**

Change `create_address` signature in `backend/crates/api-server/src/routes.rs`:

```rust
async fn create_address(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Json(mut request): Json<CreateWatchedAddressRequest>,
) -> Result<Response, ApiError> {
    request.tenant_id = Some(auth.tenant_id);
    let address = repositories::create_watched_address(&state.postgres, request).await?;
    Ok((StatusCode::CREATED, Json(address)).into_response())
}
```

- [ ] **Step 7: Wire auth settings into binaries and env example**

In `backend/crates/api-server/src/main.rs`, change imports:

```rust
use api_server::{auth, build_router, ApiState};
```

Create auth settings after config load:

```rust
let config = AppConfig::from_env()?;
let auth_settings = auth::token_settings(
    config.auth.token_secret.clone(),
    config.auth.token_ttl_seconds,
)?;
```

Add `auth: auth_settings,` to `ApiState`.

In `backend/crates/all-in-one/src/main.rs`, change imports:

```rust
use api_server::{auth, build_router, ApiState};
```

Create auth settings after config load:

```rust
let config = AppConfig::from_env()?;
let auth_settings = auth::token_settings(
    config.auth.token_secret.clone(),
    config.auth.token_ttl_seconds,
)?;
```

Add `auth: auth_settings,` to `ApiState`.

In `.env.example`, add:

```dotenv
AUTH_TOKEN_SECRET=change-me-to-a-long-random-secret
AUTH_TOKEN_TTL_SECONDS=43200
```

- [ ] **Step 8: Run route tests to verify they pass**

Run:

```bash
cargo test -p api-server --manifest-path backend/Cargo.toml protected_api_route_rejects_missing_token -- --nocapture
cargo test -p api-server --manifest-path backend/Cargo.toml health_route_remains_public -- --nocapture
cargo test -p api-server --manifest-path backend/Cargo.toml login_route_remains_public -- --nocapture
cargo test -p api-server --manifest-path backend/Cargo.toml router_exposes -- --nocapture
```

Expected: PASS.

- [ ] **Step 9: Commit Task 3**

Run:

```bash
git add .env.example backend/crates/api-server/src/auth.rs backend/crates/api-server/src/routes.rs backend/crates/api-server/src/main.rs backend/crates/all-in-one/src/main.rs
git commit -m "Protect API routes with bearer auth"
```

## Task 4: Frontend session boundary and API client token handling

**Files:**
- Create: `frontend/src/auth/session.ts`
- Modify: `frontend/src/api/client.ts`

- [ ] **Step 1: Add frontend session helper**

Create `frontend/src/auth/session.ts`:

```ts
import type { LoginResponse } from '../api/types';

const SESSION_STORAGE_KEY = 'coin-listener.session.v1';
let currentSession: LoginResponse | null = null;
let unauthorizedHandler: (() => void) | null = null;

export function loadStoredSession(): LoginResponse | null {
  if (typeof window === 'undefined') return currentSession;

  const raw = window.localStorage.getItem(SESSION_STORAGE_KEY);
  if (!raw) {
    currentSession = null;
    return null;
  }

  try {
    currentSession = JSON.parse(raw) as LoginResponse;
    return currentSession;
  } catch {
    window.localStorage.removeItem(SESSION_STORAGE_KEY);
    currentSession = null;
    return null;
  }
}

export function saveSession(session: LoginResponse): void {
  currentSession = session;
  if (typeof window !== 'undefined') {
    window.localStorage.setItem(SESSION_STORAGE_KEY, JSON.stringify(session));
  }
}

export function clearSession(): void {
  currentSession = null;
  if (typeof window !== 'undefined') {
    window.localStorage.removeItem(SESSION_STORAGE_KEY);
  }
}

export function getAuthToken(): string | null {
  if (currentSession) return currentSession.token;
  return loadStoredSession()?.token ?? null;
}

export function setUnauthorizedHandler(handler: (() => void) | null): void {
  unauthorizedHandler = handler;
}

export function handleUnauthorized(): void {
  clearSession();
  unauthorizedHandler?.();
}
```

- [ ] **Step 2: Update API client to attach token and clear on 401**

Modify `frontend/src/api/client.ts` imports:

```ts
import { getAuthToken, handleUnauthorized } from '../auth/session';
```

Replace `request` with:

```ts
async function request<T>(path: string, options: RequestInit = {}): Promise<T> {
  const token = path === '/api/auth/login' ? null : getAuthToken();
  const headers = new Headers(options.headers);
  headers.set('Content-Type', 'application/json');
  if (token) {
    headers.set('Authorization', `Bearer ${token}`);
  }

  const response = await fetch(`${apiBaseUrl}${path}`, {
    ...options,
    headers,
  });

  if (!response.ok) {
    if (response.status === 401 && path !== '/api/auth/login') {
      handleUnauthorized();
    }
    const body = await response.json().catch(() => ({ error: response.statusText }));
    throw new Error(body.error ?? response.statusText);
  }

  if (response.status === 204) {
    return undefined as T;
  }

  return response.json();
}
```

- [ ] **Step 3: Run frontend build to verify TypeScript passes**

Run:

```bash
npm run build --prefix frontend
```

Expected: PASS. If it fails because TypeScript cannot resolve `../auth/session`, check that the new file path is exactly `frontend/src/auth/session.ts`.

- [ ] **Step 4: Commit Task 4**

Run:

```bash
git add frontend/src/auth/session.ts frontend/src/api/client.ts
git commit -m "Persist frontend auth token"
```

## Task 5: Frontend app session initialization, logout, and login UI cleanup

**Files:**
- Modify: `frontend/src/App.tsx`
- Modify: `frontend/src/pages/LoginPage.tsx`

- [ ] **Step 1: Update App session lifecycle**

Modify imports in `frontend/src/App.tsx`:

```tsx
import { useEffect, useState } from 'react';
import { loadStoredSession, saveSession, clearSession, setUnauthorizedHandler } from './auth/session';
```

Replace the session state line:

```tsx
const [session, setSession] = useState<LoginResponse | null>(() => loadStoredSession());
```

Add these functions inside `App` before the `if (!session)` block:

```tsx
useEffect(() => {
  setUnauthorizedHandler(() => setSession(null));
  return () => setUnauthorizedHandler(null);
}, []);

function handleLogin(nextSession: LoginResponse) {
  saveSession(nextSession);
  setSession(nextSession);
}

function handleLogout() {
  clearSession();
  setSession(null);
  setPage('dashboard');
}
```

Change the login render:

```tsx
return <LoginPage onLogin={handleLogin} />;
```

Change the header user display to include logout:

```tsx
<Space align="center">
  <Text type="tertiary">{session.user.display_name} / {session.tenant.name}</Text>
  <Button size="small" onClick={handleLogout}>退出登录</Button>
</Space>
```

- [ ] **Step 2: Remove default password display and prefill**

In `frontend/src/pages/LoginPage.tsx`, replace:

```tsx
<Text type="tertiary">默认账号：admin@example.com / admin</Text>
```

with:

```tsx
<Text type="tertiary">请输入账号密码登录</Text>
```

Replace the password input:

```tsx
<Form.Input field="password" label="密码" mode="password" rules={[{ required: true, message: '请输入密码' }]} />
```

Keep the email field initial value if desired:

```tsx
<Form.Input field="email" label="邮箱" initValue="admin@example.com" rules={[{ required: true, message: '请输入邮箱' }]} />
```

- [ ] **Step 3: Run frontend build**

Run:

```bash
npm run build --prefix frontend
```

Expected: PASS.

- [ ] **Step 4: Commit Task 5**

Run:

```bash
git add frontend/src/App.tsx frontend/src/pages/LoginPage.tsx
git commit -m "Add frontend logout flow"
```

## Task 6: Final verification and auth regression checks

**Files:**
- Verify only unless a previous task missed a required change.

- [ ] **Step 1: Run formatting**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: PASS. If it fails, run `cargo fmt --all --manifest-path backend/Cargo.toml`, inspect the diff, and re-run the check.

- [ ] **Step 2: Run full backend tests**

Run:

```bash
cargo test --workspace --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 3: Run frontend build**

Run:

```bash
npm run build --prefix frontend
```

Expected: PASS.

- [ ] **Step 4: Confirm no plaintext/dev-token auth remains**

Run:

```bash
git grep -n "dev-token\|password_hash != request.password\|默认账号：admin@example.com / admin" -- backend frontend
```

Expected: no matches.

- [ ] **Step 5: Confirm auth env is documented**

Run:

```bash
git grep -n "AUTH_TOKEN_SECRET\|AUTH_TOKEN_TTL_SECONDS" -- .env.example backend/crates/core/src/config.rs backend/crates/api-server/src/main.rs backend/crates/all-in-one/src/main.rs
```

Expected: matches in `.env.example`, config parsing, and both API-serving binaries.

- [ ] **Step 6: Commit final verification fixes if any**

If Steps 1-5 required fixes, stage only the files changed by those fixes and commit them with this message:

```bash
git status --short
git add backend/crates/api-server/src/auth.rs backend/crates/api-server/src/routes.rs backend/crates/api-server/src/main.rs backend/crates/all-in-one/src/main.rs backend/crates/core/src/config.rs backend/crates/core/src/error.rs backend/crates/core/src/lib.rs backend/crates/storage/src/repositories.rs backend/crates/storage/migrations/0012_auth_session_baseline.sql frontend/src/auth/session.ts frontend/src/api/client.ts frontend/src/App.tsx frontend/src/pages/LoginPage.tsx .env.example
git commit -m "Verify auth session hardening"
```

If `git status --short` shows that only a subset of those files changed, stage only that subset. If no fixes were needed, do not create an empty commit.

- [ ] **Step 7: Request final code review**

Dispatch a `superpowers:code-reviewer` agent with:

```text
Review the auth/session hardening implementation against docs/superpowers/specs/2026-05-19-coin-listener-auth-session-hardening-design.md and docs/superpowers/plans/2026-05-19-coin-listener-auth-session-hardening.md. Focus on: plaintext-password removal, JWT validation, protected route coverage, tenant context enforcement, frontend token handling, logout behavior, and migration safety. Report Critical/Important/Minor issues only.
```

Fix Critical and Important issues, then re-run the closest affected tests plus the full final verification commands.

## Self-review checklist

- Spec coverage: Tasks cover backend Argon2 verification, signed expiring tokens, auth config, route protection, tenant context, seed migration, frontend persistence, token attachment, 401 clearing, logout, login UI cleanup, and final verification.
- Placeholder scan: checked for disallowed placeholder phrases and removed actionable placeholder instructions from implementation steps.
- Type consistency: `TokenSettings`, `AuthClaims`, `AuthContext`, `AuthConfig`, `verify_password`, `issue_token`, `validate_token`, `token_settings`, `loadStoredSession`, `saveSession`, `clearSession`, `getAuthToken`, `setUnauthorizedHandler`, and `handleUnauthorized` are consistently named across tasks.
