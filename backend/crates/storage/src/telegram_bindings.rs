use chrono::{DateTime, Duration, Utc};
use coin_listener_core::{
    models::{TelegramBindingRequest, TelegramChatBinding},
    AppError, AppResult,
};
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
        BINDING_STATUS_PENDING
        | BINDING_STATUS_BOUND
        | BINDING_STATUS_CANCELLED
        | BINDING_STATUS_EXPIRED => Ok(()),
        _ => Err(AppError::Validation(
            "telegram binding status must be pending, bound, expired, or cancelled".to_string(),
        )),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct TelegramUpdateOffsetClaim {
    pub last_update_id: i64,
    pub locked_at: DateTime<Utc>,
}

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

pub const EXPIRE_BINDING_REQUEST_IF_DUE_QUERY: &str = r#"
    UPDATE telegram_binding_requests
    SET status = 'expired',
        updated_at = NOW()
    WHERE id = $1
      AND tenant_id = $2
      AND status = 'pending'
      AND expires_at <= $3
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

pub const UPDATE_BINDING_CONFIRMATION_ERROR_QUERY: &str = r#"
    UPDATE telegram_binding_requests
    SET confirmation_error = $3,
        updated_at = NOW()
    WHERE id = $1
      AND tenant_id = $2
      AND status = 'bound'
    "#;

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
    DO UPDATE SET last_update_id = GREATEST(
            telegram_bot_update_offsets.last_update_id,
            EXCLUDED.last_update_id
        ),
        updated_at = NOW()
    "#;

pub const CLAIM_TELEGRAM_UPDATE_OFFSET_QUERY: &str = r#"
    INSERT INTO telegram_bot_update_offsets (tenant_id, telegram_bot_id, last_update_id, locked_at)
    VALUES ($1, $2, 0, $3)
    ON CONFLICT (tenant_id, telegram_bot_id)
    DO UPDATE SET locked_at = EXCLUDED.locked_at,
                  updated_at = NOW()
    WHERE telegram_bot_update_offsets.locked_at IS NULL
       OR telegram_bot_update_offsets.locked_at < $4
    RETURNING last_update_id, locked_at
    "#;

pub const RELEASE_TELEGRAM_UPDATE_OFFSET_LOCK_QUERY: &str = r#"
    UPDATE telegram_bot_update_offsets
    SET locked_at = NULL,
        updated_at = NOW()
    WHERE tenant_id = $1
      AND telegram_bot_id = $2
      AND locked_at = $3
    "#;

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

pub async fn get_binding_request(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
) -> AppResult<TelegramBindingRequest> {
    expire_binding_request_if_due(pool, tenant_id, id, Utc::now()).await?;

    sqlx::query_as::<_, TelegramBindingRequest>(GET_BINDING_REQUEST_QUERY)
        .bind(id)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("telegram binding request".to_string()))
}

pub async fn expire_binding_request_if_due(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
    now: DateTime<Utc>,
) -> AppResult<()> {
    sqlx::query(EXPIRE_BINDING_REQUEST_IF_DUE_QUERY)
        .bind(id)
        .bind(tenant_id)
        .bind(now)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    Ok(())
}

pub async fn cancel_binding_request(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
) -> AppResult<TelegramBindingRequest> {
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

pub async fn update_binding_confirmation_error(
    pool: &PgPool,
    binding: &TelegramBindingRequest,
    confirmation_error: String,
) -> AppResult<()> {
    sqlx::query(UPDATE_BINDING_CONFIRMATION_ERROR_QUERY)
        .bind(binding.id)
        .bind(binding.tenant_id)
        .bind(confirmation_error)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    Ok(())
}

pub async fn get_telegram_update_offset(
    pool: &PgPool,
    tenant_id: Uuid,
    telegram_bot_id: Uuid,
) -> AppResult<i64> {
    sqlx::query_scalar::<_, i64>(GET_TELEGRAM_UPDATE_OFFSET_QUERY)
        .bind(tenant_id)
        .bind(telegram_bot_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
        .map(|offset| offset.unwrap_or(0))
}

pub async fn claim_telegram_update_offset(
    pool: &PgPool,
    tenant_id: Uuid,
    telegram_bot_id: Uuid,
    locked_at: DateTime<Utc>,
    stale_before: DateTime<Utc>,
) -> AppResult<Option<TelegramUpdateOffsetClaim>> {
    sqlx::query_as::<_, TelegramUpdateOffsetClaim>(CLAIM_TELEGRAM_UPDATE_OFFSET_QUERY)
        .bind(tenant_id)
        .bind(telegram_bot_id)
        .bind(locked_at)
        .bind(stale_before)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn release_telegram_update_offset_lock(
    pool: &PgPool,
    tenant_id: Uuid,
    telegram_bot_id: Uuid,
    locked_at: DateTime<Utc>,
) -> AppResult<()> {
    sqlx::query(RELEASE_TELEGRAM_UPDATE_OFFSET_LOCK_QUERY)
        .bind(tenant_id)
        .bind(telegram_bot_id)
        .bind(locked_at)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    Ok(())
}

pub async fn upsert_telegram_update_offset(
    pool: &PgPool,
    tenant_id: Uuid,
    telegram_bot_id: Uuid,
    update_id: i64,
) -> AppResult<()> {
    sqlx::query(UPSERT_TELEGRAM_UPDATE_OFFSET_QUERY)
        .bind(tenant_id)
        .bind(telegram_bot_id)
        .bind(update_id)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    Ok(())
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
        for status in [
            BINDING_STATUS_PENDING,
            BINDING_STATUS_BOUND,
            BINDING_STATUS_CANCELLED,
            BINDING_STATUS_EXPIRED,
        ] {
            validate_binding_status(status).expect("known status");
        }
        assert!(matches!(
            validate_binding_status("failed"),
            Err(AppError::Validation(_))
        ));
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

    #[test]
    fn expired_binding_requests_are_materialized_before_read() {
        assert!(EXPIRE_BINDING_REQUEST_IF_DUE_QUERY.contains("status = 'expired'"));
        assert!(EXPIRE_BINDING_REQUEST_IF_DUE_QUERY.contains("status = 'pending'"));
        assert!(EXPIRE_BINDING_REQUEST_IF_DUE_QUERY.contains("expires_at <= $3"));
        assert!(GET_BINDING_REQUEST_QUERY.contains("status, bind_token"));

        let source = include_str!("telegram_bindings.rs");
        let production_source = source
            .split("#[cfg(test)]")
            .next()
            .expect("production source is before tests");
        let get_function = production_source
            .split("pub async fn get_binding_request")
            .nth(1)
            .expect("get function exists");

        assert!(get_function.contains("expire_binding_request_if_due"));
    }

    #[test]
    fn binding_confirmation_error_can_be_persisted() {
        assert!(UPDATE_BINDING_CONFIRMATION_ERROR_QUERY.contains("confirmation_error = $3"));
        assert!(UPDATE_BINDING_CONFIRMATION_ERROR_QUERY.contains("status = 'bound'"));
        assert!(UPDATE_BINDING_CONFIRMATION_ERROR_QUERY.contains("WHERE id = $1"));

        let source = include_str!("telegram_bindings.rs");
        let production_source = source
            .split("#[cfg(test)]")
            .next()
            .expect("production source is before tests");
        assert!(production_source.contains("pub async fn update_binding_confirmation_error"));
    }

    #[test]
    fn offset_queries_are_scoped_to_bot_and_tenant() {
        assert!(GET_TELEGRAM_UPDATE_OFFSET_QUERY.contains("tenant_id = $1"));
        assert!(GET_TELEGRAM_UPDATE_OFFSET_QUERY.contains("telegram_bot_id = $2"));
        assert!(UPSERT_TELEGRAM_UPDATE_OFFSET_QUERY
            .contains("ON CONFLICT (tenant_id, telegram_bot_id)"));
    }

    #[test]
    fn update_offset_claim_uses_locked_at_lease() {
        assert!(CLAIM_TELEGRAM_UPDATE_OFFSET_QUERY.contains("locked_at"));
        assert!(
            CLAIM_TELEGRAM_UPDATE_OFFSET_QUERY.contains("ON CONFLICT (tenant_id, telegram_bot_id)")
        );
        assert!(CLAIM_TELEGRAM_UPDATE_OFFSET_QUERY
            .contains("telegram_bot_update_offsets.locked_at IS NULL"));
        assert!(CLAIM_TELEGRAM_UPDATE_OFFSET_QUERY
            .contains("telegram_bot_update_offsets.locked_at < $4"));
        assert!(CLAIM_TELEGRAM_UPDATE_OFFSET_QUERY.contains("RETURNING last_update_id, locked_at"));
        assert!(RELEASE_TELEGRAM_UPDATE_OFFSET_LOCK_QUERY.contains("locked_at = $3"));
    }
}
