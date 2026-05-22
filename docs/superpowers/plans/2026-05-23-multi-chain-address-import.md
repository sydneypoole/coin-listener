# Multi-Chain Address Import Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Batch watched-address import creates one import task that expands each valid imported address across every selected chain configuration.

**Architecture:** Keep the existing import task and source-row tables, add task-level `chain_configs` plus an `watched_address_import_attempts` queue table. The API remains backward-compatible by accepting legacy `chain_id` and `asset_ids`; storage derives a single effective chain config when `chain_configs` is omitted. The worker processes address-chain attempts, and the frontend batch modal reuses the existing chain-row UI pattern from single-address creation.

**Tech Stack:** Rust 2021, Axum, SQLx/PostgreSQL, serde/serde_json, React, TypeScript, TanStack Query, Semi Design, Node test runner.

---

## File Map

- Modify `backend/crates/core/src/models.rs`
  - Add `WatchedAddressImportChainConfig`.
  - Add `chain_configs` to import defaults and task responses.
  - Add `chain_id` and `chain_name` to import error rows.
  - Extend serialization/deserialization tests.
- Create `backend/crates/storage/migrations/0018_multi_chain_address_import_attempts.sql`
  - Add `watched_address_import_tasks.chain_configs JSONB`.
  - Create `watched_address_import_attempts`.
  - Backfill existing tasks and row statuses into single-chain attempts.
- Modify `backend/crates/storage/src/address_imports.rs`
  - Derive effective chain configs from new or legacy request shape.
  - Validate empty configs, empty assets, duplicate chains, rows, and duplicate addresses.
  - Insert attempts in the same transaction as tasks and source rows.
  - Move claim, pending-work, mark-success, mark-failed, count refresh, cancel, and error-list queries to attempts.
- Modify `backend/crates/worker/src/lib.rs`
  - Fetch pending attempts instead of pending source rows.
  - Build `CreateWatchedAddressRequest` from source-row metadata plus attempt chain/assets.
  - Mark each attempt success or failed independently.
- Modify `frontend/src/api/types.ts`
  - Add `WatchedAddressImportChainConfig`.
  - Add `chain_configs` to request defaults and task responses.
  - Add chain context to import error rows.
- Modify `frontend/src/pages/AddressesPage.tsx`
  - Replace batch import single chain/asset fields with batch chain rows.
  - Validate duplicate chains before task creation.
  - Send legacy first config plus `chain_configs`.
  - Label progress as address-chain attempts and add chain column to errors.
- Modify `frontend/src/ui-regression.test.ts`
  - Assert the new frontend request shape and UI strings.
  - Assert parser still treats chain CSV fields as unknown address-level metadata.

---

### Task 1: Backend API Model Contract

**Files:**
- Modify: `backend/crates/core/src/models.rs`
- Test: `backend/crates/core/src/models.rs`

- [ ] **Step 1: Write failing model contract tests**

Add this import inside the existing `#[cfg(test)] mod tests` in `backend/crates/core/src/models.rs`, below the current `use super::{ ... };` block:

```rust
    use super::{
        WatchedAddressImportChainConfig, WatchedAddressImportDefaults,
        WatchedAddressImportErrorRow, WatchedAddressImportTask,
    };
```

Add these tests after the existing `address_import_create_request_carries_defaults_and_rows` test:

```rust
    #[test]
    fn address_import_create_request_accepts_chain_configs() {
        let payload = r#"{
            "defaults": {
                "chain_id":"00000000-0000-0000-0000-000000000002",
                "asset_ids":["00000000-0000-0000-0000-000000000101"],
                "chain_configs":[
                    {
                        "chain_id":"00000000-0000-0000-0000-000000000002",
                        "asset_ids":["00000000-0000-0000-0000-000000000101"]
                    },
                    {
                        "chain_id":"00000000-0000-0000-0000-000000000003",
                        "asset_ids":[
                            "00000000-0000-0000-0000-000000000201",
                            "00000000-0000-0000-0000-000000000202"
                        ]
                    }
                ],
                "priority":"normal",
                "scan_interval_seconds":300,
                "transfer_filter_enabled":true,
                "balance_change_filter_enabled":true,
                "status":"active"
            },
            "rows": [{
                "row_number":1,
                "raw_text":"0x0000000000000000000000000000000000000001,Hot wallet,critical",
                "address":"0x0000000000000000000000000000000000000001",
                "label":"Hot wallet",
                "priority":"critical"
            }]
        }"#;

        let request: CreateWatchedAddressImportRequest = serde_json::from_str(payload).unwrap();

        assert_eq!(request.defaults.chain_configs.len(), 2);
        assert_eq!(request.defaults.chain_configs[0].chain_id, Uuid::from_u128(2));
        assert_eq!(request.defaults.chain_configs[0].asset_ids, vec![Uuid::from_u128(0x101)]);
        assert_eq!(request.defaults.chain_configs[1].chain_id, Uuid::from_u128(3));
        assert_eq!(
            request.defaults.chain_configs[1].asset_ids,
            vec![Uuid::from_u128(0x201), Uuid::from_u128(0x202)]
        );
        assert_eq!(request.defaults.chain_id, request.defaults.chain_configs[0].chain_id);
        assert_eq!(request.defaults.asset_ids, request.defaults.chain_configs[0].asset_ids);
    }

    #[test]
    fn address_import_create_request_keeps_legacy_defaults_compatible() {
        let payload = r#"{
            "defaults": {
                "chain_id":"00000000-0000-0000-0000-000000000002",
                "asset_ids":["00000000-0000-0000-0000-000000000101"],
                "priority":"normal",
                "scan_interval_seconds":300,
                "transfer_filter_enabled":true,
                "balance_change_filter_enabled":true,
                "status":"active"
            },
            "rows": [{
                "row_number":1,
                "raw_text":"0x0000000000000000000000000000000000000001",
                "address":"0x0000000000000000000000000000000000000001"
            }]
        }"#;

        let request: CreateWatchedAddressImportRequest = serde_json::from_str(payload).unwrap();

        assert!(request.defaults.chain_configs.is_empty());
        assert_eq!(request.defaults.chain_id, Uuid::from_u128(2));
        assert_eq!(request.defaults.asset_ids, vec![Uuid::from_u128(0x101)]);
    }

    #[test]
    fn address_import_task_serializes_chain_configs_and_error_chain_context() {
        let now = Utc.with_ymd_and_hms(2026, 5, 23, 8, 0, 0).unwrap();
        let first = WatchedAddressImportChainConfig {
            chain_id: Uuid::from_u128(2),
            asset_ids: vec![Uuid::from_u128(0x101)],
        };
        let second = WatchedAddressImportChainConfig {
            chain_id: Uuid::from_u128(3),
            asset_ids: vec![Uuid::from_u128(0x201), Uuid::from_u128(0x202)],
        };
        let task = WatchedAddressImportTask {
            id: Uuid::from_u128(10),
            tenant_id: Uuid::from_u128(1),
            status: "running".to_string(),
            chain_id: first.chain_id,
            asset_ids: first.asset_ids.clone(),
            chain_configs: vec![first, second],
            priority: "normal".to_string(),
            scan_interval_seconds: 300,
            transfer_filter_enabled: true,
            balance_change_filter_enabled: true,
            address_status: "active".to_string(),
            total_rows: 2,
            processed_rows: 1,
            success_rows: 1,
            failed_rows: 0,
            locked_at: Some(now),
            locked_by: Some("worker".to_string()),
            started_at: Some(now),
            completed_at: None,
            last_error: None,
            created_at: now,
            updated_at: now,
        };
        let error = WatchedAddressImportErrorRow {
            row_number: 1,
            address: "0x0000000000000000000000000000000000000001".to_string(),
            raw_text: "0x0000000000000000000000000000000000000001".to_string(),
            chain_id: Uuid::from_u128(3),
            chain_name: Some("Base".to_string()),
            error_code: Some("create_failed".to_string()),
            error_message: Some("address already exists".to_string()),
        };

        let payload = serde_json::to_string(&(task, error)).unwrap();

        assert!(payload.contains("\"chain_configs\""));
        assert!(payload.contains("00000000-0000-0000-0000-000000000003"));
        assert!(payload.contains("\"chain_name\":\"Base\""));
    }
```

- [ ] **Step 2: Run model tests and verify they fail for the missing contract**

Run:

```bash
cargo test --locked --manifest-path "/Users/huangkunhuang/Public/程序工程目录/复合工程/coin-listener/backend/Cargo.toml" -p coin-listener-core address_import -- --nocapture
```

Expected: FAIL with compile errors naming missing `WatchedAddressImportChainConfig`, missing `chain_configs`, and missing `chain_id` / `chain_name` fields on `WatchedAddressImportErrorRow`.

- [ ] **Step 3: Add the minimal model implementation**

In `backend/crates/core/src/models.rs`, replace the current import defaults, task, and error-row structs with these definitions:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchedAddressImportChainConfig {
    pub chain_id: Uuid,
    pub asset_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchedAddressImportDefaults {
    pub chain_id: Uuid,
    pub asset_ids: Vec<Uuid>,
    #[serde(default)]
    pub chain_configs: Vec<WatchedAddressImportChainConfig>,
    pub priority: String,
    pub scan_interval_seconds: i32,
    pub transfer_filter_enabled: bool,
    pub balance_change_filter_enabled: bool,
    pub status: String,
}
```

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchedAddressImportTask {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub status: String,
    pub chain_id: Uuid,
    pub asset_ids: Vec<Uuid>,
    #[serde(default)]
    pub chain_configs: Vec<WatchedAddressImportChainConfig>,
    pub priority: String,
    pub scan_interval_seconds: i32,
    pub transfer_filter_enabled: bool,
    pub balance_change_filter_enabled: bool,
    pub address_status: String,
    pub total_rows: i32,
    pub processed_rows: i32,
    pub success_rows: i32,
    pub failed_rows: i32,
    pub locked_at: Option<DateTime<Utc>>,
    pub locked_by: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct WatchedAddressImportErrorRow {
    pub row_number: i32,
    pub address: String,
    pub raw_text: String,
    pub chain_id: Uuid,
    pub chain_name: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}
```

- [ ] **Step 4: Run model tests and verify they pass**

Run:

```bash
cargo test --locked --manifest-path "/Users/huangkunhuang/Public/程序工程目录/复合工程/coin-listener/backend/Cargo.toml" -p coin-listener-core address_import -- --nocapture
```

Expected: PASS for the three new import model tests and the existing import model test.

- [ ] **Step 5: Commit Task 1**

Run:

```bash
git add backend/crates/core/src/models.rs
git commit -m "$(cat <<'EOF'
添加导入多链配置模型
EOF
)"
```

---

### Task 2: Storage Migration for Chain Configs and Attempts

**Files:**
- Create: `backend/crates/storage/migrations/0018_multi_chain_address_import_attempts.sql`
- Modify: `backend/crates/storage/src/address_imports.rs`
- Test: `backend/crates/storage/src/address_imports.rs`

- [ ] **Step 1: Write failing migration coverage**

In `backend/crates/storage/src/address_imports.rs`, add this test inside the existing `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn multi_chain_import_migration_defines_attempt_storage() {
        let migration = include_str!("../migrations/0018_multi_chain_address_import_attempts.sql");

        assert!(migration.contains("ADD COLUMN IF NOT EXISTS chain_configs JSONB NOT NULL DEFAULT '[]'::jsonb"));
        assert!(migration.contains("CREATE TABLE IF NOT EXISTS watched_address_import_attempts"));
        assert!(migration.contains("import_task_id UUID NOT NULL REFERENCES watched_address_import_tasks(id) ON DELETE CASCADE"));
        assert!(migration.contains("asset_ids UUID[] NOT NULL"));
        assert!(migration.contains("status IN ('pending', 'success', 'failed', 'skipped')"));
        assert!(migration.contains("watched_address_import_attempts_unique_row_chain"));
        assert!(migration.contains("watched_address_import_attempts_source_row_fk"));
        assert!(migration.contains("idx_watched_address_import_attempts_task_status"));
        assert!(migration.contains("jsonb_build_array"));
        assert!(migration.contains("INSERT INTO watched_address_import_attempts"));
    }
```

- [ ] **Step 2: Run the migration test and verify it fails because the migration does not exist**

Run:

```bash
cargo test --locked --manifest-path "/Users/huangkunhuang/Public/程序工程目录/复合工程/coin-listener/backend/Cargo.toml" -p coin-listener-storage multi_chain_import_migration_defines_attempt_storage -- --nocapture
```

Expected: FAIL with an `include_str!` error for `0018_multi_chain_address_import_attempts.sql`.

- [ ] **Step 3: Create the migration**

Create `backend/crates/storage/migrations/0018_multi_chain_address_import_attempts.sql` with exactly this SQL:

```sql
ALTER TABLE watched_address_import_tasks
    ADD COLUMN IF NOT EXISTS chain_configs JSONB NOT NULL DEFAULT '[]'::jsonb;

UPDATE watched_address_import_tasks
SET chain_configs = jsonb_build_array(
    jsonb_build_object(
        'chain_id', chain_id,
        'asset_ids', to_jsonb(asset_ids)
    )
)
WHERE chain_configs = '[]'::jsonb;

CREATE TABLE IF NOT EXISTS watched_address_import_attempts (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    import_task_id UUID NOT NULL REFERENCES watched_address_import_tasks(id) ON DELETE CASCADE,
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    row_number INTEGER NOT NULL,
    chain_id UUID NOT NULL REFERENCES chains(id),
    asset_ids UUID[] NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    watched_address_id UUID REFERENCES watched_addresses(id) ON DELETE SET NULL,
    error_code TEXT,
    error_message TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT watched_address_import_attempts_status_check CHECK (status IN ('pending', 'success', 'failed', 'skipped')),
    CONSTRAINT watched_address_import_attempts_unique_row_chain UNIQUE (import_task_id, row_number, chain_id),
    CONSTRAINT watched_address_import_attempts_source_row_fk
        FOREIGN KEY (import_task_id, row_number)
        REFERENCES watched_address_import_rows(import_task_id, row_number)
        ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_watched_address_import_attempts_task_status
    ON watched_address_import_attempts(import_task_id, status, row_number, chain_id);

CREATE INDEX IF NOT EXISTS idx_watched_address_import_attempts_tenant_task
    ON watched_address_import_attempts(tenant_id, import_task_id);

INSERT INTO watched_address_import_attempts (
    import_task_id, tenant_id, row_number, chain_id, asset_ids, status,
    watched_address_id, error_code, error_message, created_at, updated_at
)
SELECT import_row.import_task_id,
       import_row.tenant_id,
       import_row.row_number,
       task.chain_id,
       task.asset_ids,
       import_row.status,
       import_row.watched_address_id,
       import_row.error_code,
       import_row.error_message,
       import_row.created_at,
       import_row.updated_at
FROM watched_address_import_rows import_row
JOIN watched_address_import_tasks task
  ON task.id = import_row.import_task_id
 AND task.tenant_id = import_row.tenant_id
ON CONFLICT (import_task_id, row_number, chain_id) DO NOTHING;
```

- [ ] **Step 4: Run the migration test and verify it passes**

Run:

```bash
cargo test --locked --manifest-path "/Users/huangkunhuang/Public/程序工程目录/复合工程/coin-listener/backend/Cargo.toml" -p coin-listener-storage multi_chain_import_migration_defines_attempt_storage -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit Task 2**

Run:

```bash
git add backend/crates/storage/migrations/0018_multi_chain_address_import_attempts.sql backend/crates/storage/src/address_imports.rs
git commit -m "$(cat <<'EOF'
添加地址导入尝试表迁移
EOF
)"
```

---

### Task 3: Storage Create Path and Validation

**Files:**
- Modify: `backend/crates/storage/src/address_imports.rs`
- Test: `backend/crates/storage/src/address_imports.rs`

- [ ] **Step 1: Write failing validation and create-path tests**

In `backend/crates/storage/src/address_imports.rs`, extend the test module import block to include the new helper and constants:

```rust
    use super::{
        effective_import_chain_configs, validate_import_create_request,
        CREATE_WATCHED_ADDRESS_IMPORT_QUERY, INSERT_IMPORT_ATTEMPT_QUERY,
        CLAIM_WATCHED_ADDRESS_IMPORT_QUERY, MARK_IMPORT_ROW_FAILED_QUERY,
        MARK_IMPORT_ROW_SUCCESS_QUERY, PENDING_IMPORT_ROWS_QUERY,
        REFRESH_IMPORT_TASK_COUNTS_QUERY,
    };
```

Replace the current `request_with_rows` helper with these helpers:

```rust
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
```

Update the test module model import to include `WatchedAddressImportChainConfig`:

```rust
    use coin_listener_core::models::{
        CreateWatchedAddressImportRequest, WatchedAddressImportChainConfig,
        WatchedAddressImportDefaults, WatchedAddressImportRowRequest,
    };
```

Add these tests after `import_validation_rejects_duplicate_addresses`:

```rust
    #[test]
    fn import_validation_derives_legacy_single_chain_config() {
        let request = request_with_rows(vec![row(1, "0x0000000000000000000000000000000000000001")]);

        validate_import_create_request(&request).unwrap();
        let configs = effective_import_chain_configs(&request.defaults);

        assert_eq!(configs, vec![chain_config(2, vec![101])]);
    }

    #[test]
    fn import_validation_rejects_empty_effective_assets() {
        let mut request = request_with_rows(vec![row(1, "0x0000000000000000000000000000000000000001")]);
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
    fn create_import_query_persists_chain_configs_and_attempt_total() {
        assert!(CREATE_WATCHED_ADDRESS_IMPORT_QUERY.contains("chain_configs"));
        assert!(CREATE_WATCHED_ADDRESS_IMPORT_QUERY.contains("total_rows"));
        assert!(CREATE_WATCHED_ADDRESS_IMPORT_QUERY.contains("VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"));
    }

    #[test]
    fn insert_attempt_query_uses_each_row_chain_pair() {
        assert!(INSERT_IMPORT_ATTEMPT_QUERY.contains("watched_address_import_attempts"));
        assert!(INSERT_IMPORT_ATTEMPT_QUERY.contains("row_number, chain_id, asset_ids"));
        assert!(INSERT_IMPORT_ATTEMPT_QUERY.contains("VALUES ($1, $2, $3, $4, $5)"));
    }
```

- [ ] **Step 2: Run storage validation tests and verify they fail**

Run:

```bash
cargo test --locked --manifest-path "/Users/huangkunhuang/Public/程序工程目录/复合工程/coin-listener/backend/Cargo.toml" -p coin-listener-storage address_imports -- --nocapture
```

Expected: FAIL with unresolved `effective_import_chain_configs`, missing `chain_configs` field in test helpers, missing `INSERT_IMPORT_ATTEMPT_QUERY`, or validation assertions still seeing the old legacy-only asset check.

- [ ] **Step 3: Add storage create-path implementation**

In `backend/crates/storage/src/address_imports.rs`, update imports at the top:

```rust
use coin_listener_core::{
    models::{
        CreateWatchedAddressImportRequest, WatchedAddressImportChainConfig,
        WatchedAddressImportErrorRow, WatchedAddressImportRowRequest,
        WatchedAddressImportTask,
    },
    AppError, AppResult,
};
use sqlx::{types::Json, PgPool, Postgres, Transaction};
```

Add this private SQLx row record and conversion near the existing row structs:

```rust
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
```

Replace `CREATE_WATCHED_ADDRESS_IMPORT_QUERY` with:

```rust
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
```

Add this query below `INSERT_IMPORT_ROW_QUERY`:

```rust
pub const INSERT_IMPORT_ATTEMPT_QUERY: &str = r#"
INSERT INTO watched_address_import_attempts (
    import_task_id, tenant_id, row_number, chain_id, asset_ids
)
VALUES ($1, $2, $3, $4, $5)
"#;
```

Add these helpers above `validate_import_create_request`:

```rust
pub fn effective_import_chain_configs(
    defaults: &coin_listener_core::models::WatchedAddressImportDefaults,
) -> Vec<WatchedAddressImportChainConfig> {
    if defaults.chain_configs.is_empty() {
        return vec![WatchedAddressImportChainConfig {
            chain_id: defaults.chain_id,
            asset_ids: defaults.asset_ids.clone(),
        }];
    }

    defaults.chain_configs.clone()
}

fn import_attempt_count(row_count: usize, chain_config_count: usize) -> AppResult<i32> {
    row_count
        .checked_mul(chain_config_count)
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| AppError::Validation("import attempt count exceeds i32 range".to_string()))
}
```

Replace the first part of `validate_import_create_request` with this code and keep the existing row-number/address validation loop after it:

```rust
pub fn validate_import_create_request(
    request: &CreateWatchedAddressImportRequest,
) -> AppResult<()> {
    if request.rows.is_empty() {
        return Err(AppError::Validation("import rows are required".to_string()));
    }

    let chain_configs = effective_import_chain_configs(&request.defaults);
    if chain_configs.is_empty() {
        return Err(AppError::Validation("chain_configs are required".to_string()));
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
```

Replace `create_watched_address_import` with:

```rust
pub async fn create_watched_address_import(
    pool: &PgPool,
    tenant_id: Uuid,
    request: CreateWatchedAddressImportRequest,
) -> AppResult<WatchedAddressImportTask> {
    validate_import_create_request(&request)?;
    let defaults = request.defaults;
    let rows = request.rows;
    let chain_configs = effective_import_chain_configs(&defaults);
    let first_config = chain_configs
        .first()
        .ok_or_else(|| AppError::Validation("chain_configs are required".to_string()))?;
    let total_rows = import_attempt_count(rows.len(), chain_configs.len())?;

    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let task_record = sqlx::query_as::<_, WatchedAddressImportTaskRecord>(
        CREATE_WATCHED_ADDRESS_IMPORT_QUERY,
    )
    .bind(tenant_id)
    .bind(first_config.chain_id)
    .bind(&first_config.asset_ids)
    .bind(Json(chain_configs.clone()))
    .bind(defaults.priority)
    .bind(defaults.scan_interval_seconds)
    .bind(defaults.transfer_filter_enabled)
    .bind(defaults.balance_change_filter_enabled)
    .bind(defaults.status)
    .bind(total_rows)
    .fetch_one(transaction.as_mut())
    .await
    .map_err(|error| AppError::Database(error.to_string()))?;
    let task = WatchedAddressImportTask::from(task_record);

    insert_import_rows(&mut transaction, tenant_id, task.id, &rows).await?;
    insert_import_attempts(&mut transaction, tenant_id, task.id, &rows, &chain_configs).await?;
    transaction
        .commit()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    Ok(task)
}
```

Add this helper after `insert_import_rows`:

```rust
async fn insert_import_attempts(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    task_id: Uuid,
    rows: &[WatchedAddressImportRowRequest],
    chain_configs: &[WatchedAddressImportChainConfig],
) -> AppResult<()> {
    for row in rows {
        for config in chain_configs {
            sqlx::query(INSERT_IMPORT_ATTEMPT_QUERY)
                .bind(task_id)
                .bind(tenant_id)
                .bind(row.row_number)
                .bind(config.chain_id)
                .bind(&config.asset_ids)
                .execute(transaction.as_mut())
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
        }
    }
    Ok(())
}
```

Update any `sqlx::query_as::<_, WatchedAddressImportTask>(...)` calls in this file to use `WatchedAddressImportTaskRecord` and convert records with `WatchedAddressImportTask::from(record)`. Task 4 will update the lifecycle queries these calls use.

- [ ] **Step 4: Run storage validation tests and verify they pass**

Run:

```bash
cargo test --locked --manifest-path "/Users/huangkunhuang/Public/程序工程目录/复合工程/coin-listener/backend/Cargo.toml" -p coin-listener-storage address_imports -- --nocapture
```

Expected: PASS for validation and query-shape tests.

- [ ] **Step 5: Commit Task 3**

Run:

```bash
git add backend/crates/storage/src/address_imports.rs
git commit -m "$(cat <<'EOF'
按链配置创建地址导入尝试
EOF
)"
```

---

### Task 4: Storage Attempt Queue, Progress, Errors, and Cancel

**Files:**
- Modify: `backend/crates/storage/src/address_imports.rs`
- Test: `backend/crates/storage/src/address_imports.rs`

- [ ] **Step 1: Write failing attempt lifecycle tests**

Replace the existing `row_processing_queries_are_tenant_scoped` test with these tests:

```rust
    #[test]
    fn claim_query_resumes_running_tasks_from_pending_attempts() {
        assert!(CLAIM_WATCHED_ADDRESS_IMPORT_QUERY.contains("watched_address_import_attempts"));
        assert!(CLAIM_WATCHED_ADDRESS_IMPORT_QUERY.contains("attempt.status = 'pending'"));
        assert!(CLAIM_WATCHED_ADDRESS_IMPORT_QUERY.contains("locked_by = $2"));
        assert!(CLAIM_WATCHED_ADDRESS_IMPORT_QUERY.contains("FOR UPDATE SKIP LOCKED"));
    }

    #[test]
    fn attempt_processing_queries_are_tenant_scoped() {
        for query in [
            PENDING_IMPORT_ATTEMPTS_QUERY,
            MARK_IMPORT_ATTEMPT_SUCCESS_QUERY,
            MARK_IMPORT_ATTEMPT_FAILED_QUERY,
            REFRESH_IMPORT_TASK_COUNTS_QUERY,
        ] {
            assert!(query.contains("tenant_id = $2"), "{query}");
        }
    }

    #[test]
    fn pending_attempts_join_source_rows_for_row_metadata() {
        assert!(PENDING_IMPORT_ATTEMPTS_QUERY.contains("watched_address_import_attempts attempt"));
        assert!(PENDING_IMPORT_ATTEMPTS_QUERY.contains("JOIN watched_address_import_rows import_row"));
        assert!(PENDING_IMPORT_ATTEMPTS_QUERY.contains("attempt.chain_id"));
        assert!(PENDING_IMPORT_ATTEMPTS_QUERY.contains("attempt.asset_ids"));
    }

    #[test]
    fn progress_counts_are_refreshed_from_attempts() {
        assert!(REFRESH_IMPORT_TASK_COUNTS_QUERY.contains("FROM watched_address_import_attempts"));
        assert!(REFRESH_IMPORT_TASK_COUNTS_QUERY.contains("status IN ('success', 'failed', 'skipped')"));
    }

    #[test]
    fn error_query_returns_chain_context() {
        assert!(LIST_WATCHED_ADDRESS_IMPORT_ERRORS_QUERY.contains("attempt.chain_id"));
        assert!(LIST_WATCHED_ADDRESS_IMPORT_ERRORS_QUERY.contains("chain.name AS chain_name"));
        assert!(LIST_WATCHED_ADDRESS_IMPORT_ERRORS_QUERY.contains("LEFT JOIN chains chain"));
    }

    #[test]
    fn cancel_query_marks_pending_attempts_skipped() {
        assert!(CANCEL_WATCHED_ADDRESS_IMPORT_ATTEMPTS_QUERY.contains("watched_address_import_attempts"));
        assert!(CANCEL_WATCHED_ADDRESS_IMPORT_ATTEMPTS_QUERY.contains("status = 'skipped'"));
        assert!(CANCEL_WATCHED_ADDRESS_IMPORT_QUERY.contains("FROM watched_address_import_attempts"));
    }
```

Update the `use super::{ ... };` block in the tests to include:

```rust
        CANCEL_WATCHED_ADDRESS_IMPORT_ATTEMPTS_QUERY,
        LIST_WATCHED_ADDRESS_IMPORT_ERRORS_QUERY, MARK_IMPORT_ATTEMPT_FAILED_QUERY,
        MARK_IMPORT_ATTEMPT_SUCCESS_QUERY, PENDING_IMPORT_ATTEMPTS_QUERY,
```

Remove `MARK_IMPORT_ROW_FAILED_QUERY`, `MARK_IMPORT_ROW_SUCCESS_QUERY`, and `PENDING_IMPORT_ROWS_QUERY` from the test import list after the replacement tests no longer reference them.

- [ ] **Step 2: Run attempt lifecycle tests and verify they fail**

Run:

```bash
cargo test --locked --manifest-path "/Users/huangkunhuang/Public/程序工程目录/复合工程/coin-listener/backend/Cargo.toml" -p coin-listener-storage address_imports -- --nocapture
```

Expected: FAIL with missing attempt query constants, old row-based query strings, or compile errors in the new attempt lifecycle tests.

- [ ] **Step 3: Implement attempt lifecycle queries and functions**

Replace the claim, pending, mark, refresh, complete, get, error-list, and cancel SQL constants with these attempt-based versions. Keep the exact names used by the tests:

```rust
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
               FROM watched_address_import_attempts attempt
               WHERE attempt.import_task_id = task.id
                 AND attempt.tenant_id = task.tenant_id
                 AND attempt.status = 'pending'
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
          task.address_status, task.total_rows, task.processed_rows,
          task.success_rows, task.failed_rows, task.locked_at, task.locked_by,
          task.started_at, task.completed_at, task.last_error, task.created_at,
          task.updated_at
"#;

pub const PENDING_IMPORT_ATTEMPTS_QUERY: &str = r#"
SELECT attempt.id AS attempt_id,
       attempt.row_number,
       import_row.raw_text,
       import_row.address,
       import_row.label,
       import_row.priority,
       import_row.scan_interval_seconds,
       import_row.transfer_filter_enabled,
       import_row.balance_change_filter_enabled,
       import_row.address_status AS status,
       attempt.chain_id,
       attempt.asset_ids
FROM watched_address_import_attempts attempt
JOIN watched_address_import_rows import_row
  ON import_row.import_task_id = attempt.import_task_id
 AND import_row.tenant_id = attempt.tenant_id
 AND import_row.row_number = attempt.row_number
WHERE attempt.import_task_id = $1
  AND attempt.tenant_id = $2
  AND attempt.status = 'pending'
ORDER BY attempt.row_number ASC, attempt.chain_id ASC
LIMIT $3
"#;

pub const MARK_IMPORT_ATTEMPT_SUCCESS_QUERY: &str = r#"
UPDATE watched_address_import_attempts
SET status = 'success',
    watched_address_id = $3,
    error_code = NULL,
    error_message = NULL,
    updated_at = NOW()
WHERE id = $1
  AND tenant_id = $2
  AND status = 'pending'
"#;

pub const MARK_IMPORT_ATTEMPT_FAILED_QUERY: &str = r#"
UPDATE watched_address_import_attempts
SET status = 'failed',
    error_code = $3,
    error_message = $4,
    updated_at = NOW()
WHERE id = $1
  AND tenant_id = $2
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
    FROM watched_address_import_attempts
    WHERE import_task_id = $1
      AND tenant_id = $2
) counts
WHERE task.id = $1
  AND task.tenant_id = $2
RETURNING task.id, task.tenant_id, task.status, task.chain_id, task.asset_ids,
          task.chain_configs, task.priority, task.scan_interval_seconds,
          task.transfer_filter_enabled, task.balance_change_filter_enabled,
          task.address_status, task.total_rows, task.processed_rows,
          task.success_rows, task.failed_rows, task.locked_at, task.locked_by,
          task.started_at, task.completed_at, task.last_error, task.created_at,
          task.updated_at
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
          locked_at, locked_by, started_at, completed_at, last_error, created_at,
          updated_at
"#;

const GET_WATCHED_ADDRESS_IMPORT_QUERY: &str = r#"
SELECT id, tenant_id, status, chain_id, asset_ids, chain_configs, priority,
       scan_interval_seconds, transfer_filter_enabled, balance_change_filter_enabled,
       address_status, total_rows, processed_rows, success_rows, failed_rows,
       locked_at, locked_by, started_at, completed_at, last_error, created_at,
       updated_at
FROM watched_address_import_tasks
WHERE id = $1
  AND tenant_id = $2
"#;

pub const LIST_WATCHED_ADDRESS_IMPORT_ERRORS_QUERY: &str = r#"
SELECT attempt.row_number,
       import_row.address,
       import_row.raw_text,
       attempt.chain_id,
       chain.name AS chain_name,
       attempt.error_code,
       attempt.error_message
FROM watched_address_import_attempts attempt
JOIN watched_address_import_rows import_row
  ON import_row.import_task_id = attempt.import_task_id
 AND import_row.tenant_id = attempt.tenant_id
 AND import_row.row_number = attempt.row_number
LEFT JOIN chains chain
  ON chain.id = attempt.chain_id
WHERE attempt.import_task_id = $1
  AND attempt.tenant_id = $2
  AND attempt.status = 'failed'
ORDER BY attempt.row_number ASC, chain.name ASC, attempt.chain_id ASC
"#;

pub const CANCEL_WATCHED_ADDRESS_IMPORT_ATTEMPTS_QUERY: &str = r#"
UPDATE watched_address_import_attempts attempt
SET status = 'skipped',
    error_code = 'cancelled',
    error_message = 'import task cancelled',
    updated_at = NOW()
WHERE attempt.import_task_id = $1
  AND attempt.tenant_id = $2
  AND attempt.status = 'pending'
  AND EXISTS (
      SELECT 1
      FROM watched_address_import_tasks task
      WHERE task.id = attempt.import_task_id
        AND task.tenant_id = attempt.tenant_id
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
    FROM watched_address_import_attempts
    WHERE import_task_id = $1
      AND tenant_id = $2
) counts
WHERE task.id = $1
  AND task.tenant_id = $2
  AND task.status IN ('pending', 'running')
RETURNING task.id, task.tenant_id, task.status, task.chain_id, task.asset_ids,
          task.chain_configs, task.priority, task.scan_interval_seconds,
          task.transfer_filter_enabled, task.balance_change_filter_enabled,
          task.address_status, task.total_rows, task.processed_rows,
          task.success_rows, task.failed_rows, task.locked_at, task.locked_by,
          task.started_at, task.completed_at, task.last_error, task.created_at,
          task.updated_at
"#;
```

Add this public attempt row type near `WatchedAddressImportRow`:

```rust
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WatchedAddressImportAttempt {
    pub attempt_id: Uuid,
    pub row_number: i32,
    pub raw_text: String,
    pub address: String,
    pub label: Option<String>,
    pub priority: Option<String>,
    pub scan_interval_seconds: Option<i32>,
    pub transfer_filter_enabled: Option<bool>,
    pub balance_change_filter_enabled: Option<bool>,
    pub status: Option<String>,
    pub chain_id: Uuid,
    pub asset_ids: Vec<Uuid>,
}
```

Replace `get_watched_address_import`, `cancel_watched_address_import`, `claim_next_watched_address_import`, `refresh_import_task_counts`, and `complete_import_if_finished` internals so they query `WatchedAddressImportTaskRecord` and convert it. Use this pattern for each task-returning query:

```rust
let record = sqlx::query_as::<_, WatchedAddressImportTaskRecord>(GET_WATCHED_ADDRESS_IMPORT_QUERY)
    .bind(id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("watched address import".to_string()))?;
Ok(WatchedAddressImportTask::from(record))
```

Replace `pending_import_rows`, `mark_import_row_success`, and `mark_import_row_failed` with attempt-based functions:

```rust
pub async fn pending_import_attempts(
    pool: &PgPool,
    tenant_id: Uuid,
    task_id: Uuid,
    limit: i64,
) -> AppResult<Vec<WatchedAddressImportAttempt>> {
    if limit <= 0 {
        return Ok(Vec::new());
    }

    sqlx::query_as::<_, WatchedAddressImportAttempt>(PENDING_IMPORT_ATTEMPTS_QUERY)
        .bind(task_id)
        .bind(tenant_id)
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn mark_import_attempt_success(
    pool: &PgPool,
    tenant_id: Uuid,
    attempt_id: Uuid,
    watched_address_id: Uuid,
) -> AppResult<()> {
    let result = sqlx::query(MARK_IMPORT_ATTEMPT_SUCCESS_QUERY)
        .bind(attempt_id)
        .bind(tenant_id)
        .bind(watched_address_id)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    ensure_import_attempt_updated(result.rows_affected())
}

pub async fn mark_import_attempt_failed(
    pool: &PgPool,
    tenant_id: Uuid,
    attempt_id: Uuid,
    error_code: &str,
    error_message: &str,
) -> AppResult<()> {
    let result = sqlx::query(MARK_IMPORT_ATTEMPT_FAILED_QUERY)
        .bind(attempt_id)
        .bind(tenant_id)
        .bind(error_code)
        .bind(error_message)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    ensure_import_attempt_updated(result.rows_affected())
}

fn ensure_import_attempt_updated(rows_affected: u64) -> AppResult<()> {
    if rows_affected == 0 {
        return Err(AppError::NotFound("watched address import attempt".to_string()));
    }

    Ok(())
}
```

Update `cancel_watched_address_import` to execute `CANCEL_WATCHED_ADDRESS_IMPORT_ATTEMPTS_QUERY` instead of the old row-cancel query.

Delete `ensure_import_row_updated` after all row mark functions are removed.

- [ ] **Step 4: Run attempt lifecycle tests and verify they pass**

Run:

```bash
cargo test --locked --manifest-path "/Users/huangkunhuang/Public/程序工程目录/复合工程/coin-listener/backend/Cargo.toml" -p coin-listener-storage address_imports -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit Task 4**

Run:

```bash
git add backend/crates/storage/src/address_imports.rs
git commit -m "$(cat <<'EOF'
按导入尝试刷新进度和错误
EOF
)"
```

---

### Task 5: Worker Expansion Across Attempts

**Files:**
- Modify: `backend/crates/worker/src/lib.rs`
- Test: `backend/crates/worker/src/lib.rs`

- [ ] **Step 1: Write failing worker source regression tests**

In `backend/crates/worker/src/lib.rs`, add these tests near the existing address-import worker tests:

```rust
    #[test]
    fn address_import_worker_processes_attempt_chain_configs() {
        let source = include_str!("lib.rs");
        let start = source
            .find("pub async fn process_one_address_import_task")
            .expect("address import worker function");
        let end = source[start..]
            .find("async fn process_locked_scan_task")
            .expect("next worker function")
            + start;
        let body = &source[start..end];

        assert!(body.contains("pending_import_attempts"));
        assert!(body.contains("chain_id: attempt.chain_id"));
        assert!(body.contains("asset_ids: attempt.asset_ids.clone()"));
        assert!(body.contains("mark_import_attempt_success"));
        assert!(body.contains("mark_import_attempt_failed"));
        assert!(!body.contains("chain_id: task.chain_id"));
        assert!(!body.contains("asset_ids: task.asset_ids.clone()"));
    }

    #[test]
    fn address_import_worker_marks_attempts_by_attempt_id() {
        let source = include_str!("lib.rs");
        let start = source
            .find("pub async fn process_one_address_import_task")
            .expect("address import worker function");
        let end = source[start..]
            .find("async fn process_locked_scan_task")
            .expect("next worker function")
            + start;
        let body = &source[start..end];

        assert!(body.contains("attempt.attempt_id"));
        assert!(!body.contains("row.row_number"));
    }
```

- [ ] **Step 2: Run worker tests and verify they fail against row-based processing**

Run:

```bash
cargo test --locked --manifest-path "/Users/huangkunhuang/Public/程序工程目录/复合工程/coin-listener/backend/Cargo.toml" -p coin-listener-worker address_import_worker -- --nocapture
```

Expected: FAIL because the function still calls `pending_import_rows`, uses `task.chain_id`, and marks rows by row number.

- [ ] **Step 3: Update the worker to process attempts**

In `backend/crates/worker/src/lib.rs`, replace `process_one_address_import_task` with:

```rust
pub async fn process_one_address_import_task(
    pool: &PgPool,
    worker_id: &str,
    now: DateTime<Utc>,
) -> AppResult<bool> {
    let Some(task) =
        address_imports::claim_next_watched_address_import(pool, now, worker_id).await?
    else {
        return Ok(false);
    };

    let attempts = address_imports::pending_import_attempts(
        pool,
        task.tenant_id,
        task.id,
        ADDRESS_IMPORT_ROW_BATCH_SIZE,
    )
    .await?;

    for attempt in attempts {
        let request = CreateWatchedAddressRequest {
            tenant_id: Some(task.tenant_id),
            chain_id: attempt.chain_id,
            address: attempt.address,
            label: attempt.label,
            priority: attempt.priority.unwrap_or_else(|| task.priority.clone()),
            scan_interval_seconds: attempt
                .scan_interval_seconds
                .unwrap_or(task.scan_interval_seconds),
            transfer_filter_enabled: attempt
                .transfer_filter_enabled
                .unwrap_or(task.transfer_filter_enabled),
            balance_change_filter_enabled: attempt
                .balance_change_filter_enabled
                .unwrap_or(task.balance_change_filter_enabled),
            status: attempt.status.unwrap_or_else(|| task.address_status.clone()),
            asset_ids: attempt.asset_ids.clone(),
        };

        match repositories::create_watched_address(pool, request).await {
            Ok(address) => {
                address_imports::mark_import_attempt_success(
                    pool,
                    task.tenant_id,
                    attempt.attempt_id,
                    address.id,
                )
                .await?;
            }
            Err(error) => {
                address_imports::mark_import_attempt_failed(
                    pool,
                    task.tenant_id,
                    attempt.attempt_id,
                    "create_failed",
                    &error.to_string(),
                )
                .await?;
            }
        }
    }

    address_imports::refresh_import_task_counts(pool, task.tenant_id, task.id).await?;
    address_imports::complete_import_if_finished(pool, task.tenant_id, task.id, now).await?;
    Ok(true)
}
```

- [ ] **Step 4: Run worker tests and verify they pass**

Run:

```bash
cargo test --locked --manifest-path "/Users/huangkunhuang/Public/程序工程目录/复合工程/coin-listener/backend/Cargo.toml" -p coin-listener-worker address_import_worker -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit Task 5**

Run:

```bash
git add backend/crates/worker/src/lib.rs
git commit -m "$(cat <<'EOF'
按地址链尝试处理导入任务
EOF
)"
```

---

### Task 6: Frontend Batch Import Chain Configuration UI

**Files:**
- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/pages/AddressesPage.tsx`
- Modify: `frontend/src/ui-regression.test.ts`
- Test: `frontend/src/ui-regression.test.ts`

- [ ] **Step 1: Write failing frontend regression tests**

In `frontend/src/ui-regression.test.ts`, extend the `notification and telegram API contracts are exposed to frontend` type expectations by adding:

```ts
      'export type WatchedAddressImportChainConfig',
      'chain_configs: WatchedAddressImportChainConfig[]',
      'chain_name?: string | null',
```

Replace the existing `watched address page supports backend task batch import` test with:

```ts
  test('watched address page supports backend task batch import', () => {
    const page = readSource('pages/AddressesPage.tsx');

    for (const expected of [
      '批量添加',
      'parseAddressImportInput',
      'createWatchedAddressImport',
      'getWatchedAddressImport',
      'listWatchedAddressImportErrors',
      'cancelWatchedAddressImport',
      'tableId="address-import-preview"',
      'tableId="address-import-errors"',
      'importTaskId',
      'batchChainRows',
      'addBatchChainRow',
      'removeBatchChainRow',
      'updateBatchChainRow',
      'normalizedBatchChainConfigs',
      'chain_configs',
      '不能重复选择链',
      '预计创建尝试',
      '导入进度（按地址-链尝试计数）',
      '总尝试',
      'chain_name',
      'row => `${row.row_number}-${row.chain_id}`',
      "queryClient.invalidateQueries({ queryKey: ['address-import-errors', importTaskId] })",
    ]) {
      expectContains(page, expected);
    }

    expectNotContains(page, 'handleBatchChainChange');
    expectNotContains(page, "setValue('asset_ids', [])");
  });
```

Add this assertion to the existing `address import parser reports duplicates and unknown CSV fields` test:

```ts
    const chainFieldResult = parseAddressImportInput('address,chain_id\n0x0000000000000000000000000000000000000005,base');
    if (!chainFieldResult.warnings.some(warning => warning.includes('chain_id'))) throw new Error('chain_id CSV field should remain unknown');
```

- [ ] **Step 2: Run frontend UI regression and verify it fails**

Run:

```bash
npm --prefix "/Users/huangkunhuang/Public/程序工程目录/复合工程/coin-listener/frontend" run test:ui-regression
```

Expected: FAIL because `WatchedAddressImportChainConfig`, `batchChainRows`, `chain_configs`, attempt labels, and chain error context are not implemented.

- [ ] **Step 3: Update frontend API types**

In `frontend/src/api/types.ts`, replace the import defaults and task/error-row types with:

```ts
export type WatchedAddressImportChainConfig = {
  chain_id: string;
  asset_ids: string[];
};

export type WatchedAddressImportDefaults = {
  chain_id: string;
  asset_ids: string[];
  chain_configs: WatchedAddressImportChainConfig[];
  priority: string;
  scan_interval_seconds: number;
  transfer_filter_enabled: boolean;
  balance_change_filter_enabled: boolean;
  status: string;
};
```

```ts
export type WatchedAddressImportTask = {
  id: string;
  tenant_id: string;
  status: string;
  chain_id: string;
  asset_ids: string[];
  chain_configs: WatchedAddressImportChainConfig[];
  priority: string;
  scan_interval_seconds: number;
  transfer_filter_enabled: boolean;
  balance_change_filter_enabled: boolean;
  address_status: string;
  total_rows: number;
  processed_rows: number;
  success_rows: number;
  failed_rows: number;
  locked_at?: string | null;
  locked_by?: string | null;
  started_at?: string | null;
  completed_at?: string | null;
  last_error?: string | null;
  created_at: string;
  updated_at: string;
};
```

```ts
export type WatchedAddressImportErrorRow = {
  row_number: number;
  address: string;
  raw_text: string;
  chain_id: string;
  chain_name?: string | null;
  error_code?: string | null;
  error_message?: string | null;
};
```

- [ ] **Step 4: Update batch import state and request building**

In `frontend/src/pages/AddressesPage.tsx`:

Remove this import because batch form no longer uses `FormApi`:

```ts
import type { FormApi } from '@douyinfe/semi-ui/lib/es/form/interface';
```

Remove these type aliases and state entries:

```ts
type BatchImportFormApi = FormApi<BatchImportForm>;
const [batchFormApi, setBatchFormApi] = useState<BatchImportFormApi | null>(null);
```

Add this state next to `chainRows`:

```ts
  const [batchChainRows, setBatchChainRows] = useState<ChainRow[]>([emptyChainRow()]);
```

Add these derived values after `importableRows`:

```ts
  const selectedBatchChainCount = batchChainRows.filter(row => row.chain_id && row.asset_ids.length > 0).length;
  const batchAttemptCount = importableRows.length * selectedBatchChainCount;
```

Replace `createImportMutation` with:

```tsx
  const createImportMutation = useMutation({
    mutationFn: (values: Record<string, unknown>) => {
      const chainConfigs = normalizedBatchChainConfigs();
      const firstConfig = chainConfigs[0];
      if (!firstConfig) {
        throw new Error('至少添加一条链配置');
      }
      return createWatchedAddressImport({
        defaults: {
          chain_id: firstConfig.chain_id,
          asset_ids: firstConfig.asset_ids,
          chain_configs: chainConfigs,
          priority: String(values.priority),
          scan_interval_seconds: Number(values.scan_interval_seconds),
          transfer_filter_enabled: Boolean(values.transfer_filter_enabled),
          balance_change_filter_enabled: Boolean(values.balance_change_filter_enabled),
          status: String(values.status),
        },
        rows: importableRows.map(row => ({
          row_number: row.row_number,
          raw_text: row.raw_text,
          address: row.address,
          label: row.label ?? null,
          priority: row.priority ?? null,
          scan_interval_seconds: row.scan_interval_seconds ?? null,
          transfer_filter_enabled: row.transfer_filter_enabled ?? null,
          balance_change_filter_enabled: row.balance_change_filter_enabled ?? null,
          status: row.status ?? null,
        })),
      });
    },
    onSuccess: task => {
      Toast.success('导入任务已创建');
      setImportTaskId(task.id);
      queryClient.invalidateQueries({ queryKey: ['addresses'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '导入任务创建失败'),
  });
```

Replace `closeBatchModal` and delete `handleBatchChainChange`:

```tsx
  function openBatchModal() {
    setBatchInput('');
    setImportTaskId(null);
    setBatchChainRows([emptyChainRow()]);
    setBatchVisible(true);
  }

  function closeBatchModal() {
    setBatchVisible(false);
  }
```

Add these batch helpers after `updateChainRow`:

```tsx
  function addBatchChainRow() {
    setBatchChainRows(rows => [...rows, emptyChainRow()]);
  }

  function removeBatchChainRow(rowId: string) {
    setBatchChainRows(rows => rows.length === 1 ? rows : rows.filter(row => row.id !== rowId));
  }

  function updateBatchChainRow(rowId: string, patch: Partial<Pick<ChainRow, 'chain_id' | 'asset_ids'>>) {
    setBatchChainRows(rows => rows.map(row => row.id === rowId ? { ...row, ...patch } : row));
  }

  function normalizedBatchChainConfigs() {
    if (batchChainRows.length === 0) {
      throw new Error('至少添加一条链配置');
    }
    const seen = new Set<string>();
    return batchChainRows.map(row => {
      if (!row.chain_id) {
        throw new Error('请选择链');
      }
      if (!row.asset_ids.length) {
        throw new Error('每条链至少选择一个资产');
      }
      if (seen.has(row.chain_id)) {
        throw new Error('不能重复选择链');
      }
      seen.add(row.chain_id);
      return { chain_id: row.chain_id, asset_ids: row.asset_ids };
    });
  }
```

Change the batch action button:

```tsx
<Button onClick={openBatchModal}>批量添加</Button>
```

- [ ] **Step 5: Replace batch modal single-chain controls with chain rows and attempt labels**

In the batch import `<Form<BatchImportForm>`, remove `getFormApi={setBatchFormApi}`.

Inside the form render, replace the current `默认链` and `监听资产` fields with:

```tsx
              <div className="address-chain-rows">
                {batchChainRows.map((row, index) => (
                  <Space key={row.id} align="start" style={{ width: '100%', marginBottom: 12 }}>
                    <div style={{ width: 180 }}>
                      <div style={{ marginBottom: 4 }}>{index === 0 ? '链配置' : '\u00a0'}</div>
                      <Select
                        value={row.chain_id}
                        placeholder="选择链"
                        style={{ width: '100%' }}
                        onChange={value => updateBatchChainRow(row.id, {
                          chain_id: typeof value === 'string' ? value : '',
                          asset_ids: [],
                        })}
                      >
                        {(chainsQuery.data ?? []).map(chain => <Select.Option key={chain.id} value={chain.id}>{chain.name}</Select.Option>)}
                      </Select>
                    </div>
                    <div style={{ width: 260 }}>
                      <div style={{ marginBottom: 4 }}>资产</div>
                      <Select
                        multiple
                        filter
                        value={row.asset_ids}
                        placeholder="选择资产"
                        optionList={assetOptionsForChain(row.chain_id)}
                        style={{ width: '100%' }}
                        onChange={value => updateBatchChainRow(row.id, { asset_ids: Array.isArray(value) ? value.map(String) : [] })}
                      />
                    </div>
                    <Button htmlType="button" onClick={() => removeBatchChainRow(row.id)} disabled={batchChainRows.length === 1}>移除</Button>
                  </Space>
                ))}
              </div>
              <Button htmlType="button" onClick={addBatchChainRow} theme="borderless">新增链配置</Button>
```

Add this summary above the preview `DataTable`:

```tsx
              <Space wrap className="address-import-summary">
                <Tag>有效地址 {importableRows.length}</Tag>
                <Tag color="blue">链配置 {selectedBatchChainCount}</Tag>
                <Tag color="green">预计创建尝试 {batchAttemptCount}</Tag>
              </Space>
```

Replace the progress title and tags with attempt wording:

```tsx
                  <div className="address-import-progress-title">导入进度（按地址-链尝试计数）</div>
                  <Progress percent={importProgress(importTaskQuery.data)} />
                  <Space wrap>
                    <Tag>总尝试 {importTaskQuery.data.total_rows}</Tag>
                    <Tag color="blue">已处理 {importTaskQuery.data.processed_rows}</Tag>
                    <Tag color="green">成功 {importTaskQuery.data.success_rows}</Tag>
                    <Tag color="red">失败 {importTaskQuery.data.failed_rows}</Tag>
                    <Tag>{importTaskQuery.data.status}</Tag>
                  </Space>
```

Update the error table row key and columns:

```tsx
                  rowKey={row => `${row.row_number}-${row.chain_id}`}
                  pagination={{ pageSize: 10 }}
                  scroll={{ x: 1050 }}
                  columns={[
                    { title: '行号', dataIndex: 'row_number', width: 80 },
                    { title: '链', dataIndex: 'chain_id', width: 160, render: (_, record) => record.chain_name ?? chainMap.get(String(record.chain_id)) ?? String(record.chain_id) },
                    { title: '地址', dataIndex: 'address', width: 320, className: 'table-cell-mono', ellipsis: { showTitle: true } },
                    { title: '原始内容', dataIndex: 'raw_text', width: 260, ellipsis: { showTitle: true } },
                    { title: '错误', dataIndex: 'error_message', width: 260, ellipsis: { showTitle: true } },
                  ]}
```

- [ ] **Step 6: Run frontend UI regression and verify it passes**

Run:

```bash
npm --prefix "/Users/huangkunhuang/Public/程序工程目录/复合工程/coin-listener/frontend" run test:ui-regression
```

Expected: PASS.

- [ ] **Step 7: Commit Task 6**

Run:

```bash
git add frontend/src/api/types.ts frontend/src/pages/AddressesPage.tsx frontend/src/ui-regression.test.ts
git commit -m "$(cat <<'EOF'
支持批量导入多链配置界面
EOF
)"
```

---

### Task 7: Full Verification and Integration Fixes

**Files:**
- Modify only files touched by Tasks 1-6 if verification exposes compile or type issues.
- Test: backend crates and frontend regression/build commands below.

- [ ] **Step 1: Run Rust formatting check**

Run:

```bash
cargo fmt --manifest-path "/Users/huangkunhuang/Public/程序工程目录/复合工程/coin-listener/backend/Cargo.toml" -- --check
```

Expected: PASS. If it fails, run this formatter and then rerun the check:

```bash
cargo fmt --manifest-path "/Users/huangkunhuang/Public/程序工程目录/复合工程/coin-listener/backend/Cargo.toml"
```

- [ ] **Step 2: Run backend model, storage, and worker tests**

Run:

```bash
cargo test --locked --manifest-path "/Users/huangkunhuang/Public/程序工程目录/复合工程/coin-listener/backend/Cargo.toml" -p coin-listener-core address_import -- --nocapture
cargo test --locked --manifest-path "/Users/huangkunhuang/Public/程序工程目录/复合工程/coin-listener/backend/Cargo.toml" -p coin-listener-storage address_import -- --nocapture
cargo test --locked --manifest-path "/Users/huangkunhuang/Public/程序工程目录/复合工程/coin-listener/backend/Cargo.toml" -p coin-listener-worker address_import -- --nocapture
```

Expected: all three commands PASS.

- [ ] **Step 3: Run API server compile tests for route contract compatibility**

Run:

```bash
cargo test --locked --manifest-path "/Users/huangkunhuang/Public/程序工程目录/复合工程/coin-listener/backend/Cargo.toml" -p api-server router_exposes_watched_address_import_routes -- --nocapture
```

Expected: PASS. This proves the existing import routes still compile against the updated request and response models.

- [ ] **Step 4: Run frontend regression and build**

Run:

```bash
npm --prefix "/Users/huangkunhuang/Public/程序工程目录/复合工程/coin-listener/frontend" run test:ui-regression
npm --prefix "/Users/huangkunhuang/Public/程序工程目录/复合工程/coin-listener/frontend" run build
```

Expected: UI regression PASS and frontend build exits 0. Existing Vite warnings about bundle size or dependency `eval` are acceptable only if the command exits 0.

- [ ] **Step 5: Inspect changed files before final commit**

Run:

```bash
git status --short
git diff --stat
```

Expected: only files from this plan are changed. If `cargo fmt` changed files, include those exact files in the final commit.

- [ ] **Step 6: Commit verification fixes if any files changed after previous task commits**

If Step 5 shows no changes, skip this commit. If Step 5 shows formatting or compile-fix changes, run:

```bash
git add backend/crates/core/src/models.rs backend/crates/storage/src/address_imports.rs backend/crates/worker/src/lib.rs frontend/src/api/types.ts frontend/src/pages/AddressesPage.tsx frontend/src/ui-regression.test.ts
git commit -m "$(cat <<'EOF'
验证多链批量导入实现
EOF
)"
```

---

## Acceptance Checklist

- [ ] New frontend requests send `defaults.chain_configs` and keep `defaults.chain_id` / `defaults.asset_ids` mirrored from the first chain config.
- [ ] Older clients that omit `chain_configs` still create a single-chain import from legacy fields.
- [ ] Backend rejects empty effective configs, configs with empty assets, and duplicate chain IDs.
- [ ] A single backend task represents the whole multi-chain import.
- [ ] Source rows remain address-level; attempts are address-chain work items.
- [ ] Worker creates one watched address per address-chain attempt and records failures independently.
- [ ] Progress totals count attempts, not raw input rows.
- [ ] Error rows include chain ID/name and use one row per failed attempt.
- [ ] Cancellation marks pending attempts skipped and recomputes task counts.
- [ ] CSV parsing remains address-only and reports chain fields as unknown.
- [ ] Backend targeted tests, frontend UI regression, and frontend build pass.
