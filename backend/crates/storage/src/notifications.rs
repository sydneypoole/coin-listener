use std::collections::HashSet;

use chrono::{DateTime, Utc};
use coin_listener_core::{
    models::{
        AddressEvent, CreateNotificationChannelRequest, CreateNotificationRuleRequest,
        InAppNotification, InAppNotificationQuery, NotificationChannel, NotificationDelivery,
        NotificationDeliveryListItem, NotificationDeliveryQuery, NotificationRule,
    },
    AppError, AppResult,
};
use sqlx::PgPool;
use uuid::Uuid;

pub const DEFAULT_TENANT_ID: Uuid = Uuid::from_u128(1);
pub const DEFAULT_IN_APP_CHANNEL_NAME: &str = "Default In-App";
const CHANNEL_TYPE_IN_APP: &str = "in_app";
const CHANNEL_TYPE_TELEGRAM: &str = "telegram";
const CHANNEL_TYPE_WEBHOOK: &str = "webhook";
const STATUS_ACTIVE: &str = "active";
const STATUS_INACTIVE: &str = "inactive";
const DELIVERY_STATUS_PROCESSING: &str = "processing";
const DELIVERY_STATUS_SENT: &str = "sent";
const DELIVERY_STATUS_SKIPPED: &str = "skipped";
const DELIVERY_STATUS_FAILED: &str = "failed";

pub const LIST_NOTIFICATION_CHANNELS_QUERY: &str = r#"
        SELECT id, tenant_id, channel_type, name, config, status, created_at, updated_at
        FROM notification_channels
        WHERE tenant_id = $1
        ORDER BY created_at DESC
        "#;

pub const LIST_NOTIFICATION_RULES_QUERY: &str = r#"
        SELECT id, tenant_id, name, chain_id, address_id, asset_id, event_type, is_transfer,
               min_amount_raw, direction, channel_ids, enabled, created_at, updated_at
        FROM notification_rules
        WHERE tenant_id = $1
        ORDER BY created_at DESC
        "#;

pub const UPDATE_NOTIFICATION_RULE_QUERY: &str = r#"
        UPDATE notification_rules
        SET name = $2,
            chain_id = $3,
            address_id = $4,
            asset_id = $5,
            event_type = $6,
            is_transfer = $7,
            min_amount_raw = $8,
            direction = $9,
            channel_ids = $10,
            enabled = $11,
            updated_at = NOW()
        WHERE id = $1
          AND tenant_id = $12
        RETURNING id, tenant_id, name, chain_id, address_id, asset_id, event_type, is_transfer,
                  min_amount_raw, direction, channel_ids, enabled, created_at, updated_at
        "#;

pub const DELETE_NOTIFICATION_RULE_QUERY: &str =
    "DELETE FROM notification_rules WHERE id = $1 AND tenant_id = $2";

pub const LIST_IN_APP_NOTIFICATIONS_QUERY: &str = r#"
        SELECT id, tenant_id, event_id, delivery_id, title, body, read_at, created_at
        FROM in_app_notifications
        WHERE tenant_id = $1
          AND ($2::boolean IS NULL OR read_at IS NULL)
        ORDER BY created_at DESC
        LIMIT 200
        "#;

pub const MARK_IN_APP_NOTIFICATION_READ_QUERY: &str = r#"
        UPDATE in_app_notifications
        SET read_at = COALESCE(read_at, NOW())
        WHERE id = $1
          AND tenant_id = $2
        RETURNING id, tenant_id, event_id, delivery_id, title, body, read_at, created_at
        "#;

const CREATE_IN_APP_NOTIFICATION_QUERY: &str = r#"
        INSERT INTO in_app_notifications (tenant_id, event_id, delivery_id, title, body)
        SELECT $1, $2, $3, $4, $5
        WHERE EXISTS (
            SELECT 1 FROM address_events
            WHERE id = $2
              AND tenant_id = $1
        )
          AND ($3::uuid IS NULL OR EXISTS (
              SELECT 1 FROM notification_deliveries
              WHERE id = $3
                AND tenant_id = $1
                AND event_id = $2
          ))
        RETURNING id, tenant_id, event_id, delivery_id, title, body, read_at, created_at
        "#;

pub const CREATE_IN_APP_NOTIFICATION_DELIVERY_QUERY: &str = r#"
        INSERT INTO notification_deliveries (
            tenant_id, event_id, rule_id, channel_id, channel_type, status, attempt_count,
            last_error, sent_at
        )
        SELECT $1, $2, $3, $4, 'in_app', $5, $6, $7, $8
        WHERE EXISTS (
            SELECT 1 FROM address_events
            WHERE id = $2
              AND tenant_id = $1
        )
          AND ($3::uuid IS NULL OR EXISTS (
              SELECT 1 FROM notification_rules
              WHERE id = $3
                AND tenant_id = $1
          ))
          AND ($4::uuid IS NULL OR EXISTS (
              SELECT 1 FROM notification_channels
              WHERE id = $4
                AND tenant_id = $1
                AND channel_type = 'in_app'
          ))
        RETURNING id, tenant_id, event_id, rule_id, channel_id, status, attempt_count,
                  last_error, sent_at, created_at, channel_type, idempotency_key,
                  provider_message_id, provider_status_code, provider_response
        "#;

pub const SELECT_EXTERNAL_NOTIFICATION_DELIVERY_FOR_UPDATE_QUERY: &str = r#"
        SELECT id, status, attempt_count
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
                AND channel_type = $5
          )
        ON CONFLICT (event_id, rule_id, channel_id, idempotency_key)
            WHERE idempotency_key IS NOT NULL
            DO NOTHING
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
          AND attempt_count < $3
        RETURNING id, tenant_id, event_id, rule_id, channel_id, channel_type, status, attempt_count,
                  idempotency_key, provider_message_id, provider_status_code, provider_response,
                  last_error, sent_at, created_at
        "#;

pub const MARK_EXTERNAL_NOTIFICATION_DELIVERY_SENT_QUERY: &str = r#"
        UPDATE notification_deliveries
        SET status = 'sent',
            last_error = NULL,
            sent_at = $4,
            provider_message_id = $5,
            provider_status_code = $6,
            provider_response = $7
        WHERE id = $1
          AND tenant_id = $2
          AND status = 'processing'
          AND attempt_count = $3
        RETURNING id, tenant_id, event_id, rule_id, channel_id, channel_type, status, attempt_count,
                  idempotency_key, provider_message_id, provider_status_code, provider_response,
                  last_error, sent_at, created_at
        "#;

pub const MARK_EXTERNAL_NOTIFICATION_DELIVERY_FAILED_QUERY: &str = r#"
        UPDATE notification_deliveries
        SET status = 'failed',
            last_error = $4,
            provider_status_code = $5,
            provider_response = $6,
            sent_at = NULL,
            provider_message_id = NULL
        WHERE id = $1
          AND tenant_id = $2
          AND status = 'processing'
          AND attempt_count = $3
        RETURNING id, tenant_id, event_id, rule_id, channel_id, channel_type, status, attempt_count,
                  idempotency_key, provider_message_id, provider_status_code, provider_response,
                  last_error, sent_at, created_at
        "#;

pub const LIST_NOTIFICATION_DELIVERIES_QUERY: &str = r#"
SELECT id,
       tenant_id,
       event_id,
       rule_id,
       channel_id,
       channel_type,
       status,
       attempt_count,
       last_error,
       sent_at,
       created_at,
       idempotency_key,
       provider_message_id,
       provider_status_code,
       provider_response
FROM notification_deliveries
WHERE tenant_id = $1
  AND ($2::uuid IS NULL OR event_id = $2)
  AND ($3::text IS NULL OR status = $3)
  AND ($4::text IS NULL OR channel_type = $4)
  AND ($5::uuid IS NULL OR rule_id = $5)
  AND ($6::uuid IS NULL OR channel_id = $6)
ORDER BY created_at DESC
LIMIT $7 OFFSET $8
"#;

pub const LIST_NOTIFICATION_DELIVERIES_FOR_EVENT_QUERY: &str = r#"
SELECT id,
       tenant_id,
       event_id,
       rule_id,
       channel_id,
       channel_type,
       status,
       attempt_count,
       last_error,
       sent_at,
       created_at,
       idempotency_key,
       provider_message_id,
       provider_status_code,
       provider_response
FROM notification_deliveries
WHERE tenant_id = $1
  AND event_id = $2
ORDER BY created_at DESC
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalDeliveryStart {
    AlreadyComplete { delivery_id: Uuid },
    ReadyToSend { delivery_id: Uuid },
}

pub fn validate_notification_channel_request(
    request: &CreateNotificationChannelRequest,
) -> AppResult<()> {
    if request.name.trim().is_empty() {
        return Err(AppError::Validation("channel name is required".to_string()));
    }
    if !matches!(
        request.channel_type.as_str(),
        CHANNEL_TYPE_IN_APP | CHANNEL_TYPE_TELEGRAM | CHANNEL_TYPE_WEBHOOK
    ) {
        return Err(AppError::Validation(
            "channel_type must be in_app, telegram, or webhook".to_string(),
        ));
    }
    if let Some(status) = &request.status {
        if !matches!(status.as_str(), STATUS_ACTIVE | STATUS_INACTIVE) {
            return Err(AppError::Validation(
                "status must be active or inactive".to_string(),
            ));
        }
    }
    Ok(())
}

pub fn validate_notification_rule_request(
    request: &CreateNotificationRuleRequest,
) -> AppResult<()> {
    if request.name.trim().is_empty() {
        return Err(AppError::Validation("rule name is required".to_string()));
    }
    if let Some(min_amount_raw) = &request.min_amount_raw {
        if min_amount_raw.is_empty()
            || !min_amount_raw
                .chars()
                .all(|character| character.is_ascii_digit())
        {
            return Err(AppError::Validation(
                "min_amount_raw must be a non-negative integer string".to_string(),
            ));
        }
    }
    if let Some(direction) = &request.direction {
        if !matches!(direction.as_str(), "in" | "out" | "self" | "unknown") {
            return Err(AppError::Validation(
                "direction must be in, out, self, or unknown".to_string(),
            ));
        }
    }
    if let Some(channel_ids) = &request.channel_ids {
        let unique_channel_ids = channel_ids.iter().copied().collect::<HashSet<_>>();
        if unique_channel_ids.len() != channel_ids.len() {
            return Err(AppError::Validation(
                "channel_ids must be unique".to_string(),
            ));
        }
    }
    Ok(())
}

pub fn validate_notification_delivery_status(status: &str) -> AppResult<()> {
    if !matches!(
        status,
        DELIVERY_STATUS_PROCESSING
            | DELIVERY_STATUS_SENT
            | DELIVERY_STATUS_SKIPPED
            | DELIVERY_STATUS_FAILED
    ) {
        return Err(AppError::Validation(
            "delivery status must be processing, sent, skipped, or failed".to_string(),
        ));
    }
    Ok(())
}

pub fn validate_notification_delivery_channel_type(channel_type: &str) -> AppResult<()> {
    if !matches!(
        channel_type,
        CHANNEL_TYPE_IN_APP | CHANNEL_TYPE_TELEGRAM | CHANNEL_TYPE_WEBHOOK
    ) {
        return Err(AppError::Validation(
            "channel_type must be in_app, telegram, or webhook".to_string(),
        ));
    }
    Ok(())
}

pub fn notification_delivery_ops_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(50).clamp(1, 100)
}

pub fn notification_delivery_ops_offset(offset: Option<i64>) -> i64 {
    offset.unwrap_or(0).max(0)
}

pub fn external_delivery_start_from_status(
    delivery_id: Uuid,
    status: &str,
) -> ExternalDeliveryStart {
    external_delivery_start_from_status_and_attempt(delivery_id, status, 0, 1)
}

pub fn external_delivery_start_from_status_and_attempt(
    delivery_id: Uuid,
    status: &str,
    existing_attempt_count: i32,
    requested_attempt_count: i32,
) -> ExternalDeliveryStart {
    match status {
        DELIVERY_STATUS_SENT | DELIVERY_STATUS_SKIPPED => {
            ExternalDeliveryStart::AlreadyComplete { delivery_id }
        }
        _ if existing_attempt_count >= requested_attempt_count => {
            ExternalDeliveryStart::AlreadyComplete { delivery_id }
        }
        _ => ExternalDeliveryStart::ReadyToSend { delivery_id },
    }
}

pub fn validate_notification_rule_reference_consistency(
    chain_id: Option<Uuid>,
    address_chain_id: Option<Uuid>,
    asset_chain_id: Option<Uuid>,
) -> AppResult<()> {
    if let (Some(chain_id), Some(address_chain_id)) = (chain_id, address_chain_id) {
        if chain_id != address_chain_id {
            return Err(AppError::Validation(
                "address_id must belong to chain_id".to_string(),
            ));
        }
    }
    if let (Some(chain_id), Some(asset_chain_id)) = (chain_id, asset_chain_id) {
        if chain_id != asset_chain_id {
            return Err(AppError::Validation(
                "asset_id must belong to chain_id".to_string(),
            ));
        }
    }
    if chain_id.is_none() {
        if let (Some(address_chain_id), Some(asset_chain_id)) = (address_chain_id, asset_chain_id) {
            if address_chain_id != asset_chain_id {
                return Err(AppError::Validation(
                    "address_id and asset_id must belong to the same chain".to_string(),
                ));
            }
        }
    }
    Ok(())
}

pub async fn list_notification_channels(
    pool: &PgPool,
    tenant_id: Uuid,
) -> AppResult<Vec<NotificationChannel>> {
    sqlx::query_as::<_, NotificationChannel>(LIST_NOTIFICATION_CHANNELS_QUERY)
        .bind(tenant_id)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn create_notification_channel(
    pool: &PgPool,
    tenant_id: Uuid,
    request: CreateNotificationChannelRequest,
) -> AppResult<NotificationChannel> {
    validate_notification_channel_request(&request)?;
    let config = request.config.unwrap_or_else(|| serde_json::json!({}));
    let status = request.status.unwrap_or_else(|| STATUS_ACTIVE.to_string());

    sqlx::query_as::<_, NotificationChannel>(
        r#"
        INSERT INTO notification_channels (tenant_id, channel_type, name, config, status)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, tenant_id, channel_type, name, config, status, created_at, updated_at
        "#,
    )
    .bind(tenant_id)
    .bind(request.channel_type)
    .bind(request.name)
    .bind(config)
    .bind(status)
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn list_notification_rules(
    pool: &PgPool,
    tenant_id: Uuid,
) -> AppResult<Vec<NotificationRule>> {
    sqlx::query_as::<_, NotificationRule>(LIST_NOTIFICATION_RULES_QUERY)
        .bind(tenant_id)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn list_enabled_notification_rules(
    pool: &PgPool,
    tenant_id: Uuid,
) -> AppResult<Vec<NotificationRule>> {
    sqlx::query_as::<_, NotificationRule>(
        r#"
        SELECT id, tenant_id, name, chain_id, address_id, asset_id, event_type, is_transfer,
               min_amount_raw, direction, channel_ids, enabled, created_at, updated_at
        FROM notification_rules
        WHERE tenant_id = $1
          AND enabled = TRUE
        ORDER BY created_at ASC
        "#,
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn create_notification_rule(
    pool: &PgPool,
    tenant_id: Uuid,
    request: CreateNotificationRuleRequest,
) -> AppResult<NotificationRule> {
    validate_notification_rule_request(&request)?;
    validate_notification_rule_references(pool, tenant_id, &request).await?;
    let channel_ids = request.channel_ids.unwrap_or_default();
    let enabled = request.enabled.unwrap_or(true);

    sqlx::query_as::<_, NotificationRule>(
        r#"
        INSERT INTO notification_rules (
            tenant_id, name, chain_id, address_id, asset_id, event_type, is_transfer,
            min_amount_raw, direction, channel_ids, enabled
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        RETURNING id, tenant_id, name, chain_id, address_id, asset_id, event_type, is_transfer,
                  min_amount_raw, direction, channel_ids, enabled, created_at, updated_at
        "#,
    )
    .bind(tenant_id)
    .bind(request.name)
    .bind(request.chain_id)
    .bind(request.address_id)
    .bind(request.asset_id)
    .bind(request.event_type)
    .bind(request.is_transfer)
    .bind(request.min_amount_raw)
    .bind(request.direction)
    .bind(channel_ids)
    .bind(enabled)
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn update_notification_rule(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
    request: CreateNotificationRuleRequest,
) -> AppResult<NotificationRule> {
    validate_notification_rule_request(&request)?;
    validate_notification_rule_references(pool, tenant_id, &request).await?;
    let channel_ids = request.channel_ids.unwrap_or_default();
    let enabled = request.enabled.unwrap_or(true);

    sqlx::query_as::<_, NotificationRule>(UPDATE_NOTIFICATION_RULE_QUERY)
        .bind(id)
        .bind(request.name)
        .bind(request.chain_id)
        .bind(request.address_id)
        .bind(request.asset_id)
        .bind(request.event_type)
        .bind(request.is_transfer)
        .bind(request.min_amount_raw)
        .bind(request.direction)
        .bind(channel_ids)
        .bind(enabled)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("notification rule".to_string()))
}

pub async fn delete_notification_rule(pool: &PgPool, tenant_id: Uuid, id: Uuid) -> AppResult<()> {
    let result = sqlx::query(DELETE_NOTIFICATION_RULE_QUERY)
        .bind(id)
        .bind(tenant_id)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("notification rule".to_string()));
    }

    Ok(())
}

pub async fn list_in_app_notifications(
    pool: &PgPool,
    tenant_id: Uuid,
    query: InAppNotificationQuery,
) -> AppResult<Vec<InAppNotification>> {
    sqlx::query_as::<_, InAppNotification>(LIST_IN_APP_NOTIFICATIONS_QUERY)
        .bind(tenant_id)
        .bind(query.unread_only.filter(|value| *value))
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn mark_in_app_notification_read(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
) -> AppResult<InAppNotification> {
    sqlx::query_as::<_, InAppNotification>(MARK_IN_APP_NOTIFICATION_READ_QUERY)
        .bind(id)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("in-app notification".to_string()))
}

pub async fn get_address_event(
    pool: &PgPool,
    event_id: Uuid,
    tenant_id: Uuid,
) -> AppResult<AddressEvent> {
    sqlx::query_as::<_, AddressEvent>(
        r#"
        SELECT id, tenant_id, chain_id, address_id, asset_id, event_type, direction, is_transfer,
               tx_hash, log_index, block_number, block_hash, confirmations, from_address, to_address,
               amount_raw, amount_decimal, balance_before_raw, balance_after_raw, balance_delta_raw,
               metadata, detected_at, created_at
        FROM address_events
        WHERE id = $1
          AND tenant_id = $2
        "#,
    )
    .bind(event_id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("address event".to_string()))
}

pub async fn list_channels_by_ids(
    pool: &PgPool,
    tenant_id: Uuid,
    channel_ids: &[Uuid],
) -> AppResult<Vec<NotificationChannel>> {
    sqlx::query_as::<_, NotificationChannel>(
        r#"
        SELECT id, tenant_id, channel_type, name, config, status, created_at, updated_at
        FROM notification_channels
        WHERE tenant_id = $1
          AND id = ANY($2)
        ORDER BY created_at ASC
        "#,
    )
    .bind(tenant_id)
    .bind(channel_ids)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn list_active_channels_by_ids(
    pool: &PgPool,
    tenant_id: Uuid,
    channel_ids: &[Uuid],
) -> AppResult<Vec<NotificationChannel>> {
    sqlx::query_as::<_, NotificationChannel>(
        r#"
        SELECT id, tenant_id, channel_type, name, config, status, created_at, updated_at
        FROM notification_channels
        WHERE tenant_id = $1
          AND status = 'active'
          AND id = ANY($2)
        ORDER BY created_at ASC
        "#,
    )
    .bind(tenant_id)
    .bind(channel_ids)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn get_or_create_default_in_app_channel(
    pool: &PgPool,
    tenant_id: Uuid,
) -> AppResult<NotificationChannel> {
    if let Some(channel) = sqlx::query_as::<_, NotificationChannel>(
        r#"
        SELECT id, tenant_id, channel_type, name, config, status, created_at, updated_at
        FROM notification_channels
        WHERE tenant_id = $1
          AND channel_type = 'in_app'
          AND status = 'active'
        ORDER BY created_at ASC
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    {
        return Ok(channel);
    }

    let inserted = sqlx::query_as::<_, NotificationChannel>(
        r#"
        INSERT INTO notification_channels (tenant_id, channel_type, name, config, status)
        VALUES ($1, 'in_app', $2, '{}'::jsonb, 'active')
        ON CONFLICT DO NOTHING
        RETURNING id, tenant_id, channel_type, name, config, status, created_at, updated_at
        "#,
    )
    .bind(tenant_id)
    .bind(DEFAULT_IN_APP_CHANNEL_NAME)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?;

    if let Some(channel) = inserted {
        return Ok(channel);
    }

    sqlx::query_as::<_, NotificationChannel>(
        r#"
        SELECT id, tenant_id, channel_type, name, config, status, created_at, updated_at
        FROM notification_channels
        WHERE tenant_id = $1
          AND channel_type = 'in_app'
          AND name = $2
          AND status = 'active'
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .bind(DEFAULT_IN_APP_CHANNEL_NAME)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("default in-app channel".to_string()))
}

pub async fn create_notification_delivery(
    pool: &PgPool,
    tenant_id: Uuid,
    event_id: Uuid,
    rule_id: Option<Uuid>,
    channel_id: Option<Uuid>,
    status: &str,
    attempt_count: i32,
    last_error: Option<String>,
    sent_at: Option<DateTime<Utc>>,
) -> AppResult<NotificationDelivery> {
    validate_notification_delivery_status(status)?;

    sqlx::query_as::<_, NotificationDelivery>(
        r#"
        INSERT INTO notification_deliveries (
            tenant_id, event_id, rule_id, channel_id, status, attempt_count, last_error, sent_at
        )
        SELECT $1, $2, $3, $4, $5, $6, $7, $8
        WHERE EXISTS (
            SELECT 1 FROM address_events
            WHERE id = $2
              AND tenant_id = $1
        )
          AND ($3::uuid IS NULL OR EXISTS (
              SELECT 1 FROM notification_rules
              WHERE id = $3
                AND tenant_id = $1
          ))
          AND ($4::uuid IS NULL OR EXISTS (
              SELECT 1 FROM notification_channels
              WHERE id = $4
                AND tenant_id = $1
          ))
        RETURNING id, tenant_id, event_id, rule_id, channel_id, status, attempt_count,
                  last_error, sent_at, created_at, channel_type, idempotency_key,
                  provider_message_id, provider_status_code, provider_response
        "#,
    )
    .bind(tenant_id)
    .bind(event_id)
    .bind(rule_id)
    .bind(channel_id)
    .bind(status)
    .bind(attempt_count)
    .bind(last_error)
    .bind(sent_at)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("notification delivery target".to_string()))
}

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

    let inserted =
        sqlx::query_as::<_, NotificationDelivery>(INSERT_EXTERNAL_NOTIFICATION_DELIVERY_QUERY)
            .bind(tenant_id)
            .bind(event_id)
            .bind(rule_id)
            .bind(channel_id)
            .bind(channel_type)
            .bind(attempt_count)
            .bind(idempotency_key)
            .fetch_optional(transaction.as_mut())
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;

    let start = if let Some(delivery) = inserted {
        ExternalDeliveryStart::ReadyToSend {
            delivery_id: delivery.id,
        }
    } else if let Some((delivery_id, status, existing_attempt_count)) =
        sqlx::query_as::<_, (Uuid, String, i32)>(
            SELECT_EXTERNAL_NOTIFICATION_DELIVERY_FOR_UPDATE_QUERY,
        )
        .bind(tenant_id)
        .bind(event_id)
        .bind(rule_id)
        .bind(channel_id)
        .bind(idempotency_key)
        .fetch_optional(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
    {
        match external_delivery_start_from_status_and_attempt(
            delivery_id,
            &status,
            existing_attempt_count,
            attempt_count,
        ) {
            ExternalDeliveryStart::AlreadyComplete { delivery_id } => {
                ExternalDeliveryStart::AlreadyComplete { delivery_id }
            }
            ExternalDeliveryStart::ReadyToSend { delivery_id } => {
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

                ExternalDeliveryStart::ReadyToSend { delivery_id }
            }
        }
    } else {
        return Err(AppError::NotFound(
            "external notification delivery target".to_string(),
        ));
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
    attempt_count: i32,
    sent_at: DateTime<Utc>,
    provider_message_id: Option<&str>,
    provider_status_code: Option<i32>,
    provider_response: Option<&str>,
) -> AppResult<NotificationDelivery> {
    sqlx::query_as::<_, NotificationDelivery>(MARK_EXTERNAL_NOTIFICATION_DELIVERY_SENT_QUERY)
        .bind(delivery_id)
        .bind(tenant_id)
        .bind(attempt_count)
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
    attempt_count: i32,
    last_error: &str,
    provider_status_code: Option<i32>,
    provider_response: Option<&str>,
) -> AppResult<NotificationDelivery> {
    sqlx::query_as::<_, NotificationDelivery>(MARK_EXTERNAL_NOTIFICATION_DELIVERY_FAILED_QUERY)
        .bind(delivery_id)
        .bind(tenant_id)
        .bind(attempt_count)
        .bind(last_error)
        .bind(provider_status_code)
        .bind(provider_response)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("external notification delivery".to_string()))
}

pub async fn list_notification_deliveries(
    pool: &PgPool,
    tenant_id: Uuid,
    query: NotificationDeliveryQuery,
) -> AppResult<Vec<NotificationDeliveryListItem>> {
    if let Some(status) = query.status.as_deref() {
        validate_notification_delivery_status(status)?;
    }
    if let Some(channel_type) = query.channel_type.as_deref() {
        validate_notification_delivery_channel_type(channel_type)?;
    }

    sqlx::query_as::<_, NotificationDeliveryListItem>(LIST_NOTIFICATION_DELIVERIES_QUERY)
        .bind(tenant_id)
        .bind(query.event_id)
        .bind(query.status)
        .bind(query.channel_type)
        .bind(query.rule_id)
        .bind(query.channel_id)
        .bind(notification_delivery_ops_limit(query.limit))
        .bind(notification_delivery_ops_offset(query.offset))
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn list_notification_deliveries_for_event(
    pool: &PgPool,
    tenant_id: Uuid,
    event_id: Uuid,
) -> AppResult<Vec<NotificationDeliveryListItem>> {
    sqlx::query_as::<_, NotificationDeliveryListItem>(LIST_NOTIFICATION_DELIVERIES_FOR_EVENT_QUERY)
        .bind(tenant_id)
        .bind(event_id)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

async fn create_in_app_notification_delivery_with_executor(
    executor: &mut sqlx::PgConnection,
    tenant_id: Uuid,
    event_id: Uuid,
    rule_id: Option<Uuid>,
    channel_id: Option<Uuid>,
    status: &str,
    attempt_count: i32,
    last_error: Option<String>,
    sent_at: Option<DateTime<Utc>>,
) -> AppResult<NotificationDelivery> {
    validate_notification_delivery_status(status)?;

    sqlx::query_as::<_, NotificationDelivery>(CREATE_IN_APP_NOTIFICATION_DELIVERY_QUERY)
        .bind(tenant_id)
        .bind(event_id)
        .bind(rule_id)
        .bind(channel_id)
        .bind(status)
        .bind(attempt_count)
        .bind(last_error)
        .bind(sent_at)
        .fetch_optional(executor)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("notification delivery target".to_string()))
}

pub async fn update_notification_delivery_status(
    pool: &PgPool,
    id: Uuid,
    tenant_id: Uuid,
    status: &str,
    last_error: Option<String>,
    sent_at: Option<DateTime<Utc>>,
) -> AppResult<NotificationDelivery> {
    validate_notification_delivery_status(status)?;

    sqlx::query_as::<_, NotificationDelivery>(
        r#"
        UPDATE notification_deliveries
        SET status = $3,
            last_error = $4,
            sent_at = $5
        WHERE id = $1
          AND tenant_id = $2
        RETURNING id, tenant_id, event_id, rule_id, channel_id, status, attempt_count,
                  last_error, sent_at, created_at, channel_type, idempotency_key,
                  provider_message_id, provider_status_code, provider_response
        "#,
    )
    .bind(id)
    .bind(tenant_id)
    .bind(status)
    .bind(last_error)
    .bind(sent_at)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("notification delivery".to_string()))
}

pub async fn create_in_app_notification(
    pool: &PgPool,
    tenant_id: Uuid,
    event_id: Uuid,
    delivery_id: Option<Uuid>,
    title: String,
    body: String,
) -> AppResult<InAppNotification> {
    sqlx::query_as::<_, InAppNotification>(CREATE_IN_APP_NOTIFICATION_QUERY)
        .bind(tenant_id)
        .bind(event_id)
        .bind(delivery_id)
        .bind(title)
        .bind(body)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("in-app notification target".to_string()))
}

pub async fn create_sent_in_app_delivery(
    pool: &PgPool,
    tenant_id: Uuid,
    event_id: Uuid,
    rule_id: Option<Uuid>,
    channel_id: Option<Uuid>,
    attempt_count: i32,
    sent_at: DateTime<Utc>,
    title: String,
    body: String,
) -> AppResult<(NotificationDelivery, InAppNotification)> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let delivery = create_in_app_notification_delivery_with_executor(
        transaction.as_mut(),
        tenant_id,
        event_id,
        rule_id,
        channel_id,
        DELIVERY_STATUS_SENT,
        attempt_count,
        None,
        Some(sent_at),
    )
    .await?;

    let notification = sqlx::query_as::<_, InAppNotification>(CREATE_IN_APP_NOTIFICATION_QUERY)
        .bind(tenant_id)
        .bind(event_id)
        .bind(Some(delivery.id))
        .bind(title)
        .bind(body)
        .fetch_optional(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("in-app notification target".to_string()))?;

    transaction
        .commit()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    Ok((delivery, notification))
}

async fn validate_notification_rule_references(
    pool: &PgPool,
    tenant_id: Uuid,
    request: &CreateNotificationRuleRequest,
) -> AppResult<()> {
    if let Some(chain_id) = request.chain_id {
        let exists =
            sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM chains WHERE id = $1)")
                .bind(chain_id)
                .fetch_one(pool)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
        if !exists {
            return Err(AppError::Validation("chain_id must exist".to_string()));
        }
    }

    let address_chain_id = if let Some(address_id) = request.address_id {
        let chain_id = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT chain_id
            FROM watched_addresses
            WHERE id = $1
              AND tenant_id = $2
            "#,
        )
        .bind(address_id)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
        if chain_id.is_none() {
            return Err(AppError::Validation(
                "address_id must belong to tenant".to_string(),
            ));
        }
        chain_id
    } else {
        None
    };

    let asset_chain_id = if let Some(asset_id) = request.asset_id {
        let chain_id = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT chain_id
            FROM assets
            WHERE id = $1
            "#,
        )
        .bind(asset_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
        if chain_id.is_none() {
            return Err(AppError::Validation("asset_id must exist".to_string()));
        }
        chain_id
    } else {
        None
    };

    validate_notification_rule_reference_consistency(
        request.chain_id,
        address_chain_id,
        asset_chain_id,
    )?;

    if let Some(channel_ids) = request.channel_ids.as_deref() {
        validate_notification_rule_channel_ids(pool, tenant_id, channel_ids).await?;
    }

    Ok(())
}

async fn validate_notification_rule_channel_ids(
    pool: &PgPool,
    tenant_id: Uuid,
    channel_ids: &[Uuid],
) -> AppResult<()> {
    if channel_ids.is_empty() {
        return Ok(());
    }

    let unique_channel_ids = channel_ids.iter().copied().collect::<HashSet<_>>();
    let unique_channel_ids = unique_channel_ids.into_iter().collect::<Vec<_>>();
    let count = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COUNT(*)
        FROM notification_channels
        WHERE tenant_id = $1
          AND id = ANY($2)
        "#,
    )
    .bind(tenant_id)
    .bind(&unique_channel_ids)
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?;

    if count != unique_channel_ids.len() as i64 {
        return Err(AppError::Validation(
            "channel_ids must belong to tenant".to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        external_delivery_start_from_status, external_delivery_start_from_status_and_attempt,
        validate_notification_channel_request, validate_notification_delivery_status,
        validate_notification_rule_reference_consistency, validate_notification_rule_request,
        ExternalDeliveryStart, CREATE_IN_APP_NOTIFICATION_DELIVERY_QUERY,
        CREATE_IN_APP_NOTIFICATION_QUERY, DEFAULT_IN_APP_CHANNEL_NAME,
        DELETE_NOTIFICATION_RULE_QUERY, INSERT_EXTERNAL_NOTIFICATION_DELIVERY_QUERY,
        LIST_IN_APP_NOTIFICATIONS_QUERY, LIST_NOTIFICATION_CHANNELS_QUERY,
        LIST_NOTIFICATION_DELIVERIES_FOR_EVENT_QUERY, LIST_NOTIFICATION_DELIVERIES_QUERY,
        LIST_NOTIFICATION_RULES_QUERY, MARK_EXTERNAL_NOTIFICATION_DELIVERY_FAILED_QUERY,
        MARK_EXTERNAL_NOTIFICATION_DELIVERY_SENT_QUERY, MARK_IN_APP_NOTIFICATION_READ_QUERY,
        SELECT_EXTERNAL_NOTIFICATION_DELIVERY_FOR_UPDATE_QUERY,
        UPDATE_EXTERNAL_NOTIFICATION_DELIVERY_PROCESSING_QUERY, UPDATE_NOTIFICATION_RULE_QUERY,
    };
    use coin_listener_core::{
        models::{
            CreateNotificationChannelRequest, CreateNotificationRuleRequest,
            NotificationDeliveryListItem, NotificationDeliveryQuery,
        },
        AppError, AppResult,
    };
    use sqlx::PgPool;
    use uuid::Uuid;

    #[test]
    fn channel_validation_rejects_unknown_type() {
        let request = CreateNotificationChannelRequest {
            channel_type: "email".to_string(),
            name: "Email".to_string(),
            config: None,
            status: Some("active".to_string()),
        };

        let result = validate_notification_channel_request(&request);

        assert!(matches!(
            result,
            Err(AppError::Validation(message)) if message == "channel_type must be in_app, telegram, or webhook"
        ));
    }

    #[test]
    fn rule_validation_rejects_invalid_min_amount_raw() {
        let request = CreateNotificationRuleRequest {
            name: "Large transfers".to_string(),
            chain_id: None,
            address_id: None,
            asset_id: None,
            event_type: None,
            is_transfer: None,
            min_amount_raw: Some("12.5".to_string()),
            direction: None,
            channel_ids: None,
            enabled: Some(true),
        };

        let result = validate_notification_rule_request(&request);

        assert!(matches!(
            result,
            Err(AppError::Validation(message)) if message == "min_amount_raw must be a non-negative integer string"
        ));
    }

    #[test]
    fn rule_validation_rejects_duplicate_channel_ids() {
        let channel_id = Uuid::from_u128(7);
        let request = CreateNotificationRuleRequest {
            name: "Large transfers".to_string(),
            chain_id: None,
            address_id: None,
            asset_id: None,
            event_type: None,
            is_transfer: None,
            min_amount_raw: None,
            direction: None,
            channel_ids: Some(vec![channel_id, channel_id]),
            enabled: Some(true),
        };

        let result = validate_notification_rule_request(&request);

        assert!(matches!(
            result,
            Err(AppError::Validation(message)) if message == "channel_ids must be unique"
        ));
    }

    #[test]
    fn default_in_app_channel_name_is_stable() {
        assert_eq!(DEFAULT_IN_APP_CHANNEL_NAME, "Default In-App");
    }

    #[test]
    fn delivery_status_validation_rejects_unknown_status() {
        let result = validate_notification_delivery_status("pending");

        assert!(matches!(
            result,
            Err(AppError::Validation(message)) if message == "delivery status must be processing, sent, skipped, or failed"
        ));
    }

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
    fn external_delivery_start_blocks_same_or_older_non_terminal_attempt() {
        let delivery_id = Uuid::from_u128(43);

        let same_attempt =
            external_delivery_start_from_status_and_attempt(delivery_id, "processing", 3, 3);
        let older_attempt =
            external_delivery_start_from_status_and_attempt(delivery_id, "failed", 4, 3);

        assert_eq!(
            same_attempt,
            ExternalDeliveryStart::AlreadyComplete { delivery_id }
        );
        assert_eq!(
            older_attempt,
            ExternalDeliveryStart::AlreadyComplete { delivery_id }
        );
    }

    #[test]
    fn external_delivery_start_allows_newer_non_terminal_attempt() {
        let delivery_id = Uuid::from_u128(44);

        let start = external_delivery_start_from_status_and_attempt(delivery_id, "failed", 3, 4);

        assert_eq!(start, ExternalDeliveryStart::ReadyToSend { delivery_id });
    }

    #[test]
    fn external_delivery_start_keeps_sent_attempt_complete() {
        let delivery_id = Uuid::from_u128(45);

        let start = external_delivery_start_from_status_and_attempt(delivery_id, "sent", 3, 4);

        assert_eq!(
            start,
            ExternalDeliveryStart::AlreadyComplete { delivery_id }
        );
    }

    #[test]
    fn external_delivery_queries_use_idempotency_key_and_row_lock() {
        assert!(SELECT_EXTERNAL_NOTIFICATION_DELIVERY_FOR_UPDATE_QUERY.contains("FOR UPDATE"));
        assert!(
            SELECT_EXTERNAL_NOTIFICATION_DELIVERY_FOR_UPDATE_QUERY.contains("idempotency_key = $5")
        );
        assert!(SELECT_EXTERNAL_NOTIFICATION_DELIVERY_FOR_UPDATE_QUERY
            .contains("SELECT id, status, attempt_count"));
        assert!(INSERT_EXTERNAL_NOTIFICATION_DELIVERY_QUERY.contains("idempotency_key"));
        assert!(INSERT_EXTERNAL_NOTIFICATION_DELIVERY_QUERY.contains("channel_type = $5"));
        assert!(INSERT_EXTERNAL_NOTIFICATION_DELIVERY_QUERY.contains("ON CONFLICT"));
        assert!(UPDATE_EXTERNAL_NOTIFICATION_DELIVERY_PROCESSING_QUERY
            .contains("status = 'processing'"));
        assert!(
            UPDATE_EXTERNAL_NOTIFICATION_DELIVERY_PROCESSING_QUERY.contains("attempt_count < $3")
        );
        assert!(MARK_EXTERNAL_NOTIFICATION_DELIVERY_SENT_QUERY.contains("provider_message_id"));
        assert!(MARK_EXTERNAL_NOTIFICATION_DELIVERY_SENT_QUERY.contains("status = 'processing'"));
        assert!(MARK_EXTERNAL_NOTIFICATION_DELIVERY_SENT_QUERY.contains("attempt_count = $3"));
        assert!(MARK_EXTERNAL_NOTIFICATION_DELIVERY_FAILED_QUERY.contains("provider_status_code"));
        assert!(MARK_EXTERNAL_NOTIFICATION_DELIVERY_FAILED_QUERY.contains("status = 'processing'"));
        assert!(MARK_EXTERNAL_NOTIFICATION_DELIVERY_FAILED_QUERY.contains("attempt_count = $3"));
        assert!(MARK_EXTERNAL_NOTIFICATION_DELIVERY_FAILED_QUERY.contains("sent_at = NULL"));
        assert!(
            MARK_EXTERNAL_NOTIFICATION_DELIVERY_FAILED_QUERY.contains("provider_message_id = NULL")
        );
    }

    #[test]
    fn delivery_ops_validates_channel_type_filter() {
        for channel_type in ["in_app", "telegram", "webhook"] {
            assert!(super::validate_notification_delivery_channel_type(channel_type).is_ok());
        }
        assert!(super::validate_notification_delivery_channel_type("email").is_err());
    }

    #[test]
    fn delivery_ops_pagination_defaults_and_clamps() {
        let default_query = NotificationDeliveryQuery {
            event_id: None,
            status: None,
            channel_type: None,
            rule_id: None,
            channel_id: None,
            limit: None,
            offset: None,
        };
        assert_eq!(
            super::notification_delivery_ops_limit(default_query.limit),
            50
        );
        assert_eq!(
            super::notification_delivery_ops_offset(default_query.offset),
            0
        );
        assert_eq!(super::notification_delivery_ops_limit(Some(0)), 1);
        assert_eq!(super::notification_delivery_ops_limit(Some(500)), 100);
        assert_eq!(super::notification_delivery_ops_offset(Some(-10)), 0);
        assert_eq!(super::notification_delivery_ops_offset(Some(25)), 25);
    }

    #[test]
    fn notification_config_queries_filter_by_tenant_parameter() {
        assert!(LIST_NOTIFICATION_CHANNELS_QUERY.contains("WHERE tenant_id = $1"));
        assert!(LIST_NOTIFICATION_RULES_QUERY.contains("WHERE tenant_id = $1"));
        assert!(UPDATE_NOTIFICATION_RULE_QUERY.contains("WHERE id = $1"));
        assert!(UPDATE_NOTIFICATION_RULE_QUERY.contains("AND tenant_id = $12"));
        assert!(DELETE_NOTIFICATION_RULE_QUERY.contains("WHERE id = $1 AND tenant_id = $2"));
    }

    #[test]
    fn in_app_notification_queries_filter_by_tenant_parameter() {
        assert!(LIST_IN_APP_NOTIFICATIONS_QUERY.contains("WHERE tenant_id = $1"));
        assert!(MARK_IN_APP_NOTIFICATION_READ_QUERY.contains("WHERE id = $1"));
        assert!(MARK_IN_APP_NOTIFICATION_READ_QUERY.contains("AND tenant_id = $2"));
    }

    #[test]
    fn delivery_ops_list_query_filters_metadata_and_orders_newest_first() {
        assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("FROM notification_deliveries"));
        assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("tenant_id = $1"));
        assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("$2::uuid IS NULL OR event_id = $2"));
        assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("$3::text IS NULL OR status = $3"));
        assert!(
            LIST_NOTIFICATION_DELIVERIES_QUERY.contains("$4::text IS NULL OR channel_type = $4")
        );
        assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("$5::uuid IS NULL OR rule_id = $5"));
        assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("$6::uuid IS NULL OR channel_id = $6"));
        assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("provider_message_id"));
        assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("provider_status_code"));
        assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("provider_response"));
        assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("ORDER BY created_at DESC"));
        assert!(LIST_NOTIFICATION_DELIVERIES_QUERY.contains("LIMIT $7 OFFSET $8"));
    }

    #[test]
    fn delivery_ops_event_query_is_event_scoped() {
        assert!(LIST_NOTIFICATION_DELIVERIES_FOR_EVENT_QUERY.contains("tenant_id = $1"));
        assert!(LIST_NOTIFICATION_DELIVERIES_FOR_EVENT_QUERY.contains("event_id = $2"));
        assert!(LIST_NOTIFICATION_DELIVERIES_FOR_EVENT_QUERY.contains("ORDER BY created_at DESC"));
    }

    #[allow(dead_code)]
    async fn assert_list_notification_deliveries_signature(
        pool: &PgPool,
        tenant_id: uuid::Uuid,
        query: NotificationDeliveryQuery,
    ) -> AppResult<Vec<NotificationDeliveryListItem>> {
        super::list_notification_deliveries(pool, tenant_id, query).await
    }

    #[allow(dead_code)]
    async fn assert_list_notification_deliveries_for_event_signature(
        pool: &PgPool,
        tenant_id: uuid::Uuid,
        event_id: uuid::Uuid,
    ) -> AppResult<Vec<NotificationDeliveryListItem>> {
        super::list_notification_deliveries_for_event(pool, tenant_id, event_id).await
    }

    #[test]
    fn delivery_ops_helper_signatures_are_stable() {
        let _ = assert_list_notification_deliveries_signature;
        let _ = assert_list_notification_deliveries_for_event_signature;
    }

    #[test]
    fn rule_reference_consistency_rejects_address_chain_mismatch() {
        let chain_id = Uuid::from_u128(1);
        let address_chain_id = Uuid::from_u128(2);

        let result = validate_notification_rule_reference_consistency(
            Some(chain_id),
            Some(address_chain_id),
            None,
        );

        assert!(matches!(
            result,
            Err(AppError::Validation(message)) if message == "address_id must belong to chain_id"
        ));
    }

    #[test]
    fn in_app_delivery_rows_set_channel_type_for_ops_filtering() {
        assert!(CREATE_IN_APP_NOTIFICATION_DELIVERY_QUERY.contains("channel_type"));
        assert!(CREATE_IN_APP_NOTIFICATION_DELIVERY_QUERY.contains("'in_app'"));
    }

    #[test]
    fn in_app_notification_delivery_target_requires_same_event() {
        assert!(CREATE_IN_APP_NOTIFICATION_QUERY.contains("WHERE id = $3"));
        assert!(CREATE_IN_APP_NOTIFICATION_QUERY.contains("AND tenant_id = $1"));
        assert!(CREATE_IN_APP_NOTIFICATION_QUERY.contains("AND event_id = $2"));
    }

    #[test]
    fn external_delivery_migration_adds_metadata_and_idempotency_index() {
        let migration = include_str!("../migrations/0008_external_notification_deliveries.sql");

        assert!(migration.contains("ADD COLUMN IF NOT EXISTS channel_type TEXT"));
        assert!(migration.contains("ADD COLUMN IF NOT EXISTS idempotency_key TEXT"));
        assert!(migration.contains("ADD COLUMN IF NOT EXISTS provider_message_id TEXT"));
        assert!(migration.contains("ADD COLUMN IF NOT EXISTS provider_status_code INTEGER"));
        assert!(migration.contains("ADD COLUMN IF NOT EXISTS provider_response TEXT"));
        assert!(migration
            .contains("CREATE UNIQUE INDEX IF NOT EXISTS idx_notification_deliveries_idempotency"));
        assert!(migration.contains(
            "ON notification_deliveries(event_id, rule_id, channel_id, idempotency_key)"
        ));
        assert!(migration.contains("WHERE idempotency_key IS NOT NULL"));
    }
}
