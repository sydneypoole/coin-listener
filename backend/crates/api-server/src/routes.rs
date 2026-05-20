use crate::{
    auth::{self, AuthContext, TokenSettings},
    realtime::{self, RealtimeHub},
};
use axum::{
    extract::{ws::WebSocketUpgrade, Extension, FromRequestParts, Path, Query, Request, State},
    http::{StatusCode, Uri},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{Duration, Utc};
use coin_listener_chain_providers::evm::EvmRpcClient;
use coin_listener_core::{
    models::{
        CreateNotificationChannelRequest, CreateNotificationRuleRequest, CreateProviderRequest,
        CreateWatchedAddressRequest, EventQuery, InAppNotificationQuery, LoginRequest,
        LoginResponse, NotificationDeliveryListResponse, NotificationDeliveryQuery,
        NotificationOutboxListResponse, NotificationOutboxQuery, QueueStatus,
        RetryNotificationOutboxResponse, SystemStatus, UserSummary,
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
use serde::Serialize;
use sqlx::PgPool;
use std::{sync::Arc, time::Duration as StdDuration};
use uuid::Uuid;

#[derive(Clone)]
pub struct ApiState {
    pub postgres: PgPool,
    pub redis: Option<redis::Client>,
    pub scan_queue_key: String,
    pub notify_queue_key: String,
    pub enable_dev_routes: bool,
    pub auth: TokenSettings,
    pub realtime: RealtimeHub,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub service: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct ProviderTestResponse {
    pub ok: bool,
    pub message: String,
    pub latest_block: Option<i64>,
}

pub fn build_router(state: Arc<ApiState>) -> Router {
    let protected = Router::new()
        .route("/api/system/status", get(system_status_handler))
        .route("/api/chains", get(list_chains))
        .route("/api/assets", get(list_assets))
        .route("/api/providers", get(list_providers).post(create_provider))
        .route("/api/providers/:id", put(update_provider))
        .route("/api/providers/:id/test", post(test_provider))
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
        .route("/api/realtime/notifications", get(realtime_notifications))
        .merge(protected)
        .with_state(state)
}

async fn health(State(_state): State<Arc<ApiState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        service: "api-server",
    })
}

async fn realtime_notifications(
    State(state): State<Arc<ApiState>>,
    uri: Uri,
    request: Request,
) -> Result<Response, ApiError> {
    let query = uri.query().unwrap_or_default();
    let token = realtime::realtime_token_from_query(query)?;
    let claims = auth::validate_token(&state.auth, token)?;
    let user_id = claims.subject_uuid()?;
    let tenant_id = claims.tenant_uuid()?;
    repositories::active_user(&state.postgres, user_id).await?;
    repositories::active_tenant_membership(&state.postgres, user_id, tenant_id).await?;
    let hub = state.realtime.clone();
    let (mut parts, _) = request.into_parts();
    let ws = match WebSocketUpgrade::from_request_parts(&mut parts, &state).await {
        Ok(ws) => ws,
        Err(rejection) => return Ok(rejection.into_response()),
    };

    Ok(ws
        .on_upgrade(move |socket| realtime::websocket_connection(socket, hub, tenant_id))
        .into_response())
}

async fn system_status_handler(State(state): State<Arc<ApiState>>) -> Result<Response, ApiError> {
    let queues = queue_status(&state).await;
    let scans = system_status::system_scan_status(&state.postgres).await?;
    let events = system_status::system_event_status(&state.postgres).await?;
    let notifications = system_status::system_notification_status(&state.postgres).await?;
    let providers = system_status::system_provider_status(&state.postgres).await?;
    let now = Utc::now();
    let services =
        coin_listener_storage::service_heartbeats::system_service_health(&state.postgres, now)
            .await?;

    Ok(Json(SystemStatus {
        generated_at: now,
        queues,
        scans,
        events,
        notifications,
        providers,
        services,
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
                    Err(error) => {
                        queue_errors.push(format!("scan queue depth unavailable: {error}"))
                    }
                }
            }
            Err(error) => queue_errors.push(format!("scan queue redis unavailable: {error}")),
        }

        match connect_notify_queue(redis_client).await {
            Ok(mut connection) => {
                let queue = NotifyQueue::new(state.notify_queue_key.clone());
                match queue.depth(&mut connection).await {
                    Ok(depth) => notify_queue_depth = Some(depth),
                    Err(error) => {
                        queue_errors.push(format!("notify queue depth unavailable: {error}"))
                    }
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

async fn login(
    State(state): State<Arc<ApiState>>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    let user = repositories::find_user_by_email(&state.postgres, &request.email).await?;
    if !auth::verify_password(&request.password, &user.password_hash)? {
        return Err(AppError::Unauthorized.into());
    }

    let tenant = repositories::default_tenant_for_user(&state.postgres, user.id).await?;
    let token = auth::issue_token(&state.auth, user.id, tenant.id, &user.email)?;

    Ok(Json(LoginResponse {
        token,
        user: UserSummary {
            id: user.id,
            email: user.email,
            display_name: user.display_name,
        },
        tenant,
    }))
}

async fn list_chains(State(state): State<Arc<ApiState>>) -> Result<Response, ApiError> {
    let chains = repositories::list_chains(&state.postgres).await?;
    Ok(Json(chains).into_response())
}

async fn list_assets(State(state): State<Arc<ApiState>>) -> Result<Response, ApiError> {
    let assets = repositories::list_assets(&state.postgres).await?;
    Ok(Json(assets).into_response())
}

async fn list_providers(State(state): State<Arc<ApiState>>) -> Result<Response, ApiError> {
    let providers = repositories::list_providers(&state.postgres).await?;
    Ok(Json(providers).into_response())
}

async fn create_provider(
    State(state): State<Arc<ApiState>>,
    Json(request): Json<CreateProviderRequest>,
) -> Result<Response, ApiError> {
    let provider = repositories::create_provider(&state.postgres, request).await?;
    Ok((StatusCode::CREATED, Json(provider)).into_response())
}

async fn update_provider(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<Uuid>,
    Json(request): Json<CreateProviderRequest>,
) -> Result<Response, ApiError> {
    let provider = repositories::update_provider(&state.postgres, id, request).await?;
    Ok(Json(provider).into_response())
}

async fn test_provider(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let provider = repositories::get_provider(&state.postgres, id).await?;
    if provider.provider_type != "rpc" {
        return Err(AppError::Validation("only rpc providers can be tested".to_string()).into());
    }

    let timeout_ms = u64::try_from(provider.timeout_ms)
        .map_err(|_| AppError::Validation("timeout_ms must be positive".to_string()))?;
    if timeout_ms == 0 {
        return Err(AppError::Validation("timeout_ms must be positive".to_string()).into());
    }

    let chain = repositories::chain_by_id(&state.postgres, provider.chain_id).await?;
    if chain.chain_type != "evm" {
        return Err(AppError::Validation(
            "provider connectivity test currently supports EVM RPC only".to_string(),
        )
        .into());
    }

    let client = EvmRpcClient::new(provider.base_url, StdDuration::from_millis(timeout_ms));
    let latest_block = client.eth_block_number().await?;
    Ok(Json(ProviderTestResponse {
        ok: true,
        message: "provider rpc reachable".to_string(),
        latest_block: Some(latest_block),
    })
    .into_response())
}

async fn list_addresses(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Response, ApiError> {
    let addresses = repositories::list_watched_addresses(&state.postgres, auth.tenant_id).await?;
    Ok(Json(addresses).into_response())
}

async fn create_address(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Json(mut request): Json<CreateWatchedAddressRequest>,
) -> Result<Response, ApiError> {
    request.tenant_id = Some(auth.tenant_id);
    let address = repositories::create_watched_address(&state.postgres, request).await?;
    Ok((StatusCode::CREATED, Json(address)).into_response())
}

async fn update_address(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(request): Json<CreateWatchedAddressRequest>,
) -> Result<Response, ApiError> {
    let address =
        repositories::update_watched_address(&state.postgres, auth.tenant_id, id, request).await?;
    Ok(Json(address).into_response())
}

async fn delete_address(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    repositories::delete_watched_address(&state.postgres, auth.tenant_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_events(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<EventQuery>,
) -> Result<Response, ApiError> {
    let events = repositories::list_events(&state.postgres, auth.tenant_id, query).await?;
    Ok(Json(events).into_response())
}

async fn scan_address(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let event = repositories::create_mock_evm_event(&state.postgres, auth.tenant_id, id).await?;
    Ok((StatusCode::CREATED, Json(event)).into_response())
}

async fn list_notification_channels(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Response, ApiError> {
    let channels =
        notifications::list_notification_channels(&state.postgres, auth.tenant_id).await?;
    Ok(Json(channels).into_response())
}

async fn create_notification_channel(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<CreateNotificationChannelRequest>,
) -> Result<Response, ApiError> {
    let channel =
        notifications::create_notification_channel(&state.postgres, auth.tenant_id, request)
            .await?;
    Ok((StatusCode::CREATED, Json(channel)).into_response())
}

async fn list_notification_rules(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Response, ApiError> {
    let rules = notifications::list_notification_rules(&state.postgres, auth.tenant_id).await?;
    Ok(Json(rules).into_response())
}

async fn create_notification_rule(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<CreateNotificationRuleRequest>,
) -> Result<Response, ApiError> {
    let rule =
        notifications::create_notification_rule(&state.postgres, auth.tenant_id, request).await?;
    Ok((StatusCode::CREATED, Json(rule)).into_response())
}

async fn update_notification_rule(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(request): Json<CreateNotificationRuleRequest>,
) -> Result<Response, ApiError> {
    let rule =
        notifications::update_notification_rule(&state.postgres, auth.tenant_id, id, request)
            .await?;
    Ok(Json(rule).into_response())
}

async fn delete_notification_rule(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    notifications::delete_notification_rule(&state.postgres, auth.tenant_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_in_app_notifications(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<InAppNotificationQuery>,
) -> Result<Response, ApiError> {
    let notifications =
        notifications::list_in_app_notifications(&state.postgres, auth.tenant_id, query).await?;
    Ok(Json(notifications).into_response())
}

async fn mark_in_app_notification_read(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let notification =
        notifications::mark_in_app_notification_read(&state.postgres, auth.tenant_id, id).await?;
    Ok(Json(notification).into_response())
}

fn notification_ops_stale_before() -> chrono::DateTime<Utc> {
    Utc::now() - Duration::minutes(15)
}

async fn list_notification_outbox(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<NotificationOutboxQuery>,
) -> Result<Response, ApiError> {
    let limit = repositories::notification_ops_limit(query.limit);
    let offset = repositories::notification_ops_offset(query.offset);
    let items = repositories::list_notification_outbox(
        &state.postgres,
        auth.tenant_id,
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
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let detail = repositories::get_notification_outbox_detail(
        &state.postgres,
        auth.tenant_id,
        id,
        notification_ops_stale_before(),
    )
    .await?;
    Ok(Json(detail).into_response())
}

async fn retry_notification_outbox(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let outbox =
        repositories::retry_notification_outbox(&state.postgres, auth.tenant_id, id, Utc::now())
            .await?;
    Ok(Json(RetryNotificationOutboxResponse { outbox }).into_response())
}

async fn list_notification_deliveries(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<NotificationDeliveryQuery>,
) -> Result<Response, ApiError> {
    let limit = notifications::notification_delivery_ops_limit(query.limit);
    let offset = notifications::notification_delivery_ops_offset(query.offset);
    let items =
        notifications::list_notification_deliveries(&state.postgres, auth.tenant_id, query).await?;

    Ok(Json(NotificationDeliveryListResponse {
        items,
        limit,
        offset,
    })
    .into_response())
}

pub struct ApiError(AppError);

impl From<AppError> for ApiError {
    fn from(error: AppError) -> Self {
        Self(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0 {
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::Forbidden => StatusCode::FORBIDDEN,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Validation(_) => StatusCode::BAD_REQUEST,
            AppError::Config(_)
            | AppError::Database(_)
            | AppError::ExternalNotification(_)
            | AppError::Redis(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let body = Json(ErrorResponse {
            error: self.0.to_string(),
        });

        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::{build_router, ApiState};
    use crate::{auth::TokenSettings, realtime::RealtimeHub};
    use axum::{
        body::Body,
        http::{header, Method, Request, StatusCode},
        response::IntoResponse,
    };
    use chrono::Duration;
    use sqlx::PgPool;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_state() -> Arc<ApiState> {
        test_state_with_dev_routes(true)
    }

    fn test_state_with_dev_routes(enable_dev_routes: bool) -> Arc<ApiState> {
        Arc::new(ApiState {
            postgres: PgPool::connect_lazy(
                "postgres://postgres:postgres@localhost/coin_listener_test",
            )
            .expect("valid postgres url"),
            redis: None,
            scan_queue_key: "scan:address:queue".to_string(),
            notify_queue_key: "notify:event:queue".to_string(),
            enable_dev_routes,
            auth: TokenSettings {
                secret: "test-secret-with-enough-entropy".to_string(),
                ttl: Duration::seconds(3600),
            },
            realtime: RealtimeHub::new(16),
        })
    }

    #[test]
    fn forbidden_errors_map_to_http_403() {
        let response =
            super::ApiError::from(coin_listener_core::AppError::Forbidden).into_response();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

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
                    .body(Body::from(r#"{"email":"missing@example.com"}"#))
                    .expect("valid request"),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

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

    #[tokio::test]
    async fn router_exposes_provider_edit_and_test_routes() {
        let app = build_router(test_state());

        for (method, uri) in [
            (Method::PUT, "/api/providers/not-a-uuid"),
            (Method::POST, "/api/providers/not-a-uuid/test"),
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

    #[tokio::test]
    async fn router_exposes_watched_address_crud_routes() {
        let app = build_router(test_state());

        for (method, uri) in [
            (Method::GET, "/api/addresses"),
            (Method::POST, "/api/addresses"),
            (Method::PUT, "/api/addresses/not-a-uuid"),
            (Method::DELETE, "/api/addresses/not-a-uuid"),
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

    #[tokio::test]
    async fn router_exposes_events_filter_query() {
        let app = build_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/events?is_transfer=not-bool")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn router_exposes_system_status_route() {
        let app = build_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/system/status")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn router_exposes_notification_routes() {
        let app = build_router(test_state());

        for (method, uri, status) in [
            (
                Method::PUT,
                "/api/notification-channels",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::PATCH,
                "/api/notification-rules",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::POST,
                "/api/notification-channels",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::POST,
                "/api/notification-rules",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::PUT,
                "/api/notification-rules/not-a-uuid",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::DELETE,
                "/api/notification-rules/not-a-uuid",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::GET,
                "/api/in-app-notifications?unread_only=not-bool",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::POST,
                "/api/in-app-notifications/not-a-uuid/read",
                StatusCode::UNAUTHORIZED,
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

    #[tokio::test]
    async fn router_exposes_notification_operations_routes() {
        let app = build_router(test_state());

        for (method, uri, status) in [
            (
                Method::GET,
                "/api/notification-outbox?status=unknown",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::GET,
                "/api/notification-deliveries?status=unknown",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::GET,
                "/api/notification-deliveries?channel_type=email",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::GET,
                "/api/notification-outbox/not-a-uuid",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::POST,
                "/api/notification-outbox/not-a-uuid/retry",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::POST,
                "/api/notification-deliveries",
                StatusCode::UNAUTHORIZED,
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

    #[tokio::test]
    async fn router_exposes_dev_scan_address_route_when_enabled() {
        let app = build_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/dev/scan-address/not-a-uuid")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn router_hides_dev_scan_address_route_when_disabled() {
        let app = build_router(test_state_with_dev_routes(false));

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/dev/scan-address/not-a-uuid")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
