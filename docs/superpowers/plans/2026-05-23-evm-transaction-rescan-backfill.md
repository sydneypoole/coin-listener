# EVM Transaction Rescan Backfill Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add transaction-hash EVM rescan/backfill so Base authorization/batch-token transactions are classified from receipt `Transfer` logs instead of being confused with native ETH gas balance changes.

**Architecture:** Extend the existing EVM JSON-RPC client to fetch transactions and receipts, add pure transaction-classification helpers that convert receipt logs into existing `AddressEventDraft`s, expose a protected rescan API, and add a small Events page rescan form. Reuse `address_events` and existing idempotent insert paths; do not move normal scan cursors during one-off rescans.

**Tech Stack:** Rust 2021, Tokio, Axum, SQLx, PostgreSQL, reqwest JSON-RPC, serde/serde_json, uuid, React, TypeScript, TanStack Query, Semi UI.

---

## File Structure

Modify:

```text
backend/crates/core/src/models.rs
backend/crates/chain-providers/src/evm.rs
backend/crates/storage/src/repositories.rs
backend/crates/api-server/src/routes.rs
frontend/src/api/types.ts
frontend/src/api/client.ts
frontend/src/pages/EventsPage.tsx
```

No database migration is required. The implementation stores rescan-generated data in existing `address_events` columns and `metadata` JSON.

Responsibility boundaries:

| File | Responsibility |
|---|---|
| `core/src/models.rs` | API request/response DTOs shared by backend crates. |
| `chain-providers/src/evm.rs` | EVM JSON-RPC data shapes, receipt/transaction parsing, pure event-draft classification. No DB access. |
| `storage/src/repositories.rs` | Tenant-scoped watched-address/asset lookup helpers and idempotent event insertion reuse. |
| `api-server/src/routes.rs` | Protected `/api/evm/transactions/rescan` orchestration and route tests. |
| `frontend/src/api/types.ts` | TypeScript DTOs for rescan API. |
| `frontend/src/api/client.ts` | API client wrapper for rescan POST. |
| `frontend/src/pages/EventsPage.tsx` | Small rescan form and result summary in Event Center. |

---

### Task 1: Add shared rescan DTOs

**Files:**
- Modify: `backend/crates/core/src/models.rs`
- Modify: `frontend/src/api/types.ts`

- [ ] **Step 1: Write failing Rust DTO deserialization test**

In `backend/crates/core/src/models.rs`, update the test import list in `#[cfg(test)] mod tests` to include the new types:

```rust
use super::{
    AddressEvent, CreateBalanceSnapshotRequest, CreateTelegramBindingRequest,
    CreateTelegramBotRequest, CreateWatchedAddressImportRequest, CreateWatchedAddressRequest,
    EvmTransactionRescanRequest, EvmTransactionRescanResponse, EvmTransactionRescanSummary,
    EvmTransactionRescanTransferSummary, EventStatus, NotificationDelivery,
    NotificationDeliveryListItem, NotificationDeliveryListResponse, NotificationDeliveryQuery,
    NotificationOutboxDetail, NotificationOutboxItem, NotificationOutboxListItem,
    NotificationOutboxListResponse, NotificationOutboxQuery, NotificationStatus,
    NotifyEventTask, OutboxStatusCounts, ProviderChainStatus, ProviderHealthStatus,
    ProviderStatus, ProviderStatusItem, QueueStatus, RetryNotificationOutboxResponse,
    ScanAddressTask, ScanCursor, ScanStatus, ServiceHealthStatus, ServiceHeartbeatStatusItem,
    SystemStatus, TelegramBindingRequest, UpdateTelegramBotRequest,
    WatchedAddressImportChainConfig, WatchedAddressImportDefaults, WatchedAddressImportErrorRow,
    WatchedAddressImportTask, WatchedAddressResponse,
};
```

Add this test inside the same test module:

```rust
#[test]
fn evm_transaction_rescan_request_and_response_round_trip_json() {
    let request_payload = r#"{
        "chain_id":"10000000-0000-0000-0000-000000000004",
        "tx_hash":"0x7e88e5d67ead0c0605f3bed96071ec4be14112bed2d929ee57e5b161bf6f2389"
    }"#;

    let request: EvmTransactionRescanRequest = serde_json::from_str(request_payload).unwrap();
    assert_eq!(
        request.chain_id,
        Uuid::parse_str("10000000-0000-0000-0000-000000000004").unwrap()
    );
    assert_eq!(
        request.tx_hash,
        "0x7e88e5d67ead0c0605f3bed96071ec4be14112bed2d929ee57e5b161bf6f2389"
    );

    let response = EvmTransactionRescanResponse {
        summary: EvmTransactionRescanSummary {
            chain_id: request.chain_id,
            tx_hash: request.tx_hash.clone(),
            tx_from: "0xa9236f4950001355455a5b016a25fa27b947c9ac".to_string(),
            tx_to: Some("0x887749abb233682aa7d5594a54659c51501445b1".to_string()),
            native_value_raw: "0".to_string(),
            block_number: 46_345_642,
            token_transfer_count: 1,
            inserted_event_count: 1,
            skipped_event_count: 0,
        },
        token_transfers: vec![EvmTransactionRescanTransferSummary {
            asset_id: Uuid::from_u128(44),
            symbol: "USDC".to_string(),
            token_contract: "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".to_string(),
            from_address: "0x70196e53fa11b4621290144ccc8f4624ddff1058".to_string(),
            to_address: "0x65722a6603b00f31922bc39737cc7ee24cd3d862".to_string(),
            amount_raw: "100000".to_string(),
            amount_decimal: "0.1".to_string(),
            log_index: 937,
        }],
        events: Vec::new(),
    };

    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["summary"]["token_transfer_count"], 1);
    assert_eq!(json["summary"]["inserted_event_count"], 1);
    assert_eq!(json["token_transfers"][0]["symbol"], "USDC");
}
```

- [ ] **Step 2: Run the DTO test and verify RED**

Run:

```bash
cargo test -p coin-listener-core evm_transaction_rescan_request_and_response_round_trip_json --manifest-path backend/Cargo.toml
```

Expected: FAIL because the rescan DTO types do not exist.

- [ ] **Step 3: Add Rust DTOs**

In `backend/crates/core/src/models.rs`, add these structs after `EventQuery`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct EvmTransactionRescanRequest {
    pub chain_id: Uuid,
    pub tx_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvmTransactionRescanTransferSummary {
    pub asset_id: Uuid,
    pub symbol: String,
    pub token_contract: String,
    pub from_address: String,
    pub to_address: String,
    pub amount_raw: String,
    pub amount_decimal: String,
    pub log_index: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvmTransactionRescanSummary {
    pub chain_id: Uuid,
    pub tx_hash: String,
    pub tx_from: String,
    pub tx_to: Option<String>,
    pub native_value_raw: String,
    pub block_number: i64,
    pub token_transfer_count: usize,
    pub inserted_event_count: usize,
    pub skipped_event_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvmTransactionRescanResponse {
    pub summary: EvmTransactionRescanSummary,
    pub token_transfers: Vec<EvmTransactionRescanTransferSummary>,
    pub events: Vec<AddressEvent>,
}
```

- [ ] **Step 4: Run the Rust DTO test and verify GREEN**

Run:

```bash
cargo test -p coin-listener-core evm_transaction_rescan_request_and_response_round_trip_json --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 5: Add frontend DTOs**

In `frontend/src/api/types.ts`, add these types after `EventQuery`:

```ts
export type EvmTransactionRescanRequest = {
  chain_id: string;
  tx_hash: string;
};

export type EvmTransactionRescanTransferSummary = {
  asset_id: string;
  symbol: string;
  token_contract: string;
  from_address: string;
  to_address: string;
  amount_raw: string;
  amount_decimal: string;
  log_index: number;
};

export type EvmTransactionRescanSummary = {
  chain_id: string;
  tx_hash: string;
  tx_from: string;
  tx_to?: string | null;
  native_value_raw: string;
  block_number: number;
  token_transfer_count: number;
  inserted_event_count: number;
  skipped_event_count: number;
};

export type EvmTransactionRescanResponse = {
  summary: EvmTransactionRescanSummary;
  token_transfers: EvmTransactionRescanTransferSummary[];
  events: AddressEvent[];
};
```

- [ ] **Step 6: Run core check**

Run:

```bash
cargo check -p coin-listener-core --manifest-path backend/Cargo.toml
```

Expected: exit 0.

---

### Task 2: Add EVM transaction and receipt RPC parsing

**Files:**
- Modify: `backend/crates/chain-providers/src/evm.rs`

- [ ] **Step 1: Write failing transaction/receipt parser tests**

In `backend/crates/chain-providers/src/evm.rs`, update the test imports to include the new functions and structs:

```rust
use super::{
    address_to_topic, build_json_rpc_request, decode_erc20_transfer_log,
    decode_rescan_token_transfers, evm_balance_change_event, evm_fee_only_event_draft,
    format_rpc_request_error_with_sources, mock_evm_transfer, parse_hex_quantity_to_i64,
    parse_hex_u256_to_decimal_string, parse_json_rpc_hex_result,
    parse_json_rpc_transaction_receipt_result, parse_json_rpc_transaction_result, topic_to_address,
    transfer_event_draft, transfer_event_draft_with_source, wei_to_decimal_string,
    DecodedErc20Transfer, EvmBlockTag, EvmLog, EvmLogFilter, EvmTransaction,
    EvmTransactionReceipt, TRANSFER_TOPIC0,
};
```

Add these tests inside the existing test module:

```rust
#[test]
fn evm_transaction_json_decodes_zero_native_value_contract_call() {
    let payload = r#"{
      "jsonrpc":"2.0",
      "id":1,
      "result":{
        "hash":"0x7e88e5d67ead0c0605f3bed96071ec4be14112bed2d929ee57e5b161bf6f2389",
        "from":"0xa9236f4950001355455a5b016a25fa27b947c9ac",
        "to":"0x887749abb233682aa7d5594a54659c51501445b1",
        "value":"0x0",
        "blockNumber":"0x2c32daa",
        "blockHash":"0xb1aa002fc5fc438301e27470e81ad06c69e601565d730b8c8a66d5ced9090c8f",
        "input":"0xcccbb34c"
      }
    }"#;

    let tx = parse_json_rpc_transaction_result(payload, "eth_getTransactionByHash").unwrap();

    assert_eq!(
        tx.hash,
        "0x7e88e5d67ead0c0605f3bed96071ec4be14112bed2d929ee57e5b161bf6f2389"
    );
    assert_eq!(tx.from, "0xa9236f4950001355455a5b016a25fa27b947c9ac");
    assert_eq!(tx.to.as_deref(), Some("0x887749abb233682aa7d5594a54659c51501445b1"));
    assert_eq!(tx.value, "0x0");
    assert_eq!(tx.block_number.as_deref(), Some("0x2c32daa"));
}

#[test]
fn evm_transaction_receipt_json_decodes_transfer_logs() {
    let payload = r#"{
      "jsonrpc":"2.0",
      "id":1,
      "result":{
        "transactionHash":"0x7e88e5d67ead0c0605f3bed96071ec4be14112bed2d929ee57e5b161bf6f2389",
        "status":"0x1",
        "blockNumber":"0x2c32daa",
        "blockHash":"0xb1aa002fc5fc438301e27470e81ad06c69e601565d730b8c8a66d5ced9090c8f",
        "from":"0xa9236f4950001355455a5b016a25fa27b947c9ac",
        "to":"0x887749abb233682aa7d5594a54659c51501445b1",
        "logs":[
          {
            "address":"0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
            "topics":[
              "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef",
              "0x00000000000000000000000070196e53fa11b4621290144ccc8f4624ddff1058",
              "0x00000000000000000000000065722a6603b00f31922bc39737cc7ee24cd3d862"
            ],
            "data":"0x00000000000000000000000000000000000000000000000000000000000186a0",
            "transactionHash":"0x7e88e5d67ead0c0605f3bed96071ec4be14112bed2d929ee57e5b161bf6f2389",
            "logIndex":"0x3a9",
            "blockNumber":"0x2c32daa",
            "blockHash":"0xb1aa002fc5fc438301e27470e81ad06c69e601565d730b8c8a66d5ced9090c8f"
          }
        ]
      }
    }"#;

    let receipt = parse_json_rpc_transaction_receipt_result(payload, "eth_getTransactionReceipt").unwrap();

    assert_eq!(receipt.status.as_deref(), Some("0x1"));
    assert_eq!(receipt.logs.len(), 1);
    assert_eq!(receipt.logs[0].log_index.as_deref(), Some("0x3a9"));
}
```

- [ ] **Step 2: Run parser tests and verify RED**

Run:

```bash
cargo test -p coin-listener-chain-providers evm_transaction_ --manifest-path backend/Cargo.toml
```

Expected: FAIL because transaction/receipt structs and parsers do not exist.

- [ ] **Step 3: Add transaction and receipt structs**

In `backend/crates/chain-providers/src/evm.rs`, add these structs after `EvmLog`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EvmTransaction {
    pub hash: String,
    pub from: String,
    pub to: Option<String>,
    pub value: String,
    #[serde(rename = "blockNumber")]
    pub block_number: Option<String>,
    #[serde(rename = "blockHash")]
    pub block_hash: Option<String>,
    pub input: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EvmTransactionReceipt {
    #[serde(rename = "transactionHash")]
    pub transaction_hash: String,
    pub status: Option<String>,
    #[serde(rename = "blockNumber")]
    pub block_number: Option<String>,
    #[serde(rename = "blockHash")]
    pub block_hash: Option<String>,
    pub from: String,
    pub to: Option<String>,
    pub logs: Vec<EvmLog>,
}
```

- [ ] **Step 4: Add JSON-RPC client methods and parsers**

In `impl EvmRpcClient`, after `eth_get_logs`, add:

```rust
pub async fn eth_get_transaction_by_hash(&self, tx_hash: &str) -> AppResult<EvmTransaction> {
    let body = self
        .rpc_result_body("eth_getTransactionByHash", json!([tx_hash]))
        .await?;
    parse_json_rpc_transaction_result(&body, "eth_getTransactionByHash")
}

pub async fn eth_get_transaction_receipt(&self, tx_hash: &str) -> AppResult<EvmTransactionReceipt> {
    let body = self
        .rpc_result_body("eth_getTransactionReceipt", json!([tx_hash]))
        .await?;
    parse_json_rpc_transaction_receipt_result(&body, "eth_getTransactionReceipt")
}
```

After `parse_json_rpc_logs_result`, add:

```rust
pub fn parse_json_rpc_transaction_result(payload: &str, method: &str) -> AppResult<EvmTransaction> {
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
    serde_json::from_value(result).map_err(|error| {
        AppError::Validation(format!("invalid evm rpc {method} transaction result: {error}"))
    })
}

pub fn parse_json_rpc_transaction_receipt_result(
    payload: &str,
    method: &str,
) -> AppResult<EvmTransactionReceipt> {
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
    serde_json::from_value(result).map_err(|error| {
        AppError::Validation(format!("invalid evm rpc {method} receipt result: {error}"))
    })
}
```

- [ ] **Step 5: Run parser tests and verify GREEN**

Run:

```bash
cargo test -p coin-listener-chain-providers evm_transaction_ --manifest-path backend/Cargo.toml
```

Expected: PASS for the two new parser tests.

---

### Task 3: Add pure EVM rescan classification helpers

**Files:**
- Modify: `backend/crates/chain-providers/src/evm.rs`

- [ ] **Step 1: Write failing classification tests**

In `backend/crates/chain-providers/src/evm.rs`, add these tests inside the existing test module:

```rust
#[test]
fn rescan_decodes_selected_usdc_transfer_from_receipt() {
    let chain_id = Uuid::from_u128(103);
    let usdc = Asset {
        id: Uuid::from_u128(204),
        chain_id,
        asset_type: "erc20".to_string(),
        symbol: "USDC".to_string(),
        name: "USD Coin".to_string(),
        contract_address: Some("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".to_string()),
        decimals: 6,
        is_builtin: true,
        status: "active".to_string(),
    };
    let receipt = sample_rescan_receipt();

    let transfers = decode_rescan_token_transfers(&receipt, std::slice::from_ref(&usdc)).unwrap();

    assert_eq!(transfers.len(), 1);
    assert_eq!(transfers[0].asset.id, usdc.id);
    assert_eq!(transfers[0].transfer.amount_raw, "100000");
    assert_eq!(transfers[0].transfer.amount_decimal, "0.1");
    assert_eq!(transfers[0].transfer.log_index, 937);
}

#[test]
fn rescan_transfer_event_marks_source_and_tx_context() {
    let context = scan_context_with_address("0x65722a6603b00f31922bc39737cc7ee24cd3d862");
    let asset = erc20_asset(context.chain_id);
    let transfer = decoded_transfer(
        "0x70196e53fa11b4621290144ccc8f4624ddff1058",
        "0x65722a6603b00f31922bc39737cc7ee24cd3d862",
    );
    let tx = sample_rescan_transaction();

    let draft = transfer_event_draft_with_source(&context, &asset, transfer, "evm_tx_rescan", &tx).unwrap();

    assert_eq!(draft.event_type, "transfer");
    assert!(draft.is_transfer);
    assert_eq!(draft.direction, "in");
    assert_eq!(draft.asset_id, asset.id);
    assert_eq!(draft.metadata["source"], "evm_tx_rescan");
    assert_eq!(draft.metadata["tx_from"], "0xa9236f4950001355455a5b016a25fa27b947c9ac");
    assert_eq!(draft.metadata["native_value_raw"], "0");
}

#[test]
fn fee_only_event_is_not_a_transfer_and_uses_native_asset() {
    let context = scan_context_with_address("0xa9236f4950001355455a5b016a25fa27b947c9ac");
    let native = native_asset(context.chain_id);
    let tx = sample_rescan_transaction();

    let draft = evm_fee_only_event_draft(&context, &native, &tx, "evm_tx_rescan").unwrap();

    assert_eq!(draft.event_type, "fee_only_change");
    assert!(!draft.is_transfer);
    assert_eq!(draft.direction, "out");
    assert_eq!(draft.asset_id, native.id);
    assert_eq!(draft.tx_hash.as_deref(), Some(&tx.hash));
    assert_eq!(draft.log_index, None);
    assert_eq!(draft.amount_raw, None);
    assert_eq!(draft.metadata["source"], "evm_tx_rescan");
    assert_eq!(draft.metadata["native_value_raw"], "0");
}
```

Add these helper functions inside the test module near the existing helpers:

```rust
fn sample_rescan_transaction() -> EvmTransaction {
    EvmTransaction {
        hash: "0x7e88e5d67ead0c0605f3bed96071ec4be14112bed2d929ee57e5b161bf6f2389".to_string(),
        from: "0xa9236f4950001355455a5b016a25fa27b947c9ac".to_string(),
        to: Some("0x887749abb233682aa7d5594a54659c51501445b1".to_string()),
        value: "0x0".to_string(),
        block_number: Some("0x2c32daa".to_string()),
        block_hash: Some("0xb1aa002fc5fc438301e27470e81ad06c69e601565d730b8c8a66d5ced9090c8f".to_string()),
        input: "0xcccbb34c".to_string(),
    }
}

fn sample_rescan_receipt() -> EvmTransactionReceipt {
    EvmTransactionReceipt {
        transaction_hash: "0x7e88e5d67ead0c0605f3bed96071ec4be14112bed2d929ee57e5b161bf6f2389".to_string(),
        status: Some("0x1".to_string()),
        block_number: Some("0x2c32daa".to_string()),
        block_hash: Some("0xb1aa002fc5fc438301e27470e81ad06c69e601565d730b8c8a66d5ced9090c8f".to_string()),
        from: "0xa9236f4950001355455a5b016a25fa27b947c9ac".to_string(),
        to: Some("0x887749abb233682aa7d5594a54659c51501445b1".to_string()),
        logs: vec![EvmLog {
            address: "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".to_string(),
            topics: vec![
                TRANSFER_TOPIC0.to_string(),
                address_to_topic("0x70196e53fa11b4621290144ccc8f4624ddff1058").unwrap(),
                address_to_topic("0x65722a6603b00f31922bc39737cc7ee24cd3d862").unwrap(),
            ],
            data: "0x00000000000000000000000000000000000000000000000000000000000186a0".to_string(),
            transaction_hash: Some("0x7e88e5d67ead0c0605f3bed96071ec4be14112bed2d929ee57e5b161bf6f2389".to_string()),
            log_index: Some("0x3a9".to_string()),
            block_number: Some("0x2c32daa".to_string()),
            block_hash: Some("0xb1aa002fc5fc438301e27470e81ad06c69e601565d730b8c8a66d5ced9090c8f".to_string()),
        }],
    }
}

fn scan_context_with_address(address: &str) -> ScanAddressContext {
    ScanAddressContext {
        id: Uuid::from_u128(101),
        tenant_id: Uuid::from_u128(102),
        chain_id: Uuid::from_u128(103),
        address: address.to_string(),
        scan_interval_seconds: 300,
        chain_type: "evm".to_string(),
    }
}

fn erc20_asset(chain_id: Uuid) -> Asset {
    Asset {
        id: Uuid::from_u128(204),
        chain_id,
        asset_type: "erc20".to_string(),
        symbol: "USDC".to_string(),
        name: "USD Coin".to_string(),
        contract_address: Some("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".to_string()),
        decimals: 6,
        is_builtin: true,
        status: "active".to_string(),
    }
}
```

- [ ] **Step 2: Run classification tests and verify RED**

Run:

```bash
cargo test -p coin-listener-chain-providers rescan_ --manifest-path backend/Cargo.toml
```

Expected: FAIL because rescan classification helpers do not exist.

- [ ] **Step 3: Add decoded rescan transfer struct**

In `backend/crates/chain-providers/src/evm.rs`, add after `DecodedErc20Transfer`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedRescanTokenTransfer {
    pub asset: Asset,
    pub transfer: DecodedErc20Transfer,
}
```

- [ ] **Step 4: Add receipt transfer decoder**

In `backend/crates/chain-providers/src/evm.rs`, add after `decode_erc20_transfer_log`:

```rust
pub fn decode_rescan_token_transfers(
    receipt: &EvmTransactionReceipt,
    assets: &[Asset],
) -> AppResult<Vec<DecodedRescanTokenTransfer>> {
    let mut transfers = Vec::new();
    for log in &receipt.logs {
        if log.topics.first().map(|topic| topic.to_lowercase()) != Some(TRANSFER_TOPIC0.to_string()) {
            continue;
        }
        let log_contract = normalize_hex(&log.address, 40, "address")?.to_ascii_lowercase();
        let Some(asset) = assets.iter().find(|asset| {
            asset.asset_type == "erc20"
                && asset
                    .contract_address
                    .as_deref()
                    .and_then(|contract| normalize_hex(contract, 40, "asset contract").ok())
                    .map(|contract| contract.eq_ignore_ascii_case(&log_contract))
                    .unwrap_or(false)
        }) else {
            continue;
        };
        let transfer = decode_erc20_transfer_log(log, asset.decimals)?;
        transfers.push(DecodedRescanTokenTransfer {
            asset: asset.clone(),
            transfer,
        });
    }
    Ok(transfers)
}
```

- [ ] **Step 5: Add source-aware transfer event helper**

Change the existing `transfer_event_draft` implementation to delegate to a new helper. Replace the function body with:

```rust
pub fn transfer_event_draft(
    context: &ScanAddressContext,
    asset: &Asset,
    transfer: DecodedErc20Transfer,
) -> AddressEventDraft {
    transfer_event_draft_with_metadata(
        context,
        asset,
        transfer,
        json!({ "source": "evm_erc20_transfer_log" }),
    )
}
```

Then add below it:

```rust
pub fn transfer_event_draft_with_source(
    context: &ScanAddressContext,
    asset: &Asset,
    transfer: DecodedErc20Transfer,
    source: &str,
    tx: &EvmTransaction,
) -> AppResult<AddressEventDraft> {
    let native_value_raw = parse_hex_u256_to_decimal_string(&tx.value)?;
    Ok(transfer_event_draft_with_metadata(
        context,
        asset,
        transfer,
        json!({
            "source": source,
            "tx_from": tx.from,
            "tx_to": tx.to,
            "native_value_raw": native_value_raw,
        }),
    ))
}

fn transfer_event_draft_with_metadata(
    context: &ScanAddressContext,
    asset: &Asset,
    transfer: DecodedErc20Transfer,
    metadata: serde_json::Value,
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

    let metadata = match metadata {
        serde_json::Value::Object(mut object) => {
            object.insert("token_contract".to_string(), json!(transfer.token_contract));
            serde_json::Value::Object(object)
        }
        _ => json!({
            "source": "evm_erc20_transfer_log",
            "token_contract": transfer.token_contract,
        }),
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
        metadata,
    }
}
```

- [ ] **Step 6: Add fee-only draft helper**

In `backend/crates/chain-providers/src/evm.rs`, add after `evm_balance_change_event`:

```rust
pub fn evm_fee_only_event_draft(
    context: &ScanAddressContext,
    asset: &Asset,
    tx: &EvmTransaction,
    source: &str,
) -> AppResult<AddressEventDraft> {
    let native_value_raw = parse_hex_u256_to_decimal_string(&tx.value)?;
    let block_number = tx
        .block_number
        .as_deref()
        .map(parse_hex_quantity_to_i64)
        .transpose()?;

    Ok(AddressEventDraft {
        tenant_id: context.tenant_id,
        chain_id: context.chain_id,
        address_id: context.id,
        asset_id: asset.id,
        event_type: "fee_only_change".to_string(),
        direction: "out".to_string(),
        is_transfer: false,
        tx_hash: Some(tx.hash.clone()),
        log_index: None,
        block_number,
        block_hash: tx.block_hash.clone(),
        confirmations: 0,
        from_address: Some(tx.from.clone()),
        to_address: tx.to.clone(),
        amount_raw: None,
        amount_decimal: None,
        balance_before_raw: None,
        balance_after_raw: None,
        balance_delta_raw: None,
        metadata: json!({
            "source": source,
            "tx_from": tx.from,
            "tx_to": tx.to,
            "native_value_raw": native_value_raw,
        }),
    })
}
```

- [ ] **Step 7: Run classification tests and verify GREEN**

Run:

```bash
cargo test -p coin-listener-chain-providers rescan_ --manifest-path backend/Cargo.toml
```

Expected: PASS for the rescan classification tests.

---

### Task 4: Add storage helpers for tenant-scoped rescan lookup

**Files:**
- Modify: `backend/crates/storage/src/repositories.rs`

- [ ] **Step 1: Write failing storage query constant tests**

In `backend/crates/storage/src/repositories.rs`, add these tests inside the existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn rescan_watched_addresses_query_is_tenant_and_chain_scoped() {
    assert!(RESCAN_WATCHED_ADDRESSES_FOR_CHAIN_QUERY.contains("tenant_id = $1"));
    assert!(RESCAN_WATCHED_ADDRESSES_FOR_CHAIN_QUERY.contains("chain_id = $2"));
    assert!(RESCAN_WATCHED_ADDRESSES_FOR_CHAIN_QUERY.contains("status = 'active'"));
    assert!(RESCAN_WATCHED_ADDRESSES_FOR_CHAIN_QUERY.contains("ORDER BY address"));
}

#[test]
fn assets_for_chain_query_returns_native_and_erc20_assets() {
    assert!(ASSETS_FOR_CHAIN_QUERY.contains("WHERE chain_id = $1"));
    assert!(ASSETS_FOR_CHAIN_QUERY.contains("status = 'active'"));
    assert!(ASSETS_FOR_CHAIN_QUERY.contains("ORDER BY asset_type, symbol, name"));
}
```

- [ ] **Step 2: Run storage tests and verify RED**

Run:

```bash
cargo test -p coin-listener-storage rescan_watched_addresses_query_is_tenant_and_chain_scoped assets_for_chain_query_returns_native_and_erc20_assets --manifest-path backend/Cargo.toml
```

Expected: FAIL because the query constants do not exist.

- [ ] **Step 3: Add query constants**

In `backend/crates/storage/src/repositories.rs`, after `LIST_WATCHED_ADDRESSES_QUERY`, add:

```rust
pub const RESCAN_WATCHED_ADDRESSES_FOR_CHAIN_QUERY: &str = r#"
SELECT id, tenant_id, chain_id, address, label, priority, scan_interval_seconds,
       transfer_filter_enabled, balance_change_filter_enabled, status
FROM watched_addresses
WHERE tenant_id = $1
  AND chain_id = $2
  AND status = 'active'
ORDER BY address
"#;

pub const ASSETS_FOR_CHAIN_QUERY: &str = r#"
SELECT id, chain_id, asset_type, symbol, name, contract_address, decimals, is_builtin, status
FROM assets
WHERE chain_id = $1
  AND status = 'active'
ORDER BY asset_type, symbol, name
"#;
```

- [ ] **Step 4: Add repository functions**

In `backend/crates/storage/src/repositories.rs`, after `list_watched_addresses`, add:

```rust
pub async fn rescan_watched_addresses_for_chain(
    pool: &PgPool,
    tenant_id: Uuid,
    chain_id: Uuid,
) -> AppResult<Vec<WatchedAddress>> {
    sqlx::query_as::<_, WatchedAddress>(RESCAN_WATCHED_ADDRESSES_FOR_CHAIN_QUERY)
        .bind(tenant_id)
        .bind(chain_id)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn assets_for_chain(pool: &PgPool, chain_id: Uuid) -> AppResult<Vec<Asset>> {
    sqlx::query_as::<_, Asset>(ASSETS_FOR_CHAIN_QUERY)
        .bind(chain_id)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}
```

- [ ] **Step 5: Run storage tests and verify GREEN**

Run:

```bash
cargo test -p coin-listener-storage rescan_watched_addresses_query_is_tenant_and_chain_scoped assets_for_chain_query_returns_native_and_erc20_assets --manifest-path backend/Cargo.toml
```

Expected: PASS.

---

### Task 5: Add protected backend rescan route

**Files:**
- Modify: `backend/crates/api-server/src/routes.rs`

- [ ] **Step 1: Write failing router exposure test**

In `backend/crates/api-server/src/routes.rs`, add this test inside `#[cfg(test)] mod tests`:

```rust
#[tokio::test]
async fn router_exposes_evm_transaction_rescan_route() {
    let app = build_router(test_state());

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/evm/transactions/rescan")
                .body(Body::empty())
                .expect("valid request"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
```

- [ ] **Step 2: Run router test and verify RED**

Run:

```bash
cargo test -p api-server router_exposes_evm_transaction_rescan_route --manifest-path backend/Cargo.toml
```

Expected: FAIL with 404 or other non-401 because the route does not exist.

- [ ] **Step 3: Add imports**

In `backend/crates/api-server/src/routes.rs`, update the imports:

```rust
use coin_listener_chain_providers::{
    btc::BtcClient,
    evm::{self, EvmRpcClient},
    tron::TronClient,
};
```

Add `EvmTransactionRescanRequest`, `EvmTransactionRescanResponse`, `EvmTransactionRescanSummary`, and `EvmTransactionRescanTransferSummary` to the `coin_listener_core::models` import list:

```rust
EvmTransactionRescanRequest, EvmTransactionRescanResponse, EvmTransactionRescanSummary,
EvmTransactionRescanTransferSummary,
```

- [ ] **Step 4: Add route registration**

In `build_router`, after `.route("/api/events", get(list_events))`, add:

```rust
.route("/api/evm/transactions/rescan", post(rescan_evm_transaction))
```

- [ ] **Step 5: Add rescan handler and helpers**

In `backend/crates/api-server/src/routes.rs`, add after `list_events`:

```rust
async fn rescan_evm_transaction(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<EvmTransactionRescanRequest>,
) -> Result<Response, ApiError> {
    validate_evm_tx_hash(&request.tx_hash)?;
    let chain = repositories::chain_by_id(&state.postgres, request.chain_id).await?;
    if chain.chain_type != "evm" {
        return Err(AppError::Validation("rescan only supports evm chains".to_string()).into());
    }

    let provider = repositories::active_rpc_provider_for_chain(&state.postgres, request.chain_id).await?;
    let timeout = coin_listener_worker::provider_timeout_duration(&provider)?;
    let rpc = EvmRpcClient::new(provider.base_url.clone(), timeout);
    let tx = rpc.eth_get_transaction_by_hash(&request.tx_hash).await?;
    let receipt = rpc.eth_get_transaction_receipt(&request.tx_hash).await?;
    let block_number = tx
        .block_number
        .as_deref()
        .map(evm::parse_hex_quantity_to_i64)
        .transpose()?
        .ok_or_else(|| AppError::Validation("transaction is not mined".to_string()))?;
    let native_value_raw = evm::parse_hex_u256_to_decimal_string(&tx.value)?;

    let watched_addresses = repositories::rescan_watched_addresses_for_chain(
        &state.postgres,
        auth.tenant_id,
        request.chain_id,
    )
    .await?;
    let assets = repositories::assets_for_chain(&state.postgres, request.chain_id).await?;
    let native_asset = assets
        .iter()
        .find(|asset| asset.asset_type == "native")
        .cloned()
        .ok_or_else(|| AppError::NotFound("native asset".to_string()))?;
    let token_transfers = evm::decode_rescan_token_transfers(&receipt, &assets)?;

    let mut inserted_events = Vec::new();
    let mut skipped_event_count = 0usize;
    for address in &watched_addresses {
        let context = ScanAddressContext {
            id: address.id,
            tenant_id: address.tenant_id,
            chain_id: address.chain_id,
            address: address.address.clone(),
            scan_interval_seconds: address.scan_interval_seconds,
            chain_type: "evm".to_string(),
        };
        let watched = address.address.to_lowercase();
        let mut matched_token_transfer = false;
        for decoded in &token_transfers {
            let from = decoded.transfer.from_address.to_lowercase();
            let to = decoded.transfer.to_address.to_lowercase();
            if from != watched && to != watched {
                continue;
            }
            matched_token_transfer = true;
            let draft = evm::transfer_event_draft_with_source(
                &context,
                &decoded.asset,
                decoded.transfer.clone(),
                "evm_tx_rescan",
                &tx,
            )?;
            match repositories::insert_event_and_outbox_if_not_exists(&state.postgres, draft).await? {
                Some(event) => inserted_events.push(event),
                None => skipped_event_count += 1,
            }
        }

        let is_fee_payer = tx.from.eq_ignore_ascii_case(&address.address);
        let is_zero_value = native_value_raw == "0";
        if is_fee_payer && is_zero_value && !matched_token_transfer {
            let draft = evm::evm_fee_only_event_draft(&context, &native_asset, &tx, "evm_tx_rescan")?;
            match repositories::insert_event_and_outbox_if_not_exists(&state.postgres, draft).await? {
                Some(event) => inserted_events.push(event),
                None => skipped_event_count += 1,
            }
        }
    }

    let transfer_summaries = token_transfers
        .iter()
        .map(|decoded| EvmTransactionRescanTransferSummary {
            asset_id: decoded.asset.id,
            symbol: decoded.asset.symbol.clone(),
            token_contract: decoded
                .transfer
                .token_contract
                .clone()
                .unwrap_or_else(|| decoded.asset.contract_address.clone().unwrap_or_default()),
            from_address: decoded.transfer.from_address.clone(),
            to_address: decoded.transfer.to_address.clone(),
            amount_raw: decoded.transfer.amount_raw.clone(),
            amount_decimal: decoded.transfer.amount_decimal.clone(),
            log_index: decoded.transfer.log_index,
        })
        .collect::<Vec<_>>();

    let response = EvmTransactionRescanResponse {
        summary: EvmTransactionRescanSummary {
            chain_id: request.chain_id,
            tx_hash: tx.hash,
            tx_from: tx.from,
            tx_to: tx.to,
            native_value_raw,
            block_number,
            token_transfer_count: transfer_summaries.len(),
            inserted_event_count: inserted_events.len(),
            skipped_event_count,
        },
        token_transfers: transfer_summaries,
        events: inserted_events,
    };

    Ok(Json(response).into_response())
}

fn validate_evm_tx_hash(tx_hash: &str) -> AppResult<()> {
    let digits = tx_hash
        .strip_prefix("0x")
        .ok_or_else(|| AppError::Validation("tx_hash must start with 0x".to_string()))?;
    if digits.len() != 64 || !digits.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(AppError::Validation("tx_hash must be 32-byte hex".to_string()));
    }
    Ok(())
}
```

- [ ] **Step 6: Add missing worker dependency to API server**

Because the handler uses `provider_timeout_duration`, add this dependency to `backend/crates/api-server/Cargo.toml` under `[dependencies]`:

```toml
coin-listener-worker = { path = "../worker" }
```

- [ ] **Step 7: Run router test and verify GREEN**

Run:

```bash
cargo test -p api-server router_exposes_evm_transaction_rescan_route --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 8: Run API server check**

Run:

```bash
cargo check -p api-server --manifest-path backend/Cargo.toml
```

Expected: exit 0.

---

### Task 6: Add frontend API wrapper

**Files:**
- Modify: `frontend/src/api/client.ts`
- Modify: `frontend/src/api/types.ts`

- [ ] **Step 1: Update client type imports**

In `frontend/src/api/client.ts`, add these names to the existing `import type { ... } from './types';` block:

```ts
EvmTransactionRescanRequest,
EvmTransactionRescanResponse,
```

- [ ] **Step 2: Add rescan API function**

In `frontend/src/api/client.ts`, after `listEvents`, add:

```ts
export function rescanEvmTransaction(payload: EvmTransactionRescanRequest): Promise<EvmTransactionRescanResponse> {
  return request<EvmTransactionRescanResponse>('/api/evm/transactions/rescan', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}
```

- [ ] **Step 3: Run frontend typecheck/build**

Run:

```bash
npm --prefix frontend run build
```

Expected: TypeScript compiles and Vite build exits 0.

---

### Task 7: Add Event Center rescan form and clearer event labels

**Files:**
- Modify: `frontend/src/pages/EventsPage.tsx`

- [ ] **Step 1: Update imports**

In `frontend/src/pages/EventsPage.tsx`, change the API client import to include `rescanEvmTransaction`:

```ts
import { ApiRequestError, listAssets, listChains, listEvents, listWatchedAddresses, rescanEvmTransaction, scanAddress } from '../api/client';
```

Update the type import to include `EvmTransactionRescanResponse`:

```ts
import type { AddressEvent, EventQuery, EvmTransactionRescanResponse } from '../api/types';
```

- [ ] **Step 2: Add rescan form state and EVM chains**

Inside `EventsPage`, after the existing `const [scanAddressId, setScanAddressId] = useState<string>();`, add:

```ts
const [rescanChainId, setRescanChainId] = useState<string>();
const [rescanTxHash, setRescanTxHash] = useState('');
const [rescanResult, setRescanResult] = useState<EvmTransactionRescanResponse>();
```

After `evmChainIds`, add:

```ts
const evmChains = useMemo(
  () => (chainsQuery.data ?? []).filter(chain => chain.chain_type === 'evm'),
  [chainsQuery.data],
);
```

- [ ] **Step 3: Add rescan mutation**

After `scanMutation`, add:

```ts
const rescanMutation = useMutation({
  mutationFn: rescanEvmTransaction,
  onSuccess: result => {
    setRescanResult(result);
    Toast.success(`交易回填完成：新增 ${result.summary.inserted_event_count} 条，跳过 ${result.summary.skipped_event_count} 条`);
    queryClient.invalidateQueries({ queryKey: ['events'] });
  },
  onError: error => {
    Toast.error(error instanceof Error ? error.message : '交易回填失败');
  },
});

const normalizedRescanTxHash = rescanTxHash.trim();
const rescanDisabledReason = !rescanChainId
  ? '请选择 EVM 链'
  : !/^0x[0-9a-fA-F]{64}$/.test(normalizedRescanTxHash)
    ? '请输入 0x 开头的 32 字节交易哈希'
    : undefined;
```

- [ ] **Step 4: Add event type label helper**

Before `return`, add:

```ts
function renderEventType(event: AddressEvent) {
  if (event.event_type === 'fee_only_change') {
    return <Tag color="orange">Gas 消耗</Tag>;
  }
  if (event.event_type === 'balance_change') {
    return <Tag color="blue">余额变化</Tag>;
  }
  if (event.event_type === 'transfer') {
    return <Tag color="green">转账</Tag>;
  }
  return <Tag>{event.event_type}</Tag>;
}

function renderEventSource(event: AddressEvent) {
  const source = typeof event.metadata?.source === 'string' ? event.metadata.source : undefined;
  if (source === 'evm_tx_rescan') return <Tag color="purple">交易回填</Tag>;
  if (source === 'evm_erc20_transfer_log') return <Tag color="green">EVM 日志</Tag>;
  if (source === 'evm_balance_snapshot') return <Tag color="blue">余额快照</Tag>;
  if (source === 'mock_evm_transfer') return <Tag color="grey">模拟</Tag>;
  return <Tag color="grey">-</Tag>;
}
```

- [ ] **Step 5: Add rescan panel JSX**

Insert this panel between the existing “开发模拟扫描” `FilterPanel` and the `DataSurface`:

```tsx
<FilterPanel title="EVM 交易回填">
  <Space vertical align="start">
    <Space>
      <Select
        value={rescanChainId}
        onChange={value => setRescanChainId(value as string | undefined)}
        showClear
        filter
        placeholder="选择 EVM 链"
        style={{ width: 220 }}
        disabled={evmChains.length === 0}
      >
        {evmChains.map(chain => (
          <Select.Option key={chain.id} value={chain.id}>{chain.name}</Select.Option>
        ))}
      </Select>
      <Form.Input
        field="rescan_tx_hash"
        noLabel
        value={rescanTxHash}
        onChange={value => setRescanTxHash(String(value))}
        placeholder="0x...交易哈希"
        style={{ width: 520 }}
      />
      <Button
        type="primary"
        loading={rescanMutation.isPending}
        disabled={Boolean(rescanDisabledReason)}
        onClick={() => rescanChainId && rescanMutation.mutate({ chain_id: rescanChainId, tx_hash: normalizedRescanTxHash })}
      >
        重扫交易
      </Button>
    </Space>
    <Text type={rescanDisabledReason ? 'warning' : 'tertiary'}>
      {rescanDisabledReason ?? '按 tx hash 拉取交易和 receipt，解析 Transfer logs 并为命中的监听地址回填事件。'}
    </Text>
    {rescanResult ? (
      <Banner
        type="info"
        title={`回填结果：新增 ${rescanResult.summary.inserted_event_count} 条，跳过 ${rescanResult.summary.skipped_event_count} 条`}
        description={`Token transfers: ${rescanResult.summary.token_transfer_count}; native value raw: ${rescanResult.summary.native_value_raw}; block: ${rescanResult.summary.block_number}`}
      />
    ) : null}
  </Space>
</FilterPanel>
```

- [ ] **Step 6: Update event table columns**

In the `columns` array, replace the existing type column:

```tsx
{ title: '类型', dataIndex: 'event_type', width: 150, render: value => <Tag>{String(value)}</Tag> },
```

with:

```tsx
{ title: '类型', width: 150, render: (_, event) => renderEventType(event) },
{ title: '来源', width: 110, render: (_, event) => renderEventSource(event) },
```

- [ ] **Step 7: Run frontend build**

Run:

```bash
npm --prefix frontend run build
```

Expected: TypeScript compiles and Vite build exits 0.

---

### Task 8: Add focused end-to-end verification script for the sample transaction

**Files:**
- Modify: none unless previous tasks need fixes.

- [ ] **Step 1: Run backend unit tests**

Run:

```bash
cargo test -p coin-listener-chain-providers rescan_ --manifest-path backend/Cargo.toml && cargo test -p coin-listener-core evm_transaction_rescan_request_and_response_round_trip_json --manifest-path backend/Cargo.toml && cargo test -p api-server router_exposes_evm_transaction_rescan_route --manifest-path backend/Cargo.toml
```

Expected: all selected tests pass.

- [ ] **Step 2: Run backend checks**

Run:

```bash
cargo check -p coin-listener-chain-providers --manifest-path backend/Cargo.toml && cargo check -p coin-listener-storage --manifest-path backend/Cargo.toml && cargo check -p api-server --manifest-path backend/Cargo.toml
```

Expected: all checks exit 0.

- [ ] **Step 3: Run frontend build**

Run:

```bash
npm --prefix frontend run build
```

Expected: TypeScript and Vite build exit 0.

- [ ] **Step 4: Manual verification with a running stack**

If the local stack is running and an auth token is available, call the protected API with the sample transaction:

```bash
curl -sS -X POST "$VITE_API_BASE_URL/api/evm/transactions/rescan" \
  -H "Authorization: Bearer $COIN_LISTENER_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"chain_id":"10000000-0000-0000-0000-000000000004","tx_hash":"0x7e88e5d67ead0c0605f3bed96071ec4be14112bed2d929ee57e5b161bf6f2389"}'
```

Expected JSON shape:

```json
{
  "summary": {
    "chain_id": "10000000-0000-0000-0000-000000000004",
    "tx_hash": "0x7e88e5d67ead0c0605f3bed96071ec4be14112bed2d929ee57e5b161bf6f2389",
    "native_value_raw": "0",
    "token_transfer_count": 1
  },
  "token_transfers": [
    {
      "symbol": "USDC",
      "token_contract": "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
      "amount_raw": "100000",
      "amount_decimal": "0.1"
    }
  ]
}
```

If the stack or token is not available, skip this step and report that only unit/build verification was run.

---

## Self-Review Notes

Spec coverage:

| Requirement | Covered by |
|---|---|
| Fetch transaction and receipt by tx hash | Task 2, Task 5 |
| Decode receipt ERC20 Transfer logs | Task 3 |
| Mark token transfer events from receipt logs | Task 3, Task 5 |
| Mark fee-only gas payer events separately | Task 3, Task 5, Task 7 |
| Add manual tx-hash rescan/backfill API | Task 5 |
| Keep normal scan cursors untouched | Task 5 uses point lookup and no cursor functions |
| Add frontend rescan entry | Task 6, Task 7 |
| Verify with sample transaction shape | Task 8 |

Placeholder scan: no TBD/TODO placeholders remain. All code steps include concrete snippets and commands.

Type consistency: Rust DTO names match TypeScript DTO names; route path is consistently `/api/evm/transactions/rescan`; metadata source is consistently `evm_tx_rescan`.
