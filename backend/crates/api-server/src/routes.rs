use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use chrono::Utc;
use coin_listener_core::{
    models::{
        CreateNotificationChannelRequest, CreateNotificationRuleRequest, CreateProviderRequest,
        CreateWatchedAddressRequest, EventQuery, InAppNotificationQuery, LoginRequest,
        LoginResponse, QueueStatus, SystemStatus, UserSummary,
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
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct ApiState {
    pub postgres: PgPool,
    pub redis: Option<redis::Client>,
    pub scan_queue_key: String,
    pub notify_queue_key: String,
    pub enable_dev_routes: bool,
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

pub fn build_router(state: Arc<ApiState>) -> Router {
    let router = Router::new()
        .route("/health", get(health))
        .route("/api/system/status", get(system_status_handler))
        .route("/api/auth/login", post(login))
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
        );

    if state.enable_dev_routes {
        router.route("/api/dev/scan-address/:id", post(scan_address))
    } else {
        router
    }
    .with_state(state)
}

async fn health(State(_state): State<Arc<ApiState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        service: "api-server",
    })
}

async fn system_status_handler(State(state): State<Arc<ApiState>>) -> Result<Response, ApiError> {
    let queues = queue_status(&state).await;
    let scans = system_status::system_scan_status(&state.postgres).await?;
    let events = system_status::system_event_status(&state.postgres).await?;
    let notifications = system_status::system_notification_status(&state.postgres).await?;
    let providers = system_status::system_provider_status(&state.postgres).await?;

    Ok(Json(SystemStatus {
        generated_at: Utc::now(),
        queues,
        scans,
        events,
        notifications,
        providers,
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
    if user.password_hash != request.password {
        return Err(AppError::Unauthorized.into());
    }

    let tenant = repositories::default_tenant_for_user(&state.postgres, user.id).await?;
    let token = format!("dev-token-{}", user.id);

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

async fn list_addresses(State(state): State<Arc<ApiState>>) -> Result<Response, ApiError> {
    let addresses = repositories::list_watched_addresses(&state.postgres).await?;
    Ok(Json(addresses).into_response())
}

async fn create_address(
    State(state): State<Arc<ApiState>>,
    Json(request): Json<CreateWatchedAddressRequest>,
) -> Result<Response, ApiError> {
    let address = repositories::create_watched_address(&state.postgres, request).await?;
    Ok((StatusCode::CREATED, Json(address)).into_response())
}

async fn update_address(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<Uuid>,
    Json(request): Json<CreateWatchedAddressRequest>,
) -> Result<Response, ApiError> {
    let address = repositories::update_watched_address(&state.postgres, id, request).await?;
    Ok(Json(address).into_response())
}

async fn delete_address(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    repositories::delete_watched_address(&state.postgres, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_events(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<EventQuery>,
) -> Result<Response, ApiError> {
    let events = repositories::list_events(&state.postgres, query).await?;
    Ok(Json(events).into_response())
}

async fn scan_address(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let event = repositories::create_mock_evm_event(&state.postgres, id).await?;
    Ok((StatusCode::CREATED, Json(event)).into_response())
}

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
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Validation(_) => StatusCode::BAD_REQUEST,
            AppError::Config(_) | AppError::Database(_) | AppError::Redis(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
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
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use sqlx::PgPool;
    use std::sync::Arc;
    use tower::ServiceExt;

    #[tokio::test]
    async fn router_exposes_events_filter_query() {
        let app = build_router(Arc::new(ApiState {
            postgres: PgPool::connect_lazy(
                "postgres://postgres:postgres@localhost/coin_listener_test",
            )
            .expect("valid postgres url"),
            redis: None,
            scan_queue_key: "scan:address:queue".to_string(),
            notify_queue_key: "notify:event:queue".to_string(),
            enable_dev_routes: true,
        }));

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

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn router_exposes_system_status_route() {
        let app = build_router(Arc::new(ApiState {
            postgres: PgPool::connect_lazy(
                "postgres://postgres:postgres@localhost/coin_listener_test",
            )
            .expect("valid postgres url"),
            redis: None,
            scan_queue_key: "scan:address:queue".to_string(),
            notify_queue_key: "notify:event:queue".to_string(),
            enable_dev_routes: false,
        }));

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/system/status")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn router_exposes_notification_routes() {
        let app = build_router(Arc::new(ApiState {
            postgres: PgPool::connect_lazy(
                "postgres://postgres:postgres@localhost/coin_listener_test",
            )
            .expect("valid postgres url"),
            redis: None,
            scan_queue_key: "scan:address:queue".to_string(),
            notify_queue_key: "notify:event:queue".to_string(),
            enable_dev_routes: true,
        }));

        for (method, uri, status) in [
            (
                Method::PUT,
                "/api/notification-channels",
                StatusCode::METHOD_NOT_ALLOWED,
            ),
            (
                Method::PATCH,
                "/api/notification-rules",
                StatusCode::METHOD_NOT_ALLOWED,
            ),
            (
                Method::POST,
                "/api/notification-channels",
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
            (
                Method::POST,
                "/api/notification-rules",
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
            (
                Method::PUT,
                "/api/notification-rules/not-a-uuid",
                StatusCode::BAD_REQUEST,
            ),
            (
                Method::DELETE,
                "/api/notification-rules/not-a-uuid",
                StatusCode::BAD_REQUEST,
            ),
            (
                Method::GET,
                "/api/in-app-notifications?unread_only=not-bool",
                StatusCode::BAD_REQUEST,
            ),
            (
                Method::POST,
                "/api/in-app-notifications/not-a-uuid/read",
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

    #[tokio::test]
    async fn router_exposes_dev_scan_address_route_when_enabled() {
        let app = build_router(Arc::new(ApiState {
            postgres: PgPool::connect_lazy(
                "postgres://postgres:postgres@localhost/coin_listener_test",
            )
            .expect("valid postgres url"),
            redis: None,
            scan_queue_key: "scan:address:queue".to_string(),
            notify_queue_key: "notify:event:queue".to_string(),
            enable_dev_routes: true,
        }));

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

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn router_hides_dev_scan_address_route_when_disabled() {
        let app = build_router(Arc::new(ApiState {
            postgres: PgPool::connect_lazy(
                "postgres://postgres:postgres@localhost/coin_listener_test",
            )
            .expect("valid postgres url"),
            redis: None,
            scan_queue_key: "scan:address:queue".to_string(),
            notify_queue_key: "notify:event:queue".to_string(),
            enable_dev_routes: false,
        }));

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
