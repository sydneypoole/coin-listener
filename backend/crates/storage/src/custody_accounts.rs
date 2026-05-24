use chrono::{DateTime, Utc};
use coin_listener_core::{
    models::{
        AssignCustodyAccountRequest, AssignCustodyAccountResponse, CreateCustodyAccountRequest,
        CreateWatchedAddressRequest, CustodyAccount, CustodyAccountAssignment, CustodyAccountQuery,
        CustodyAssignmentQuery, WatchedAddress,
    },
    AppError, AppResult,
};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::repositories;

pub const CUSTODY_SOURCE_POOL: &str = "pool";
pub const CUSTODY_SOURCE_USER: &str = "user";
pub const CUSTODY_ACCOUNT_STATUS_AVAILABLE: &str = "available";
pub const CUSTODY_ACCOUNT_STATUS_ASSIGNED: &str = "assigned";
pub const CUSTODY_ACCOUNT_STATUS_DISABLED: &str = "disabled";
pub const CUSTODY_ASSIGNMENT_STATUS_ACTIVE: &str = "active";
pub const CUSTODY_ASSIGNMENT_STATUS_RELEASED: &str = "released";
pub const CUSTODY_ASSIGNMENT_STATUS_CANCELLED: &str = "cancelled";
pub const CUSTODY_APPLICANT_TYPE_API: &str = "api";
pub const CUSTODY_APPLICANT_TYPE_INTERNAL: &str = "internal";

pub const LIST_CUSTODY_ACCOUNTS_QUERY: &str = r#"
SELECT ca.id,
       ca.tenant_id,
       ca.chain_id,
       c.name AS chain_name,
       ca.address,
       ca.label,
       ca.source,
       ca.status,
       ca.watched_address_id,
       active_assignment.id AS current_assignment_id,
       active_assignment.business_ref AS current_business_ref,
       ca.created_at,
       ca.updated_at
FROM custody_accounts ca
JOIN chains c ON c.id = ca.chain_id
LEFT JOIN custody_account_assignments active_assignment ON active_assignment.custody_account_id = ca.id
    AND active_assignment.status = 'active'
WHERE ca.tenant_id = $1
  AND ($2::uuid IS NULL OR ca.chain_id = $2)
  AND ($3::text IS NULL OR ca.source = $3)
  AND ($4::text IS NULL OR ca.status = $4)
ORDER BY ca.created_at DESC
"#;

pub const LIST_CUSTODY_ASSIGNMENTS_QUERY: &str = r#"
SELECT assignment.id,
       assignment.tenant_id,
       assignment.custody_account_id,
       account.chain_id,
       chain.name AS chain_name,
       account.address,
       assignment.applicant_type,
       assignment.business_ref,
       assignment.purpose,
       assignment.status,
       assignment.watched_address_id,
       assignment.assigned_at,
       assignment.released_at,
       assignment.created_at,
       assignment.updated_at
FROM custody_account_assignments assignment
JOIN custody_accounts account ON account.id = assignment.custody_account_id
    AND account.tenant_id = assignment.tenant_id
JOIN chains chain ON chain.id = account.chain_id
WHERE assignment.tenant_id = $1
  AND ($2::uuid IS NULL OR account.chain_id = $2)
  AND ($3::text IS NULL OR assignment.status = $3)
  AND ($4::text IS NULL OR assignment.business_ref = $4)
ORDER BY assignment.assigned_at DESC
"#;

pub const INSERT_CUSTODY_ACCOUNT_QUERY: &str = r#"
INSERT INTO custody_accounts (
    tenant_id, chain_id, address, address_normalized, label, source, status
)
VALUES ($1, $2, $3, $4, $5, $6, $7)
RETURNING id, tenant_id, chain_id, address, label, source, status, watched_address_id, created_at, updated_at
"#;

pub const CLAIM_AVAILABLE_POOL_ACCOUNT_QUERY: &str = r#"
SELECT id, tenant_id, chain_id, address, label, source, status, watched_address_id, created_at, updated_at
FROM custody_accounts
WHERE tenant_id = $1
  AND chain_id = $2
  AND source = 'pool'
  AND status = 'available'
ORDER BY created_at ASC
FOR UPDATE SKIP LOCKED
LIMIT 1
"#;

pub const SELECT_USER_CUSTODY_ACCOUNT_FOR_UPDATE_QUERY: &str = r#"
SELECT id, tenant_id, chain_id, address, label, source, status, watched_address_id, created_at, updated_at
FROM custody_accounts
WHERE tenant_id = $1
  AND chain_id = $2
  AND address_normalized = $3
FOR UPDATE
"#;

pub const INSERT_USER_CUSTODY_ACCOUNT_FOR_ASSIGNMENT_QUERY: &str = r#"
INSERT INTO custody_accounts (
    tenant_id, chain_id, address, address_normalized, label, source, status
)
VALUES ($1, $2, $3, $4, NULL, 'user', 'assigned')
ON CONFLICT (tenant_id, chain_id, address_normalized) DO NOTHING
RETURNING id, tenant_id, chain_id, address, label, source, status, watched_address_id, created_at, updated_at
"#;

pub const SELECT_ACTIVE_CUSTODY_ASSIGNMENT_FOR_ACCOUNT_QUERY: &str = r#"
SELECT id
FROM custody_account_assignments
WHERE tenant_id = $1
  AND custody_account_id = $2
  AND status = 'active'
LIMIT 1
"#;

pub const INSERT_CUSTODY_ASSIGNMENT_QUERY: &str = r#"
INSERT INTO custody_account_assignments (
    tenant_id, custody_account_id, applicant_type, business_ref, purpose, status, watched_address_id
)
VALUES ($1, $2, $3, $4, $5, 'active', $6)
ON CONFLICT DO NOTHING
RETURNING id
"#;

pub const MARK_CUSTODY_ACCOUNT_ASSIGNED_QUERY: &str = r#"
UPDATE custody_accounts
SET status = 'assigned',
    watched_address_id = $2,
    updated_at = NOW()
WHERE id = $1
  AND tenant_id = $3
"#;

pub const ENSURE_WATCHED_ADDRESS_SELECT_QUERY: &str = r#"
SELECT id, tenant_id, chain_id, address, label, priority, scan_interval_seconds,
       transfer_filter_enabled, balance_change_filter_enabled, status
FROM watched_addresses
WHERE tenant_id = $1
  AND chain_id = $2
  AND (
    (lower($4::text) = 'evm' AND lower(address) = $3)
    OR (lower($4::text) <> 'evm' AND address = $3)
  )
LIMIT 1
"#;

pub const ENSURE_WATCHED_ADDRESS_UPDATE_QUERY: &str = r#"
UPDATE watched_addresses
SET status = 'active',
    updated_at = NOW()
WHERE id = $1
  AND tenant_id = $2
RETURNING id, tenant_id, chain_id, address, label, priority, scan_interval_seconds,
          transfer_filter_enabled, balance_change_filter_enabled, status
"#;

pub const ENSURE_WATCHED_ADDRESS_ASSET_QUERY: &str = r#"
INSERT INTO watched_address_assets (address_id, asset_id)
VALUES ($1, $2)
ON CONFLICT DO NOTHING
"#;

pub const RELEASE_CUSTODY_ASSIGNMENT_QUERY: &str = r#"
UPDATE custody_account_assignments
SET status = 'released',
    released_at = NOW(),
    updated_at = NOW()
WHERE id = $1
  AND tenant_id = $2
  AND status = 'active'
RETURNING custody_account_id
"#;

pub const MARK_RELEASED_POOL_ACCOUNT_AVAILABLE_QUERY: &str = r#"
UPDATE custody_accounts
SET status = CASE WHEN source = 'pool' THEN 'available' ELSE status END,
    updated_at = NOW()
WHERE id = $1
  AND tenant_id = $2
"#;

#[derive(Debug, FromRow)]
struct CustodyAccountRow {
    id: Uuid,
    tenant_id: Uuid,
    chain_id: Uuid,
    address: String,
    label: Option<String>,
    source: String,
    status: String,
    watched_address_id: Option<Uuid>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct InsertedAssignmentRow {
    id: Uuid,
}

#[derive(Debug, FromRow)]
struct ActiveAssignmentRow {
    id: Uuid,
}

#[derive(Debug, FromRow)]
struct ReleasedAssignmentRow {
    custody_account_id: Uuid,
}

pub fn normalize_custody_address_for_chain(chain_type: &str, address: &str) -> String {
    let address = address.trim();
    if chain_type == "evm" {
        return address.to_lowercase();
    }
    address.to_string()
}

pub fn normalize_custody_address(address: &str) -> String {
    normalize_custody_address_for_chain("evm", address)
}

pub fn validate_custody_source(source: &str) -> AppResult<()> {
    if !matches!(source, CUSTODY_SOURCE_POOL | CUSTODY_SOURCE_USER) {
        return Err(AppError::Validation(
            "custody source must be pool or user".to_string(),
        ));
    }
    Ok(())
}

pub fn validate_custody_account_status(status: &str) -> AppResult<()> {
    if !matches!(
        status,
        CUSTODY_ACCOUNT_STATUS_AVAILABLE
            | CUSTODY_ACCOUNT_STATUS_ASSIGNED
            | CUSTODY_ACCOUNT_STATUS_DISABLED
    ) {
        return Err(AppError::Validation(
            "custody account status must be available, assigned, or disabled".to_string(),
        ));
    }
    Ok(())
}

pub fn validate_custody_applicant_type(applicant_type: &str) -> AppResult<()> {
    if !matches!(
        applicant_type,
        CUSTODY_APPLICANT_TYPE_API | CUSTODY_APPLICANT_TYPE_INTERNAL
    ) {
        return Err(AppError::Validation(
            "custody applicant_type must be api or internal".to_string(),
        ));
    }
    Ok(())
}

fn validate_custody_assignment_status(status: &str) -> AppResult<()> {
    if !matches!(
        status,
        CUSTODY_ASSIGNMENT_STATUS_ACTIVE
            | CUSTODY_ASSIGNMENT_STATUS_RELEASED
            | CUSTODY_ASSIGNMENT_STATUS_CANCELLED
    ) {
        return Err(AppError::Validation(
            "custody assignment status must be active, released, or cancelled".to_string(),
        ));
    }
    Ok(())
}

fn validate_business_ref(business_ref: &str) -> AppResult<()> {
    if business_ref.trim().is_empty() {
        return Err(AppError::Validation("business_ref is required".to_string()));
    }
    Ok(())
}

pub async fn list_custody_accounts(
    pool: &PgPool,
    tenant_id: Uuid,
    query: CustodyAccountQuery,
) -> AppResult<Vec<CustodyAccount>> {
    if let Some(source) = query.source.as_deref() {
        validate_custody_source(source)?;
    }
    if let Some(status) = query.status.as_deref() {
        validate_custody_account_status(status)?;
    }

    sqlx::query_as::<_, CustodyAccount>(LIST_CUSTODY_ACCOUNTS_QUERY)
        .bind(tenant_id)
        .bind(query.chain_id)
        .bind(query.source)
        .bind(query.status)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn list_custody_assignments(
    pool: &PgPool,
    tenant_id: Uuid,
    query: CustodyAssignmentQuery,
) -> AppResult<Vec<CustodyAccountAssignment>> {
    if let Some(status) = query.status.as_deref() {
        validate_custody_assignment_status(status)?;
    }

    sqlx::query_as::<_, CustodyAccountAssignment>(LIST_CUSTODY_ASSIGNMENTS_QUERY)
        .bind(tenant_id)
        .bind(query.chain_id)
        .bind(query.status)
        .bind(query.business_ref)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn create_custody_account(
    pool: &PgPool,
    tenant_id: Uuid,
    request: CreateCustodyAccountRequest,
) -> AppResult<CustodyAccount> {
    validate_custody_source(&request.source)?;
    let status = request.status.unwrap_or_else(|| {
        if request.source == CUSTODY_SOURCE_POOL {
            CUSTODY_ACCOUNT_STATUS_AVAILABLE.to_string()
        } else {
            CUSTODY_ACCOUNT_STATUS_ASSIGNED.to_string()
        }
    });
    validate_custody_account_status(&status)?;
    let chain = repositories::get_chain(pool, request.chain_id).await?;
    let address = request.address.trim().to_string();
    repositories::validate_address_for_chain(&chain, &address)?;
    let normalized = normalize_custody_address_for_chain(&chain.chain_type, &address);

    let row = sqlx::query_as::<_, CustodyAccountRow>(INSERT_CUSTODY_ACCOUNT_QUERY)
        .bind(tenant_id)
        .bind(request.chain_id)
        .bind(address)
        .bind(normalized)
        .bind(request.label)
        .bind(request.source)
        .bind(status)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    custody_account_from_row(row, chain.name, None, None)
}

pub async fn assign_custody_account(
    pool: &PgPool,
    tenant_id: Uuid,
    request: AssignCustodyAccountRequest,
) -> AppResult<AssignCustodyAccountResponse> {
    validate_custody_source(&request.source)?;
    validate_custody_applicant_type(&request.applicant_type)?;
    let business_ref = request.business_ref.trim().to_string();
    validate_business_ref(&business_ref)?;
    let chain = repositories::get_chain(pool, request.chain_id).await?;
    let address = request
        .address
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if let Some(address) = address.as_deref() {
        repositories::validate_address_for_chain(&chain, address)?;
    }
    let request = AssignCustodyAccountRequest {
        business_ref,
        address,
        ..request
    };

    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let account =
        claim_or_create_account_for_assignment(&mut transaction, tenant_id, &request).await?;
    let account_id = account.id;
    let watched_address_id = ensure_watched_address_for_custody_account(
        &mut transaction,
        tenant_id,
        account.chain_id,
        &account.address,
        &request.business_ref,
    )
    .await?;

    let assignment_row =
        sqlx::query_as::<_, InsertedAssignmentRow>(INSERT_CUSTODY_ASSIGNMENT_QUERY)
            .bind(tenant_id)
            .bind(account.id)
            .bind(&request.applicant_type)
            .bind(&request.business_ref)
            .bind(&request.purpose)
            .bind(watched_address_id)
            .fetch_optional(transaction.as_mut())
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .ok_or_else(|| {
                AppError::Validation(
                    "custody assignment conflicts with existing active assignment or business_ref"
                        .to_string(),
                )
            })?;

    sqlx::query(MARK_CUSTODY_ACCOUNT_ASSIGNED_QUERY)
        .bind(account.id)
        .bind(watched_address_id)
        .bind(tenant_id)
        .execute(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    transaction
        .commit()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let account = list_custody_accounts(
        pool,
        tenant_id,
        CustodyAccountQuery {
            chain_id: Some(request.chain_id),
            source: None,
            status: None,
        },
    )
    .await?
    .into_iter()
    .find(|candidate| candidate.id == account_id)
    .ok_or_else(|| AppError::NotFound("custody account".to_string()))?;

    let assignment = list_custody_assignments(
        pool,
        tenant_id,
        CustodyAssignmentQuery {
            chain_id: Some(request.chain_id),
            status: Some(CUSTODY_ASSIGNMENT_STATUS_ACTIVE.to_string()),
            business_ref: Some(request.business_ref),
        },
    )
    .await?
    .into_iter()
    .find(|assignment| assignment.id == assignment_row.id)
    .ok_or_else(|| AppError::NotFound("custody assignment".to_string()))?;

    Ok(AssignCustodyAccountResponse {
        account,
        assignment,
    })
}

pub async fn release_custody_assignment(pool: &PgPool, tenant_id: Uuid, id: Uuid) -> AppResult<()> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let released = sqlx::query_as::<_, ReleasedAssignmentRow>(RELEASE_CUSTODY_ASSIGNMENT_QUERY)
        .bind(id)
        .bind(tenant_id)
        .fetch_optional(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("custody assignment".to_string()))?;

    sqlx::query(MARK_RELEASED_POOL_ACCOUNT_AVAILABLE_QUERY)
        .bind(released.custody_account_id)
        .bind(tenant_id)
        .execute(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    transaction
        .commit()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    Ok(())
}

async fn claim_or_create_account_for_assignment(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    request: &AssignCustodyAccountRequest,
) -> AppResult<CustodyAccountRow> {
    if request.source == CUSTODY_SOURCE_POOL {
        return sqlx::query_as::<_, CustodyAccountRow>(CLAIM_AVAILABLE_POOL_ACCOUNT_QUERY)
            .bind(tenant_id)
            .bind(request.chain_id)
            .fetch_optional(transaction.as_mut())
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .ok_or_else(|| AppError::Validation("no available custody account".to_string()));
    }

    let address = request.address.as_deref().ok_or_else(|| {
        AppError::Validation("address is required for user custody account".to_string())
    })?;
    let normalized = custody_address_normalized_for_request(transaction, request).await?;
    let existing =
        sqlx::query_as::<_, CustodyAccountRow>(SELECT_USER_CUSTODY_ACCOUNT_FOR_UPDATE_QUERY)
            .bind(tenant_id)
            .bind(request.chain_id)
            .bind(&normalized)
            .fetch_optional(transaction.as_mut())
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;

    if let Some(existing) = existing {
        validate_assignable_custody_account(transaction, tenant_id, &existing).await?;
        if existing.source != CUSTODY_SOURCE_USER {
            return Err(AppError::Validation(
                "custody account source does not match user request".to_string(),
            ));
        }
        return Ok(existing);
    }

    let inserted =
        sqlx::query_as::<_, CustodyAccountRow>(INSERT_USER_CUSTODY_ACCOUNT_FOR_ASSIGNMENT_QUERY)
            .bind(tenant_id)
            .bind(request.chain_id)
            .bind(address)
            .bind(&normalized)
            .fetch_optional(transaction.as_mut())
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;

    if let Some(inserted) = inserted {
        return Ok(inserted);
    }

    let existing =
        sqlx::query_as::<_, CustodyAccountRow>(SELECT_USER_CUSTODY_ACCOUNT_FOR_UPDATE_QUERY)
            .bind(tenant_id)
            .bind(request.chain_id)
            .bind(&normalized)
            .fetch_one(transaction.as_mut())
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
    validate_assignable_custody_account(transaction, tenant_id, &existing).await?;
    if existing.source != CUSTODY_SOURCE_USER {
        return Err(AppError::Validation(
            "custody account source does not match user request".to_string(),
        ));
    }
    Ok(existing)
}

async fn custody_address_normalized_for_request(
    transaction: &mut Transaction<'_, Postgres>,
    request: &AssignCustodyAccountRequest,
) -> AppResult<String> {
    let chain_type = sqlx::query_scalar::<_, String>("SELECT chain_type FROM chains WHERE id = $1")
        .bind(request.chain_id)
        .fetch_one(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    let address = request.address.as_deref().ok_or_else(|| {
        AppError::Validation("address is required for user custody account".to_string())
    })?;
    Ok(normalize_custody_address_for_chain(&chain_type, address))
}

async fn validate_assignable_custody_account(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    account: &CustodyAccountRow,
) -> AppResult<()> {
    if account.status == CUSTODY_ACCOUNT_STATUS_DISABLED {
        return Err(AppError::Validation(
            "custody account is disabled".to_string(),
        ));
    }

    let active = sqlx::query_as::<_, ActiveAssignmentRow>(
        SELECT_ACTIVE_CUSTODY_ASSIGNMENT_FOR_ACCOUNT_QUERY,
    )
    .bind(tenant_id)
    .bind(account.id)
    .fetch_optional(transaction.as_mut())
    .await
    .map_err(|error| AppError::Database(error.to_string()))?;
    if let Some(active) = active {
        return Err(AppError::Validation(format!(
            "custody account already has active assignment {}",
            active.id
        )));
    }

    Ok(())
}

async fn ensure_watched_address_for_custody_account(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    chain_id: Uuid,
    address: &str,
    business_ref: &str,
) -> AppResult<Uuid> {
    let (normalized, chain_type) =
        custody_address_normalized_for_chain_id(transaction, chain_id, address).await?;
    let existing = sqlx::query_as::<_, WatchedAddress>(ENSURE_WATCHED_ADDRESS_SELECT_QUERY)
        .bind(tenant_id)
        .bind(chain_id)
        .bind(&normalized)
        .bind(&chain_type)
        .fetch_optional(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    let asset_id = native_asset_id_for_chain(transaction, chain_id).await?;

    if let Some(existing) = existing {
        if existing.status == "active" {
            ensure_watched_address_asset(transaction, existing.id, asset_id).await?;
            return Ok(existing.id);
        }
        let active = sqlx::query_as::<_, WatchedAddress>(ENSURE_WATCHED_ADDRESS_UPDATE_QUERY)
            .bind(existing.id)
            .bind(tenant_id)
            .fetch_one(transaction.as_mut())
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        ensure_watched_address_asset(transaction, active.id, asset_id).await?;
        return Ok(active.id);
    }

    let request = CreateWatchedAddressRequest {
        tenant_id: Some(tenant_id),
        chain_id,
        address: address.to_string(),
        label: Some(format!("托管地址 / {business_ref}")),
        priority: "normal".to_string(),
        scan_interval_seconds: 60,
        transfer_filter_enabled: true,
        balance_change_filter_enabled: true,
        status: "active".to_string(),
        asset_ids: vec![asset_id],
    };
    let watched =
        repositories::create_watched_address_in_transaction(transaction, request, vec![asset_id])
            .await?;
    Ok(watched.id)
}

async fn custody_address_normalized_for_chain_id(
    transaction: &mut Transaction<'_, Postgres>,
    chain_id: Uuid,
    address: &str,
) -> AppResult<(String, String)> {
    let chain_type = sqlx::query_scalar::<_, String>("SELECT chain_type FROM chains WHERE id = $1")
        .bind(chain_id)
        .fetch_one(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    let normalized = normalize_custody_address_for_chain(&chain_type, address);
    Ok((normalized, chain_type))
}

async fn ensure_watched_address_asset(
    transaction: &mut Transaction<'_, Postgres>,
    watched_address_id: Uuid,
    asset_id: Uuid,
) -> AppResult<()> {
    sqlx::query(ENSURE_WATCHED_ADDRESS_ASSET_QUERY)
        .bind(watched_address_id)
        .bind(asset_id)
        .execute(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    Ok(())
}

async fn native_asset_id_for_chain(
    transaction: &mut Transaction<'_, Postgres>,
    chain_id: Uuid,
) -> AppResult<Uuid> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM assets WHERE chain_id = $1 AND asset_type = 'native' AND status = 'active' LIMIT 1",
    )
    .bind(chain_id)
    .fetch_optional(transaction.as_mut())
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("native asset".to_string()))
}

fn custody_account_from_row(
    row: CustodyAccountRow,
    chain_name: String,
    current_assignment_id: Option<Uuid>,
    current_business_ref: Option<String>,
) -> AppResult<CustodyAccount> {
    Ok(CustodyAccount {
        id: row.id,
        tenant_id: row.tenant_id,
        chain_id: row.chain_id,
        chain_name,
        address: row.address,
        label: row.label,
        source: row.source,
        status: row.status,
        watched_address_id: row.watched_address_id,
        current_assignment_id,
        current_business_ref,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custody_migration_defines_unique_accounts_and_single_active_assignment() {
        let migration = include_str!("../migrations/0020_custody_accounts.sql");

        assert!(migration.contains("CREATE TABLE IF NOT EXISTS custody_accounts"));
        assert!(migration.contains("address_normalized TEXT NOT NULL"));
        assert!(migration.contains("UNIQUE (tenant_id, chain_id, address_normalized)"));
        assert!(migration.contains("UNIQUE (id, tenant_id)"));
        assert!(migration.contains("CREATE TABLE IF NOT EXISTS custody_account_assignments"));
        assert!(migration.contains("UNIQUE (tenant_id, business_ref)"));
        assert!(migration.contains("FOREIGN KEY (custody_account_id, tenant_id)"));
        assert!(migration.contains("idx_custody_assignments_one_active"));
        assert!(migration.contains("WHERE status = 'active'"));
    }

    #[test]
    fn custody_status_and_source_validation_is_explicit() {
        assert!(validate_custody_source("pool").is_ok());
        assert!(validate_custody_source("user").is_ok());
        assert!(validate_custody_source("other").is_err());
        assert!(validate_custody_account_status("available").is_ok());
        assert!(validate_custody_account_status("assigned").is_ok());
        assert!(validate_custody_account_status("disabled").is_ok());
        assert!(validate_custody_account_status("deleted").is_err());
        assert!(validate_custody_applicant_type("api").is_ok());
        assert!(validate_custody_applicant_type("internal").is_ok());
        assert!(validate_custody_applicant_type("external").is_err());
    }

    #[test]
    fn custody_queries_are_tenant_scoped_and_concurrency_safe() {
        assert!(LIST_CUSTODY_ACCOUNTS_QUERY.contains("ca.tenant_id = $1"));
        assert!(INSERT_CUSTODY_ACCOUNT_QUERY.contains("address_normalized"));
        assert!(CLAIM_AVAILABLE_POOL_ACCOUNT_QUERY.contains("FOR UPDATE SKIP LOCKED"));
        assert!(CLAIM_AVAILABLE_POOL_ACCOUNT_QUERY.contains("status = 'available'"));
        assert!(INSERT_CUSTODY_ASSIGNMENT_QUERY.contains("custody_account_assignments"));
        assert!(INSERT_CUSTODY_ASSIGNMENT_QUERY.contains("'active'"));
        assert!(INSERT_CUSTODY_ASSIGNMENT_QUERY.contains("ON CONFLICT DO NOTHING"));
        assert!(INSERT_USER_CUSTODY_ACCOUNT_FOR_ASSIGNMENT_QUERY
            .contains("ON CONFLICT (tenant_id, chain_id, address_normalized) DO NOTHING"));
        assert!(SELECT_ACTIVE_CUSTODY_ASSIGNMENT_FOR_ACCOUNT_QUERY.contains("status = 'active'"));
        assert!(ENSURE_WATCHED_ADDRESS_SELECT_QUERY.contains("tenant_id = $1"));
        assert!(ENSURE_WATCHED_ADDRESS_SELECT_QUERY.contains("chain_id = $2"));
        assert!(ENSURE_WATCHED_ADDRESS_SELECT_QUERY.contains("lower(address) = $3"));
        assert!(ENSURE_WATCHED_ADDRESS_SELECT_QUERY.contains("address = $3"));
        assert!(ENSURE_WATCHED_ADDRESS_UPDATE_QUERY.contains("status = 'active'"));
        assert!(ENSURE_WATCHED_ADDRESS_ASSET_QUERY.contains("ON CONFLICT DO NOTHING"));
        assert!(RELEASE_CUSTODY_ASSIGNMENT_QUERY.contains("status = 'released'"));
    }

    #[test]
    fn custody_address_normalization_is_chain_aware() {
        assert_eq!(
            normalize_custody_address_for_chain(
                "evm",
                "  0xABCDEF0000000000000000000000000000000001  "
            ),
            "0xabcdef0000000000000000000000000000000001"
        );
        assert_eq!(
            normalize_custody_address_for_chain("tron", "  TABcDEF0000000000000000000000000000  "),
            "TABcDEF0000000000000000000000000000"
        );
        assert_eq!(
            normalize_custody_address_for_chain("utxo", "  bc1QMixedCaseExample  "),
            "bc1QMixedCaseExample"
        );
    }
}
