# Coin Listener Real EVM RPC Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace scheduled EVM mock scans with a real lightweight JSON-RPC native-balance scan loop that records snapshots and emits balance-change events.

**Architecture:** Keep the worker as the scan orchestrator. Put HTTP JSON-RPC and balance conversion in `chain-providers`, database reads/writes in `storage::repositories`, and shared request/data models in `core::models`. Leave the API dev mock scan route unchanged and use real RPC only for scheduled worker scans.

**Tech Stack:** Rust 2021, Tokio, SQLx, PostgreSQL, Redis queues, reqwest with rustls, serde/serde_json, chrono, uuid, num-bigint.

---

## File Structure

- `backend/Cargo.toml` — add workspace dependencies for `reqwest` and `num-bigint`.
- `backend/crates/core/src/models.rs` — add `CreateBalanceSnapshotRequest` and serialization test coverage.
- `backend/crates/chain-providers/Cargo.toml` — opt into `reqwest` and `num-bigint` workspace dependencies.
- `backend/crates/chain-providers/src/evm.rs` — add JSON-RPC client, request/response parsing, hex/decimal conversion, and balance-change event draft creation while keeping existing mock transfer helpers.
- `backend/crates/storage/src/repositories.rs` — add active RPC provider selection, native asset lookup, snapshot insert, and latest snapshot lookup.
- `backend/crates/worker/Cargo.toml` — depend directly on `coin-listener-chain-providers`.
- `backend/crates/worker/src/lib.rs` — replace scheduled `MockEvm` scan plan with `EvmNativeBalance`, call real RPC scan, insert snapshots/events, and enqueue notifications only when an event exists.
- `backend/crates/api-server/src/routes.rs` — no planned behavior change; existing dev route regression tests must stay green.

The current working directory has previously been observed as not being a git repository. For checkpoint steps, commit only if `git status` succeeds. If it does not, record the changed file list in the implementation report and continue.

---

### Task 1: Add shared balance snapshot request model and dependency wiring

**Files:**
- Modify: `backend/Cargo.toml`
- Modify: `backend/crates/core/src/models.rs`
- Modify: `backend/crates/chain-providers/Cargo.toml`
- Modify: `backend/crates/worker/Cargo.toml`

- [ ] **Step 1: Write the failing core model test**

Add `CreateBalanceSnapshotRequest` to the test imports in `backend/crates/core/src/models.rs`:

```rust
use super::{
    CreateBalanceSnapshotRequest, EventStatus, NotificationStatus, NotifyEventTask,
    ProviderChainStatus, ProviderStatus, ProviderStatusItem, QueueStatus, ScanAddressTask,
    ScanStatus, SystemStatus,
};
```

Add this test inside the existing `#[cfg(test)] mod tests` block:

```rust
#[test]
fn create_balance_snapshot_request_round_trips_as_json() {
    let request = CreateBalanceSnapshotRequest {
        tenant_id: Uuid::from_u128(101),
        chain_id: Uuid::from_u128(102),
        address_id: Uuid::from_u128(103),
        asset_id: Uuid::from_u128(104),
        balance_raw: "1000000000000000000".to_string(),
        balance_decimal: "1.0".to_string(),
        block_number: Some(20_000_000),
        block_hash: None,
        source_provider_id: Some(Uuid::from_u128(105)),
    };

    let payload = serde_json::to_string(&request).expect("serialize snapshot request");
    let decoded: CreateBalanceSnapshotRequest =
        serde_json::from_str(&payload).expect("deserialize snapshot request");

    assert_eq!(decoded, request);
    assert!(payload.contains("\"balance_raw\":\"1000000000000000000\""));
    assert!(payload.contains("\"block_number\":20000000"));
}
```

- [ ] **Step 2: Run the model test to verify it fails**

Run:

```bash
cargo test -p coin-listener-core create_balance_snapshot_request_round_trips_as_json --manifest-path backend/Cargo.toml
```

Expected: FAIL because `CreateBalanceSnapshotRequest` does not exist.

- [ ] **Step 3: Add dependency wiring**

In `backend/Cargo.toml`, add these workspace dependencies under `[workspace.dependencies]`:

```toml
num-bigint = "0.4"
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
```

In `backend/crates/chain-providers/Cargo.toml`, update dependencies to include:

```toml
num-bigint.workspace = true
reqwest.workspace = true
```

In `backend/crates/worker/Cargo.toml`, add the direct chain provider dependency:

```toml
coin-listener-chain-providers = { path = "../chain-providers" }
```

- [ ] **Step 4: Add the shared request model**

In `backend/crates/core/src/models.rs`, add this struct immediately after `BalanceSnapshot`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateBalanceSnapshotRequest {
    pub tenant_id: Uuid,
    pub chain_id: Uuid,
    pub address_id: Uuid,
    pub asset_id: Uuid,
    pub balance_raw: String,
    pub balance_decimal: String,
    pub block_number: Option<i64>,
    pub block_hash: Option<String>,
    pub source_provider_id: Option<Uuid>,
}
```

- [ ] **Step 5: Run the model test to verify it passes**

Run:

```bash
cargo test -p coin-listener-core create_balance_snapshot_request_round_trips_as_json --manifest-path backend/Cargo.toml
```

Expected: PASS with 1 test passing.

- [ ] **Step 6: Checkpoint**

If `git status` succeeds, run:

```bash
git add backend/Cargo.toml backend/crates/core/src/models.rs backend/crates/chain-providers/Cargo.toml backend/crates/worker/Cargo.toml
git commit -m "Add EVM balance snapshot model"
```

Expected in a git repository: a new checkpoint commit. Expected outside a git repository: skip commit and report the changed files.

---

### Task 2: Add EVM JSON-RPC request parsing and balance conversion

**Files:**
- Modify: `backend/crates/chain-providers/src/evm.rs`
- Modify: `backend/crates/chain-providers/Cargo.toml`

- [ ] **Step 1: Write failing tests for RPC payloads and conversion helpers**

In `backend/crates/chain-providers/src/evm.rs`, update the test module imports:

```rust
use super::{
    build_json_rpc_request, mock_evm_transfer, parse_hex_quantity_to_i64,
    parse_hex_u256_to_decimal_string, parse_json_rpc_hex_result, wei_to_decimal_string,
    EvmBlockTag,
};
use coin_listener_core::{
    models::{Asset, WatchedAddress},
    AppError,
};
use serde_json::json;
use uuid::Uuid;
```

Add these tests inside the existing test module:

```rust
#[test]
fn json_rpc_request_body_uses_method_params_and_jsonrpc_version() {
    let request = build_json_rpc_request(
        "eth_getBalance",
        json!(["0x1111111111111111111111111111111111111111", EvmBlockTag::Latest.as_param()]),
    );

    assert_eq!(request["jsonrpc"], "2.0");
    assert_eq!(request["id"], 1);
    assert_eq!(request["method"], "eth_getBalance");
    assert_eq!(request["params"][0], "0x1111111111111111111111111111111111111111");
    assert_eq!(request["params"][1], "latest");
}

#[test]
fn json_rpc_response_parser_returns_hex_result() {
    let payload = r#"{"jsonrpc":"2.0","id":1,"result":"0xde0b6b3a7640000"}"#;

    let result = parse_json_rpc_hex_result(payload, "eth_getBalance").unwrap();

    assert_eq!(result, "0xde0b6b3a7640000");
}

#[test]
fn json_rpc_response_parser_rejects_error_payload() {
    let payload = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"execution reverted"}}"#;

    let result = parse_json_rpc_hex_result(payload, "eth_getBalance");

    assert!(matches!(
        result,
        Err(AppError::Validation(message))
            if message.contains("eth_getBalance") && message.contains("execution reverted")
    ));
}

#[test]
fn hex_quantity_parsing_supports_block_numbers_and_large_balances() {
    assert_eq!(parse_hex_quantity_to_i64("0x0").unwrap(), 0);
    assert_eq!(parse_hex_quantity_to_i64("0x1").unwrap(), 1);
    assert_eq!(
        parse_hex_u256_to_decimal_string("0xde0b6b3a7640000").unwrap(),
        "1000000000000000000"
    );
}

#[test]
fn invalid_hex_quantity_returns_validation_error() {
    let result = parse_hex_u256_to_decimal_string("0xnothex");

    assert!(matches!(
        result,
        Err(AppError::Validation(message)) if message.contains("invalid hex quantity")
    ));
}

#[test]
fn wei_decimal_formatting_respects_asset_decimals() {
    assert_eq!(
        wei_to_decimal_string("1000000000000000000", 18).unwrap(),
        "1.0"
    );
    assert_eq!(wei_to_decimal_string("1", 18).unwrap(), "0.000000000000000001");
    assert_eq!(wei_to_decimal_string("123450000", 6).unwrap(), "123.45");
    assert_eq!(wei_to_decimal_string("1000", 0).unwrap(), "1000");
}
```

- [ ] **Step 2: Run the chain provider tests to verify they fail**

Run:

```bash
cargo test -p coin-listener-chain-providers json_rpc --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers hex_quantity --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers wei_decimal --manifest-path backend/Cargo.toml
```

Expected: FAIL because `EvmBlockTag`, JSON-RPC helpers, and conversion helpers do not exist.

- [ ] **Step 3: Add JSON-RPC and conversion implementation**

In `backend/crates/chain-providers/src/evm.rs`, replace the current imports with:

```rust
use coin_listener_core::{
    models::{AddressEventDraft, Asset, WatchedAddress},
    AppError, AppResult,
};
use num_bigint::BigUint;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{str::FromStr, time::Duration};
```

Add this code after the imports and before `RawEvmTransfer`:

```rust
#[derive(Debug, Clone)]
pub struct EvmRpcClient {
    base_url: String,
    timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvmBlockTag {
    Latest,
}

impl EvmBlockTag {
    fn as_param(self) -> &'static str {
        match self {
            Self::Latest => "latest",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmBalance {
    pub block_number: i64,
    pub balance_raw: String,
    pub balance_decimal: String,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    result: Option<Value>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

impl EvmRpcClient {
    pub fn new(base_url: String, timeout: Duration) -> Self {
        Self { base_url, timeout }
    }

    pub async fn eth_block_number(&self) -> AppResult<i64> {
        let value = self.rpc_hex_result("eth_blockNumber", json!([])).await?;
        parse_hex_quantity_to_i64(&value)
    }

    pub async fn eth_get_balance(&self, address: &str, block: EvmBlockTag) -> AppResult<String> {
        self.rpc_hex_result("eth_getBalance", json!([address, block.as_param()]))
            .await
    }

    async fn rpc_hex_result(&self, method: &str, params: Value) -> AppResult<String> {
        let client = reqwest::Client::builder()
            .timeout(self.timeout)
            .build()
            .map_err(|error| AppError::Config(format!("evm rpc client build failed: {error}")))?;

        let response = client
            .post(&self.base_url)
            .json(&build_json_rpc_request(method, params))
            .send()
            .await
            .map_err(|error| {
                AppError::Config(format!(
                    "evm rpc {method} request failed for {}: {error}",
                    self.base_url
                ))
            })?;

        let status = response.status();
        let body = response.text().await.map_err(|error| {
            AppError::Config(format!(
                "evm rpc {method} response body failed for {}: {error}",
                self.base_url
            ))
        })?;

        if !status.is_success() {
            return Err(AppError::Config(format!(
                "evm rpc {method} returned http {status} for {}: {body}",
                self.base_url
            )));
        }

        parse_json_rpc_hex_result(&body, method)
    }
}

fn build_json_rpc_request(method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    })
}

fn parse_json_rpc_hex_result(payload: &str, method: &str) -> AppResult<String> {
    let response: JsonRpcResponse = serde_json::from_str(payload).map_err(|error| {
        AppError::Validation(format!("invalid evm rpc {method} response json: {error}"))
    })?;

    if let Some(error) = response.error {
        return Err(AppError::Validation(format!(
            "evm rpc {method} error {}: {}",
            error.code, error.message
        )));
    }

    let result = response.result.ok_or_else(|| {
        AppError::Validation(format!("evm rpc {method} response missing result"))
    })?;

    result.as_str().map(ToOwned::to_owned).ok_or_else(|| {
        AppError::Validation(format!("evm rpc {method} result is not a hex string"))
    })
}

pub fn parse_hex_quantity_to_i64(hex: &str) -> AppResult<i64> {
    let digits = hex_digits(hex)?;
    i64::from_str_radix(digits, 16)
        .map_err(|error| AppError::Validation(format!("invalid hex quantity {hex}: {error}")))
}

pub fn parse_hex_u256_to_decimal_string(hex: &str) -> AppResult<String> {
    let digits = hex_digits(hex)?;
    BigUint::parse_bytes(digits.as_bytes(), 16)
        .map(|value| value.to_string())
        .ok_or_else(|| AppError::Validation(format!("invalid hex quantity {hex}")))
}

pub fn wei_to_decimal_string(raw: &str, decimals: i32) -> AppResult<String> {
    if decimals < 0 {
        return Err(AppError::Validation("asset decimals cannot be negative".to_string()));
    }

    let _ = BigUint::from_str(raw).map_err(|error| {
        AppError::Validation(format!("invalid decimal balance {raw}: {error}"))
    })?;

    let decimals = decimals as usize;
    if decimals == 0 {
        return Ok(raw.to_string());
    }

    let padded = if raw.len() <= decimals {
        format!("{}{}", "0".repeat(decimals + 1 - raw.len()), raw)
    } else {
        raw.to_string()
    };
    let split_at = padded.len() - decimals;
    let integer = &padded[..split_at];
    let fraction = padded[split_at..].trim_end_matches('0');

    if fraction.is_empty() {
        Ok(format!("{integer}.0"))
    } else {
        Ok(format!("{integer}.{fraction}"))
    }
}

fn hex_digits(hex: &str) -> AppResult<&str> {
    let digits = hex.strip_prefix("0x").ok_or_else(|| {
        AppError::Validation(format!("invalid hex quantity {hex}: missing 0x prefix"))
    })?;
    if digits.is_empty() || !digits.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(AppError::Validation(format!("invalid hex quantity {hex}")));
    }
    Ok(digits)
}
```

- [ ] **Step 4: Run the chain provider tests to verify they pass**

Run:

```bash
cargo test -p coin-listener-chain-providers json_rpc --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers hex_quantity --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers wei_decimal --manifest-path backend/Cargo.toml
```

Expected: PASS for the new JSON-RPC, hex parsing, and decimal formatting tests.

- [ ] **Step 5: Run chain provider check**

Run:

```bash
cargo check -p coin-listener-chain-providers --manifest-path backend/Cargo.toml
```

Expected: exit 0.

- [ ] **Step 6: Checkpoint**

If `git status` succeeds, run:

```bash
git add backend/crates/chain-providers/src/evm.rs backend/crates/chain-providers/Cargo.toml backend/Cargo.toml
git commit -m "Add lightweight EVM JSON-RPC helpers"
```

Expected in a git repository: a new checkpoint commit. Expected outside a git repository: skip commit and report the changed files.

---

### Task 3: Add provider and balance snapshot repository helpers

**Files:**
- Modify: `backend/crates/storage/src/repositories.rs`
- Modify: `backend/crates/core/src/models.rs`

- [ ] **Step 1: Write failing repository query tests**

In `backend/crates/storage/src/repositories.rs`, update the test imports at the bottom:

```rust
use super::{
    next_scan_at_from, ACTIVE_RPC_PROVIDER_QUERY, CLAIM_ONE_DUE_SCAN_ADDRESS_QUERY,
    INSERT_BALANCE_SNAPSHOT_QUERY, LATEST_BALANCE_SNAPSHOT_QUERY,
    MARK_CLAIMED_SCAN_ENQUEUED_QUERY,
};
```

Add these tests inside the existing test module:

```rust
#[test]
fn active_rpc_provider_query_filters_active_rpc_by_priority() {
    assert!(ACTIVE_RPC_PROVIDER_QUERY.contains("provider_type = 'rpc'"));
    assert!(ACTIVE_RPC_PROVIDER_QUERY.contains("status = 'active'"));
    assert!(ACTIVE_RPC_PROVIDER_QUERY.contains("ORDER BY priority ASC, name ASC"));
    assert!(ACTIVE_RPC_PROVIDER_QUERY.contains("LIMIT 1"));
}

#[test]
fn latest_balance_snapshot_query_can_exclude_current_snapshot() {
    assert!(LATEST_BALANCE_SNAPSHOT_QUERY.contains("address_id = $1"));
    assert!(LATEST_BALANCE_SNAPSHOT_QUERY.contains("asset_id = $2"));
    assert!(LATEST_BALANCE_SNAPSHOT_QUERY.contains("$3::uuid IS NULL OR id <> $3"));
    assert!(LATEST_BALANCE_SNAPSHOT_QUERY.contains("ORDER BY observed_at DESC"));
    assert!(LATEST_BALANCE_SNAPSHOT_QUERY.contains("LIMIT 1"));
}

#[test]
fn insert_balance_snapshot_query_maps_all_scan_fields() {
    assert!(INSERT_BALANCE_SNAPSHOT_QUERY.contains("tenant_id"));
    assert!(INSERT_BALANCE_SNAPSHOT_QUERY.contains("balance_raw"));
    assert!(INSERT_BALANCE_SNAPSHOT_QUERY.contains("balance_decimal"));
    assert!(INSERT_BALANCE_SNAPSHOT_QUERY.contains("block_number"));
    assert!(INSERT_BALANCE_SNAPSHOT_QUERY.contains("source_provider_id"));
    assert!(INSERT_BALANCE_SNAPSHOT_QUERY.contains("RETURNING id"));
}
```

- [ ] **Step 2: Run the repository tests to verify they fail**

Run:

```bash
cargo test -p coin-listener-storage active_rpc_provider_query --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage latest_balance_snapshot_query --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage insert_balance_snapshot_query --manifest-path backend/Cargo.toml
```

Expected: FAIL because the query constants do not exist.

- [ ] **Step 3: Add repository imports and query constants**

In `backend/crates/storage/src/repositories.rs`, update the model imports to include `BalanceSnapshot` and `CreateBalanceSnapshotRequest`:

```rust
models::{
    AddressEvent, AddressEventDraft, Asset, BalanceSnapshot, Chain, CreateBalanceSnapshotRequest,
    CreateProviderRequest, CreateWatchedAddressRequest, EventQuery, Provider, ScanAddressCandidate,
    ScanAddressContext, Tenant, User, WatchedAddress,
},
```

Add these constants after `MARK_CLAIMED_SCAN_ENQUEUED_QUERY`:

```rust
pub const ACTIVE_RPC_PROVIDER_QUERY: &str = r#"
SELECT id, chain_id, provider_type, name, base_url, api_key_ref, priority, qps_limit, timeout_ms, status
FROM providers
WHERE chain_id = $1
  AND provider_type = 'rpc'
  AND status = 'active'
ORDER BY priority ASC, name ASC
LIMIT 1
"#;

pub const LATEST_BALANCE_SNAPSHOT_QUERY: &str = r#"
SELECT id, tenant_id, chain_id, address_id, asset_id, balance_raw, balance_decimal,
       block_number, block_hash, observed_at, source_provider_id
FROM balance_snapshots
WHERE address_id = $1
  AND asset_id = $2
  AND ($3::uuid IS NULL OR id <> $3)
ORDER BY observed_at DESC
LIMIT 1
"#;

pub const INSERT_BALANCE_SNAPSHOT_QUERY: &str = r#"
INSERT INTO balance_snapshots (
    tenant_id, chain_id, address_id, asset_id, balance_raw, balance_decimal,
    block_number, block_hash, source_provider_id
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
RETURNING id, tenant_id, chain_id, address_id, asset_id, balance_raw, balance_decimal,
          block_number, block_hash, observed_at, source_provider_id
"#;
```

- [ ] **Step 4: Add repository functions**

Add these functions after `list_providers`:

```rust
pub async fn active_rpc_provider_for_chain(pool: &PgPool, chain_id: Uuid) -> AppResult<Provider> {
    sqlx::query_as::<_, Provider>(ACTIVE_RPC_PROVIDER_QUERY)
        .bind(chain_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("active rpc provider".to_string()))
}

pub async fn native_asset_for_chain(pool: &PgPool, chain_id: Uuid) -> AppResult<Asset> {
    get_native_asset(pool, chain_id).await
}

pub async fn latest_balance_snapshot(
    pool: &PgPool,
    address_id: Uuid,
    asset_id: Uuid,
    before_snapshot_id: Option<Uuid>,
) -> AppResult<Option<BalanceSnapshot>> {
    sqlx::query_as::<_, BalanceSnapshot>(LATEST_BALANCE_SNAPSHOT_QUERY)
        .bind(address_id)
        .bind(asset_id)
        .bind(before_snapshot_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn insert_balance_snapshot(
    pool: &PgPool,
    request: CreateBalanceSnapshotRequest,
) -> AppResult<BalanceSnapshot> {
    sqlx::query_as::<_, BalanceSnapshot>(INSERT_BALANCE_SNAPSHOT_QUERY)
        .bind(request.tenant_id)
        .bind(request.chain_id)
        .bind(request.address_id)
        .bind(request.asset_id)
        .bind(request.balance_raw)
        .bind(request.balance_decimal)
        .bind(request.block_number)
        .bind(request.block_hash)
        .bind(request.source_provider_id)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}
```

- [ ] **Step 5: Run the repository tests to verify they pass**

Run:

```bash
cargo test -p coin-listener-storage active_rpc_provider_query --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage latest_balance_snapshot_query --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage insert_balance_snapshot_query --manifest-path backend/Cargo.toml
```

Expected: PASS for all three query tests.

- [ ] **Step 6: Run storage check**

Run:

```bash
cargo check -p coin-listener-storage --manifest-path backend/Cargo.toml
```

Expected: exit 0.

- [ ] **Step 7: Checkpoint**

If `git status` succeeds, run:

```bash
git add backend/crates/storage/src/repositories.rs backend/crates/core/src/models.rs
git commit -m "Add EVM scan repository helpers"
```

Expected in a git repository: a new checkpoint commit. Expected outside a git repository: skip commit and report the changed files.

---

### Task 4: Add EVM balance-change event draft creation

**Files:**
- Modify: `backend/crates/chain-providers/src/evm.rs`

- [ ] **Step 1: Write failing event draft tests**

In `backend/crates/chain-providers/src/evm.rs`, update the test imports to include the new function and model types:

```rust
use super::{
    build_json_rpc_request, evm_balance_change_event, mock_evm_transfer,
    parse_hex_quantity_to_i64, parse_hex_u256_to_decimal_string,
    parse_json_rpc_hex_result, wei_to_decimal_string, EvmBlockTag,
};
use chrono::{TimeZone, Utc};
use coin_listener_core::{
    models::{Asset, BalanceSnapshot, Provider, ScanAddressContext, WatchedAddress},
    AppError,
};
use serde_json::json;
use uuid::Uuid;
```

Add these test helpers inside the test module:

```rust
fn scan_context() -> ScanAddressContext {
    ScanAddressContext {
        id: Uuid::from_u128(201),
        tenant_id: Uuid::from_u128(202),
        chain_id: Uuid::from_u128(203),
        address: "0x1111111111111111111111111111111111111111".to_string(),
        scan_interval_seconds: 300,
        chain_type: "evm".to_string(),
    }
}

fn native_asset(chain_id: Uuid) -> Asset {
    Asset {
        id: Uuid::from_u128(204),
        chain_id,
        asset_type: "native".to_string(),
        symbol: "ETH".to_string(),
        name: "Ether".to_string(),
        contract_address: None,
        decimals: 18,
        is_builtin: true,
        status: "active".to_string(),
    }
}

fn rpc_provider(chain_id: Uuid) -> Provider {
    Provider {
        id: Uuid::from_u128(205),
        chain_id,
        provider_type: "rpc".to_string(),
        name: "Primary RPC".to_string(),
        base_url: "https://example.invalid".to_string(),
        api_key_ref: None,
        priority: 1,
        qps_limit: 10,
        timeout_ms: 5000,
        status: "active".to_string(),
    }
}

fn snapshot(id: u128, raw: &str, block_number: i64) -> BalanceSnapshot {
    let context = scan_context();
    BalanceSnapshot {
        id: Uuid::from_u128(id),
        tenant_id: context.tenant_id,
        chain_id: context.chain_id,
        address_id: context.id,
        asset_id: Uuid::from_u128(204),
        balance_raw: raw.to_string(),
        balance_decimal: wei_to_decimal_string(raw, 18).unwrap(),
        block_number: Some(block_number),
        block_hash: None,
        observed_at: Utc.with_ymd_and_hms(2026, 5, 17, 20, 0, 0).unwrap(),
        source_provider_id: Some(Uuid::from_u128(205)),
    }
}
```

Add these tests:

```rust
#[test]
fn balance_change_event_marks_inbound_balance_increase() {
    let context = scan_context();
    let asset = native_asset(context.chain_id);
    let provider = rpc_provider(context.chain_id);
    let previous = snapshot(301, "100", 10);
    let current = snapshot(302, "150", 11);

    let draft = evm_balance_change_event(&context, &asset, &previous, &current, &provider).unwrap();

    assert_eq!(draft.tenant_id, context.tenant_id);
    assert_eq!(draft.chain_id, context.chain_id);
    assert_eq!(draft.address_id, context.id);
    assert_eq!(draft.asset_id, asset.id);
    assert_eq!(draft.event_type, "balance_change");
    assert_eq!(draft.direction, "in");
    assert!(!draft.is_transfer);
    assert_eq!(draft.block_number, Some(11));
    assert_eq!(draft.balance_before_raw, Some("100".to_string()));
    assert_eq!(draft.balance_after_raw, Some("150".to_string()));
    assert_eq!(draft.balance_delta_raw, Some("50".to_string()));
    assert_eq!(draft.metadata["source"], "evm_balance_snapshot");
    assert_eq!(draft.metadata["provider_name"], "Primary RPC");
}

#[test]
fn balance_change_event_marks_outbound_balance_decrease() {
    let context = scan_context();
    let asset = native_asset(context.chain_id);
    let provider = rpc_provider(context.chain_id);
    let previous = snapshot(303, "150", 10);
    let current = snapshot(304, "100", 11);

    let draft = evm_balance_change_event(&context, &asset, &previous, &current, &provider).unwrap();

    assert_eq!(draft.direction, "out");
    assert_eq!(draft.balance_delta_raw, Some("-50".to_string()));
}
```

- [ ] **Step 2: Run the event draft tests to verify they fail**

Run:

```bash
cargo test -p coin-listener-chain-providers balance_change_event --manifest-path backend/Cargo.toml
```

Expected: FAIL because `evm_balance_change_event` does not exist.

- [ ] **Step 3: Implement balance-change event draft creation**

In `backend/crates/chain-providers/src/evm.rs`, update the top imports to this complete set:

```rust
use coin_listener_core::{
    models::{
        AddressEventDraft, Asset, BalanceSnapshot, Provider, ScanAddressContext, WatchedAddress,
    },
    AppError, AppResult,
};
use num_bigint::{BigInt, BigUint, Sign};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{str::FromStr, time::Duration};
```

Add this function after `wei_to_decimal_string`:

```rust
pub fn evm_balance_change_event(
    context: &ScanAddressContext,
    asset: &Asset,
    previous: &BalanceSnapshot,
    current: &BalanceSnapshot,
    provider: &Provider,
) -> AppResult<AddressEventDraft> {
    let previous_raw = parse_decimal_bigint(&previous.balance_raw)?;
    let current_raw = parse_decimal_bigint(&current.balance_raw)?;
    let delta = current_raw - previous_raw;
    let direction = if delta.sign() == Sign::Minus { "out" } else { "in" };

    Ok(AddressEventDraft {
        tenant_id: context.tenant_id,
        chain_id: context.chain_id,
        address_id: context.id,
        asset_id: asset.id,
        event_type: "balance_change".to_string(),
        direction: direction.to_string(),
        is_transfer: false,
        tx_hash: None,
        log_index: None,
        block_number: current.block_number,
        block_hash: current.block_hash.clone(),
        confirmations: 0,
        from_address: None,
        to_address: None,
        amount_raw: None,
        amount_decimal: None,
        balance_before_raw: Some(previous.balance_raw.clone()),
        balance_after_raw: Some(current.balance_raw.clone()),
        balance_delta_raw: Some(delta.to_string()),
        metadata: json!({
            "source": "evm_balance_snapshot",
            "provider_id": provider.id,
            "provider_name": provider.name,
            "previous_snapshot_id": previous.id,
            "current_snapshot_id": current.id,
            "source_provider_id": current.source_provider_id,
            "block_number": current.block_number,
        }),
    })
}

fn parse_decimal_bigint(value: &str) -> AppResult<BigInt> {
    BigInt::parse_bytes(value.as_bytes(), 10)
        .ok_or_else(|| AppError::Validation(format!("invalid decimal balance {value}")))
}
```

- [ ] **Step 4: Run the event draft tests to verify they pass**

Run:

```bash
cargo test -p coin-listener-chain-providers balance_change_event --manifest-path backend/Cargo.toml
```

Expected: PASS for inbound and outbound balance-change event tests.

- [ ] **Step 5: Run all chain provider tests**

Run:

```bash
cargo test -p coin-listener-chain-providers --manifest-path backend/Cargo.toml
```

Expected: PASS for all chain provider tests.

- [ ] **Step 6: Checkpoint**

If `git status` succeeds, run:

```bash
git add backend/crates/chain-providers/src/evm.rs
git commit -m "Add EVM balance change event drafts"
```

Expected in a git repository: a new checkpoint commit. Expected outside a git repository: skip commit and report the changed files.

---

### Task 5: Replace scheduled mock EVM scans with real native balance scans

**Files:**
- Modify: `backend/crates/worker/src/lib.rs`
- Modify: `backend/crates/worker/Cargo.toml`

- [ ] **Step 1: Write failing worker tests for real scan planning and notification gating**

In `backend/crates/worker/src/lib.rs`, update the `scan_plan_for_chain` test:

```rust
#[test]
fn evm_chain_uses_native_balance_scan() {
    assert_eq!(scan_plan_for_chain("evm"), ScanPlan::EvmNativeBalance);
}
```

Remove the `mock_evm_scan_outcome` test module because scheduled worker scans will no longer use the mock path.

Add this test module inside the existing `#[cfg(test)] mod tests` block:

```rust
mod balance_change_gating {
    use chrono::{TimeZone, Utc};
    use coin_listener_core::models::{AddressEvent, BalanceSnapshot};
    use uuid::Uuid;

    use crate::{notify_task_for_scan_event, should_emit_balance_change};

    fn snapshot(id: u128, raw: &str) -> BalanceSnapshot {
        BalanceSnapshot {
            id: Uuid::from_u128(id),
            tenant_id: Uuid::from_u128(1),
            chain_id: Uuid::from_u128(2),
            address_id: Uuid::from_u128(3),
            asset_id: Uuid::from_u128(4),
            balance_raw: raw.to_string(),
            balance_decimal: raw.to_string(),
            block_number: Some(100),
            block_hash: None,
            observed_at: Utc.with_ymd_and_hms(2026, 5, 17, 21, 0, 0).unwrap(),
            source_provider_id: Some(Uuid::from_u128(5)),
        }
    }

    fn event() -> AddressEvent {
        AddressEvent {
            id: Uuid::from_u128(41),
            tenant_id: Uuid::from_u128(42),
            chain_id: Uuid::from_u128(43),
            address_id: Uuid::from_u128(44),
            asset_id: Uuid::from_u128(45),
            event_type: "balance_change".to_string(),
            direction: "in".to_string(),
            is_transfer: false,
            tx_hash: None,
            log_index: None,
            block_number: Some(100),
            block_hash: None,
            confirmations: 0,
            from_address: None,
            to_address: None,
            amount_raw: None,
            amount_decimal: None,
            balance_before_raw: Some("100".to_string()),
            balance_after_raw: Some("150".to_string()),
            balance_delta_raw: Some("50".to_string()),
            metadata: Default::default(),
            detected_at: Utc.with_ymd_and_hms(2026, 5, 17, 21, 1, 0).unwrap(),
            created_at: Utc.with_ymd_and_hms(2026, 5, 17, 21, 1, 1).unwrap(),
        }
    }

    #[test]
    fn first_snapshot_does_not_emit_balance_change() {
        let current = snapshot(11, "100");

        assert!(!should_emit_balance_change(None, &current));
    }

    #[test]
    fn unchanged_snapshot_does_not_emit_balance_change() {
        let previous = snapshot(11, "100");
        let current = snapshot(12, "100");

        assert!(!should_emit_balance_change(Some(&previous), &current));
    }

    #[test]
    fn changed_snapshot_emits_balance_change() {
        let previous = snapshot(11, "100");
        let current = snapshot(12, "150");

        assert!(should_emit_balance_change(Some(&previous), &current));
    }

    #[test]
    fn notify_task_is_created_only_when_event_exists() {
        let now = Utc.with_ymd_and_hms(2026, 5, 17, 21, 2, 0).unwrap();
        assert!(notify_task_for_scan_event(None, now).is_none());

        let event = event();
        let task = notify_task_for_scan_event(Some(&event), now).expect("notify task");

        assert_eq!(task.event_id, event.id);
        assert_eq!(task.tenant_id, event.tenant_id);
        assert_eq!(task.attempt, 1);
        assert_eq!(task.enqueued_at, now);
    }
}
```

- [ ] **Step 2: Run worker tests to verify they fail**

Run:

```bash
cargo test -p worker scan_plan_for_chain --manifest-path backend/Cargo.toml
cargo test -p worker balance_change_gating --manifest-path backend/Cargo.toml
```

Expected: FAIL because `EvmNativeBalance`, `should_emit_balance_change`, and `notify_task_for_scan_event` do not exist.

- [ ] **Step 3: Add worker imports and scan plan changes**

In `backend/crates/worker/src/lib.rs`, update imports:

```rust
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration as StdDuration,
};

use chrono::{DateTime, Utc};
use coin_listener_chain_providers::evm::{
    self, parse_hex_u256_to_decimal_string, wei_to_decimal_string, EvmBlockTag, EvmRpcClient,
};
use coin_listener_core::{
    models::{
        AddressEvent, BalanceSnapshot, CreateBalanceSnapshotRequest, NotifyEventTask,
        ScanAddressTask,
    },
    AppError, AppResult,
};
use coin_listener_storage::{notify_queue::NotifyQueue, repositories, scan_queue::ScanQueue};
use redis::aio::MultiplexedConnection;
use sqlx::PgPool;
use tracing::{info, warn};
use uuid::Uuid;
```

Update `ScanPlan` and `scan_plan_for_chain`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanPlan {
    EvmNativeBalance,
    Unsupported(String),
}

pub fn scan_plan_for_chain(chain_type: &str) -> ScanPlan {
    match chain_type {
        "evm" => ScanPlan::EvmNativeBalance,
        other => ScanPlan::Unsupported(other.to_string()),
    }
}
```

Delete `mock_evm_scan_outcome` because scheduled worker scans no longer use mock EVM results.

- [ ] **Step 4: Add real EVM native balance scan helpers**

Add these functions after `build_notify_event_task`:

```rust
pub fn notify_task_for_scan_event(
    event: Option<&AddressEvent>,
    now: DateTime<Utc>,
) -> Option<NotifyEventTask> {
    event.map(|event| build_notify_event_task(event, now))
}

pub fn should_emit_balance_change(
    previous: Option<&BalanceSnapshot>,
    current: &BalanceSnapshot,
) -> bool {
    previous
        .map(|previous| previous.balance_raw != current.balance_raw)
        .unwrap_or(false)
}

pub async fn scan_evm_native_balance(
    pool: &PgPool,
    task: &ScanAddressTask,
    _now: DateTime<Utc>,
) -> AppResult<Option<AddressEvent>> {
    let context = repositories::get_scan_address_context(pool, task.address_id).await?;
    let asset = repositories::native_asset_for_chain(pool, context.chain_id).await?;
    let provider = repositories::active_rpc_provider_for_chain(pool, context.chain_id).await?;
    let timeout_ms = u64::try_from(provider.timeout_ms).map_err(|error| {
        AppError::Validation(format!("invalid provider timeout_ms {}: {error}", provider.timeout_ms))
    })?;
    let rpc = EvmRpcClient::new(
        provider.base_url.clone(),
        StdDuration::from_millis(timeout_ms),
    );

    let block_number = rpc.eth_block_number().await?;
    let balance_hex = rpc
        .eth_get_balance(&context.address, EvmBlockTag::Latest)
        .await?;
    let balance_raw = parse_hex_u256_to_decimal_string(&balance_hex)?;
    let balance_decimal = wei_to_decimal_string(&balance_raw, asset.decimals)?;

    let snapshot = repositories::insert_balance_snapshot(
        pool,
        CreateBalanceSnapshotRequest {
            tenant_id: context.tenant_id,
            chain_id: context.chain_id,
            address_id: context.id,
            asset_id: asset.id,
            balance_raw,
            balance_decimal,
            block_number: Some(block_number),
            block_hash: None,
            source_provider_id: Some(provider.id),
        },
    )
    .await?;

    let previous = repositories::latest_balance_snapshot(
        pool,
        context.id,
        asset.id,
        Some(snapshot.id),
    )
    .await?;

    if !should_emit_balance_change(previous.as_ref(), &snapshot) {
        return Ok(None);
    }

    let previous = previous.expect("previous snapshot exists when balance change is emitted");
    let draft = evm::evm_balance_change_event(&context, &asset, &previous, &snapshot, &provider)?;
    let event = repositories::insert_event(pool, draft).await?;

    Ok(Some(event))
}
```

- [ ] **Step 5: Wire the real scan branch into locked task processing**

In `process_locked_scan_task`, replace the `ScanPlan::MockEvm` branch with:

```rust
ScanPlan::EvmNativeBalance => {
    let event = scan_evm_native_balance(pool, task, now).await?;
    if let Some(notify_task) = notify_task_for_scan_event(event.as_ref(), now) {
        notify_queue.enqueue(redis, &notify_task).await?;
    }
    repositories::finish_address_scan(pool, task.address_id, now, next_scan_at).await?;
    Ok(ScanTaskOutcome::Scanned)
}
```

Keep the unsupported-chain branch unchanged.

This placement intentionally means RPC/provider/database errors returned by `scan_evm_native_balance` exit before `finish_address_scan` and before notification enqueue.

- [ ] **Step 6: Run worker tests to verify they pass**

Run:

```bash
cargo test -p worker scan_plan_for_chain --manifest-path backend/Cargo.toml
cargo test -p worker balance_change_gating --manifest-path backend/Cargo.toml
cargo test -p worker build_notify_event_task --manifest-path backend/Cargo.toml
```

Expected: PASS for scan plan, balance gating, and existing notification task tests.

- [ ] **Step 7: Run worker check**

Run:

```bash
cargo check -p worker --manifest-path backend/Cargo.toml
```

Expected: exit 0.

- [ ] **Step 8: Checkpoint**

If `git status` succeeds, run:

```bash
git add backend/crates/worker/src/lib.rs backend/crates/worker/Cargo.toml
git commit -m "Use real EVM balance scans in worker"
```

Expected in a git repository: a new checkpoint commit. Expected outside a git repository: skip commit and report the changed files.

---

### Task 6: Run API regression and final verification

**Files:**
- Verify: `backend/crates/api-server/src/routes.rs`
- Verify: `backend/crates/worker/src/lib.rs`
- Verify: `backend/crates/storage/src/repositories.rs`
- Verify: `backend/crates/chain-providers/src/evm.rs`
- Verify: `frontend/`
- Verify: `docker-compose.yml`

- [ ] **Step 1: Verify the API dev route behavior remains unchanged**

Run:

```bash
cargo test -p api-server dev_scan_address --manifest-path backend/Cargo.toml
```

Expected: PASS for both dev scan route visibility tests.

- [ ] **Step 2: Run backend formatting check**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: exit 0. If formatting fails, run `cargo fmt --all --manifest-path backend/Cargo.toml`, then rerun the check.

- [ ] **Step 3: Run backend workspace check**

Run:

```bash
cargo check --workspace --manifest-path backend/Cargo.toml
```

Expected: exit 0.

- [ ] **Step 4: Run backend workspace tests**

Run:

```bash
cargo test --workspace --manifest-path backend/Cargo.toml
```

Expected: exit 0 with all Rust tests passing.

- [ ] **Step 5: Run frontend build regression**

Run:

```bash
npm run build --prefix frontend
```

Expected: exit 0. Existing Vite chunk-size or lottie direct-eval warnings may appear, but there must be no build failure.

- [ ] **Step 6: Validate Docker Compose configuration**

Run:

```bash
docker compose -f docker-compose.yml config
```

Expected: exit 0 and rendered compose configuration.

- [ ] **Step 7: Final checkpoint**

If `git status` succeeds, run:

```bash
git add backend/Cargo.toml backend/crates/core/src/models.rs backend/crates/chain-providers/Cargo.toml backend/crates/chain-providers/src/evm.rs backend/crates/storage/src/repositories.rs backend/crates/worker/Cargo.toml backend/crates/worker/src/lib.rs
git commit -m "Implement real EVM RPC balance scans"
```

Expected in a git repository: a new checkpoint commit if there are staged changes not already committed by earlier checkpoints. Expected outside a git repository: skip commit and report the changed files.

---

## Self-Review Notes

- Spec coverage: Tasks 1-5 cover lightweight JSON-RPC, native balance snapshots, active provider selection, previous snapshot comparison, balance-change event creation, notification gating, and no mock fallback for scheduled scans. Task 6 covers dev route regression and final verification.
- Scope control: The plan does not add WebSocket subscriptions, transaction-list scanning, ERC20 log scanning, provider failover, or frontend changes.
- Type consistency: `CreateBalanceSnapshotRequest`, `EvmRpcClient`, `EvmBlockTag`, repository helper names, and worker helper names are consistent across tasks.
- Failure behavior: `scan_evm_native_balance(...).await?` is called before `finish_address_scan`, so RPC/provider/database failures return without finishing the scan or enqueueing notification work.
