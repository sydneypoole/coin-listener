use chrono::{DateTime, Utc};
use coin_listener_core::{
    models::{
        ScanAddressContext, ScanAddressTask, ScanRun, ScanRunDetail, ScanRunListItem, ScanRunQuery,
    },
    AppError, AppResult,
};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

pub const SCAN_RUN_STATUS_RUNNING: &str = "running";
pub const SCAN_RUN_STATUS_SUCCESS: &str = "success";
pub const SCAN_RUN_STATUS_FAILED: &str = "failed";
pub const SCAN_RUN_STATUS_LOCKED: &str = "locked";
pub const SCAN_RUN_STATUS_UNSUPPORTED: &str = "unsupported";

pub const INSERT_SCAN_RUN_QUERY: &str = r#"
INSERT INTO scan_runs (
    tenant_id, task_id, address_id, chain_id, chain_type, status, started_at, metadata
)
VALUES ($1, $2, $3, $4, $5, 'running', $6, $7)
RETURNING id, tenant_id, task_id, address_id, chain_id, chain_type, status,
          event_count, started_at, finished_at, duration_ms, error_message,
          metadata, created_at, updated_at
"#;

pub const FINISH_SCAN_RUN_QUERY: &str = r#"
UPDATE scan_runs
SET status = $2,
    event_count = $3,
    finished_at = $4,
    duration_ms = GREATEST(0::double precision, EXTRACT(EPOCH FROM ($4::timestamptz - started_at)) * 1000)::BIGINT,
    error_message = $5,
    metadata = scan_runs.metadata || $6::jsonb,
    updated_at = NOW()
WHERE id = $1
RETURNING id, tenant_id, task_id, address_id, chain_id, chain_type, status,
          event_count, started_at, finished_at, duration_ms, error_message,
          metadata, created_at, updated_at
"#;

pub const LIST_SCAN_RUNS_QUERY: &str = r#"
SELECT sr.id,
       sr.tenant_id,
       sr.task_id,
       sr.address_id,
       sr.chain_id,
       c.name AS chain_name,
       wa.address,
       wa.label AS address_label,
       sr.chain_type,
       sr.status,
       sr.event_count,
       sr.started_at,
       sr.finished_at,
       sr.duration_ms,
       sr.error_message
FROM scan_runs sr
JOIN chains c ON c.id = sr.chain_id
JOIN watched_addresses wa ON wa.id = sr.address_id
    AND wa.tenant_id = sr.tenant_id
    AND wa.chain_id = sr.chain_id
WHERE sr.tenant_id = $1
  AND ($2::uuid IS NULL OR sr.chain_id = $2)
  AND ($3::uuid IS NULL OR sr.address_id = $3)
  AND ($4::text IS NULL OR sr.status = $4)
  AND ($5::timestamptz IS NULL OR sr.started_at >= $5)
  AND ($6::timestamptz IS NULL OR sr.started_at <= $6)
ORDER BY sr.started_at DESC
LIMIT $7 OFFSET $8
"#;

pub const GET_SCAN_RUN_DETAIL_QUERY: &str = r#"
SELECT sr.id,
       sr.tenant_id,
       sr.task_id,
       sr.address_id,
       sr.chain_id,
       c.name AS chain_name,
       wa.address,
       wa.label AS address_label,
       sr.chain_type,
       sr.status,
       sr.event_count,
       sr.started_at,
       sr.finished_at,
       sr.duration_ms,
       sr.error_message,
       sr.metadata,
       sr.created_at,
       sr.updated_at
FROM scan_runs sr
JOIN chains c ON c.id = sr.chain_id
JOIN watched_addresses wa ON wa.id = sr.address_id
    AND wa.tenant_id = sr.tenant_id
    AND wa.chain_id = sr.chain_id
WHERE sr.tenant_id = $1
  AND sr.id = $2
"#;

pub const SELECT_RETRY_SCAN_RUN_QUERY: &str = r#"
SELECT sr.tenant_id,
       sr.address_id,
       sr.chain_id,
       sr.status
FROM scan_runs sr
JOIN watched_addresses wa ON wa.id = sr.address_id
    AND wa.tenant_id = sr.tenant_id
    AND wa.chain_id = sr.chain_id
    AND wa.status = 'active'
WHERE sr.id = $1
  AND sr.tenant_id = $2
"#;

pub const CLAIM_RETRY_SCAN_RUN_QUERY: &str = r#"
UPDATE scan_runs
SET metadata = (metadata - 'retry_enqueue_error') || jsonb_build_object(
        'retry_task_id', $3::text,
        'retry_claimed_at', $4::text
    ),
    updated_at = NOW()
WHERE id = $1
  AND tenant_id = $2
  AND (
      NOT (metadata ? 'retry_task_id')
      OR (
          metadata ? 'retry_claimed_at'
          AND NOT (metadata ? 'retry_enqueued_at')
          AND (metadata->>'retry_claimed_at')::timestamptz <= $5
      )
  )
RETURNING id
"#;

pub const MARK_RETRY_SCAN_RUN_ENQUEUED_QUERY: &str = r#"
UPDATE scan_runs
SET metadata = metadata || jsonb_build_object('retry_enqueued_at', $3::text),
    updated_at = NOW()
WHERE id = $1
  AND tenant_id = $2
  AND metadata->>'retry_task_id' = $4
"#;

pub const CLEAR_RETRY_SCAN_RUN_CLAIM_QUERY: &str = r#"
UPDATE scan_runs
SET metadata = (metadata - 'retry_task_id' - 'retry_claimed_at' - 'retry_enqueued_at')
        || jsonb_build_object('retry_enqueue_error', $4::text),
    updated_at = NOW()
WHERE id = $1
  AND tenant_id = $2
  AND metadata->>'retry_task_id' = $3
"#;

pub const SCAN_RUN_CONTEXT_QUERY: &str = r#"
SELECT wa.id,
       wa.tenant_id,
       wa.chain_id,
       wa.address,
       wa.scan_interval_seconds,
       c.chain_type
FROM watched_addresses wa
JOIN chains c ON c.id = wa.chain_id
WHERE wa.id = $1
  AND wa.tenant_id = $2
  AND wa.chain_id = $3
  AND wa.status = 'active'
"#;

pub const SCAN_RUN_HEALTH_SUMMARY_QUERY: &str = r#"
SELECT MAX(finished_at) FILTER (WHERE status = 'success') AS last_success_at,
       MAX(finished_at) FILTER (WHERE status = 'failed') AS last_failed_at,
       COUNT(*) FILTER (
           WHERE status = 'success'
             AND finished_at >= NOW() - INTERVAL '24 hours'
       ) AS last_24h_success,
       COUNT(*) FILTER (
           WHERE status = 'failed'
             AND finished_at >= NOW() - INTERVAL '24 hours'
       ) AS last_24h_failed
FROM scan_runs
WHERE tenant_id = $1
"#;

pub const RECENT_SCAN_RUNS_QUERY: &str = r#"
SELECT sr.id,
       sr.tenant_id,
       sr.task_id,
       sr.address_id,
       sr.chain_id,
       c.name AS chain_name,
       wa.address,
       wa.label AS address_label,
       sr.chain_type,
       sr.status,
       sr.event_count,
       sr.started_at,
       sr.finished_at,
       sr.duration_ms,
       sr.error_message
FROM scan_runs sr
JOIN chains c ON c.id = sr.chain_id
JOIN watched_addresses wa ON wa.id = sr.address_id
    AND wa.tenant_id = sr.tenant_id
    AND wa.chain_id = sr.chain_id
WHERE sr.tenant_id = $1
ORDER BY sr.started_at DESC
LIMIT $2
"#;

#[derive(Debug, FromRow)]
pub struct ScanRunHealthSummary {
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failed_at: Option<DateTime<Utc>>,
    pub last_24h_success: i64,
    pub last_24h_failed: i64,
}

#[derive(Debug, FromRow)]
struct RetryScanRunRow {
    tenant_id: Uuid,
    address_id: Uuid,
    chain_id: Uuid,
    status: String,
}

#[derive(Debug, FromRow)]
struct ClaimedRetryScanRunRow {
    id: Uuid,
}

pub fn validate_scan_run_status(status: &str) -> AppResult<()> {
    if !matches!(
        status,
        SCAN_RUN_STATUS_RUNNING
            | SCAN_RUN_STATUS_SUCCESS
            | SCAN_RUN_STATUS_FAILED
            | SCAN_RUN_STATUS_LOCKED
            | SCAN_RUN_STATUS_UNSUPPORTED
    ) {
        return Err(AppError::Validation(
            "scan run status must be running, success, failed, locked, or unsupported".to_string(),
        ));
    }
    Ok(())
}

pub fn scan_run_status_allows_retry(status: &str) -> bool {
    matches!(status, SCAN_RUN_STATUS_FAILED | SCAN_RUN_STATUS_UNSUPPORTED)
}

pub fn scan_runs_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(50).clamp(1, 100)
}

pub fn scan_runs_offset(offset: Option<i64>) -> i64 {
    offset.unwrap_or(0).max(0)
}

pub fn build_retry_scan_task(
    tenant_id: Uuid,
    address_id: Uuid,
    chain_id: Uuid,
    now: DateTime<Utc>,
) -> ScanAddressTask {
    ScanAddressTask {
        task_id: Uuid::new_v4(),
        address_id,
        tenant_id,
        chain_id,
        attempt: 1,
        enqueued_at: now,
    }
}

pub async fn scan_run_context(
    pool: &PgPool,
    task: &ScanAddressTask,
) -> AppResult<ScanAddressContext> {
    sqlx::query_as::<_, ScanAddressContext>(SCAN_RUN_CONTEXT_QUERY)
        .bind(task.address_id)
        .bind(task.tenant_id)
        .bind(task.chain_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("watched address".to_string()))
}

pub async fn create_scan_run(
    pool: &PgPool,
    task: &ScanAddressTask,
    context: &ScanAddressContext,
    started_at: DateTime<Utc>,
    metadata: serde_json::Value,
) -> AppResult<ScanRun> {
    sqlx::query_as::<_, ScanRun>(INSERT_SCAN_RUN_QUERY)
        .bind(context.tenant_id)
        .bind(task.task_id)
        .bind(context.id)
        .bind(context.chain_id)
        .bind(&context.chain_type)
        .bind(started_at)
        .bind(metadata)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn finish_scan_run(
    pool: &PgPool,
    scan_run_id: Uuid,
    status: &str,
    event_count: i32,
    finished_at: DateTime<Utc>,
    error_message: Option<&str>,
    metadata: serde_json::Value,
) -> AppResult<ScanRun> {
    validate_scan_run_status(status)?;
    sqlx::query_as::<_, ScanRun>(FINISH_SCAN_RUN_QUERY)
        .bind(scan_run_id)
        .bind(status)
        .bind(event_count)
        .bind(finished_at)
        .bind(error_message)
        .bind(metadata)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("scan run".to_string()))
}

pub async fn list_scan_runs(
    pool: &PgPool,
    tenant_id: Uuid,
    query: ScanRunQuery,
) -> AppResult<Vec<ScanRunListItem>> {
    if let Some(status) = query.status.as_deref() {
        validate_scan_run_status(status)?;
    }

    sqlx::query_as::<_, ScanRunListItem>(LIST_SCAN_RUNS_QUERY)
        .bind(tenant_id)
        .bind(query.chain_id)
        .bind(query.address_id)
        .bind(query.status)
        .bind(query.started_after)
        .bind(query.started_before)
        .bind(scan_runs_limit(query.limit))
        .bind(scan_runs_offset(query.offset))
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn get_scan_run_detail(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
) -> AppResult<ScanRunDetail> {
    sqlx::query_as::<_, ScanRunDetail>(GET_SCAN_RUN_DETAIL_QUERY)
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("scan run".to_string()))
}

pub async fn retry_scan_run_task(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
    now: DateTime<Utc>,
    stale_claim_before: DateTime<Utc>,
) -> AppResult<ScanAddressTask> {
    let row = sqlx::query_as::<_, RetryScanRunRow>(SELECT_RETRY_SCAN_RUN_QUERY)
        .bind(id)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("scan run".to_string()))?;

    if !scan_run_status_allows_retry(&row.status) {
        return Err(AppError::Validation(
            "only failed or unsupported scan runs can be retried".to_string(),
        ));
    }

    let task = build_retry_scan_task(row.tenant_id, row.address_id, row.chain_id, now);
    let claimed = sqlx::query_as::<_, ClaimedRetryScanRunRow>(CLAIM_RETRY_SCAN_RUN_QUERY)
        .bind(id)
        .bind(tenant_id)
        .bind(task.task_id.to_string())
        .bind(now.to_rfc3339())
        .bind(stale_claim_before)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    if let Some(claimed) = claimed {
        let _claimed_id = claimed.id;
    } else {
        return Err(AppError::Validation(
            "scan run retry has already been requested".to_string(),
        ));
    }

    Ok(task)
}

pub async fn mark_retry_scan_run_enqueued(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
    task_id: Uuid,
    enqueued_at: DateTime<Utc>,
) -> AppResult<()> {
    sqlx::query(MARK_RETRY_SCAN_RUN_ENQUEUED_QUERY)
        .bind(id)
        .bind(tenant_id)
        .bind(enqueued_at.to_rfc3339())
        .bind(task_id.to_string())
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    Ok(())
}

pub async fn clear_retry_scan_run_claim(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
    task_id: Uuid,
    error_message: &str,
) -> AppResult<()> {
    sqlx::query(CLEAR_RETRY_SCAN_RUN_CLAIM_QUERY)
        .bind(id)
        .bind(tenant_id)
        .bind(task_id.to_string())
        .bind(error_message)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    Ok(())
}

pub async fn scan_run_health_summary(
    pool: &PgPool,
    tenant_id: Uuid,
) -> AppResult<ScanRunHealthSummary> {
    sqlx::query_as::<_, ScanRunHealthSummary>(SCAN_RUN_HEALTH_SUMMARY_QUERY)
        .bind(tenant_id)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn recent_scan_runs(
    pool: &PgPool,
    tenant_id: Uuid,
    limit: i64,
) -> AppResult<Vec<ScanRunListItem>> {
    sqlx::query_as::<_, ScanRunListItem>(RECENT_SCAN_RUNS_QUERY)
        .bind(tenant_id)
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn scan_runs_migration_defines_table_statuses_and_indexes() {
        let migration = include_str!("../migrations/0019_scan_runs.sql");

        assert!(migration.contains("CREATE TABLE IF NOT EXISTS scan_runs"));
        for field in [
            "id UUID PRIMARY KEY",
            "tenant_id UUID NOT NULL",
            "task_id UUID NOT NULL",
            "address_id UUID NOT NULL",
            "chain_id UUID NOT NULL",
            "chain_type TEXT NOT NULL",
            "status TEXT NOT NULL",
            "event_count INTEGER NOT NULL DEFAULT 0",
            "started_at TIMESTAMPTZ NOT NULL",
            "finished_at TIMESTAMPTZ",
            "duration_ms BIGINT",
            "error_message TEXT",
            "metadata JSONB NOT NULL DEFAULT '{}'::jsonb",
            "created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()",
            "updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()",
        ] {
            assert!(migration.contains(field), "missing migration field {field}");
        }

        for status in ["running", "success", "failed", "locked", "unsupported"] {
            assert!(migration.contains(status), "missing status {status}");
        }

        assert!(migration.contains("idx_scan_runs_tenant_started_at"));
        assert!(migration.contains("ON scan_runs(tenant_id, started_at DESC)"));
        assert!(migration.contains("idx_scan_runs_tenant_status_started_at"));
        assert!(migration.contains("ON scan_runs(tenant_id, status, started_at DESC)"));
        assert!(migration.contains("idx_scan_runs_address_started_at"));
        assert!(migration.contains("ON scan_runs(address_id, started_at DESC)"));
        assert!(migration.contains("idx_scan_runs_task_id"));
        assert!(migration.contains("ON scan_runs(task_id)"));
    }

    #[test]
    fn scan_run_status_validation_and_retryability_are_explicit() {
        for status in ["running", "success", "failed", "locked", "unsupported"] {
            assert!(validate_scan_run_status(status).is_ok(), "status {status}");
        }
        assert!(validate_scan_run_status("unknown").is_err());
        assert!(scan_run_status_allows_retry("failed"));
        assert!(scan_run_status_allows_retry("unsupported"));
        assert!(!scan_run_status_allows_retry("running"));
        assert!(!scan_run_status_allows_retry("success"));
        assert!(!scan_run_status_allows_retry("locked"));
    }

    #[test]
    fn scan_run_pagination_defaults_and_clamps() {
        assert_eq!(scan_runs_limit(None), 50);
        assert_eq!(scan_runs_limit(Some(0)), 1);
        assert_eq!(scan_runs_limit(Some(100)), 100);
        assert_eq!(scan_runs_limit(Some(500)), 100);
        assert_eq!(scan_runs_offset(None), 0);
        assert_eq!(scan_runs_offset(Some(-10)), 0);
        assert_eq!(scan_runs_offset(Some(25)), 25);
    }

    #[test]
    fn scan_run_queries_are_tenant_scoped_and_filterable() {
        assert!(LIST_SCAN_RUNS_QUERY.contains("sr.tenant_id = $1"));
        assert!(LIST_SCAN_RUNS_QUERY.contains("$2::uuid IS NULL OR sr.chain_id = $2"));
        assert!(LIST_SCAN_RUNS_QUERY.contains("$3::uuid IS NULL OR sr.address_id = $3"));
        assert!(LIST_SCAN_RUNS_QUERY.contains("$4::text IS NULL OR sr.status = $4"));
        assert!(LIST_SCAN_RUNS_QUERY.contains("$5::timestamptz IS NULL OR sr.started_at >= $5"));
        assert!(LIST_SCAN_RUNS_QUERY.contains("$6::timestamptz IS NULL OR sr.started_at <= $6"));
        assert!(LIST_SCAN_RUNS_QUERY.contains("JOIN chains c"));
        assert!(LIST_SCAN_RUNS_QUERY.contains("JOIN watched_addresses wa"));
        assert!(LIST_SCAN_RUNS_QUERY.contains("wa.tenant_id = sr.tenant_id"));
        assert!(LIST_SCAN_RUNS_QUERY.contains("wa.chain_id = sr.chain_id"));
        assert!(GET_SCAN_RUN_DETAIL_QUERY.contains("sr.tenant_id = $1"));
        assert!(GET_SCAN_RUN_DETAIL_QUERY.contains("sr.id = $2"));
        assert!(GET_SCAN_RUN_DETAIL_QUERY.contains("wa.tenant_id = sr.tenant_id"));
        assert!(GET_SCAN_RUN_DETAIL_QUERY.contains("wa.chain_id = sr.chain_id"));
        assert!(SELECT_RETRY_SCAN_RUN_QUERY.contains("sr.tenant_id = $2"));
        assert!(SELECT_RETRY_SCAN_RUN_QUERY.contains("JOIN watched_addresses wa"));
        assert!(SELECT_RETRY_SCAN_RUN_QUERY.contains("wa.tenant_id = sr.tenant_id"));
        assert!(SELECT_RETRY_SCAN_RUN_QUERY.contains("wa.chain_id = sr.chain_id"));
        assert!(SELECT_RETRY_SCAN_RUN_QUERY.contains("wa.status = 'active'"));
        assert!(CLAIM_RETRY_SCAN_RUN_QUERY.contains("'retry_task_id', $3::text"));
        assert!(CLAIM_RETRY_SCAN_RUN_QUERY.contains("'retry_claimed_at', $4::text"));
        assert!(CLAIM_RETRY_SCAN_RUN_QUERY.contains("metadata - 'retry_enqueue_error'"));
        assert!(CLAIM_RETRY_SCAN_RUN_QUERY.contains("NOT (metadata ? 'retry_task_id')"));
        assert!(CLAIM_RETRY_SCAN_RUN_QUERY.contains("NOT (metadata ? 'retry_enqueued_at')"));
        assert!(CLAIM_RETRY_SCAN_RUN_QUERY
            .contains("(metadata->>'retry_claimed_at')::timestamptz <= $5"));
        assert!(MARK_RETRY_SCAN_RUN_ENQUEUED_QUERY.contains("'retry_enqueued_at', $3::text"));
        assert!(MARK_RETRY_SCAN_RUN_ENQUEUED_QUERY.contains("metadata->>'retry_task_id' = $4"));
        assert!(CLEAR_RETRY_SCAN_RUN_CLAIM_QUERY
            .contains("metadata - 'retry_task_id' - 'retry_claimed_at' - 'retry_enqueued_at'"));
        assert!(CLEAR_RETRY_SCAN_RUN_CLAIM_QUERY.contains("'retry_enqueue_error', $4::text"));
        assert!(CLEAR_RETRY_SCAN_RUN_CLAIM_QUERY.contains("metadata->>'retry_task_id' = $3"));
    }

    #[test]
    fn scan_run_summary_and_context_queries_are_tenant_scoped() {
        assert!(SCAN_RUN_HEALTH_SUMMARY_QUERY.contains("WHERE tenant_id = $1"));
        assert!(RECENT_SCAN_RUNS_QUERY.contains("WHERE sr.tenant_id = $1"));
        assert!(RECENT_SCAN_RUNS_QUERY.contains("wa.tenant_id = sr.tenant_id"));
        assert!(RECENT_SCAN_RUNS_QUERY.contains("wa.chain_id = sr.chain_id"));
        assert!(RECENT_SCAN_RUNS_QUERY.contains("LIMIT $2"));
        assert!(SCAN_RUN_CONTEXT_QUERY.contains("wa.tenant_id = $2"));
        assert!(SCAN_RUN_CONTEXT_QUERY.contains("wa.chain_id = $3"));
        assert!(SCAN_RUN_CONTEXT_QUERY.contains("wa.status = 'active'"));
    }

    #[test]
    fn scan_run_completion_query_sets_duration_and_merges_metadata() {
        assert!(FINISH_SCAN_RUN_QUERY.contains("finished_at = $4"));
        assert!(FINISH_SCAN_RUN_QUERY.contains("duration_ms"));
        assert!(
            FINISH_SCAN_RUN_QUERY.contains("EXTRACT(EPOCH FROM ($4::timestamptz - started_at))")
        );
        assert!(FINISH_SCAN_RUN_QUERY.contains("metadata = scan_runs.metadata || $6::jsonb"));
        assert!(FINISH_SCAN_RUN_QUERY.contains("updated_at = NOW()"));
    }

    #[test]
    fn retry_scan_run_task_uses_new_task_id_and_attempt_one() {
        let now = Utc.with_ymd_and_hms(2026, 5, 24, 8, 0, 0).unwrap();
        let task = build_retry_scan_task(
            Uuid::from_u128(2),
            Uuid::from_u128(4),
            Uuid::from_u128(5),
            now,
        );

        assert_ne!(task.task_id, Uuid::nil());
        assert_eq!(task.tenant_id, Uuid::from_u128(2));
        assert_eq!(task.address_id, Uuid::from_u128(4));
        assert_eq!(task.chain_id, Uuid::from_u128(5));
        assert_eq!(task.attempt, 1);
        assert_eq!(task.enqueued_at, now);
    }
}
