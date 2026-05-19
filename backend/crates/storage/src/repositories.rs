use chrono::{DateTime, Duration, Utc};
use coin_listener_chain_providers::evm;
use coin_listener_core::{
    models::{
        AddressEvent, AddressEventDraft, Asset, BalanceSnapshot, Chain,
        CreateBalanceSnapshotRequest, CreateProviderRequest, CreateWatchedAddressRequest,
        EventQuery, NotificationOutboxDetail, NotificationOutboxItem, NotificationOutboxListItem,
        NotificationOutboxQuery, OutboxStatusCounts, Provider, ScanAddressCandidate,
        ScanAddressContext, ScanCursor, Tenant, User, WatchedAddress,
    },
    AppError, AppResult,
};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

const DEFAULT_TENANT_ID: Uuid = Uuid::from_u128(1);
const CLAIM_ONE_DUE_SCAN_ADDRESS_QUERY: &str = r#"
SELECT id, tenant_id, chain_id, scan_interval_seconds
FROM watched_addresses
WHERE status = 'active'
  AND next_scan_at <= $1
ORDER BY next_scan_at ASC, created_at ASC
FOR UPDATE SKIP LOCKED
LIMIT 1
"#;
const MARK_CLAIMED_SCAN_ENQUEUED_QUERY: &str = r#"
UPDATE watched_addresses
SET next_scan_at = $2,
    updated_at = NOW()
WHERE id = $1
"#;
pub const ACTIVE_RPC_PROVIDER_QUERY: &str = r#"
SELECT id, chain_id, provider_type, name, base_url, api_key_ref, priority, qps_limit, timeout_ms, status
FROM providers
WHERE chain_id = $1
  AND provider_type = 'rpc'
  AND status = 'active'
ORDER BY priority ASC, name ASC
LIMIT 1
"#;

pub const LATEST_BALANCE_SNAPSHOT_QUERY: &str = r#"
SELECT id, tenant_id, chain_id, address_id, asset_id, balance_raw, balance_decimal,
       block_number, block_hash, observed_at, source_provider_id
FROM balance_snapshots
WHERE address_id = $1
  AND asset_id = $2
  AND ($3::uuid IS NULL OR id <> $3)
ORDER BY observed_at DESC
LIMIT 1
"#;

pub const INSERT_BALANCE_SNAPSHOT_QUERY: &str = r#"
INSERT INTO balance_snapshots (
    tenant_id, chain_id, address_id, asset_id, balance_raw, balance_decimal,
    block_number, block_hash, source_provider_id
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
RETURNING id, tenant_id, chain_id, address_id, asset_id, balance_raw, balance_decimal,
          block_number, block_hash, observed_at, source_provider_id
"#;

pub const ACTIVE_ERC20_ASSETS_QUERY: &str = r#"
SELECT id, chain_id, asset_type, symbol, name, contract_address, decimals, is_builtin, status
FROM assets
WHERE chain_id = $1
  AND asset_type = 'erc20'
  AND status = 'active'
  AND contract_address IS NOT NULL
ORDER BY symbol, name
"#;

pub const ACTIVE_ASSETS_BY_TYPE_QUERY: &str = r#"
SELECT id, chain_id, asset_type, symbol, name, contract_address, decimals, is_builtin, status
FROM assets
WHERE chain_id = $1
  AND asset_type = $2
  AND status = 'active'
ORDER BY symbol, name
"#;

pub const SCAN_CURSOR_QUERY: &str = r#"
SELECT id, tenant_id, chain_id, address_id, cursor_type, last_scanned_block, updated_at
FROM scan_cursors
WHERE address_id = $1
  AND cursor_type = $2
"#;

pub const UPSERT_SCAN_CURSOR_QUERY: &str = r#"
INSERT INTO scan_cursors (tenant_id, chain_id, address_id, cursor_type, last_scanned_block)
VALUES ($1, $2, $3, $4, $5)
ON CONFLICT (address_id, cursor_type)
DO UPDATE SET last_scanned_block = GREATEST(scan_cursors.last_scanned_block, EXCLUDED.last_scanned_block),
              updated_at = NOW()
RETURNING id, tenant_id, chain_id, address_id, cursor_type, last_scanned_block, updated_at
"#;

pub const INSERT_EVENT_IF_NOT_EXISTS_QUERY: &str = r#"
INSERT INTO address_events (
    tenant_id, chain_id, address_id, asset_id, event_type, direction, is_transfer,
    tx_hash, log_index, block_number, block_hash, confirmations, from_address, to_address,
    amount_raw, amount_decimal, balance_before_raw, balance_after_raw, balance_delta_raw, metadata
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)
ON CONFLICT DO NOTHING
RETURNING id, tenant_id, chain_id, address_id, asset_id, event_type, direction, is_transfer,
          tx_hash, log_index, block_number, block_hash, confirmations, from_address, to_address,
          amount_raw, amount_decimal, balance_before_raw, balance_after_raw, balance_delta_raw,
          metadata, detected_at, created_at
"#;

pub const INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY: &str = r#"
INSERT INTO notification_outbox (tenant_id, event_id, status)
VALUES ($1, $2, 'pending')
ON CONFLICT (event_id) DO NOTHING
"#;

pub const CLAIM_DUE_NOTIFICATION_OUTBOX_QUERY: &str = r#"
WITH due AS (
    SELECT id
    FROM notification_outbox
    WHERE status IN ('pending', 'retryable')
      AND next_attempt_at <= $1
    ORDER BY next_attempt_at ASC, created_at ASC
    LIMIT $2
    FOR UPDATE SKIP LOCKED
)
UPDATE notification_outbox o
SET status = 'processing',
    locked_at = $1,
    locked_by = $3,
    attempt_count = attempt_count + 1,
    updated_at = NOW()
FROM due
WHERE o.id = due.id
RETURNING o.id, o.tenant_id, o.event_id, o.status, o.attempt_count,
          o.next_attempt_at, o.locked_at, o.locked_by, o.last_error,
          o.delivered_at, o.created_at, o.updated_at
"#;

pub const MARK_NOTIFICATION_OUTBOX_DELIVERED_QUERY: &str = r#"
UPDATE notification_outbox
SET status = 'delivered',
    delivered_at = $2,
    locked_at = NULL,
    locked_by = NULL,
    last_error = NULL,
    updated_at = NOW()
WHERE id = $1
  AND status = 'processing'
"#;

pub const MARK_NOTIFICATION_OUTBOX_RETRYABLE_QUERY: &str = r#"
UPDATE notification_outbox
SET status = 'retryable',
    next_attempt_at = $2,
    locked_at = NULL,
    locked_by = NULL,
    last_error = $3,
    updated_at = NOW()
WHERE id = $1
  AND status = 'processing'
"#;

pub const MARK_NOTIFICATION_OUTBOX_FAILED_QUERY: &str = r#"
UPDATE notification_outbox
SET status = 'failed',
    locked_at = NULL,
    locked_by = NULL,
    last_error = $2,
    updated_at = NOW()
WHERE id = $1
  AND status = 'processing'
"#;

pub const RELEASE_STALE_NOTIFICATION_OUTBOX_QUERY: &str = r#"
UPDATE notification_outbox
SET status = 'retryable',
    next_attempt_at = $2,
    locked_at = NULL,
    locked_by = NULL,
    updated_at = NOW()
WHERE status = 'processing'
  AND locked_at < $1
"#;

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
  AND status IN ('failed', 'retryable')
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

pub fn validate_notification_outbox_status(status: &str) -> AppResult<()> {
    if !matches!(
        status,
        "pending" | "processing" | "retryable" | "delivered" | "failed"
    ) {
        return Err(AppError::Validation(
            "outbox status must be pending, processing, retryable, delivered, or failed"
                .to_string(),
        ));
    }
    Ok(())
}

pub fn notification_outbox_status_allows_manual_retry(status: &str) -> bool {
    matches!(status, "failed" | "retryable")
}

fn manual_retry_validation_error() -> AppError {
    AppError::Validation(
        "only failed or retryable notification outbox rows can be retried".to_string(),
    )
}

fn retry_notification_outbox_update_miss_error(status: Option<String>) -> AppError {
    match status {
        Some(_) => manual_retry_validation_error(),
        None => AppError::NotFound("notification outbox".to_string()),
    }
}

pub fn notification_ops_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(50).clamp(1, 100)
}

pub fn notification_ops_offset(offset: Option<i64>) -> i64 {
    offset.unwrap_or(0).max(0)
}

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

pub async fn list_chains(pool: &PgPool) -> AppResult<Vec<Chain>> {
    sqlx::query_as::<_, Chain>(
        "SELECT id, key, name, chain_type, native_asset_symbol, status, default_confirmations FROM chains ORDER BY name",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn list_assets(pool: &PgPool) -> AppResult<Vec<Asset>> {
    sqlx::query_as::<_, Asset>(
        "SELECT id, chain_id, asset_type, symbol, name, contract_address, decimals, is_builtin, status FROM assets ORDER BY symbol, name",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn list_providers(pool: &PgPool) -> AppResult<Vec<Provider>> {
    sqlx::query_as::<_, Provider>(
        "SELECT id, chain_id, provider_type, name, base_url, api_key_ref, priority, qps_limit, timeout_ms, status FROM providers ORDER BY priority, name",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn active_rpc_provider_for_chain(pool: &PgPool, chain_id: Uuid) -> AppResult<Provider> {
    sqlx::query_as::<_, Provider>(ACTIVE_RPC_PROVIDER_QUERY)
        .bind(chain_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("active rpc provider".to_string()))
}

pub async fn native_asset_for_chain(pool: &PgPool, chain_id: Uuid) -> AppResult<Asset> {
    get_native_asset(pool, chain_id).await
}

pub async fn chain_by_id(pool: &PgPool, chain_id: Uuid) -> AppResult<Chain> {
    get_chain(pool, chain_id).await
}

pub async fn active_erc20_assets_for_chain(pool: &PgPool, chain_id: Uuid) -> AppResult<Vec<Asset>> {
    sqlx::query_as::<_, Asset>(ACTIVE_ERC20_ASSETS_QUERY)
        .bind(chain_id)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn active_assets_for_chain_by_type(
    pool: &PgPool,
    chain_id: Uuid,
    asset_type: &str,
) -> AppResult<Vec<Asset>> {
    sqlx::query_as::<_, Asset>(ACTIVE_ASSETS_BY_TYPE_QUERY)
        .bind(chain_id)
        .bind(asset_type)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn scan_cursor(
    pool: &PgPool,
    address_id: Uuid,
    cursor_type: &str,
) -> AppResult<Option<ScanCursor>> {
    sqlx::query_as::<_, ScanCursor>(SCAN_CURSOR_QUERY)
        .bind(address_id)
        .bind(cursor_type)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn upsert_scan_cursor(
    pool: &PgPool,
    tenant_id: Uuid,
    chain_id: Uuid,
    address_id: Uuid,
    cursor_type: &str,
    last_scanned_block: i64,
) -> AppResult<ScanCursor> {
    sqlx::query_as::<_, ScanCursor>(UPSERT_SCAN_CURSOR_QUERY)
        .bind(tenant_id)
        .bind(chain_id)
        .bind(address_id)
        .bind(cursor_type)
        .bind(last_scanned_block)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn latest_balance_snapshot(
    pool: &PgPool,
    address_id: Uuid,
    asset_id: Uuid,
    before_snapshot_id: Option<Uuid>,
) -> AppResult<Option<BalanceSnapshot>> {
    sqlx::query_as::<_, BalanceSnapshot>(LATEST_BALANCE_SNAPSHOT_QUERY)
        .bind(address_id)
        .bind(asset_id)
        .bind(before_snapshot_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn insert_balance_snapshot(
    pool: &PgPool,
    request: CreateBalanceSnapshotRequest,
) -> AppResult<BalanceSnapshot> {
    sqlx::query_as::<_, BalanceSnapshot>(INSERT_BALANCE_SNAPSHOT_QUERY)
        .bind(request.tenant_id)
        .bind(request.chain_id)
        .bind(request.address_id)
        .bind(request.asset_id)
        .bind(request.balance_raw)
        .bind(request.balance_decimal)
        .bind(request.block_number)
        .bind(request.block_hash)
        .bind(request.source_provider_id)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn create_provider(pool: &PgPool, request: CreateProviderRequest) -> AppResult<Provider> {
    validate_provider(&request)?;

    sqlx::query_as::<_, Provider>(
        r#"
        INSERT INTO providers (chain_id, provider_type, name, base_url, api_key_ref, priority, qps_limit, timeout_ms, status)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        RETURNING id, chain_id, provider_type, name, base_url, api_key_ref, priority, qps_limit, timeout_ms, status
        "#,
    )
    .bind(request.chain_id)
    .bind(request.provider_type)
    .bind(request.name)
    .bind(request.base_url)
    .bind(request.api_key_ref)
    .bind(request.priority)
    .bind(request.qps_limit)
    .bind(request.timeout_ms)
    .bind(request.status)
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn list_watched_addresses(pool: &PgPool) -> AppResult<Vec<WatchedAddress>> {
    sqlx::query_as::<_, WatchedAddress>(
        r#"
        SELECT id, tenant_id, chain_id, address, label, priority, scan_interval_seconds,
               transfer_filter_enabled, balance_change_filter_enabled, status
        FROM watched_addresses
        ORDER BY created_at DESC
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn create_watched_address(
    pool: &PgPool,
    request: CreateWatchedAddressRequest,
) -> AppResult<WatchedAddress> {
    let chain = get_chain(pool, request.chain_id).await?;
    validate_address_for_chain(&chain, &request.address)?;
    validate_watched_address(&request)?;

    sqlx::query_as::<_, WatchedAddress>(
        r#"
        INSERT INTO watched_addresses (
            tenant_id, chain_id, address, label, priority, scan_interval_seconds,
            transfer_filter_enabled, balance_change_filter_enabled, status
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        RETURNING id, tenant_id, chain_id, address, label, priority, scan_interval_seconds,
                  transfer_filter_enabled, balance_change_filter_enabled, status
        "#,
    )
    .bind(request.tenant_id.unwrap_or(DEFAULT_TENANT_ID))
    .bind(request.chain_id)
    .bind(request.address)
    .bind(request.label)
    .bind(request.priority)
    .bind(request.scan_interval_seconds)
    .bind(request.transfer_filter_enabled)
    .bind(request.balance_change_filter_enabled)
    .bind(request.status)
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn update_watched_address(
    pool: &PgPool,
    id: Uuid,
    request: CreateWatchedAddressRequest,
) -> AppResult<WatchedAddress> {
    let chain = get_chain(pool, request.chain_id).await?;
    validate_address_for_chain(&chain, &request.address)?;
    validate_watched_address(&request)?;

    sqlx::query_as::<_, WatchedAddress>(
        r#"
        UPDATE watched_addresses
        SET chain_id = $2,
            address = $3,
            label = $4,
            priority = $5,
            scan_interval_seconds = $6,
            transfer_filter_enabled = $7,
            balance_change_filter_enabled = $8,
            status = $9,
            updated_at = NOW()
        WHERE id = $1
        RETURNING id, tenant_id, chain_id, address, label, priority, scan_interval_seconds,
                  transfer_filter_enabled, balance_change_filter_enabled, status
        "#,
    )
    .bind(id)
    .bind(request.chain_id)
    .bind(request.address)
    .bind(request.label)
    .bind(request.priority)
    .bind(request.scan_interval_seconds)
    .bind(request.transfer_filter_enabled)
    .bind(request.balance_change_filter_enabled)
    .bind(request.status)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("watched address".to_string()))
}

pub async fn delete_watched_address(pool: &PgPool, id: Uuid) -> AppResult<()> {
    let result = sqlx::query("DELETE FROM watched_addresses WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("watched address".to_string()));
    }

    Ok(())
}

pub async fn list_events(pool: &PgPool, query: EventQuery) -> AppResult<Vec<AddressEvent>> {
    sqlx::query_as::<_, AddressEvent>(
        r#"
        SELECT id, tenant_id, chain_id, address_id, asset_id, event_type, direction, is_transfer,
               tx_hash, log_index, block_number, block_hash, confirmations, from_address, to_address,
               amount_raw, amount_decimal, balance_before_raw, balance_after_raw, balance_delta_raw,
               metadata, detected_at, created_at
        FROM address_events
        WHERE tenant_id = $1
          AND ($2::uuid IS NULL OR chain_id = $2)
          AND ($3::uuid IS NULL OR address_id = $3)
          AND ($4::uuid IS NULL OR asset_id = $4)
          AND ($5::text IS NULL OR event_type = $5)
          AND ($6::text IS NULL OR direction = $6)
          AND ($7::boolean IS NULL OR is_transfer = $7)
        ORDER BY created_at DESC
        LIMIT 200
        "#,
    )
    .bind(DEFAULT_TENANT_ID)
    .bind(query.chain_id)
    .bind(query.address_id)
    .bind(query.asset_id)
    .bind(query.event_type)
    .bind(query.direction)
    .bind(query.is_transfer)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn insert_event(pool: &PgPool, draft: AddressEventDraft) -> AppResult<AddressEvent> {
    sqlx::query_as::<_, AddressEvent>(
        r#"
        INSERT INTO address_events (
            tenant_id, chain_id, address_id, asset_id, event_type, direction, is_transfer,
            tx_hash, log_index, block_number, block_hash, confirmations, from_address, to_address,
            amount_raw, amount_decimal, balance_before_raw, balance_after_raw, balance_delta_raw, metadata
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)
        ON CONFLICT DO NOTHING
        RETURNING id, tenant_id, chain_id, address_id, asset_id, event_type, direction, is_transfer,
                  tx_hash, log_index, block_number, block_hash, confirmations, from_address, to_address,
                  amount_raw, amount_decimal, balance_before_raw, balance_after_raw, balance_delta_raw,
                  metadata, detected_at, created_at
        "#,
    )
    .bind(draft.tenant_id)
    .bind(draft.chain_id)
    .bind(draft.address_id)
    .bind(draft.asset_id)
    .bind(draft.event_type)
    .bind(draft.direction)
    .bind(draft.is_transfer)
    .bind(draft.tx_hash)
    .bind(draft.log_index)
    .bind(draft.block_number)
    .bind(draft.block_hash)
    .bind(draft.confirmations)
    .bind(draft.from_address)
    .bind(draft.to_address)
    .bind(draft.amount_raw)
    .bind(draft.amount_decimal)
    .bind(draft.balance_before_raw)
    .bind(draft.balance_after_raw)
    .bind(draft.balance_delta_raw)
    .bind(draft.metadata)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::Validation("event already exists".to_string()))
}

pub async fn insert_event_if_not_exists(
    pool: &PgPool,
    draft: AddressEventDraft,
) -> AppResult<Option<AddressEvent>> {
    sqlx::query_as::<_, AddressEvent>(INSERT_EVENT_IF_NOT_EXISTS_QUERY)
        .bind(draft.tenant_id)
        .bind(draft.chain_id)
        .bind(draft.address_id)
        .bind(draft.asset_id)
        .bind(draft.event_type)
        .bind(draft.direction)
        .bind(draft.is_transfer)
        .bind(draft.tx_hash)
        .bind(draft.log_index)
        .bind(draft.block_number)
        .bind(draft.block_hash)
        .bind(draft.confirmations)
        .bind(draft.from_address)
        .bind(draft.to_address)
        .bind(draft.amount_raw)
        .bind(draft.amount_decimal)
        .bind(draft.balance_before_raw)
        .bind(draft.balance_after_raw)
        .bind(draft.balance_delta_raw)
        .bind(draft.metadata)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn insert_event_and_outbox_if_not_exists(
    pool: &PgPool,
    draft: AddressEventDraft,
) -> AppResult<Option<AddressEvent>> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let event = sqlx::query_as::<_, AddressEvent>(INSERT_EVENT_IF_NOT_EXISTS_QUERY)
        .bind(draft.tenant_id)
        .bind(draft.chain_id)
        .bind(draft.address_id)
        .bind(draft.asset_id)
        .bind(draft.event_type)
        .bind(draft.direction)
        .bind(draft.is_transfer)
        .bind(draft.tx_hash)
        .bind(draft.log_index)
        .bind(draft.block_number)
        .bind(draft.block_hash)
        .bind(draft.confirmations)
        .bind(draft.from_address)
        .bind(draft.to_address)
        .bind(draft.amount_raw)
        .bind(draft.amount_decimal)
        .bind(draft.balance_before_raw)
        .bind(draft.balance_after_raw)
        .bind(draft.balance_delta_raw)
        .bind(draft.metadata)
        .fetch_optional(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    if let Some(event) = event {
        sqlx::query(INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY)
            .bind(event.tenant_id)
            .bind(event.id)
            .execute(transaction.as_mut())
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;

        transaction
            .commit()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        return Ok(Some(event));
    }

    transaction
        .commit()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    Ok(None)
}

pub async fn claim_due_notification_outbox(
    pool: &PgPool,
    now: DateTime<Utc>,
    worker_id: &str,
    limit: i64,
) -> AppResult<Vec<NotificationOutboxItem>> {
    sqlx::query_as::<_, NotificationOutboxItem>(CLAIM_DUE_NOTIFICATION_OUTBOX_QUERY)
        .bind(now)
        .bind(limit)
        .bind(worker_id)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn mark_notification_outbox_delivered(
    pool: &PgPool,
    id: Uuid,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let result = sqlx::query(MARK_NOTIFICATION_OUTBOX_DELIVERED_QUERY)
        .bind(id)
        .bind(now)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    ensure_notification_outbox_updated(result.rows_affected())
}

pub async fn mark_notification_outbox_retryable(
    pool: &PgPool,
    id: Uuid,
    next_attempt_at: DateTime<Utc>,
    last_error: &str,
) -> AppResult<()> {
    let result = sqlx::query(MARK_NOTIFICATION_OUTBOX_RETRYABLE_QUERY)
        .bind(id)
        .bind(next_attempt_at)
        .bind(last_error)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    ensure_notification_outbox_updated(result.rows_affected())
}

pub async fn mark_notification_outbox_failed(
    pool: &PgPool,
    id: Uuid,
    last_error: &str,
) -> AppResult<()> {
    let result = sqlx::query(MARK_NOTIFICATION_OUTBOX_FAILED_QUERY)
        .bind(id)
        .bind(last_error)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    ensure_notification_outbox_updated(result.rows_affected())
}

pub async fn release_stale_notification_outbox(
    pool: &PgPool,
    stale_before: DateTime<Utc>,
    next_attempt_at: DateTime<Utc>,
) -> AppResult<u64> {
    let result = sqlx::query(RELEASE_STALE_NOTIFICATION_OUTBOX_QUERY)
        .bind(stale_before)
        .bind(next_attempt_at)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    Ok(result.rows_affected())
}

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
    let outbox =
        sqlx::query_as::<_, NotificationOutboxListItem>(GET_NOTIFICATION_OUTBOX_ITEM_QUERY)
            .bind(DEFAULT_TENANT_ID)
            .bind(id)
            .bind(stale_before)
            .fetch_optional(pool)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .ok_or_else(|| AppError::NotFound("notification outbox".to_string()))?;

    let event =
        crate::notifications::get_address_event(pool, outbox.event_id, outbox.tenant_id).await?;
    let deliveries =
        crate::notifications::list_notification_deliveries_for_event(pool, outbox.event_id).await?;

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
        return Err(manual_retry_validation_error());
    }

    if let Some(outbox) =
        sqlx::query_as::<_, NotificationOutboxItem>(MANUAL_RETRY_NOTIFICATION_OUTBOX_QUERY)
            .bind(id)
            .bind(now)
            .bind(DEFAULT_TENANT_ID)
            .fetch_optional(pool)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
    {
        return Ok(outbox);
    }

    let current_status = sqlx::query_scalar::<_, String>(SELECT_NOTIFICATION_OUTBOX_STATUS_QUERY)
        .bind(id)
        .bind(DEFAULT_TENANT_ID)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    Err(retry_notification_outbox_update_miss_error(current_status))
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

pub async fn create_mock_evm_event(pool: &PgPool, address_id: Uuid) -> AppResult<AddressEvent> {
    let address = get_watched_address(pool, address_id).await?;
    let chain = get_chain(pool, address.chain_id).await?;
    if chain.chain_type != "evm" {
        return Err(AppError::Validation(
            "mock EVM scan only supports EVM chains".to_string(),
        ));
    }
    let asset = get_native_asset(pool, chain.id).await?;
    let sequence = next_mock_event_sequence(pool, address.id, asset.id).await?;
    let draft = evm::mock_evm_transfer(&address, &asset, sequence);
    insert_event(pool, draft).await
}

pub fn next_scan_at_from(now: DateTime<Utc>, scan_interval_seconds: i32) -> DateTime<Utc> {
    now + Duration::seconds(i64::from(scan_interval_seconds))
}

pub async fn claim_one_due_scan_address_for_update(
    transaction: &mut Transaction<'_, Postgres>,
    now: DateTime<Utc>,
) -> AppResult<Option<ScanAddressCandidate>> {
    sqlx::query_as::<_, ScanAddressCandidate>(CLAIM_ONE_DUE_SCAN_ADDRESS_QUERY)
        .bind(now)
        .fetch_optional(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn list_due_scan_addresses(
    pool: &PgPool,
    limit: i64,
) -> AppResult<Vec<ScanAddressCandidate>> {
    sqlx::query_as::<_, ScanAddressCandidate>(
        r#"
        SELECT id, tenant_id, chain_id, scan_interval_seconds
        FROM watched_addresses
        WHERE status = 'active'
          AND next_scan_at <= NOW()
        ORDER BY next_scan_at ASC, created_at ASC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn get_scan_address_context(
    pool: &PgPool,
    address_id: Uuid,
) -> AppResult<ScanAddressContext> {
    sqlx::query_as::<_, ScanAddressContext>(
        r#"
        SELECT wa.id,
               wa.tenant_id,
               wa.chain_id,
               wa.address,
               wa.scan_interval_seconds,
               c.chain_type
        FROM watched_addresses wa
        INNER JOIN chains c ON c.id = wa.chain_id
        WHERE wa.id = $1
          AND wa.status = 'active'
        "#,
    )
    .bind(address_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("watched address".to_string()))
}

pub async fn mark_claimed_scan_enqueued(
    transaction: &mut Transaction<'_, Postgres>,
    address_id: Uuid,
    next_scan_at: DateTime<Utc>,
) -> AppResult<()> {
    let result = sqlx::query(MARK_CLAIMED_SCAN_ENQUEUED_QUERY)
        .bind(address_id)
        .bind(next_scan_at)
        .execute(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    ensure_updated(result.rows_affected())
}

pub async fn finish_address_scan(
    pool: &PgPool,
    address_id: Uuid,
    last_scanned_at: DateTime<Utc>,
    next_scan_at: DateTime<Utc>,
) -> AppResult<()> {
    let result = sqlx::query(
        r#"
        UPDATE watched_addresses
        SET last_scanned_at = $2,
            next_scan_at = $3,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(address_id)
    .bind(last_scanned_at)
    .bind(next_scan_at)
    .execute(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("watched address".to_string()));
    }

    Ok(())
}

fn ensure_updated(rows_affected: u64) -> AppResult<()> {
    if rows_affected == 0 {
        return Err(AppError::NotFound("watched address".to_string()));
    }

    Ok(())
}

fn ensure_notification_outbox_updated(rows_affected: u64) -> AppResult<()> {
    if rows_affected == 0 {
        return Err(AppError::NotFound(
            "processing notification outbox item".to_string(),
        ));
    }

    Ok(())
}

async fn next_mock_event_sequence(
    pool: &PgPool,
    address_id: Uuid,
    asset_id: Uuid,
) -> AppResult<i64> {
    sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COUNT(*) + 1
        FROM address_events
        WHERE address_id = $1
          AND asset_id = $2
          AND metadata->>'source' = 'mock_evm_transfer'
        "#,
    )
    .bind(address_id)
    .bind(asset_id)
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

async fn get_watched_address(pool: &PgPool, id: Uuid) -> AppResult<WatchedAddress> {
    sqlx::query_as::<_, WatchedAddress>(
        r#"
        SELECT id, tenant_id, chain_id, address, label, priority, scan_interval_seconds,
               transfer_filter_enabled, balance_change_filter_enabled, status
        FROM watched_addresses
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("watched address".to_string()))
}

async fn get_native_asset(pool: &PgPool, chain_id: Uuid) -> AppResult<Asset> {
    sqlx::query_as::<_, Asset>(
        "SELECT id, chain_id, asset_type, symbol, name, contract_address, decimals, is_builtin, status FROM assets WHERE chain_id = $1 AND asset_type = 'native' LIMIT 1",
    )
    .bind(chain_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("native asset".to_string()))
}

async fn get_chain(pool: &PgPool, id: Uuid) -> AppResult<Chain> {
    sqlx::query_as::<_, Chain>(
        "SELECT id, key, name, chain_type, native_asset_symbol, status, default_confirmations FROM chains WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("chain".to_string()))
}

fn validate_provider(request: &CreateProviderRequest) -> AppResult<()> {
    if request.name.trim().is_empty() {
        return Err(AppError::Validation(
            "provider name is required".to_string(),
        ));
    }
    if request.base_url.trim().is_empty() {
        return Err(AppError::Validation(
            "provider base_url is required".to_string(),
        ));
    }
    if request.qps_limit <= 0 {
        return Err(AppError::Validation(
            "qps_limit must be positive".to_string(),
        ));
    }
    if request.timeout_ms <= 0 {
        return Err(AppError::Validation(
            "timeout_ms must be positive".to_string(),
        ));
    }
    Ok(())
}

fn validate_watched_address(request: &CreateWatchedAddressRequest) -> AppResult<()> {
    if request.scan_interval_seconds < 10 {
        return Err(AppError::Validation(
            "scan_interval_seconds must be at least 10".to_string(),
        ));
    }
    if !matches!(request.priority.as_str(), "normal" | "high" | "critical") {
        return Err(AppError::Validation(
            "priority must be normal, high, or critical".to_string(),
        ));
    }
    Ok(())
}

fn validate_address_for_chain(chain: &Chain, address: &str) -> AppResult<()> {
    let address = address.trim();
    if address.is_empty() {
        return Err(AppError::Validation("address is required".to_string()));
    }

    match chain.chain_type.as_str() {
        "evm" => {
            let valid = address.len() == 42
                && address.starts_with("0x")
                && address[2..]
                    .chars()
                    .all(|character| character.is_ascii_hexdigit());
            if !valid {
                return Err(AppError::Validation("invalid EVM address".to_string()));
            }
        }
        "tron" => {
            if !(address.starts_with('T') && address.len() >= 26 && address.len() <= 36) {
                return Err(AppError::Validation("invalid TRON address".to_string()));
            }
        }
        "utxo" => {
            let valid = address.starts_with("bc1")
                || address.starts_with('1')
                || address.starts_with('3')
                || address.starts_with("tb1");
            if !valid {
                return Err(AppError::Validation("invalid BTC address".to_string()));
            }
        }
        _ => return Err(AppError::Validation("unsupported chain type".to_string())),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        next_scan_at_from, ACTIVE_ASSETS_BY_TYPE_QUERY, ACTIVE_ERC20_ASSETS_QUERY,
        ACTIVE_RPC_PROVIDER_QUERY, ACTIVE_TENANT_MEMBERSHIP_QUERY,
        CLAIM_DUE_NOTIFICATION_OUTBOX_QUERY, CLAIM_ONE_DUE_SCAN_ADDRESS_QUERY,
        GET_NOTIFICATION_OUTBOX_ITEM_QUERY, INSERT_BALANCE_SNAPSHOT_QUERY,
        INSERT_EVENT_IF_NOT_EXISTS_QUERY, INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY,
        LATEST_BALANCE_SNAPSHOT_QUERY, LIST_NOTIFICATION_OUTBOX_QUERY,
        MANUAL_RETRY_NOTIFICATION_OUTBOX_QUERY, MARK_CLAIMED_SCAN_ENQUEUED_QUERY,
        MARK_NOTIFICATION_OUTBOX_DELIVERED_QUERY, MARK_NOTIFICATION_OUTBOX_FAILED_QUERY,
        MARK_NOTIFICATION_OUTBOX_RETRYABLE_QUERY, NOTIFICATION_OUTBOX_STATUS_COUNTS_QUERY,
        RELEASE_STALE_NOTIFICATION_OUTBOX_QUERY, SCAN_CURSOR_QUERY,
        SELECT_NOTIFICATION_OUTBOX_STATUS_QUERY, UPSERT_SCAN_CURSOR_QUERY,
    };
    use chrono::{TimeZone, Utc};
    use coin_listener_core::{
        models::{
            AddressEvent, AddressEventDraft, NotificationOutboxDetail, NotificationOutboxItem,
            NotificationOutboxListItem, NotificationOutboxQuery, OutboxStatusCounts,
        },
        AppError, AppResult,
    };
    use sqlx::PgPool;

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
    fn migration_versions_are_unique() {
        let migration_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        let mut versions = std::collections::HashSet::new();

        for entry in std::fs::read_dir(migration_dir).expect("migration dir readable") {
            let path = entry.expect("migration entry readable").path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("sql") {
                continue;
            }

            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .expect("migration file name utf8");
            let version = file_name
                .split_once('_')
                .map(|(version, _)| version)
                .expect("migration file uses version prefix");

            assert!(
                versions.insert(version.to_string()),
                "duplicate migration version {version}"
            );
        }
    }

    #[test]
    fn active_tenant_membership_query_checks_user_and_tenant_status() {
        assert!(ACTIVE_TENANT_MEMBERSHIP_QUERY.contains("u.status = 'active'"));
        assert!(ACTIVE_TENANT_MEMBERSHIP_QUERY.contains("t.status = 'active'"));
        assert!(ACTIVE_TENANT_MEMBERSHIP_QUERY.contains("tm.user_id = $1"));
        assert!(ACTIVE_TENANT_MEMBERSHIP_QUERY.contains("tm.tenant_id = $2"));
    }

    #[test]
    fn next_scan_at_from_adds_scan_interval_seconds() {
        let now = Utc.with_ymd_and_hms(2026, 5, 17, 12, 0, 0).unwrap();

        let next_scan_at = next_scan_at_from(now, 300);

        assert_eq!(
            next_scan_at,
            Utc.with_ymd_and_hms(2026, 5, 17, 12, 5, 0).unwrap()
        );
    }

    #[test]
    fn claim_one_due_scan_address_locks_without_updating_next_scan() {
        assert!(CLAIM_ONE_DUE_SCAN_ADDRESS_QUERY.contains("FOR UPDATE SKIP LOCKED"));
        assert!(!CLAIM_ONE_DUE_SCAN_ADDRESS_QUERY.contains("SET next_scan_at"));
    }

    #[test]
    fn mark_claimed_scan_enqueued_requires_same_transaction() {
        assert!(MARK_CLAIMED_SCAN_ENQUEUED_QUERY.contains("WHERE id = $1"));
        assert!(!MARK_CLAIMED_SCAN_ENQUEUED_QUERY.contains("scan_claim_token"));
    }

    #[test]
    fn active_rpc_provider_query_filters_active_rpc_by_priority() {
        assert!(ACTIVE_RPC_PROVIDER_QUERY.contains("provider_type = 'rpc'"));
        assert!(ACTIVE_RPC_PROVIDER_QUERY.contains("status = 'active'"));
        assert!(ACTIVE_RPC_PROVIDER_QUERY.contains("ORDER BY priority ASC, name ASC"));
        assert!(ACTIVE_RPC_PROVIDER_QUERY.contains("LIMIT 1"));
    }

    #[test]
    fn active_erc20_assets_query_filters_active_contract_assets() {
        assert!(ACTIVE_ERC20_ASSETS_QUERY.contains("asset_type = 'erc20'"));
        assert!(ACTIVE_ERC20_ASSETS_QUERY.contains("status = 'active'"));
        assert!(ACTIVE_ERC20_ASSETS_QUERY.contains("contract_address IS NOT NULL"));
    }

    #[test]
    fn active_assets_by_type_query_filters_chain_type_and_status() {
        assert!(ACTIVE_ASSETS_BY_TYPE_QUERY.contains("chain_id = $1"));
        assert!(ACTIVE_ASSETS_BY_TYPE_QUERY.contains("asset_type = $2"));
        assert!(ACTIVE_ASSETS_BY_TYPE_QUERY.contains("status = 'active'"));
        assert!(ACTIVE_ASSETS_BY_TYPE_QUERY.contains("ORDER BY symbol, name"));
    }

    #[test]
    fn scan_cursor_queries_use_address_and_cursor_type() {
        assert!(SCAN_CURSOR_QUERY.contains("address_id = $1"));
        assert!(SCAN_CURSOR_QUERY.contains("cursor_type = $2"));
        assert!(UPSERT_SCAN_CURSOR_QUERY.contains("ON CONFLICT (address_id, cursor_type)"));
        assert!(UPSERT_SCAN_CURSOR_QUERY.contains(
            "last_scanned_block = GREATEST(scan_cursors.last_scanned_block, EXCLUDED.last_scanned_block)"
        ));
    }

    #[test]
    fn insert_event_if_not_exists_query_returns_optional_event() {
        assert!(INSERT_EVENT_IF_NOT_EXISTS_QUERY.contains("ON CONFLICT DO NOTHING"));
        assert!(INSERT_EVENT_IF_NOT_EXISTS_QUERY.contains("RETURNING id"));
        assert!(INSERT_EVENT_IF_NOT_EXISTS_QUERY.contains("address_events"));
        for field in [
            "chain_id",
            "tx_hash",
            "log_index",
            "address_id",
            "asset_id",
            "event_type",
        ] {
            assert!(INSERT_EVENT_IF_NOT_EXISTS_QUERY.contains(field));
        }
    }

    #[test]
    fn notification_outbox_migration_defines_reliable_task_table() {
        let migration = include_str!("../migrations/0007_notification_outbox.sql");

        assert!(migration.contains("CREATE TABLE IF NOT EXISTS notification_outbox"));
        assert!(
            migration.contains("tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE")
        );
        assert!(migration
            .contains("event_id UUID NOT NULL REFERENCES address_events(id) ON DELETE CASCADE"));
        assert!(migration.contains("UNIQUE(event_id)"));
        assert!(migration.contains("idx_notification_outbox_claim"));
        assert!(migration.contains("WHERE status IN ('pending', 'retryable')"));
        assert!(migration.contains("idx_notification_outbox_processing_stale"));
        assert!(migration.contains("WHERE status = 'processing'"));
    }

    #[test]
    fn notification_ops_indexes_migration_adds_outbox_and_delivery_indexes() {
        let migration = include_str!("../migrations/0009_notification_ops_indexes.sql");

        assert!(migration.contains("idx_notification_outbox_tenant_status_created"));
        assert!(migration.contains("ON notification_outbox(tenant_id, status, created_at DESC)"));
        assert!(migration.contains("idx_notification_outbox_tenant_next_attempt"));
        assert!(migration.contains("ON notification_outbox(tenant_id, next_attempt_at)"));
        assert!(migration.contains("idx_notification_deliveries_tenant_status_created"));
        assert!(
            migration.contains("ON notification_deliveries(tenant_id, status, created_at DESC)")
        );
        assert!(migration.contains("idx_notification_deliveries_tenant_channel_type_created"));
        assert!(migration
            .contains("ON notification_deliveries(tenant_id, channel_type, created_at DESC)"));
    }

    #[test]
    fn notification_outbox_insert_query_links_new_event() {
        assert!(INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY.contains("notification_outbox"));
        assert!(INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY.contains("tenant_id"));
        assert!(INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY.contains("event_id"));
        assert!(INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY.contains("'pending'"));
        assert!(INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY
            .contains("ON CONFLICT (event_id) DO NOTHING"));
    }

    #[allow(dead_code)]
    async fn assert_event_outbox_helper_signature(
        pool: &PgPool,
        draft: AddressEventDraft,
    ) -> AppResult<Option<AddressEvent>> {
        super::insert_event_and_outbox_if_not_exists(pool, draft).await
    }

    #[test]
    fn insert_event_and_outbox_helper_signature_is_stable() {
        let _ = assert_event_outbox_helper_signature;
    }

    #[allow(dead_code)]
    async fn assert_claim_outbox_signature(
        pool: &PgPool,
        now: chrono::DateTime<Utc>,
        worker_id: &str,
        limit: i64,
    ) -> AppResult<Vec<NotificationOutboxItem>> {
        super::claim_due_notification_outbox(pool, now, worker_id, limit).await
    }

    #[allow(dead_code)]
    async fn assert_mark_outbox_delivered_signature(
        pool: &PgPool,
        id: uuid::Uuid,
        now: chrono::DateTime<Utc>,
    ) -> AppResult<()> {
        super::mark_notification_outbox_delivered(pool, id, now).await
    }

    #[allow(dead_code)]
    async fn assert_mark_outbox_failed_signature(
        pool: &PgPool,
        id: uuid::Uuid,
        last_error: &str,
    ) -> AppResult<()> {
        super::mark_notification_outbox_failed(pool, id, last_error).await
    }

    #[allow(dead_code)]
    async fn assert_mark_outbox_retryable_signature(
        pool: &PgPool,
        id: uuid::Uuid,
        next_attempt_at: chrono::DateTime<Utc>,
        last_error: &str,
    ) -> AppResult<()> {
        super::mark_notification_outbox_retryable(pool, id, next_attempt_at, last_error).await
    }

    #[allow(dead_code)]
    async fn assert_release_stale_outbox_signature(
        pool: &PgPool,
        stale_before: chrono::DateTime<Utc>,
        next_attempt_at: chrono::DateTime<Utc>,
    ) -> AppResult<u64> {
        super::release_stale_notification_outbox(pool, stale_before, next_attempt_at).await
    }

    #[test]
    fn notification_outbox_repository_helper_signatures_are_stable() {
        let _ = assert_claim_outbox_signature;
        let _ = assert_mark_outbox_delivered_signature;
        let _ = assert_mark_outbox_retryable_signature;
        let _ = assert_mark_outbox_failed_signature;
        let _ = assert_release_stale_outbox_signature;
    }

    #[test]
    fn insert_event_and_outbox_helper_uses_transaction_safe_queries() {
        assert!(INSERT_EVENT_IF_NOT_EXISTS_QUERY.contains("ON CONFLICT DO NOTHING"));
        assert!(INSERT_EVENT_IF_NOT_EXISTS_QUERY.contains("RETURNING id"));
        assert!(INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY
            .contains("ON CONFLICT (event_id) DO NOTHING"));
        assert!(INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY.contains("VALUES ($1, $2, 'pending')"));
    }

    #[test]
    fn notification_outbox_claim_query_uses_skip_locked_and_increments_attempt() {
        assert!(CLAIM_DUE_NOTIFICATION_OUTBOX_QUERY.contains("FOR UPDATE SKIP LOCKED"));
        assert!(CLAIM_DUE_NOTIFICATION_OUTBOX_QUERY.contains("status IN ('pending', 'retryable')"));
        assert!(CLAIM_DUE_NOTIFICATION_OUTBOX_QUERY.contains("next_attempt_at <= $1"));
        assert!(CLAIM_DUE_NOTIFICATION_OUTBOX_QUERY.contains("attempt_count = attempt_count + 1"));
        assert!(CLAIM_DUE_NOTIFICATION_OUTBOX_QUERY.contains("locked_by = $3"));
    }

    #[test]
    fn notification_outbox_mark_queries_require_processing_status() {
        for query in [
            MARK_NOTIFICATION_OUTBOX_DELIVERED_QUERY,
            MARK_NOTIFICATION_OUTBOX_RETRYABLE_QUERY,
            MARK_NOTIFICATION_OUTBOX_FAILED_QUERY,
        ] {
            assert!(query.contains("WHERE id = $1"));
            assert!(query.contains("status = 'processing'"));
        }
    }

    #[test]
    fn notification_outbox_update_miss_reports_outbox_item() {
        let error = super::ensure_notification_outbox_updated(0)
            .expect_err("missing processing outbox item");

        assert!(error
            .to_string()
            .contains("processing notification outbox item"));
    }

    #[test]
    fn notification_outbox_stale_release_only_matches_stale_processing_rows() {
        assert!(RELEASE_STALE_NOTIFICATION_OUTBOX_QUERY.contains("status = 'processing'"));
        assert!(RELEASE_STALE_NOTIFICATION_OUTBOX_QUERY.contains("locked_at < $1"));
        assert!(RELEASE_STALE_NOTIFICATION_OUTBOX_QUERY.contains("status = 'retryable'"));
        assert!(RELEASE_STALE_NOTIFICATION_OUTBOX_QUERY.contains("locked_by = NULL"));
    }

    #[test]
    fn retry_outbox_update_miss_maps_current_row_to_validation() {
        let current_status = Some("processing".to_string());

        let result = super::retry_notification_outbox_update_miss_error(current_status);

        assert!(matches!(
            result,
            AppError::Validation(message)
                if message == "only failed or retryable notification outbox rows can be retried"
        ));
    }

    #[test]
    fn retry_outbox_update_miss_maps_missing_row_to_not_found() {
        let result = super::retry_notification_outbox_update_miss_error(None);

        assert!(matches!(
            result,
            AppError::NotFound(resource) if resource == "notification outbox"
        ));
    }

    #[test]
    fn notification_outbox_ops_validates_status_and_retryability() {
        for status in ["pending", "processing", "retryable", "delivered", "failed"] {
            assert!(
                super::validate_notification_outbox_status(status).is_ok(),
                "{status}"
            );
        }
        assert!(super::validate_notification_outbox_status("unknown").is_err());
        assert!(super::notification_outbox_status_allows_manual_retry(
            "failed"
        ));
        assert!(super::notification_outbox_status_allows_manual_retry(
            "retryable"
        ));
        assert!(!super::notification_outbox_status_allows_manual_retry(
            "pending"
        ));
        assert!(!super::notification_outbox_status_allows_manual_retry(
            "processing"
        ));
        assert!(!super::notification_outbox_status_allows_manual_retry(
            "delivered"
        ));
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
        assert!(MANUAL_RETRY_NOTIFICATION_OUTBOX_QUERY
            .contains("AND status IN ('failed', 'retryable')"));
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

    #[test]
    fn latest_balance_snapshot_query_can_exclude_current_snapshot() {
        assert!(LATEST_BALANCE_SNAPSHOT_QUERY.contains("address_id = $1"));
        assert!(LATEST_BALANCE_SNAPSHOT_QUERY.contains("asset_id = $2"));
        assert!(LATEST_BALANCE_SNAPSHOT_QUERY.contains("$3::uuid IS NULL OR id <> $3"));
        assert!(LATEST_BALANCE_SNAPSHOT_QUERY.contains("ORDER BY observed_at DESC"));
        assert!(LATEST_BALANCE_SNAPSHOT_QUERY.contains("LIMIT 1"));
    }

    #[test]
    fn insert_balance_snapshot_query_maps_all_scan_fields() {
        assert!(INSERT_BALANCE_SNAPSHOT_QUERY.contains("tenant_id"));
        assert!(INSERT_BALANCE_SNAPSHOT_QUERY.contains("balance_raw"));
        assert!(INSERT_BALANCE_SNAPSHOT_QUERY.contains("balance_decimal"));
        assert!(INSERT_BALANCE_SNAPSHOT_QUERY.contains("block_number"));
        assert!(INSERT_BALANCE_SNAPSHOT_QUERY.contains("source_provider_id"));
        assert!(INSERT_BALANCE_SNAPSHOT_QUERY.contains("RETURNING id"));
    }
}
