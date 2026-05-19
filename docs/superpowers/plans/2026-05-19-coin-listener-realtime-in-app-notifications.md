# Realtime In-App Notifications Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add authenticated WebSocket delivery for newly created in-app notifications so online tenant users receive realtime notification events.

**Architecture:** API and all-in-one processes own an in-memory `RealtimeHub` keyed by tenant. Storage publishes PostgreSQL `NOTIFY` payloads after in-app notification commits; API/all-in-one listener tasks receive those payloads and broadcast JSON WebSocket messages to matching tenant connections. The frontend connects after login, shows Semi notifications, refreshes relevant TanStack Query caches, and disconnects on logout/session reset.

**Tech Stack:** Rust, axum WebSocket, tokio broadcast channels, SQLx PostgreSQL LISTEN/NOTIFY, existing JWT auth primitives, React, TypeScript, Vite, TanStack Query, Semi Design.

---

## File Map

- Create `backend/crates/api-server/src/realtime.rs`: realtime message types, WebSocket token query parsing, tenant hub, connection loop, PostgreSQL listener loop.
- Modify `backend/crates/api-server/src/lib.rs`: export the realtime module.
- Modify `backend/crates/api-server/src/routes.rs`: add `RealtimeHub` to `ApiState`, add `/api/realtime/notifications` WebSocket route, wire handler tests.
- Modify `backend/crates/api-server/src/main.rs`: create hub and spawn PostgreSQL realtime listener.
- Modify `backend/crates/all-in-one/src/main.rs`: create hub and spawn listener in single-process runtime.
- Modify `backend/crates/storage/src/notifications.rs`: add PostgreSQL notification channel/payload constants, publish helper, and publish after `create_sent_in_app_delivery` commits.
- Modify `frontend/src/api/types.ts`: add realtime message types.
- Create `frontend/src/realtime/notifications.ts`: WebSocket URL builder, parser, reconnecting client boundary.
- Modify `frontend/src/App.tsx`: connect realtime client while authenticated, show Semi notifications, invalidate queries, maintain unread badge.
- Verification: `cargo fmt --all --check --manifest-path backend/Cargo.toml`, `cargo test --workspace --manifest-path backend/Cargo.toml`, `npm run build --prefix frontend`.

---

### Task 1: Backend realtime hub and message contract

**Files:**
- Create: `backend/crates/api-server/src/realtime.rs`
- Modify: `backend/crates/api-server/src/lib.rs`
- Test: `backend/crates/api-server/src/realtime.rs`

- [ ] **Step 1: Write failing realtime unit tests**

Create `backend/crates/api-server/src/realtime.rs` with only imports needed for the tests and this test module:

```rust
#[cfg(test)]
mod tests {
    use super::{
        notification_message, realtime_token_from_query, RealtimeHub, RealtimeServerMessage,
    };
    use chrono::Utc;
    use coin_listener_core::models::InAppNotification;
    use uuid::Uuid;

    fn notification_for_tenant(tenant_id: Uuid) -> InAppNotification {
        InAppNotification {
            id: Uuid::from_u128(10),
            tenant_id,
            event_id: Uuid::from_u128(11),
            delivery_id: Some(Uuid::from_u128(12)),
            title: "transfer in".to_string(),
            body: "address: watched; asset: ETH; amount: 1; tx: 0x1".to_string(),
            read_at: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn realtime_token_query_extracts_non_empty_token() {
        assert_eq!(
            realtime_token_from_query("token=abc.def.ghi").expect("token present"),
            "abc.def.ghi"
        );
        assert!(realtime_token_from_query("token=").is_err());
        assert!(realtime_token_from_query("other=value").is_err());
    }

    #[test]
    fn notification_message_uses_stable_type() {
        let notification = notification_for_tenant(Uuid::from_u128(1));
        let message = notification_message(notification.clone());

        assert!(matches!(
            message,
            RealtimeServerMessage::InAppNotificationCreated(_)
        ));
        assert_eq!(message.message_type(), "in_app_notification.created");
        assert_eq!(message.tenant_id(), notification.tenant_id);
    }

    #[test]
    fn realtime_hub_broadcasts_only_to_matching_tenant() {
        let hub = RealtimeHub::new(16);
        let tenant_a = Uuid::from_u128(1);
        let tenant_b = Uuid::from_u128(2);
        let mut tenant_a_rx = hub.subscribe(tenant_a);
        let mut tenant_b_rx = hub.subscribe(tenant_b);
        let message = notification_message(notification_for_tenant(tenant_a));

        assert_eq!(hub.broadcast(message), 1);
        assert!(tenant_a_rx.try_recv().is_ok());
        assert!(tenant_b_rx.try_recv().is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p api-server --manifest-path backend/Cargo.toml realtime_ -- --nocapture
```

Expected: FAIL because `notification_message`, `realtime_token_from_query`, `RealtimeHub`, and `RealtimeServerMessage` are not implemented.

- [ ] **Step 3: Implement realtime module**

Replace `backend/crates/api-server/src/realtime.rs` with:

```rust
use axum::extract::ws::{Message, WebSocket};
use chrono::{DateTime, Utc};
use coin_listener_core::{models::InAppNotification, AppError, AppResult};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    sync::{broadcast, RwLock},
    time,
};
use uuid::Uuid;

pub const REALTIME_NOTIFICATION_CHANNEL: &str = "coin_listener_in_app_notifications";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum RealtimeServerMessage {
    #[serde(rename = "in_app_notification.created")]
    InAppNotificationCreated(InAppNotification),
    #[serde(rename = "ping")]
    Ping { sent_at: DateTime<Utc> },
}

impl RealtimeServerMessage {
    pub fn message_type(&self) -> &'static str {
        match self {
            Self::InAppNotificationCreated(_) => "in_app_notification.created",
            Self::Ping { .. } => "ping",
        }
    }

    pub fn tenant_id(&self) -> Uuid {
        match self {
            Self::InAppNotificationCreated(notification) => notification.tenant_id,
            Self::Ping { .. } => Uuid::nil(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RealtimeHub {
    inner: Arc<RwLock<HashMap<Uuid, broadcast::Sender<RealtimeServerMessage>>>>,
    buffer_size: usize,
}

impl RealtimeHub {
    pub fn new(buffer_size: usize) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            buffer_size,
        }
    }

    pub async fn subscribe_async(&self, tenant_id: Uuid) -> broadcast::Receiver<RealtimeServerMessage> {
        let mut inner = self.inner.write().await;
        inner
            .entry(tenant_id)
            .or_insert_with(|| broadcast::channel(self.buffer_size).0)
            .subscribe()
    }

    pub fn subscribe(&self, tenant_id: Uuid) -> broadcast::Receiver<RealtimeServerMessage> {
        let mut inner = self.inner.blocking_write();
        inner
            .entry(tenant_id)
            .or_insert_with(|| broadcast::channel(self.buffer_size).0)
            .subscribe()
    }

    pub fn broadcast(&self, message: RealtimeServerMessage) -> usize {
        let tenant_id = message.tenant_id();
        let inner = self.inner.blocking_read();
        inner
            .get(&tenant_id)
            .and_then(|sender| sender.send(message).ok())
            .unwrap_or(0)
    }

    pub async fn broadcast_async(&self, message: RealtimeServerMessage) -> usize {
        let tenant_id = message.tenant_id();
        let inner = self.inner.read().await;
        inner
            .get(&tenant_id)
            .and_then(|sender| sender.send(message).ok())
            .unwrap_or(0)
    }
}

pub fn notification_message(notification: InAppNotification) -> RealtimeServerMessage {
    RealtimeServerMessage::InAppNotificationCreated(notification)
}

pub fn realtime_token_from_query(query: &str) -> AppResult<&str> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find_map(|(key, value)| (key == "token" && !value.trim().is_empty()).then_some(value))
        .ok_or(AppError::Unauthorized)
}

pub async fn websocket_connection(socket: WebSocket, hub: RealtimeHub, tenant_id: Uuid) {
    let mut receiver = hub.subscribe_async(tenant_id).await;
    let (mut sender, mut socket_receiver) = socket.split();
    let mut heartbeat = time::interval(Duration::from_secs(30));

    loop {
        tokio::select! {
            message = receiver.recv() => {
                let Ok(message) = message else { continue; };
                let Ok(text) = serde_json::to_string(&message) else { continue; };
                if sender.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
            _ = heartbeat.tick() => {
                let message = RealtimeServerMessage::Ping { sent_at: Utc::now() };
                let Ok(text) = serde_json::to_string(&message) else { continue; };
                if sender.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
            incoming = socket_receiver.recv() => {
                if incoming.is_none() {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        notification_message, realtime_token_from_query, RealtimeHub, RealtimeServerMessage,
    };
    use chrono::Utc;
    use coin_listener_core::models::InAppNotification;
    use uuid::Uuid;

    fn notification_for_tenant(tenant_id: Uuid) -> InAppNotification {
        InAppNotification {
            id: Uuid::from_u128(10),
            tenant_id,
            event_id: Uuid::from_u128(11),
            delivery_id: Some(Uuid::from_u128(12)),
            title: "transfer in".to_string(),
            body: "address: watched; asset: ETH; amount: 1; tx: 0x1".to_string(),
            read_at: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn realtime_token_query_extracts_non_empty_token() {
        assert_eq!(
            realtime_token_from_query("token=abc.def.ghi").expect("token present"),
            "abc.def.ghi"
        );
        assert!(realtime_token_from_query("token=").is_err());
        assert!(realtime_token_from_query("other=value").is_err());
    }

    #[test]
    fn notification_message_uses_stable_type() {
        let notification = notification_for_tenant(Uuid::from_u128(1));
        let message = notification_message(notification.clone());

        assert!(matches!(
            message,
            RealtimeServerMessage::InAppNotificationCreated(_)
        ));
        assert_eq!(message.message_type(), "in_app_notification.created");
        assert_eq!(message.tenant_id(), notification.tenant_id);
    }

    #[test]
    fn realtime_hub_broadcasts_only_to_matching_tenant() {
        let hub = RealtimeHub::new(16);
        let tenant_a = Uuid::from_u128(1);
        let tenant_b = Uuid::from_u128(2);
        let mut tenant_a_rx = hub.subscribe(tenant_a);
        let mut tenant_b_rx = hub.subscribe(tenant_b);
        let message = notification_message(notification_for_tenant(tenant_a));

        assert_eq!(hub.broadcast(message), 1);
        assert!(tenant_a_rx.try_recv().is_ok());
        assert!(tenant_b_rx.try_recv().is_err());
    }
}
```

Modify `backend/crates/api-server/src/lib.rs`:

```rust
pub mod auth;
pub mod realtime;
pub mod routes;

pub use routes::{build_router, ApiState};
```

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
cargo test -p api-server --manifest-path backend/Cargo.toml realtime_ -- --nocapture
```

Expected: PASS for the three realtime unit tests.

- [ ] **Step 5: Commit**

```bash
git add backend/crates/api-server/src/lib.rs backend/crates/api-server/src/realtime.rs
git commit -m "Add realtime notification hub"
```

---

### Task 2: WebSocket route and auth handshake

**Files:**
- Modify: `backend/crates/api-server/src/realtime.rs`
- Modify: `backend/crates/api-server/src/routes.rs`
- Test: `backend/crates/api-server/src/routes.rs`

- [ ] **Step 1: Write failing route tests**

In `backend/crates/api-server/src/routes.rs`, update the tests module imports and `test_state_with_dev_routes` state creation:

```rust
use crate::{auth::TokenSettings, realtime::RealtimeHub};
```

Add `realtime: RealtimeHub::new(16),` in `ApiState` construction.

Add these tests to the same tests module:

```rust
#[tokio::test]
async fn realtime_websocket_rejects_missing_token() {
    let app = build_router(test_state());

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/realtime/notifications")
                .header(header::CONNECTION, "upgrade")
                .header(header::UPGRADE, "websocket")
                .header(header::SEC_WEBSOCKET_VERSION, "13")
                .header(header::SEC_WEBSOCKET_KEY, "dGhlIHNhbXBsZSBub25jZQ==")
                .body(Body::empty())
                .expect("valid request"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn realtime_websocket_rejects_malformed_token_before_database_lookup() {
    let app = build_router(test_state());

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/realtime/notifications?token=not-a-jwt")
                .header(header::CONNECTION, "upgrade")
                .header(header::UPGRADE, "websocket")
                .header(header::SEC_WEBSOCKET_VERSION, "13")
                .header(header::SEC_WEBSOCKET_KEY, "dGhlIHNhbXBsZSBub25jZQ==")
                .body(Body::empty())
                .expect("valid request"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p api-server --manifest-path backend/Cargo.toml realtime_websocket_ -- --nocapture
```

Expected: FAIL because `/api/realtime/notifications` is not registered and/or `ApiState` has no realtime hub field.

- [ ] **Step 3: Implement route handshake**

Modify imports at the top of `backend/crates/api-server/src/routes.rs`:

```rust
use crate::{
    auth::{self, AuthContext, TokenSettings},
    realtime::{self, RealtimeHub},
};
use axum::{
    extract::{ws::WebSocketUpgrade, Extension, Path, Query, State},
    http::{StatusCode, Uri},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
```

Also add `futures-util` to `backend/crates/api-server/Cargo.toml` because the WebSocket connection splits the socket stream:

```toml
futures-util = "0.3"
```

Add `pub realtime: RealtimeHub,` to `ApiState`.

Register the route before `.merge(protected)`:

```rust
Router::new()
    .route("/health", get(health))
    .route("/api/auth/login", post(login))
    .route("/api/realtime/notifications", get(realtime_notifications))
    .merge(protected)
    .with_state(state)
```

Add handler near `health`:

```rust
async fn realtime_notifications(
    State(state): State<Arc<ApiState>>,
    uri: Uri,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let query = uri.query().unwrap_or_default();
    let token = realtime::realtime_token_from_query(query)?;
    let claims = auth::validate_token(&state.auth, token)?;
    let user_id = claims.subject_uuid()?;
    let tenant_id = claims.tenant_uuid()?;
    repositories::active_user(&state.postgres, user_id).await?;
    repositories::active_tenant_membership(&state.postgres, user_id, tenant_id).await?;
    let hub = state.realtime.clone();

    Ok(ws
        .on_upgrade(move |socket| realtime::websocket_connection(socket, hub, tenant_id))
        .into_response())
}
```

Update `test_state_with_dev_routes` to include:

```rust
realtime: RealtimeHub::new(16),
```

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
cargo test -p api-server --manifest-path backend/Cargo.toml realtime_websocket_ -- --nocapture
```

Expected: PASS for missing and malformed token WebSocket route tests.

- [ ] **Step 5: Commit**

```bash
git add backend/crates/api-server/Cargo.toml backend/Cargo.lock backend/crates/api-server/src/realtime.rs backend/crates/api-server/src/routes.rs
git commit -m "Add authenticated realtime websocket route"
```

---

### Task 3: Publish in-app notification database events

**Files:**
- Modify: `backend/crates/storage/src/notifications.rs`
- Test: `backend/crates/storage/src/notifications.rs`

- [ ] **Step 1: Write failing storage tests**

Add constants import in the tests module `use super::{ ... }` list:

```rust
IN_APP_NOTIFICATION_NOTIFY_CHANNEL, IN_APP_NOTIFICATION_NOTIFY_QUERY,
```

Add these tests. The source-order test intentionally searches for the final publish call signature so it does not match the helper definition before the transaction commit:

```rust
#[test]
fn in_app_notification_notify_channel_is_stable() {
    assert_eq!(
        IN_APP_NOTIFICATION_NOTIFY_CHANNEL,
        "coin_listener_in_app_notifications"
    );
    assert!(IN_APP_NOTIFICATION_NOTIFY_QUERY.contains("pg_notify"));
    assert!(IN_APP_NOTIFICATION_NOTIFY_QUERY.contains(IN_APP_NOTIFICATION_NOTIFY_CHANNEL));
}

#[test]
fn create_sent_in_app_delivery_publishes_after_commit() {
    let source = include_str!("notifications.rs");
    let commit_index = source
        .rfind(".commit()")
        .expect("in-app transaction commit is present");
    let notify_index = source
        .find("publish_in_app_notification_created(pool, &notification)")
        .expect("publish helper is called after commit");

    assert!(notify_index > commit_index);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p coin-listener-storage --manifest-path backend/Cargo.toml in_app_notification_notify -- --nocapture
```

Expected: FAIL because notification channel/query constants and publish helper do not exist.

- [ ] **Step 3: Implement database notification publishing**

Add constants near existing notification constants:

```rust
pub const IN_APP_NOTIFICATION_NOTIFY_CHANNEL: &str = "coin_listener_in_app_notifications";
pub const IN_APP_NOTIFICATION_NOTIFY_QUERY: &str = "SELECT pg_notify($1, $2)";
```

Add helper after `create_in_app_notification`:

```rust
pub async fn publish_in_app_notification_created(
    pool: &PgPool,
    notification: &InAppNotification,
) -> AppResult<()> {
    let payload = serde_json::to_string(notification)
        .map_err(|error| AppError::Validation(error.to_string()))?;

    sqlx::query(IN_APP_NOTIFICATION_NOTIFY_QUERY)
        .bind(IN_APP_NOTIFICATION_NOTIFY_CHANNEL)
        .bind(payload)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    Ok(())
}
```

Add `tracing` to `backend/crates/storage/Cargo.toml` if it is not already present:

```toml
tracing.workspace = true
```

Add `warn` near the top of `backend/crates/storage/src/notifications.rs`:

```rust
use tracing::warn;
```

Modify the tail of `create_sent_in_app_delivery` so publish happens after commit and does not make a successfully written notification retry as failed work:

```rust
transaction
    .commit()
    .await
    .map_err(|error| AppError::Database(error.to_string()))?;

if let Err(error) = publish_in_app_notification_created(pool, &notification).await {
    warn!(%error, notification_id = %notification.id, "in-app notification realtime publish failed");
}

Ok((delivery, notification))
```

Update the tests module import list to include the two constants.

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
cargo test -p coin-listener-storage --manifest-path backend/Cargo.toml in_app_notification_notify -- --nocapture
```

Expected: PASS for both notification publishing tests.

- [ ] **Step 5: Commit**

```bash
git add backend/crates/storage/Cargo.toml backend/crates/storage/src/notifications.rs
git commit -m "Publish realtime in-app notification events"
```

---

### Task 4: PostgreSQL realtime listener startup

**Files:**
- Modify: `backend/crates/api-server/src/realtime.rs`
- Modify: `backend/crates/api-server/src/main.rs`
- Modify: `backend/crates/all-in-one/src/main.rs`
- Modify: `backend/crates/api-server/src/routes.rs`
- Test: `backend/crates/api-server/src/realtime.rs`
- Test: `backend/crates/all-in-one/src/main.rs`

- [ ] **Step 1: Write failing listener tests**

Add this test to `backend/crates/api-server/src/realtime.rs` tests module:

```rust
#[test]
fn realtime_notify_payload_deserializes_to_broadcast_message() {
    let notification = notification_for_tenant(Uuid::from_u128(1));
    let payload = serde_json::to_string(&notification).expect("notification serializes");
    let message = super::message_from_notify_payload(&payload).expect("payload parses");

    assert_eq!(message.message_type(), "in_app_notification.created");
    assert_eq!(message.tenant_id(), notification.tenant_id);
}
```

Add this test to `backend/crates/all-in-one/src/main.rs` tests module:

```rust
#[test]
fn all_in_one_wires_realtime_listener() {
    let source = include_str!("main.rs");

    assert!(source.contains("run_realtime_notification_listener"));
    assert!(source.contains("realtime_handle"));
    assert!(source.contains("RealtimeHub::new"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p api-server -p all-in-one --manifest-path backend/Cargo.toml realtime_listener -- --nocapture
```

Expected: FAIL because `message_from_notify_payload`, `run_realtime_notification_listener`, and startup wiring are missing.

- [ ] **Step 3: Implement listener helper and startup wiring**

Add to `backend/crates/api-server/src/realtime.rs`:

```rust
use sqlx::postgres::PgListener;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::warn;

pub fn message_from_notify_payload(payload: &str) -> AppResult<RealtimeServerMessage> {
    let notification = serde_json::from_str::<InAppNotification>(payload)
        .map_err(|error| AppError::Validation(error.to_string()))?;
    Ok(notification_message(notification))
}

pub async fn run_realtime_notification_listener(
    database_url: String,
    hub: RealtimeHub,
    shutdown: Arc<AtomicBool>,
) {
    while !shutdown.load(Ordering::Relaxed) {
        match PgListener::connect(&database_url).await {
            Ok(mut listener) => {
                if let Err(error) = listener.listen(REALTIME_NOTIFICATION_CHANNEL).await {
                    warn!(%error, "realtime notification listen failed");
                    time::sleep(Duration::from_secs(1)).await;
                    continue;
                }

                while !shutdown.load(Ordering::Relaxed) {
                    match listener.recv().await {
                        Ok(notification) => match message_from_notify_payload(notification.payload()) {
                            Ok(message) => {
                                hub.broadcast_async(message).await;
                            }
                            Err(error) => warn!(%error, "invalid realtime notification payload"),
                        },
                        Err(error) => {
                            warn!(%error, "realtime notification listener disconnected");
                            break;
                        }
                    }
                }
            }
            Err(error) => warn!(%error, "realtime notification listener connect failed"),
        }

        if !shutdown.load(Ordering::Relaxed) {
            time::sleep(Duration::from_secs(1)).await;
        }
    }
}
```

Modify `backend/crates/api-server/src/main.rs` imports:

```rust
use api_server::{auth, build_router, realtime, ApiState};
```

Before `let state = Arc::new(ApiState { ... })`, add:

```rust
let realtime_hub = realtime::RealtimeHub::new(256);
let realtime_shutdown = Arc::clone(&shutdown);
tokio::spawn(realtime::run_realtime_notification_listener(
    config.postgres.database_url.clone(),
    realtime_hub.clone(),
    realtime_shutdown,
));
```

Add `realtime: realtime_hub,` to `ApiState`.

Modify `backend/crates/all-in-one/src/main.rs` imports:

```rust
use api_server::{auth, build_router, realtime, ApiState};
```

Before `let api_state = Arc::new(ApiState { ... })`, add:

```rust
let realtime_hub = realtime::RealtimeHub::new(256);
let realtime_handle = tokio::spawn(realtime::run_realtime_notification_listener(
    config.postgres.database_url.clone(),
    realtime_hub.clone(),
    Arc::clone(&shutdown),
));
```

Add `realtime: realtime_hub,` to `ApiState`.

Update the all-in-one runtime enum:

```rust
enum RuntimeEvent {
    Shutdown,
    Server(anyhow::Result<()>),
    Scheduler(anyhow::Result<()>),
    Worker(anyhow::Result<()>),
    Notifier(anyhow::Result<()>),
    Realtime(anyhow::Result<()>),
}
```

Update `tokio::select!` to watch the realtime task:

```rust
result = &mut realtime_handle => RuntimeEvent::Realtime(log_realtime_result("realtime", realtime_task_result("realtime", result))),
```

Add a realtime shutdown helper because the realtime listener returns `()` instead of `AppResult<()>`:

```rust
fn realtime_task_result(
    service: &'static str,
    result: Result<(), tokio::task::JoinError>,
) -> anyhow::Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(error) => Err(anyhow::anyhow!("{service} task failed: {error}")),
    }
}

async fn wait_for_realtime_shutdown(
    service: &'static str,
    handle: JoinHandle<()>,
) -> anyhow::Result<()> {
    log_realtime_result(service, realtime_task_result(service, handle.await))
}

fn log_realtime_result(service: &'static str, result: anyhow::Result<()>) -> anyhow::Result<()> {
    if let Err(error) = &result {
        error!(service, error = %error, "all-in-one service stopped with error");
    }
    result
}
```

Include `wait_for_realtime_shutdown("realtime", realtime_handle).await` in each shutdown/error collection branch where scheduler/worker/notifier handles are collected, except the new `RuntimeEvent::Realtime(result)` branch. Add the `RuntimeEvent::Realtime(result)` branch to shut down server, scheduler, worker, notifier, and heartbeats, then call `preserve_primary_result(result, secondary)`.

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
cargo test -p api-server -p all-in-one --manifest-path backend/Cargo.toml realtime_listener -- --nocapture
```

Expected: PASS for payload parsing and all-in-one wiring tests.

- [ ] **Step 5: Commit**

```bash
git add backend/crates/api-server/src/realtime.rs backend/crates/api-server/src/main.rs backend/crates/api-server/src/routes.rs backend/crates/all-in-one/src/main.rs
git commit -m "Start realtime notification listeners"
```

---

### Task 5: Frontend realtime client boundary

**Files:**
- Modify: `frontend/src/api/types.ts`
- Create: `frontend/src/realtime/notifications.ts`
- Test: `frontend/src/realtime/notifications.ts`

- [ ] **Step 1: Add realtime message types**

Add to `frontend/src/api/types.ts`:

```ts
export type RealtimeNotificationCreatedMessage = {
  type: 'in_app_notification.created';
  payload: InAppNotification;
};

export type RealtimePingMessage = {
  type: 'ping';
  payload: { sent_at: string };
};

export type RealtimeServerMessage = RealtimeNotificationCreatedMessage | RealtimePingMessage;
```

- [ ] **Step 2: Run build to verify the type addition is safe**

Run:

```bash
npm run build --prefix frontend
```

Expected: PASS because only exported types were added. Existing Vite warnings about `lottie-web` eval and chunk size are acceptable.

- [ ] **Step 3: Implement pure frontend helpers and reconnecting client**

Replace `frontend/src/realtime/notifications.ts` with:

```ts
import type { InAppNotification, LoginResponse, RealtimeServerMessage } from '../api/types';

const REALTIME_PATH = '/api/realtime/notifications';
const MAX_RECONNECT_DELAY_MS = 30_000;

type RealtimeNotificationMessage = Extract<RealtimeServerMessage, { type: 'in_app_notification.created' }>;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isString(value: unknown): value is string {
  return typeof value === 'string';
}

function isNotification(value: unknown): value is InAppNotification {
  return (
    isRecord(value) &&
    isString(value.id) &&
    isString(value.tenant_id) &&
    isString(value.event_id) &&
    isString(value.title) &&
    isString(value.body) &&
    isString(value.created_at)
  );
}

export function realtimeWebSocketUrl(apiBaseUrl: string, token: string): string {
  const base = apiBaseUrl || window.location.origin;
  const url = new URL(REALTIME_PATH, base);
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
  url.searchParams.set('token', token);
  return url.toString();
}

export function parseRealtimeMessage(raw: string): RealtimeServerMessage | null {
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (!isRecord(parsed) || !isString(parsed.type)) return null;

    if (parsed.type === 'in_app_notification.created' && isNotification(parsed.payload)) {
      return parsed as RealtimeNotificationMessage;
    }

    if (
      parsed.type === 'ping' &&
      isRecord(parsed.payload) &&
      isString(parsed.payload.sent_at)
    ) {
      return parsed as RealtimeServerMessage;
    }

    return null;
  } catch {
    return null;
  }
}

export function reconnectDelayMs(attempt: number): number {
  return Math.min(1000 * 2 ** Math.max(0, attempt), MAX_RECONNECT_DELAY_MS);
}

export type RealtimeNotificationHandlers = {
  onNotification: (notification: InAppNotification) => void;
  onUnauthorized?: () => void;
};

export type RealtimeConnectOptions = {
  apiBaseUrl?: string;
  getGeneration?: () => number;
  generation?: number;
};

export function connectRealtimeNotifications(
  session: LoginResponse,
  handlers: RealtimeNotificationHandlers,
  options: RealtimeConnectOptions = {},
): () => void {
  let stopped = false;
  let attempt = 0;
  let socket: WebSocket | null = null;
  let reconnectTimer: number | null = null;
  const apiBaseUrl = options.apiBaseUrl ?? import.meta.env.VITE_API_BASE_URL ?? '';
  const initialGeneration = options.generation;

  const isStale = () =>
    initialGeneration !== undefined &&
    options.getGeneration !== undefined &&
    options.getGeneration() !== initialGeneration;

  const cleanupTimer = () => {
    if (reconnectTimer !== null) {
      window.clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
  };

  const connect = () => {
    if (stopped || isStale()) return;
    socket = new WebSocket(realtimeWebSocketUrl(apiBaseUrl, session.token));

    socket.onopen = () => {
      attempt = 0;
    };

    socket.onmessage = event => {
      if (typeof event.data !== 'string') return;
      const message = parseRealtimeMessage(event.data);
      if (message?.type === 'in_app_notification.created') {
        handlers.onNotification(message.payload);
      }
    };

    socket.onclose = event => {
      if (stopped || isStale()) return;
      if (event.code === 1008) {
        handlers.onUnauthorized?.();
        return;
      }
      const delay = reconnectDelayMs(attempt);
      attempt += 1;
      cleanupTimer();
      reconnectTimer = window.setTimeout(connect, delay);
    };

    socket.onerror = () => {
      socket?.close();
    };
  };

  connect();

  return () => {
    stopped = true;
    cleanupTimer();
    socket?.close();
    socket = null;
  };
}
```

- [ ] **Step 4: Run frontend build**

Run:

```bash
npm run build --prefix frontend
```

Expected: PASS, with existing Vite warnings about `lottie-web` eval and chunk size allowed.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/api/types.ts frontend/src/realtime/notifications.ts
git commit -m "Add realtime notification client"
```

---

### Task 6: Frontend app integration and unread badge

**Files:**
- Modify: `frontend/src/App.tsx`
- Modify: `frontend/src/auth/session.ts`
- Modify: `frontend/src/pages/InAppNotificationsPage.tsx`
- Test: frontend production build

- [ ] **Step 1: Write failing integration change**

Modify `frontend/src/App.tsx` imports to include `Notification` and realtime client before implementing missing symbols:

```ts
import { Banner, Button, Card, Layout, Nav, Notification, Space, Tag, Typography } from '@douyinfe/semi-ui';
import { connectRealtimeNotifications } from './realtime/notifications';
```

Add this state near the page state:

```ts
const [realtimeUnreadCount, setRealtimeUnreadCount] = useState(0);
```

- [ ] **Step 2: Run build to verify it fails or exposes missing integration work**

Run:

```bash
npm run build --prefix frontend
```

Expected: FAIL if imports or state are not yet used under current TypeScript settings, or PASS with no behavior yet. Do not stop here; continue with minimal integration.

- [ ] **Step 3: Implement realtime connection in App**

Update `frontend/src/App.tsx` imports:

```ts
import { Banner, Button, Card, Layout, Nav, Notification, Space, Tag, Typography } from '@douyinfe/semi-ui';
import { connectRealtimeNotifications } from './realtime/notifications';
```

Also import `getSessionGeneration` from the session boundary:

```ts
import { clearSession, getSessionGeneration, loadStoredSession, saveSession, setUnauthorizedHandler } from './auth/session';
```

If `getSessionGeneration` does not exist yet, add it to `frontend/src/auth/session.ts`:

```ts
export function getSessionGeneration(): number {
  return sessionGeneration;
}
```

Add after unauthorized handler effect:

```ts
useEffect(() => {
  if (!session) return undefined;

  const generation = getSessionGeneration();
  return connectRealtimeNotifications(
    session,
    {
      onNotification: notification => {
        setRealtimeUnreadCount(count => count + 1);
        Notification.info({
          title: notification.title,
          content: notification.body,
        });
        queryClient.invalidateQueries({ queryKey: ['in-app-notifications'] });
        queryClient.invalidateQueries({ queryKey: ['events'] });
        queryClient.invalidateQueries({ queryKey: ['system-status'] });
      },
      onUnauthorized: resetAuthenticatedState,
    },
    { generation, getGeneration: getSessionGeneration },
  );
}, [queryClient, resetAuthenticatedState, session]);
```

Update `resetAuthenticatedState`:

```ts
const resetAuthenticatedState = useCallback(() => {
  queryClient.clear();
  setPage('dashboard');
  setSession(null);
  setRealtimeUnreadCount(0);
}, [queryClient]);
```

Update the in-app nav item:

```tsx
{
  itemKey: 'in-app-notifications',
  text: realtimeUnreadCount > 0 ? `站内通知 (${realtimeUnreadCount})` : '站内通知',
  icon: <IconBell />,
},
```

When rendering the in-app page, reset the local realtime count:

```ts
if (page === 'in-app-notifications') {
  return <InAppNotificationsPage onUnreadSettled={setRealtimeUnreadCount} />;
}
```

- [ ] **Step 4: Update InAppNotificationsPage to sync count**

Modify signature and effect:

```ts
import { useEffect, useState } from 'react';
```

```ts
type InAppNotificationsPageProps = {
  onUnreadSettled?: (count: number) => void;
};

export function InAppNotificationsPage({ onUnreadSettled }: InAppNotificationsPageProps) {
```

Add after the query:

```ts
useEffect(() => {
  if (!notificationsQuery.data) return;
  onUnreadSettled?.(notificationsQuery.data.filter(notification => !notification.read_at).length);
}, [notificationsQuery.data, onUnreadSettled]);
```

- [ ] **Step 5: Run frontend build**

Run:

```bash
npm run build --prefix frontend
```

Expected: PASS, with existing Vite warnings about `lottie-web` eval and chunk size allowed.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/App.tsx frontend/src/auth/session.ts frontend/src/pages/InAppNotificationsPage.tsx
git commit -m "Wire realtime notifications into app shell"
```

---

### Task 7: Final verification and regression checks

**Files:**
- Verify full repository state only.

- [ ] **Step 1: Run Rust formatting check**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: PASS with no output.

- [ ] **Step 2: Run backend workspace tests**

Run:

```bash
cargo test --workspace --manifest-path backend/Cargo.toml
```

Expected: PASS for all backend crate tests. Existing `sqlx-postgres v0.7.4` future incompatibility warning is acceptable.

- [ ] **Step 3: Run frontend production build**

Run:

```bash
npm run build --prefix frontend
```

Expected: PASS. Existing Vite warnings about `lottie-web` direct eval and large chunks are acceptable.

- [ ] **Step 4: Run realtime regression grep**

Run:

```bash
git grep -n -E "ws://localhost|dev-token|默认账号：admin@example.com / admin|password_hash != request.password" -- backend frontend || true
```

Expected: no output.

- [ ] **Step 5: Check git status**

Run:

```bash
git status --short
```

Expected: only intended realtime notification files are committed; no unrelated `frontend/package-lock.json` changes are staged.

- [ ] **Step 6: Commit final fixes if verification required any code changes**

If formatting or tests required edits, commit them:

```bash
git add <changed-files>
git commit -m "Stabilize realtime notification verification"
```

Expected: no commit needed if earlier tasks were clean.
