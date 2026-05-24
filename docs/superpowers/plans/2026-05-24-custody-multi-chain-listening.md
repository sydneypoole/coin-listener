# Custody Multi-Chain Listening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade custody accounts so one custody account can represent one address with multiple chain configurations and multiple selected assets per chain, and assignment automatically enables all configured watched addresses.

**Architecture:** Add normalized custody chain-config tables under `custody_accounts`, keep legacy single-chain fields only as compatibility data, and make storage read/write expanded configs. Assignment remains one custody assignment per business reference, but it ensures a watched address per configured chain and binds all selected assets in one transaction. The frontend reuses the watched-address multi-chain asset-row pattern for custody account creation and user-provided assignment creation.

**Tech Stack:** Rust, Axum, SQLx, PostgreSQL migrations, React, TypeScript, TanStack Query, Semi UI, Node test runner.

---

## File Structure

**Create:**
- `backend/crates/storage/migrations/0021_custody_account_chain_configs.sql` — adds normalized chain config and asset mapping tables, backfills existing single-chain custody rows, and adds address-level uniqueness.

**Modify:**
- `backend/crates/core/src/models.rs` — adds custody chain config DTOs and multi-watched-address assignment response fields.
- `backend/crates/storage/src/custody_accounts.rs` — validates chain configs, stores config rows/assets, lists expanded accounts, and ensures all watched addresses during assignment.
- `backend/crates/api-server/src/routes.rs` — keeps custody endpoints and extends route source tests for the new multi-chain contract.
- `frontend/src/api/types.ts` — adds chain config request/response and watched-address assignment response types.
- `frontend/src/api/client.ts` — keeps existing client functions with updated request/response types.
- `frontend/src/pages/CustodyAccountsPage.tsx` — adds multi-chain asset rows to create and user-assignment flows; displays chain/asset summaries.
- `frontend/src/ui-regression.test.ts` — covers multi-chain custody contracts and UI wiring.

**Reference only:**
- `frontend/src/pages/AddressesPage.tsx` — reuse `ChainRow`, active asset filtering, duplicate chain prevention, and multi-select asset UX patterns.
- `backend/crates/storage/src/repositories.rs` — reuse address and asset validation patterns.

---

### Task 1: Add multi-chain custody DTOs and migration

**Files:**
- Create: `backend/crates/storage/migrations/0021_custody_account_chain_configs.sql`
- Modify: `backend/crates/core/src/models.rs:96-169`
- Modify: `backend/crates/storage/src/custody_accounts.rs:722-793`

- [ ] **Step 1: Write failing core DTO and migration tests**

In `backend/crates/core/src/models.rs`, extend `mod custody_model_tests` with this test:

```rust
    #[test]
    fn custody_models_include_multi_chain_configs_and_watched_addresses() {
        use crate::models::{
            AssignCustodyAccountRequest, AssignCustodyAccountResponse,
            CreateCustodyAccountRequest, CustodyAccount, CustodyAccountAssignment,
            CustodyAccountChainConfig, CustodyAccountChainConfigRequest,
            CustodyAssignmentWatchedAddress,
        };
        use chrono::{TimeZone, Utc};
        use uuid::Uuid;

        let now = Utc.with_ymd_and_hms(2026, 5, 24, 12, 0, 0).unwrap();
        let chain_config = CustodyAccountChainConfig {
            id: Uuid::from_u128(10),
            chain_id: Uuid::from_u128(3),
            chain_name: "Ethereum".to_string(),
            asset_ids: vec![Uuid::from_u128(11), Uuid::from_u128(12)],
            asset_symbols: vec!["ETH".to_string(), "USDT".to_string()],
        };
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
            chain_configs: vec![chain_config.clone()],
            created_at: now,
            updated_at: now,
        };
        let account_json = serde_json::to_value(account).unwrap();
        assert_eq!(account_json["chain_configs"][0]["chain_name"], "Ethereum");
        assert_eq!(account_json["chain_configs"][0]["asset_symbols"][1], "USDT");

        let create_request = CreateCustodyAccountRequest {
            chain_id: Uuid::from_u128(3),
            address: "0x0000000000000000000000000000000000000001".to_string(),
            label: Some("池地址 1".to_string()),
            source: "pool".to_string(),
            status: Some("available".to_string()),
            chain_configs: vec![CustodyAccountChainConfigRequest {
                chain_id: Uuid::from_u128(3),
                asset_ids: vec![Uuid::from_u128(11), Uuid::from_u128(12)],
            }],
        };
        assert_eq!(create_request.chain_configs[0].asset_ids.len(), 2);

        let assign_request = AssignCustodyAccountRequest {
            chain_id: Some(Uuid::from_u128(3)),
            source: "user".to_string(),
            address: Some("0x0000000000000000000000000000000000000001".to_string()),
            applicant_type: "api".to_string(),
            business_ref: "order_10001".to_string(),
            purpose: Some("deposit_address".to_string()),
            chain_configs: Some(vec![CustodyAccountChainConfigRequest {
                chain_id: Uuid::from_u128(3),
                asset_ids: vec![Uuid::from_u128(11)],
            }]),
        };
        assert_eq!(assign_request.chain_configs.unwrap()[0].asset_ids[0], Uuid::from_u128(11));

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
        let response = AssignCustodyAccountResponse {
            account: CustodyAccount {
                id: Uuid::from_u128(1),
                tenant_id: Uuid::from_u128(2),
                chain_id: Uuid::from_u128(3),
                chain_name: "Ethereum".to_string(),
                address: "0x0000000000000000000000000000000000000001".to_string(),
                label: None,
                source: "pool".to_string(),
                status: "assigned".to_string(),
                watched_address_id: Some(Uuid::from_u128(4)),
                current_assignment_id: Some(Uuid::from_u128(6)),
                current_business_ref: Some("order_10001".to_string()),
                chain_configs: vec![chain_config],
                created_at: now,
                updated_at: now,
            },
            assignment,
            watched_addresses: vec![CustodyAssignmentWatchedAddress {
                chain_id: Uuid::from_u128(3),
                chain_name: "Ethereum".to_string(),
                watched_address_id: Uuid::from_u128(4),
                asset_ids: vec![Uuid::from_u128(11), Uuid::from_u128(12)],
            }],
        };
        let response_json = serde_json::to_value(response).unwrap();
        assert_eq!(response_json["watched_addresses"][0]["asset_ids"].as_array().unwrap().len(), 2);
    }
```

In `backend/crates/storage/src/custody_accounts.rs`, extend `custody_migration_defines_unique_accounts_and_single_active_assignment`:

```rust
        let chain_config_migration = include_str!("../migrations/0021_custody_account_chain_configs.sql");
        assert!(chain_config_migration.contains("CREATE TABLE IF NOT EXISTS custody_account_chain_configs"));
        assert!(chain_config_migration.contains("CREATE TABLE IF NOT EXISTS custody_account_chain_config_assets"));
        assert!(chain_config_migration.contains("UNIQUE (tenant_id, custody_account_id, chain_id)"));
        assert!(chain_config_migration.contains("idx_custody_accounts_tenant_address_normalized"));
        assert!(chain_config_migration.contains("INSERT INTO custody_account_chain_configs"));
        assert!(chain_config_migration.contains("INSERT INTO custody_account_chain_config_assets"));
```

- [ ] **Step 2: Run tests to verify RED**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-core custody_models_include_multi_chain_configs_and_watched_addresses -- --nocapture
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage custody_migration_defines_unique_accounts_and_single_active_assignment -- --nocapture
```

Expected:

- Core test fails with unresolved imports or missing struct fields such as `CustodyAccountChainConfigRequest`, `CustodyAccountChainConfig`, `CustodyAssignmentWatchedAddress`, `chain_configs`, and `watched_addresses`.
- Storage test fails because `0021_custody_account_chain_configs.sql` does not exist.

- [ ] **Step 3: Implement DTOs**

In `backend/crates/core/src/models.rs`, add these structs before `CustodyAccount`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustodyAccountChainConfigRequest {
    pub chain_id: Uuid,
    pub asset_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustodyAccountChainConfig {
    pub id: Uuid,
    pub chain_id: Uuid,
    pub chain_name: String,
    pub asset_ids: Vec<Uuid>,
    pub asset_symbols: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustodyAssignmentWatchedAddress {
    pub chain_id: Uuid,
    pub chain_name: String,
    pub watched_address_id: Uuid,
    pub asset_ids: Vec<Uuid>,
}
```

Update `CustodyAccount`:

```rust
    pub chain_configs: Vec<CustodyAccountChainConfig>,
```

Update `CreateCustodyAccountRequest`:

```rust
    pub chain_configs: Vec<CustodyAccountChainConfigRequest>,
```

Update `AssignCustodyAccountRequest`:

```rust
    pub chain_id: Option<Uuid>,
    pub chain_configs: Option<Vec<CustodyAccountChainConfigRequest>>,
```

Update `AssignCustodyAccountResponse`:

```rust
    pub watched_addresses: Vec<CustodyAssignmentWatchedAddress>,
```

Update the older `custody_models_serialize_expected_status_and_source_fields` test in the same file so every `CustodyAccount` literal includes:

```rust
            chain_configs: vec![],
```

and every `CreateCustodyAccountRequest` literal includes:

```rust
            chain_configs: vec![CustodyAccountChainConfigRequest {
                chain_id: Uuid::from_u128(3),
                asset_ids: vec![Uuid::from_u128(4)],
            }],
```

and every `AssignCustodyAccountRequest` literal uses:

```rust
            chain_id: Some(Uuid::from_u128(3)),
            chain_configs: None,
```

- [ ] **Step 4: Add migration**

Create `backend/crates/storage/migrations/0021_custody_account_chain_configs.sql`:

```sql
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM custody_accounts
        GROUP BY tenant_id, address_normalized
        HAVING COUNT(*) > 1
    ) THEN
        RAISE EXCEPTION 'duplicate custody account normalized addresses must be merged before applying multi-chain custody migration';
    END IF;
END $$;

CREATE TABLE IF NOT EXISTS custody_account_chain_configs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    custody_account_id UUID NOT NULL,
    chain_id UUID NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, custody_account_id, chain_id),
    UNIQUE (id, tenant_id),
    FOREIGN KEY (custody_account_id, tenant_id)
        REFERENCES custody_accounts(id, tenant_id)
        ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS custody_account_chain_config_assets (
    chain_config_id UUID NOT NULL REFERENCES custody_account_chain_configs(id) ON DELETE CASCADE,
    asset_id UUID NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    PRIMARY KEY (chain_config_id, asset_id)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_custody_accounts_tenant_address_normalized
ON custody_accounts(tenant_id, address_normalized);

CREATE INDEX IF NOT EXISTS idx_custody_chain_configs_tenant_account
ON custody_account_chain_configs(tenant_id, custody_account_id);

INSERT INTO custody_account_chain_configs (tenant_id, custody_account_id, chain_id)
SELECT tenant_id, id, chain_id
FROM custody_accounts
ON CONFLICT (tenant_id, custody_account_id, chain_id) DO NOTHING;

INSERT INTO custody_account_chain_config_assets (chain_config_id, asset_id)
SELECT config.id, asset.id
FROM custody_account_chain_configs config
JOIN assets asset ON asset.chain_id = config.chain_id
WHERE asset.asset_type = 'native'
  AND asset.status = 'active'
ON CONFLICT DO NOTHING;
```

- [ ] **Step 5: Run tests to verify GREEN**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-core custody_models_include_multi_chain_configs_and_watched_addresses -- --nocapture
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage custody_migration_defines_unique_accounts_and_single_active_assignment -- --nocapture
```

Expected: both commands pass.

- [ ] **Step 6: Commit**

```bash
git add backend/crates/core/src/models.rs backend/crates/storage/src/custody_accounts.rs backend/crates/storage/migrations/0021_custody_account_chain_configs.sql
git commit -m "添加托管账户多链配置模型"
```

---

### Task 2: Add custody chain-config storage validation and account listing

**Files:**
- Modify: `backend/crates/storage/src/custody_accounts.rs`

- [ ] **Step 1: Write failing storage tests**

Add these tests inside `#[cfg(test)] mod tests` in `backend/crates/storage/src/custody_accounts.rs`:

```rust
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
        assert!(INSERT_CUSTODY_ACCOUNT_CHAIN_CONFIG_ASSET_QUERY.contains("custody_account_chain_config_assets"));
        assert!(INSERT_USER_CUSTODY_ACCOUNT_FOR_ASSIGNMENT_QUERY.contains("ON CONFLICT (tenant_id, address_normalized) DO NOTHING"));
        assert!(SELECT_USER_CUSTODY_ACCOUNT_FOR_UPDATE_QUERY.contains("address_normalized = $2"));
        assert!(CLAIM_AVAILABLE_POOL_ACCOUNT_QUERY.contains("source = 'pool'"));
        assert!(!CLAIM_AVAILABLE_POOL_ACCOUNT_QUERY.contains("chain_id = $2"));
    }
```

- [ ] **Step 2: Run tests to verify RED**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage custody_chain_config -- --nocapture
```

Expected: fails because `validate_custody_chain_config_shape`, `LIST_CUSTODY_ACCOUNT_CONFIGS_QUERY`, `INSERT_CUSTODY_ACCOUNT_CHAIN_CONFIG_QUERY`, and `INSERT_CUSTODY_ACCOUNT_CHAIN_CONFIG_ASSET_QUERY` do not exist, and existing SQL still uses `(tenant_id, chain_id, address_normalized)` for user insert conflicts.

- [ ] **Step 3: Add imports and row structs**

In `backend/crates/storage/src/custody_accounts.rs`, update imports:

```rust
use coin_listener_core::models::{
    AssignCustodyAccountRequest, AssignCustodyAccountResponse, CreateCustodyAccountRequest,
    CreateWatchedAddressRequest, CustodyAccount, CustodyAccountAssignment,
    CustodyAccountChainConfig, CustodyAccountChainConfigRequest,
    CustodyAssignmentQuery, CustodyAssignmentWatchedAddress, CustodyAccountQuery,
    WatchedAddress,
};
use std::collections::{HashMap, HashSet};
```

Add row structs near `CustodyAccountRow`:

```rust
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
```

- [ ] **Step 4: Add chain-config SQL constants**

Add constants after `LIST_CUSTODY_ASSIGNMENTS_QUERY`:

```rust
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
```

Change `CLAIM_AVAILABLE_POOL_ACCOUNT_QUERY` to remove the chain filter:

```rust
pub const CLAIM_AVAILABLE_POOL_ACCOUNT_QUERY: &str = r#"
SELECT id, tenant_id, chain_id, address, label, source, status, watched_address_id, created_at, updated_at
FROM custody_accounts
WHERE tenant_id = $1
  AND source = 'pool'
  AND status = 'available'
ORDER BY created_at ASC
FOR UPDATE SKIP LOCKED
LIMIT 1
"#;
```

Change `SELECT_USER_CUSTODY_ACCOUNT_FOR_UPDATE_QUERY`:

```rust
pub const SELECT_USER_CUSTODY_ACCOUNT_FOR_UPDATE_QUERY: &str = r#"
SELECT id, tenant_id, chain_id, address, label, source, status, watched_address_id, created_at, updated_at
FROM custody_accounts
WHERE tenant_id = $1
  AND address_normalized = $2
FOR UPDATE
"#;
```

Change `INSERT_USER_CUSTODY_ACCOUNT_FOR_ASSIGNMENT_QUERY` conflict target:

```rust
ON CONFLICT (tenant_id, address_normalized) DO NOTHING
```

- [ ] **Step 5: Implement shape validation and normalization helpers**

Add these helpers before public functions:

```rust
fn validate_custody_chain_config_shape(
    configs: &[CustodyAccountChainConfigRequest],
) -> AppResult<()> {
    if configs.is_empty() {
        return Err(AppError::Validation("at least one custody chain config is required".to_string()));
    }

    let mut chain_ids = HashSet::new();
    for config in configs {
        if !chain_ids.insert(config.chain_id) {
            return Err(AppError::Validation("custody chain configs cannot repeat chain_id".to_string()));
        }
        if config.asset_ids.is_empty() {
            return Err(AppError::Validation("each custody chain config requires at least one asset".to_string()));
        }
        let mut asset_ids = HashSet::new();
        for asset_id in &config.asset_ids {
            if !asset_ids.insert(*asset_id) {
                return Err(AppError::Validation("custody chain config asset_ids cannot contain duplicates".to_string()));
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
```

Add async helper skeletons in this task; Task 3 will reuse them for assignment:

```rust
async fn chain_types_for_configs(
    transaction: &mut Transaction<'_, Postgres>,
    configs: &[CustodyAccountChainConfigRequest],
) -> AppResult<HashMap<Uuid, String>> {
    let chain_ids = configs.iter().map(|config| config.chain_id).collect::<Vec<_>>();
    let rows = sqlx::query_as::<_, ChainTypeRow>(SELECT_CHAIN_TYPES_FOR_CONFIGS_QUERY)
        .bind(&chain_ids)
        .fetch_all(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    let chain_types = rows.into_iter().map(|row| (row.id, row.chain_type)).collect::<HashMap<_, _>>();
    if chain_types.len() != chain_ids.len() {
        return Err(AppError::Validation("custody chain config contains unknown chain".to_string()));
    }
    Ok(chain_types)
}
```

- [ ] **Step 6: Validate assets and persist configs**

Add this helper:

```rust
async fn validate_and_insert_custody_chain_configs(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    custody_account_id: Uuid,
    configs: &[CustodyAccountChainConfigRequest],
) -> AppResult<()> {
    validate_custody_chain_config_shape(configs)?;

    for config in configs {
        repositories::validate_assets_for_chain(transaction, config.chain_id, &config.asset_ids).await?;
        let inserted = sqlx::query_as::<_, InsertedChainConfigRow>(
            INSERT_CUSTODY_ACCOUNT_CHAIN_CONFIG_QUERY,
        )
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
```

If `repositories::validate_assets_for_chain` is private, change it in `backend/crates/storage/src/repositories.rs` from private to:

```rust
pub(crate) async fn validate_assets_for_chain(
```

- [ ] **Step 7: Expand account results with chain configs**

Add helpers:

```rust
async fn configs_for_accounts(
    pool: &PgPool,
    tenant_id: Uuid,
    account_ids: &[Uuid],
) -> AppResult<HashMap<Uuid, Vec<CustodyAccountChainConfig>>> {
    if account_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let rows = sqlx::query_as::<_, CustodyAccountChainConfigRow>(LIST_CUSTODY_ACCOUNT_CONFIGS_QUERY)
        .bind(tenant_id)
        .bind(account_ids)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let mut grouped: HashMap<Uuid, Vec<CustodyAccountChainConfig>> = HashMap::new();
    for row in rows {
        grouped.entry(row.custody_account_id).or_default().push(CustodyAccountChainConfig {
            id: row.id,
            chain_id: row.chain_id,
            chain_name: row.chain_name,
            asset_ids: row.asset_ids,
            asset_symbols: row.asset_symbols,
        });
    }
    Ok(grouped)
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
```

Replace the existing `custody_account_from_row` signature and all call sites. For list calls, fetch rows into a row struct first, collect IDs, fetch config map, then map accounts with config vectors.

- [ ] **Step 8: Update create custody account to write configs in one transaction**

Change `create_custody_account` to begin a transaction and use the first config chain as legacy `chain_id`:

```rust
    validate_custody_source(&request.source)?;
    validate_custody_chain_config_shape(&request.chain_configs)?;
    let status = request.status.unwrap_or_else(|| {
        if request.source == CUSTODY_SOURCE_POOL {
            CUSTODY_ACCOUNT_STATUS_AVAILABLE.to_string()
        } else {
            CUSTODY_ACCOUNT_STATUS_ASSIGNED.to_string()
        }
    });
    validate_custody_account_status(&status)?;
    let address = request.address.trim().to_string();

    let mut transaction = pool.begin().await.map_err(|error| AppError::Database(error.to_string()))?;
    let chain_types = chain_types_for_configs(&mut transaction, &request.chain_configs).await?;
    for config in &request.chain_configs {
        let chain = repositories::get_chain(pool, config.chain_id).await?;
        repositories::validate_address_for_chain(&chain, &address)?;
    }
    let chain_type_values = chain_types.values().cloned().collect::<Vec<_>>();
    let normalized = normalize_custody_account_address(&address, &chain_type_values);
    let legacy_chain_id = request.chain_configs[0].chain_id;
    let legacy_chain = repositories::get_chain(pool, legacy_chain_id).await?;

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

    validate_and_insert_custody_chain_configs(
        &mut transaction,
        tenant_id,
        row.id,
        &request.chain_configs,
    )
    .await?;

    transaction.commit().await.map_err(|error| AppError::Database(error.to_string()))?;
```

Then return the account by calling `list_custody_accounts` and finding `row.id`; this keeps response expansion in one path.

- [ ] **Step 9: Run storage tests to verify GREEN**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage custody_chain_config -- --nocapture
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage custody -- --nocapture
```

Expected: both pass.

- [ ] **Step 10: Commit**

```bash
git add backend/crates/storage/src/custody_accounts.rs backend/crates/storage/src/repositories.rs
git commit -m "支持托管账户链资产配置存储"
```

---

### Task 3: Apply all configured chains and assets during assignment

**Files:**
- Modify: `backend/crates/storage/src/custody_accounts.rs`
- Modify: `backend/crates/core/src/models.rs` only if Task 1 missed response fields

- [ ] **Step 1: Write failing assignment tests**

Add these source-level tests in `backend/crates/storage/src/custody_accounts.rs`:

```rust
    #[test]
    fn custody_assignment_queries_apply_all_configured_assets() {
        assert!(LIST_CUSTODY_ACCOUNT_CONFIGS_QUERY.contains("asset_ids"));
        assert!(ENSURE_WATCHED_ADDRESS_ASSET_QUERY.contains("ON CONFLICT DO NOTHING"));
        assert!(MARK_CUSTODY_ACCOUNT_ASSIGNED_QUERY.contains("watched_address_id = $2"));
        assert!(INSERT_CUSTODY_ASSIGNMENT_QUERY.contains("ON CONFLICT DO NOTHING"));
    }

    #[test]
    fn custody_account_normalization_prefers_evm_when_any_config_is_evm() {
        let evm_and_base = vec!["evm".to_string(), "evm".to_string()];
        assert_eq!(
            normalize_custody_account_address(
                "  0xABCDEF0000000000000000000000000000000001  ",
                &evm_and_base,
            ),
            "0xabcdef0000000000000000000000000000000001"
        );

        let tron_only = vec!["tron".to_string()];
        assert_eq!(
            normalize_custody_account_address("  TABcDEF0000000000000000000000000000  ", &tron_only),
            "TABcDEF0000000000000000000000000000"
        );
    }
```

- [ ] **Step 2: Run tests to verify RED or current partial failure**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage custody_assignment_queries_apply_all_configured_assets custody_account_normalization_prefers_evm_when_any_config_is_evm -- --nocapture
```

Expected: fails if Task 2 has not yet added `normalize_custody_account_address`, or passes the normalization part but assignment behavior remains incomplete until the implementation step is done.

- [ ] **Step 3: Add config loading inside transactions**

Add transaction variant:

```rust
async fn configs_for_account_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    account_id: Uuid,
) -> AppResult<Vec<CustodyAccountChainConfig>> {
    let rows = sqlx::query_as::<_, CustodyAccountChainConfigRow>(LIST_CUSTODY_ACCOUNT_CONFIGS_QUERY)
        .bind(tenant_id)
        .bind(&vec![account_id])
        .fetch_all(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let configs = rows
        .into_iter()
        .map(|row| CustodyAccountChainConfig {
            id: row.id,
            chain_id: row.chain_id,
            chain_name: row.chain_name,
            asset_ids: row.asset_ids,
            asset_symbols: row.asset_symbols,
        })
        .collect::<Vec<_>>();

    if configs.is_empty() {
        return Err(AppError::Validation("custody account has no chain configs".to_string()));
    }

    Ok(configs)
}
```

- [ ] **Step 4: Replace single watched-address ensure with multi-chain ensure**

Add this function:

```rust
async fn ensure_watched_addresses_for_custody_account(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    address: &str,
    business_ref: &str,
    configs: &[CustodyAccountChainConfig],
) -> AppResult<Vec<CustodyAssignmentWatchedAddress>> {
    let mut watched_addresses = Vec::new();

    for config in configs {
        let chain = repositories::get_chain_in_transaction(transaction, config.chain_id).await?;
        repositories::validate_address_for_chain(&chain, address)?;
        let watched_address_id = ensure_watched_address_for_chain_config(
            transaction,
            tenant_id,
            config.chain_id,
            address,
            business_ref,
            &config.asset_ids,
        )
        .await?;
        watched_addresses.push(CustodyAssignmentWatchedAddress {
            chain_id: config.chain_id,
            chain_name: config.chain_name.clone(),
            watched_address_id,
            asset_ids: config.asset_ids.clone(),
        });
    }

    Ok(watched_addresses)
}
```

If `repositories::get_chain_in_transaction` does not exist, add it to `backend/crates/storage/src/repositories.rs`:

```rust
pub(crate) async fn get_chain_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> AppResult<Chain> {
    sqlx::query_as::<_, Chain>("SELECT * FROM chains WHERE id = $1")
        .bind(id)
        .fetch_optional(transaction.as_mut())
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or(AppError::NotFound("chain".to_string()))
}
```

- [ ] **Step 5: Replace native-only ensure with asset-list ensure**

Rename the old `ensure_watched_address_for_custody_account` body into this chain-level function:

```rust
async fn ensure_watched_address_for_chain_config(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    chain_id: Uuid,
    address: &str,
    business_ref: &str,
    asset_ids: &[Uuid],
) -> AppResult<Uuid> {
    if asset_ids.is_empty() {
        return Err(AppError::Validation("custody chain config requires at least one asset".to_string()));
    }

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

    let watched_address_id = if let Some(existing) = existing {
        if existing.status == "active" {
            existing.id
        } else {
            sqlx::query_as::<_, WatchedAddress>(ENSURE_WATCHED_ADDRESS_UPDATE_QUERY)
                .bind(existing.id)
                .bind(tenant_id)
                .fetch_one(transaction.as_mut())
                .await
                .map_err(|error| AppError::Database(error.to_string()))?
                .id
        }
    } else {
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
            asset_ids: asset_ids.to_vec(),
        };
        repositories::create_watched_address_in_transaction(
            transaction,
            request,
            asset_ids.to_vec(),
        )
        .await?
        .id
    };

    for asset_id in asset_ids {
        ensure_watched_address_asset(transaction, watched_address_id, *asset_id).await?;
    }

    Ok(watched_address_id)
}
```

Remove calls to `native_asset_id_for_chain` from custody assignment. Keep the function only if other code uses it; otherwise delete it.

- [ ] **Step 6: Update pool and user assignment account selection**

For pool source, call `CLAIM_AVAILABLE_POOL_ACCOUNT_QUERY` with only `tenant_id`:

```rust
.bind(tenant_id)
```

For user source:

- If `request.chain_configs` is present, validate shape and compute normalized address from those configs.
- If no existing account is found and `request.chain_configs` is missing, return `AppError::Validation("chain_configs are required for new user custody account".to_string())`.
- If creating a user account, insert legacy `chain_id` from the first config and then call `validate_and_insert_custody_chain_configs`.
- If an existing user account is found, do not overwrite its configs.

Use this helper outline:

```rust
async fn normalized_user_assignment_address(
    transaction: &mut Transaction<'_, Postgres>,
    address: &str,
    configs: Option<&[CustodyAccountChainConfigRequest]>,
) -> AppResult<String> {
    if let Some(configs) = configs {
        validate_custody_chain_config_shape(configs)?;
        let chain_types = chain_types_for_configs(transaction, configs).await?;
        let chain_type_values = chain_types.values().cloned().collect::<Vec<_>>();
        return Ok(normalize_custody_account_address(address, &chain_type_values));
    }
    Ok(normalize_custody_address(address))
}
```

- [ ] **Step 7: Update `assign_custody_account` response**

Inside `assign_custody_account`, after account selection:

```rust
    let configs = configs_for_account_in_transaction(&mut transaction, tenant_id, account.id).await?;
    let watched_addresses = ensure_watched_addresses_for_custody_account(
        &mut transaction,
        tenant_id,
        &account.address,
        &request.business_ref,
        &configs,
    )
    .await?;
    let legacy_watched_address_id = watched_addresses
        .first()
        .map(|watched| watched.watched_address_id)
        .ok_or_else(|| AppError::Validation("custody account has no watched addresses".to_string()))?;
```

Bind `legacy_watched_address_id` into `INSERT_CUSTODY_ASSIGNMENT_QUERY` and `MARK_CUSTODY_ACCOUNT_ASSIGNED_QUERY`.

Return:

```rust
    Ok(AssignCustodyAccountResponse {
        account,
        assignment,
        watched_addresses,
    })
```

- [ ] **Step 8: Run storage tests to verify GREEN**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage custody -- --nocapture
cargo test --manifest-path backend/Cargo.toml -p coin-listener-core custody -- --nocapture
```

Expected: both pass.

- [ ] **Step 9: Commit**

```bash
git add backend/crates/storage/src/custody_accounts.rs backend/crates/storage/src/repositories.rs backend/crates/core/src/models.rs
git commit -m "按托管链配置启用监听地址"
```

---

### Task 4: Update API route tests and backend integration surface

**Files:**
- Modify: `backend/crates/api-server/src/routes.rs`
- Modify: `backend/crates/storage/src/custody_accounts.rs` if API compile reveals missing public types

- [ ] **Step 1: Write failing API source test**

Extend `custody_handlers_use_authenticated_tenant_scope` or add this test in `backend/crates/api-server/src/routes.rs`:

```rust
    #[test]
    fn custody_api_uses_multi_chain_request_and_response_contracts() {
        let source = production_source();

        assert!(source.contains("AssignCustodyAccountRequest"));
        assert!(source.contains("CreateCustodyAccountRequest"));
        assert!(source.contains("assign_custody_account"));
        assert!(source.contains("create_custody_account"));
        assert!(source.contains("Json(response)"));
    }
```

- [ ] **Step 2: Run API test to verify RED or compile failure**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p api-server custody_api_uses_multi_chain_request_and_response_contracts -- --nocapture
```

Expected: passes if route source already compiles with new DTOs, or fails to compile if any request/response type update was missed in Tasks 1-3.

- [ ] **Step 3: Fix compile issues only**

If compile fails because `AssignCustodyAccountRequest.chain_id` is now `Option<Uuid>`, update storage call sites to stop assuming a required `chain_id`. API handlers should keep passing the JSON body directly:

```rust
async fn assign_custody_account(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<AssignCustodyAccountRequest>,
) -> Result<Response, ApiError> {
    let response =
        custody_accounts::assign_custody_account(&state.postgres, auth.tenant_id, request).await?;
    Ok(Json(response).into_response())
}
```

- [ ] **Step 4: Run API tests to verify GREEN**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p api-server custody -- --nocapture
```

Expected: all custody API tests pass.

- [ ] **Step 5: Commit**

```bash
git add backend/crates/api-server/src/routes.rs backend/crates/storage/src/custody_accounts.rs
git commit -m "更新托管多链 API 合约"
```

---

### Task 5: Update frontend contracts and custody page multi-chain UI

**Files:**
- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/api/client.ts`
- Modify: `frontend/src/pages/CustodyAccountsPage.tsx`
- Modify: `frontend/src/ui-regression.test.ts`

- [ ] **Step 1: Write failing frontend regression test**

In `frontend/src/ui-regression.test.ts`, extend `custody account mode is wired into frontend contracts and navigation` with these expected strings:

```ts
      'export type CustodyAccountChainConfigRequest',
      'export type CustodyAccountChainConfig',
      'export type CustodyAssignmentWatchedAddress',
      'chain_configs',
      'watched_addresses',
      'custodyChainRows',
      'assignChainRows',
      'addCustodyChainRow',
      'assetOptionsForChain',
      'selectedAssetSymbols',
      '每条链至少选择一个资产',
      '不能重复选择链',
      '监听链配置',
      'multiple',
```

- [ ] **Step 2: Run frontend test to verify RED**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: fails because the new type names and custody page multi-chain row helpers do not exist.

- [ ] **Step 3: Update frontend types**

In `frontend/src/api/types.ts`, add:

```ts
export type CustodyAccountChainConfigRequest = {
  chain_id: string;
  asset_ids: string[];
};

export type CustodyAccountChainConfig = {
  id: string;
  chain_id: string;
  chain_name: string;
  asset_ids: string[];
  asset_symbols: string[];
};

export type CustodyAssignmentWatchedAddress = {
  chain_id: string;
  chain_name: string;
  watched_address_id: string;
  asset_ids: string[];
};
```

Update `CustodyAccount`:

```ts
  chain_configs: CustodyAccountChainConfig[];
```

Update `CreateCustodyAccountRequest`:

```ts
  chain_configs: CustodyAccountChainConfigRequest[];
```

Update `AssignCustodyAccountRequest`:

```ts
  chain_id?: string | null;
  chain_configs?: CustodyAccountChainConfigRequest[] | null;
```

Update `AssignCustodyAccountResponse`:

```ts
  watched_addresses: CustodyAssignmentWatchedAddress[];
```

No client function body changes are required in `frontend/src/api/client.ts` unless TypeScript import lists need updating.

- [ ] **Step 4: Add custody page chain-row state and helpers**

In `frontend/src/pages/CustodyAccountsPage.tsx`, add imports:

```ts
import { Select } from '@douyinfe/semi-ui';
import { listAssets } from '../api/client';
import type { Asset, CustodyAccountChainConfigRequest } from '../api/types';
```

If keeping the grouped Semi import, merge `Select` into the existing import list.

Add local type and row ID helpers near existing form types:

```ts
type ChainRow = {
  id: string;
  chain_id: string;
  asset_ids: string[];
};

let custodyChainRowIdSequence = 0;

function createCustodyChainRowId() {
  custodyChainRowIdSequence += 1;
  return `custody-chain-row-${custodyChainRowIdSequence}`;
}

function emptyCustodyChainRow(): ChainRow {
  return { id: createCustodyChainRowId(), chain_id: '', asset_ids: [] };
}
```

Inside `CustodyAccountsPage`, add state and assets query:

```ts
  const [custodyChainRows, setCustodyChainRows] = useState<ChainRow[]>([emptyCustodyChainRow()]);
  const [assignChainRows, setAssignChainRows] = useState<ChainRow[]>([emptyCustodyChainRow()]);
  const assetsQuery = useQuery({ queryKey: ['assets'], queryFn: listAssets });
```

Add helper functions inside the component:

```ts
  function assetLabel(asset: Asset) {
    if (!asset.contract_address) return asset.symbol;
    return `${asset.symbol} (${asset.asset_type}, ${asset.contract_address.slice(0, 6)}...${asset.contract_address.slice(-4)})`;
  }

  function assetOptionsForChain(chainId: string) {
    return (assetsQuery.data ?? [])
      .filter(asset => asset.chain_id === chainId && asset.status === 'active')
      .map(asset => ({ value: asset.id, label: assetLabel(asset) }));
  }

  function selectedAssetSymbols(assetIds: string[] = []) {
    if (assetIds.length === 0) return '-';
    const assetMap = new Map((assetsQuery.data ?? []).map(asset => [asset.id, asset.symbol]));
    return assetIds.map(assetId => assetMap.get(assetId) ?? assetId).join(', ');
  }

  function normalizedChainConfigs(rows: ChainRow[]): CustodyAccountChainConfigRequest[] {
    const seen = new Set<string>();
    return rows.map(row => {
      if (!row.chain_id) throw new Error('请选择链');
      if (seen.has(row.chain_id)) throw new Error('不能重复选择链');
      seen.add(row.chain_id);
      if (row.asset_ids.length === 0) throw new Error('每条链至少选择一个资产');
      return { chain_id: row.chain_id, asset_ids: row.asset_ids };
    });
  }

  function addCustodyChainRow() {
    setCustodyChainRows(rows => [...rows, emptyCustodyChainRow()]);
  }

  function addAssignChainRow() {
    setAssignChainRows(rows => [...rows, emptyCustodyChainRow()]);
  }

  function updateCustodyChainRow(rowId: string, patch: Partial<ChainRow>) {
    setCustodyChainRows(rows => rows.map(row => row.id === rowId ? { ...row, ...patch } : row));
  }

  function updateAssignChainRow(rowId: string, patch: Partial<ChainRow>) {
    setAssignChainRows(rows => rows.map(row => row.id === rowId ? { ...row, ...patch } : row));
  }
```

- [ ] **Step 5: Send chain configs in create and user assign payloads**

Update create payload:

```ts
      const payload: CreateCustodyAccountRequest = {
        address: String(values.address).trim(),
        label: optionalString(values.label) ?? null,
        source: 'pool',
        status: 'available',
        chain_configs: normalizedChainConfigs(custodyChainRows),
      };
```

Update assign validation:

```ts
function validateAssignCustodyAccountForm(values: Record<string, unknown>, sourceRows: ChainRow[]) {
  if (String(values.source) === 'user' && !optionalString(values.address)) {
    Toast.warning('用户自带地址需填写地址');
    return false;
  }
  if (String(values.source) === 'user') {
    normalizedChainConfigs(sourceRows);
  }
  return true;
}
```

Update assign payload:

```ts
      if (!validateAssignCustodyAccountForm(values, assignChainRows)) {
        return Promise.reject(new Error('用户自带地址需填写地址'));
      }
      const source = String(values.source);
      const payload: AssignCustodyAccountRequest = {
        source,
        address: source === 'user' ? optionalString(values.address) : null,
        applicant_type: String(values.applicant_type),
        business_ref: String(values.business_ref).trim(),
        purpose: optionalString(values.purpose) ?? null,
        chain_configs: source === 'user' ? normalizedChainConfigs(assignChainRows) : null,
      };
```

- [ ] **Step 6: Render chain config rows**

Create a local renderer inside `CustodyAccountsPage`:

```tsx
  function renderChainRows(
    title: string,
    rows: ChainRow[],
    updateRow: (rowId: string, patch: Partial<ChainRow>) => void,
    addRow: () => void,
  ) {
    return (
      <DataSurface title={title} actions={<Button onClick={addRow}>添加链配置</Button>}>
        <Space vertical align="start" style={{ width: '100%' }}>
          {rows.map(row => (
            <Space key={row.id} wrap>
              <Select
                placeholder="选择链"
                value={row.chain_id || undefined}
                optionList={chainOptions}
                style={{ width: 220 }}
                onChange={value => updateRow(row.id, { chain_id: String(value), asset_ids: [] })}
              />
              <Select
                multiple
                filter
                placeholder="选择监听币种"
                value={row.asset_ids}
                optionList={assetOptionsForChain(row.chain_id)}
                style={{ minWidth: 320 }}
                onChange={value => updateRow(row.id, { asset_ids: Array.isArray(value) ? value.map(String) : [] })}
              />
              <Text type="tertiary">{selectedAssetSymbols(row.asset_ids)}</Text>
            </Space>
          ))}
        </Space>
      </DataSurface>
    );
  }
```

Use it in the create modal after the label field:

```tsx
          {renderChainRows('监听链配置', custodyChainRows, updateCustodyChainRow, addCustodyChainRow)}
```

Use it in the assign modal after the user address field. Keep it visible with explanatory copy for `source=user`; source pool submissions ignore it:

```tsx
          <Banner type="info" title="用户自带地址链配置" description="用户自带地址首次创建时会保存这些链和币种；如果地址已存在，则使用已有配置。" />
          {renderChainRows('监听链配置', assignChainRows, updateAssignChainRow, addAssignChainRow)}
```

- [ ] **Step 7: Display configured chains and assets in the account table**

Add table columns before status:

```tsx
            {
              title: '监听链',
              dataIndex: 'chain_configs',
              width: 220,
              ellipsis: { showTitle: true },
              render: value => (value as CustodyAccount['chain_configs']).map(config => config.chain_name).join(', ') || '-',
            },
            {
              title: '监听币种',
              dataIndex: 'chain_configs',
              width: 260,
              ellipsis: { showTitle: true },
              render: value => (value as CustodyAccount['chain_configs'])
                .map(config => `${config.chain_name}: ${config.asset_symbols.join('/')}`)
                .join('; ') || '-',
            },
```

- [ ] **Step 8: Run frontend tests to verify GREEN**

Run:

```bash
npm --prefix frontend run test:ui-regression
npm --prefix frontend run build
```

Expected:

- UI regression passes.
- Build passes. Existing `lottie-web` eval and chunk-size warnings may remain.

- [ ] **Step 9: Commit**

```bash
git add frontend/src/api/types.ts frontend/src/api/client.ts frontend/src/pages/CustodyAccountsPage.tsx frontend/src/ui-regression.test.ts
git commit -m "支持托管账户多链币种配置界面"
```

---

### Task 6: Final verification and cleanup

**Files:**
- Verify all files changed by Tasks 1-5.

- [ ] **Step 1: Run backend format check**

Run:

```bash
cargo fmt --manifest-path backend/Cargo.toml --all -- --check
```

Expected: exits 0 with no formatting diff.

- [ ] **Step 2: Run focused backend tests**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-core custody -- --nocapture
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage custody -- --nocapture
cargo test --manifest-path backend/Cargo.toml -p api-server custody -- --nocapture
```

Expected: all focused custody tests pass.

- [ ] **Step 3: Run full backend suite**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml --quiet
```

Expected: full backend suite exits 0.

- [ ] **Step 4: Run frontend verification**

Run:

```bash
npm --prefix frontend run test:ui-regression
npm --prefix frontend run build
```

Expected: UI regression passes and production build exits 0. Existing `lottie-web` eval and chunk-size warnings may remain.

- [ ] **Step 5: Inspect git diff**

Run:

```bash
git status --short
git diff --stat
```

Expected: only files listed in this plan are changed, unless a formatter touched related code.

- [ ] **Step 6: Commit final verification fixes if any**

If previous steps required fixes, commit them:

```bash
git add backend/crates/core/src/models.rs backend/crates/storage/src/custody_accounts.rs backend/crates/storage/src/repositories.rs backend/crates/api-server/src/routes.rs frontend/src/api/types.ts frontend/src/api/client.ts frontend/src/pages/CustodyAccountsPage.tsx frontend/src/ui-regression.test.ts
git commit -m "完善托管多链监听验证"
```

If no fixes were needed after Task 5, do not create an empty commit.
