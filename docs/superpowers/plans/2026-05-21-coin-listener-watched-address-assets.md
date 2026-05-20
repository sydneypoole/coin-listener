# Coin Listener Watched Address Assets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add explicit per-chain asset selection for watched addresses so the same address string can be monitored on multiple chains with different selected assets.

**Architecture:** Keep `watched_addresses` as the tenant + chain + address scheduling entity and add a normalized `watched_address_assets` join table for selected assets. API repository functions validate non-empty asset selections, enforce chain ownership, and return an aggregate response containing `asset_ids`; workers query selected assets and scan only those assets. The frontend creates one address string with one or more chain rows, each row requiring at least one active asset.

**Tech Stack:** Rust 2021, Axum, SQLx/PostgreSQL migrations, Tokio tests, React + TypeScript, TanStack Query, Semi Design `Form.Select` multi-select.

---

## File Structure

| File | Responsibility |
|---|---|
| `backend/crates/storage/migrations/0013_watched_address_assets.sql` | Create `watched_address_assets` join table and asset lookup index. |
| `backend/crates/core/src/models.rs` | Add `asset_ids` to `CreateWatchedAddressRequest`; add `WatchedAddressResponse` aggregate API model. |
| `backend/crates/storage/src/repositories.rs` | Validate selected assets, write associations transactionally, list association ids without N+1, and expose selected asset helpers for workers. |
| `backend/crates/api-server/src/routes.rs` | Keep address endpoints on existing paths while returning aggregate address responses with `asset_ids`; add route regression tests for address methods. |
| `backend/crates/worker/src/lib.rs` | Replace “all active chain assets” scans with selected-asset scan sets for EVM, TRON, and BTC. |
| `frontend/src/api/types.ts` | Add `asset_ids: string[]` to watched address request/response type. |
| `frontend/src/pages/AddressesPage.tsx` | Add asset metadata query, create modal chain rows with asset multi-select, edit modal for one watched address, and “监听资产” table column. |
| `frontend/src/ui-regression.test.ts` | Add source-level frontend regression tests for the asset selector and table column; preserve the file if it already exists with unrelated UI regressions. |

## Implementation Tasks

### Task 1: Add watched-address asset join table migration

**Files:**
- Create: `backend/crates/storage/migrations/0013_watched_address_assets.sql`
- Modify: `backend/crates/storage/src/repositories.rs`

- [ ] **Step 1: Write the failing migration regression test**

Add `WATCHED_ADDRESS_ASSETS_MIGRATION` import in the existing `#[cfg(test)] mod tests` import list in `backend/crates/storage/src/repositories.rs`:

```rust
use super::{
    next_scan_at_from, ACTIVE_ASSETS_BY_TYPE_QUERY, ACTIVE_ERC20_ASSETS_QUERY,
    ACTIVE_RPC_PROVIDER_QUERY, ACTIVE_TENANT_MEMBERSHIP_QUERY, ACTIVE_USER_QUERY,
    CLAIM_DUE_NOTIFICATION_OUTBOX_QUERY, CLAIM_ONE_DUE_SCAN_ADDRESS_QUERY,
    DELETE_WATCHED_ADDRESS_QUERY, GET_NOTIFICATION_OUTBOX_ITEM_QUERY,
    GET_WATCHED_ADDRESS_QUERY, INSERT_BALANCE_SNAPSHOT_QUERY, INSERT_EVENT_IF_NOT_EXISTS_QUERY,
    INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY, LATEST_BALANCE_SNAPSHOT_QUERY,
    LIST_EVENTS_QUERY, LIST_NOTIFICATION_OUTBOX_QUERY, LIST_WATCHED_ADDRESSES_QUERY,
    MANUAL_RETRY_NOTIFICATION_OUTBOX_QUERY, MARK_CLAIMED_SCAN_ENQUEUED_QUERY,
    MARK_NOTIFICATION_OUTBOX_DELIVERED_QUERY, MARK_NOTIFICATION_OUTBOX_FAILED_QUERY,
    MARK_NOTIFICATION_OUTBOX_RETRYABLE_QUERY, NOTIFICATION_OUTBOX_STATUS_COUNTS_QUERY,
    RELEASE_STALE_NOTIFICATION_OUTBOX_QUERY, SCAN_CURSOR_QUERY,
    SELECT_NOTIFICATION_OUTBOX_STATUS_QUERY, UPDATE_WATCHED_ADDRESS_QUERY,
    UPSERT_SCAN_CURSOR_QUERY, WATCHED_ADDRESS_ASSETS_MIGRATION,
};
```

Add this test near the other migration tests in the same module:

```rust
#[test]
fn watched_address_assets_migration_defines_join_table() {
    assert!(WATCHED_ADDRESS_ASSETS_MIGRATION.contains("CREATE TABLE IF NOT EXISTS watched_address_assets"));
    assert!(WATCHED_ADDRESS_ASSETS_MIGRATION.contains(
        "address_id UUID NOT NULL REFERENCES watched_addresses(id) ON DELETE CASCADE"
    ));
    assert!(WATCHED_ADDRESS_ASSETS_MIGRATION
        .contains("asset_id UUID NOT NULL REFERENCES assets(id) ON DELETE CASCADE"));
    assert!(WATCHED_ADDRESS_ASSETS_MIGRATION.contains("PRIMARY KEY (address_id, asset_id)"));
    assert!(WATCHED_ADDRESS_ASSETS_MIGRATION.contains("idx_watched_address_assets_asset"));
}
```

- [ ] **Step 2: Run the storage test to verify it fails**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage watched_address_assets_migration_defines_join_table
```

Expected: FAIL because `WATCHED_ADDRESS_ASSETS_MIGRATION` does not exist yet.

- [ ] **Step 3: Create the migration and expose it to the test**

Create `backend/crates/storage/migrations/0013_watched_address_assets.sql`:

```sql
CREATE TABLE IF NOT EXISTS watched_address_assets (
    address_id UUID NOT NULL REFERENCES watched_addresses(id) ON DELETE CASCADE,
    asset_id UUID NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (address_id, asset_id)
);

CREATE INDEX IF NOT EXISTS idx_watched_address_assets_asset
    ON watched_address_assets(asset_id);
```

Add this constant near the other query constants in `backend/crates/storage/src/repositories.rs`:

```rust
pub const WATCHED_ADDRESS_ASSETS_MIGRATION: &str =
    include_str!("../migrations/0013_watched_address_assets.sql");
```

- [ ] **Step 4: Run the storage test to verify it passes**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage watched_address_assets_migration_defines_join_table
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add backend/crates/storage/migrations/0013_watched_address_assets.sql backend/crates/storage/src/repositories.rs
git commit -m "$(cat <<'EOF'
Add watched address asset join table migration
EOF
)"
```

### Task 2: Add watched address asset request and response models

**Files:**
- Modify: `backend/crates/core/src/models.rs`

- [ ] **Step 1: Write the failing model serialization tests**

Update the existing `use super` list in `backend/crates/core/src/models.rs` tests to include the address request/response types:

```rust
use super::{
    AddressEvent, CreateBalanceSnapshotRequest, CreateWatchedAddressRequest, EventStatus,
    NotificationDelivery, NotificationDeliveryListItem, NotificationDeliveryListResponse,
    NotificationDeliveryQuery, NotificationOutboxDetail, NotificationOutboxItem,
    NotificationOutboxListItem, NotificationOutboxListResponse, NotificationOutboxQuery,
    NotificationStatus, NotifyEventTask, OutboxStatusCounts, ProviderChainStatus,
    ProviderHealthStatus, ProviderStatus, ProviderStatusItem, QueueStatus,
    RetryNotificationOutboxResponse, ScanAddressTask, ScanCursor, ScanStatus,
    ServiceHealthStatus, ServiceHeartbeatStatusItem, SystemStatus, WatchedAddressResponse,
};
```

Add these tests near the existing model round-trip tests:

```rust
#[test]
fn watched_address_request_deserializes_asset_ids() {
    let payload = r#"{
        "chain_id":"00000000-0000-0000-0000-000000000002",
        "address":"0x0000000000000000000000000000000000000001",
        "label":"Main wallet",
        "priority":"normal",
        "scan_interval_seconds":300,
        "transfer_filter_enabled":true,
        "balance_change_filter_enabled":true,
        "status":"active",
        "asset_ids":[
            "00000000-0000-0000-0000-000000000101",
            "00000000-0000-0000-0000-000000000102"
        ]
    }"#;

    let request: CreateWatchedAddressRequest =
        serde_json::from_str(payload).expect("deserialize watched address request");

    assert_eq!(request.chain_id, Uuid::from_u128(2));
    assert_eq!(request.asset_ids, vec![Uuid::from_u128(0x101), Uuid::from_u128(0x102)]);
}

#[test]
fn watched_address_response_serializes_asset_ids() {
    let response = WatchedAddressResponse {
        id: Uuid::from_u128(10),
        tenant_id: Uuid::from_u128(1),
        chain_id: Uuid::from_u128(2),
        address: "0x0000000000000000000000000000000000000001".to_string(),
        label: Some("Main wallet".to_string()),
        priority: "normal".to_string(),
        scan_interval_seconds: 300,
        transfer_filter_enabled: true,
        balance_change_filter_enabled: true,
        status: "active".to_string(),
        asset_ids: vec![Uuid::from_u128(0x101), Uuid::from_u128(0x102)],
    };

    let payload = serde_json::to_string(&response).expect("serialize watched address response");

    assert!(payload.contains("\"asset_ids\""));
    assert!(payload.contains("00000000-0000-0000-0000-000000000101"));
    assert!(payload.contains("00000000-0000-0000-0000-000000000102"));
}
```

- [ ] **Step 2: Run the model tests to verify they fail**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-core watched_address_
```

Expected: FAIL because `CreateWatchedAddressRequest` lacks `asset_ids` and `WatchedAddressResponse` does not exist.

- [ ] **Step 3: Add the request field and response model**

Change `CreateWatchedAddressRequest` in `backend/crates/core/src/models.rs` to:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct CreateWatchedAddressRequest {
    pub tenant_id: Option<Uuid>,
    pub chain_id: Uuid,
    pub address: String,
    pub label: Option<String>,
    pub priority: String,
    pub scan_interval_seconds: i32,
    pub transfer_filter_enabled: bool,
    pub balance_change_filter_enabled: bool,
    pub status: String,
    pub asset_ids: Vec<Uuid>,
}
```

Add this model immediately after `WatchedAddress`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchedAddressResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub chain_id: Uuid,
    pub address: String,
    pub label: Option<String>,
    pub priority: String,
    pub scan_interval_seconds: i32,
    pub transfer_filter_enabled: bool,
    pub balance_change_filter_enabled: bool,
    pub status: String,
    pub asset_ids: Vec<Uuid>,
}
```

- [ ] **Step 4: Run the model tests to verify they pass**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-core watched_address_
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add backend/crates/core/src/models.rs
git commit -m "$(cat <<'EOF'
Add watched address asset API models
EOF
)"
```

### Task 3: Add selected-asset repository helpers and validations

**Files:**
- Modify: `backend/crates/storage/src/repositories.rs`

- [ ] **Step 1: Write failing repository helper tests**

Update the test module imports to include the new constants and helper functions:

```rust
use super::{
    asset_ids_for_address, next_scan_at_from, selected_assets_for_address,
    validate_assets_for_chain, ACTIVE_ASSETS_BY_TYPE_QUERY, ACTIVE_ERC20_ASSETS_QUERY,
    ACTIVE_RPC_PROVIDER_QUERY, ACTIVE_TENANT_MEMBERSHIP_QUERY, ACTIVE_USER_QUERY,
    ASSET_IDS_FOR_ADDRESS_QUERY, CLAIM_DUE_NOTIFICATION_OUTBOX_QUERY,
    CLAIM_ONE_DUE_SCAN_ADDRESS_QUERY, DELETE_WATCHED_ADDRESS_QUERY,
    GET_NOTIFICATION_OUTBOX_ITEM_QUERY, GET_WATCHED_ADDRESS_QUERY,
    INSERT_BALANCE_SNAPSHOT_QUERY, INSERT_EVENT_IF_NOT_EXISTS_QUERY,
    INSERT_NOTIFICATION_OUTBOX_FOR_EVENT_QUERY, LATEST_BALANCE_SNAPSHOT_QUERY,
    LIST_EVENTS_QUERY, LIST_NOTIFICATION_OUTBOX_QUERY, LIST_WATCHED_ADDRESSES_QUERY,
    MANUAL_RETRY_NOTIFICATION_OUTBOX_QUERY, MARK_CLAIMED_SCAN_ENQUEUED_QUERY,
    MARK_NOTIFICATION_OUTBOX_DELIVERED_QUERY, MARK_NOTIFICATION_OUTBOX_FAILED_QUERY,
    MARK_NOTIFICATION_OUTBOX_RETRYABLE_QUERY, NOTIFICATION_OUTBOX_STATUS_COUNTS_QUERY,
    RELEASE_STALE_NOTIFICATION_OUTBOX_QUERY, REPLACE_WATCHED_ADDRESS_ASSETS_DELETE_QUERY,
    REPLACE_WATCHED_ADDRESS_ASSETS_INSERT_QUERY, SCAN_CURSOR_QUERY,
    SELECTED_ASSETS_FOR_ADDRESS_QUERY, SELECT_NOTIFICATION_OUTBOX_STATUS_QUERY,
    UPDATE_WATCHED_ADDRESS_QUERY, UPSERT_SCAN_CURSOR_QUERY, VALIDATE_ASSETS_FOR_CHAIN_QUERY,
    WATCHED_ADDRESS_ASSETS_MIGRATION,
};
```

Add these tests near the existing watched address query tests:

```rust
#[test]
fn watched_address_asset_queries_are_scoped_and_ordered() {
    assert!(SELECTED_ASSETS_FOR_ADDRESS_QUERY.contains("FROM watched_address_assets waa"));
    assert!(SELECTED_ASSETS_FOR_ADDRESS_QUERY.contains("INNER JOIN assets a ON a.id = waa.asset_id"));
    assert!(SELECTED_ASSETS_FOR_ADDRESS_QUERY.contains("waa.address_id = $1"));
    assert!(SELECTED_ASSETS_FOR_ADDRESS_QUERY.contains("a.status = 'active'"));
    assert!(SELECTED_ASSETS_FOR_ADDRESS_QUERY.contains("ORDER BY a.asset_type, a.symbol, a.name"));

    assert!(ASSET_IDS_FOR_ADDRESS_QUERY.contains("SELECT asset_id"));
    assert!(ASSET_IDS_FOR_ADDRESS_QUERY.contains("WHERE address_id = $1"));
    assert!(ASSET_IDS_FOR_ADDRESS_QUERY.contains("ORDER BY created_at, asset_id"));

    assert!(VALIDATE_ASSETS_FOR_CHAIN_QUERY.contains("WHERE id = ANY($1)"));
    assert!(VALIDATE_ASSETS_FOR_CHAIN_QUERY.contains("ORDER BY id"));

    assert!(REPLACE_WATCHED_ADDRESS_ASSETS_DELETE_QUERY.contains("DELETE FROM watched_address_assets"));
    assert!(REPLACE_WATCHED_ADDRESS_ASSETS_DELETE_QUERY.contains("WHERE address_id = $1"));
    assert!(REPLACE_WATCHED_ADDRESS_ASSETS_INSERT_QUERY.contains("UNNEST($2::uuid[])"));
    assert!(REPLACE_WATCHED_ADDRESS_ASSETS_INSERT_QUERY.contains("ON CONFLICT DO NOTHING"));
}

#[test]
fn watched_address_asset_helper_signatures_are_stable() {
    #[allow(dead_code)]
    async fn assert_selected(pool: &PgPool, address_id: uuid::Uuid) -> AppResult<Vec<coin_listener_core::models::Asset>> {
        selected_assets_for_address(pool, address_id).await
    }

    #[allow(dead_code)]
    async fn assert_asset_ids(pool: &PgPool, address_id: uuid::Uuid) -> AppResult<Vec<uuid::Uuid>> {
        asset_ids_for_address(pool, address_id).await
    }

    #[allow(dead_code)]
    async fn assert_validate(
        pool: &PgPool,
        chain_id: uuid::Uuid,
        asset_ids: &[uuid::Uuid],
    ) -> AppResult<Vec<uuid::Uuid>> {
        validate_assets_for_chain(pool, chain_id, asset_ids).await
    }

    let _ = assert_selected;
    let _ = assert_asset_ids;
    let _ = assert_validate;
}

#[test]
fn validate_assets_for_chain_error_text_is_stable() {
    let empty = super::validate_asset_selection_input(uuid::Uuid::from_u128(1), &[])
        .expect_err("empty asset selection rejected");
    assert!(matches!(empty, AppError::Validation(message) if message == "asset_ids must not be empty"));

    let wrong_chain = super::validate_asset_chain_match(
        uuid::Uuid::from_u128(1),
        &coin_listener_core::models::Asset {
            id: uuid::Uuid::from_u128(2),
            chain_id: uuid::Uuid::from_u128(3),
            asset_type: "native".to_string(),
            symbol: "ETH".to_string(),
            name: "Ether".to_string(),
            contract_address: None,
            decimals: 18,
            is_builtin: true,
            status: "active".to_string(),
        },
    )
    .expect_err("wrong chain rejected");
    assert!(matches!(wrong_chain, AppError::Validation(message) if message.contains("asset must belong to watched address chain")));
}
```

- [ ] **Step 2: Run the helper tests to verify they fail**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage watched_address_asset_
```

Expected: FAIL because helper constants/functions do not exist.

- [ ] **Step 3: Add query constants**

Add these constants near the watched-address query constants in `backend/crates/storage/src/repositories.rs`:

```rust
pub const SELECTED_ASSETS_FOR_ADDRESS_QUERY: &str = r#"
SELECT a.id, a.chain_id, a.asset_type, a.symbol, a.name, a.contract_address, a.decimals, a.is_builtin, a.status
FROM watched_address_assets waa
INNER JOIN assets a ON a.id = waa.asset_id
WHERE waa.address_id = $1
  AND a.status = 'active'
ORDER BY a.asset_type, a.symbol, a.name
"#;

pub const ASSET_IDS_FOR_ADDRESS_QUERY: &str = r#"
SELECT asset_id
FROM watched_address_assets
WHERE address_id = $1
ORDER BY created_at, asset_id
"#;

pub const VALIDATE_ASSETS_FOR_CHAIN_QUERY: &str = r#"
SELECT id, chain_id, asset_type, symbol, name, contract_address, decimals, is_builtin, status
FROM assets
WHERE id = ANY($1)
ORDER BY id
"#;

pub const REPLACE_WATCHED_ADDRESS_ASSETS_DELETE_QUERY: &str =
    "DELETE FROM watched_address_assets WHERE address_id = $1";

pub const REPLACE_WATCHED_ADDRESS_ASSETS_INSERT_QUERY: &str = r#"
INSERT INTO watched_address_assets (address_id, asset_id)
SELECT $1, asset_id
FROM UNNEST($2::uuid[]) AS asset_id
ON CONFLICT DO NOTHING
"#;
```

- [ ] **Step 4: Add validation and selected asset helpers**

Add `HashSet` import at the top:

```rust
use std::collections::HashSet;
```

Add these helpers after `active_assets_for_chain_by_type`:

```rust
pub async fn selected_assets_for_address(pool: &PgPool, address_id: Uuid) -> AppResult<Vec<Asset>> {
    sqlx::query_as::<_, Asset>(SELECTED_ASSETS_FOR_ADDRESS_QUERY)
        .bind(address_id)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn asset_ids_for_address(pool: &PgPool, address_id: Uuid) -> AppResult<Vec<Uuid>> {
    sqlx::query_scalar::<_, Uuid>(ASSET_IDS_FOR_ADDRESS_QUERY)
        .bind(address_id)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn validate_assets_for_chain(
    pool: &PgPool,
    chain_id: Uuid,
    asset_ids: &[Uuid],
) -> AppResult<Vec<Uuid>> {
    validate_asset_selection_input(chain_id, asset_ids)?;
    let mut deduped = Vec::new();
    let mut seen = HashSet::new();
    for asset_id in asset_ids {
        if seen.insert(*asset_id) {
            deduped.push(*asset_id);
        }
    }

    let assets = sqlx::query_as::<_, Asset>(VALIDATE_ASSETS_FOR_CHAIN_QUERY)
        .bind(&deduped)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    if assets.len() != deduped.len() {
        return Err(AppError::Validation("asset does not exist".to_string()));
    }
    for asset in &assets {
        validate_asset_chain_match(chain_id, asset)?;
    }
    Ok(deduped)
}

pub fn validate_asset_selection_input(_chain_id: Uuid, asset_ids: &[Uuid]) -> AppResult<()> {
    if asset_ids.is_empty() {
        return Err(AppError::Validation("asset_ids must not be empty".to_string()));
    }
    Ok(())
}

pub fn validate_asset_chain_match(chain_id: Uuid, asset: &Asset) -> AppResult<()> {
    if asset.chain_id != chain_id {
        return Err(AppError::Validation(
            "asset must belong to watched address chain".to_string(),
        ));
    }
    Ok(())
}
```

Add transaction helper after `validate_assets_for_chain`:

```rust
pub async fn replace_watched_address_assets(
    transaction: &mut Transaction<'_, Postgres>,
    address_id: Uuid,
    asset_ids: &[Uuid],
) -> AppResult<()> {
    sqlx::query(REPLACE_WATCHED_ADDRESS_ASSETS_DELETE_QUERY)
        .bind(address_id)
        .execute(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    sqlx::query(REPLACE_WATCHED_ADDRESS_ASSETS_INSERT_QUERY)
        .bind(address_id)
        .bind(asset_ids)
        .execute(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    Ok(())
}
```

- [ ] **Step 5: Run the helper tests to verify they pass**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage watched_address_asset_
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add backend/crates/storage/src/repositories.rs
git commit -m "$(cat <<'EOF'
Add watched address selected asset helpers
EOF
)"
```

### Task 4: Return asset ids from watched address repository flows

**Files:**
- Modify: `backend/crates/storage/src/repositories.rs`
- Modify: `backend/crates/api-server/src/routes.rs`

- [ ] **Step 1: Write failing repository response tests**

Update the top-level model import in `backend/crates/storage/src/repositories.rs` from:

```rust
ScanAddressContext, ScanCursor, Tenant, User, WatchedAddress,
```

to:

```rust
ScanAddressContext, ScanCursor, Tenant, User, WatchedAddress, WatchedAddressResponse,
```

Add these tests in the storage test module:

```rust
#[test]
fn watched_address_list_query_has_batch_asset_association_query() {
    assert!(WATCHED_ADDRESS_ASSET_IDS_FOR_ADDRESSES_QUERY.contains("address_id = ANY($1)"));
    assert!(WATCHED_ADDRESS_ASSET_IDS_FOR_ADDRESSES_QUERY.contains("ORDER BY address_id, created_at, asset_id"));
}

#[test]
fn watched_address_response_builder_attaches_asset_ids() {
    let address = coin_listener_core::models::WatchedAddress {
        id: uuid::Uuid::from_u128(10),
        tenant_id: uuid::Uuid::from_u128(1),
        chain_id: uuid::Uuid::from_u128(2),
        address: "0x0000000000000000000000000000000000000001".to_string(),
        label: Some("Main wallet".to_string()),
        priority: "normal".to_string(),
        scan_interval_seconds: 300,
        transfer_filter_enabled: true,
        balance_change_filter_enabled: true,
        status: "active".to_string(),
    };
    let response = super::watched_address_response(
        address,
        vec![uuid::Uuid::from_u128(0x101), uuid::Uuid::from_u128(0x102)],
    );

    assert_eq!(response.id, uuid::Uuid::from_u128(10));
    assert_eq!(response.asset_ids, vec![uuid::Uuid::from_u128(0x101), uuid::Uuid::from_u128(0x102)]);
}

#[allow(dead_code)]
async fn assert_list_watched_addresses_signature(
    pool: &PgPool,
    tenant_id: uuid::Uuid,
) -> AppResult<Vec<WatchedAddressResponse>> {
    super::list_watched_addresses(pool, tenant_id).await
}

#[allow(dead_code)]
async fn assert_create_watched_address_signature(
    pool: &PgPool,
    request: coin_listener_core::models::CreateWatchedAddressRequest,
) -> AppResult<WatchedAddressResponse> {
    super::create_watched_address(pool, request).await
}

#[allow(dead_code)]
async fn assert_update_watched_address_signature(
    pool: &PgPool,
    tenant_id: uuid::Uuid,
    id: uuid::Uuid,
    request: coin_listener_core::models::CreateWatchedAddressRequest,
) -> AppResult<WatchedAddressResponse> {
    super::update_watched_address(pool, tenant_id, id, request).await
}

#[test]
fn watched_address_repository_returns_response_model() {
    let _ = assert_list_watched_addresses_signature;
    let _ = assert_create_watched_address_signature;
    let _ = assert_update_watched_address_signature;
}
```

- [ ] **Step 2: Run repository tests to verify they fail**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage watched_address_
```

Expected: FAIL because response helpers and signatures are not implemented.

- [ ] **Step 3: Add batch association query and response builder**

Add this struct near the query constants:

```rust
#[derive(Debug, sqlx::FromRow)]
struct WatchedAddressAssetAssociation {
    address_id: Uuid,
    asset_id: Uuid,
}
```

Add this constant near `ASSET_IDS_FOR_ADDRESS_QUERY`:

```rust
pub const WATCHED_ADDRESS_ASSET_IDS_FOR_ADDRESSES_QUERY: &str = r#"
SELECT address_id, asset_id
FROM watched_address_assets
WHERE address_id = ANY($1)
ORDER BY address_id, created_at, asset_id
"#;
```

Add these helper functions after `asset_ids_for_address`:

```rust
pub fn watched_address_response(
    address: WatchedAddress,
    asset_ids: Vec<Uuid>,
) -> WatchedAddressResponse {
    WatchedAddressResponse {
        id: address.id,
        tenant_id: address.tenant_id,
        chain_id: address.chain_id,
        address: address.address,
        label: address.label,
        priority: address.priority,
        scan_interval_seconds: address.scan_interval_seconds,
        transfer_filter_enabled: address.transfer_filter_enabled,
        balance_change_filter_enabled: address.balance_change_filter_enabled,
        status: address.status,
        asset_ids,
    }
}

async fn asset_ids_by_address(
    pool: &PgPool,
    address_ids: &[Uuid],
) -> AppResult<std::collections::HashMap<Uuid, Vec<Uuid>>> {
    let rows = sqlx::query_as::<_, WatchedAddressAssetAssociation>(
        WATCHED_ADDRESS_ASSET_IDS_FOR_ADDRESSES_QUERY,
    )
    .bind(address_ids)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?;

    let mut grouped = std::collections::HashMap::<Uuid, Vec<Uuid>>::new();
    for row in rows {
        grouped.entry(row.address_id).or_default().push(row.asset_id);
    }
    Ok(grouped)
}
```

- [ ] **Step 4: Update list/create/update flows to use responses and transactions**

Replace `list_watched_addresses` with:

```rust
pub async fn list_watched_addresses(
    pool: &PgPool,
    tenant_id: Uuid,
) -> AppResult<Vec<WatchedAddressResponse>> {
    let addresses = sqlx::query_as::<_, WatchedAddress>(LIST_WATCHED_ADDRESSES_QUERY)
        .bind(tenant_id)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    let address_ids = addresses.iter().map(|address| address.id).collect::<Vec<_>>();
    let mut asset_ids = asset_ids_by_address(pool, &address_ids).await?;

    Ok(addresses
        .into_iter()
        .map(|address| {
            let ids = asset_ids.remove(&address.id).unwrap_or_default();
            watched_address_response(address, ids)
        })
        .collect())
}
```

Replace `create_watched_address` with:

```rust
pub async fn create_watched_address(
    pool: &PgPool,
    request: CreateWatchedAddressRequest,
) -> AppResult<WatchedAddressResponse> {
    let chain = get_chain(pool, request.chain_id).await?;
    validate_address_for_chain(&chain, &request.address)?;
    validate_watched_address(&request)?;
    let asset_ids = validate_assets_for_chain(pool, request.chain_id, &request.asset_ids).await?;

    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let address = sqlx::query_as::<_, WatchedAddress>(
        r#"
        INSERT INTO watched_addresses (
            tenant_id, chain_id, address, label, priority, scan_interval_seconds,
            transfer_filter_enabled, balance_change_filter_enabled, status
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        RETURNING id, tenant_id, chain_id, address, label, priority, scan_interval_seconds,
                  transfer_filter_enabled, balance_change_filter_enabled, status
        "#,
    )
    .bind(request.tenant_id.unwrap_or(DEFAULT_TENANT_ID))
    .bind(request.chain_id)
    .bind(request.address)
    .bind(request.label)
    .bind(request.priority)
    .bind(request.scan_interval_seconds)
    .bind(request.transfer_filter_enabled)
    .bind(request.balance_change_filter_enabled)
    .bind(request.status)
    .fetch_one(transaction.as_mut())
    .await
    .map_err(|error| AppError::Database(error.to_string()))?;

    replace_watched_address_assets(&mut transaction, address.id, &asset_ids).await?;

    transaction
        .commit()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    Ok(watched_address_response(address, asset_ids))
}
```

Replace `update_watched_address` with:

```rust
pub async fn update_watched_address(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
    request: CreateWatchedAddressRequest,
) -> AppResult<WatchedAddressResponse> {
    let chain = get_chain(pool, request.chain_id).await?;
    validate_address_for_chain(&chain, &request.address)?;
    validate_watched_address(&request)?;
    let asset_ids = validate_assets_for_chain(pool, request.chain_id, &request.asset_ids).await?;

    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let address = sqlx::query_as::<_, WatchedAddress>(UPDATE_WATCHED_ADDRESS_QUERY)
        .bind(id)
        .bind(request.chain_id)
        .bind(request.address)
        .bind(request.label)
        .bind(request.priority)
        .bind(request.scan_interval_seconds)
        .bind(request.transfer_filter_enabled)
        .bind(request.balance_change_filter_enabled)
        .bind(request.status)
        .bind(tenant_id)
        .fetch_optional(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("watched address".to_string()))?;

    replace_watched_address_assets(&mut transaction, address.id, &asset_ids).await?;

    transaction
        .commit()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    Ok(watched_address_response(address, asset_ids))
}
```

- [ ] **Step 5: Write failing API route regression test**

Add to `backend/crates/api-server/src/routes.rs` tests near the provider route test:

```rust
#[tokio::test]
async fn router_exposes_watched_address_crud_routes() {
    let app = build_router(test_state());

    for (method, uri) in [
        (Method::GET, "/api/addresses"),
        (Method::POST, "/api/addresses"),
        (Method::PUT, "/api/addresses/not-a-uuid"),
        (Method::DELETE, "/api/addresses/not-a-uuid"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");
    }
}
```

- [ ] **Step 6: Run route test and repository tests**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p api-server router_exposes_watched_address_crud_routes
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage watched_address_
```

Expected: both PASS after repository signatures are updated.

- [ ] **Step 7: Commit**

```bash
git add backend/crates/storage/src/repositories.rs backend/crates/api-server/src/routes.rs
git commit -m "$(cat <<'EOF'
Return selected asset ids with watched addresses
EOF
)"
```

### Task 5: Filter worker scans by selected assets

**Files:**
- Modify: `backend/crates/worker/src/lib.rs`

- [ ] **Step 1: Write failing worker selection helper tests**

Add this test module inside `#[cfg(test)] mod tests` in `backend/crates/worker/src/lib.rs`:

```rust
mod selected_asset_filters {
    use coin_listener_core::models::Asset;
    use uuid::Uuid;

    use crate::{asset_type_selected, native_asset_selected, selected_assets_by_type};

    fn asset(id: u128, asset_type: &str, symbol: &str, contract_address: Option<&str>) -> Asset {
        Asset {
            id: Uuid::from_u128(id),
            chain_id: Uuid::from_u128(2),
            asset_type: asset_type.to_string(),
            symbol: symbol.to_string(),
            name: symbol.to_string(),
            contract_address: contract_address.map(ToString::to_string),
            decimals: 18,
            is_builtin: true,
            status: "active".to_string(),
        }
    }

    #[test]
    fn native_asset_selected_only_when_native_id_is_present() {
        let native = asset(1, "native", "ETH", None);
        let usdt = asset(2, "erc20", "USDT", Some("0xdAC17F958D2ee523a2206206994597C13D831ec7"));

        assert!(native_asset_selected(&[native.clone(), usdt.clone()], &native));
        assert!(!native_asset_selected(&[usdt], &native));
    }

    #[test]
    fn selected_assets_by_type_filters_contract_assets() {
        let eth = asset(1, "native", "ETH", None);
        let usdt = asset(2, "erc20", "USDT", Some("0xdAC17F958D2ee523a2206206994597C13D831ec7"));
        let usdc = asset(3, "erc20", "USDC", Some("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"));

        let selected = selected_assets_by_type(&[eth, usdt.clone(), usdc.clone()], "erc20");

        assert_eq!(selected, vec![usdc, usdt]);
    }

    #[test]
    fn asset_type_selected_detects_any_selected_asset_of_type() {
        let btc = asset(1, "native", "BTC", None);
        assert!(asset_type_selected(&[btc], "native"));
        assert!(!asset_type_selected(&[], "native"));
    }
}
```

- [ ] **Step 2: Run worker helper tests to verify they fail**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p worker selected_asset_filters
```

Expected: FAIL because helper functions do not exist.

- [ ] **Step 3: Add selected asset helper functions**

Add after `ensure_provider_matches_context`:

```rust
pub fn native_asset_selected(selected_assets: &[Asset], native_asset: &Asset) -> bool {
    selected_assets.iter().any(|asset| asset.id == native_asset.id)
}

pub fn selected_assets_by_type(selected_assets: &[Asset], asset_type: &str) -> Vec<Asset> {
    let mut assets = selected_assets
        .iter()
        .filter(|asset| asset.asset_type == asset_type)
        .cloned()
        .collect::<Vec<_>>();
    assets.sort_by(|left, right| left.symbol.cmp(&right.symbol).then(left.name.cmp(&right.name)));
    assets
}

pub fn asset_type_selected(selected_assets: &[Asset], asset_type: &str) -> bool {
    selected_assets.iter().any(|asset| asset.asset_type == asset_type)
}
```

- [ ] **Step 4: Update EVM scans to use selected assets**

Change `scan_evm_erc20_transfers` signature:

```rust
pub async fn scan_evm_erc20_transfers(
    pool: &PgPool,
    rpc: &EvmRpcClient,
    context: &ScanAddressContext,
    latest_block: i64,
    default_confirmations: i32,
    selected_assets: &[Asset],
) -> AppResult<Vec<AddressEvent>> {
```

Replace the current `active_erc20_assets_for_chain` lookup with:

```rust
let assets = selected_assets_by_type(selected_assets, "erc20");
if assets.is_empty() {
    return Ok(Vec::new());
}
```

In `scan_evm_address_with_provider`, replace native/all-assets loading and scan calls with:

```rust
let native_asset = repositories::native_asset_for_chain(pool, context.chain_id).await?;
let selected_assets = repositories::selected_assets_for_address(pool, context.id).await?;
let timeout = provider_timeout_duration(provider)?;
let rpc = EvmRpcClient::new(provider.base_url.clone(), timeout);
let latest_block = rpc.eth_block_number().await?;

let mut events = Vec::new();
if native_asset_selected(&selected_assets, &native_asset) {
    if let Some(event) = scan_evm_native_balance_with_context(
        pool,
        &rpc,
        context,
        &native_asset,
        provider,
        latest_block,
    )
    .await?
    {
        events.push(event);
    }
}
events.extend(
    scan_evm_erc20_transfers(
        pool,
        &rpc,
        context,
        latest_block,
        chain.default_confirmations,
        &selected_assets,
    )
    .await?,
);
```

- [ ] **Step 5: Update TRON scans to use selected assets**

At the beginning of `scan_tron_address_with_provider`, after the native asset load, add:

```rust
let selected_assets = repositories::selected_assets_for_address(pool, context.id).await?;
```

Wrap TRX account transaction page scanning in:

```rust
if native_asset_selected(&selected_assets, &native_asset) {
    loop {
        // existing TRX account transaction page loop body
    }
}
```

Replace TRC20 lookup:

```rust
let trc20_assets = selected_assets_by_type(&selected_assets, "trc20");
```

Leave cursor behavior unchanged: update the TRX cursor only when `trx_cursor_value` exists and update the TRC20 cursor only when matching selected TRC20 transfers exist.

- [ ] **Step 6: Update BTC scan to honor selected native asset**

At the beginning of `scan_btc_address_with_provider`, after native asset load, add:

```rust
let selected_assets = repositories::selected_assets_for_address(pool, context.id).await?;
if !native_asset_selected(&selected_assets, &native_asset) {
    return Ok(Vec::new());
}
```

- [ ] **Step 7: Add source-level worker regression tests**

Add these tests near existing source-level worker tests:

```rust
#[test]
fn evm_scan_uses_selected_assets_for_native_and_erc20_paths() {
    let source = include_str!("lib.rs");
    let start = source
        .find("pub async fn scan_evm_address_with_provider(")
        .expect("EVM provider function");
    let end = source
        .find("pub async fn scan_evm_address(")
        .expect("EVM scan function");
    let evm = &source[start..end];

    assert!(evm.contains("selected_assets_for_address(pool, context.id).await?"));
    assert!(evm.contains("native_asset_selected(&selected_assets, &native_asset)"));
    assert!(evm.contains("&selected_assets"));
}

#[test]
fn worker_no_longer_scans_all_active_assets_for_transfer_paths() {
    let source = include_str!("lib.rs");
    let end = source.find("#[cfg(test)]").expect("test module");
    let production = &source[..end];

    assert!(!production.contains("active_erc20_assets_for_chain(pool, context.chain_id).await?"));
    assert!(!production.contains("active_assets_for_chain_by_type(pool, context.chain_id, \"trc20\").await?"));
    assert!(production.contains("selected_assets_by_type(selected_assets, \"erc20\")"));
    assert!(production.contains("selected_assets_by_type(&selected_assets, \"trc20\")"));
}

#[test]
fn btc_scan_skips_when_native_asset_is_not_selected() {
    let source = include_str!("lib.rs");
    let start = source
        .find("pub async fn scan_btc_address_with_provider(")
        .expect("BTC provider function");
    let end = source
        .find("pub async fn scan_btc_address(")
        .expect("BTC scan function");
    let btc = &source[start..end];

    assert!(btc.contains("selected_assets_for_address(pool, context.id).await?"));
    assert!(btc.contains("if !native_asset_selected(&selected_assets, &native_asset)"));
    assert!(btc.contains("return Ok(Vec::new())"));
}
```

- [ ] **Step 8: Run worker tests to verify they pass**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p worker selected_asset_filters
cargo test --manifest-path backend/Cargo.toml -p worker selected_assets
cargo test --manifest-path backend/Cargo.toml -p worker active_assets
cargo test --manifest-path backend/Cargo.toml -p worker btc_scan_skips
```

Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add backend/crates/worker/src/lib.rs
git commit -m "$(cat <<'EOF'
Filter worker scans by selected watched assets
EOF
)"
```

### Task 6: Update frontend watched address types and API expectations

**Files:**
- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/ui-regression.test.ts`

- [ ] **Step 1: Write failing frontend type regression test**

Add this test to `frontend/src/ui-regression.test.ts`:

```ts
test('watched address API types include selected asset ids', () => {
  const types = readSource('api/types.ts');
  const client = readSource('api/client.ts');

  expectContains(types, 'asset_ids: string[]');
  expectContains(types, "export type CreateWatchedAddressRequest = Omit<WatchedAddress, 'id' | 'tenant_id'>");
  expectContains(client, 'createWatchedAddress');
  expectContains(client, 'updateWatchedAddress');
});
```

- [ ] **Step 2: Run frontend regression test to verify it fails**

Run:

```bash
npm run test:ui-regression --prefix frontend
```

Expected: FAIL because `WatchedAddress` lacks `asset_ids`.

- [ ] **Step 3: Add `asset_ids` to the TypeScript watched address model**

Change `WatchedAddress` in `frontend/src/api/types.ts` to include:

```ts
  asset_ids: string[];
```

The full type should be:

```ts
export type WatchedAddress = {
  id: string;
  tenant_id: string;
  chain_id: string;
  address: string;
  label?: string | null;
  priority: string;
  scan_interval_seconds: number;
  transfer_filter_enabled: boolean;
  balance_change_filter_enabled: boolean;
  status: string;
  asset_ids: string[];
};
```

Keep this existing request type shape so create/update payloads inherit `asset_ids`:

```ts
export type CreateWatchedAddressRequest = Omit<WatchedAddress, 'id' | 'tenant_id'> & {
  tenant_id?: string;
};
```

- [ ] **Step 4: Run frontend regression test to verify it passes**

Run:

```bash
npm run test:ui-regression --prefix frontend
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/api/types.ts frontend/src/ui-regression.test.ts
git commit -m "$(cat <<'EOF'
Add selected asset ids to watched address frontend types
EOF
)"
```

### Task 7: Add frontend chain asset selectors and watched asset table column

**Files:**
- Modify: `frontend/src/pages/AddressesPage.tsx`
- Modify: `frontend/src/ui-regression.test.ts`

- [ ] **Step 1: Write failing UI regression test for asset selection**

Add this test to `frontend/src/ui-regression.test.ts`:

```ts
test('watched address form supports multi-chain asset selection', () => {
  const page = readSource('pages/AddressesPage.tsx');

  expectContains(page, 'listAssets');
  expectContains(page, 'chainRows');
  expectContains(page, 'assetOptionsForChain');
  expectContains(page, 'multiple');
  expectContains(page, 'asset_ids');
  expectContains(page, '监听资产');
  expectContains(page, '新增链配置');
  expectContains(page, '编辑监听地址');
  expectContains(page, 'updateWatchedAddress');
  expectContains(page, 'Promise.allSettled');
  expectContains(page, '部分链配置添加失败');
});
```

- [ ] **Step 2: Run frontend regression test to verify it fails**

Run:

```bash
npm run test:ui-regression --prefix frontend
```

Expected: FAIL because `AddressesPage.tsx` has only a single chain select and no asset selector.

- [ ] **Step 3: Replace imports in `AddressesPage.tsx`**

Change imports to:

```tsx
import { useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Button, Card, Form, Modal, Popconfirm, Space, Table, Tag, Toast } from '@douyinfe/semi-ui';
import {
  createWatchedAddress,
  deleteWatchedAddress,
  listAssets,
  listChains,
  listWatchedAddresses,
  updateWatchedAddress,
} from '../api/client';
import type { Asset, CreateWatchedAddressRequest, WatchedAddress } from '../api/types';
```

- [ ] **Step 4: Add chain row state and asset helper functions**

Inside `AddressesPage`, add state and queries:

```tsx
const [editingAddress, setEditingAddress] = useState<WatchedAddress | null>(null);
const [chainRows, setChainRows] = useState([{ id: crypto.randomUUID(), chain_id: '', asset_ids: [] as string[] }]);
const assetsQuery = useQuery({ queryKey: ['assets'], queryFn: listAssets });
const assetMap = useMemo(() => new Map((assetsQuery.data ?? []).map(asset => [asset.id, asset])), [assetsQuery.data]);
```

Add helper functions before `return`:

```tsx
function assetLabel(asset: Asset) {
  return asset.contract_address ? `${asset.symbol} (${asset.asset_type})` : asset.symbol;
}

function assetOptionsForChain(chainId: string) {
  return (assetsQuery.data ?? [])
    .filter(asset => asset.chain_id === chainId && asset.status === 'active')
    .map(asset => ({ value: asset.id, label: assetLabel(asset) }));
}

function selectedAssetSymbols(assetIds: string[]) {
  if (assetIds.length === 0) {
    return '-';
  }
  return assetIds.map(assetId => assetMap.get(assetId)?.symbol ?? assetId).join(', ');
}

function emptyChainRow() {
  return { id: crypto.randomUUID(), chain_id: '', asset_ids: [] as string[] };
}

function resetCreateForm() {
  setEditingAddress(null);
  setChainRows([emptyChainRow()]);
}

function openCreateModal() {
  resetCreateForm();
  setVisible(true);
}

function openEditModal(address: WatchedAddress) {
  setEditingAddress(address);
  setChainRows([{ id: crypto.randomUUID(), chain_id: address.chain_id, asset_ids: address.asset_ids }]);
  setVisible(true);
}

function closeModal() {
  setVisible(false);
  resetCreateForm();
}

function addChainRow() {
  setChainRows(rows => [...rows, emptyChainRow()]);
}

function removeChainRow(rowId: string) {
  setChainRows(rows => rows.length === 1 ? rows : rows.filter(row => row.id !== rowId));
}

function updateChainRow(rowId: string, patch: Partial<{ chain_id: string; asset_ids: string[] }>) {
  setChainRows(rows => rows.map(row => row.id === rowId ? { ...row, ...patch } : row));
}

function basePayload(values: Record<string, unknown>) {
  return {
    address: String(values.address),
    label: values.label ? String(values.label) : null,
    priority: String(values.priority),
    scan_interval_seconds: Number(values.scan_interval_seconds),
    transfer_filter_enabled: Boolean(values.transfer_filter_enabled),
    balance_change_filter_enabled: Boolean(values.balance_change_filter_enabled),
    status: String(values.status),
  };
}
```

- [ ] **Step 5: Replace create mutation with save mutation**

Replace the old `createMutation` with:

```tsx
const saveMutation = useMutation({
  mutationFn: async (values: Record<string, unknown>) => {
    const base = basePayload(values);
    if (editingAddress) {
      const row = chainRows[0];
      if (!row.asset_ids.length) {
        throw new Error('每条链至少选择一个资产');
      }
      return updateWatchedAddress(editingAddress.id, {
        ...base,
        chain_id: editingAddress.chain_id,
        asset_ids: row.asset_ids,
      } satisfies CreateWatchedAddressRequest);
    }

    for (const row of chainRows) {
      if (!row.chain_id) {
        throw new Error('请选择链');
      }
      if (!row.asset_ids.length) {
        throw new Error('每条链至少选择一个资产');
      }
    }

    const results = await Promise.allSettled(chainRows.map(row => createWatchedAddress({
      ...base,
      chain_id: row.chain_id,
      asset_ids: row.asset_ids,
    } satisfies CreateWatchedAddressRequest)));
    const failures = results.filter(result => result.status === 'rejected');
    if (failures.length > 0) {
      throw new Error(`部分链配置添加失败：${failures.length}/${results.length}`);
    }
    return results;
  },
  onSuccess: () => {
    Toast.success(editingAddress ? '地址已更新' : '地址已添加');
    closeModal();
    queryClient.invalidateQueries({ queryKey: ['addresses'] });
  },
  onError: error => {
    Toast.error(error instanceof Error ? error.message : '保存失败');
    queryClient.invalidateQueries({ queryKey: ['addresses'] });
  },
});
```

Update `handleSubmit`:

```tsx
function handleSubmit(values: Record<string, unknown>) {
  saveMutation.mutate(values);
}
```

- [ ] **Step 6: Update table columns**

Change the header button:

```tsx
<Card title="监听地址" headerExtraContent={<Button onClick={openCreateModal}>新增地址</Button>}>
```

Add this column after the address column:

```tsx
{ title: '监听资产', dataIndex: 'asset_ids', width: 180, ellipsis: { showTitle: true }, render: value => selectedAssetSymbols(value as string[]) },
```

Change the operations column to include edit:

```tsx
render: (_, record) => (
  <Space>
    <Button theme="borderless" onClick={() => openEditModal(record)}>编辑</Button>
    <Popconfirm title="确认删除该地址？" onConfirm={() => deleteMutation.mutate(record.id)}>
      <Button type="danger" theme="borderless">删除</Button>
    </Popconfirm>
  </Space>
),
```

- [ ] **Step 7: Update the modal form**

Replace the modal and form body with:

```tsx
<Modal title={editingAddress ? '编辑监听地址' : '新增监听地址'} visible={visible} onCancel={closeModal} footer={null}>
  <Form
    onSubmit={handleSubmit}
    initValues={editingAddress ?? {
      priority: 'normal',
      scan_interval_seconds: 300,
      transfer_filter_enabled: true,
      balance_change_filter_enabled: true,
      status: 'active',
    }}
  >
    <Form.Input field="address" label="地址" disabled={Boolean(editingAddress)} rules={[{ required: true, message: '请输入地址' }]} />
    <Form.Input field="label" label="标签" />
    <Form.Select field="priority" label="优先级" initValue="normal">
      <Form.Select.Option value="normal">normal</Form.Select.Option>
      <Form.Select.Option value="high">high</Form.Select.Option>
      <Form.Select.Option value="critical">critical</Form.Select.Option>
    </Form.Select>
    <Form.InputNumber field="scan_interval_seconds" label="扫描间隔秒" initValue={300} min={10} />
    <Form.Switch field="transfer_filter_enabled" label="关注转账" initValue={true} />
    <Form.Switch field="balance_change_filter_enabled" label="关注余额变化" initValue={true} />
    <Form.Select field="status" label="状态" initValue="active">
      <Form.Select.Option value="active">active</Form.Select.Option>
      <Form.Select.Option value="paused">paused</Form.Select.Option>
    </Form.Select>

    <div className="address-chain-rows">
      {chainRows.map((row, index) => (
        <Space key={row.id} align="start" style={{ width: '100%', marginBottom: 12 }}>
          <Form.Select
            field={`chain_${row.id}`}
            label={index === 0 ? '链配置' : ' '}
            disabled={Boolean(editingAddress)}
            value={row.chain_id}
            placeholder="选择链"
            onChange={value => updateChainRow(row.id, { chain_id: String(value), asset_ids: [] })}
          >
            {(chainsQuery.data ?? []).map(chain => <Form.Select.Option key={chain.id} value={chain.id}>{chain.name}</Form.Select.Option>)}
          </Form.Select>
          <Form.Select
            field={`assets_${row.id}`}
            multiple
            filter
            label="资产"
            value={row.asset_ids}
            placeholder="选择资产"
            optionList={assetOptionsForChain(row.chain_id)}
            onChange={value => updateChainRow(row.id, { asset_ids: Array.isArray(value) ? value.map(String) : [] })}
          />
          {!editingAddress && <Button onClick={() => removeChainRow(row.id)} disabled={chainRows.length === 1}>移除</Button>}
        </Space>
      ))}
    </div>
    {!editingAddress && <Button onClick={addChainRow} theme="borderless">新增链配置</Button>}

    <Space style={{ marginTop: 16 }}>
      <Button htmlType="submit" type="primary" loading={saveMutation.isPending}>保存</Button>
      <Button onClick={closeModal}>取消</Button>
    </Space>
  </Form>
</Modal>
```

If TypeScript rejects `Form.Select` controlled `value` props because of Semi typing, replace the row controls with top-level `Select` imported from `@douyinfe/semi-ui` and keep the same `multiple`, `filter`, `optionList`, and `onChange` semantics.

- [ ] **Step 8: Run frontend tests and build**

Run:

```bash
npm run test:ui-regression --prefix frontend
npm run build --prefix frontend
```

Expected: PASS. Existing Vite warnings about lottie eval or chunk size may remain; do not change unrelated build config for those warnings.

- [ ] **Step 9: Commit**

```bash
git add frontend/src/pages/AddressesPage.tsx frontend/src/ui-regression.test.ts
git commit -m "$(cat <<'EOF'
Add watched address asset selection UI
EOF
)"
```

### Task 8: Add API-level request contract regressions

**Files:**
- Modify: `backend/crates/api-server/src/routes.rs`

- [ ] **Step 1: Write failing API request contract test**

Add this test in the `backend/crates/api-server/src/routes.rs` test module:

```rust
#[test]
fn watched_address_request_requires_asset_ids_field() {
    let missing_assets = r#"{
        "chain_id":"00000000-0000-0000-0000-000000000002",
        "address":"0x0000000000000000000000000000000000000001",
        "label":null,
        "priority":"normal",
        "scan_interval_seconds":300,
        "transfer_filter_enabled":true,
        "balance_change_filter_enabled":true,
        "status":"active"
    }"#;

    let error = serde_json::from_str::<coin_listener_core::models::CreateWatchedAddressRequest>(missing_assets)
        .expect_err("missing asset_ids should fail deserialization");

    assert!(error.to_string().contains("asset_ids"));
}

#[test]
fn watched_address_request_accepts_non_empty_asset_ids() {
    let payload = r#"{
        "chain_id":"00000000-0000-0000-0000-000000000002",
        "address":"0x0000000000000000000000000000000000000001",
        "label":null,
        "priority":"normal",
        "scan_interval_seconds":300,
        "transfer_filter_enabled":true,
        "balance_change_filter_enabled":true,
        "status":"active",
        "asset_ids":["00000000-0000-0000-0000-000000000101"]
    }"#;

    let request = serde_json::from_str::<coin_listener_core::models::CreateWatchedAddressRequest>(payload)
        .expect("request with asset_ids deserializes");

    assert_eq!(request.asset_ids, vec![uuid::Uuid::from_u128(0x101)]);
}
```

- [ ] **Step 2: Run API tests to verify they pass after model work**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p api-server watched_address_request_
```

Expected: PASS. If this fails because `asset_ids` still defaults to an empty vector on missing field, remove any serde default from the request model so missing `asset_ids` returns HTTP 422 at extraction and repository validation handles explicit empty lists with HTTP 400.

- [ ] **Step 3: Commit**

```bash
git add backend/crates/api-server/src/routes.rs
git commit -m "$(cat <<'EOF'
Add watched address asset API contract tests
EOF
)"
```

### Task 9: Run full verification and final review

**Files:**
- Verify all changed files.

- [ ] **Step 1: Run Rust formatting check**

Run:

```bash
cargo fmt --manifest-path backend/Cargo.toml --all -- --check
```

Expected: PASS. If it fails, run `cargo fmt --manifest-path backend/Cargo.toml --all`, inspect the diff, then re-run the check.

- [ ] **Step 2: Run backend tests**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 3: Run frontend regression tests**

Run:

```bash
npm run test:ui-regression --prefix frontend
```

Expected: PASS.

- [ ] **Step 4: Run frontend build**

Run:

```bash
npm run build --prefix frontend
```

Expected: PASS. Report any existing non-fatal Vite warnings without expanding scope.

- [ ] **Step 5: Check diff hygiene**

Run:

```bash
git diff --check
git status --short
git diff -- frontend/package-lock.json
```

Expected:
- `git diff --check`: PASS.
- `frontend/package-lock.json`: no diff.
- Only intended files are changed or committed; preserve unrelated pre-existing files if still untracked.

- [ ] **Step 6: Request final code review**

Dispatch `superpowers:code-reviewer` with:

```text
Review the watched-address selected assets implementation against docs/superpowers/specs/2026-05-21-coin-listener-watched-address-assets-design.md and docs/superpowers/plans/2026-05-21-coin-listener-watched-address-assets.md. Check database schema, API request/response contract, repository validation, worker asset filtering, frontend multi-chain asset selector, and tests. Report Critical/Important/Minor issues only.
```

Fix Critical and Important issues before completion. If feedback is technically incorrect for this codebase, document the reason and do not make speculative changes.

- [ ] **Step 7: Commit final fixes if any**

If review or verification required fixes, commit only the relevant files:

```bash
git add <specific-fixed-files>
git commit -m "$(cat <<'EOF'
Fix watched address asset selection follow-ups
EOF
)"
```

Expected: No unrelated files staged.

## Self-Review Checklist

- [ ] Spec coverage: migration, non-empty `asset_ids`, asset existence, asset-chain validation, dedupe, create/update replacement, list aggregation, worker filtering, frontend create/edit/list behavior, and cursor non-backfill behavior are all represented.
- [ ] TDD order: every behavior task writes or updates a failing test before implementation.
- [ ] Placeholder scan: no unfinished-marker text, deferred-action wording, or copy-by-reference task instructions remain.
- [ ] Type consistency: Rust uses `Vec<Uuid>` and TypeScript uses `string[]` for `asset_ids`; API returns `WatchedAddressResponse` from repository functions.
- [ ] Scope control: no asset management page, no batch backend endpoint, and no per-asset cursor backfill are included.
