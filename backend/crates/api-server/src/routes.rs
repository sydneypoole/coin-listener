use crate::{
    auth::{self, AuthContext, TokenSettings},
    realtime::{self, RealtimeHub},
};
use axum::{
    extract::{ws::WebSocketUpgrade, Extension, FromRequestParts, Path, Query, Request, State},
    http::{HeaderMap, StatusCode, Uri},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{Duration, Utc};
use coin_listener_chain_providers::{
    btc::BtcClient,
    evm::{self, EvmRpcClient},
    tron::TronClient,
};
use coin_listener_core::{
    models::{
        AddressEventDraft, Asset, CreateNotificationChannelRequest, CreateNotificationRuleRequest,
        CreateProviderRequest, CreateTelegramBindingRequest, CreateTelegramBotRequest,
        CreateWatchedAddressImportRequest, CreateWatchedAddressRequest, EventQuery,
        EvmTransactionRescanRequest, EvmTransactionRescanResponse, EvmTransactionRescanSummary,
        EvmTransactionRescanTransferSummary, InAppNotificationQuery, LoginRequest, LoginResponse,
        NotificationChannelTestResponse, NotificationDeliveryListResponse,
        NotificationDeliveryQuery, NotificationOutboxListResponse, NotificationOutboxQuery,
        Provider, QueueStatus, RetryNotificationOutboxResponse, RetryScanRunResponse,
        ScanAddressContext, ScanRunListResponse, ScanRunQuery, SystemStatus,
        UpdateNotificationChannelRequest, UpdateTelegramBotRequest, UpdateTelegramSettingsRequest,
        UserSummary, VerificationResponse,
    },
    AppError, AppResult,
};
use coin_listener_storage::{
    notifications,
    notify_queue::{connect_notify_queue, NotifyQueue},
    repositories,
    scan_queue::{connect_scan_queue, ScanQueue},
    scan_runs, system_status, telegram_settings,
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
    pub telegram_webhook_secret: Option<String>,
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
    pub chain_type: String,
    pub provider_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderTestKind {
    EvmRpc,
    TronRest,
    BtcRest,
}

pub const SCAN_RETRY_CLAIM_STALE_MINUTES: i64 = 15;

pub fn provider_test_kind(chain_type: &str, provider_type: &str) -> AppResult<ProviderTestKind> {
    match (chain_type, provider_type) {
        (_, "websocket") => Err(AppError::Validation(
            "provider connectivity test does not support websocket providers".to_string(),
        )),
        ("evm", "rpc") => Ok(ProviderTestKind::EvmRpc),
        ("tron", "rest_api" | "rpc") => Ok(ProviderTestKind::TronRest),
        ("utxo", "rest_api") => Ok(ProviderTestKind::BtcRest),
        ("evm", other) => Err(AppError::Validation(format!(
            "provider connectivity test does not support {other} providers for evm chains"
        ))),
        ("tron", other) => Err(AppError::Validation(format!(
            "provider connectivity test does not support {other} providers for tron chains"
        ))),
        ("utxo", other) => Err(AppError::Validation(format!(
            "provider connectivity test does not support {other} providers for utxo chains"
        ))),
        (chain_type, _) => Err(AppError::Validation(format!(
            "provider connectivity test does not support {chain_type} chains"
        ))),
    }
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
        .route("/api/addresses/imports", post(create_address_import))
        .route("/api/addresses/imports/:id", get(get_address_import))
        .route(
            "/api/addresses/imports/:id/errors",
            get(list_address_import_errors),
        )
        .route(
            "/api/addresses/imports/:id/cancel",
            post(cancel_address_import),
        )
        .route(
            "/api/addresses/:id",
            put(update_address).delete(delete_address),
        )
        .route("/api/events", get(list_events))
        .route("/api/scan-runs", get(list_scan_runs))
        .route("/api/scan-runs/:id", get(get_scan_run))
        .route("/api/scan-runs/:id/retry", post(retry_scan_run))
        .route("/api/evm/transactions/rescan", post(rescan_evm_transaction))
        .route(
            "/api/telegram-settings",
            get(get_telegram_settings).put(update_telegram_settings),
        )
        .route(
            "/api/telegram-bots",
            get(list_telegram_bots).post(create_telegram_bot),
        )
        .route(
            "/api/telegram-bots/:id",
            put(update_telegram_bot).delete(delete_telegram_bot),
        )
        .route("/api/telegram-bots/:id/verify", post(verify_telegram_bot))
        .route("/api/telegram-bindings", post(create_telegram_binding))
        .route("/api/telegram-bindings/:id", get(get_telegram_binding))
        .route(
            "/api/telegram-bindings/:id/cancel",
            post(cancel_telegram_binding),
        )
        .route(
            "/api/notification-channels",
            get(list_notification_channels).post(create_notification_channel),
        )
        .route(
            "/api/notification-channels/:id",
            put(update_notification_channel).delete(delete_notification_channel),
        )
        .route(
            "/api/notification-channels/:id/verify",
            post(verify_notification_channel),
        )
        .route(
            "/api/notification-channels/:id/test",
            post(test_notification_channel),
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
        .route("/api/telegram/webhook/:bot_id", post(telegram_webhook))
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

async fn system_status_handler(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Response, ApiError> {
    let queues = queue_status(&state).await;
    let scans = system_status::system_scan_status(&state.postgres, auth.tenant_id).await?;
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
    let timeout_ms = u64::try_from(provider.timeout_ms)
        .map_err(|_| AppError::Validation("timeout_ms must be positive".to_string()))?;
    if timeout_ms == 0 {
        return Err(AppError::Validation("timeout_ms must be positive".to_string()).into());
    }

    let chain = repositories::chain_by_id(&state.postgres, provider.chain_id).await?;
    let timeout = StdDuration::from_millis(timeout_ms);
    let kind = provider_test_kind(&chain.chain_type, &provider.provider_type)?;

    let (message, latest_block) = match kind {
        ProviderTestKind::EvmRpc => {
            let client = EvmRpcClient::new(provider.base_url.clone(), timeout);
            let latest_block = client.eth_block_number().await?;
            ("EVM RPC reachable".to_string(), Some(latest_block))
        }
        ProviderTestKind::TronRest => {
            let client = TronClient::new(provider.base_url.clone(), timeout);
            client.test_connectivity().await?;
            ("TRON REST provider reachable".to_string(), None)
        }
        ProviderTestKind::BtcRest => {
            let client = BtcClient::new(provider.base_url.clone(), timeout);
            let latest_block = client.tip_height().await?;
            (
                "BTC REST provider reachable".to_string(),
                Some(latest_block),
            )
        }
    };

    Ok(Json(ProviderTestResponse {
        ok: true,
        message,
        latest_block,
        chain_type: chain.chain_type,
        provider_type: provider.provider_type,
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

async fn list_address_import_errors(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let errors = coin_listener_storage::address_imports::list_watched_address_import_errors(
        &state.postgres,
        auth.tenant_id,
        id,
    )
    .await?;
    Ok(Json(errors).into_response())
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

async fn list_events(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<EventQuery>,
) -> Result<Response, ApiError> {
    let events = repositories::list_events(&state.postgres, auth.tenant_id, query).await?;
    Ok(Json(events).into_response())
}

async fn rescan_evm_transaction(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<EvmTransactionRescanRequest>,
) -> Result<Response, ApiError> {
    validate_evm_tx_hash(&request.tx_hash)?;

    let chain = repositories::chain_by_id(&state.postgres, request.chain_id).await?;
    if chain.chain_type != "evm" {
        return Err(AppError::Validation("rescan only supports evm chains".to_string()).into());
    }

    let provider =
        repositories::active_rpc_provider_for_chain(&state.postgres, request.chain_id).await?;
    let rpc = EvmRpcClient::new(
        provider.base_url.clone(),
        provider_timeout_duration(&provider)?,
    );
    let tx = rpc.eth_get_transaction_by_hash(&request.tx_hash).await?;
    let receipt = rpc.eth_get_transaction_receipt(&request.tx_hash).await?;
    validate_evm_rescan_rpc_consistency(&request.tx_hash, &tx, &receipt)?;
    let receipt_succeeded = evm_receipt_succeeded(&receipt)?;
    let block_number = tx
        .block_number
        .as_deref()
        .map(evm::parse_hex_quantity_to_i64)
        .transpose()?
        .ok_or_else(|| AppError::Validation("transaction is not mined".to_string()))?;
    let native_value_raw = evm::parse_hex_u256_to_decimal_string(&tx.value)?;

    let watched_addresses = repositories::rescan_watched_addresses_for_chain(
        &state.postgres,
        auth.tenant_id,
        request.chain_id,
    )
    .await?;
    let assets = repositories::assets_for_chain(&state.postgres, request.chain_id).await?;
    let native_asset = assets
        .iter()
        .find(|asset| asset.asset_type == "native")
        .cloned()
        .ok_or_else(|| AppError::NotFound("native asset".to_string()))?;
    let token_transfers = evm::decode_rescan_token_transfers(&receipt, &assets)?;

    let mut inserted_events = Vec::new();
    let mut skipped_event_count = 0usize;
    let mut transfer_summaries = Vec::new();
    for address in &watched_addresses {
        let selected_assets =
            repositories::selected_assets_for_address(&state.postgres, address.id).await?;
        let context = ScanAddressContext {
            id: address.id,
            tenant_id: address.tenant_id,
            chain_id: address.chain_id,
            address: address.address.clone(),
            scan_interval_seconds: address.scan_interval_seconds,
            chain_type: chain.chain_type.clone(),
        };
        let watched = address.address.to_lowercase();

        if receipt_succeeded {
            for decoded in token_transfers
                .iter()
                .filter(|decoded| asset_selected(&selected_assets, decoded.asset.id))
            {
                let from = decoded.transfer.from_address.to_lowercase();
                let to = decoded.transfer.to_address.to_lowercase();
                if from != watched && to != watched {
                    continue;
                }

                push_transfer_summary(&mut transfer_summaries, decoded);
                let draft = evm::transfer_event_draft_with_source(
                    &context,
                    &decoded.asset,
                    decoded.transfer.clone(),
                    "evm_tx_rescan",
                    &tx,
                )?;
                match repositories::insert_event_and_outbox_if_not_exists(&state.postgres, draft)
                    .await?
                {
                    Some(event) => inserted_events.push(event),
                    None => skipped_event_count += 1,
                }
            }
        }

        let native_asset_is_selected = asset_selected(&selected_assets, native_asset.id);
        if should_insert_native_transfer_event(
            receipt_succeeded,
            native_asset_is_selected,
            &native_value_raw,
            &tx,
            &address.address,
        ) {
            let draft = evm_native_transfer_event_draft(
                &context,
                &native_asset,
                &tx,
                block_number,
                &native_value_raw,
            )?;
            match repositories::insert_event_and_outbox_if_not_exists(&state.postgres, draft)
                .await?
            {
                Some(event) => inserted_events.push(event),
                None => skipped_event_count += 1,
            }
        }

        if should_insert_fee_only_event(
            receipt_succeeded,
            native_asset_is_selected,
            &native_value_raw,
            &tx,
            &address.address,
        ) {
            let draft =
                evm::evm_fee_only_event_draft(&context, &native_asset, &tx, "evm_tx_rescan")?;
            match repositories::insert_event_and_outbox_if_not_exists(&state.postgres, draft)
                .await?
            {
                Some(event) => inserted_events.push(event),
                None => skipped_event_count += 1,
            }
        }
    }

    dedupe_transfer_summaries(&mut transfer_summaries);

    Ok(Json(EvmTransactionRescanResponse {
        summary: EvmTransactionRescanSummary {
            chain_id: request.chain_id,
            tx_hash: tx.hash,
            tx_from: tx.from,
            tx_to: tx.to,
            native_value_raw,
            block_number,
            token_transfer_count: transfer_summaries.len(),
            inserted_event_count: inserted_events.len(),
            skipped_event_count,
        },
        token_transfers: transfer_summaries,
        events: inserted_events,
    })
    .into_response())
}

fn validate_evm_tx_hash(tx_hash: &str) -> AppResult<()> {
    let digits = tx_hash
        .strip_prefix("0x")
        .ok_or_else(|| AppError::Validation("tx_hash must start with 0x".to_string()))?;
    if digits.len() != 64
        || !digits
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(AppError::Validation(
            "tx_hash must be 32-byte hex".to_string(),
        ));
    }
    Ok(())
}

fn provider_timeout_duration(provider: &Provider) -> AppResult<StdDuration> {
    let timeout_ms = u64::try_from(provider.timeout_ms)
        .map_err(|_| AppError::Validation("timeout_ms must be positive".to_string()))?;
    if timeout_ms == 0 {
        return Err(AppError::Validation(
            "timeout_ms must be positive".to_string(),
        ));
    }
    Ok(StdDuration::from_millis(timeout_ms))
}

fn asset_selected(selected_assets: &[Asset], asset_id: Uuid) -> bool {
    selected_assets.iter().any(|asset| asset.id == asset_id)
}

fn validate_evm_rescan_rpc_consistency(
    requested_tx_hash: &str,
    tx: &evm::EvmTransaction,
    receipt: &evm::EvmTransactionReceipt,
) -> AppResult<()> {
    if !tx.hash.eq_ignore_ascii_case(requested_tx_hash) {
        return Err(AppError::Validation(
            "transaction hash does not match request".to_string(),
        ));
    }
    if !tx.hash.eq_ignore_ascii_case(&receipt.transaction_hash) {
        return Err(AppError::Validation(
            "transaction hash does not match receipt".to_string(),
        ));
    }
    if !tx.from.eq_ignore_ascii_case(&receipt.from)
        || !optional_address_matches(&tx.to, &receipt.to)
    {
        return Err(AppError::Validation(
            "transaction endpoints do not match receipt".to_string(),
        ));
    }
    if tx.block_number != receipt.block_number || tx.block_hash != receipt.block_hash {
        return Err(AppError::Validation(
            "transaction block does not match receipt".to_string(),
        ));
    }
    Ok(())
}

fn optional_address_matches(left: &Option<String>, right: &Option<String>) -> bool {
    match (left.as_deref(), right.as_deref()) {
        (Some(left), Some(right)) => left.eq_ignore_ascii_case(right),
        (None, None) => true,
        _ => false,
    }
}

fn evm_receipt_succeeded(receipt: &evm::EvmTransactionReceipt) -> AppResult<bool> {
    match receipt.status.as_deref() {
        Some("0x1") | Some("0X1") => Ok(true),
        Some("0x0") | Some("0X0") => Ok(false),
        Some(status) => Err(AppError::Validation(format!(
            "unsupported transaction receipt status: {status}"
        ))),
        None => Ok(true),
    }
}

fn native_transfer_matches(tx: &evm::EvmTransaction, watched_address: &str) -> bool {
    tx.from.eq_ignore_ascii_case(watched_address)
        || tx
            .to
            .as_deref()
            .map(|to| to.eq_ignore_ascii_case(watched_address))
            .unwrap_or(false)
}

fn should_insert_native_transfer_event(
    receipt_succeeded: bool,
    native_asset_is_selected: bool,
    native_value_raw: &str,
    tx: &evm::EvmTransaction,
    watched_address: &str,
) -> bool {
    receipt_succeeded
        && native_asset_is_selected
        && native_value_raw != "0"
        && native_transfer_matches(tx, watched_address)
}

fn should_insert_fee_only_event(
    receipt_succeeded: bool,
    native_asset_is_selected: bool,
    native_value_raw: &str,
    tx: &evm::EvmTransaction,
    watched_address: &str,
) -> bool {
    native_asset_is_selected
        && tx.from.eq_ignore_ascii_case(watched_address)
        && (!receipt_succeeded || native_value_raw == "0")
}

fn evm_native_transfer_event_draft(
    context: &ScanAddressContext,
    native_asset: &Asset,
    tx: &evm::EvmTransaction,
    block_number: i64,
    native_value_raw: &str,
) -> AppResult<AddressEventDraft> {
    let watched = context.address.to_lowercase();
    let from = tx.from.to_lowercase();
    let to = tx.to.clone().unwrap_or_default();
    let to_lower = to.to_lowercase();
    let direction = if from == watched && to_lower == watched {
        "self"
    } else if to_lower == watched {
        "in"
    } else if from == watched {
        "out"
    } else {
        "unknown"
    };

    Ok(AddressEventDraft {
        tenant_id: context.tenant_id,
        chain_id: context.chain_id,
        address_id: context.id,
        asset_id: native_asset.id,
        event_type: "transfer".to_string(),
        direction: direction.to_string(),
        is_transfer: true,
        tx_hash: Some(tx.hash.clone()),
        log_index: Some(-1),
        block_number: Some(block_number),
        block_hash: tx.block_hash.clone(),
        confirmations: 0,
        from_address: Some(tx.from.clone()),
        to_address: tx.to.clone(),
        amount_raw: Some(native_value_raw.to_string()),
        amount_decimal: Some(evm::wei_to_decimal_string(
            native_value_raw,
            native_asset.decimals,
        )?),
        balance_before_raw: None,
        balance_after_raw: None,
        balance_delta_raw: None,
        metadata: serde_json::json!({
            "source": "evm_tx_rescan",
            "native_value_raw": native_value_raw,
        }),
    })
}

fn push_transfer_summary(
    summaries: &mut Vec<EvmTransactionRescanTransferSummary>,
    decoded: &evm::DecodedRescanTokenTransfer,
) {
    summaries.push(EvmTransactionRescanTransferSummary {
        asset_id: decoded.asset.id,
        symbol: decoded.asset.symbol.clone(),
        token_contract: decoded
            .transfer
            .token_contract
            .clone()
            .unwrap_or_else(|| decoded.asset.contract_address.clone().unwrap_or_default()),
        from_address: decoded.transfer.from_address.clone(),
        to_address: decoded.transfer.to_address.clone(),
        amount_raw: decoded.transfer.amount_raw.clone(),
        amount_decimal: decoded.transfer.amount_decimal.clone(),
        log_index: decoded.transfer.log_index,
    });
}

fn dedupe_transfer_summaries(summaries: &mut Vec<EvmTransactionRescanTransferSummary>) {
    let mut seen = std::collections::HashSet::new();
    summaries.retain(|summary| seen.insert((summary.asset_id, summary.log_index)));
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

async fn get_telegram_settings(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Response, ApiError> {
    let settings =
        telegram_settings::get_telegram_settings(&state.postgres, auth.tenant_id).await?;
    Ok(Json(settings).into_response())
}

async fn update_telegram_settings(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<UpdateTelegramSettingsRequest>,
) -> Result<Response, ApiError> {
    let settings =
        telegram_settings::update_telegram_settings(&state.postgres, auth.tenant_id, request)
            .await?;
    Ok(Json(settings).into_response())
}

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

async fn update_telegram_bot(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateTelegramBotRequest>,
) -> Result<Response, ApiError> {
    let bot =
        notifications::update_telegram_bot(&state.postgres, auth.tenant_id, id, request).await?;
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
    let outcome = notifier::external::ExternalNotificationSender::new(reqwest::Client::new())
        .verify_telegram_bot(&bot.bot_token, bot.effective_proxy_url.as_deref())
        .await;
    let ok = outcome.is_sent();
    let message = external_outcome_message(&outcome, "telegram bot verified");
    let status = if ok { "verified" } else { "failed" };
    let last_error = (!ok).then(|| message.clone());
    let username = outcome
        .metadata()
        .provider_response
        .as_deref()
        .and_then(|body| notifier::external::parse_telegram_verify_username(200, body));
    notifications::mark_telegram_bot_verification(
        &state.postgres,
        auth.tenant_id,
        id,
        status,
        username,
        last_error,
        Utc::now(),
    )
    .await?;

    Ok(Json(VerificationResponse { ok, message }).into_response())
}

async fn create_telegram_binding(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<CreateTelegramBindingRequest>,
) -> Result<Response, ApiError> {
    let bot = notifications::get_telegram_bot_secret(
        &state.postgres,
        auth.tenant_id,
        request.telegram_bot_id,
    )
    .await?;
    let bind_token = format!("bind_{}", Uuid::new_v4().simple());
    let short_code = format!(
        "CL-{}",
        Uuid::new_v4().simple().to_string()[..6].to_ascii_uppercase()
    );
    let deep_link_url = telegram_deep_link(bot.username.as_deref(), &bind_token);
    let binding = coin_listener_storage::telegram_bindings::create_binding_request(
        &state.postgres,
        auth.tenant_id,
        request.telegram_bot_id,
        bind_token,
        short_code,
        deep_link_url,
        Utc::now(),
    )
    .await?;

    Ok((StatusCode::CREATED, Json(binding)).into_response())
}

async fn get_telegram_binding(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let binding = coin_listener_storage::telegram_bindings::get_binding_request(
        &state.postgres,
        auth.tenant_id,
        id,
    )
    .await?;

    Ok(Json(binding).into_response())
}

async fn cancel_telegram_binding(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let binding = coin_listener_storage::telegram_bindings::cancel_binding_request(
        &state.postgres,
        auth.tenant_id,
        id,
    )
    .await?;

    Ok(Json(binding).into_response())
}

async fn telegram_webhook(
    State(state): State<Arc<ApiState>>,
    Path(bot_id): Path<Uuid>,
    headers: HeaderMap,
    Json(update): Json<notifier::telegram_updates::TelegramUpdate>,
) -> Result<Response, ApiError> {
    let header_secret = headers
        .get("X-Telegram-Bot-Api-Secret-Token")
        .and_then(|value| value.to_str().ok());
    if !telegram_webhook_secret_matches(state.telegram_webhook_secret.as_deref(), header_secret) {
        return Err(AppError::Unauthorized.into());
    }

    let bot = notifications::telegram_bot_secret_by_id_any_tenant(&state.postgres, bot_id).await?;
    let sender = notifier::external::ExternalNotificationSender::new(reqwest::Client::new());
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

    Ok(StatusCode::NO_CONTENT.into_response())
}

fn telegram_webhook_secret_matches(configured: Option<&str>, header: Option<&str>) -> bool {
    match configured {
        Some(secret) => header == Some(secret),
        None => true,
    }
}

fn telegram_deep_link(bot_username: Option<&str>, bind_token: &str) -> Option<String> {
    let username = bot_username?.trim_start_matches('@');
    if username.is_empty() || username.chars().any(char::is_whitespace) {
        return None;
    }
    Some(format!("https://t.me/{username}?start={bind_token}"))
}

async fn update_notification_channel(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateNotificationChannelRequest>,
) -> Result<Response, ApiError> {
    let channel =
        notifications::update_notification_channel(&state.postgres, auth.tenant_id, id, request)
            .await?;
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

async fn telegram_channel_bot_token(
    pool: &PgPool,
    tenant_id: Uuid,
    channel_id: Uuid,
    verify_mode: bool,
) -> AppResult<(
    notifier::external::TelegramChannelConfig,
    String,
    Option<String>,
)> {
    let channel = notifications::get_notification_channel(pool, tenant_id, channel_id).await?;
    if channel.channel_type != "telegram" {
        return Err(AppError::Validation(
            "only telegram channels can be verified or tested".to_string(),
        ));
    }
    if channel.status != "active" {
        return Err(AppError::Validation(
            "notification channel is inactive".to_string(),
        ));
    }

    let config = notifier::external::TelegramChannelConfig::parse(&channel.config)
        .map_err(|error| AppError::Validation(error.message))?;
    let bot_id = config.telegram_bot_id.ok_or_else(|| {
        let action = if verify_mode { "verified" } else { "tested" };
        AppError::Validation(format!("telegram_bot_id is required for channel {action}"))
    })?;
    let bot = notifications::get_telegram_bot_secret(pool, tenant_id, bot_id).await?;
    if bot.status != "active" {
        return Err(AppError::Validation("telegram bot is inactive".to_string()));
    }

    Ok((config, bot.bot_token, bot.effective_proxy_url))
}

async fn verify_notification_channel(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
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
    let ok = outcome.is_sent();
    let message = external_outcome_message(&outcome, "telegram channel verified");

    Ok(Json(VerificationResponse { ok, message }).into_response())
}

async fn test_notification_channel(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let (config, bot_token, proxy_url) =
        telegram_channel_bot_token(&state.postgres, auth.tenant_id, id, false).await?;
    let outcome = notifier::external::ExternalNotificationSender::new(reqwest::Client::new())
        .send_telegram(
            &config,
            &bot_token,
            "Coin Listener test notification",
            proxy_url.as_deref(),
        )
        .await;
    let ok = outcome.is_sent();
    let message = external_outcome_message(&outcome, "telegram channel test sent");

    Ok(Json(NotificationChannelTestResponse { ok, message }).into_response())
}

fn external_outcome_message(
    outcome: &notifier::external::ExternalSendOutcome,
    success: &str,
) -> String {
    if outcome.is_sent() {
        return success.to_string();
    }
    outcome
        .metadata()
        .last_error
        .clone()
        .unwrap_or_else(|| "external notification failed".to_string())
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

async fn list_scan_runs(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<ScanRunQuery>,
) -> Result<Response, ApiError> {
    let limit = scan_runs::scan_runs_limit(query.limit);
    let offset = scan_runs::scan_runs_offset(query.offset);
    let items = scan_runs::list_scan_runs(&state.postgres, auth.tenant_id, query).await?;

    Ok(Json(ScanRunListResponse {
        items,
        limit,
        offset,
    })
    .into_response())
}

async fn get_scan_run(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let detail = scan_runs::get_scan_run_detail(&state.postgres, auth.tenant_id, id).await?;
    Ok(Json(detail).into_response())
}

async fn retry_scan_run(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let now = Utc::now();
    let stale_claim_before = now - Duration::minutes(SCAN_RETRY_CLAIM_STALE_MINUTES);
    let task = scan_runs::retry_scan_run_task(
        &state.postgres,
        auth.tenant_id,
        id,
        now,
        stale_claim_before,
    )
    .await?;
    let enqueue_result = async {
        let redis_client = state
            .redis
            .as_ref()
            .ok_or_else(|| AppError::Redis("redis unavailable".to_string()))?;
        let mut connection = connect_scan_queue(redis_client).await?;
        let queue = ScanQueue::new(state.scan_queue_key.clone(), 1);
        queue.enqueue(&mut connection, &task).await
    }
    .await;

    if let Err(error) = enqueue_result {
        let error_message = error.to_string();
        if let Err(cleanup_error) = scan_runs::clear_retry_scan_run_claim(
            &state.postgres,
            auth.tenant_id,
            id,
            task.task_id,
            &error_message,
        )
        .await
        {
            tracing::warn!(
                scan_run_id = %id,
                task_id = %task.task_id,
                error = %cleanup_error,
                "failed to clear scan retry claim after enqueue failure"
            );
        }
        return Err(error.into());
    }

    scan_runs::mark_retry_scan_run_enqueued(
        &state.postgres,
        auth.tenant_id,
        id,
        task.task_id,
        Utc::now(),
    )
    .await?;

    Ok(Json(RetryScanRunResponse { task }).into_response())
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
            telegram_webhook_secret: None,
        })
    }

    #[test]
    fn telegram_webhook_secret_is_optional_but_enforced_when_configured() {
        assert!(super::telegram_webhook_secret_matches(None, None));
        assert!(super::telegram_webhook_secret_matches(None, Some("wrong")));
        assert!(!super::telegram_webhook_secret_matches(
            Some("secret"),
            None
        ));
        assert!(!super::telegram_webhook_secret_matches(
            Some("secret"),
            Some("wrong")
        ));
        assert!(super::telegram_webhook_secret_matches(
            Some("secret"),
            Some("secret")
        ));
    }

    #[test]
    fn telegram_deep_link_requires_verified_username_shape() {
        assert_eq!(
            super::telegram_deep_link(Some("coin_listener_bot"), "bind_abc").as_deref(),
            Some("https://t.me/coin_listener_bot?start=bind_abc")
        );
        assert_eq!(
            super::telegram_deep_link(Some("@coin_listener_bot"), "bind_abc").as_deref(),
            Some("https://t.me/coin_listener_bot?start=bind_abc")
        );
        assert_eq!(super::telegram_deep_link(None, "bind_abc"), None);
        assert_eq!(super::telegram_deep_link(Some(""), "bind_abc"), None);
        assert_eq!(super::telegram_deep_link(Some("Ops Bot"), "bind_abc"), None);
        assert_eq!(
            super::telegram_deep_link(Some(" coin_listener_bot "), "bind_abc"),
            None
        );
    }

    #[test]
    fn forbidden_errors_map_to_http_403() {
        let response =
            super::ApiError::from(coin_listener_core::AppError::Forbidden).into_response();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    fn production_source() -> &'static str {
        include_str!("routes.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production source is present")
    }

    #[test]
    fn system_status_handler_scopes_scan_runs_to_authenticated_tenant() {
        let source = production_source();

        assert!(source.contains("async fn system_status_handler("));
        assert!(source.contains("Extension(auth): Extension<AuthContext>"));
        assert!(source
            .contains("system_status::system_scan_status(&state.postgres, auth.tenant_id).await?"));
    }

    #[test]
    fn provider_test_response_includes_chain_and_provider_type() {
        let response = super::ProviderTestResponse {
            ok: true,
            message: "TRON REST provider reachable".to_string(),
            latest_block: None,
            chain_type: "tron".to_string(),
            provider_type: "rest_api".to_string(),
        };
        let json = serde_json::to_value(response).unwrap();

        assert_eq!(json["ok"], true);
        assert_eq!(json["message"], "TRON REST provider reachable");
        assert!(json["latest_block"].is_null());
        assert_eq!(json["chain_type"], "tron");
        assert_eq!(json["provider_type"], "rest_api");
    }

    #[test]
    fn provider_test_dispatch_supports_evm_tron_and_utxo_only() {
        assert_eq!(
            super::provider_test_kind("evm", "rpc").unwrap(),
            super::ProviderTestKind::EvmRpc
        );
        assert_eq!(
            super::provider_test_kind("tron", "rest_api").unwrap(),
            super::ProviderTestKind::TronRest
        );
        assert_eq!(
            super::provider_test_kind("tron", "rpc").unwrap(),
            super::ProviderTestKind::TronRest
        );
        assert_eq!(
            super::provider_test_kind("utxo", "rest_api").unwrap(),
            super::ProviderTestKind::BtcRest
        );

        let websocket = super::provider_test_kind("evm", "websocket")
            .unwrap_err()
            .to_string();
        assert!(websocket.contains("websocket providers"));

        let utxo_rpc = super::provider_test_kind("utxo", "rpc")
            .unwrap_err()
            .to_string();
        assert!(utxo_rpc.contains("rpc providers for utxo chains"));

        let evm_rest = super::provider_test_kind("evm", "rest_api")
            .unwrap_err()
            .to_string();
        assert!(evm_rest.contains("rest_api providers for evm chains"));
    }

    #[test]
    fn provider_test_handler_source_uses_all_supported_clients() {
        let source = include_str!("routes.rs");
        assert!(source.contains("BtcClient"));
        assert!(source.contains("TronClient"));
        assert!(source.contains("ProviderTestKind::EvmRpc"));
        assert!(source.contains("ProviderTestKind::TronRest"));
        assert!(source.contains("ProviderTestKind::BtcRest"));
        assert!(source.contains("eth_block_number().await"));
        assert!(source.contains("test_connectivity().await"));
        assert!(source.contains("tip_height().await"));
    }

    #[test]
    fn telegram_binding_routes_are_registered() {
        let source = production_source();

        for route in [
            "/api/telegram-bindings",
            "/api/telegram-bindings/:id",
            "/api/telegram-bindings/:id/cancel",
            "/api/telegram/webhook/:bot_id",
        ] {
            assert!(source.contains(route), "missing route {route}");
        }
    }

    #[test]
    fn telegram_settings_routes_are_protected() {
        let source = production_source();
        let route_index = source
            .find("/api/telegram-settings")
            .expect("telegram settings route is registered");
        let auth_layer_index = source
            .find("route_layer(middleware::from_fn_with_state")
            .expect("auth route layer is registered");

        assert!(route_index < auth_layer_index);
        assert!(source.contains("get(get_telegram_settings).put(update_telegram_settings)"));
    }

    #[test]
    fn telegram_webhook_route_is_not_under_auth_layer() {
        let source = production_source();
        let webhook_index = source
            .find("/api/telegram/webhook/:bot_id")
            .expect("webhook route is registered");
        let auth_layer_index = source
            .find("route_layer(middleware::from_fn_with_state")
            .expect("auth route layer is registered");

        assert!(webhook_index > auth_layer_index);
    }

    #[test]
    fn telegram_webhook_uses_bot_id_without_token_path() {
        let source = production_source();

        assert!(source.contains("telegram_bot_secret_by_id_any_tenant"));
        assert!(source.contains("/api/telegram/webhook/:bot_id"));
        assert!(!source.contains("/api/telegram/webhook/:bot_token"));
    }

    #[test]
    fn telegram_api_uses_effective_proxy_url() {
        let source = production_source();

        assert!(source.contains("bot.effective_proxy_url.as_deref()"));
        assert!(source.contains("process_telegram_binding_update("));
        assert!(source.contains("proxy_url.as_deref()"));
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

    #[test]
    fn watched_address_request_requires_asset_ids_field() {
        let missing_assets = r#"{
            "chain_id":"00000000-0000-0000-0000-000000000002",
            "address":"0x0000000000000000000000000000000000000001",
            "label":null,
            "priority":"normal",
            "scan_interval_seconds":300,
            "transfer_filter_enabled":true,
            "balance_change_filter_enabled":true,
            "status":"active"
        }"#;

        let error =
            serde_json::from_str::<coin_listener_core::models::CreateWatchedAddressRequest>(
                missing_assets,
            )
            .expect_err("missing asset_ids should fail deserialization");

        assert!(error.to_string().contains("asset_ids"));
    }

    #[test]
    fn watched_address_request_accepts_non_empty_asset_ids() {
        let payload = r#"{
            "chain_id":"00000000-0000-0000-0000-000000000002",
            "address":"0x0000000000000000000000000000000000000001",
            "label":null,
            "priority":"normal",
            "scan_interval_seconds":300,
            "transfer_filter_enabled":true,
            "balance_change_filter_enabled":true,
            "status":"active",
            "asset_ids":["00000000-0000-0000-0000-000000000101"]
        }"#;

        let request =
            serde_json::from_str::<coin_listener_core::models::CreateWatchedAddressRequest>(
                payload,
            )
            .expect("request with asset_ids deserializes");

        assert_eq!(request.asset_ids, vec![uuid::Uuid::from_u128(0x101)]);
    }

    #[tokio::test]
    async fn router_exposes_evm_transaction_rescan_route() {
        let app = build_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/evm/transactions/rescan")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn evm_tx_hash_validation_accepts_32_byte_hex() {
        super::validate_evm_tx_hash(
            "0x7e88e5d67ead0c0605f3bed96071ec4be14112bed2d929ee57e5b161bf6f2389",
        )
        .expect("valid tx hash");
    }

    #[test]
    fn evm_tx_hash_validation_rejects_invalid_hashes() {
        for tx_hash in [
            "",
            "7e88",
            "0x1234",
            "0xzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        ] {
            assert!(super::validate_evm_tx_hash(tx_hash).is_err(), "{tx_hash}");
        }
    }

    #[test]
    fn rescan_rpc_consistency_rejects_mismatched_receipt() {
        let tx = super::evm::EvmTransaction {
            hash: "0x7e88e5d67ead0c0605f3bed96071ec4be14112bed2d929ee57e5b161bf6f2389".to_string(),
            from: "0x0000000000000000000000000000000000000001".to_string(),
            to: Some("0x0000000000000000000000000000000000000002".to_string()),
            value: "0x0".to_string(),
            block_number: Some("0x10".to_string()),
            block_hash: Some("0xblock".to_string()),
            input: "0x".to_string(),
        };
        let mut receipt = super::evm::EvmTransactionReceipt {
            transaction_hash: tx.hash.clone(),
            status: Some("0x1".to_string()),
            block_number: tx.block_number.clone(),
            block_hash: tx.block_hash.clone(),
            from: tx.from.clone(),
            to: tx.to.clone(),
            logs: Vec::new(),
        };

        assert!(super::validate_evm_rescan_rpc_consistency(&tx.hash, &tx, &receipt).is_ok());

        assert!(super::validate_evm_rescan_rpc_consistency(
            "0x1111111111111111111111111111111111111111111111111111111111111111",
            &tx,
            &receipt,
        )
        .is_err());

        receipt.transaction_hash =
            "0x2222222222222222222222222222222222222222222222222222222222222222".to_string();
        assert!(super::validate_evm_rescan_rpc_consistency(&tx.hash, &tx, &receipt).is_err());

        receipt.transaction_hash = tx.hash.clone();
        receipt.from = "0x0000000000000000000000000000000000000009".to_string();
        assert!(super::validate_evm_rescan_rpc_consistency(&tx.hash, &tx, &receipt).is_err());
    }

    #[test]
    fn failed_receipt_blocks_transfer_but_keeps_fee_only_decision() {
        let tx = super::evm::EvmTransaction {
            hash: "0x7e88e5d67ead0c0605f3bed96071ec4be14112bed2d929ee57e5b161bf6f2389".to_string(),
            from: "0x0000000000000000000000000000000000000001".to_string(),
            to: Some("0x0000000000000000000000000000000000000002".to_string()),
            value: "0xde0b6b3a7640000".to_string(),
            block_number: Some("0x10".to_string()),
            block_hash: Some("0xblock".to_string()),
            input: "0x".to_string(),
        };
        let failed_receipt = super::evm::EvmTransactionReceipt {
            transaction_hash: tx.hash.clone(),
            status: Some("0x0".to_string()),
            block_number: tx.block_number.clone(),
            block_hash: tx.block_hash.clone(),
            from: tx.from.clone(),
            to: tx.to.clone(),
            logs: Vec::new(),
        };

        assert!(!super::evm_receipt_succeeded(&failed_receipt).unwrap());
        assert!(!super::should_insert_native_transfer_event(
            false,
            true,
            "1000000000000000000",
            &tx,
            &tx.from,
        ));
        assert!(super::should_insert_fee_only_event(
            false,
            true,
            "1000000000000000000",
            &tx,
            &tx.from,
        ));
        assert!(!super::should_insert_fee_only_event(
            true,
            true,
            "1000000000000000000",
            &tx,
            &tx.from,
        ));

        let zero_value_tx = super::evm::EvmTransaction {
            value: "0x0".to_string(),
            ..tx
        };
        assert!(super::should_insert_fee_only_event(
            true,
            true,
            "0",
            &zero_value_tx,
            &zero_value_tx.from,
        ));
    }

    #[test]
    fn native_transfer_draft_marks_value_transfer() {
        let context = coin_listener_core::models::ScanAddressContext {
            id: uuid::Uuid::from_u128(1),
            tenant_id: uuid::Uuid::from_u128(2),
            chain_id: uuid::Uuid::from_u128(3),
            address: "0x0000000000000000000000000000000000000002".to_string(),
            scan_interval_seconds: 300,
            chain_type: "evm".to_string(),
        };
        let asset = coin_listener_core::models::Asset {
            id: uuid::Uuid::from_u128(4),
            chain_id: context.chain_id,
            asset_type: "native".to_string(),
            symbol: "ETH".to_string(),
            name: "Ether".to_string(),
            contract_address: None,
            decimals: 18,
            is_builtin: true,
            status: "active".to_string(),
        };
        let tx = super::evm::EvmTransaction {
            hash: "0x7e88e5d67ead0c0605f3bed96071ec4be14112bed2d929ee57e5b161bf6f2389".to_string(),
            from: "0x0000000000000000000000000000000000000001".to_string(),
            to: Some(context.address.clone()),
            value: "0xde0b6b3a7640000".to_string(),
            block_number: Some("0x10".to_string()),
            block_hash: Some("0xblock".to_string()),
            input: "0x".to_string(),
        };

        let draft = super::evm_native_transfer_event_draft(
            &context,
            &asset,
            &tx,
            16,
            "1000000000000000000",
        )
        .unwrap();

        assert_eq!(draft.event_type, "transfer");
        assert!(draft.is_transfer);
        assert_eq!(draft.direction, "in");
        assert_eq!(draft.asset_id, asset.id);
        assert_eq!(draft.log_index, Some(-1));
        assert_eq!(draft.amount_decimal.as_deref(), Some("1.0"));
        assert_eq!(draft.metadata["source"], "evm_tx_rescan");
    }

    #[test]
    fn transfer_summary_dedupe_keeps_unique_log_assets() {
        let mut summaries = vec![
            coin_listener_core::models::EvmTransactionRescanTransferSummary {
                asset_id: uuid::Uuid::from_u128(1),
                symbol: "USDC".to_string(),
                token_contract: "0x1".to_string(),
                from_address: "0x2".to_string(),
                to_address: "0x3".to_string(),
                amount_raw: "100".to_string(),
                amount_decimal: "0.0001".to_string(),
                log_index: 7,
            },
            coin_listener_core::models::EvmTransactionRescanTransferSummary {
                asset_id: uuid::Uuid::from_u128(1),
                symbol: "USDC".to_string(),
                token_contract: "0x1".to_string(),
                from_address: "0x2".to_string(),
                to_address: "0x3".to_string(),
                amount_raw: "100".to_string(),
                amount_decimal: "0.0001".to_string(),
                log_index: 7,
            },
        ];

        super::dedupe_transfer_summaries(&mut summaries);

        assert_eq!(summaries.len(), 1);
    }

    #[test]
    fn rescan_selected_asset_gate_matches_by_asset_id() {
        let selected = vec![coin_listener_core::models::Asset {
            id: uuid::Uuid::from_u128(1),
            chain_id: uuid::Uuid::from_u128(2),
            asset_type: "erc20".to_string(),
            symbol: "USDC".to_string(),
            name: "USD Coin".to_string(),
            contract_address: Some("0x0000000000000000000000000000000000000001".to_string()),
            decimals: 6,
            is_builtin: true,
            status: "active".to_string(),
        }];

        assert!(super::asset_selected(&selected, uuid::Uuid::from_u128(1)));
        assert!(!super::asset_selected(&selected, uuid::Uuid::from_u128(3)));
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
            (Method::GET, "/api/telegram-bots", StatusCode::UNAUTHORIZED),
            (Method::POST, "/api/telegram-bots", StatusCode::UNAUTHORIZED),
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
    async fn router_exposes_scan_run_routes() {
        let app = build_router(test_state());

        for (method, uri, status) in [
            (
                Method::GET,
                "/api/scan-runs?status=failed&limit=50&offset=0",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::GET,
                "/api/scan-runs/not-a-uuid",
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::POST,
                "/api/scan-runs/not-a-uuid/retry",
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

    #[test]
    fn scan_run_handlers_use_tenant_scope_and_scan_queue_retry() {
        let source = production_source();

        assert!(source.contains("Extension(auth): Extension<AuthContext>"));
        assert!(source.contains("scan_runs::list_scan_runs(&state.postgres, auth.tenant_id"));
        assert!(source.contains("scan_runs::get_scan_run_detail(&state.postgres, auth.tenant_id"));
        assert!(source.contains("scan_runs::retry_scan_run_task("));
        assert!(source.contains("stale_claim_before"));
        assert!(source.contains("SCAN_RETRY_CLAIM_STALE_MINUTES"));
        assert!(source.contains("ScanQueue::new(state.scan_queue_key.clone(), 1)"));
        assert!(source.contains("queue.enqueue(&mut connection, &task).await"));
        assert!(source.contains("scan_runs::clear_retry_scan_run_claim("));
        assert!(source.contains("failed to clear scan retry claim after enqueue failure"));
        assert!(source.contains("return Err(error.into())"));
        assert!(source.contains("scan_runs::mark_retry_scan_run_enqueued("));
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
