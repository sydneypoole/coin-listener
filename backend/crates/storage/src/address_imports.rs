use std::collections::HashSet;

use chrono::{DateTime, Utc};
use coin_listener_core::{
    models::{
        CreateWatchedAddressImportRequest, WatchedAddressImportChainConfig,
        WatchedAddressImportDefaults, WatchedAddressImportErrorRow, WatchedAddressImportRowRequest,
        WatchedAddressImportTask,
    },
    AppError, AppResult,
};
use sqlx::{types::Json, PgPool, Postgres, Transaction};
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
          task.chain_configs, task.priority, task.scan_interval_seconds,
          task.transfer_filter_enabled, task.balance_change_filter_enabled,
          task.address_status, task.total_rows,
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
          task.chain_configs, task.priority, task.scan_interval_seconds,
          task.transfer_filter_enabled, task.balance_change_filter_enabled,
          task.address_status, task.total_rows,
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
RETURNING id, tenant_id, status, chain_id, asset_ids, chain_configs, priority,
          scan_interval_seconds, transfer_filter_enabled, balance_change_filter_enabled,
          address_status, total_rows, processed_rows, success_rows, failed_rows,
          locked_at, locked_by, started_at, completed_at, last_error, created_at, updated_at
"#;

pub const CREATE_WATCHED_ADDRESS_IMPORT_QUERY: &str = r#"
INSERT INTO watched_address_import_tasks (
    tenant_id, chain_id, asset_ids, chain_configs, priority, scan_interval_seconds,
    transfer_filter_enabled, balance_change_filter_enabled, address_status, total_rows
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
RETURNING id, tenant_id, status, chain_id, asset_ids, chain_configs, priority,
          scan_interval_seconds, transfer_filter_enabled, balance_change_filter_enabled,
          address_status, total_rows, processed_rows, success_rows, failed_rows,
          locked_at, locked_by, started_at, completed_at, last_error, created_at, updated_at
"#;

const INSERT_IMPORT_ROW_QUERY: &str = r#"
INSERT INTO watched_address_import_rows (
    import_task_id, tenant_id, row_number, raw_text, address, label, priority,
    scan_interval_seconds, transfer_filter_enabled, balance_change_filter_enabled,
    address_status
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
"#;

pub const INSERT_IMPORT_ATTEMPT_QUERY: &str = r#"
INSERT INTO watched_address_import_attempts (
    import_task_id, tenant_id, row_number, chain_id, asset_ids
)
VALUES ($1, $2, $3, $4, $5)
"#;

const GET_WATCHED_ADDRESS_IMPORT_QUERY: &str = r#"
SELECT id, tenant_id, status, chain_id, asset_ids, chain_configs, priority, scan_interval_seconds,
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
          task.chain_configs, task.priority, task.scan_interval_seconds,
          task.transfer_filter_enabled, task.balance_change_filter_enabled,
          task.address_status, task.total_rows,
          task.processed_rows, task.success_rows, task.failed_rows, task.locked_at,
          task.locked_by, task.started_at, task.completed_at, task.last_error,
          task.created_at, task.updated_at
"#;

#[derive(Debug, sqlx::FromRow)]
struct WatchedAddressImportTaskRecord {
    id: Uuid,
    tenant_id: Uuid,
    status: String,
    chain_id: Uuid,
    asset_ids: Vec<Uuid>,
    chain_configs: Json<Vec<WatchedAddressImportChainConfig>>,
    priority: String,
    scan_interval_seconds: i32,
    transfer_filter_enabled: bool,
    balance_change_filter_enabled: bool,
    address_status: String,
    total_rows: i32,
    processed_rows: i32,
    success_rows: i32,
    failed_rows: i32,
    locked_at: Option<DateTime<Utc>>,
    locked_by: Option<String>,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<WatchedAddressImportTaskRecord> for WatchedAddressImportTask {
    fn from(record: WatchedAddressImportTaskRecord) -> Self {
        Self {
            id: record.id,
            tenant_id: record.tenant_id,
            status: record.status,
            chain_id: record.chain_id,
            asset_ids: record.asset_ids,
            chain_configs: record.chain_configs.0,
            priority: record.priority,
            scan_interval_seconds: record.scan_interval_seconds,
            transfer_filter_enabled: record.transfer_filter_enabled,
            balance_change_filter_enabled: record.balance_change_filter_enabled,
            address_status: record.address_status,
            total_rows: record.total_rows,
            processed_rows: record.processed_rows,
            success_rows: record.success_rows,
            failed_rows: record.failed_rows,
            locked_at: record.locked_at,
            locked_by: record.locked_by,
            started_at: record.started_at,
            completed_at: record.completed_at,
            last_error: record.last_error,
            created_at: record.created_at,
            updated_at: record.updated_at,
        }
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImportAttemptSeed {
    row_number: i32,
    chain_id: Uuid,
    asset_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImportCreatePlan {
    chain_id: Uuid,
    asset_ids: Vec<Uuid>,
    chain_configs: Vec<WatchedAddressImportChainConfig>,
    total_rows: i32,
    attempts: Vec<ImportAttemptSeed>,
}

pub fn effective_import_chain_configs(
    defaults: &WatchedAddressImportDefaults,
) -> Vec<WatchedAddressImportChainConfig> {
    if defaults.chain_configs.is_empty() {
        return vec![WatchedAddressImportChainConfig {
            chain_id: defaults.chain_id,
            asset_ids: defaults.asset_ids.clone(),
        }];
    }

    defaults.chain_configs.clone()
}

fn import_create_plan(request: &CreateWatchedAddressImportRequest) -> AppResult<ImportCreatePlan> {
    let chain_configs = effective_import_chain_configs(&request.defaults);
    let first_config = chain_configs
        .first()
        .ok_or_else(|| AppError::Validation("chain_configs are required".to_string()))?;
    let total_rows = import_attempt_count(request.rows.len(), chain_configs.len())?;
    let mut attempts = Vec::with_capacity(total_rows as usize);

    for row in &request.rows {
        for config in &chain_configs {
            attempts.push(ImportAttemptSeed {
                row_number: row.row_number,
                chain_id: config.chain_id,
                asset_ids: config.asset_ids.clone(),
            });
        }
    }

    Ok(ImportCreatePlan {
        chain_id: first_config.chain_id,
        asset_ids: first_config.asset_ids.clone(),
        chain_configs,
        total_rows,
        attempts,
    })
}

fn import_attempt_count(row_count: usize, chain_config_count: usize) -> AppResult<i32> {
    row_count
        .checked_mul(chain_config_count)
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| AppError::Validation("import attempt count exceeds i32 range".to_string()))
}

pub fn validate_import_create_request(
    request: &CreateWatchedAddressImportRequest,
) -> AppResult<()> {
    if request.rows.is_empty() {
        return Err(AppError::Validation("import rows are required".to_string()));
    }

    let chain_configs = effective_import_chain_configs(&request.defaults);
    if chain_configs.is_empty() {
        return Err(AppError::Validation(
            "chain_configs are required".to_string(),
        ));
    }

    let mut chain_ids = HashSet::new();
    for config in &chain_configs {
        if config.asset_ids.is_empty() {
            return Err(AppError::Validation(
                "asset_ids are required for every chain config".to_string(),
            ));
        }
        if !chain_ids.insert(config.chain_id) {
            return Err(AppError::Validation(
                "chain_id must be unique within an import".to_string(),
            ));
        }
    }

    import_attempt_count(request.rows.len(), chain_configs.len())?;

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
    let plan = import_create_plan(&request)?;
    let defaults = request.defaults;
    let rows = request.rows;

    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let task_record =
        sqlx::query_as::<_, WatchedAddressImportTaskRecord>(CREATE_WATCHED_ADDRESS_IMPORT_QUERY)
            .bind(tenant_id)
            .bind(plan.chain_id)
            .bind(&plan.asset_ids)
            .bind(Json(plan.chain_configs.clone()))
            .bind(defaults.priority)
            .bind(defaults.scan_interval_seconds)
            .bind(defaults.transfer_filter_enabled)
            .bind(defaults.balance_change_filter_enabled)
            .bind(defaults.status)
            .bind(plan.total_rows)
            .fetch_one(transaction.as_mut())
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
    let task = WatchedAddressImportTask::from(task_record);

    insert_import_rows(&mut transaction, tenant_id, task.id, &rows).await?;
    insert_import_attempts(&mut transaction, tenant_id, task.id, &plan.attempts).await?;
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

async fn insert_import_attempts(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    task_id: Uuid,
    attempts: &[ImportAttemptSeed],
) -> AppResult<()> {
    for attempt in attempts {
        sqlx::query(INSERT_IMPORT_ATTEMPT_QUERY)
            .bind(task_id)
            .bind(tenant_id)
            .bind(attempt.row_number)
            .bind(attempt.chain_id)
            .bind(&attempt.asset_ids)
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
    sqlx::query_as::<_, WatchedAddressImportTaskRecord>(GET_WATCHED_ADDRESS_IMPORT_QUERY)
        .bind(id)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .map(WatchedAddressImportTask::from)
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

    let task =
        sqlx::query_as::<_, WatchedAddressImportTaskRecord>(CANCEL_WATCHED_ADDRESS_IMPORT_QUERY)
            .bind(id)
            .bind(tenant_id)
            .fetch_optional(transaction.as_mut())
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .map(WatchedAddressImportTask::from)
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
    sqlx::query_as::<_, WatchedAddressImportTaskRecord>(CLAIM_WATCHED_ADDRESS_IMPORT_QUERY)
        .bind(now)
        .bind(worker_id)
        .fetch_optional(pool)
        .await
        .map(|task| task.map(WatchedAddressImportTask::from))
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
    sqlx::query_as::<_, WatchedAddressImportTaskRecord>(REFRESH_IMPORT_TASK_COUNTS_QUERY)
        .bind(task_id)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .map(WatchedAddressImportTask::from)
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

    sqlx::query_as::<_, WatchedAddressImportTaskRecord>(COMPLETE_IMPORT_IF_FINISHED_QUERY)
        .bind(task_id)
        .bind(tenant_id)
        .bind(next_status)
        .bind(now)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .map(WatchedAddressImportTask::from)
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
        effective_import_chain_configs, import_create_plan, validate_import_create_request,
        ImportAttemptSeed, CLAIM_WATCHED_ADDRESS_IMPORT_QUERY, CREATE_WATCHED_ADDRESS_IMPORT_QUERY,
        INSERT_IMPORT_ATTEMPT_QUERY, MARK_IMPORT_ROW_FAILED_QUERY, MARK_IMPORT_ROW_SUCCESS_QUERY,
        PENDING_IMPORT_ROWS_QUERY, REFRESH_IMPORT_TASK_COUNTS_QUERY,
    };
    use coin_listener_core::models::{
        CreateWatchedAddressImportRequest, WatchedAddressImportChainConfig,
        WatchedAddressImportDefaults, WatchedAddressImportRowRequest,
    };
    use uuid::Uuid;

    fn request_with_rows(
        rows: Vec<WatchedAddressImportRowRequest>,
    ) -> CreateWatchedAddressImportRequest {
        request_with_defaults(
            WatchedAddressImportDefaults {
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
        )
    }

    fn request_with_defaults(
        defaults: WatchedAddressImportDefaults,
        rows: Vec<WatchedAddressImportRowRequest>,
    ) -> CreateWatchedAddressImportRequest {
        CreateWatchedAddressImportRequest { defaults, rows }
    }

    fn chain_config(chain_id: u128, asset_ids: Vec<u128>) -> WatchedAddressImportChainConfig {
        WatchedAddressImportChainConfig {
            chain_id: Uuid::from_u128(chain_id),
            asset_ids: asset_ids.into_iter().map(Uuid::from_u128).collect(),
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
    fn import_validation_derives_legacy_single_chain_config() {
        let request = request_with_rows(vec![row(1, "0x0000000000000000000000000000000000000001")]);

        validate_import_create_request(&request).unwrap();
        let configs = effective_import_chain_configs(&request.defaults);

        assert_eq!(configs, vec![chain_config(2, vec![101])]);
    }

    #[test]
    fn import_validation_rejects_empty_effective_assets() {
        let mut request =
            request_with_rows(vec![row(1, "0x0000000000000000000000000000000000000001")]);
        request.defaults.asset_ids = Vec::new();

        let error = validate_import_create_request(&request).unwrap_err();

        assert_eq!(
            error.to_string(),
            "validation error: asset_ids are required for every chain config"
        );
    }

    #[test]
    fn import_validation_rejects_empty_chain_config_assets() {
        let defaults = WatchedAddressImportDefaults {
            chain_id: Uuid::from_u128(2),
            asset_ids: vec![Uuid::from_u128(101)],
            chain_configs: vec![chain_config(2, Vec::new())],
            priority: "normal".to_string(),
            scan_interval_seconds: 300,
            transfer_filter_enabled: true,
            balance_change_filter_enabled: true,
            status: "active".to_string(),
        };
        let request = request_with_defaults(
            defaults,
            vec![row(1, "0x0000000000000000000000000000000000000001")],
        );

        let error = validate_import_create_request(&request).unwrap_err();

        assert_eq!(
            error.to_string(),
            "validation error: asset_ids are required for every chain config"
        );
    }

    #[test]
    fn import_validation_rejects_duplicate_chain_configs() {
        let defaults = WatchedAddressImportDefaults {
            chain_id: Uuid::from_u128(2),
            asset_ids: vec![Uuid::from_u128(101)],
            chain_configs: vec![chain_config(2, vec![101]), chain_config(2, vec![102])],
            priority: "normal".to_string(),
            scan_interval_seconds: 300,
            transfer_filter_enabled: true,
            balance_change_filter_enabled: true,
            status: "active".to_string(),
        };
        let request = request_with_defaults(
            defaults,
            vec![row(1, "0x0000000000000000000000000000000000000001")],
        );

        let error = validate_import_create_request(&request).unwrap_err();

        assert_eq!(
            error.to_string(),
            "validation error: chain_id must be unique within an import"
        );
    }

    #[test]
    fn multi_chain_create_plan_persists_configs_mirrors_first_config_and_expands_attempts() {
        let configs = vec![chain_config(2, vec![201]), chain_config(3, vec![301, 302])];
        let defaults = WatchedAddressImportDefaults {
            chain_id: Uuid::from_u128(99),
            asset_ids: vec![Uuid::from_u128(999)],
            chain_configs: configs.clone(),
            priority: "normal".to_string(),
            scan_interval_seconds: 300,
            transfer_filter_enabled: true,
            balance_change_filter_enabled: true,
            status: "active".to_string(),
        };
        let rows = vec![
            row(1, "0x0000000000000000000000000000000000000001"),
            row(2, "0x0000000000000000000000000000000000000002"),
        ];
        let request = request_with_defaults(defaults, rows);

        let plan = import_create_plan(&request).unwrap();

        assert_eq!(plan.chain_id, configs[0].chain_id);
        assert_eq!(plan.asset_ids, configs[0].asset_ids);
        assert_eq!(plan.chain_configs, configs);
        assert_eq!(plan.total_rows, 4);
        assert_eq!(
            plan.attempts,
            vec![
                ImportAttemptSeed {
                    row_number: 1,
                    chain_id: Uuid::from_u128(2),
                    asset_ids: vec![Uuid::from_u128(201)],
                },
                ImportAttemptSeed {
                    row_number: 1,
                    chain_id: Uuid::from_u128(3),
                    asset_ids: vec![Uuid::from_u128(301), Uuid::from_u128(302)],
                },
                ImportAttemptSeed {
                    row_number: 2,
                    chain_id: Uuid::from_u128(2),
                    asset_ids: vec![Uuid::from_u128(201)],
                },
                ImportAttemptSeed {
                    row_number: 2,
                    chain_id: Uuid::from_u128(3),
                    asset_ids: vec![Uuid::from_u128(301), Uuid::from_u128(302)],
                },
            ]
        );
    }

    #[test]
    fn legacy_empty_chain_configs_create_plan_derives_single_config_and_one_attempt_per_row() {
        let request = request_with_rows(vec![
            row(1, "0x0000000000000000000000000000000000000001"),
            row(2, "0x0000000000000000000000000000000000000002"),
        ]);

        let plan = import_create_plan(&request).unwrap();

        assert_eq!(plan.chain_configs, vec![chain_config(2, vec![101])]);
        assert_eq!(plan.chain_id, Uuid::from_u128(2));
        assert_eq!(plan.asset_ids, vec![Uuid::from_u128(101)]);
        assert_eq!(plan.total_rows, 2);
        assert_eq!(plan.attempts.len(), 2);
        assert!(plan
            .attempts
            .iter()
            .all(|attempt| attempt.chain_id == Uuid::from_u128(2)));
    }

    #[test]
    fn create_import_query_persists_chain_configs_and_attempt_total() {
        assert!(CREATE_WATCHED_ADDRESS_IMPORT_QUERY.contains("chain_configs"));
        assert!(CREATE_WATCHED_ADDRESS_IMPORT_QUERY.contains("total_rows"));
        assert!(CREATE_WATCHED_ADDRESS_IMPORT_QUERY
            .contains("VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"));
    }

    #[test]
    fn insert_attempt_query_uses_each_row_chain_pair() {
        assert!(INSERT_IMPORT_ATTEMPT_QUERY.contains("watched_address_import_attempts"));
        assert!(INSERT_IMPORT_ATTEMPT_QUERY.contains("row_number, chain_id, asset_ids"));
        assert!(INSERT_IMPORT_ATTEMPT_QUERY.contains("VALUES ($1, $2, $3, $4, $5)"));
    }

    #[test]
    fn multi_chain_import_migration_defines_attempt_storage() {
        let migration = include_str!("../migrations/0018_multi_chain_address_import_attempts.sql");

        assert!(migration
            .contains("ADD COLUMN IF NOT EXISTS chain_configs JSONB NOT NULL DEFAULT '[]'::jsonb"));
        assert!(migration.contains("CREATE TABLE IF NOT EXISTS watched_address_import_attempts"));
        assert!(migration.contains(
            "import_task_id UUID NOT NULL REFERENCES watched_address_import_tasks(id) ON DELETE CASCADE"
        ));
        assert!(migration.contains("asset_ids UUID[] NOT NULL"));
        assert!(migration.contains("status IN ('pending', 'success', 'failed', 'skipped')"));
        assert!(migration.contains("watched_address_import_attempts_unique_row_chain"));
        assert!(migration.contains("watched_address_import_attempts_source_row_fk"));
        assert!(migration.contains("idx_watched_address_import_attempts_task_status"));
        assert!(migration.contains("jsonb_build_array"));
        assert!(migration.contains("INSERT INTO watched_address_import_attempts"));
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
