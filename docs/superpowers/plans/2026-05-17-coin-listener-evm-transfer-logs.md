# Coin Listener EVM Transfer Logs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add polling-based EVM / BASE ERC20 `Transfer` log scanning so watched addresses generate real transfer `address_events` from `eth_getLogs`.

**Architecture:** Extend the existing lightweight EVM JSON-RPC client with log filters and decoding helpers, add storage cursor/repository helpers, then update the worker EVM path to combine native balance scanning with ERC20 transfer log scanning. Keep chain-provider code database-free, keep storage SQL in `repositories.rs`, and preserve the existing event center / notification APIs.

**Tech Stack:** Rust 2021, Tokio, SQLx, PostgreSQL, Redis queues, reqwest JSON-RPC, serde/serde_json, chrono, uuid, num-bigint.

---

## File Structure

Modify:

```text
backend/crates/core/src/models.rs
backend/crates/storage/src/repositories.rs
backend/crates/chain-providers/src/evm.rs
backend/crates/worker/src/lib.rs
```

Create:

```text
backend/crates/storage/migrations/0006_scan_cursors.sql
```

No frontend changes are planned. Existing `frontend/src/pages/EventsPage.tsx` consumes `address_events` and should show inserted transfer events through existing APIs.

The current directory has previously been observed as not being a git repository. For checkpoint steps, commit only if `git status` succeeds. If it does not, report changed files and continue.

---

### Task 1: Add scan cursor model and migration

**Files:**
- Modify: `backend/crates/core/src/models.rs`
- Create: `backend/crates/storage/migrations/0006_scan_cursors.sql`

- [ ] **Step 1: Write failing cursor model serialization test**

In `backend/crates/core/src/models.rs`, update the test import block:

```rust
use super::{
    CreateBalanceSnapshotRequest, EventStatus, NotificationStatus, NotifyEventTask,
    ProviderChainStatus, ProviderStatus, ProviderStatusItem, QueueStatus, ScanAddressTask,
    ScanCursor, ScanStatus, SystemStatus,
};
```

Add this test inside the existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn scan_cursor_round_trips_as_json() {
    let cursor = ScanCursor {
        id: Uuid::from_u128(21),
        tenant_id: Uuid::from_u128(22),
        chain_id: Uuid::from_u128(23),
        address_id: Uuid::from_u128(24),
        cursor_type: "evm_erc20_transfer".to_string(),
        last_scanned_block: 20_000_000,
        updated_at: Utc.with_ymd_and_hms(2026, 5, 17, 22, 0, 0).unwrap(),
    };

    let payload = serde_json::to_string(&cursor).expect("serialize scan cursor");
    let decoded: ScanCursor = serde_json::from_str(&payload).expect("deserialize scan cursor");

    assert_eq!(decoded.id, cursor.id);
    assert_eq!(decoded.cursor_type, "evm_erc20_transfer");
    assert_eq!(decoded.last_scanned_block, 20_000_000);
}
```

- [ ] **Step 2: Run the cursor model test to verify RED**

Run:

```bash
cargo test -p coin-listener-core scan_cursor_round_trips_as_json --manifest-path backend/Cargo.toml
```

Expected: FAIL because `ScanCursor` does not exist.

- [ ] **Step 3: Add the shared `ScanCursor` model**

In `backend/crates/core/src/models.rs`, add this struct after `ScanAddressContext`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ScanCursor {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub chain_id: Uuid,
    pub address_id: Uuid,
    pub cursor_type: String,
    pub last_scanned_block: i64,
    pub updated_at: DateTime<Utc>,
}
```

- [ ] **Step 4: Create scan cursor migration**

Create `backend/crates/storage/migrations/0006_scan_cursors.sql` with exactly:

```sql
CREATE TABLE IF NOT EXISTS scan_cursors (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    chain_id UUID NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
    address_id UUID NOT NULL REFERENCES watched_addresses(id) ON DELETE CASCADE,
    cursor_type TEXT NOT NULL,
    last_scanned_block BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(address_id, cursor_type)
);
```

- [ ] **Step 5: Run cursor model test to verify GREEN**

Run:

```bash
cargo test -p coin-listener-core scan_cursor_round_trips_as_json --manifest-path backend/Cargo.toml
```

Expected: PASS for `scan_cursor_round_trips_as_json`.

- [ ] **Step 6: Run core check**

Run:

```bash
cargo check -p coin-listener-core --manifest-path backend/Cargo.toml
```

Expected: exit 0.

- [ ] **Step 7: Checkpoint**

If `git status` succeeds, run:

```bash
git add backend/crates/core/src/models.rs backend/crates/storage/migrations/0006_scan_cursors.sql
git commit -m "Add scan cursor model and migration"
```

Expected outside a git repository: skip commit and report changed files.

---

### Task 2: Add EVM log filter, RPC parsing, and Transfer log decoding

**Files:**
- Modify: `backend/crates/chain-providers/src/evm.rs`

- [ ] **Step 1: Write failing EVM log helper tests**

In `backend/crates/chain-providers/src/evm.rs`, update the test imports to include the new helpers and structs:

```rust
use super::{
    address_to_topic, build_json_rpc_request, decode_erc20_transfer_log,
    evm_balance_change_event, format_rpc_request_error, mock_evm_transfer,
    parse_hex_quantity_to_i64, parse_hex_u256_to_decimal_string, parse_json_rpc_hex_result,
    topic_to_address, transfer_event_draft, wei_to_decimal_string, EvmBlockTag, EvmLog,
    EvmLogFilter, TRANSFER_TOPIC0,
};
```

Add these tests inside the existing test module:

```rust
#[test]
fn transfer_topic_and_address_topic_encoding_are_stable() {
    assert_eq!(
        TRANSFER_TOPIC0,
        "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"
    );
    assert_eq!(
        address_to_topic("0x1111111111111111111111111111111111111111").unwrap(),
        "0x0000000000000000000000001111111111111111111111111111111111111111"
    );
    assert_eq!(
        topic_to_address("0x0000000000000000000000001111111111111111111111111111111111111111").unwrap(),
        "0x1111111111111111111111111111111111111111"
    );
}

#[test]
fn eth_get_logs_request_body_contains_range_address_and_topics() {
    let filter = EvmLogFilter {
        address: "0xdac17f958d2ee523a2206206994597c13d831ec7".to_string(),
        from_block: 20_000_000,
        to_block: 20_000_010,
        topics: vec![
            Some(TRANSFER_TOPIC0.to_string()),
            None,
            Some(address_to_topic("0x1111111111111111111111111111111111111111").unwrap()),
        ],
    };

    let request = build_json_rpc_request("eth_getLogs", filter.to_rpc_params());

    assert_eq!(request["method"], "eth_getLogs");
    assert_eq!(request["params"][0]["address"], "0xdac17f958d2ee523a2206206994597c13d831ec7");
    assert_eq!(request["params"][0]["fromBlock"], "0x1312d00");
    assert_eq!(request["params"][0]["toBlock"], "0x1312d0a");
    assert_eq!(request["params"][0]["topics"][0], TRANSFER_TOPIC0);
    assert!(request["params"][0]["topics"][1].is_null());
}

#[test]
fn erc20_transfer_log_decodes_to_transfer_fields() {
    let log = EvmLog {
        address: "0xdac17f958d2ee523a2206206994597c13d831ec7".to_string(),
        topics: vec![
            TRANSFER_TOPIC0.to_string(),
            address_to_topic("0x2222222222222222222222222222222222222222").unwrap(),
            address_to_topic("0x1111111111111111111111111111111111111111").unwrap(),
        ],
        data: "0x00000000000000000000000000000000000000000000000000000000000f4240".to_string(),
        transaction_hash: Some("0xtxhash".to_string()),
        log_index: Some("0x2".to_string()),
        block_number: Some("0x1312d00".to_string()),
        block_hash: Some("0xblockhash".to_string()),
    };

    let decoded = decode_erc20_transfer_log(&log, 6).unwrap();

    assert_eq!(decoded.tx_hash, "0xtxhash");
    assert_eq!(decoded.log_index, 2);
    assert_eq!(decoded.block_number, 20_000_000);
    assert_eq!(decoded.block_hash, Some("0xblockhash".to_string()));
    assert_eq!(decoded.from_address, "0x2222222222222222222222222222222222222222");
    assert_eq!(decoded.to_address, "0x1111111111111111111111111111111111111111");
    assert_eq!(decoded.amount_raw, "1000000");
    assert_eq!(decoded.amount_decimal, "1.0");
    assert_eq!(decoded.token_contract, Some("0xdac17f958d2ee523a2206206994597c13d831ec7".to_string()));
}
```

- [ ] **Step 2: Run log helper tests to verify RED**

Run:

```bash
cargo test -p coin-listener-chain-providers transfer_topic --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers eth_get_logs_request --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers erc20_transfer_log_decodes --manifest-path backend/Cargo.toml
```

Expected: FAIL because `EvmLogFilter`, `EvmLog`, topic helpers, and decode function do not exist.

- [ ] **Step 3: Add EVM log types and request params**

In `backend/crates/chain-providers/src/evm.rs`, add this constant and structs after `EvmBalance`:

```rust
pub const TRANSFER_TOPIC0: &str =
    "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmLogFilter {
    pub address: String,
    pub from_block: i64,
    pub to_block: i64,
    pub topics: Vec<Option<String>>,
}

impl EvmLogFilter {
    pub fn to_rpc_params(&self) -> Value {
        json!([{
            "address": self.address,
            "fromBlock": format!("0x{:x}", self.from_block),
            "toBlock": format!("0x{:x}", self.to_block),
            "topics": self.topics,
        }])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EvmLog {
    pub address: String,
    pub topics: Vec<String>,
    pub data: String,
    #[serde(rename = "transactionHash")]
    pub transaction_hash: Option<String>,
    #[serde(rename = "logIndex")]
    pub log_index: Option<String>,
    #[serde(rename = "blockNumber")]
    pub block_number: Option<String>,
    #[serde(rename = "blockHash")]
    pub block_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedErc20Transfer {
    pub tx_hash: String,
    pub log_index: i32,
    pub block_number: i64,
    pub block_hash: Option<String>,
    pub from_address: String,
    pub to_address: String,
    pub amount_raw: String,
    pub amount_decimal: String,
    pub token_contract: Option<String>,
}
```

- [ ] **Step 4: Add `eth_getLogs` RPC method and parser**

In the `impl EvmRpcClient` block, add:

```rust
pub async fn eth_get_logs(&self, filter: EvmLogFilter) -> AppResult<Vec<EvmLog>> {
    let body = self.rpc_result_body("eth_getLogs", filter.to_rpc_params()).await?;
    parse_json_rpc_logs_result(&body, "eth_getLogs")
}

async fn rpc_result_body(&self, method: &str, params: Value) -> AppResult<String> {
    let response = self
        .client
        .post(&self.base_url)
        .json(&build_json_rpc_request(method, params))
        .send()
        .await
        .map_err(|error| {
            AppError::Config(format_rpc_request_error(
                method,
                &self.base_url,
                &error.without_url().to_string(),
            ))
        })?;
    let status = response.status();
    let body = response.text().await.map_err(|error| {
        AppError::Config(format_rpc_body_error(
            method,
            &error.without_url().to_string(),
        ))
    })?;
    if !status.is_success() {
        return Err(AppError::Config(format_rpc_status_error(
            method, status, &body,
        )));
    }
    Ok(body)
}
```

Then replace `rpc_hex_result` body with:

```rust
let body = self.rpc_result_body(method, params).await?;
parse_json_rpc_hex_result(&body, method)
```

Add parser:

```rust
pub fn parse_json_rpc_logs_result(payload: &str, method: &str) -> AppResult<Vec<EvmLog>> {
    let response: JsonRpcResponse = serde_json::from_str(payload).map_err(|error| {
        AppError::Validation(format!("invalid evm rpc {method} response json: {error}"))
    })?;
    if let Some(error) = response.error {
        return Err(AppError::Validation(format!(
            "evm rpc {method} error {}: {}",
            error.code, error.message
        )));
    }
    let result = response
        .result
        .ok_or_else(|| AppError::Validation(format!("evm rpc {method} response missing result")))?;
    serde_json::from_value(result)
        .map_err(|error| AppError::Validation(format!("invalid evm rpc {method} logs result: {error}")))
}
```

- [ ] **Step 5: Add topic helpers and transfer log decoder**

Add these functions near the hex helpers:

```rust
pub fn address_to_topic(address: &str) -> AppResult<String> {
    let address = address.to_lowercase();
    let digits = address.strip_prefix("0x").ok_or_else(|| {
        AppError::Validation(format!("invalid evm address {address}: missing 0x prefix"))
    })?;
    if digits.len() != 40 || !digits.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(AppError::Validation(format!("invalid evm address {address}")));
    }
    Ok(format!("0x{}{}", "0".repeat(24), digits))
}

pub fn topic_to_address(topic: &str) -> AppResult<String> {
    let digits = hex_digits(topic)?;
    if digits.len() != 64 {
        return Err(AppError::Validation(format!("invalid address topic {topic}")));
    }
    let padding = &digits[..24];
    if !padding.chars().all(|character| character == '0') {
        return Err(AppError::Validation(format!("invalid address topic {topic}")));
    }
    Ok(format!("0x{}", &digits[24..].to_lowercase()))
}

pub fn decode_erc20_transfer_log(log: &EvmLog, decimals: i32) -> AppResult<DecodedErc20Transfer> {
    if log.topics.len() < 3 {
        return Err(AppError::Validation("erc20 transfer log missing topics".to_string()));
    }
    if log.topics[0].to_lowercase() != TRANSFER_TOPIC0 {
        return Err(AppError::Validation("erc20 transfer log has wrong topic0".to_string()));
    }
    let tx_hash = log
        .transaction_hash
        .clone()
        .ok_or_else(|| AppError::Validation("erc20 transfer log missing transactionHash".to_string()))?;
    let log_index_hex = log
        .log_index
        .as_ref()
        .ok_or_else(|| AppError::Validation("erc20 transfer log missing logIndex".to_string()))?;
    let block_number_hex = log
        .block_number
        .as_ref()
        .ok_or_else(|| AppError::Validation("erc20 transfer log missing blockNumber".to_string()))?;
    let log_index = i32::try_from(parse_hex_quantity_to_i64(log_index_hex)?).map_err(|error| {
        AppError::Validation(format!("invalid erc20 transfer logIndex {log_index_hex}: {error}"))
    })?;
    let block_number = parse_hex_quantity_to_i64(block_number_hex)?;
    let amount_raw = parse_hex_u256_to_decimal_string(&log.data)?;
    let amount_decimal = wei_to_decimal_string(&amount_raw, decimals)?;

    Ok(DecodedErc20Transfer {
        tx_hash,
        log_index,
        block_number,
        block_hash: log.block_hash.clone(),
        from_address: topic_to_address(&log.topics[1])?,
        to_address: topic_to_address(&log.topics[2])?,
        amount_raw,
        amount_decimal,
        token_contract: Some(log.address.to_lowercase()),
    })
}
```

- [ ] **Step 6: Add transfer event draft builder**

Add this function after `decode_erc20_transfer_log`:

```rust
pub fn transfer_event_draft(
    context: &ScanAddressContext,
    asset: &Asset,
    transfer: DecodedErc20Transfer,
) -> AddressEventDraft {
    let watched = context.address.to_lowercase();
    let from = transfer.from_address.to_lowercase();
    let to = transfer.to_address.to_lowercase();
    let direction = if from == watched && to == watched {
        "self"
    } else if to == watched {
        "in"
    } else if from == watched {
        "out"
    } else {
        "unknown"
    };

    AddressEventDraft {
        tenant_id: context.tenant_id,
        chain_id: context.chain_id,
        address_id: context.id,
        asset_id: asset.id,
        event_type: "transfer".to_string(),
        direction: direction.to_string(),
        is_transfer: true,
        tx_hash: Some(transfer.tx_hash),
        log_index: Some(transfer.log_index),
        block_number: Some(transfer.block_number),
        block_hash: transfer.block_hash,
        confirmations: 0,
        from_address: Some(transfer.from_address),
        to_address: Some(transfer.to_address),
        amount_raw: Some(transfer.amount_raw),
        amount_decimal: Some(transfer.amount_decimal),
        balance_before_raw: None,
        balance_after_raw: None,
        balance_delta_raw: None,
        metadata: json!({
            "source": "evm_erc20_transfer_log",
            "token_contract": transfer.token_contract,
        }),
    }
}
```

- [ ] **Step 7: Run EVM log tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-chain-providers transfer_topic --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers eth_get_logs_request --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers erc20_transfer_log_decodes --manifest-path backend/Cargo.toml
```

Expected: all three commands pass.

- [ ] **Step 8: Run all chain provider tests**

Run:

```bash
cargo test -p coin-listener-chain-providers --manifest-path backend/Cargo.toml
cargo fmt -p coin-listener-chain-providers --check --manifest-path backend/Cargo.toml
```

Expected: tests pass and fmt check exits 0.

- [ ] **Step 9: Checkpoint**

If `git status` succeeds, run:

```bash
git add backend/crates/chain-providers/src/evm.rs
git commit -m "Add EVM ERC20 transfer log decoding"
```

Expected outside a git repository: skip commit and report changed files.

---

### Task 3: Add storage helpers for ERC20 assets, cursors, and idempotent event inserts

**Files:**
- Modify: `backend/crates/storage/src/repositories.rs`

- [ ] **Step 1: Write failing repository query tests**

In `backend/crates/storage/src/repositories.rs`, update imports to include `ScanCursor`:

```rust
models::{
    AddressEvent, AddressEventDraft, Asset, BalanceSnapshot, Chain,
    CreateBalanceSnapshotRequest, CreateProviderRequest, CreateWatchedAddressRequest,
    EventQuery, Provider, ScanAddressCandidate, ScanAddressContext, ScanCursor, Tenant, User,
    WatchedAddress,
},
```

Add these constants near existing query constants before implementing them:

```rust
pub const ACTIVE_ERC20_ASSETS_QUERY: &str = r#""#;
pub const SCAN_CURSOR_QUERY: &str = r#""#;
pub const UPSERT_SCAN_CURSOR_QUERY: &str = r#""#;
pub const INSERT_EVENT_IF_NOT_EXISTS_QUERY: &str = r#""#;
```

Add these tests inside the existing repository tests module:

```rust
#[test]
fn active_erc20_assets_query_filters_active_contract_assets() {
    assert!(ACTIVE_ERC20_ASSETS_QUERY.contains("asset_type = 'erc20'"));
    assert!(ACTIVE_ERC20_ASSETS_QUERY.contains("status = 'active'"));
    assert!(ACTIVE_ERC20_ASSETS_QUERY.contains("contract_address IS NOT NULL"));
}

#[test]
fn scan_cursor_queries_use_address_and_cursor_type() {
    assert!(SCAN_CURSOR_QUERY.contains("address_id = $1"));
    assert!(SCAN_CURSOR_QUERY.contains("cursor_type = $2"));
    assert!(UPSERT_SCAN_CURSOR_QUERY.contains("ON CONFLICT (address_id, cursor_type)"));
    assert!(UPSERT_SCAN_CURSOR_QUERY.contains("last_scanned_block = EXCLUDED.last_scanned_block"));
}

#[test]
fn insert_event_if_not_exists_query_returns_optional_event() {
    assert!(INSERT_EVENT_IF_NOT_EXISTS_QUERY.contains("ON CONFLICT DO NOTHING"));
    assert!(INSERT_EVENT_IF_NOT_EXISTS_QUERY.contains("RETURNING id"));
    assert!(INSERT_EVENT_IF_NOT_EXISTS_QUERY.contains("address_events"));
}
```

- [ ] **Step 2: Run repository tests to verify RED**

Run:

```bash
cargo test -p coin-listener-storage active_erc20_assets_query --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage scan_cursor_queries --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage insert_event_if_not_exists_query --manifest-path backend/Cargo.toml
```

Expected: FAIL because query constants are empty or model imports do not compile.

- [ ] **Step 3: Add repository query constants**

Replace the empty constants with:

```rust
pub const ACTIVE_ERC20_ASSETS_QUERY: &str = r#"
SELECT id, chain_id, asset_type, symbol, name, contract_address, decimals, is_builtin, status
FROM assets
WHERE chain_id = $1
  AND asset_type = 'erc20'
  AND status = 'active'
  AND contract_address IS NOT NULL
ORDER BY symbol, name
"#;

pub const SCAN_CURSOR_QUERY: &str = r#"
SELECT id, tenant_id, chain_id, address_id, cursor_type, last_scanned_block, updated_at
FROM scan_cursors
WHERE address_id = $1
  AND cursor_type = $2
"#;

pub const UPSERT_SCAN_CURSOR_QUERY: &str = r#"
INSERT INTO scan_cursors (tenant_id, chain_id, address_id, cursor_type, last_scanned_block)
VALUES ($1, $2, $3, $4, $5)
ON CONFLICT (address_id, cursor_type)
DO UPDATE SET last_scanned_block = EXCLUDED.last_scanned_block,
              updated_at = NOW()
RETURNING id, tenant_id, chain_id, address_id, cursor_type, last_scanned_block, updated_at
"#;

pub const INSERT_EVENT_IF_NOT_EXISTS_QUERY: &str = r#"
INSERT INTO address_events (
    tenant_id, chain_id, address_id, asset_id, event_type, direction, is_transfer,
    tx_hash, log_index, block_number, block_hash, confirmations, from_address, to_address,
    amount_raw, amount_decimal, balance_before_raw, balance_after_raw, balance_delta_raw, metadata
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)
ON CONFLICT DO NOTHING
RETURNING id, tenant_id, chain_id, address_id, asset_id, event_type, direction, is_transfer,
          tx_hash, log_index, block_number, block_hash, confirmations, from_address, to_address,
          amount_raw, amount_decimal, balance_before_raw, balance_after_raw, balance_delta_raw,
          metadata, detected_at, created_at
"#;
```

- [ ] **Step 4: Add repository functions**

Add these functions after `native_asset_for_chain`:

```rust
pub async fn chain_by_id(pool: &PgPool, chain_id: Uuid) -> AppResult<Chain> {
    get_chain(pool, chain_id).await
}

pub async fn active_erc20_assets_for_chain(pool: &PgPool, chain_id: Uuid) -> AppResult<Vec<Asset>> {
    sqlx::query_as::<_, Asset>(ACTIVE_ERC20_ASSETS_QUERY)
        .bind(chain_id)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn scan_cursor(
    pool: &PgPool,
    address_id: Uuid,
    cursor_type: &str,
) -> AppResult<Option<ScanCursor>> {
    sqlx::query_as::<_, ScanCursor>(SCAN_CURSOR_QUERY)
        .bind(address_id)
        .bind(cursor_type)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn upsert_scan_cursor(
    pool: &PgPool,
    tenant_id: Uuid,
    chain_id: Uuid,
    address_id: Uuid,
    cursor_type: &str,
    last_scanned_block: i64,
) -> AppResult<ScanCursor> {
    sqlx::query_as::<_, ScanCursor>(UPSERT_SCAN_CURSOR_QUERY)
        .bind(tenant_id)
        .bind(chain_id)
        .bind(address_id)
        .bind(cursor_type)
        .bind(last_scanned_block)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}
```

Add this function after `insert_event`:

```rust
pub async fn insert_event_if_not_exists(
    pool: &PgPool,
    draft: AddressEventDraft,
) -> AppResult<Option<AddressEvent>> {
    sqlx::query_as::<_, AddressEvent>(INSERT_EVENT_IF_NOT_EXISTS_QUERY)
        .bind(draft.tenant_id)
        .bind(draft.chain_id)
        .bind(draft.address_id)
        .bind(draft.asset_id)
        .bind(draft.event_type)
        .bind(draft.direction)
        .bind(draft.is_transfer)
        .bind(draft.tx_hash)
        .bind(draft.log_index)
        .bind(draft.block_number)
        .bind(draft.block_hash)
        .bind(draft.confirmations)
        .bind(draft.from_address)
        .bind(draft.to_address)
        .bind(draft.amount_raw)
        .bind(draft.amount_decimal)
        .bind(draft.balance_before_raw)
        .bind(draft.balance_after_raw)
        .bind(draft.balance_delta_raw)
        .bind(draft.metadata)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}
```

- [ ] **Step 5: Run repository tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-storage active_erc20_assets_query --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage scan_cursor_queries --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage insert_event_if_not_exists_query --manifest-path backend/Cargo.toml
```

Expected: all three commands pass.

- [ ] **Step 6: Run storage check**

Run:

```bash
cargo check -p coin-listener-storage --manifest-path backend/Cargo.toml
cargo fmt -p coin-listener-storage --check --manifest-path backend/Cargo.toml
```

Expected: both commands exit 0.

- [ ] **Step 7: Checkpoint**

If `git status` succeeds, run:

```bash
git add backend/crates/storage/src/repositories.rs
git commit -m "Add EVM transfer scan storage helpers"
```

Expected outside a git repository: skip commit and report changed files.

---

### Task 4: Add worker block-range and ERC20 scan orchestration helpers

**Files:**
- Modify: `backend/crates/worker/src/lib.rs`

- [ ] **Step 1: Write failing worker range and event collection tests**

In `backend/crates/worker/src/lib.rs`, update imports:

```rust
use coin_listener_chain_providers::evm::{
    self, address_to_topic, evm_balance_change_event, parse_hex_u256_to_decimal_string,
    transfer_event_draft, wei_to_decimal_string, EvmBlockTag, EvmLogFilter, EvmRpcClient,
    TRANSFER_TOPIC0,
};
```

Update core model imports:

```rust
models::{
    AddressEvent, Asset, BalanceSnapshot, CreateBalanceSnapshotRequest, NotifyEventTask, Provider,
    ScanAddressContext, ScanAddressTask, ScanCursor,
},
```

Add these constants near the top of the file:

```rust
pub const EVM_ERC20_TRANSFER_CURSOR: &str = "evm_erc20_transfer";
pub const EVM_TRANSFER_INITIAL_WINDOW_BLOCKS: i64 = 1_000;
```

Add this test module inside `#[cfg(test)] mod tests`:

```rust
mod evm_transfer_ranges {
    use chrono::{TimeZone, Utc};
    use coin_listener_core::models::ScanCursor;
    use uuid::Uuid;

    use crate::{evm_transfer_scan_range, EVM_ERC20_TRANSFER_CURSOR};

    fn cursor(last_scanned_block: i64) -> ScanCursor {
        ScanCursor {
            id: Uuid::from_u128(1),
            tenant_id: Uuid::from_u128(2),
            chain_id: Uuid::from_u128(3),
            address_id: Uuid::from_u128(4),
            cursor_type: EVM_ERC20_TRANSFER_CURSOR.to_string(),
            last_scanned_block,
            updated_at: Utc.with_ymd_and_hms(2026, 5, 17, 23, 0, 0).unwrap(),
        }
    }

    #[test]
    fn initial_range_uses_latest_confirmed_window() {
        let range = evm_transfer_scan_range(None, 20_000, 12).unwrap();

        assert_eq!(range, Some((18_989, 19_988)));
    }

    #[test]
    fn cursor_range_starts_after_last_scanned_block() {
        let range = evm_transfer_scan_range(Some(&cursor(19_900)), 20_000, 12).unwrap();

        assert_eq!(range, Some((19_901, 19_988)));
    }

    #[test]
    fn cursor_ahead_of_confirmed_block_is_noop() {
        let range = evm_transfer_scan_range(Some(&cursor(19_988)), 20_000, 12).unwrap();

        assert_eq!(range, None);
    }

    #[test]
    fn confirmations_cannot_be_negative() {
        let error = evm_transfer_scan_range(None, 20_000, -1).unwrap_err();

        assert!(error.to_string().contains("default_confirmations"));
    }
}
```

- [ ] **Step 2: Run worker range tests to verify RED**

Run:

```bash
cargo test -p worker evm_transfer_ranges --manifest-path backend/Cargo.toml
```

Expected: FAIL because `evm_transfer_scan_range` and constants are missing.

- [ ] **Step 3: Implement range calculation**

Add this function after `should_emit_balance_change`:

```rust
pub fn evm_transfer_scan_range(
    cursor: Option<&ScanCursor>,
    latest_block: i64,
    default_confirmations: i32,
) -> AppResult<Option<(i64, i64)>> {
    if default_confirmations < 0 {
        return Err(AppError::Validation(
            "default_confirmations cannot be negative".to_string(),
        ));
    }
    let confirmed_to = latest_block - i64::from(default_confirmations);
    if confirmed_to < 0 {
        return Ok(None);
    }
    let from_block = cursor
        .map(|cursor| cursor.last_scanned_block + 1)
        .unwrap_or_else(|| (confirmed_to - EVM_TRANSFER_INITIAL_WINDOW_BLOCKS + 1).max(0));
    if confirmed_to < from_block {
        return Ok(None);
    }
    Ok(Some((from_block, confirmed_to)))
}
```

- [ ] **Step 4: Add native scan helper that reuses loaded context and RPC client**

Replace the body of existing `scan_evm_native_balance` with:

```rust
let context = repositories::get_scan_address_context(pool, task.address_id).await?;
let asset = repositories::native_asset_for_chain(pool, context.chain_id).await?;
let provider = repositories::active_rpc_provider_for_chain(pool, context.chain_id).await?;
let timeout_ms = u64::try_from(provider.timeout_ms)
    .map_err(|_| AppError::Validation("timeout_ms must be positive".to_string()))?;
if timeout_ms == 0 {
    return Err(AppError::Validation(
        "timeout_ms must be positive".to_string(),
    ));
}

let rpc = EvmRpcClient::new(provider.base_url.clone(), Duration::from_millis(timeout_ms));
let block_number = rpc.eth_block_number().await?;
scan_evm_native_balance_with_context(pool, &rpc, &context, &asset, &provider, block_number).await
```

Add this helper after `scan_evm_native_balance`:

```rust
async fn scan_evm_native_balance_with_context(
    pool: &PgPool,
    rpc: &EvmRpcClient,
    context: &ScanAddressContext,
    asset: &Asset,
    provider: &Provider,
    block_number: i64,
) -> AppResult<Option<AddressEvent>> {
    let balance_hex = rpc
        .eth_get_balance(&context.address, EvmBlockTag::Latest)
        .await?;
    let balance_raw = parse_hex_u256_to_decimal_string(&balance_hex)?;
    let balance_decimal = wei_to_decimal_string(&balance_raw, asset.decimals)?;
    let current = repositories::insert_balance_snapshot(
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
    let previous =
        repositories::latest_balance_snapshot(pool, context.id, asset.id, Some(current.id)).await?;
    if !should_emit_balance_change(previous.as_ref(), &current) {
        return Ok(None);
    }

    let previous = previous.expect("previous snapshot checked before event creation");
    let draft = evm_balance_change_event(context, asset, &previous, &current, provider)?;
    repositories::insert_event(pool, draft).await.map(Some)
}
```

- [ ] **Step 5: Add ERC20 transfer scan helper**

Add this function after `scan_evm_native_balance_with_context`:

```rust
pub async fn scan_evm_erc20_transfers(
    pool: &PgPool,
    rpc: &EvmRpcClient,
    context: &ScanAddressContext,
    latest_block: i64,
    default_confirmations: i32,
) -> AppResult<Vec<AddressEvent>> {
    let cursor = repositories::scan_cursor(pool, context.id, EVM_ERC20_TRANSFER_CURSOR).await?;
    let Some((from_block, to_block)) =
        evm_transfer_scan_range(cursor.as_ref(), latest_block, default_confirmations)?
    else {
        return Ok(Vec::new());
    };

    let assets = repositories::active_erc20_assets_for_chain(pool, context.chain_id).await?;
    if assets.is_empty() {
        return Ok(Vec::new());
    }

    let watched_topic = address_to_topic(&context.address)?;
    let mut events = Vec::new();

    for asset in assets {
        let Some(contract_address) = asset.contract_address.clone() else {
            continue;
        };
        let incoming = EvmLogFilter {
            address: contract_address.clone(),
            from_block,
            to_block,
            topics: vec![
                Some(TRANSFER_TOPIC0.to_string()),
                None,
                Some(watched_topic.clone()),
            ],
        };
        let outgoing = EvmLogFilter {
            address: contract_address,
            from_block,
            to_block,
            topics: vec![
                Some(TRANSFER_TOPIC0.to_string()),
                Some(watched_topic.clone()),
                None,
            ],
        };

        for filter in [incoming, outgoing] {
            let logs = rpc.eth_get_logs(filter).await?;
            for log in logs {
                let transfer = evm::decode_erc20_transfer_log(&log, asset.decimals)?;
                let draft = transfer_event_draft(context, &asset, transfer);
                if let Some(event) = repositories::insert_event_if_not_exists(pool, draft).await? {
                    events.push(event);
                }
            }
        }
    }

    repositories::upsert_scan_cursor(
        pool,
        context.tenant_id,
        context.chain_id,
        context.id,
        EVM_ERC20_TRANSFER_CURSOR,
        to_block,
    )
    .await?;

    Ok(events)
}
```

- [ ] **Step 6: Add combined EVM scan helper and update process branch**

Add this function after `scan_evm_erc20_transfers`:

```rust
pub async fn scan_evm_address(
    pool: &PgPool,
    task: &ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<Vec<AddressEvent>> {
    let context = repositories::get_scan_address_context(pool, task.address_id).await?;
    let provider = repositories::active_rpc_provider_for_chain(pool, context.chain_id).await?;
    let chain = repositories::chain_by_id(pool, context.chain_id).await?;
    let native_asset = repositories::native_asset_for_chain(pool, context.chain_id).await?;
    let timeout_ms = u64::try_from(provider.timeout_ms)
        .map_err(|_| AppError::Validation("timeout_ms must be positive".to_string()))?;
    if timeout_ms == 0 {
        return Err(AppError::Validation(
            "timeout_ms must be positive".to_string(),
        ));
    }
    let rpc = EvmRpcClient::new(provider.base_url.clone(), Duration::from_millis(timeout_ms));
    let latest_block = rpc.eth_block_number().await?;

    let mut events = Vec::new();
    if let Some(event) = scan_evm_native_balance_with_context(
        pool,
        &rpc,
        &context,
        &native_asset,
        &provider,
        latest_block,
    )
    .await?
    {
        events.push(event);
    }
    events.extend(
        scan_evm_erc20_transfers(
            pool,
            &rpc,
            &context,
            latest_block,
            chain.default_confirmations,
        )
        .await?,
    );
    Ok(events)
}
```

In `process_locked_scan_task`, replace the EVM branch with:

```rust
ScanPlan::EvmNativeBalance => {
    let events = scan_evm_address(pool, task, now).await?;
    for event in &events {
        let notify_task = build_notify_event_task(event, now);
        notify_queue.enqueue(redis, &notify_task).await?;
    }
    repositories::finish_address_scan(pool, task.address_id, now, next_scan_at).await?;
    Ok(ScanTaskOutcome::Scanned)
}
```

- [ ] **Step 7: Run worker range tests to verify GREEN**

Run:

```bash
cargo test -p worker evm_transfer_ranges --manifest-path backend/Cargo.toml
```

Expected: PASS for all range tests.

- [ ] **Step 8: Run worker regression tests**

Run:

```bash
cargo test -p worker scan_plan_for_chain --manifest-path backend/Cargo.toml
cargo test -p worker balance_change_gating --manifest-path backend/Cargo.toml
cargo test -p worker build_notify_event_task --manifest-path backend/Cargo.toml
cargo check -p worker --manifest-path backend/Cargo.toml
cargo fmt -p worker --check --manifest-path backend/Cargo.toml
```

Expected: all commands pass.

- [ ] **Step 9: Checkpoint**

If `git status` succeeds, run:

```bash
git add backend/crates/worker/src/lib.rs
git commit -m "Scan EVM ERC20 transfer logs in worker"
```

Expected outside a git repository: skip commit and report changed files.

---

### Task 5: Final verification

**Files:**
- Verify: `backend/crates/core/src/models.rs`
- Verify: `backend/crates/storage/migrations/0006_scan_cursors.sql`
- Verify: `backend/crates/storage/src/repositories.rs`
- Verify: `backend/crates/chain-providers/src/evm.rs`
- Verify: `backend/crates/worker/src/lib.rs`
- Verify: `frontend/`
- Verify: `docker-compose.yml`

- [ ] **Step 1: Run backend formatting check**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: exit 0. If it fails, run `cargo fmt --all --manifest-path backend/Cargo.toml`, then rerun the check.

- [ ] **Step 2: Run backend workspace check**

Run:

```bash
cargo check --workspace --manifest-path backend/Cargo.toml
```

Expected: exit 0.

- [ ] **Step 3: Run backend workspace tests**

Run:

```bash
cargo test --workspace --manifest-path backend/Cargo.toml
```

Expected: all Rust unit tests and doc-tests pass.

- [ ] **Step 4: Run frontend build regression**

Run:

```bash
npm run build --prefix frontend
```

Expected: exit 0. Existing Vite warnings about `lottie-web` direct eval or chunk size may appear, but there must be no build failure.

- [ ] **Step 5: Validate Docker Compose configuration**

Run:

```bash
docker compose -f docker-compose.yml config
```

Expected: exit 0 and rendered compose configuration.

- [ ] **Step 6: Final checkpoint**

If `git status` succeeds, run:

```bash
git add backend/crates/core/src/models.rs backend/crates/storage/migrations/0006_scan_cursors.sql backend/crates/storage/src/repositories.rs backend/crates/chain-providers/src/evm.rs backend/crates/worker/src/lib.rs
git commit -m "Implement EVM ERC20 transfer log scanning"
```

Expected outside a git repository: skip commit and report changed files.

---

## Self-Review Notes

- Spec coverage: Tasks 1-4 cover scan cursor persistence, `eth_getLogs`, Transfer topic/log decoding, ERC20 asset lookup, idempotent event insert, worker range calculation, notification gating, and scan completion ordering. Task 5 covers final workspace verification.
- Scope control: The plan does not add native ETH transaction scanning, WebSocket subscriptions, provider failover, ERC20 token discovery, frontend changes, or complex reorg rollback.
- Existing index alignment: The plan reuses the existing `idx_address_events_unique_transfer` semantics through `ON CONFLICT DO NOTHING`; it does not add a duplicate transfer unique index.
- Failure behavior: RPC/provider/database/log validation errors propagate before cursor advancement, notify enqueue, and `finish_address_scan`; duplicate events return `None` and do not enqueue notifications.
- Plan self-review fixes applied: removed an extra cursor index not present in the approved spec, added `Asset`/`Provider` imports required by the worker helper, and changed the combined EVM scan to reuse one loaded context/provider/RPC block number for native balance plus ERC20 transfer scanning.
