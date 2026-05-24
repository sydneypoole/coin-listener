use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use coin_listener_core::{
    models::{
        AssignCustodyAccountRequest, AssignCustodyAccountResponse, CreateCustodyAccountRequest,
        CreateWatchedAddressRequest, CustodyAccount, CustodyAccountAssignment,
        CustodyAccountChainConfig, CustodyAccountChainConfigRequest, CustodyAccountQuery,
        CustodyAssignmentQuery, CustodyAssignmentWatchedAddress, WatchedAddress,
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
  AND (
      $2::uuid IS NULL
      OR EXISTS (
          SELECT 1
          FROM custody_account_chain_configs filter_config
          WHERE filter_config.tenant_id = ca.tenant_id
            AND filter_config.custody_account_id = ca.id
            AND filter_config.chain_id = $2
      )
      OR (
          NOT EXISTS (
              SELECT 1
              FROM custody_account_chain_configs fallback_config
              WHERE fallback_config.tenant_id = ca.tenant_id
                AND fallback_config.custody_account_id = ca.id
          )
          AND ca.chain_id = $2
      )
  )
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

pub const LIST_CUSTODY_ACCOUNT_CONFIGS_QUERY: &str = r#"
SELECT config.id,
       config.custody_account_id,
       config.chain_id,
       chain.name AS chain_name,
       COALESCE(array_agg(asset.id ORDER BY asset.symbol) FILTER (WHERE asset.id IS NOT NULL), ARRAY[]::uuid[]) AS asset_ids,
       COALESCE(array_agg(asset.symbol ORDER BY asset.symbol) FILTER (WHERE asset.id IS NOT NULL), ARRAY[]::text[]) AS asset_symbols
FROM custody_account_chain_configs config
JOIN chains chain ON chain.id = config.chain_id
LEFT JOIN custody_account_chain_config_assets config_asset ON config_asset.chain_config_id = config.id
LEFT JOIN assets asset ON asset.id = config_asset.asset_id
WHERE config.tenant_id = $1
  AND config.custody_account_id = ANY($2)
GROUP BY config.id, config.custody_account_id, config.chain_id, chain.name
ORDER BY chain.name ASC
"#;

pub const INSERT_CUSTODY_ACCOUNT_CHAIN_CONFIG_QUERY: &str = r#"
INSERT INTO custody_account_chain_configs (tenant_id, custody_account_id, chain_id)
VALUES ($1, $2, $3)
ON CONFLICT (tenant_id, custody_account_id, chain_id) DO UPDATE
SET updated_at = NOW()
RETURNING id
"#;

pub const INSERT_CUSTODY_ACCOUNT_CHAIN_CONFIG_ASSET_QUERY: &str = r#"
INSERT INTO custody_account_chain_config_assets (chain_config_id, asset_id)
VALUES ($1, $2)
ON CONFLICT DO NOTHING
"#;

pub const SELECT_CHAIN_TYPES_FOR_CONFIGS_QUERY: &str = r#"
SELECT id, chain_type
FROM chains
WHERE id = ANY($1)
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
  AND address_normalized = $2
FOR UPDATE
"#;

pub const INSERT_USER_CUSTODY_ACCOUNT_FOR_ASSIGNMENT_QUERY: &str = r#"
INSERT INTO custody_accounts (
    tenant_id, chain_id, address, address_normalized, label, source, status
)
VALUES ($1, $2, $3, $4, NULL, 'user', 'assigned')
ON CONFLICT (tenant_id, address_normalized) DO NOTHING
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
struct CustodyAccountListRow {
    id: Uuid,
    tenant_id: Uuid,
    chain_id: Uuid,
    chain_name: String,
    address: String,
    label: Option<String>,
    source: String,
    status: String,
    watched_address_id: Option<Uuid>,
    current_assignment_id: Option<Uuid>,
    current_business_ref: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct CustodyAccountChainConfigRow {
    id: Uuid,
    custody_account_id: Uuid,
    chain_id: Uuid,
    chain_name: String,
    asset_ids: Vec<Uuid>,
    asset_symbols: Vec<String>,
}

#[derive(Debug, FromRow)]
struct InsertedChainConfigRow {
    id: Uuid,
}

#[derive(Debug, FromRow)]
struct ChainTypeRow {
    id: Uuid,
    chain_type: String,
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

fn validate_custody_chain_config_shape(
    configs: &[CustodyAccountChainConfigRequest],
) -> AppResult<()> {
    if configs.is_empty() {
        return Err(AppError::Validation(
            "at least one custody chain config is required".to_string(),
        ));
    }

    let mut chain_ids = HashSet::new();
    for config in configs {
        if !chain_ids.insert(config.chain_id) {
            return Err(AppError::Validation(
                "custody chain configs cannot repeat chain_id".to_string(),
            ));
        }
        if config.asset_ids.is_empty() {
            return Err(AppError::Validation(
                "each custody chain config requires at least one asset".to_string(),
            ));
        }
        let mut asset_ids = HashSet::new();
        for asset_id in &config.asset_ids {
            if !asset_ids.insert(*asset_id) {
                return Err(AppError::Validation(
                    "custody chain config asset_ids cannot contain duplicates".to_string(),
                ));
            }
        }
    }

    Ok(())
}

fn normalize_custody_account_address(address: &str, chain_types: &[String]) -> String {
    let address = address.trim();
    if chain_types.iter().any(|chain_type| chain_type == "evm") {
        return address.to_lowercase();
    }
    address.to_string()
}

fn legacy_chain_id_for_create_request(request: &CreateCustodyAccountRequest) -> AppResult<Uuid> {
    if request
        .chain_configs
        .iter()
        .any(|config| config.chain_id == request.chain_id)
    {
        return Ok(request.chain_id);
    }
    Err(AppError::Validation(
        "legacy chain_id must be included in custody chain_configs".to_string(),
    ))
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

async fn chain_types_for_configs(
    transaction: &mut Transaction<'_, Postgres>,
    configs: &[CustodyAccountChainConfigRequest],
) -> AppResult<HashMap<Uuid, String>> {
    let chain_ids = configs
        .iter()
        .map(|config| config.chain_id)
        .collect::<Vec<_>>();
    let rows = sqlx::query_as::<_, ChainTypeRow>(SELECT_CHAIN_TYPES_FOR_CONFIGS_QUERY)
        .bind(&chain_ids)
        .fetch_all(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    let chain_types = rows
        .into_iter()
        .map(|row| (row.id, row.chain_type))
        .collect::<HashMap<_, _>>();
    if chain_types.len() != chain_ids.len() {
        return Err(AppError::Validation(
            "custody chain config contains unknown chain".to_string(),
        ));
    }
    Ok(chain_types)
}

async fn validate_and_insert_custody_chain_configs(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    custody_account_id: Uuid,
    configs: &[CustodyAccountChainConfigRequest],
) -> AppResult<()> {
    validate_custody_chain_config_shape(configs)?;

    for config in configs {
        repositories::validate_assets_for_chain(
            transaction.as_mut(),
            config.chain_id,
            &config.asset_ids,
        )
        .await?;
        let inserted =
            sqlx::query_as::<_, InsertedChainConfigRow>(INSERT_CUSTODY_ACCOUNT_CHAIN_CONFIG_QUERY)
                .bind(tenant_id)
                .bind(custody_account_id)
                .bind(config.chain_id)
                .fetch_one(transaction.as_mut())
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;

        for asset_id in &config.asset_ids {
            sqlx::query(INSERT_CUSTODY_ACCOUNT_CHAIN_CONFIG_ASSET_QUERY)
                .bind(inserted.id)
                .bind(asset_id)
                .execute(transaction.as_mut())
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
        }
    }

    Ok(())
}

async fn configs_for_accounts(
    pool: &PgPool,
    tenant_id: Uuid,
    account_ids: &[Uuid],
) -> AppResult<HashMap<Uuid, Vec<CustodyAccountChainConfig>>> {
    if account_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let rows =
        sqlx::query_as::<_, CustodyAccountChainConfigRow>(LIST_CUSTODY_ACCOUNT_CONFIGS_QUERY)
            .bind(tenant_id)
            .bind(account_ids)
            .fetch_all(pool)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;

    let mut grouped: HashMap<Uuid, Vec<CustodyAccountChainConfig>> = HashMap::new();
    for row in rows {
        grouped
            .entry(row.custody_account_id)
            .or_default()
            .push(CustodyAccountChainConfig {
                id: row.id,
                chain_id: row.chain_id,
                chain_name: row.chain_name,
                asset_ids: row.asset_ids,
                asset_symbols: row.asset_symbols,
            });
    }
    Ok(grouped)
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

    let rows = sqlx::query_as::<_, CustodyAccountListRow>(LIST_CUSTODY_ACCOUNTS_QUERY)
        .bind(tenant_id)
        .bind(query.chain_id)
        .bind(query.source)
        .bind(query.status)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let account_ids = rows.iter().map(|row| row.id).collect::<Vec<_>>();
    let mut config_map = configs_for_accounts(pool, tenant_id, &account_ids).await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let chain_configs = config_map.remove(&row.id).unwrap_or_default();
            custody_account_from_list_row(row, chain_configs)
        })
        .collect())
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
    validate_custody_chain_config_shape(&request.chain_configs)?;
    let status = request.status.clone().unwrap_or_else(|| {
        if request.source == CUSTODY_SOURCE_POOL {
            CUSTODY_ACCOUNT_STATUS_AVAILABLE.to_string()
        } else {
            CUSTODY_ACCOUNT_STATUS_ASSIGNED.to_string()
        }
    });
    validate_custody_account_status(&status)?;
    let address = request.address.trim().to_string();

    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    let chain_types = chain_types_for_configs(&mut transaction, &request.chain_configs).await?;
    for config in &request.chain_configs {
        let chain =
            repositories::get_chain_in_transaction(&mut transaction, config.chain_id).await?;
        repositories::validate_address_for_chain(&chain, &address)?;
    }
    let chain_type_values = chain_types.values().cloned().collect::<Vec<_>>();
    let normalized = normalize_custody_account_address(&address, &chain_type_values);
    let legacy_chain_id = legacy_chain_id_for_create_request(&request)?;

    let row = sqlx::query_as::<_, CustodyAccountRow>(INSERT_CUSTODY_ACCOUNT_QUERY)
        .bind(tenant_id)
        .bind(legacy_chain_id)
        .bind(&address)
        .bind(normalized)
        .bind(request.label)
        .bind(request.source)
        .bind(status)
        .fetch_one(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    let account_id = row.id;

    validate_and_insert_custody_chain_configs(
        &mut transaction,
        tenant_id,
        account_id,
        &request.chain_configs,
    )
    .await?;

    transaction
        .commit()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    list_custody_accounts(
        pool,
        tenant_id,
        CustodyAccountQuery {
            chain_id: Some(legacy_chain_id),
            source: None,
            status: None,
        },
    )
    .await?
    .into_iter()
    .find(|candidate| candidate.id == account_id)
    .ok_or_else(|| AppError::NotFound("custody account".to_string()))
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
    let chain_id = request.chain_id.ok_or_else(|| {
        AppError::Validation("chain_id is required for custody assignment".to_string())
    })?;
    let chain = repositories::get_chain(pool, chain_id).await?;
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
        chain_id: Some(chain_id),
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
            chain_id: Some(chain_id),
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
            chain_id: Some(chain_id),
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
        watched_addresses: vec![CustodyAssignmentWatchedAddress {
            chain_id,
            chain_name: chain.name,
            watched_address_id,
            asset_ids: vec![],
        }],
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
    let chain_id = request.chain_id.ok_or_else(|| {
        AppError::Validation("chain_id is required for custody assignment".to_string())
    })?;
    if request.source == CUSTODY_SOURCE_POOL {
        return sqlx::query_as::<_, CustodyAccountRow>(CLAIM_AVAILABLE_POOL_ACCOUNT_QUERY)
            .bind(tenant_id)
            .bind(chain_id)
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
            .bind(&normalized)
            .fetch_optional(transaction.as_mut())
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;

    if let Some(existing) = existing {
        validate_assignable_custody_account(transaction, tenant_id, &existing).await?;
        validate_legacy_assignment_chain(&existing, chain_id)?;
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
            .bind(chain_id)
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
            .bind(&normalized)
            .fetch_one(transaction.as_mut())
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
    validate_assignable_custody_account(transaction, tenant_id, &existing).await?;
    validate_legacy_assignment_chain(&existing, chain_id)?;
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
    let chain_id = request.chain_id.ok_or_else(|| {
        AppError::Validation("chain_id is required for custody assignment".to_string())
    })?;
    let chain_type = sqlx::query_scalar::<_, String>("SELECT chain_type FROM chains WHERE id = $1")
        .bind(chain_id)
        .fetch_one(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    let address = request.address.as_deref().ok_or_else(|| {
        AppError::Validation("address is required for user custody account".to_string())
    })?;
    Ok(normalize_custody_address_for_chain(&chain_type, address))
}

fn validate_legacy_assignment_chain(account: &CustodyAccountRow, chain_id: Uuid) -> AppResult<()> {
    if account.chain_id != chain_id {
        return Err(AppError::Validation(
            "custody account chain does not match assignment chain".to_string(),
        ));
    }
    Ok(())
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
    chain_configs: Vec<CustodyAccountChainConfig>,
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
        chain_configs,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn custody_account_from_list_row(
    row: CustodyAccountListRow,
    chain_configs: Vec<CustodyAccountChainConfig>,
) -> CustodyAccount {
    let chain_name = row.chain_name;
    let current_assignment_id = row.current_assignment_id;
    let current_business_ref = row.current_business_ref;
    custody_account_from_row(
        CustodyAccountRow {
            id: row.id,
            tenant_id: row.tenant_id,
            chain_id: row.chain_id,
            address: row.address,
            label: row.label,
            source: row.source,
            status: row.status,
            watched_address_id: row.watched_address_id,
            created_at: row.created_at,
            updated_at: row.updated_at,
        },
        chain_name,
        current_assignment_id,
        current_business_ref,
        chain_configs,
    )
    .expect("custody account row conversion should not fail")
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

        let chain_config_migration =
            include_str!("../migrations/0021_custody_account_chain_configs.sql");
        assert!(chain_config_migration
            .contains("CREATE TABLE IF NOT EXISTS custody_account_chain_configs"));
        assert!(chain_config_migration
            .contains("CREATE TABLE IF NOT EXISTS custody_account_chain_config_assets"));
        assert!(chain_config_migration.contains("UNIQUE (tenant_id, custody_account_id, chain_id)"));
        assert!(chain_config_migration.contains("idx_custody_accounts_tenant_address_normalized"));
        assert!(chain_config_migration.contains("INSERT INTO custody_account_chain_configs"));
        assert!(chain_config_migration.contains("INSERT INTO custody_account_chain_config_assets"));
    }

    #[test]
    fn custody_chain_config_validation_requires_unique_chains_and_assets() {
        use coin_listener_core::models::CustodyAccountChainConfigRequest;
        use uuid::Uuid;

        let chain_a = Uuid::from_u128(1);
        let asset_a = Uuid::from_u128(2);

        let empty_configs: Vec<CustodyAccountChainConfigRequest> = vec![];
        assert!(validate_custody_chain_config_shape(&empty_configs).is_err());

        let duplicate_chain = vec![
            CustodyAccountChainConfigRequest {
                chain_id: chain_a,
                asset_ids: vec![asset_a],
            },
            CustodyAccountChainConfigRequest {
                chain_id: chain_a,
                asset_ids: vec![Uuid::from_u128(3)],
            },
        ];
        assert!(validate_custody_chain_config_shape(&duplicate_chain).is_err());

        let empty_assets = vec![CustodyAccountChainConfigRequest {
            chain_id: chain_a,
            asset_ids: vec![],
        }];
        assert!(validate_custody_chain_config_shape(&empty_assets).is_err());

        let duplicate_assets = vec![CustodyAccountChainConfigRequest {
            chain_id: chain_a,
            asset_ids: vec![asset_a, asset_a],
        }];
        assert!(validate_custody_chain_config_shape(&duplicate_assets).is_err());

        let valid = vec![CustodyAccountChainConfigRequest {
            chain_id: chain_a,
            asset_ids: vec![asset_a],
        }];
        assert!(validate_custody_chain_config_shape(&valid).is_ok());
    }

    #[test]
    fn custody_chain_config_queries_join_assets_and_support_account_level_uniqueness() {
        assert!(LIST_CUSTODY_ACCOUNT_CONFIGS_QUERY.contains("custody_account_chain_configs"));
        assert!(LIST_CUSTODY_ACCOUNT_CONFIGS_QUERY.contains("custody_account_chain_config_assets"));
        assert!(LIST_CUSTODY_ACCOUNT_CONFIGS_QUERY.contains("array_agg"));
        assert!(INSERT_CUSTODY_ACCOUNT_CHAIN_CONFIG_QUERY.contains("custody_account_chain_configs"));
        assert!(INSERT_CUSTODY_ACCOUNT_CHAIN_CONFIG_ASSET_QUERY
            .contains("custody_account_chain_config_assets"));
        assert!(INSERT_USER_CUSTODY_ACCOUNT_FOR_ASSIGNMENT_QUERY
            .contains("ON CONFLICT (tenant_id, address_normalized) DO NOTHING"));
        assert!(SELECT_USER_CUSTODY_ACCOUNT_FOR_UPDATE_QUERY.contains("address_normalized = $2"));
        assert!(LIST_CUSTODY_ACCOUNTS_QUERY.contains("custody_account_chain_configs filter_config"));
        assert!(LIST_CUSTODY_ACCOUNTS_QUERY.contains("filter_config.chain_id = $2"));
        assert!(LIST_CUSTODY_ACCOUNTS_QUERY.contains("fallback_config"));
        assert!(LIST_CUSTODY_ACCOUNTS_QUERY.contains("AND ca.chain_id = $2"));
        assert!(CLAIM_AVAILABLE_POOL_ACCOUNT_QUERY.contains("source = 'pool'"));
        assert!(CLAIM_AVAILABLE_POOL_ACCOUNT_QUERY.contains("chain_id = $2"));
    }

    #[test]
    fn create_custody_account_validates_chains_without_pool_reentry() {
        let source = include_str!("custody_accounts.rs");
        let create_body = source
            .split("pub async fn create_custody_account")
            .nth(1)
            .expect("create_custody_account should exist")
            .split("pub async fn assign_custody_account")
            .next()
            .expect("assign_custody_account should follow create_custody_account");

        assert!(create_body.contains("repositories::get_chain_in_transaction"));
        assert!(!create_body.contains("repositories::get_chain(pool"));
    }

    #[test]
    fn create_request_legacy_chain_must_exist_in_configs() {
        let chain_a = Uuid::from_u128(20);
        let chain_b = Uuid::from_u128(21);
        let asset = Uuid::from_u128(22);
        let mut request = CreateCustodyAccountRequest {
            chain_id: chain_b,
            address: "0x0000000000000000000000000000000000000001".to_string(),
            label: None,
            source: CUSTODY_SOURCE_POOL.to_string(),
            status: Some(CUSTODY_ACCOUNT_STATUS_AVAILABLE.to_string()),
            chain_configs: vec![
                CustodyAccountChainConfigRequest {
                    chain_id: chain_a,
                    asset_ids: vec![asset],
                },
                CustodyAccountChainConfigRequest {
                    chain_id: chain_b,
                    asset_ids: vec![asset],
                },
            ],
        };

        assert_eq!(
            legacy_chain_id_for_create_request(&request).unwrap(),
            chain_b
        );

        request.chain_id = Uuid::from_u128(23);
        assert!(legacy_chain_id_for_create_request(&request).is_err());
    }

    #[test]
    fn existing_user_assignment_rejects_legacy_chain_mismatch_until_multi_chain_assignment() {
        let now = chrono::Utc::now();
        let account = CustodyAccountRow {
            id: Uuid::from_u128(10),
            tenant_id: Uuid::from_u128(11),
            chain_id: Uuid::from_u128(12),
            address: "0x0000000000000000000000000000000000000001".to_string(),
            label: None,
            source: CUSTODY_SOURCE_USER.to_string(),
            status: CUSTODY_ACCOUNT_STATUS_ASSIGNED.to_string(),
            watched_address_id: None,
            created_at: now,
            updated_at: now,
        };

        assert!(validate_legacy_assignment_chain(&account, Uuid::from_u128(12)).is_ok());
        assert!(validate_legacy_assignment_chain(&account, Uuid::from_u128(13)).is_err());
    }

    #[test]
    fn existing_user_assignment_wires_legacy_chain_guard() {
        let source = include_str!("custody_accounts.rs");
        let assignment_body = source
            .split("async fn claim_or_create_account_for_assignment")
            .nth(1)
            .expect("claim_or_create_account_for_assignment should exist")
            .split("async fn custody_address_normalized_for_request")
            .next()
            .expect("custody_address_normalized_for_request should follow assignment selection");

        assert_eq!(
            assignment_body
                .matches("validate_legacy_assignment_chain(&existing, chain_id)?")
                .count(),
            2
        );
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
            .contains("ON CONFLICT (tenant_id, address_normalized) DO NOTHING"));
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
