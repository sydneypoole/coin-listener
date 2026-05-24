# Custody Account Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a custody account mode that supports pooled and user-provided blockchain addresses, allocates them through API/internal business requests, prevents duplicate active address assignment, and automatically enables watched-address monitoring.

**Architecture:** Add a focused custody storage module backed by `custody_accounts` and `custody_account_assignments`. Allocation runs in a PostgreSQL transaction using row locks and partial unique indexes, then ensures a matching `watched_addresses` row exists and is active. Expose tenant-scoped Axum APIs and a React control-plane page without adding private-key custody, signing, withdrawals, collection, or address generation.

**Tech Stack:** Rust 2021, Axum, SQLx, PostgreSQL, React, TypeScript, TanStack Query, Semi UI.

---

## File Structure

- Create: `backend/crates/storage/migrations/0020_custody_accounts.sql`
  - Defines custody account and assignment tables, status checks, tenant/chain/address uniqueness, and one-active-assignment constraint.
- Create: `backend/crates/storage/src/custody_accounts.rs`
  - Owns custody SQL, validation helpers, transactional allocation/release, and watched-address ensure logic.
- Modify: `backend/crates/storage/src/lib.rs`
  - Exposes the new storage module.
- Modify: `backend/crates/storage/src/repositories.rs`
  - Exposes existing `get_chain`, `validate_address_for_chain`, and default watched-address helpers as needed by custody logic.
- Modify: `backend/crates/core/src/models.rs`
  - Adds custody DTOs, list/query types, create/register/assign/release request/response types.
- Modify: `backend/crates/api-server/src/routes.rs`
  - Adds authenticated custody routes using `AuthContext.tenant_id` and storage helpers.
- Modify: `frontend/src/api/types.ts`
  - Adds custody API contract types.
- Modify: `frontend/src/api/client.ts`
  - Adds custody API client functions.
- Create: `frontend/src/pages/CustodyAccountsPage.tsx`
  - Adds custody account management UI with accounts and assignment records.
- Modify: `frontend/src/App.tsx`
  - Adds navigation item `托管账户` and render branch.
- Modify: `frontend/src/ui-regression.test.ts`
  - Adds source-level regression coverage for custody API contracts, page wiring, uniqueness hints, and automatic watch linkage.

---

### Task 1: Add custody DTOs and migration

**Files:**
- Modify: `backend/crates/core/src/models.rs`
- Create: `backend/crates/storage/migrations/0020_custody_accounts.sql`
- Test: `backend/crates/core/src/models.rs`
- Test: `backend/crates/storage/src/custody_accounts.rs` after Task 2 creates it

- [ ] **Step 1: Add failing model and migration tests**

Add this test module content near the existing model tests in `backend/crates/core/src/models.rs`; if there is already a `#[cfg(test)] mod tests`, add only the test function inside it.

```rust
#[test]
fn custody_models_serialize_expected_status_and_source_fields() {
    use crate::models::{
        AssignCustodyAccountRequest, CreateCustodyAccountRequest, CustodyAccount,
        CustodyAccountAssignment,
    };
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    let now = Utc.with_ymd_and_hms(2026, 5, 24, 12, 0, 0).unwrap();
    let account = CustodyAccount {
        id: Uuid::from_u128(1),
        tenant_id: Uuid::from_u128(2),
        chain_id: Uuid::from_u128(3),
        chain_name: "Ethereum".to_string(),
        address: "0x0000000000000000000000000000000000000001".to_string(),
        label: Some("池地址 1".to_string()),
        source: "pool".to_string(),
        status: "available".to_string(),
        watched_address_id: Some(Uuid::from_u128(4)),
        current_assignment_id: Some(Uuid::from_u128(5)),
        current_business_ref: Some("order_10001".to_string()),
        created_at: now,
        updated_at: now,
    };
    let account_json = serde_json::to_value(account).unwrap();
    assert_eq!(account_json["source"], "pool");
    assert_eq!(account_json["status"], "available");
    assert_eq!(account_json["current_business_ref"], "order_10001");

    let assignment = CustodyAccountAssignment {
        id: Uuid::from_u128(6),
        tenant_id: Uuid::from_u128(2),
        custody_account_id: Uuid::from_u128(1),
        chain_id: Uuid::from_u128(3),
        chain_name: "Ethereum".to_string(),
        address: "0x0000000000000000000000000000000000000001".to_string(),
        applicant_type: "api".to_string(),
        business_ref: "order_10001".to_string(),
        purpose: Some("deposit_address".to_string()),
        status: "active".to_string(),
        watched_address_id: Some(Uuid::from_u128(4)),
        assigned_at: now,
        released_at: None,
        created_at: now,
        updated_at: now,
    };
    let assignment_json = serde_json::to_value(assignment).unwrap();
    assert_eq!(assignment_json["applicant_type"], "api");
    assert_eq!(assignment_json["business_ref"], "order_10001");

    let create_request = CreateCustodyAccountRequest {
        chain_id: Uuid::from_u128(3),
        address: "0x0000000000000000000000000000000000000001".to_string(),
        label: Some("池地址 1".to_string()),
        source: "pool".to_string(),
        status: Some("available".to_string()),
    };
    assert_eq!(create_request.source, "pool");

    let assign_request = AssignCustodyAccountRequest {
        chain_id: Uuid::from_u128(3),
        source: "pool".to_string(),
        address: None,
        applicant_type: "api".to_string(),
        business_ref: "order_10001".to_string(),
        purpose: Some("deposit_address".to_string()),
    };
    assert_eq!(assign_request.applicant_type, "api");
}
```

- [ ] **Step 2: Run the model test and verify it fails**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-core custody_models_serialize_expected_status_and_source_fields -- --nocapture
```

Expected: FAIL with unresolved custody types.

- [ ] **Step 3: Add custody DTOs**

Append these models in `backend/crates/core/src/models.rs` near the watched-address DTOs.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct CustodyAccount {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub chain_id: Uuid,
    pub chain_name: String,
    pub address: String,
    pub label: Option<String>,
    pub source: String,
    pub status: String,
    pub watched_address_id: Option<Uuid>,
    pub current_assignment_id: Option<Uuid>,
    pub current_business_ref: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct CustodyAccountAssignment {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub custody_account_id: Uuid,
    pub chain_id: Uuid,
    pub chain_name: String,
    pub address: String,
    pub applicant_type: String,
    pub business_ref: String,
    pub purpose: Option<String>,
    pub status: String,
    pub watched_address_id: Option<Uuid>,
    pub assigned_at: DateTime<Utc>,
    pub released_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateCustodyAccountRequest {
    pub chain_id: Uuid,
    pub address: String,
    pub label: Option<String>,
    pub source: String,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssignCustodyAccountRequest {
    pub chain_id: Uuid,
    pub source: String,
    pub address: Option<String>,
    pub applicant_type: String,
    pub business_ref: String,
    pub purpose: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CustodyAccountQuery {
    pub chain_id: Option<Uuid>,
    pub source: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CustodyAssignmentQuery {
    pub chain_id: Option<Uuid>,
    pub status: Option<String>,
    pub business_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssignCustodyAccountResponse {
    pub account: CustodyAccount,
    pub assignment: CustodyAccountAssignment,
}
```

- [ ] **Step 4: Create migration**

Create `backend/crates/storage/migrations/0020_custody_accounts.sql` with this content:

```sql
CREATE TABLE IF NOT EXISTS custody_accounts (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    chain_id UUID NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
    address TEXT NOT NULL,
    address_normalized TEXT NOT NULL,
    label TEXT,
    source TEXT NOT NULL CHECK (source IN ('pool', 'user')),
    status TEXT NOT NULL CHECK (status IN ('available', 'assigned', 'disabled')),
    watched_address_id UUID REFERENCES watched_addresses(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, chain_id, address_normalized)
);

CREATE TABLE IF NOT EXISTS custody_account_assignments (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    custody_account_id UUID NOT NULL REFERENCES custody_accounts(id) ON DELETE CASCADE,
    applicant_type TEXT NOT NULL CHECK (applicant_type IN ('api', 'internal')),
    business_ref TEXT NOT NULL,
    purpose TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'released', 'cancelled')),
    watched_address_id UUID REFERENCES watched_addresses(id) ON DELETE SET NULL,
    assigned_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    released_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, business_ref)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_custody_assignments_one_active
ON custody_account_assignments(custody_account_id)
WHERE status = 'active';

CREATE INDEX IF NOT EXISTS idx_custody_accounts_tenant_chain_status
ON custody_accounts(tenant_id, chain_id, status, created_at ASC);

CREATE INDEX IF NOT EXISTS idx_custody_assignments_tenant_status
ON custody_account_assignments(tenant_id, status, assigned_at DESC);
```

- [ ] **Step 5: Run the model test and verify it passes**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-core custody_models_serialize_expected_status_and_source_fields -- --nocapture
```

Expected: PASS.

---

### Task 2: Add custody storage module and transactional allocation

**Files:**
- Create: `backend/crates/storage/src/custody_accounts.rs`
- Modify: `backend/crates/storage/src/lib.rs`
- Modify: `backend/crates/storage/src/repositories.rs`
- Test: `backend/crates/storage/src/custody_accounts.rs`

- [ ] **Step 1: Add failing storage tests**

Create `backend/crates/storage/src/custody_accounts.rs` with this initial test module and constants referenced by tests.

```rust
use chrono::Utc;
use coin_listener_core::{
    models::{
        AssignCustodyAccountRequest, AssignCustodyAccountResponse, CreateCustodyAccountRequest,
        CustodyAccount, CustodyAccountAssignment, CustodyAccountQuery, CustodyAssignmentQuery,
    },
    AppError, AppResult,
};
use sqlx::PgPool;
use uuid::Uuid;

pub const CUSTODY_SOURCE_POOL: &str = "pool";
pub const CUSTODY_SOURCE_USER: &str = "user";
pub const CUSTODY_ACCOUNT_STATUS_AVAILABLE: &str = "available";
pub const CUSTODY_ACCOUNT_STATUS_ASSIGNED: &str = "assigned";
pub const CUSTODY_ACCOUNT_STATUS_DISABLED: &str = "disabled";
pub const CUSTODY_ASSIGNMENT_STATUS_ACTIVE: &str = "active";
pub const CUSTODY_ASSIGNMENT_STATUS_RELEASED: &str = "released";
pub const CUSTODY_APPLICANT_TYPE_API: &str = "api";
pub const CUSTODY_APPLICANT_TYPE_INTERNAL: &str = "internal";

pub const LIST_CUSTODY_ACCOUNTS_QUERY: &str = "";
pub const INSERT_CUSTODY_ACCOUNT_QUERY: &str = "";
pub const CLAIM_AVAILABLE_POOL_ACCOUNT_QUERY: &str = "";
pub const INSERT_CUSTODY_ASSIGNMENT_QUERY: &str = "";
pub const ENSURE_WATCHED_ADDRESS_SELECT_QUERY: &str = "";
pub const ENSURE_WATCHED_ADDRESS_UPDATE_QUERY: &str = "";
pub const RELEASE_CUSTODY_ASSIGNMENT_QUERY: &str = "";

pub fn normalize_custody_address(address: &str) -> String {
    address.trim().to_lowercase()
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
        assert!(migration.contains("CREATE TABLE IF NOT EXISTS custody_account_assignments"));
        assert!(migration.contains("UNIQUE (tenant_id, business_ref)"));
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
        assert!(ENSURE_WATCHED_ADDRESS_SELECT_QUERY.contains("tenant_id = $1"));
        assert!(ENSURE_WATCHED_ADDRESS_SELECT_QUERY.contains("chain_id = $2"));
        assert!(ENSURE_WATCHED_ADDRESS_SELECT_QUERY.contains("lower(address) = $3"));
        assert!(ENSURE_WATCHED_ADDRESS_UPDATE_QUERY.contains("status = 'active'"));
        assert!(RELEASE_CUSTODY_ASSIGNMENT_QUERY.contains("status = 'released'"));
    }

    #[test]
    fn custody_address_normalization_trims_and_lowercases() {
        assert_eq!(
            normalize_custody_address("  0xABCDEF0000000000000000000000000000000001  "),
            "0xabcdef0000000000000000000000000000000001"
        );
    }
}
```

- [ ] **Step 2: Run storage tests and verify failure**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage custody -- --nocapture
```

Expected: FAIL because validation functions are not implemented and query constants are empty.

- [ ] **Step 3: Expose module**

Add to `backend/crates/storage/src/lib.rs`:

```rust
pub mod custody_accounts;
```

- [ ] **Step 4: Expose repository helpers needed by custody logic**

In `backend/crates/storage/src/repositories.rs`, change these private functions to crate-public:

```rust
pub(crate) async fn get_chain(pool: &PgPool, id: Uuid) -> AppResult<Chain> {
```

```rust
pub(crate) fn validate_address_for_chain(chain: &Chain, address: &str) -> AppResult<()> {
```

Keep behavior unchanged.

- [ ] **Step 5: Implement custody storage constants and helpers**

Replace the empty constants and add helper functions in `backend/crates/storage/src/custody_accounts.rs`:

```rust
use crate::repositories;
use coin_listener_core::models::{CreateWatchedAddressRequest, WatchedAddress};
use sqlx::{FromRow, Postgres, Transaction};

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

pub const INSERT_CUSTODY_ASSIGNMENT_QUERY: &str = r#"
INSERT INTO custody_account_assignments (
    tenant_id, custody_account_id, applicant_type, business_ref, purpose, status, watched_address_id
)
VALUES ($1, $2, $3, $4, $5, 'active', $6)
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
  AND lower(address) = $3
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
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct InsertedAssignmentRow {
    id: Uuid,
}

#[derive(Debug, FromRow)]
struct ReleasedAssignmentRow {
    custody_account_id: Uuid,
}

pub fn validate_custody_source(source: &str) -> AppResult<()> {
    if !matches!(source, CUSTODY_SOURCE_POOL | CUSTODY_SOURCE_USER) {
        return Err(AppError::Validation("custody source must be pool or user".to_string()));
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
    if !matches!(applicant_type, CUSTODY_APPLICANT_TYPE_API | CUSTODY_APPLICANT_TYPE_INTERNAL) {
        return Err(AppError::Validation(
            "custody applicant_type must be api or internal".to_string(),
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
```

- [ ] **Step 6: Implement list/create/assign/release functions**

Add these functions in `backend/crates/storage/src/custody_accounts.rs`:

```rust
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
        if !matches!(status, "active" | "released" | "cancelled") {
            return Err(AppError::Validation(
                "custody assignment status must be active, released, or cancelled".to_string(),
            ));
        }
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
    repositories::validate_address_for_chain(&chain, &request.address)?;
    let normalized = normalize_custody_address(&request.address);

    let row = sqlx::query_as::<_, CustodyAccountRow>(INSERT_CUSTODY_ACCOUNT_QUERY)
        .bind(tenant_id)
        .bind(request.chain_id)
        .bind(request.address)
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
    validate_business_ref(&request.business_ref)?;
    let chain = repositories::get_chain(pool, request.chain_id).await?;
    if let Some(address) = request.address.as_deref() {
        repositories::validate_address_for_chain(&chain, address)?;
    }

    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let account = claim_or_create_account_for_assignment(&mut transaction, tenant_id, &request).await?;
    let watched_address_id = ensure_watched_address_for_custody_account(
        &mut transaction,
        tenant_id,
        account.chain_id,
        &account.address,
        &request.business_ref,
    )
    .await?;

    let assignment_row = sqlx::query_as::<_, InsertedAssignmentRow>(INSERT_CUSTODY_ASSIGNMENT_QUERY)
        .bind(tenant_id)
        .bind(account.id)
        .bind(&request.applicant_type)
        .bind(&request.business_ref)
        .bind(&request.purpose)
        .bind(watched_address_id)
        .fetch_one(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

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
            chain_id: Some(account.chain_id),
            source: None,
            status: None,
        },
    )
    .await?
    .into_iter()
    .find(|account| account.id == account.id)
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

    Ok(AssignCustodyAccountResponse { account, assignment })
}

pub async fn release_custody_assignment(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
) -> AppResult<()> {
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
```

The `find(|account| account.id == account.id)` line is intentionally wrong for the red/green cycle. Fix it during implementation by saving `let account_id = account.id;` before reassignment and finding `candidate.id == account_id`.

- [ ] **Step 7: Implement transaction helpers**

Add these helper functions and fix the intentional bug from Step 6.

```rust
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
    let normalized = normalize_custody_address(address);
    let existing = sqlx::query_as::<_, CustodyAccountRow>(SELECT_USER_CUSTODY_ACCOUNT_FOR_UPDATE_QUERY)
        .bind(tenant_id)
        .bind(request.chain_id)
        .bind(&normalized)
        .fetch_optional(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    if let Some(existing) = existing {
        if existing.status == CUSTODY_ACCOUNT_STATUS_DISABLED {
            return Err(AppError::Validation("custody account is disabled".to_string()));
        }
        return Ok(existing);
    }

    sqlx::query_as::<_, CustodyAccountRow>(INSERT_CUSTODY_ACCOUNT_QUERY)
        .bind(tenant_id)
        .bind(request.chain_id)
        .bind(address)
        .bind(normalized)
        .bind(None::<String>)
        .bind(CUSTODY_SOURCE_USER)
        .bind(CUSTODY_ACCOUNT_STATUS_ASSIGNED)
        .fetch_one(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

async fn ensure_watched_address_for_custody_account(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    chain_id: Uuid,
    address: &str,
    business_ref: &str,
) -> AppResult<Uuid> {
    let normalized = normalize_custody_address(address);
    let existing = sqlx::query_as::<_, WatchedAddress>(ENSURE_WATCHED_ADDRESS_SELECT_QUERY)
        .bind(tenant_id)
        .bind(chain_id)
        .bind(&normalized)
        .fetch_optional(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    if let Some(existing) = existing {
        if existing.status == "active" {
            return Ok(existing.id);
        }
        let active = sqlx::query_as::<_, WatchedAddress>(ENSURE_WATCHED_ADDRESS_UPDATE_QUERY)
            .bind(existing.id)
            .bind(tenant_id)
            .fetch_one(transaction.as_mut())
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
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
        asset_ids: vec![native_asset_id_for_chain(transaction, chain_id).await?],
    };
    let asset_ids = vec![native_asset_id_for_chain(transaction, chain_id).await?];
    let watched = repositories::create_watched_address_in_transaction(transaction, request, asset_ids).await?;
    Ok(watched.id)
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
```

Remove the double `native_asset_id_for_chain` call by storing it in a local before building `CreateWatchedAddressRequest`.

- [ ] **Step 8: Run storage tests**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage custody -- --nocapture
```

Expected: PASS.

---

### Task 3: Add custody API routes

**Files:**
- Modify: `backend/crates/api-server/src/routes.rs`
- Test: `backend/crates/api-server/src/routes.rs`

- [ ] **Step 1: Add failing route tests**

Add to `backend/crates/api-server/src/routes.rs` tests:

```rust
#[test]
fn router_exposes_custody_routes() {
    let source = production_source();

    assert!(source.contains("/api/custody/accounts"));
    assert!(source.contains("/api/custody/accounts/assign"));
    assert!(source.contains("/api/custody/assignments"));
    assert!(source.contains("/api/custody/assignments/:id/release"));
}

#[test]
fn custody_handlers_use_authenticated_tenant_scope() {
    let source = production_source();

    assert!(source.contains("async fn list_custody_accounts("));
    assert!(source.contains("Extension(auth): Extension<AuthContext>"));
    assert!(source.contains("custody_accounts::list_custody_accounts(&state.postgres, auth.tenant_id"));
    assert!(source.contains("custody_accounts::create_custody_account(&state.postgres, auth.tenant_id"));
    assert!(source.contains("custody_accounts::assign_custody_account(&state.postgres, auth.tenant_id"));
    assert!(source.contains("custody_accounts::release_custody_assignment(&state.postgres, auth.tenant_id"));
}
```

- [ ] **Step 2: Run API tests and verify failure**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p api-server custody -- --nocapture
```

Expected: FAIL because routes and handlers do not exist.

- [ ] **Step 3: Add imports**

In `backend/crates/api-server/src/routes.rs`, import models:

```rust
AssignCustodyAccountRequest, AssignCustodyAccountResponse, CreateCustodyAccountRequest,
CustodyAccount, CustodyAccountAssignment, CustodyAccountQuery, CustodyAssignmentQuery,
```

Import storage module:

```rust
custody_accounts,
```

- [ ] **Step 4: Register routes**

In protected router setup, add:

```rust
.route("/api/custody/accounts", get(list_custody_accounts).post(create_custody_account))
.route("/api/custody/accounts/assign", post(assign_custody_account))
.route("/api/custody/assignments", get(list_custody_assignments))
.route("/api/custody/assignments/:id/release", post(release_custody_assignment))
```

- [ ] **Step 5: Add handlers**

Add near other business handlers:

```rust
async fn list_custody_accounts(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<CustodyAccountQuery>,
) -> Result<Response, ApiError> {
    let accounts = custody_accounts::list_custody_accounts(&state.postgres, auth.tenant_id, query).await?;
    Ok(Json(accounts).into_response())
}

async fn create_custody_account(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Json(payload): Json<CreateCustodyAccountRequest>,
) -> Result<Response, ApiError> {
    let account = custody_accounts::create_custody_account(&state.postgres, auth.tenant_id, payload).await?;
    Ok(Json(account).into_response())
}

async fn assign_custody_account(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Json(payload): Json<AssignCustodyAccountRequest>,
) -> Result<Response, ApiError> {
    let response = custody_accounts::assign_custody_account(&state.postgres, auth.tenant_id, payload).await?;
    Ok(Json(response).into_response())
}

async fn list_custody_assignments(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<CustodyAssignmentQuery>,
) -> Result<Response, ApiError> {
    let assignments = custody_accounts::list_custody_assignments(&state.postgres, auth.tenant_id, query).await?;
    Ok(Json(assignments).into_response())
}

async fn release_custody_assignment(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    custody_accounts::release_custody_assignment(&state.postgres, auth.tenant_id, id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
```

- [ ] **Step 6: Run API tests**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p api-server custody -- --nocapture
```

Expected: PASS.

---

### Task 4: Add frontend custody contracts and page

**Files:**
- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/api/client.ts`
- Create: `frontend/src/pages/CustodyAccountsPage.tsx`
- Modify: `frontend/src/App.tsx`
- Modify: `frontend/src/ui-regression.test.ts`

- [ ] **Step 1: Add failing frontend regression test**

Add to `frontend/src/ui-regression.test.ts`:

```ts
test('custody account API contracts and page are wired', () => {
  const types = readSource('api/types.ts');
  const client = readSource('api/client.ts');
  const app = readSource('App.tsx');
  const page = readSource('pages/CustodyAccountsPage.tsx');

  for (const expected of [
    'export type CustodyAccount',
    'export type CustodyAccountAssignment',
    'export type CreateCustodyAccountRequest',
    'export type AssignCustodyAccountRequest',
    'export type AssignCustodyAccountResponse',
  ]) {
    expectContains(types, expected);
  }

  for (const expected of [
    'listCustodyAccounts',
    'createCustodyAccount',
    'assignCustodyAccount',
    'listCustodyAssignments',
    'releaseCustodyAssignment',
    '/api/custody/accounts',
    '/api/custody/accounts/assign',
    '/api/custody/assignments',
  ]) {
    expectContains(client, expected);
  }

  expectContains(app, "'custody-accounts'");
  expectContains(app, 'CustodyAccountsPage');
  expectContains(app, '托管账户');

  for (const expected of [
    'tableId="custody-accounts"',
    'tableId="custody-assignments"',
    '系统地址池',
    '用户自带地址',
    '申请地址',
    '释放',
    '不能重复占用',
    'watched_address_id',
  ]) {
    expectContains(page, expected);
  }
});
```

- [ ] **Step 2: Run frontend regression and verify failure**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: FAIL because custody files/types/functions do not exist.

- [ ] **Step 3: Add frontend types**

Add to `frontend/src/api/types.ts`:

```ts
export type CustodyAccountSource = 'pool' | 'user' | string;
export type CustodyAccountStatus = 'available' | 'assigned' | 'disabled' | string;
export type CustodyAssignmentStatus = 'active' | 'released' | 'cancelled' | string;
export type CustodyApplicantType = 'api' | 'internal' | string;

export type CustodyAccount = {
  id: string;
  tenant_id: string;
  chain_id: string;
  chain_name: string;
  address: string;
  label?: string | null;
  source: CustodyAccountSource;
  status: CustodyAccountStatus;
  watched_address_id?: string | null;
  current_assignment_id?: string | null;
  current_business_ref?: string | null;
  created_at: string;
  updated_at: string;
};

export type CustodyAccountAssignment = {
  id: string;
  tenant_id: string;
  custody_account_id: string;
  chain_id: string;
  chain_name: string;
  address: string;
  applicant_type: CustodyApplicantType;
  business_ref: string;
  purpose?: string | null;
  status: CustodyAssignmentStatus;
  watched_address_id?: string | null;
  assigned_at: string;
  released_at?: string | null;
  created_at: string;
  updated_at: string;
};

export type CreateCustodyAccountRequest = {
  chain_id: string;
  address: string;
  label?: string | null;
  source: CustodyAccountSource;
  status?: CustodyAccountStatus;
};

export type AssignCustodyAccountRequest = {
  chain_id: string;
  source: CustodyAccountSource;
  address?: string | null;
  applicant_type: CustodyApplicantType;
  business_ref: string;
  purpose?: string | null;
};

export type CustodyAccountQuery = {
  chain_id?: string;
  source?: string;
  status?: string;
};

export type CustodyAssignmentQuery = {
  chain_id?: string;
  status?: string;
  business_ref?: string;
};

export type AssignCustodyAccountResponse = {
  account: CustodyAccount;
  assignment: CustodyAccountAssignment;
};
```

- [ ] **Step 4: Add frontend client functions**

Import new types in `frontend/src/api/client.ts`, then add:

```ts
export function listCustodyAccounts(filters: CustodyAccountQuery = {}): Promise<CustodyAccount[]> {
  return request<CustodyAccount[]>(`/api/custody/accounts${buildQuery(filters)}`);
}

export function createCustodyAccount(payload: CreateCustodyAccountRequest): Promise<CustodyAccount> {
  return request<CustodyAccount>('/api/custody/accounts', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export function assignCustodyAccount(payload: AssignCustodyAccountRequest): Promise<AssignCustodyAccountResponse> {
  return request<AssignCustodyAccountResponse>('/api/custody/accounts/assign', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export function listCustodyAssignments(filters: CustodyAssignmentQuery = {}): Promise<CustodyAccountAssignment[]> {
  return request<CustodyAccountAssignment[]>(`/api/custody/assignments${buildQuery(filters)}`);
}

export function releaseCustodyAssignment(id: string): Promise<void> {
  return request<void>(`/api/custody/assignments/${id}/release`, {
    method: 'POST',
  });
}
```

- [ ] **Step 5: Create CustodyAccountsPage**

Create `frontend/src/pages/CustodyAccountsPage.tsx` with this page:

```tsx
import { useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Banner, Button, Card, Form, Modal, Space, Tag, Toast, Typography } from '@douyinfe/semi-ui';
import {
  assignCustodyAccount,
  createCustodyAccount,
  listChains,
  listCustodyAccounts,
  listCustodyAssignments,
  releaseCustodyAssignment,
} from '../api/client';
import type { AssignCustodyAccountRequest, CreateCustodyAccountRequest, CustodyAccount, CustodyAccountAssignment } from '../api/types';
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { PageScaffold } from '../components/PageScaffold';

const { Text } = Typography;

type AccountForm = {
  chain_id?: string;
  address?: string;
  label?: string;
  source?: string;
  status?: string;
};

type AssignForm = {
  chain_id?: string;
  source?: string;
  address?: string;
  applicant_type?: string;
  business_ref?: string;
  purpose?: string;
};

function sourceText(source: string) {
  if (source === 'pool') return '系统地址池';
  if (source === 'user') return '用户自带地址';
  return source;
}

function statusColor(status: string): 'green' | 'blue' | 'red' | 'grey' {
  if (status === 'available') return 'green';
  if (status === 'assigned' || status === 'active') return 'blue';
  if (status === 'disabled' || status === 'cancelled') return 'red';
  return 'grey';
}

function statusText(status: string) {
  if (status === 'available') return '可用';
  if (status === 'assigned') return '已分配';
  if (status === 'disabled') return '禁用';
  if (status === 'active') return '占用中';
  if (status === 'released') return '已释放';
  if (status === 'cancelled') return '已取消';
  return status;
}

function formatTime(value?: string | null) {
  return value ? new Date(value).toLocaleString() : '-';
}

export function CustodyAccountsPage() {
  const [accountVisible, setAccountVisible] = useState(false);
  const [assignVisible, setAssignVisible] = useState(false);
  const queryClient = useQueryClient();
  const chainsQuery = useQuery({ queryKey: ['chains'], queryFn: listChains });
  const accountsQuery = useQuery({ queryKey: ['custody-accounts'], queryFn: () => listCustodyAccounts() });
  const assignmentsQuery = useQuery({ queryKey: ['custody-assignments'], queryFn: () => listCustodyAssignments() });
  const chainOptions = useMemo(() => (chainsQuery.data ?? []).map(chain => ({ label: chain.name, value: chain.id })), [chainsQuery.data]);

  const refreshCustody = () => {
    queryClient.invalidateQueries({ queryKey: ['custody-accounts'] });
    queryClient.invalidateQueries({ queryKey: ['custody-assignments'] });
    queryClient.invalidateQueries({ queryKey: ['addresses'] });
  };

  const createMutation = useMutation({
    mutationFn: (values: Record<string, unknown>) => createCustodyAccount({
      chain_id: String(values.chain_id),
      address: String(values.address).trim(),
      label: values.label ? String(values.label) : null,
      source: String(values.source),
      status: values.status ? String(values.status) : undefined,
    } satisfies CreateCustodyAccountRequest),
    onSuccess: () => {
      Toast.success('托管地址已保存');
      setAccountVisible(false);
      refreshCustody();
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '保存托管地址失败'),
  });

  const assignMutation = useMutation({
    mutationFn: (values: Record<string, unknown>) => assignCustodyAccount({
      chain_id: String(values.chain_id),
      source: String(values.source),
      address: values.address ? String(values.address).trim() : null,
      applicant_type: String(values.applicant_type),
      business_ref: String(values.business_ref).trim(),
      purpose: values.purpose ? String(values.purpose) : null,
    } satisfies AssignCustodyAccountRequest),
    onSuccess: () => {
      Toast.success('地址申请成功，已自动加入监听');
      setAssignVisible(false);
      refreshCustody();
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '申请地址失败'),
  });

  const releaseMutation = useMutation({
    mutationFn: releaseCustodyAssignment,
    onSuccess: () => {
      Toast.success('地址已释放');
      refreshCustody();
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '释放地址失败'),
  });

  return (
    <PageScaffold
      title="托管账户"
      description="管理系统地址池和用户自带地址；申请地址时会自动加入监听，并通过数据库约束确保不能重复占用。"
      actions={(
        <Space>
          <Button onClick={() => setAccountVisible(true)}>添加托管地址</Button>
          <Button type="primary" onClick={() => setAssignVisible(true)}>申请地址</Button>
        </Space>
      )}
    >
      <Banner type="info" title="防重复申请" description="同一地址只能存在一个 active 分配；系统地址池领取使用事务锁避免并发重复分配。" />

      <DataSurface title="托管地址">
        <DataTable<CustodyAccount>
          tableId="custody-accounts"
          loading={accountsQuery.isLoading}
          dataSource={accountsQuery.data ?? []}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          columns={[
            { title: '链', dataIndex: 'chain_name', width: 140 },
            { title: '地址', dataIndex: 'address', width: 260, ellipsis: { showTitle: true }, render: value => <span className="table-cell-mono">{String(value)}</span> },
            { title: '来源', dataIndex: 'source', width: 130, render: value => <Tag>{sourceText(String(value))}</Tag> },
            { title: '状态', dataIndex: 'status', width: 100, render: value => <Tag color={statusColor(String(value))}>{statusText(String(value))}</Tag> },
            { title: '业务引用', dataIndex: 'current_business_ref', width: 180, render: value => value ? String(value) : '-' },
            { title: '监听地址', dataIndex: 'watched_address_id', width: 220, ellipsis: { showTitle: true }, render: value => value ? <span className="table-cell-mono">{String(value)}</span> : '-' },
          ]}
        />
      </DataSurface>

      <DataSurface title="申请记录">
        <DataTable<CustodyAccountAssignment>
          tableId="custody-assignments"
          loading={assignmentsQuery.isLoading}
          dataSource={assignmentsQuery.data ?? []}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          columns={[
            { title: '申请时间', dataIndex: 'assigned_at', width: 180, render: value => formatTime(String(value)) },
            { title: '链', dataIndex: 'chain_name', width: 130 },
            { title: '地址', dataIndex: 'address', width: 240, ellipsis: { showTitle: true }, render: value => <span className="table-cell-mono">{String(value)}</span> },
            { title: '申请方', dataIndex: 'applicant_type', width: 100 },
            { title: '业务引用', dataIndex: 'business_ref', width: 180 },
            { title: '用途', dataIndex: 'purpose', width: 160, render: value => value ? String(value) : '-' },
            { title: '状态', dataIndex: 'status', width: 100, render: value => <Tag color={statusColor(String(value))}>{statusText(String(value))}</Tag> },
            {
              title: '操作',
              key: 'operations',
              width: 100,
              render: (_, row) => row.status === 'active' ? (
                <Button size="small" loading={releaseMutation.isPending} onClick={() => releaseMutation.mutate(row.id)}>释放</Button>
              ) : '-',
            },
          ]}
        />
      </DataSurface>

      <CustodyAccountModal
        visible={accountVisible}
        chainOptions={chainOptions}
        loading={createMutation.isPending}
        onCancel={() => setAccountVisible(false)}
        onSubmit={values => createMutation.mutate(values)}
      />
      <AssignCustodyModal
        visible={assignVisible}
        chainOptions={chainOptions}
        loading={assignMutation.isPending}
        onCancel={() => setAssignVisible(false)}
        onSubmit={values => assignMutation.mutate(values)}
      />
    </PageScaffold>
  );
}

function CustodyAccountModal({ visible, chainOptions, loading, onCancel, onSubmit }: { visible: boolean; chainOptions: Array<{ label: string; value: string }>; loading: boolean; onCancel: () => void; onSubmit: (values: Record<string, unknown>) => void }) {
  return (
    <Modal title="添加托管地址" visible={visible} onCancel={onCancel} footer={null}>
      <Form<AccountForm> onSubmit={onSubmit} initValues={{ source: 'pool', status: 'available' }}>
        <Form.Select field="chain_id" label="链" optionList={chainOptions} rules={[{ required: true, message: '请选择链' }]} />
        <Form.Input field="address" label="地址" rules={[{ required: true, message: '请输入地址' }]} />
        <Form.Input field="label" label="标签" />
        <Form.Select field="source" label="来源" optionList={[{ label: '系统地址池', value: 'pool' }, { label: '用户自带地址', value: 'user' }]} />
        <Form.Select field="status" label="状态" optionList={[{ label: '可用', value: 'available' }, { label: '已分配', value: 'assigned' }, { label: '禁用', value: 'disabled' }]} />
        <Button htmlType="submit" type="primary" loading={loading}>保存</Button>
      </Form>
    </Modal>
  );
}

function AssignCustodyModal({ visible, chainOptions, loading, onCancel, onSubmit }: { visible: boolean; chainOptions: Array<{ label: string; value: string }>; loading: boolean; onCancel: () => void; onSubmit: (values: Record<string, unknown>) => void }) {
  return (
    <Modal title="申请地址" visible={visible} onCancel={onCancel} footer={null}>
      <Card title="申请说明" style={{ marginBottom: 16 }}>
        <Text type="tertiary">系统地址池无需填写地址；用户自带地址需要填写 address。business_ref 在同一租户内唯一，避免重复业务申请。</Text>
      </Card>
      <Form<AssignForm> onSubmit={onSubmit} initValues={{ source: 'pool', applicant_type: 'api' }}>
        <Form.Select field="chain_id" label="链" optionList={chainOptions} rules={[{ required: true, message: '请选择链' }]} />
        <Form.Select field="source" label="来源" optionList={[{ label: '系统地址池', value: 'pool' }, { label: '用户自带地址', value: 'user' }]} />
        <Form.Input field="address" label="用户地址" placeholder="source=user 时必填" />
        <Form.Select field="applicant_type" label="申请方" optionList={[{ label: '外部 API', value: 'api' }, { label: '内部业务', value: 'internal' }]} />
        <Form.Input field="business_ref" label="业务引用" rules={[{ required: true, message: '请输入业务引用' }]} />
        <Form.Input field="purpose" label="用途" />
        <Button htmlType="submit" type="primary" loading={loading}>申请地址</Button>
      </Form>
    </Modal>
  );
}
```

- [ ] **Step 6: Wire navigation**

Modify `frontend/src/App.tsx`:

```tsx
import { CustodyAccountsPage } from './pages/CustodyAccountsPage';
```

Add to `PageKey`:

```ts
| 'custody-accounts'
```

Add nav item near addresses:

```tsx
{ itemKey: 'custody-accounts', text: '托管账户', icon: <IconUser /> },
```

Add render branch:

```tsx
if (page === 'custody-accounts') return <CustodyAccountsPage />;
```

- [ ] **Step 7: Run frontend tests and build**

Run:

```bash
npm --prefix frontend run test:ui-regression
npm --prefix frontend run build
```

Expected: PASS. Existing lottie/chunk warnings may remain.

---

### Task 5: Final integration verification

**Files:**
- Validate all files modified by Tasks 1-4.

- [ ] **Step 1: Run backend formatting**

Run:

```bash
cargo fmt --manifest-path backend/Cargo.toml --all -- --check
```

Expected: PASS.

If it fails, run:

```bash
cargo fmt --manifest-path backend/Cargo.toml --all
cargo fmt --manifest-path backend/Cargo.toml --all -- --check
```

Expected: PASS.

- [ ] **Step 2: Run targeted backend tests**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-core custody -- --nocapture
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage custody -- --nocapture
cargo test --manifest-path backend/Cargo.toml -p api-server custody -- --nocapture
```

Expected: PASS.

- [ ] **Step 3: Run full backend tests**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml --quiet
```

Expected: PASS.

- [ ] **Step 4: Run frontend validation**

Run:

```bash
npm --prefix frontend run test:ui-regression
npm --prefix frontend run build
```

Expected: PASS. Existing `lottie-web` direct eval and chunk-size warnings may remain.

- [ ] **Step 5: Review git diff**

Run:

```bash
git diff --stat
git diff --name-only
```

Expected changed files:

```text
backend/crates/core/src/models.rs
backend/crates/storage/migrations/0020_custody_accounts.sql
backend/crates/storage/src/custody_accounts.rs
backend/crates/storage/src/lib.rs
backend/crates/storage/src/repositories.rs
backend/crates/api-server/src/routes.rs
frontend/src/api/types.ts
frontend/src/api/client.ts
frontend/src/pages/CustodyAccountsPage.tsx
frontend/src/App.tsx
frontend/src/ui-regression.test.ts
docs/superpowers/plans/2026-05-24-custody-account-mode.md
```

- [ ] **Step 6: Commit**

```bash
git add \
  backend/crates/core/src/models.rs \
  backend/crates/storage/migrations/0020_custody_accounts.sql \
  backend/crates/storage/src/custody_accounts.rs \
  backend/crates/storage/src/lib.rs \
  backend/crates/storage/src/repositories.rs \
  backend/crates/api-server/src/routes.rs \
  frontend/src/api/types.ts \
  frontend/src/api/client.ts \
  frontend/src/pages/CustodyAccountsPage.tsx \
  frontend/src/App.tsx \
  frontend/src/ui-regression.test.ts \
  docs/superpowers/plans/2026-05-24-custody-account-mode.md

git commit -m "添加托管账户模式"
```

Expected: commit created on current branch.
