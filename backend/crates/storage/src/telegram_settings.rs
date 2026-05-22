use chrono::{DateTime, Utc};
use coin_listener_core::{
    models::{TelegramSettings, UpdateTelegramSettingsRequest},
    proxy::{mask_proxy_url, normalize_proxy_url},
    AppError, AppResult,
};
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

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct TelegramSettingsRow {
    pub tenant_id: Uuid,
    pub proxy_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TelegramSettingsRow {
    fn to_public(self) -> TelegramSettings {
        let proxy_url_preview = self.proxy_url.as_deref().map(mask_proxy_url);
        let has_proxy = self.proxy_url.is_some();

        TelegramSettings {
            tenant_id: self.tenant_id,
            proxy_url_preview,
            has_proxy,
            created_at: Some(self.created_at),
            updated_at: Some(self.updated_at),
        }
    }
}

pub async fn get_telegram_settings(pool: &PgPool, tenant_id: Uuid) -> AppResult<TelegramSettings> {
    sqlx::query_as::<_, TelegramSettingsRow>(GET_TELEGRAM_SETTINGS_QUERY)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
        .map(|row| {
            row.map(TelegramSettingsRow::to_public)
                .unwrap_or(TelegramSettings {
                    tenant_id,
                    proxy_url_preview: None,
                    has_proxy: false,
                    created_at: None,
                    updated_at: None,
                })
        })
}

pub async fn get_telegram_proxy_url(pool: &PgPool, tenant_id: Uuid) -> AppResult<Option<String>> {
    sqlx::query_as::<_, TelegramSettingsRow>(GET_TELEGRAM_SETTINGS_QUERY)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
        .map(|row| row.and_then(|settings| settings.proxy_url))
}

pub async fn update_telegram_settings(
    pool: &PgPool,
    tenant_id: Uuid,
    request: UpdateTelegramSettingsRequest,
) -> AppResult<TelegramSettings> {
    let proxy_url = match request.proxy_url {
        Some(proxy_url) => normalize_proxy_url(proxy_url.as_deref())?,
        None => get_telegram_proxy_url(pool, tenant_id).await?,
    };

    sqlx::query_as::<_, TelegramSettingsRow>(UPSERT_TELEGRAM_SETTINGS_QUERY)
        .bind(tenant_id)
        .bind(proxy_url)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
        .map(TelegramSettingsRow::to_public)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use uuid::Uuid;

    #[test]
    fn telegram_settings_queries_are_tenant_scoped_and_upsert_proxy() {
        assert!(GET_TELEGRAM_SETTINGS_QUERY
            .contains("SELECT tenant_id, proxy_url, created_at, updated_at"));
        assert!(GET_TELEGRAM_SETTINGS_QUERY.contains("FROM telegram_settings"));
        assert!(GET_TELEGRAM_SETTINGS_QUERY.contains("WHERE tenant_id = $1"));
        assert!(UPSERT_TELEGRAM_SETTINGS_QUERY.contains("ON CONFLICT (tenant_id)"));
        assert!(UPSERT_TELEGRAM_SETTINGS_QUERY.contains("proxy_url = EXCLUDED.proxy_url"));
        assert!(UPSERT_TELEGRAM_SETTINGS_QUERY.contains("updated_at = NOW()"));
    }

    #[test]
    fn telegram_settings_row_masks_proxy_url() {
        let at = chrono::Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let settings = TelegramSettingsRow {
            tenant_id: Uuid::from_u128(1),
            proxy_url: Some("http://alice:secret@proxy.example.com:7890".to_string()),
            created_at: at,
            updated_at: at,
        }
        .to_public();

        assert_eq!(settings.tenant_id, Uuid::from_u128(1));
        assert_eq!(
            settings.proxy_url_preview.as_deref(),
            Some("http://alice:***@proxy.example.com:7890")
        );
        assert!(settings.has_proxy);
        assert_eq!(settings.created_at, Some(at));
        assert_eq!(settings.updated_at, Some(at));
    }

    #[test]
    fn update_settings_source_preserves_omitted_proxy_and_normalizes_explicit_value() {
        let source = include_str!("telegram_settings.rs");
        let production_source = source
            .split("#[cfg(test)]")
            .next()
            .expect("production source is before tests");

        assert!(production_source.contains("normalize_proxy_url(proxy_url.as_deref())"));
        assert!(
            production_source.contains("None => get_telegram_proxy_url(pool, tenant_id).await?")
        );
    }
}
