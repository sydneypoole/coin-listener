use chrono::{DateTime, Duration, Utc};
use coin_listener_core::{
    models::{ScanAddressCandidate, ScanAddressTask, ScanJob, ScanJobStatusCounts},
    AppError, AppResult,
};
use sqlx::{Executor, PgPool, Postgres, Transaction};
use uuid::Uuid;

pub const SCAN_JOB_STATUS_PENDING: &str = "pending";
pub const SCAN_JOB_STATUS_RETRYABLE: &str = "retryable";
pub const SCAN_JOB_STATUS_PROCESSING: &str = "processing";
pub const SCAN_JOB_STATUS_SUCCEEDED: &str = "succeeded";
pub const SCAN_JOB_STATUS_DEAD_LETTER: &str = "dead_letter";

pub const SCAN_JOBS_MIGRATION: &str = include_str!("../migrations/0022_scan_jobs.sql");

pub const INSERT_SCHEDULED_SCAN_JOB_QUERY: &str = r#"
INSERT INTO scan_jobs (
    tenant_id, address_id, chain_id, status, max_attempts, next_attempt_at
)
VALUES ($1, $2, $3, 'pending', $4, $5)
ON CONFLICT DO NOTHING
RETURNING id, tenant_id, address_id, chain_id, status, attempt_count, max_attempts,
          next_attempt_at, locked_at, locked_by, lease_expires_at, last_error,
          last_scan_run_id, retry_of_scan_run_id, succeeded_at, dead_lettered_at,
          created_at, updated_at
"#;

pub const INSERT_RETRY_SCAN_JOB_QUERY: &str = r#"
INSERT INTO scan_jobs (
    tenant_id, address_id, chain_id, status, max_attempts, next_attempt_at, retry_of_scan_run_id
)
VALUES ($1, $2, $3, 'pending', $4, $5, $6)
ON CONFLICT (address_id) WHERE status IN ('pending', 'retryable', 'processing')
DO UPDATE SET status = 'pending',
              max_attempts = EXCLUDED.max_attempts,
              next_attempt_at = EXCLUDED.next_attempt_at,
              retry_of_scan_run_id = EXCLUDED.retry_of_scan_run_id,
              last_error = NULL,
              updated_at = NOW()
WHERE scan_jobs.status IN ('pending', 'retryable')
RETURNING id, tenant_id, address_id, chain_id, status, attempt_count, max_attempts,
          next_attempt_at, locked_at, locked_by, lease_expires_at, last_error,
          last_scan_run_id, retry_of_scan_run_id, succeeded_at, dead_lettered_at,
          created_at, updated_at
"#;

pub const CLAIM_DUE_SCAN_JOB_QUERY: &str = r#"
WITH dead_lettered AS (
    UPDATE scan_jobs
    SET status = 'dead_letter',
        last_error = COALESCE(last_error, 'scan job lease expired after max attempts'),
        dead_lettered_at = NOW(),
        locked_at = NULL,
        locked_by = NULL,
        lease_expires_at = NULL,
        updated_at = NOW()
    WHERE status = 'processing'
      AND lease_expires_at <= NOW()
      AND attempt_count >= max_attempts
    RETURNING id, last_scan_run_id
),
dead_lettered_runs AS (
    UPDATE scan_runs sr
    SET status = 'failed',
        finished_at = NOW(),
        duration_ms = GREATEST(0::double precision, EXTRACT(EPOCH FROM (NOW() - started_at)) * 1000)::BIGINT,
        error_message = COALESCE(error_message, 'scan job lease expired after max attempts'),
        metadata = metadata || jsonb_build_object('outcome', 'failed', 'scan_job_reclaimed', true),
        updated_at = NOW()
    FROM dead_lettered
    WHERE sr.id = dead_lettered.last_scan_run_id
      AND sr.status = 'running'
    RETURNING sr.id
),
due AS (
    SELECT id, last_scan_run_id
    FROM scan_jobs
    WHERE (
        status IN ('pending', 'retryable')
        AND next_attempt_at <= NOW()
    )
    OR (
        status = 'processing'
        AND lease_expires_at <= NOW()
        AND attempt_count < max_attempts
        AND NOT EXISTS (SELECT 1 FROM dead_lettered WHERE dead_lettered.id = scan_jobs.id)
    )
    ORDER BY CASE
        WHEN status = 'processing' AND lease_expires_at <= NOW() THEN 0
        ELSE 1
    END,
    next_attempt_at ASC,
    created_at ASC
    LIMIT 1
    FOR UPDATE SKIP LOCKED
),
reclaimed_runs AS (
    UPDATE scan_runs sr
    SET status = 'failed',
        finished_at = NOW(),
        duration_ms = GREATEST(0::double precision, EXTRACT(EPOCH FROM (NOW() - started_at)) * 1000)::BIGINT,
        error_message = COALESCE(error_message, 'scan job lease expired before retry'),
        metadata = metadata || jsonb_build_object('outcome', 'failed', 'scan_job_reclaimed', true),
        updated_at = NOW()
    FROM due
    WHERE sr.id = due.last_scan_run_id
      AND sr.status = 'running'
    RETURNING sr.id
)
UPDATE scan_jobs job
SET status = 'processing',
    attempt_count = attempt_count + 1,
    locked_at = NOW(),
    locked_by = $1,
    lease_expires_at = NOW() + ($2 || ' seconds')::interval,
    updated_at = NOW()
FROM due
WHERE job.id = due.id
RETURNING job.id, job.tenant_id, job.address_id, job.chain_id, job.status,
          job.attempt_count, job.max_attempts, job.next_attempt_at, job.locked_at,
          job.locked_by, job.lease_expires_at, job.last_error, job.last_scan_run_id,
          job.retry_of_scan_run_id, job.succeeded_at, job.dead_lettered_at,
          job.created_at, job.updated_at
"#;

pub const RENEW_SCAN_JOB_LEASE_QUERY: &str = r#"
UPDATE scan_jobs
SET locked_at = NOW(),
    lease_expires_at = NOW() + ($3 || ' seconds')::interval,
    updated_at = NOW()
WHERE id = $1
  AND locked_by = $2
  AND status = 'processing'
  AND lease_expires_at > NOW()
"#;

pub const MARK_SCAN_JOB_SUCCEEDED_QUERY: &str = r#"
UPDATE scan_jobs
SET status = 'succeeded',
    last_error = NULL,
    last_scan_run_id = $3,
    succeeded_at = NOW(),
    locked_at = NULL,
    locked_by = NULL,
    lease_expires_at = NULL,
    updated_at = NOW()
WHERE id = $1
  AND locked_by = $2
  AND status = 'processing'
  AND lease_expires_at > NOW()
"#;

pub const MARK_SCAN_JOB_RETRYABLE_QUERY: &str = r#"
UPDATE scan_jobs
SET status = 'retryable',
    next_attempt_at = $3,
    last_error = $4,
    last_scan_run_id = $5,
    locked_at = NULL,
    locked_by = NULL,
    lease_expires_at = NULL,
    updated_at = NOW()
WHERE id = $1
  AND locked_by = $2
  AND status = 'processing'
  AND lease_expires_at > NOW()
"#;

pub const MARK_SCAN_JOB_DEAD_LETTER_QUERY: &str = r#"
UPDATE scan_jobs
SET status = 'dead_letter',
    last_error = $3,
    last_scan_run_id = $4,
    dead_lettered_at = NOW(),
    locked_at = NULL,
    locked_by = NULL,
    lease_expires_at = NULL,
    updated_at = NOW()
WHERE id = $1
  AND locked_by = $2
  AND status = 'processing'
  AND lease_expires_at > NOW()
"#;

pub const SCAN_JOB_STATUS_COUNTS_QUERY: &str = r#"
SELECT COUNT(*) FILTER (WHERE status = 'pending') AS pending,
       COUNT(*) FILTER (WHERE status = 'retryable') AS retryable,
       COUNT(*) FILTER (WHERE status = 'processing') AS processing,
       COUNT(*) FILTER (WHERE status = 'succeeded') AS succeeded,
       COUNT(*) FILTER (WHERE status = 'dead_letter') AS dead_letter,
       COUNT(*) FILTER (
           WHERE status = 'processing'
             AND lease_expires_at <= $2
       ) AS stale_processing,
       MIN(next_attempt_at) FILTER (
           WHERE status IN ('pending', 'retryable')
       ) AS next_attempt_at
FROM scan_jobs
WHERE tenant_id = $1
"#;

pub fn scan_job_next_attempt_at(now: DateTime<Utc>, attempt_count: i32) -> DateTime<Utc> {
    let delay_seconds = match attempt_count {
        0 | 1 => 30,
        2 => 60,
        3 => 300,
        4 => 900,
        _ => 3600,
    };
    now + Duration::seconds(delay_seconds)
}

pub fn scan_job_should_dead_letter(attempt_count: i32, max_attempts: i32) -> bool {
    attempt_count >= max_attempts
}

pub fn scan_job_to_task(job: &ScanJob, enqueued_at: DateTime<Utc>) -> ScanAddressTask {
    ScanAddressTask {
        task_id: job.id,
        address_id: job.address_id,
        tenant_id: job.tenant_id,
        chain_id: job.chain_id,
        attempt: job.attempt_count.clamp(0, u16::MAX as i32) as u16,
        enqueued_at,
    }
}

pub async fn insert_scheduled_scan_job_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    candidate: &ScanAddressCandidate,
    max_attempts: i32,
    next_attempt_at: DateTime<Utc>,
) -> AppResult<Option<ScanJob>> {
    sqlx::query_as::<_, ScanJob>(INSERT_SCHEDULED_SCAN_JOB_QUERY)
        .bind(candidate.tenant_id)
        .bind(candidate.id)
        .bind(candidate.chain_id)
        .bind(max_attempts)
        .bind(next_attempt_at)
        .fetch_optional(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn insert_retry_scan_job(
    pool: &PgPool,
    tenant_id: Uuid,
    address_id: Uuid,
    chain_id: Uuid,
    retry_of_scan_run_id: Uuid,
    max_attempts: i32,
    next_attempt_at: DateTime<Utc>,
) -> AppResult<Option<ScanJob>> {
    sqlx::query_as::<_, ScanJob>(INSERT_RETRY_SCAN_JOB_QUERY)
        .bind(tenant_id)
        .bind(address_id)
        .bind(chain_id)
        .bind(max_attempts)
        .bind(next_attempt_at)
        .bind(retry_of_scan_run_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn insert_retry_scan_job_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    address_id: Uuid,
    chain_id: Uuid,
    retry_of_scan_run_id: Uuid,
    max_attempts: i32,
    next_attempt_at: DateTime<Utc>,
) -> AppResult<Option<ScanJob>> {
    sqlx::query_as::<_, ScanJob>(INSERT_RETRY_SCAN_JOB_QUERY)
        .bind(tenant_id)
        .bind(address_id)
        .bind(chain_id)
        .bind(max_attempts)
        .bind(next_attempt_at)
        .bind(retry_of_scan_run_id)
        .fetch_optional(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn claim_due_scan_job(
    pool: &PgPool,
    worker_id: &str,
    lease_ttl_seconds: u64,
) -> AppResult<Option<ScanJob>> {
    sqlx::query_as::<_, ScanJob>(CLAIM_DUE_SCAN_JOB_QUERY)
        .bind(worker_id)
        .bind(i64::try_from(lease_ttl_seconds).map_err(|_| AppError::Config("SCAN_LOCK_TTL_SECONDS must fit in i64".to_string()))?)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn renew_scan_job_lease(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    lease_ttl_seconds: u64,
) -> AppResult<bool> {
    let result = sqlx::query(RENEW_SCAN_JOB_LEASE_QUERY)
        .bind(job_id)
        .bind(worker_id)
        .bind(i64::try_from(lease_ttl_seconds).map_err(|_| AppError::Config("SCAN_LOCK_TTL_SECONDS must fit in i64".to_string()))?)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    Ok(result.rows_affected() > 0)
}

pub async fn mark_scan_job_succeeded(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    scan_run_id: Option<Uuid>,
) -> AppResult<()> {
    mark_scan_job_succeeded_with_executor(pool, job_id, worker_id, scan_run_id).await
}

pub async fn mark_scan_job_succeeded_with_executor<'e, E>(
    executor: E,
    job_id: Uuid,
    worker_id: &str,
    scan_run_id: Option<Uuid>,
) -> AppResult<()>
where
    E: Executor<'e, Database = Postgres>,
{
    ensure_scan_job_updated(
        sqlx::query(MARK_SCAN_JOB_SUCCEEDED_QUERY)
            .bind(job_id)
            .bind(worker_id)
            .bind(scan_run_id)
            .execute(executor)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .rows_affected(),
    )
}

pub async fn mark_scan_job_retryable(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    next_attempt_at: DateTime<Utc>,
    last_error: &str,
    scan_run_id: Option<Uuid>,
) -> AppResult<()> {
    mark_scan_job_retryable_with_executor(
        pool,
        job_id,
        worker_id,
        next_attempt_at,
        last_error,
        scan_run_id,
    )
    .await
}

pub async fn mark_scan_job_retryable_with_executor<'e, E>(
    executor: E,
    job_id: Uuid,
    worker_id: &str,
    next_attempt_at: DateTime<Utc>,
    last_error: &str,
    scan_run_id: Option<Uuid>,
) -> AppResult<()>
where
    E: Executor<'e, Database = Postgres>,
{
    ensure_scan_job_updated(
        sqlx::query(MARK_SCAN_JOB_RETRYABLE_QUERY)
            .bind(job_id)
            .bind(worker_id)
            .bind(next_attempt_at)
            .bind(last_error)
            .bind(scan_run_id)
            .execute(executor)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .rows_affected(),
    )
}

pub async fn mark_scan_job_dead_letter(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    last_error: &str,
    scan_run_id: Option<Uuid>,
) -> AppResult<()> {
    mark_scan_job_dead_letter_with_executor(pool, job_id, worker_id, last_error, scan_run_id).await
}

pub async fn mark_scan_job_dead_letter_with_executor<'e, E>(
    executor: E,
    job_id: Uuid,
    worker_id: &str,
    last_error: &str,
    scan_run_id: Option<Uuid>,
) -> AppResult<()>
where
    E: Executor<'e, Database = Postgres>,
{
    ensure_scan_job_updated(
        sqlx::query(MARK_SCAN_JOB_DEAD_LETTER_QUERY)
            .bind(job_id)
            .bind(worker_id)
            .bind(last_error)
            .bind(scan_run_id)
            .execute(executor)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .rows_affected(),
    )
}

pub async fn scan_job_status_counts(
    pool: &PgPool,
    tenant_id: Uuid,
    now: DateTime<Utc>,
) -> AppResult<ScanJobStatusCounts> {
    sqlx::query_as::<_, ScanJobStatusCounts>(SCAN_JOB_STATUS_COUNTS_QUERY)
        .bind(tenant_id)
        .bind(now)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub fn ensure_scan_job_updated(rows_affected: u64) -> AppResult<()> {
    if rows_affected == 0 {
        return Err(AppError::NotFound("scan job".to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn scan_jobs_migration_defines_durable_queue_table_and_indexes() {
        assert!(SCAN_JOBS_MIGRATION.contains("CREATE TABLE IF NOT EXISTS scan_jobs"));
        for status in [
            SCAN_JOB_STATUS_PENDING,
            SCAN_JOB_STATUS_RETRYABLE,
            SCAN_JOB_STATUS_PROCESSING,
            SCAN_JOB_STATUS_SUCCEEDED,
            SCAN_JOB_STATUS_DEAD_LETTER,
        ] {
            assert!(SCAN_JOBS_MIGRATION.contains(status), "missing status {status}");
        }
        assert!(SCAN_JOBS_MIGRATION.contains("idx_scan_jobs_active_address"));
        assert!(SCAN_JOBS_MIGRATION.contains("WHERE status IN ('pending', 'retryable', 'processing')"));
        assert!(SCAN_JOBS_MIGRATION.contains("idx_scan_jobs_claim"));
        assert!(SCAN_JOBS_MIGRATION.contains("idx_scan_jobs_processing_stale"));
        assert!(SCAN_JOBS_MIGRATION.contains("idx_scan_jobs_tenant_status_created"));
    }

    #[test]
    fn scan_job_claim_query_claims_due_and_stale_jobs_safely() {
        assert!(CLAIM_DUE_SCAN_JOB_QUERY.contains("FOR UPDATE SKIP LOCKED"));
        assert!(CLAIM_DUE_SCAN_JOB_QUERY.contains("status IN ('pending', 'retryable')"));
        assert!(CLAIM_DUE_SCAN_JOB_QUERY.contains("next_attempt_at <= NOW()"));
        assert!(CLAIM_DUE_SCAN_JOB_QUERY.contains("status = 'processing'"));
        assert!(CLAIM_DUE_SCAN_JOB_QUERY.contains("lease_expires_at <= NOW()"));
        assert!(CLAIM_DUE_SCAN_JOB_QUERY.contains("attempt_count = attempt_count + 1"));
        assert!(CLAIM_DUE_SCAN_JOB_QUERY.contains("locked_by = $1"));
    }

    #[test]
    fn retry_scan_job_query_reuses_pending_or_retryable_active_job() {
        assert!(INSERT_RETRY_SCAN_JOB_QUERY.contains("ON CONFLICT (address_id)"));
        assert!(INSERT_RETRY_SCAN_JOB_QUERY
            .contains("WHERE status IN ('pending', 'retryable', 'processing')"));
        assert!(INSERT_RETRY_SCAN_JOB_QUERY.contains("DO UPDATE"));
        assert!(INSERT_RETRY_SCAN_JOB_QUERY.contains("next_attempt_at = EXCLUDED.next_attempt_at"));
        assert!(INSERT_RETRY_SCAN_JOB_QUERY.contains("retry_of_scan_run_id = EXCLUDED.retry_of_scan_run_id"));
        assert!(INSERT_RETRY_SCAN_JOB_QUERY.contains("WHERE scan_jobs.status IN ('pending', 'retryable')"));
    }

    #[test]
    fn scan_job_claim_dead_letters_stale_exhausted_processing_jobs() {
        assert!(CLAIM_DUE_SCAN_JOB_QUERY.contains("dead_lettered AS"));
        assert!(CLAIM_DUE_SCAN_JOB_QUERY.contains("attempt_count >= max_attempts"));
        assert!(CLAIM_DUE_SCAN_JOB_QUERY.contains("status = 'dead_letter'"));
        assert!(CLAIM_DUE_SCAN_JOB_QUERY.contains("attempt_count < max_attempts"));
        assert!(CLAIM_DUE_SCAN_JOB_QUERY.contains("dead_lettered_runs AS"));
    }

    #[test]
    fn scan_job_claim_closes_reclaimed_running_scan_runs() {
        assert!(CLAIM_DUE_SCAN_JOB_QUERY.contains("reclaimed_runs AS"));
        assert!(CLAIM_DUE_SCAN_JOB_QUERY.contains("UPDATE scan_runs sr"));
        assert!(CLAIM_DUE_SCAN_JOB_QUERY.contains("sr.id = due.last_scan_run_id"));
        assert!(CLAIM_DUE_SCAN_JOB_QUERY.contains("sr.status = 'running'"));
        assert!(CLAIM_DUE_SCAN_JOB_QUERY.contains("'scan_job_reclaimed', true"));
    }

    #[test]
    fn scan_job_transition_queries_require_processing_owner() {
        for query in [
            RENEW_SCAN_JOB_LEASE_QUERY,
            MARK_SCAN_JOB_SUCCEEDED_QUERY,
            MARK_SCAN_JOB_RETRYABLE_QUERY,
            MARK_SCAN_JOB_DEAD_LETTER_QUERY,
        ] {
            assert!(query.contains("locked_by = $2"), "missing owner guard in {query}");
            assert!(query.contains("status = 'processing'"), "missing processing guard in {query}");
        }
    }

    #[test]
    fn scan_job_renew_and_transition_queries_require_live_database_clock_lease() {
        for query in [
            RENEW_SCAN_JOB_LEASE_QUERY,
            MARK_SCAN_JOB_SUCCEEDED_QUERY,
            MARK_SCAN_JOB_RETRYABLE_QUERY,
            MARK_SCAN_JOB_DEAD_LETTER_QUERY,
        ] {
            assert!(query.contains("lease_expires_at > NOW()"), "missing DB-clock lease guard in {query}");
        }
    }

    #[test]
    fn scan_job_retryable_query_does_not_use_backoff_time_as_lease_guard() {
        assert!(MARK_SCAN_JOB_RETRYABLE_QUERY.contains("next_attempt_at = $3"));
        assert!(!MARK_SCAN_JOB_RETRYABLE_QUERY.contains("lease_expires_at > $3"));
        assert!(MARK_SCAN_JOB_RETRYABLE_QUERY.contains("lease_expires_at > NOW()"));
    }

    #[test]
    fn scan_job_backoff_matches_notification_outbox_schedule() {
        let now = Utc.with_ymd_and_hms(2026, 5, 27, 10, 0, 0).unwrap();

        assert_eq!(scan_job_next_attempt_at(now, 1), now + Duration::seconds(30));
        assert_eq!(scan_job_next_attempt_at(now, 2), now + Duration::seconds(60));
        assert_eq!(scan_job_next_attempt_at(now, 3), now + Duration::seconds(300));
        assert_eq!(scan_job_next_attempt_at(now, 4), now + Duration::seconds(900));
        assert_eq!(scan_job_next_attempt_at(now, 5), now + Duration::seconds(3600));
    }

    #[test]
    fn scan_job_dead_letter_boundary_uses_attempt_count() {
        assert!(!scan_job_should_dead_letter(4, 5));
        assert!(scan_job_should_dead_letter(5, 5));
        assert!(scan_job_should_dead_letter(6, 5));
    }

    #[test]
    fn scan_job_to_task_preserves_job_identity_and_attempt() {
        let now = Utc.with_ymd_and_hms(2026, 5, 27, 10, 0, 0).unwrap();
        let job = ScanJob {
            id: Uuid::from_u128(1),
            tenant_id: Uuid::from_u128(2),
            address_id: Uuid::from_u128(3),
            chain_id: Uuid::from_u128(4),
            status: SCAN_JOB_STATUS_PROCESSING.to_string(),
            attempt_count: 2,
            max_attempts: 10,
            next_attempt_at: now,
            locked_at: Some(now),
            locked_by: Some("worker".to_string()),
            lease_expires_at: Some(now + Duration::seconds(120)),
            last_error: None,
            last_scan_run_id: None,
            retry_of_scan_run_id: None,
            succeeded_at: None,
            dead_lettered_at: None,
            created_at: now,
            updated_at: now,
        };

        let task = scan_job_to_task(&job, now);

        assert_eq!(task.task_id, job.id);
        assert_eq!(task.tenant_id, job.tenant_id);
        assert_eq!(task.address_id, job.address_id);
        assert_eq!(task.chain_id, job.chain_id);
        assert_eq!(task.attempt, 2);
        assert_eq!(task.enqueued_at, now);
    }
}
