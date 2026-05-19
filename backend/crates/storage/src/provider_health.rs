use chrono::{DateTime, Duration, Utc};
use coin_listener_core::{
    models::{Provider, ProviderHealthStatus},
    AppError, AppResult,
};
use redis::aio::MultiplexedConnection;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

pub const PROVIDER_CIRCUIT_FAILURE_THRESHOLD: i32 = 3;
pub const PROVIDER_CIRCUIT_COOLDOWN_SECONDS: i64 = 300;
pub const PROVIDER_LAST_ERROR_MAX_CHARS: usize = 500;

pub const ACTIVE_RPC_PROVIDER_CANDIDATES_QUERY: &str = r#"
SELECT
    p.id,
    p.chain_id,
    p.provider_type,
    p.name,
    p.base_url,
    p.api_key_ref,
    p.priority,
    p.qps_limit,
    p.timeout_ms,
    p.status,
    ph.consecutive_failures,
    ph.last_success_at,
    ph.last_failure_at,
    ph.disabled_until,
    ph.last_error
FROM providers p
LEFT JOIN provider_health ph ON ph.provider_id = p.id
WHERE p.chain_id = $1
  AND p.provider_type = 'rpc'
  AND p.status = 'active'
  AND (ph.disabled_until IS NULL OR ph.disabled_until <= $2)
ORDER BY p.priority ASC, p.name ASC
"#;

pub const RECORD_PROVIDER_SUCCESS_QUERY: &str = r#"
INSERT INTO provider_health (provider_id, consecutive_failures, last_success_at, disabled_until, last_error)
VALUES ($1, 0, $2, NULL, NULL)
ON CONFLICT (provider_id) DO UPDATE
SET consecutive_failures = 0,
    last_success_at = EXCLUDED.last_success_at,
    disabled_until = NULL,
    last_error = NULL,
    updated_at = NOW()
"#;

pub const RECORD_PROVIDER_FAILURE_QUERY: &str = r#"
INSERT INTO provider_health (
    provider_id, consecutive_failures, last_failure_at, disabled_until, last_error
)
VALUES ($1, 1, $2, NULL, $3)
ON CONFLICT (provider_id) DO UPDATE
SET consecutive_failures = provider_health.consecutive_failures + 1,
    last_failure_at = EXCLUDED.last_failure_at,
    disabled_until = CASE
        WHEN provider_health.consecutive_failures + 1 >= $4 THEN $5
        ELSE provider_health.disabled_until
    END,
    last_error = EXCLUDED.last_error,
    updated_at = NOW()
"#;

#[derive(Debug, Clone, FromRow)]
pub struct ProviderCandidateRow {
    pub id: Uuid,
    pub chain_id: Uuid,
    pub provider_type: String,
    pub name: String,
    pub base_url: String,
    pub api_key_ref: Option<String>,
    pub priority: i32,
    pub qps_limit: i32,
    pub timeout_ms: i32,
    pub status: String,
    pub consecutive_failures: Option<i32>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub disabled_until: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderHealthRow {
    pub provider_id: Uuid,
    pub consecutive_failures: i32,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub disabled_until: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProviderCandidate {
    pub provider: Provider,
    pub health: Option<ProviderHealthRow>,
}

impl PartialEq for ProviderCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.provider.id == other.provider.id
            && self.provider.chain_id == other.provider.chain_id
            && self.provider.provider_type == other.provider.provider_type
            && self.provider.name == other.provider.name
            && self.provider.base_url == other.provider.base_url
            && self.provider.api_key_ref == other.provider.api_key_ref
            && self.provider.priority == other.provider.priority
            && self.provider.qps_limit == other.provider.qps_limit
            && self.provider.timeout_ms == other.provider.timeout_ms
            && self.provider.status == other.provider.status
            && self.health == other.health
    }
}

impl Eq for ProviderCandidate {}

impl From<ProviderCandidateRow> for ProviderCandidate {
    fn from(row: ProviderCandidateRow) -> Self {
        let health = row.consecutive_failures.map(|consecutive_failures| ProviderHealthRow {
            provider_id: row.id,
            consecutive_failures,
            last_success_at: row.last_success_at,
            last_failure_at: row.last_failure_at,
            disabled_until: row.disabled_until,
            last_error: row.last_error.clone(),
        });

        Self {
            provider: Provider {
                id: row.id,
                chain_id: row.chain_id,
                provider_type: row.provider_type,
                name: row.name,
                base_url: row.base_url,
                api_key_ref: row.api_key_ref,
                priority: row.priority,
                qps_limit: row.qps_limit,
                timeout_ms: row.timeout_ms,
                status: row.status,
            },
            health,
        }
    }
}

pub fn provider_qps_key(provider_id: Uuid, epoch_second: i64) -> String {
    format!("provider:qps:{provider_id}:{epoch_second}")
}

pub fn provider_disabled_until(failure_count: i32, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    (failure_count >= PROVIDER_CIRCUIT_FAILURE_THRESHOLD)
        .then(|| now + Duration::seconds(PROVIDER_CIRCUIT_COOLDOWN_SECONDS))
}

pub fn is_provider_circuit_open(disabled_until: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    disabled_until.is_some_and(|value| value > now)
}

pub fn provider_candidate_health(
    candidate: &ProviderCandidate,
    now: DateTime<Utc>,
) -> ProviderHealthStatus {
    let Some(health) = &candidate.health else {
        return ProviderHealthStatus {
            consecutive_failures: 0,
            last_success_at: None,
            last_failure_at: None,
            disabled_until: None,
            last_error: None,
            is_circuit_open: false,
        };
    };

    ProviderHealthStatus {
        consecutive_failures: health.consecutive_failures,
        last_success_at: health.last_success_at,
        last_failure_at: health.last_failure_at,
        disabled_until: health.disabled_until,
        last_error: health.last_error.clone(),
        is_circuit_open: is_provider_circuit_open(health.disabled_until, now),
    }
}

pub fn provider_qps_permits(current_count: i64, qps_limit: i32) -> bool {
    qps_limit > 0 && current_count <= i64::from(qps_limit)
}

pub async fn active_rpc_provider_candidates(
    pool: &PgPool,
    chain_id: Uuid,
    now: DateTime<Utc>,
) -> AppResult<Vec<ProviderCandidate>> {
    sqlx::query_as::<_, ProviderCandidateRow>(ACTIVE_RPC_PROVIDER_CANDIDATES_QUERY)
        .bind(chain_id)
        .bind(now)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
        .map(|rows| rows.into_iter().map(ProviderCandidate::from).collect())
}

pub async fn record_provider_success(
    pool: &PgPool,
    provider_id: Uuid,
    now: DateTime<Utc>,
) -> AppResult<()> {
    sqlx::query(RECORD_PROVIDER_SUCCESS_QUERY)
        .bind(provider_id)
        .bind(now)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    Ok(())
}

pub async fn record_provider_failure(
    pool: &PgPool,
    provider_id: Uuid,
    now: DateTime<Utc>,
    error: &AppError,
) -> AppResult<()> {
    let sanitized = sanitize_provider_error(&error.to_string());
    let disabled_until = now + Duration::seconds(PROVIDER_CIRCUIT_COOLDOWN_SECONDS);

    sqlx::query(RECORD_PROVIDER_FAILURE_QUERY)
        .bind(provider_id)
        .bind(now)
        .bind(sanitized)
        .bind(PROVIDER_CIRCUIT_FAILURE_THRESHOLD)
        .bind(disabled_until)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    Ok(())
}

pub async fn try_acquire_provider_qps(
    redis: &mut MultiplexedConnection,
    provider_id: Uuid,
    qps_limit: i32,
    now: DateTime<Utc>,
) -> AppResult<bool> {
    let key = provider_qps_key(provider_id, now.timestamp());
    let current_count: i64 = redis::cmd("INCR")
        .arg(&key)
        .query_async(redis)
        .await
        .map_err(|error| AppError::Redis(error.to_string()))?;

    let expire_set: bool = redis::cmd("EXPIRE")
        .arg(&key)
        .arg(2)
        .query_async(redis)
        .await
        .map_err(|error| AppError::Redis(error.to_string()))?;

    if !expire_set {
        return Err(AppError::Redis("failed to expire provider QPS key".to_string()));
    }

    Ok(provider_qps_permits(current_count, qps_limit))
}

pub fn sanitize_provider_error(error: &str) -> String {
    let mut sanitized = error
        .replace("token=", "token=<redacted>")
        .replace("api_key=", "api_key=<redacted>")
        .replace("key=", "key=<redacted>");

    sanitized = redact_query_value(&sanitized, "token=<redacted>");
    sanitized = redact_query_value(&sanitized, "api_key=<redacted>");
    sanitized = redact_query_value(&sanitized, "key=<redacted>");

    sanitized.chars().take(PROVIDER_LAST_ERROR_MAX_CHARS).collect()
}

fn redact_query_value(input: &str, marker: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut remaining = input;

    while let Some(index) = remaining.find(marker) {
        output.push_str(&remaining[..index + marker.len()]);
        let after_marker = &remaining[index + marker.len()..];
        let next_separator = after_marker
            .find('&')
            .or_else(|| after_marker.find(' '))
            .unwrap_or(after_marker.len());
        remaining = &after_marker[next_separator..];
    }

    output.push_str(remaining);
    output
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use coin_listener_core::models::Provider;
    use uuid::Uuid;

    use crate::provider_health::{
        is_provider_circuit_open, provider_candidate_health, provider_disabled_until,
        provider_qps_key, provider_qps_permits, sanitize_provider_error, ProviderCandidate,
        ProviderHealthRow, ACTIVE_RPC_PROVIDER_CANDIDATES_QUERY,
        PROVIDER_CIRCUIT_COOLDOWN_SECONDS, PROVIDER_CIRCUIT_FAILURE_THRESHOLD,
        PROVIDER_LAST_ERROR_MAX_CHARS, RECORD_PROVIDER_FAILURE_QUERY,
        RECORD_PROVIDER_SUCCESS_QUERY,
    };

    #[test]
    fn provider_health_migration_defines_table_and_indexes() {
        let migration = include_str!("../migrations/0011_provider_health.sql");

        assert!(migration.contains("CREATE TABLE IF NOT EXISTS provider_health"));
        assert!(migration.contains("provider_id UUID PRIMARY KEY REFERENCES providers(id) ON DELETE CASCADE"));
        assert!(migration.contains("disabled_until TIMESTAMPTZ"));
        assert!(migration.contains("idx_provider_health_disabled_until"));
        assert!(migration.contains("idx_provider_health_last_failure"));
    }

    #[test]
    fn provider_qps_key_uses_provider_id_and_epoch_second() {
        let provider_id = uuid::Uuid::from_u128(42);

        assert_eq!(
            provider_qps_key(provider_id, 1_779_123_600),
            "provider:qps:00000000-0000-0000-0000-00000000002a:1779123600"
        );
    }

    #[test]
    fn provider_disabled_until_uses_five_minute_cooldown() {
        let now = Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 0).unwrap();

        assert_eq!(PROVIDER_CIRCUIT_FAILURE_THRESHOLD, 3);
        assert_eq!(PROVIDER_CIRCUIT_COOLDOWN_SECONDS, 300);
        assert_eq!(
            provider_disabled_until(PROVIDER_CIRCUIT_FAILURE_THRESHOLD, now),
            Some(Utc.with_ymd_and_hms(2026, 5, 19, 10, 5, 0).unwrap())
        );
        assert_eq!(provider_disabled_until(PROVIDER_CIRCUIT_FAILURE_THRESHOLD - 1, now), None);
    }

    #[test]
    fn circuit_is_open_only_while_disabled_until_is_future() {
        let now = Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 0).unwrap();
        let past = Utc.with_ymd_and_hms(2026, 5, 19, 9, 59, 59).unwrap();
        let future = Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 1).unwrap();

        assert!(!is_provider_circuit_open(None, now));
        assert!(!is_provider_circuit_open(Some(past), now));
        assert!(is_provider_circuit_open(Some(future), now));
    }

    #[test]
    fn provider_error_sanitizer_redacts_query_secrets_and_truncates() {
        let secret = format!(
            "provider request failed: https://example.invalid/rpc?token=abc&api_key=def&key=ghi&safe=ok {}",
            "x".repeat(PROVIDER_LAST_ERROR_MAX_CHARS + 20)
        );

        let sanitized = sanitize_provider_error(&secret);

        assert!(!sanitized.contains("abc"));
        assert!(!sanitized.contains("def"));
        assert!(!sanitized.contains("ghi"));
        assert!(sanitized.contains("token=<redacted>"));
        assert!(sanitized.contains("api_key=<redacted>"));
        assert!(sanitized.contains("key=<redacted>"));
        assert!(sanitized.len() <= PROVIDER_LAST_ERROR_MAX_CHARS);
    }

    #[test]
    fn active_candidate_query_excludes_open_circuits_and_orders_by_priority() {
        assert!(ACTIVE_RPC_PROVIDER_CANDIDATES_QUERY.contains("LEFT JOIN provider_health ph"));
        assert!(ACTIVE_RPC_PROVIDER_CANDIDATES_QUERY.contains("provider_type = 'rpc'"));
        assert!(ACTIVE_RPC_PROVIDER_CANDIDATES_QUERY.contains("p.status = 'active'"));
        assert!(ACTIVE_RPC_PROVIDER_CANDIDATES_QUERY
            .contains("(ph.disabled_until IS NULL OR ph.disabled_until <= $2)"));
        assert!(ACTIVE_RPC_PROVIDER_CANDIDATES_QUERY
            .contains("ORDER BY p.priority ASC, p.name ASC"));
        assert!(!ACTIVE_RPC_PROVIDER_CANDIDATES_QUERY.contains("LIMIT 1"));
    }

    #[test]
    fn success_query_resets_failures_and_clears_error_state() {
        assert!(RECORD_PROVIDER_SUCCESS_QUERY.contains("consecutive_failures = 0"));
        assert!(RECORD_PROVIDER_SUCCESS_QUERY.contains("last_success_at = EXCLUDED.last_success_at"));
        assert!(RECORD_PROVIDER_SUCCESS_QUERY.contains("disabled_until = NULL"));
        assert!(RECORD_PROVIDER_SUCCESS_QUERY.contains("last_error = NULL"));
    }

    #[test]
    fn failure_query_increments_failures_and_sets_disabled_until() {
        assert!(RECORD_PROVIDER_FAILURE_QUERY.contains("consecutive_failures + 1"));
        assert!(RECORD_PROVIDER_FAILURE_QUERY.contains("last_failure_at"));
        assert!(RECORD_PROVIDER_FAILURE_QUERY.contains("disabled_until"));
        assert!(RECORD_PROVIDER_FAILURE_QUERY.contains("last_error"));
    }

    #[test]
    fn provider_candidate_health_defaults_missing_health_to_closed_circuit() {
        let candidate = ProviderCandidate {
            provider: provider(1, 10),
            health: None,
        };
        let now = Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 0).unwrap();

        let health = provider_candidate_health(&candidate, now);

        assert_eq!(health.consecutive_failures, 0);
        assert_eq!(health.disabled_until, None);
        assert!(!health.is_circuit_open);
    }

    #[test]
    fn provider_candidate_health_marks_future_disabled_until_open() {
        let disabled_until = Utc.with_ymd_and_hms(2026, 5, 19, 10, 5, 0).unwrap();
        let candidate = ProviderCandidate {
            provider: provider(1, 10),
            health: Some(ProviderHealthRow {
                provider_id: Uuid::from_u128(1),
                consecutive_failures: 3,
                last_success_at: None,
                last_failure_at: Some(Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 0).unwrap()),
                disabled_until: Some(disabled_until),
                last_error: Some("provider request failed: timeout".to_string()),
            }),
        };
        let now = Utc.with_ymd_and_hms(2026, 5, 19, 10, 1, 0).unwrap();

        let health = provider_candidate_health(&candidate, now);

        assert_eq!(health.consecutive_failures, 3);
        assert_eq!(health.disabled_until, Some(disabled_until));
        assert!(health.is_circuit_open);
    }

    #[test]
    fn provider_qps_permits_counts_within_limit_only() {
        assert!(provider_qps_permits(1, 1));
        assert!(provider_qps_permits(10, 10));
        assert!(!provider_qps_permits(11, 10));
        assert!(!provider_qps_permits(1, 0));
    }

    fn provider(id: u128, priority: i32) -> Provider {
        Provider {
            id: Uuid::from_u128(id),
            chain_id: Uuid::from_u128(100),
            provider_type: "rpc".to_string(),
            name: format!("provider-{id}"),
            base_url: "https://example.invalid".to_string(),
            api_key_ref: None,
            priority,
            qps_limit: 10,
            timeout_ms: 5000,
            status: "active".to_string(),
        }
    }
}
