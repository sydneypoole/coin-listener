use std::collections::HashSet;

use chrono::{DateTime, Utc};
use coin_listener_core::{
    models::{
        CreateWatchedAddressImportRequest, WatchedAddressImportErrorRow,
        WatchedAddressImportRowRequest, WatchedAddressImportTask,
    },
    AppError, AppResult,
};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub const CLAIM_WATCHED_ADDRESS_IMPORT_QUERY: &str = r#"
WITH next_task AS (
    SELECT task.id
    FROM watched_address_import_tasks task
    WHERE task.status = 'pending'
       OR (
           task.status = 'running'
           AND task.locked_by = $2
           AND EXISTS (
               SELECT 1
               FROM watched_address_import_rows import_row
               WHERE import_row.import_task_id = task.id
                 AND import_row.tenant_id = task.tenant_id
                 AND import_row.status = 'pending'
           )
       )
    ORDER BY CASE
        WHEN task.status = 'running' AND task.locked_by = $2 THEN 0
        ELSE 1
    END,
    task.created_at ASC
    LIMIT 1
    FOR UPDATE SKIP LOCKED
)
UPDATE watched_address_import_tasks task
SET status = 'running',
    locked_at = $1,
    locked_by = $2,
    started_at = COALESCE(started_at, $1),
    updated_at = NOW()
FROM next_task
WHERE task.id = next_task.id
RETURNING task.id, task.tenant_id, task.status, task.chain_id, task.asset_ids,
          task.priority, task.scan_interval_seconds, task.transfer_filter_enabled,
          task.balance_change_filter_enabled, task.address_status, task.total_rows,
          task.processed_rows, task.success_rows, task.failed_rows, task.locked_at,
          task.locked_by, task.started_at, task.completed_at, task.last_error,
          task.created_at, task.updated_at
"#;

pub const PENDING_IMPORT_ROWS_QUERY: &str = r#"
SELECT row_number, raw_text, address, label, priority, scan_interval_seconds,
       transfer_filter_enabled, balance_change_filter_enabled, address_status AS status
FROM watched_address_import_rows
WHERE import_task_id = $1
  AND tenant_id = $2
  AND status = 'pending'
ORDER BY row_number ASC
LIMIT $3
"#;

pub const MARK_IMPORT_ROW_SUCCESS_QUERY: &str = r#"
UPDATE watched_address_import_rows
SET status = 'success',
    watched_address_id = $4,
    error_code = NULL,
    error_message = NULL,
    updated_at = NOW()
WHERE import_task_id = $1
  AND tenant_id = $2
  AND row_number = $3
  AND status = 'pending'
"#;

pub const MARK_IMPORT_ROW_FAILED_QUERY: &str = r#"
UPDATE watched_address_import_rows
SET status = 'failed',
    error_code = $4,
    error_message = $5,
    updated_at = NOW()
WHERE import_task_id = $1
  AND tenant_id = $2
  AND row_number = $3
  AND status = 'pending'
"#;

pub const REFRESH_IMPORT_TASK_COUNTS_QUERY: &str = r#"
UPDATE watched_address_import_tasks task
SET processed_rows = counts.processed_rows,
    success_rows = counts.success_rows,
    failed_rows = counts.failed_rows,
    updated_at = NOW()
FROM (
    SELECT COUNT(*) FILTER (WHERE status IN ('success', 'failed', 'skipped'))::int AS processed_rows,
           COUNT(*) FILTER (WHERE status = 'success')::int AS success_rows,
           COUNT(*) FILTER (WHERE status = 'failed')::int AS failed_rows
    FROM watched_address_import_rows
    WHERE import_task_id = $1
      AND tenant_id = $2
) counts
WHERE task.id = $1
  AND task.tenant_id = $2
RETURNING task.id, task.tenant_id, task.status, task.chain_id, task.asset_ids,
          task.priority, task.scan_interval_seconds, task.transfer_filter_enabled,
          task.balance_change_filter_enabled, task.address_status, task.total_rows,
          task.processed_rows, task.success_rows, task.failed_rows, task.locked_at,
          task.locked_by, task.started_at, task.completed_at, task.last_error,
          task.created_at, task.updated_at
"#;

pub const COMPLETE_IMPORT_IF_FINISHED_QUERY: &str = r#"
UPDATE watched_address_import_tasks
SET status = $3,
    completed_at = $4,
    locked_at = NULL,
    locked_by = NULL,
    updated_at = NOW()
WHERE id = $1
  AND tenant_id = $2
  AND status = 'running'
RETURNING id, tenant_id, status, chain_id, asset_ids, priority, scan_interval_seconds,
          transfer_filter_enabled, balance_change_filter_enabled, address_status,
          total_rows, processed_rows, success_rows, failed_rows, locked_at, locked_by,
          started_at, completed_at, last_error, created_at, updated_at
"#;

const CREATE_WATCHED_ADDRESS_IMPORT_QUERY: &str = r#"
INSERT INTO watched_address_import_tasks (
    tenant_id, chain_id, asset_ids, priority, scan_interval_seconds,
    transfer_filter_enabled, balance_change_filter_enabled, address_status, total_rows
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
RETURNING id, tenant_id, status, chain_id, asset_ids, priority, scan_interval_seconds,
          transfer_filter_enabled, balance_change_filter_enabled, address_status,
          total_rows, processed_rows, success_rows, failed_rows, locked_at, locked_by,
          started_at, completed_at, last_error, created_at, updated_at
"#;

const INSERT_IMPORT_ROW_QUERY: &str = r#"
INSERT INTO watched_address_import_rows (
    import_task_id, tenant_id, row_number, raw_text, address, label, priority,
    scan_interval_seconds, transfer_filter_enabled, balance_change_filter_enabled,
    address_status
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
"#;

const GET_WATCHED_ADDRESS_IMPORT_QUERY: &str = r#"
SELECT id, tenant_id, status, chain_id, asset_ids, priority, scan_interval_seconds,
       transfer_filter_enabled, balance_change_filter_enabled, address_status,
       total_rows, processed_rows, success_rows, failed_rows, locked_at, locked_by,
       started_at, completed_at, last_error, created_at, updated_at
FROM watched_address_import_tasks
WHERE id = $1
  AND tenant_id = $2
"#;

const LIST_WATCHED_ADDRESS_IMPORT_ERRORS_QUERY: &str = r#"
SELECT row_number, address, raw_text, error_code, error_message
FROM watched_address_import_rows
WHERE import_task_id = $1
  AND tenant_id = $2
  AND status = 'failed'
ORDER BY row_number ASC
"#;

const CANCEL_WATCHED_ADDRESS_IMPORT_ROWS_QUERY: &str = r#"
UPDATE watched_address_import_rows import_row
SET status = 'skipped',
    error_code = 'cancelled',
    error_message = 'import task cancelled',
    updated_at = NOW()
WHERE import_row.import_task_id = $1
  AND import_row.tenant_id = $2
  AND import_row.status = 'pending'
  AND EXISTS (
      SELECT 1
      FROM watched_address_import_tasks task
      WHERE task.id = import_row.import_task_id
        AND task.tenant_id = import_row.tenant_id
        AND task.status IN ('pending', 'running')
  )
"#;

const CANCEL_WATCHED_ADDRESS_IMPORT_QUERY: &str = r#"
UPDATE watched_address_import_tasks task
SET status = 'cancelled',
    processed_rows = counts.processed_rows,
    success_rows = counts.success_rows,
    failed_rows = counts.failed_rows,
    completed_at = COALESCE(task.completed_at, NOW()),
    locked_at = NULL,
    locked_by = NULL,
    updated_at = NOW()
FROM (
    SELECT COUNT(*) FILTER (WHERE status IN ('success', 'failed', 'skipped'))::int AS processed_rows,
           COUNT(*) FILTER (WHERE status = 'success')::int AS success_rows,
           COUNT(*) FILTER (WHERE status = 'failed')::int AS failed_rows
    FROM watched_address_import_rows
    WHERE import_task_id = $1
      AND tenant_id = $2
) counts
WHERE task.id = $1
  AND task.tenant_id = $2
  AND task.status IN ('pending', 'running')
RETURNING task.id, task.tenant_id, task.status, task.chain_id, task.asset_ids,
          task.priority, task.scan_interval_seconds, task.transfer_filter_enabled,
          task.balance_change_filter_enabled, task.address_status, task.total_rows,
          task.processed_rows, task.success_rows, task.failed_rows, task.locked_at,
          task.locked_by, task.started_at, task.completed_at, task.last_error,
          task.created_at, task.updated_at
"#;

#[derive(Debug, sqlx::FromRow)]
struct WatchedAddressImportRow {
    row_number: i32,
    raw_text: String,
    address: String,
    label: Option<String>,
    priority: Option<String>,
    scan_interval_seconds: Option<i32>,
    transfer_filter_enabled: Option<bool>,
    balance_change_filter_enabled: Option<bool>,
    status: Option<String>,
}

impl From<WatchedAddressImportRow> for WatchedAddressImportRowRequest {
    fn from(row: WatchedAddressImportRow) -> Self {
        Self {
            row_number: row.row_number,
            raw_text: row.raw_text,
            address: row.address,
            label: row.label,
            priority: row.priority,
            scan_interval_seconds: row.scan_interval_seconds,
            transfer_filter_enabled: row.transfer_filter_enabled,
            balance_change_filter_enabled: row.balance_change_filter_enabled,
            status: row.status,
        }
    }
}

pub fn validate_import_create_request(
    request: &CreateWatchedAddressImportRequest,
) -> AppResult<()> {
    if request.rows.is_empty() {
        return Err(AppError::Validation("import rows are required".to_string()));
    }
    if request.defaults.asset_ids.is_empty() {
        return Err(AppError::Validation("asset_ids are required".to_string()));
    }

    let mut row_numbers = HashSet::new();
    let mut addresses = HashSet::new();
    for row in &request.rows {
        if row.row_number <= 0 {
            return Err(AppError::Validation(
                "row_number must be positive".to_string(),
            ));
        }
        if row.address.trim().is_empty() {
            return Err(AppError::Validation("address is required".to_string()));
        }
        if !row_numbers.insert(row.row_number) {
            return Err(AppError::Validation(
                "row_number must be unique".to_string(),
            ));
        }
        let normalized = row.address.trim().to_ascii_lowercase();
        if !addresses.insert(normalized) {
            return Err(AppError::Validation(
                "addresses must be unique within an import".to_string(),
            ));
        }
    }

    Ok(())
}

pub async fn create_watched_address_import(
    pool: &PgPool,
    tenant_id: Uuid,
    request: CreateWatchedAddressImportRequest,
) -> AppResult<WatchedAddressImportTask> {
    validate_import_create_request(&request)?;
    let defaults = request.defaults;
    let rows = request.rows;

    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let task = sqlx::query_as::<_, WatchedAddressImportTask>(CREATE_WATCHED_ADDRESS_IMPORT_QUERY)
        .bind(tenant_id)
        .bind(defaults.chain_id)
        .bind(&defaults.asset_ids)
        .bind(defaults.priority)
        .bind(defaults.scan_interval_seconds)
        .bind(defaults.transfer_filter_enabled)
        .bind(defaults.balance_change_filter_enabled)
        .bind(defaults.status)
        .bind(rows.len() as i32)
        .fetch_one(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    insert_import_rows(&mut transaction, tenant_id, task.id, &rows).await?;
    transaction
        .commit()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    Ok(task)
}

async fn insert_import_rows(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    task_id: Uuid,
    rows: &[WatchedAddressImportRowRequest],
) -> AppResult<()> {
    for row in rows {
        sqlx::query(INSERT_IMPORT_ROW_QUERY)
            .bind(task_id)
            .bind(tenant_id)
            .bind(row.row_number)
            .bind(&row.raw_text)
            .bind(&row.address)
            .bind(&row.label)
            .bind(&row.priority)
            .bind(row.scan_interval_seconds)
            .bind(row.transfer_filter_enabled)
            .bind(row.balance_change_filter_enabled)
            .bind(&row.status)
            .execute(transaction.as_mut())
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
    }
    Ok(())
}

pub async fn get_watched_address_import(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
) -> AppResult<WatchedAddressImportTask> {
    sqlx::query_as::<_, WatchedAddressImportTask>(GET_WATCHED_ADDRESS_IMPORT_QUERY)
        .bind(id)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("watched address import".to_string()))
}

pub async fn list_watched_address_import_errors(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
) -> AppResult<Vec<WatchedAddressImportErrorRow>> {
    sqlx::query_as::<_, WatchedAddressImportErrorRow>(LIST_WATCHED_ADDRESS_IMPORT_ERRORS_QUERY)
        .bind(id)
        .bind(tenant_id)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn cancel_watched_address_import(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
) -> AppResult<WatchedAddressImportTask> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    sqlx::query(CANCEL_WATCHED_ADDRESS_IMPORT_ROWS_QUERY)
        .bind(id)
        .bind(tenant_id)
        .execute(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let task = sqlx::query_as::<_, WatchedAddressImportTask>(CANCEL_WATCHED_ADDRESS_IMPORT_QUERY)
        .bind(id)
        .bind(tenant_id)
        .fetch_optional(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("watched address import".to_string()))?;

    transaction
        .commit()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    Ok(task)
}

pub async fn claim_next_watched_address_import(
    pool: &PgPool,
    now: DateTime<Utc>,
    worker_id: &str,
) -> AppResult<Option<WatchedAddressImportTask>> {
    sqlx::query_as::<_, WatchedAddressImportTask>(CLAIM_WATCHED_ADDRESS_IMPORT_QUERY)
        .bind(now)
        .bind(worker_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn pending_import_rows(
    pool: &PgPool,
    tenant_id: Uuid,
    task_id: Uuid,
    limit: i64,
) -> AppResult<Vec<WatchedAddressImportRowRequest>> {
    if limit <= 0 {
        return Ok(Vec::new());
    }

    let rows = sqlx::query_as::<_, WatchedAddressImportRow>(PENDING_IMPORT_ROWS_QUERY)
        .bind(task_id)
        .bind(tenant_id)
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn mark_import_row_success(
    pool: &PgPool,
    tenant_id: Uuid,
    task_id: Uuid,
    row_number: i32,
    watched_address_id: Uuid,
) -> AppResult<()> {
    let result = sqlx::query(MARK_IMPORT_ROW_SUCCESS_QUERY)
        .bind(task_id)
        .bind(tenant_id)
        .bind(row_number)
        .bind(watched_address_id)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    ensure_import_row_updated(result.rows_affected())
}

pub async fn mark_import_row_failed(
    pool: &PgPool,
    tenant_id: Uuid,
    task_id: Uuid,
    row_number: i32,
    error_code: &str,
    error_message: &str,
) -> AppResult<()> {
    let result = sqlx::query(MARK_IMPORT_ROW_FAILED_QUERY)
        .bind(task_id)
        .bind(tenant_id)
        .bind(row_number)
        .bind(error_code)
        .bind(error_message)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    ensure_import_row_updated(result.rows_affected())
}

pub async fn refresh_import_task_counts(
    pool: &PgPool,
    tenant_id: Uuid,
    task_id: Uuid,
) -> AppResult<WatchedAddressImportTask> {
    sqlx::query_as::<_, WatchedAddressImportTask>(REFRESH_IMPORT_TASK_COUNTS_QUERY)
        .bind(task_id)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("watched address import".to_string()))
}

pub async fn complete_import_if_finished(
    pool: &PgPool,
    tenant_id: Uuid,
    task_id: Uuid,
    now: DateTime<Utc>,
) -> AppResult<WatchedAddressImportTask> {
    let task = refresh_import_task_counts(pool, tenant_id, task_id).await?;
    if task.status == "cancelled"
        || task.processed_rows < task.total_rows
        || task.status != "running"
    {
        return Ok(task);
    }

    let next_status = if task.success_rows == 0 && task.failed_rows > 0 {
        "failed"
    } else {
        "completed"
    };

    sqlx::query_as::<_, WatchedAddressImportTask>(COMPLETE_IMPORT_IF_FINISHED_QUERY)
        .bind(task_id)
        .bind(tenant_id)
        .bind(next_status)
        .bind(now)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .map_or(Ok(task), Ok)
}

fn ensure_import_row_updated(rows_affected: u64) -> AppResult<()> {
    if rows_affected == 0 {
        return Err(AppError::NotFound("watched address import row".to_string()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        validate_import_create_request, CLAIM_WATCHED_ADDRESS_IMPORT_QUERY,
        MARK_IMPORT_ROW_FAILED_QUERY, MARK_IMPORT_ROW_SUCCESS_QUERY, PENDING_IMPORT_ROWS_QUERY,
        REFRESH_IMPORT_TASK_COUNTS_QUERY,
    };
    use coin_listener_core::models::{
        CreateWatchedAddressImportRequest, WatchedAddressImportDefaults,
        WatchedAddressImportRowRequest,
    };
    use uuid::Uuid;

    fn request_with_rows(
        rows: Vec<WatchedAddressImportRowRequest>,
    ) -> CreateWatchedAddressImportRequest {
        CreateWatchedAddressImportRequest {
            defaults: WatchedAddressImportDefaults {
                chain_id: Uuid::from_u128(2),
                asset_ids: vec![Uuid::from_u128(101)],
                chain_configs: Vec::new(),
                priority: "normal".to_string(),
                scan_interval_seconds: 300,
                transfer_filter_enabled: true,
                balance_change_filter_enabled: true,
                status: "active".to_string(),
            },
            rows,
        }
    }

    fn row(row_number: i32, address: &str) -> WatchedAddressImportRowRequest {
        WatchedAddressImportRowRequest {
            row_number,
            raw_text: address.to_string(),
            address: address.to_string(),
            label: None,
            priority: None,
            scan_interval_seconds: None,
            transfer_filter_enabled: None,
            balance_change_filter_enabled: None,
            status: None,
        }
    }

    #[test]
    fn import_validation_rejects_duplicate_addresses() {
        let rows = vec![
            row(1, " 0x0000000000000000000000000000000000000001 "),
            row(2, "0X0000000000000000000000000000000000000001"),
        ];

        let error = validate_import_create_request(&request_with_rows(rows)).unwrap_err();

        assert_eq!(
            error.to_string(),
            "validation error: addresses must be unique within an import"
        );
    }

    #[test]
    fn claim_query_uses_skip_locked() {
        assert!(CLAIM_WATCHED_ADDRESS_IMPORT_QUERY.contains("FOR UPDATE SKIP LOCKED"));
        assert!(CLAIM_WATCHED_ADDRESS_IMPORT_QUERY.contains("status = 'pending'"));
    }

    #[test]
    fn claim_query_resumes_running_tasks_for_same_worker() {
        assert!(CLAIM_WATCHED_ADDRESS_IMPORT_QUERY.contains("status = 'running'"));
        assert!(CLAIM_WATCHED_ADDRESS_IMPORT_QUERY.contains("locked_by = $2"));
        assert!(CLAIM_WATCHED_ADDRESS_IMPORT_QUERY.contains("EXISTS"));
    }

    #[test]
    fn row_processing_queries_are_tenant_scoped() {
        for query in [
            PENDING_IMPORT_ROWS_QUERY,
            MARK_IMPORT_ROW_SUCCESS_QUERY,
            MARK_IMPORT_ROW_FAILED_QUERY,
            REFRESH_IMPORT_TASK_COUNTS_QUERY,
        ] {
            assert!(query.contains("tenant_id = $2"), "{query}");
        }
    }
}
