# Coin Listener BTC / TRON Listeners Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Milestone 4 BTC and TRON listener support so `utxo` and `tron` watched addresses produce normalized balance snapshots, transfer events, cursors, and notifications.

**Architecture:** Add focused `tron` and `btc` modules under `coin-listener-chain-providers` for HTTP request construction, response parsing, and chain-specific transfer decoding. Reuse existing storage primitives (`scan_cursors`, `balance_snapshots`, `address_events`, `insert_event_if_not_exists`) and extend the worker dispatcher with TRON and BTC scan helpers that return newly inserted events for the existing notification enqueue path.

**Tech Stack:** Rust 2021, Tokio, reqwest, serde/serde_json, SQLx, PostgreSQL, Redis queues, chrono, uuid, num-bigint.

---

## External API References

- TRON TRC20 account transfers: `GET https://api.trongrid.io/v1/accounts/{address}/transactions/trc20` ([TRON Developer Hub](https://developers.tron.network/docs/get-trc20-transaction-history)).
- TRON account transactions: `GET https://api.trongrid.io/v1/accounts/{address}/transactions` ([TRON API Reference](https://developers.tron.network/reference/get-transaction-info-by-account-address)).
- BTC Esplora address endpoints: `GET /address/:address`, `GET /address/:address/txs/chain[/:last_seen_txid]`, `GET /address/:address/utxo` ([Blockstream Esplora API](https://github.com/Blockstream/esplora/blob/master/API.md)).

---

## File Structure

Create:

```text
backend/crates/chain-providers/src/tron.rs
backend/crates/chain-providers/src/btc.rs
```

Modify:

```text
backend/crates/chain-providers/src/lib.rs
backend/crates/storage/src/repositories.rs
backend/crates/worker/src/lib.rs
```

No database migration is planned. Existing `scan_cursors` and `address_events` indexes are sufficient for one event per watched address / asset / transaction.

The current project directory has previously returned `fatal: not a git repository`. For checkpoint steps, run `git status --short`; if it fails, skip commit and report changed files.

---

### Task 1: Add generic asset lookup by type

**Files:**
- Modify: `backend/crates/storage/src/repositories.rs`

- [ ] **Step 1: Write failing repository query test**

In `backend/crates/storage/src/repositories.rs`, update the test import block inside `#[cfg(test)] mod tests` to include `ACTIVE_ASSETS_BY_TYPE_QUERY`:

```rust
use super::{
    next_scan_at_from, ACTIVE_ASSETS_BY_TYPE_QUERY, ACTIVE_ERC20_ASSETS_QUERY,
    ACTIVE_RPC_PROVIDER_QUERY, CLAIM_ONE_DUE_SCAN_ADDRESS_QUERY,
    INSERT_BALANCE_SNAPSHOT_QUERY, INSERT_EVENT_IF_NOT_EXISTS_QUERY,
    LATEST_BALANCE_SNAPSHOT_QUERY, MARK_CLAIMED_SCAN_ENQUEUED_QUERY,
    SCAN_CURSOR_QUERY, UPSERT_SCAN_CURSOR_QUERY,
};
```

Add this test after `active_erc20_assets_query_filters_active_contract_assets`:

```rust
#[test]
fn active_assets_by_type_query_filters_chain_type_and_status() {
    assert!(ACTIVE_ASSETS_BY_TYPE_QUERY.contains("chain_id = $1"));
    assert!(ACTIVE_ASSETS_BY_TYPE_QUERY.contains("asset_type = $2"));
    assert!(ACTIVE_ASSETS_BY_TYPE_QUERY.contains("status = 'active'"));
    assert!(ACTIVE_ASSETS_BY_TYPE_QUERY.contains("ORDER BY symbol, name"));
}
```

- [ ] **Step 2: Run test to verify RED**

Run:

```bash
cargo test -p coin-listener-storage active_assets_by_type_query --manifest-path backend/Cargo.toml
```

Expected: FAIL because `ACTIVE_ASSETS_BY_TYPE_QUERY` does not exist.

- [ ] **Step 3: Add query constant and repository helper**

In `backend/crates/storage/src/repositories.rs`, add this query constant after `ACTIVE_ERC20_ASSETS_QUERY`:

```rust
pub const ACTIVE_ASSETS_BY_TYPE_QUERY: &str = r#"
SELECT id, chain_id, asset_type, symbol, name, contract_address, decimals, is_builtin, status
FROM assets
WHERE chain_id = $1
  AND asset_type = $2
  AND status = 'active'
ORDER BY symbol, name
"#;
```

Add this function after `active_erc20_assets_for_chain`:

```rust
pub async fn active_assets_for_chain_by_type(
    pool: &PgPool,
    chain_id: Uuid,
    asset_type: &str,
) -> AppResult<Vec<Asset>> {
    sqlx::query_as::<_, Asset>(ACTIVE_ASSETS_BY_TYPE_QUERY)
        .bind(chain_id)
        .bind(asset_type)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}
```

- [ ] **Step 4: Run storage tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-storage active_assets_by_type_query --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage active_erc20_assets_query --manifest-path backend/Cargo.toml
cargo check -p coin-listener-storage --manifest-path backend/Cargo.toml
cargo fmt -p coin-listener-storage --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 5: Checkpoint**

Run:

```bash
git status --short
```

If git status succeeds, run:

```bash
git add backend/crates/storage/src/repositories.rs
git commit -m "Add active asset lookup by type"
```

Expected outside a git repository: skip commit and report changed files.

---

### Task 2: Add TRON provider decode helpers

**Files:**
- Create: `backend/crates/chain-providers/src/tron.rs`
- Modify: `backend/crates/chain-providers/src/lib.rs`

- [ ] **Step 1: Expose TRON module and write failing decode tests**

In `backend/crates/chain-providers/src/lib.rs`, add:

```rust
pub mod tron;
```

Create `backend/crates/chain-providers/src/tron.rs` with these tests first:

```rust
use coin_listener_core::{AppError, AppResult};
use num_bigint::BigUint;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{str::FromStr, time::Duration};

#[cfg(test)]
mod tests {
    use super::{
        decode_trc20_transfer, decode_tron_balance, decode_trx_transfer,
        normalize_tron_address, tron_transfer_direction, DecodedTronTransfer,
    };
    use coin_listener_core::AppError;
    use serde_json::json;

    #[test]
    fn tron_address_normalization_accepts_base58_shape() {
        assert_eq!(
            normalize_tron_address("  TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh  ").unwrap(),
            "TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh"
        );
        assert!(matches!(
            normalize_tron_address("0x1111111111111111111111111111111111111111"),
            Err(AppError::Validation(message)) if message.contains("TRON address")
        ));
    }

    #[test]
    fn trc20_payload_decodes_transfer_fields() {
        let payload = json!({
            "transaction_id": "f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1",
            "block_number": 65000000,
            "block_timestamp": 1760000000000_i64,
            "from": "TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh",
            "to": "TQ6p7JAFM2Z2V5Q3U6QwY7Xx9z5xZQkP8E",
            "value": "2500000",
            "token_info": { "address": "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t" }
        });

        let transfer = decode_trc20_transfer(
            &payload,
            "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t",
            6,
        )
        .unwrap();

        assert_eq!(transfer.tx_hash, "f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1");
        assert_eq!(transfer.cursor_value, 65_000_000);
        assert_eq!(transfer.block_number, Some(65_000_000));
        assert_eq!(transfer.from_address, "TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh");
        assert_eq!(transfer.to_address, "TQ6p7JAFM2Z2V5Q3U6QwY7Xx9z5xZQkP8E");
        assert_eq!(transfer.amount_raw, "2500000");
        assert_eq!(transfer.amount_decimal, "2.5");
        assert_eq!(transfer.token_contract, Some("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t".to_string()));
    }

    #[test]
    fn trc20_payload_rejects_wrong_contract() {
        let payload = json!({
            "transaction_id": "f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1",
            "block_number": 65000000,
            "from": "TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh",
            "to": "TQ6p7JAFM2Z2V5Q3U6QwY7Xx9z5xZQkP8E",
            "value": "2500000",
            "token_info": { "address": "TWrongqjeKQxGTCi8q8ZY4pL8otSzgjLj6t" }
        });

        let result = decode_trc20_transfer(
            &payload,
            "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t",
            6,
        );

        assert!(matches!(result, Err(AppError::Validation(message)) if message.contains("contract")));
    }

    #[test]
    fn trx_payload_decodes_transfer_contract() {
        let payload = json!({
            "txID": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "blockNumber": 65000001,
            "raw_data": {
                "contract": [{
                    "type": "TransferContract",
                    "parameter": { "value": {
                        "owner_address": "TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh",
                        "to_address": "TQ6p7JAFM2Z2V5Q3U6QwY7Xx9z5xZQkP8E",
                        "amount": 1000000
                    }}
                }]
            }
        });

        let transfer = decode_trx_transfer(&payload, 6).unwrap();

        assert_eq!(transfer.tx_hash, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(transfer.cursor_value, 65_000_001);
        assert_eq!(transfer.amount_raw, "1000000");
        assert_eq!(transfer.amount_decimal, "1.0");
        assert_eq!(transfer.token_contract, None);
    }

    #[test]
    fn tron_balance_decodes_raw_and_decimal() {
        let balance = decode_tron_balance(1234567, 6).unwrap();

        assert_eq!(balance.balance_raw, "1234567");
        assert_eq!(balance.balance_decimal, "1.234567");
    }

    #[test]
    fn tron_transfer_direction_uses_watched_address() {
        let watched = "TQ6p7JAFM2Z2V5Q3U6QwY7Xx9z5xZQkP8E";
        assert_eq!(
            tron_transfer_direction("TA111111111111111111111111111111111", watched, watched),
            "in"
        );
        assert_eq!(
            tron_transfer_direction(watched, "TA111111111111111111111111111111111", watched),
            "out"
        );
        assert_eq!(tron_transfer_direction(watched, watched, watched), "self");
    }
}
```

- [ ] **Step 2: Run TRON tests to verify RED**

Run:

```bash
cargo test -p coin-listener-chain-providers tron_ --manifest-path backend/Cargo.toml
```

Expected: FAIL because the TRON helper functions and structs do not exist.

- [ ] **Step 3: Implement TRON decode types and helpers**

In `backend/crates/chain-providers/src/tron.rs`, above the test module, add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TronBalance {
    pub balance_raw: String,
    pub balance_decimal: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedTronTransfer {
    pub tx_hash: String,
    pub cursor_value: i64,
    pub block_number: Option<i64>,
    pub from_address: String,
    pub to_address: String,
    pub amount_raw: String,
    pub amount_decimal: String,
    pub token_contract: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TronTokenInfo {
    address: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TronRawContract {
    #[serde(rename = "type")]
    contract_type: String,
    parameter: TronRawParameter,
}

#[derive(Debug, Deserialize)]
struct TronRawParameter {
    value: TronTransferValue,
}

#[derive(Debug, Deserialize)]
struct TronTransferValue {
    owner_address: String,
    to_address: String,
    amount: i64,
}

#[derive(Debug, Deserialize)]
struct TronRawData {
    contract: Vec<TronRawContract>,
}

#[derive(Debug, Deserialize)]
struct TronTransactionPayload {
    #[serde(rename = "txID")]
    tx_id: String,
    #[serde(rename = "blockNumber")]
    block_number: Option<i64>,
    raw_data: TronRawData,
}

#[derive(Debug, Deserialize)]
struct Trc20TransferPayload {
    transaction_id: String,
    block_number: Option<i64>,
    block_timestamp: Option<i64>,
    from: String,
    to: String,
    value: String,
    token_info: Option<TronTokenInfo>,
}

pub fn normalize_tron_address(address: &str) -> AppResult<String> {
    let address = address.trim();
    let valid = address.starts_with('T')
        && (26..=36).contains(&address.len())
        && address.chars().all(|character| character.is_ascii_alphanumeric());
    if !valid {
        return Err(AppError::Validation(format!("invalid TRON address {address}")));
    }
    Ok(address.to_string())
}

pub fn tron_transfer_direction(from: &str, to: &str, watched: &str) -> &'static str {
    if from == watched && to == watched {
        "self"
    } else if to == watched {
        "in"
    } else if from == watched {
        "out"
    } else {
        "unknown"
    }
}

pub fn decode_tron_balance(balance_sun: i64, decimals: i32) -> AppResult<TronBalance> {
    if balance_sun < 0 {
        return Err(AppError::Validation("TRON balance cannot be negative".to_string()));
    }
    let balance_raw = balance_sun.to_string();
    let balance_decimal = decimal_string(&balance_raw, decimals)?;
    Ok(TronBalance {
        balance_raw,
        balance_decimal,
    })
}

pub fn decode_trc20_transfer(
    payload: &Value,
    expected_contract: &str,
    decimals: i32,
) -> AppResult<DecodedTronTransfer> {
    let payload: Trc20TransferPayload = serde_json::from_value(payload.clone()).map_err(|error| {
        AppError::Validation(format!("invalid TRC20 transfer payload: {error}"))
    })?;
    let token_contract = payload
        .token_info
        .and_then(|token| token.address)
        .ok_or_else(|| AppError::Validation("TRC20 transfer missing token_info.address".to_string()))?;
    let token_contract = normalize_tron_address(&token_contract)?;
    let expected_contract = normalize_tron_address(expected_contract)?;
    if token_contract != expected_contract {
        return Err(AppError::Validation(format!(
            "TRC20 transfer contract {token_contract} does not match expected {expected_contract}"
        )));
    }
    let cursor_value = payload
        .block_number
        .or(payload.block_timestamp)
        .ok_or_else(|| AppError::Validation("TRC20 transfer missing block watermark".to_string()))?;
    if cursor_value < 0 {
        return Err(AppError::Validation("TRC20 transfer watermark cannot be negative".to_string()));
    }
    let from_address = normalize_tron_address(&payload.from)?;
    let to_address = normalize_tron_address(&payload.to)?;
    validate_tx_hash(&payload.transaction_id, "transaction_id")?;
    let amount_raw = parse_decimal_amount(&payload.value, "TRC20 value")?;
    let amount_decimal = decimal_string(&amount_raw, decimals)?;

    Ok(DecodedTronTransfer {
        tx_hash: payload.transaction_id,
        cursor_value,
        block_number: payload.block_number,
        from_address,
        to_address,
        amount_raw,
        amount_decimal,
        token_contract: Some(token_contract),
    })
}

pub fn decode_trx_transfer(payload: &Value, decimals: i32) -> AppResult<DecodedTronTransfer> {
    let payload: TronTransactionPayload = serde_json::from_value(payload.clone()).map_err(|error| {
        AppError::Validation(format!("invalid TRX transfer payload: {error}"))
    })?;
    validate_tx_hash(&payload.tx_id, "txID")?;
    let block_number = payload
        .block_number
        .ok_or_else(|| AppError::Validation("TRX transfer missing blockNumber".to_string()))?;
    if block_number < 0 {
        return Err(AppError::Validation("TRX blockNumber cannot be negative".to_string()));
    }
    let contract = payload
        .raw_data
        .contract
        .into_iter()
        .find(|contract| contract.contract_type == "TransferContract")
        .ok_or_else(|| AppError::Validation("TRX transaction missing TransferContract".to_string()))?;
    if contract.parameter.value.amount < 0 {
        return Err(AppError::Validation("TRX amount cannot be negative".to_string()));
    }
    let from_address = normalize_tron_address(&contract.parameter.value.owner_address)?;
    let to_address = normalize_tron_address(&contract.parameter.value.to_address)?;
    let amount_raw = contract.parameter.value.amount.to_string();
    let amount_decimal = decimal_string(&amount_raw, decimals)?;

    Ok(DecodedTronTransfer {
        tx_hash: payload.tx_id,
        cursor_value: block_number,
        block_number: Some(block_number),
        from_address,
        to_address,
        amount_raw,
        amount_decimal,
        token_contract: None,
    })
}

fn validate_tx_hash(value: &str, field: &str) -> AppResult<()> {
    let valid = value.len() == 64 && value.chars().all(|character| character.is_ascii_hexdigit());
    if !valid {
        return Err(AppError::Validation(format!("invalid TRON {field} {value}")));
    }
    Ok(())
}

fn parse_decimal_amount(value: &str, field: &str) -> AppResult<String> {
    BigUint::from_str(value)
        .map(|amount| amount.to_string())
        .map_err(|error| AppError::Validation(format!("invalid {field} {value}: {error}")))
}

pub fn decimal_string(raw: &str, decimals: i32) -> AppResult<String> {
    if decimals < 0 {
        return Err(AppError::Validation("asset decimals cannot be negative".to_string()));
    }
    let _ = BigUint::from_str(raw)
        .map_err(|error| AppError::Validation(format!("invalid decimal amount {raw}: {error}")))?;
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
```

- [ ] **Step 4: Run TRON decode tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-chain-providers tron_address_normalization --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers trc20_payload --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers trx_payload --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers tron_balance --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers tron_transfer_direction --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 5: Checkpoint**

Run:

```bash
git status --short
```

If git status succeeds, run:

```bash
git add backend/crates/chain-providers/src/lib.rs backend/crates/chain-providers/src/tron.rs
git commit -m "Add TRON transfer decode helpers"
```

Expected outside a git repository: skip commit and report changed files.

---

### Task 3: Add TRON HTTP client and event draft builder

**Files:**
- Modify: `backend/crates/chain-providers/src/tron.rs`

- [ ] **Step 1: Write failing TRON client tests**

Append these tests inside `#[cfg(test)] mod tests` in `backend/crates/chain-providers/src/tron.rs`:

```rust
#[test]
fn tron_client_builds_account_paths_without_double_slashes() {
    let client = super::TronClient::new(
        "https://api.trongrid.io/".to_string(),
        std::time::Duration::from_secs(5),
    );

    assert_eq!(
        client.account_transactions_path("TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh").unwrap(),
        "/v1/accounts/TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh/transactions"
    );
    assert_eq!(
        client.account_trc20_path("TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh").unwrap(),
        "/v1/accounts/TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh/transactions/trc20"
    );
}

#[test]
fn tron_request_errors_do_not_include_provider_url() {
    let message = super::format_tron_request_error(
        "account transactions",
        "https://api.trongrid.io/private-key",
        "connection refused",
    );

    assert!(message.contains("account transactions"));
    assert!(message.contains("connection refused"));
    assert!(!message.contains("trongrid.io"));
    assert!(!message.contains("private-key"));
}

#[test]
fn tron_transfer_event_draft_maps_transfer_fields() {
    let context = scan_context("TQ6p7JAFM2Z2V5Q3U6QwY7Xx9z5xZQkP8E");
    let asset = asset("trc20", "USDT", Some("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t"));
    let transfer = DecodedTronTransfer {
        tx_hash: "f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1f4a1".to_string(),
        cursor_value: 65_000_000,
        block_number: Some(65_000_000),
        from_address: "TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh".to_string(),
        to_address: context.address.clone(),
        amount_raw: "2500000".to_string(),
        amount_decimal: "2.5".to_string(),
        token_contract: Some("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t".to_string()),
    };

    let draft = super::tron_transfer_event_draft(&context, &asset, transfer);

    assert_eq!(draft.tenant_id, context.tenant_id);
    assert_eq!(draft.chain_id, context.chain_id);
    assert_eq!(draft.address_id, context.id);
    assert_eq!(draft.asset_id, asset.id);
    assert_eq!(draft.event_type, "transfer");
    assert!(draft.is_transfer);
    assert_eq!(draft.direction, "in");
    assert_eq!(draft.block_number, Some(65_000_000));
    assert_eq!(draft.metadata["source"], "tron_transfer");
    assert_eq!(draft.metadata["cursor_value"], 65_000_000);
}

fn scan_context(address: &str) -> coin_listener_core::models::ScanAddressContext {
    coin_listener_core::models::ScanAddressContext {
        id: uuid::Uuid::from_u128(101),
        tenant_id: uuid::Uuid::from_u128(102),
        chain_id: uuid::Uuid::from_u128(103),
        address: address.to_string(),
        scan_interval_seconds: 300,
        chain_type: "tron".to_string(),
    }
}

fn asset(asset_type: &str, symbol: &str, contract: Option<&str>) -> coin_listener_core::models::Asset {
    coin_listener_core::models::Asset {
        id: uuid::Uuid::from_u128(201),
        chain_id: uuid::Uuid::from_u128(103),
        asset_type: asset_type.to_string(),
        symbol: symbol.to_string(),
        name: symbol.to_string(),
        contract_address: contract.map(ToOwned::to_owned),
        decimals: 6,
        is_builtin: true,
        status: "active".to_string(),
    }
}
```

- [ ] **Step 2: Run TRON client tests to verify RED**

Run:

```bash
cargo test -p coin-listener-chain-providers tron_client_builds --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers tron_request_errors --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers tron_transfer_event_draft --manifest-path backend/Cargo.toml
```

Expected: FAIL because `TronClient`, `format_tron_request_error`, and `tron_transfer_event_draft` do not exist.

- [ ] **Step 3: Implement TRON client and draft builder**

Update imports at the top of `backend/crates/chain-providers/src/tron.rs`:

```rust
use coin_listener_core::{
    models::{AddressEventDraft, Asset, ScanAddressContext},
    AppError, AppResult,
};
use num_bigint::BigUint;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{str::FromStr, time::Duration};
```

Add this client and draft code before the test module:

```rust
#[derive(Debug, Clone)]
pub struct TronClient {
    base_url: String,
    client: reqwest::Client,
}

impl TronClient {
    pub fn new(base_url: String, timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("valid TRON client");
        Self { base_url, client }
    }

    pub fn account_transactions_path(&self, address: &str) -> AppResult<String> {
        let address = normalize_tron_address(address)?;
        Ok(format!("/v1/accounts/{address}/transactions"))
    }

    pub fn account_trc20_path(&self, address: &str) -> AppResult<String> {
        let address = normalize_tron_address(address)?;
        Ok(format!("/v1/accounts/{address}/transactions/trc20"))
    }

    pub async fn account_transactions(&self, address: &str, min_timestamp: i64) -> AppResult<Vec<Value>> {
        let path = self.account_transactions_path(address)?;
        let body = self
            .get_json_body(
                "account transactions",
                &path,
                &[
                    ("only_confirmed", "true".to_string()),
                    ("limit", "200".to_string()),
                    ("min_timestamp", min_timestamp.to_string()),
                ],
            )
            .await?;
        parse_data_array(body, "account transactions")
    }

    pub async fn account_trc20_transfers(
        &self,
        address: &str,
        contract_address: &str,
        min_timestamp: i64,
    ) -> AppResult<Vec<Value>> {
        let path = self.account_trc20_path(address)?;
        let contract_address = normalize_tron_address(contract_address)?;
        let body = self
            .get_json_body(
                "TRC20 transfers",
                &path,
                &[
                    ("only_confirmed", "true".to_string()),
                    ("limit", "200".to_string()),
                    ("contract_address", contract_address),
                    ("min_timestamp", min_timestamp.to_string()),
                ],
            )
            .await?;
        parse_data_array(body, "TRC20 transfers")
    }

    async fn get_json_body(
        &self,
        operation: &str,
        path: &str,
        query: &[(impl AsRef<str>, String)],
    ) -> AppResult<Value> {
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        let response = self
            .client
            .get(&url)
            .query(query)
            .send()
            .await
            .map_err(|error| {
                AppError::Config(format_tron_request_error(
                    operation,
                    &self.base_url,
                    &error.without_url().to_string(),
                ))
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            AppError::Config(format!(
                "TRON {operation} response body failed: {}",
                error.without_url()
            ))
        })?;
        if !status.is_success() {
            return Err(AppError::Config(format!(
                "TRON {operation} returned http {status}: {body}"
            )));
        }
        serde_json::from_str(&body).map_err(|error| {
            AppError::Validation(format!("invalid TRON {operation} response json: {error}"))
        })
    }
}

pub fn format_tron_request_error(operation: &str, _base_url: &str, error: &str) -> String {
    format!("TRON {operation} request failed: {error}")
}

pub fn parse_data_array(body: Value, operation: &str) -> AppResult<Vec<Value>> {
    body.get("data")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| AppError::Validation(format!("TRON {operation} response missing data array")))
}

pub fn tron_transfer_event_draft(
    context: &ScanAddressContext,
    asset: &Asset,
    transfer: DecodedTronTransfer,
) -> AddressEventDraft {
    let direction = tron_transfer_direction(
        &transfer.from_address,
        &transfer.to_address,
        &context.address,
    );
    AddressEventDraft {
        tenant_id: context.tenant_id,
        chain_id: context.chain_id,
        address_id: context.id,
        asset_id: asset.id,
        event_type: "transfer".to_string(),
        direction: direction.to_string(),
        is_transfer: true,
        tx_hash: Some(transfer.tx_hash),
        log_index: None,
        block_number: transfer.block_number,
        block_hash: None,
        confirmations: 0,
        from_address: Some(transfer.from_address),
        to_address: Some(transfer.to_address),
        amount_raw: Some(transfer.amount_raw),
        amount_decimal: Some(transfer.amount_decimal),
        balance_before_raw: None,
        balance_after_raw: None,
        balance_delta_raw: None,
        metadata: json!({
            "source": "tron_transfer",
            "token_contract": transfer.token_contract,
            "cursor_value": transfer.cursor_value,
        }),
    }
}
```

- [ ] **Step 4: Run TRON provider tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-chain-providers tron_ --manifest-path backend/Cargo.toml
cargo check -p coin-listener-chain-providers --manifest-path backend/Cargo.toml
cargo fmt -p coin-listener-chain-providers --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 5: Checkpoint**

Run:

```bash
git status --short
```

If git status succeeds, run:

```bash
git add backend/crates/chain-providers/src/tron.rs
git commit -m "Add TRON provider client and event drafts"
```

Expected outside a git repository: skip commit and report changed files.

---

### Task 4: Add BTC provider decode helpers

**Files:**
- Create: `backend/crates/chain-providers/src/btc.rs`
- Modify: `backend/crates/chain-providers/src/lib.rs`

- [ ] **Step 1: Expose BTC module and write failing decode tests**

In `backend/crates/chain-providers/src/lib.rs`, add:

```rust
pub mod btc;
```

Create `backend/crates/chain-providers/src/btc.rs` with these tests first:

```rust
use coin_listener_core::{AppError, AppResult};
use num_bigint::BigInt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{str::FromStr, time::Duration};

#[cfg(test)]
mod tests {
    use super::{
        btc_transfer_event_draft, classify_btc_transaction, decode_btc_confirmed_balance,
        normalize_btc_address, sats_to_decimal_string, BtcTransaction,
    };
    use coin_listener_core::AppError;
    use serde_json::json;

    #[test]
    fn btc_address_normalization_accepts_supported_shapes() {
        assert_eq!(
            normalize_btc_address("  bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080  ").unwrap(),
            "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080"
        );
        assert!(normalize_btc_address("1BoatSLRHtKNngkdXEeobR76b53LETtpyT").is_ok());
        assert!(normalize_btc_address("3J98t1WpEZ73CNmQviecrnyiWrnqRhWNLy").is_ok());
        assert!(matches!(
            normalize_btc_address("0x1111111111111111111111111111111111111111"),
            Err(AppError::Validation(message)) if message.contains("BTC address")
        ));
    }

    #[test]
    fn btc_confirmed_balance_uses_chain_stats_only() {
        let payload = json!({
            "chain_stats": { "funded_txo_sum": 150000, "spent_txo_sum": 50000 },
            "mempool_stats": { "funded_txo_sum": 999999, "spent_txo_sum": 1 }
        });

        let balance = decode_btc_confirmed_balance(&payload).unwrap();

        assert_eq!(balance.balance_raw, "100000");
        assert_eq!(balance.balance_decimal, "0.001");
    }

    #[test]
    fn btc_transaction_classifies_inbound_delta() {
        let tx = btc_tx(json!({
            "txid": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "status": { "confirmed": true, "block_height": 800000, "block_hash": "0000000000000000000000000000000000000000000000000000000000000001" },
            "vin": [],
            "vout": [{ "scriptpubkey_address": "bc1watch", "value": 25000 }]
        }));

        let transfer = classify_btc_transaction(&tx, "bc1watch").unwrap().unwrap();

        assert_eq!(transfer.direction, "in");
        assert_eq!(transfer.amount_raw, "25000");
        assert_eq!(transfer.amount_decimal, "0.00025");
        assert_eq!(transfer.block_number, 800000);
        assert_eq!(transfer.block_hash, Some("0000000000000000000000000000000000000000000000000000000000000001".to_string()));
    }

    #[test]
    fn btc_transaction_classifies_outbound_delta() {
        let tx = btc_tx(json!({
            "txid": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "status": { "confirmed": true, "block_height": 800001 },
            "vin": [{ "prevout": { "scriptpubkey_address": "bc1watch", "value": 100000 } }],
            "vout": [{ "scriptpubkey_address": "bc1change", "value": 40000 }]
        }));

        let transfer = classify_btc_transaction(&tx, "bc1watch").unwrap().unwrap();

        assert_eq!(transfer.direction, "out");
        assert_eq!(transfer.amount_raw, "100000");
        assert_eq!(transfer.amount_decimal, "0.001");
    }

    #[test]
    fn btc_transaction_ignores_unrelated_transaction() {
        let tx = btc_tx(json!({
            "txid": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            "status": { "confirmed": true, "block_height": 800002 },
            "vin": [],
            "vout": [{ "scriptpubkey_address": "bc1other", "value": 40000 }]
        }));

        assert!(classify_btc_transaction(&tx, "bc1watch").unwrap().is_none());
    }

    #[test]
    fn btc_transaction_rejects_unconfirmed_transaction() {
        let tx = btc_tx(json!({
            "txid": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            "status": { "confirmed": false },
            "vin": [],
            "vout": [{ "scriptpubkey_address": "bc1watch", "value": 40000 }]
        }));

        assert!(matches!(
            classify_btc_transaction(&tx, "bc1watch"),
            Err(AppError::Validation(message)) if message.contains("confirmed")
        ));
    }

    #[test]
    fn btc_transfer_event_draft_maps_transfer_fields() {
        let context = scan_context("bc1watch");
        let asset = native_btc_asset();
        let tx = btc_tx(json!({
            "txid": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            "status": { "confirmed": true, "block_height": 800003 },
            "vin": [],
            "vout": [{ "scriptpubkey_address": "bc1watch", "value": 50000 }]
        }));
        let transfer = classify_btc_transaction(&tx, "bc1watch").unwrap().unwrap();

        let draft = btc_transfer_event_draft(&context, &asset, transfer);

        assert_eq!(draft.event_type, "transfer");
        assert!(draft.is_transfer);
        assert_eq!(draft.direction, "in");
        assert_eq!(draft.tx_hash, Some("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string()));
        assert_eq!(draft.log_index, None);
        assert_eq!(draft.block_number, Some(800003));
        assert_eq!(draft.metadata["source"], "btc_transaction");
    }

    fn btc_tx(payload: serde_json::Value) -> BtcTransaction {
        serde_json::from_value(payload).unwrap()
    }

    fn scan_context(address: &str) -> coin_listener_core::models::ScanAddressContext {
        coin_listener_core::models::ScanAddressContext {
            id: uuid::Uuid::from_u128(101),
            tenant_id: uuid::Uuid::from_u128(102),
            chain_id: uuid::Uuid::from_u128(103),
            address: address.to_string(),
            scan_interval_seconds: 300,
            chain_type: "utxo".to_string(),
        }
    }

    fn native_btc_asset() -> coin_listener_core::models::Asset {
        coin_listener_core::models::Asset {
            id: uuid::Uuid::from_u128(201),
            chain_id: uuid::Uuid::from_u128(103),
            asset_type: "native".to_string(),
            symbol: "BTC".to_string(),
            name: "BTC".to_string(),
            contract_address: None,
            decimals: 8,
            is_builtin: true,
            status: "active".to_string(),
        }
    }
}
```

- [ ] **Step 2: Run BTC tests to verify RED**

Run:

```bash
cargo test -p coin-listener-chain-providers btc_ --manifest-path backend/Cargo.toml
```

Expected: FAIL because BTC helper functions and structs do not exist.

- [ ] **Step 3: Implement BTC decode types and helpers**

Update imports at the top of `backend/crates/chain-providers/src/btc.rs`:

```rust
use coin_listener_core::{
    models::{AddressEventDraft, Asset, ScanAddressContext},
    AppError, AppResult,
};
use num_bigint::BigInt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{str::FromStr, time::Duration};
```

Add this implementation above the test module:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtcBalance {
    pub balance_raw: String,
    pub balance_decimal: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedBtcTransfer {
    pub tx_hash: String,
    pub block_number: i64,
    pub block_hash: Option<String>,
    pub direction: String,
    pub amount_raw: String,
    pub amount_decimal: String,
    pub received_raw: String,
    pub spent_raw: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BtcTransaction {
    pub txid: String,
    pub status: BtcTxStatus,
    #[serde(default)]
    pub vin: Vec<BtcVin>,
    #[serde(default)]
    pub vout: Vec<BtcVout>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BtcTxStatus {
    pub confirmed: bool,
    pub block_height: Option<i64>,
    pub block_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BtcVin {
    pub prevout: Option<BtcVout>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BtcVout {
    pub scriptpubkey_address: Option<String>,
    pub value: i64,
}

#[derive(Debug, Deserialize)]
struct BtcAddressPayload {
    chain_stats: BtcAddressStats,
}

#[derive(Debug, Deserialize)]
struct BtcAddressStats {
    funded_txo_sum: i64,
    spent_txo_sum: i64,
}

pub fn normalize_btc_address(address: &str) -> AppResult<String> {
    let address = address.trim();
    let valid_prefix = address.starts_with("bc1")
        || address.starts_with("tb1")
        || address.starts_with('1')
        || address.starts_with('3');
    let valid_chars = address.chars().all(|character| character.is_ascii_alphanumeric());
    if !valid_prefix || !valid_chars || address.len() < 14 || address.len() > 90 {
        return Err(AppError::Validation(format!("invalid BTC address {address}")));
    }
    Ok(address.to_string())
}

pub fn decode_btc_confirmed_balance(payload: &Value) -> AppResult<BtcBalance> {
    let payload: BtcAddressPayload = serde_json::from_value(payload.clone()).map_err(|error| {
        AppError::Validation(format!("invalid BTC address payload: {error}"))
    })?;
    if payload.chain_stats.funded_txo_sum < 0 || payload.chain_stats.spent_txo_sum < 0 {
        return Err(AppError::Validation("BTC chain stats cannot be negative".to_string()));
    }
    let raw = payload.chain_stats.funded_txo_sum - payload.chain_stats.spent_txo_sum;
    if raw < 0 {
        return Err(AppError::Validation("BTC confirmed balance cannot be negative".to_string()));
    }
    let balance_raw = raw.to_string();
    let balance_decimal = sats_to_decimal_string(&balance_raw)?;
    Ok(BtcBalance {
        balance_raw,
        balance_decimal,
    })
}

pub fn classify_btc_transaction(
    tx: &BtcTransaction,
    watched_address: &str,
) -> AppResult<Option<DecodedBtcTransfer>> {
    let watched_address = normalize_btc_address(watched_address)?;
    validate_txid(&tx.txid)?;
    if !tx.status.confirmed {
        return Err(AppError::Validation("BTC transaction is not confirmed".to_string()));
    }
    let block_number = tx
        .status
        .block_height
        .ok_or_else(|| AppError::Validation("confirmed BTC transaction missing block_height".to_string()))?;
    if block_number < 0 {
        return Err(AppError::Validation("BTC block_height cannot be negative".to_string()));
    }
    if let Some(block_hash) = &tx.status.block_hash {
        validate_block_hash(block_hash)?;
    }

    let received: i64 = tx
        .vout
        .iter()
        .filter(|output| output.scriptpubkey_address.as_deref() == Some(watched_address.as_str()))
        .map(|output| output.value)
        .sum();
    let spent: i64 = tx
        .vin
        .iter()
        .filter_map(|input| input.prevout.as_ref())
        .filter(|prevout| prevout.scriptpubkey_address.as_deref() == Some(watched_address.as_str()))
        .map(|prevout| prevout.value)
        .sum();

    if received < 0 || spent < 0 {
        return Err(AppError::Validation("BTC input/output value cannot be negative".to_string()));
    }
    if received == 0 && spent == 0 {
        return Ok(None);
    }

    let delta = BigInt::from(received) - BigInt::from(spent);
    let direction = if delta.sign() == num_bigint::Sign::Plus {
        "in"
    } else if delta.sign() == num_bigint::Sign::Minus {
        "out"
    } else {
        "self"
    };
    let amount_raw = delta.abs().to_string();
    let amount_decimal = sats_to_decimal_string(&amount_raw)?;

    Ok(Some(DecodedBtcTransfer {
        tx_hash: tx.txid.clone(),
        block_number,
        block_hash: tx.status.block_hash.clone(),
        direction: direction.to_string(),
        amount_raw,
        amount_decimal,
        received_raw: received.to_string(),
        spent_raw: spent.to_string(),
    }))
}

pub fn btc_transfer_event_draft(
    context: &ScanAddressContext,
    asset: &Asset,
    transfer: DecodedBtcTransfer,
) -> AddressEventDraft {
    AddressEventDraft {
        tenant_id: context.tenant_id,
        chain_id: context.chain_id,
        address_id: context.id,
        asset_id: asset.id,
        event_type: "transfer".to_string(),
        direction: transfer.direction,
        is_transfer: true,
        tx_hash: Some(transfer.tx_hash),
        log_index: None,
        block_number: Some(transfer.block_number),
        block_hash: transfer.block_hash,
        confirmations: 0,
        from_address: None,
        to_address: None,
        amount_raw: Some(transfer.amount_raw),
        amount_decimal: Some(transfer.amount_decimal),
        balance_before_raw: None,
        balance_after_raw: None,
        balance_delta_raw: None,
        metadata: json!({
            "source": "btc_transaction",
            "received_raw": transfer.received_raw,
            "spent_raw": transfer.spent_raw,
        }),
    }
}

pub fn sats_to_decimal_string(raw: &str) -> AppResult<String> {
    decimal_string(raw, 8)
}

fn decimal_string(raw: &str, decimals: usize) -> AppResult<String> {
    let _ = BigInt::from_str(raw)
        .map_err(|error| AppError::Validation(format!("invalid BTC amount {raw}: {error}")))?;
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

fn validate_txid(txid: &str) -> AppResult<()> {
    let valid = txid.len() == 64 && txid.chars().all(|character| character.is_ascii_hexdigit());
    if !valid {
        return Err(AppError::Validation(format!("invalid BTC txid {txid}")));
    }
    Ok(())
}

fn validate_block_hash(block_hash: &str) -> AppResult<()> {
    let valid = block_hash.len() == 64
        && block_hash
            .chars()
            .all(|character| character.is_ascii_hexdigit());
    if !valid {
        return Err(AppError::Validation(format!("invalid BTC block_hash {block_hash}")));
    }
    Ok(())
}
```

- [ ] **Step 4: Fix missing trait import if needed**

If Rust reports `no method named abs found for BigInt`, add this import at the top:

```rust
use num_traits::Signed;
```

If that import is needed, add `num-traits = "0.2"` to `backend/crates/chain-providers/Cargo.toml`:

```toml
num-traits = "0.2"
```

- [ ] **Step 5: Run BTC decode tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-chain-providers btc_address_normalization --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers btc_confirmed_balance --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers btc_transaction --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers btc_transfer_event_draft --manifest-path backend/Cargo.toml
cargo check -p coin-listener-chain-providers --manifest-path backend/Cargo.toml
cargo fmt -p coin-listener-chain-providers --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 6: Checkpoint**

Run:

```bash
git status --short
```

If git status succeeds, run:

```bash
git add backend/crates/chain-providers/src/lib.rs backend/crates/chain-providers/src/btc.rs backend/crates/chain-providers/Cargo.toml
git commit -m "Add BTC transaction decode helpers"
```

Expected outside a git repository: skip commit and report changed files.

---

### Task 5: Add BTC HTTP client

**Files:**
- Modify: `backend/crates/chain-providers/src/btc.rs`

- [ ] **Step 1: Write failing BTC client tests**

Append these tests inside `#[cfg(test)] mod tests` in `backend/crates/chain-providers/src/btc.rs`:

```rust
#[test]
fn btc_client_builds_esplora_paths() {
    let client = super::BtcClient::new(
        "https://blockstream.info/api/".to_string(),
        std::time::Duration::from_secs(5),
    );

    assert_eq!(
        client.address_path("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080").unwrap(),
        "/address/bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080"
    );
    assert_eq!(
        client.address_txs_path("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080").unwrap(),
        "/address/bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080/txs/chain"
    );
}

#[test]
fn btc_request_errors_do_not_include_provider_url() {
    let message = super::format_btc_request_error(
        "address txs",
        "https://enterprise.blockstream.info/api/private-key",
        "connection refused",
    );

    assert!(message.contains("address txs"));
    assert!(message.contains("connection refused"));
    assert!(!message.contains("blockstream.info"));
    assert!(!message.contains("private-key"));
}
```

- [ ] **Step 2: Run BTC client tests to verify RED**

Run:

```bash
cargo test -p coin-listener-chain-providers btc_client_builds --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers btc_request_errors --manifest-path backend/Cargo.toml
```

Expected: FAIL because `BtcClient` and `format_btc_request_error` do not exist.

- [ ] **Step 3: Implement BTC client**

Add this code before the test module in `backend/crates/chain-providers/src/btc.rs`:

```rust
#[derive(Debug, Clone)]
pub struct BtcClient {
    base_url: String,
    client: reqwest::Client,
}

impl BtcClient {
    pub fn new(base_url: String, timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("valid BTC client");
        Self { base_url, client }
    }

    pub fn address_path(&self, address: &str) -> AppResult<String> {
        let address = normalize_btc_address(address)?;
        Ok(format!("/address/{address}"))
    }

    pub fn address_txs_path(&self, address: &str) -> AppResult<String> {
        let address = normalize_btc_address(address)?;
        Ok(format!("/address/{address}/txs/chain"))
    }

    pub async fn address_balance(&self, address: &str) -> AppResult<BtcBalance> {
        let path = self.address_path(address)?;
        let body = self.get_json_body("address balance", &path).await?;
        decode_btc_confirmed_balance(&body)
    }

    pub async fn address_transactions(&self, address: &str) -> AppResult<Vec<BtcTransaction>> {
        let path = self.address_txs_path(address)?;
        let body = self.get_json_body("address txs", &path).await?;
        serde_json::from_value(body)
            .map_err(|error| AppError::Validation(format!("invalid BTC address txs response: {error}")))
    }

    async fn get_json_body(&self, operation: &str, path: &str) -> AppResult<Value> {
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|error| {
                AppError::Config(format_btc_request_error(
                    operation,
                    &self.base_url,
                    &error.without_url().to_string(),
                ))
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            AppError::Config(format!(
                "BTC {operation} response body failed: {}",
                error.without_url()
            ))
        })?;
        if !status.is_success() {
            return Err(AppError::Config(format!(
                "BTC {operation} returned http {status}: {body}"
            )));
        }
        serde_json::from_str(&body).map_err(|error| {
            AppError::Validation(format!("invalid BTC {operation} response json: {error}"))
        })
    }
}

pub fn format_btc_request_error(operation: &str, _base_url: &str, error: &str) -> String {
    format!("BTC {operation} request failed: {error}")
}
```

- [ ] **Step 4: Run BTC provider tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-chain-providers btc_ --manifest-path backend/Cargo.toml
cargo check -p coin-listener-chain-providers --manifest-path backend/Cargo.toml
cargo fmt -p coin-listener-chain-providers --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 5: Checkpoint**

Run:

```bash
git status --short
```

If git status succeeds, run:

```bash
git add backend/crates/chain-providers/src/btc.rs
git commit -m "Add BTC Esplora client"
```

Expected outside a git repository: skip commit and report changed files.

---

### Task 6: Extend worker scan plan and shared range helpers

**Files:**
- Modify: `backend/crates/worker/src/lib.rs`

- [ ] **Step 1: Write failing worker scan plan and range tests**

In `backend/crates/worker/src/lib.rs`, update the `scan_plan_for_chain` test module:

```rust
mod scan_plan_for_chain {
    use crate::{scan_plan_for_chain, ScanPlan};

    #[test]
    fn evm_chain_uses_native_balance_scan() {
        assert_eq!(scan_plan_for_chain("evm"), ScanPlan::EvmNativeBalance);
    }

    #[test]
    fn tron_chain_uses_tron_scan() {
        assert_eq!(scan_plan_for_chain("tron"), ScanPlan::Tron);
    }

    #[test]
    fn utxo_chain_uses_btc_scan() {
        assert_eq!(scan_plan_for_chain("utxo"), ScanPlan::Btc);
    }

    #[test]
    fn unknown_chain_is_unsupported() {
        assert_eq!(
            scan_plan_for_chain("unknown"),
            ScanPlan::Unsupported("unknown".to_string())
        );
    }
}
```

Add this test module after `evm_transfer_ranges`:

```rust
mod cursor_ranges {
    use chrono::{TimeZone, Utc};
    use coin_listener_core::models::ScanCursor;
    use uuid::Uuid;

    use crate::{confirmed_cursor_range, BTC_INITIAL_BLOCK_WINDOW, TRON_INITIAL_WATERMARK_WINDOW};

    fn cursor(last_scanned_block: i64) -> ScanCursor {
        ScanCursor {
            id: Uuid::from_u128(1),
            tenant_id: Uuid::from_u128(2),
            chain_id: Uuid::from_u128(3),
            address_id: Uuid::from_u128(4),
            cursor_type: "test".to_string(),
            last_scanned_block,
            updated_at: Utc.with_ymd_and_hms(2026, 5, 18, 9, 0, 0).unwrap(),
        }
    }

    #[test]
    fn confirmed_cursor_range_uses_initial_window() {
        let range = confirmed_cursor_range(None, 100_000, 3, BTC_INITIAL_BLOCK_WINDOW, "btc").unwrap();

        assert_eq!(range, Some((99_995, 99_997)));
    }

    #[test]
    fn confirmed_cursor_range_starts_after_cursor() {
        let range = confirmed_cursor_range(Some(&cursor(99_990)), 100_000, 3, 10, "btc").unwrap();

        assert_eq!(range, Some((99_991, 99_997)));
    }

    #[test]
    fn confirmed_cursor_range_returns_none_when_cursor_is_current() {
        let range = confirmed_cursor_range(Some(&cursor(99_997)), 100_000, 3, 10, "btc").unwrap();

        assert_eq!(range, None);
    }

    #[test]
    fn confirmed_cursor_range_rejects_negative_confirmations() {
        let error = confirmed_cursor_range(None, 100, -1, TRON_INITIAL_WATERMARK_WINDOW, "tron").unwrap_err();

        assert!(error.to_string().contains("tron confirmations"));
    }
}
```

- [ ] **Step 2: Run worker tests to verify RED**

Run:

```bash
cargo test -p worker scan_plan_for_chain --manifest-path backend/Cargo.toml
cargo test -p worker cursor_ranges --manifest-path backend/Cargo.toml
```

Expected: FAIL because `ScanPlan::Tron`, `ScanPlan::Btc`, `confirmed_cursor_range`, `BTC_INITIAL_BLOCK_WINDOW`, and `TRON_INITIAL_WATERMARK_WINDOW` do not exist.

- [ ] **Step 3: Add scan plan variants and range helper**

In `backend/crates/worker/src/lib.rs`, add constants near existing EVM constants:

```rust
pub const TRON_TRX_TRANSFER_CURSOR: &str = "tron_trx_transfer";
pub const TRON_TRC20_TRANSFER_CURSOR: &str = "tron_trc20_transfer";
pub const BTC_TRANSACTION_CURSOR: &str = "btc_transaction";
pub const TRON_INITIAL_WATERMARK_WINDOW: i64 = 86_400_000;
pub const BTC_INITIAL_BLOCK_WINDOW: i64 = 3;
```

Update `ScanPlan`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanPlan {
    EvmNativeBalance,
    Tron,
    Btc,
    Unsupported(String),
}
```

Update `scan_plan_for_chain`:

```rust
pub fn scan_plan_for_chain(chain_type: &str) -> ScanPlan {
    match chain_type {
        "evm" => ScanPlan::EvmNativeBalance,
        "tron" => ScanPlan::Tron,
        "utxo" => ScanPlan::Btc,
        other => ScanPlan::Unsupported(other.to_string()),
    }
}
```

Add this helper after `evm_transfer_scan_range`:

```rust
pub fn confirmed_cursor_range(
    cursor: Option<&ScanCursor>,
    latest_value: i64,
    confirmations: i32,
    initial_window: i64,
    label: &str,
) -> AppResult<Option<(i64, i64)>> {
    if confirmations < 0 {
        return Err(AppError::Validation(format!(
            "{label} confirmations cannot be negative"
        )));
    }
    if initial_window <= 0 {
        return Err(AppError::Validation(format!(
            "{label} initial window must be positive"
        )));
    }
    let confirmed_to = latest_value - i64::from(confirmations);
    if confirmed_to < 0 {
        return Ok(None);
    }
    let from_value = cursor
        .map(|cursor| cursor.last_scanned_block + 1)
        .unwrap_or_else(|| (confirmed_to - initial_window + 1).max(0));
    if confirmed_to < from_value {
        return Ok(None);
    }
    Ok(Some((from_value, confirmed_to)))
}
```

- [ ] **Step 4: Run worker tests to verify GREEN**

Run:

```bash
cargo test -p worker scan_plan_for_chain --manifest-path backend/Cargo.toml
cargo test -p worker cursor_ranges --manifest-path backend/Cargo.toml
cargo check -p worker --manifest-path backend/Cargo.toml
cargo fmt -p worker --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 5: Checkpoint**

Run:

```bash
git status --short
```

If git status succeeds, run:

```bash
git add backend/crates/worker/src/lib.rs
git commit -m "Extend worker scan plan for BTC and TRON"
```

Expected outside a git repository: skip commit and report changed files.

---

### Task 7: Wire TRON scanning into worker

**Files:**
- Modify: `backend/crates/worker/src/lib.rs`

- [ ] **Step 1: Write failing TRON worker unit tests**

Update worker imports at the top of `backend/crates/worker/src/lib.rs` to include TRON types:

```rust
use coin_listener_chain_providers::{
    evm::{
        self, address_to_topic, evm_balance_change_event, parse_hex_u256_to_decimal_string,
        transfer_event_draft, wei_to_decimal_string, EvmBlockTag, EvmLogFilter, EvmRpcClient,
        TRANSFER_TOPIC0,
    },
    tron::{self, TronClient},
};
```

Add this test module before `build_notify_event_task`:

```rust
mod tron_worker_helpers {
    use coin_listener_chain_providers::tron::DecodedTronTransfer;
    use coin_listener_core::models::{Asset, ScanAddressContext};
    use uuid::Uuid;

    use crate::{tron_cursor_value, tron_transfer_should_scan, TRON_TRC20_TRANSFER_CURSOR, TRON_TRX_TRANSFER_CURSOR};

    #[test]
    fn tron_cursor_value_uses_max_transfer_cursor() {
        let transfers = vec![
            transfer(10, "TA111111111111111111111111111111111"),
            transfer(15, "TB111111111111111111111111111111111"),
        ];

        assert_eq!(tron_cursor_value(&transfers), Some(15));
    }

    #[test]
    fn tron_transfer_should_scan_respects_asset_contract() {
        let asset = asset("trc20", Some("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t"));
        let transfer = transfer(10, "TA111111111111111111111111111111111")
            .with_contract("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t");

        assert!(tron_transfer_should_scan(&asset, &transfer));
    }

    #[test]
    fn tron_cursor_constants_are_stable() {
        assert_eq!(TRON_TRX_TRANSFER_CURSOR, "tron_trx_transfer");
        assert_eq!(TRON_TRC20_TRANSFER_CURSOR, "tron_trc20_transfer");
    }

    fn transfer(cursor_value: i64, from: &str) -> DecodedTronTransfer {
        DecodedTronTransfer {
            tx_hash: format!("{cursor_value:064x}"),
            cursor_value,
            block_number: Some(cursor_value),
            from_address: from.to_string(),
            to_address: "TQ6p7JAFM2Z2V5Q3U6QwY7Xx9z5xZQkP8E".to_string(),
            amount_raw: "1000000".to_string(),
            amount_decimal: "1.0".to_string(),
            token_contract: None,
        }
    }

    trait WithContract {
        fn with_contract(self, contract: &str) -> Self;
    }

    impl WithContract for DecodedTronTransfer {
        fn with_contract(mut self, contract: &str) -> Self {
            self.token_contract = Some(contract.to_string());
            self
        }
    }

    fn asset(asset_type: &str, contract: Option<&str>) -> Asset {
        Asset {
            id: Uuid::from_u128(201),
            chain_id: Uuid::from_u128(103),
            asset_type: asset_type.to_string(),
            symbol: "USDT".to_string(),
            name: "USDT".to_string(),
            contract_address: contract.map(ToOwned::to_owned),
            decimals: 6,
            is_builtin: true,
            status: "active".to_string(),
        }
    }
}
```

- [ ] **Step 2: Run TRON worker tests to verify RED**

Run:

```bash
cargo test -p worker tron_worker_helpers --manifest-path backend/Cargo.toml
```

Expected: FAIL because helper functions do not exist or imports are incomplete.

- [ ] **Step 3: Add TRON helper functions**

In `backend/crates/worker/src/lib.rs`, add after `scan_evm_address`:

```rust
pub fn tron_cursor_value(transfers: &[tron::DecodedTronTransfer]) -> Option<i64> {
    transfers.iter().map(|transfer| transfer.cursor_value).max()
}

pub fn tron_transfer_should_scan(asset: &Asset, transfer: &tron::DecodedTronTransfer) -> bool {
    match (&asset.contract_address, &transfer.token_contract) {
        (Some(expected), Some(actual)) => expected == actual,
        (None, None) => asset.asset_type == "native",
        _ => false,
    }
}
```

Add TRON scan helper after those functions:

```rust
pub async fn scan_tron_address(
    pool: &PgPool,
    task: &ScanAddressTask,
    _now: DateTime<Utc>,
) -> AppResult<Vec<AddressEvent>> {
    let context = repositories::get_scan_address_context(pool, task.address_id).await?;
    let provider = repositories::active_rpc_provider_for_chain(pool, context.chain_id).await?;
    let native_asset = repositories::native_asset_for_chain(pool, context.chain_id).await?;
    let timeout_ms = u64::try_from(provider.timeout_ms)
        .map_err(|_| AppError::Validation("timeout_ms must be positive".to_string()))?;
    if timeout_ms == 0 {
        return Err(AppError::Validation("timeout_ms must be positive".to_string()));
    }
    let client = TronClient::new(provider.base_url.clone(), Duration::from_millis(timeout_ms));
    let mut events = Vec::new();

    let trx_cursor = repositories::scan_cursor(pool, context.id, TRON_TRX_TRANSFER_CURSOR).await?;
    let trx_from = trx_cursor.as_ref().map(|cursor| cursor.last_scanned_block + 1).unwrap_or(0);
    let trx_payloads = client.account_transactions(&context.address, trx_from).await?;
    let mut trx_transfers = Vec::new();
    for payload in trx_payloads {
        let transfer = tron::decode_trx_transfer(&payload, native_asset.decimals)?;
        let draft = tron::tron_transfer_event_draft(&context, &native_asset, transfer.clone());
        if let Some(event) = repositories::insert_event_if_not_exists(pool, draft).await? {
            events.push(event);
        }
        trx_transfers.push(transfer);
    }
    if let Some(cursor_value) = tron_cursor_value(&trx_transfers) {
        repositories::upsert_scan_cursor(
            pool,
            context.tenant_id,
            context.chain_id,
            context.id,
            TRON_TRX_TRANSFER_CURSOR,
            cursor_value,
        )
        .await?;
    }

    let trc20_assets = repositories::active_assets_for_chain_by_type(pool, context.chain_id, "trc20").await?;
    let trc20_cursor = repositories::scan_cursor(pool, context.id, TRON_TRC20_TRANSFER_CURSOR).await?;
    let trc20_from = trc20_cursor.as_ref().map(|cursor| cursor.last_scanned_block + 1).unwrap_or(0);
    let mut trc20_transfers = Vec::new();
    for asset in trc20_assets {
        let Some(contract_address) = asset.contract_address.clone() else {
            continue;
        };
        let payloads = client
            .account_trc20_transfers(&context.address, &contract_address, trc20_from)
            .await?;
        for payload in payloads {
            let transfer = tron::decode_trc20_transfer(&payload, &contract_address, asset.decimals)?;
            if !tron_transfer_should_scan(&asset, &transfer) {
                continue;
            }
            let draft = tron::tron_transfer_event_draft(&context, &asset, transfer.clone());
            if let Some(event) = repositories::insert_event_if_not_exists(pool, draft).await? {
                events.push(event);
            }
            trc20_transfers.push(transfer);
        }
    }
    if let Some(cursor_value) = tron_cursor_value(&trc20_transfers) {
        repositories::upsert_scan_cursor(
            pool,
            context.tenant_id,
            context.chain_id,
            context.id,
            TRON_TRC20_TRANSFER_CURSOR,
            cursor_value,
        )
        .await?;
    }

    Ok(events)
}
```

- [ ] **Step 4: Wire TRON branch into `process_locked_scan_task`**

In `process_locked_scan_task`, add this match arm before `ScanPlan::Unsupported`:

```rust
ScanPlan::Tron => {
    let events = scan_tron_address(pool, task, now).await?;
    for event in &events {
        let notify_task = build_notify_event_task(event, now);
        notify_queue.enqueue(redis, &notify_task).await?;
    }
    repositories::finish_address_scan(pool, task.address_id, now, next_scan_at).await?;
    Ok(ScanTaskOutcome::Scanned)
}
```

- [ ] **Step 5: Run TRON worker tests to verify GREEN**

Run:

```bash
cargo test -p worker tron_worker_helpers --manifest-path backend/Cargo.toml
cargo test -p worker scan_plan_for_chain --manifest-path backend/Cargo.toml
cargo check -p worker --manifest-path backend/Cargo.toml
cargo fmt -p worker --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 6: Checkpoint**

Run:

```bash
git status --short
```

If git status succeeds, run:

```bash
git add backend/crates/worker/src/lib.rs
git commit -m "Wire TRON scanning into worker"
```

Expected outside a git repository: skip commit and report changed files.

---

### Task 8: Wire BTC scanning into worker

**Files:**
- Modify: `backend/crates/worker/src/lib.rs`

- [ ] **Step 1: Write failing BTC worker unit tests**

Update worker imports at the top of `backend/crates/worker/src/lib.rs` to include BTC types:

```rust
use coin_listener_chain_providers::{
    btc::{self, BtcClient},
    evm::{
        self, address_to_topic, evm_balance_change_event, parse_hex_u256_to_decimal_string,
        transfer_event_draft, wei_to_decimal_string, EvmBlockTag, EvmLogFilter, EvmRpcClient,
        TRANSFER_TOPIC0,
    },
    tron::{self, TronClient},
};
```

Add this test module before `build_notify_event_task`:

```rust
mod btc_worker_helpers {
    use coin_listener_chain_providers::btc::DecodedBtcTransfer;

    use crate::{btc_cursor_value, BTC_TRANSACTION_CURSOR};

    #[test]
    fn btc_cursor_value_uses_highest_block_number() {
        let transfers = vec![transfer(800_000), transfer(800_003), transfer(800_001)];

        assert_eq!(btc_cursor_value(&transfers), Some(800_003));
    }

    #[test]
    fn btc_cursor_constant_is_stable() {
        assert_eq!(BTC_TRANSACTION_CURSOR, "btc_transaction");
    }

    fn transfer(block_number: i64) -> DecodedBtcTransfer {
        DecodedBtcTransfer {
            tx_hash: format!("{block_number:064x}"),
            block_number,
            block_hash: None,
            direction: "in".to_string(),
            amount_raw: "1000".to_string(),
            amount_decimal: "0.00001".to_string(),
            received_raw: "1000".to_string(),
            spent_raw: "0".to_string(),
        }
    }
}
```

- [ ] **Step 2: Run BTC worker tests to verify RED**

Run:

```bash
cargo test -p worker btc_worker_helpers --manifest-path backend/Cargo.toml
```

Expected: FAIL because BTC worker helper functions do not exist or imports are incomplete.

- [ ] **Step 3: Add BTC helper and scan function**

In `backend/crates/worker/src/lib.rs`, add after `scan_tron_address`:

```rust
pub fn btc_cursor_value(transfers: &[btc::DecodedBtcTransfer]) -> Option<i64> {
    transfers.iter().map(|transfer| transfer.block_number).max()
}

pub async fn scan_btc_address(
    pool: &PgPool,
    task: &ScanAddressTask,
    _now: DateTime<Utc>,
) -> AppResult<Vec<AddressEvent>> {
    let context = repositories::get_scan_address_context(pool, task.address_id).await?;
    let provider = repositories::active_rpc_provider_for_chain(pool, context.chain_id).await?;
    let native_asset = repositories::native_asset_for_chain(pool, context.chain_id).await?;
    let timeout_ms = u64::try_from(provider.timeout_ms)
        .map_err(|_| AppError::Validation("timeout_ms must be positive".to_string()))?;
    if timeout_ms == 0 {
        return Err(AppError::Validation("timeout_ms must be positive".to_string()));
    }
    let client = BtcClient::new(provider.base_url.clone(), Duration::from_millis(timeout_ms));

    let balance = client.address_balance(&context.address).await?;
    repositories::insert_balance_snapshot(
        pool,
        CreateBalanceSnapshotRequest {
            tenant_id: context.tenant_id,
            chain_id: context.chain_id,
            address_id: context.id,
            asset_id: native_asset.id,
            balance_raw: balance.balance_raw,
            balance_decimal: balance.balance_decimal,
            block_number: None,
            block_hash: None,
            source_provider_id: Some(provider.id),
        },
    )
    .await?;

    let cursor = repositories::scan_cursor(pool, context.id, BTC_TRANSACTION_CURSOR).await?;
    let from_block = cursor.as_ref().map(|cursor| cursor.last_scanned_block + 1).unwrap_or(0);
    let txs = client.address_transactions(&context.address).await?;
    let mut events = Vec::new();
    let mut transfers = Vec::new();
    for tx in txs {
        let Some(transfer) = btc::classify_btc_transaction(&tx, &context.address)? else {
            continue;
        };
        if transfer.block_number < from_block {
            continue;
        }
        let draft = btc::btc_transfer_event_draft(&context, &native_asset, transfer.clone());
        if let Some(event) = repositories::insert_event_if_not_exists(pool, draft).await? {
            events.push(event);
        }
        transfers.push(transfer);
    }
    if let Some(cursor_value) = btc_cursor_value(&transfers) {
        repositories::upsert_scan_cursor(
            pool,
            context.tenant_id,
            context.chain_id,
            context.id,
            BTC_TRANSACTION_CURSOR,
            cursor_value,
        )
        .await?;
    }

    Ok(events)
}
```

- [ ] **Step 4: Wire BTC branch into `process_locked_scan_task`**

In `process_locked_scan_task`, add this match arm before `ScanPlan::Unsupported`:

```rust
ScanPlan::Btc => {
    let events = scan_btc_address(pool, task, now).await?;
    for event in &events {
        let notify_task = build_notify_event_task(event, now);
        notify_queue.enqueue(redis, &notify_task).await?;
    }
    repositories::finish_address_scan(pool, task.address_id, now, next_scan_at).await?;
    Ok(ScanTaskOutcome::Scanned)
}
```

- [ ] **Step 5: Run BTC worker tests to verify GREEN**

Run:

```bash
cargo test -p worker btc_worker_helpers --manifest-path backend/Cargo.toml
cargo test -p worker scan_plan_for_chain --manifest-path backend/Cargo.toml
cargo check -p worker --manifest-path backend/Cargo.toml
cargo fmt -p worker --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 6: Checkpoint**

Run:

```bash
git status --short
```

If git status succeeds, run:

```bash
git add backend/crates/worker/src/lib.rs
git commit -m "Wire BTC scanning into worker"
```

Expected outside a git repository: skip commit and report changed files.

---

### Task 9: Final verification

**Files:**
- Verify: `backend/crates/chain-providers/src/lib.rs`
- Verify: `backend/crates/chain-providers/src/tron.rs`
- Verify: `backend/crates/chain-providers/src/btc.rs`
- Verify: `backend/crates/storage/src/repositories.rs`
- Verify: `backend/crates/worker/src/lib.rs`
- Verify: `frontend/`
- Verify: `docker-compose.yml`

- [ ] **Step 1: Run backend formatting check**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: exit 0. If it fails only due formatting, run:

```bash
cargo fmt --all --manifest-path backend/Cargo.toml
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected after formatting: exit 0.

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

Expected: exit 0. Existing Vite warnings about `lottie-web` direct eval or chunk size are acceptable; build failure is not acceptable.

- [ ] **Step 5: Validate Docker Compose configuration**

Run:

```bash
docker compose -f docker-compose.yml config
```

Expected: exit 0 and rendered compose configuration.

- [ ] **Step 6: Final checkpoint**

Run:

```bash
git status --short
```

If git status succeeds, run:

```bash
git add backend/crates/chain-providers/src/lib.rs backend/crates/chain-providers/src/tron.rs backend/crates/chain-providers/src/btc.rs backend/crates/chain-providers/Cargo.toml backend/crates/storage/src/repositories.rs backend/crates/worker/src/lib.rs
git commit -m "Implement BTC and TRON listeners"
```

Expected outside a git repository: skip commit and report changed files.

---

## Self-Review Notes

- Spec coverage: Tasks 2-3 cover TRON provider parsing, URL redaction, event drafts, and TRC20/TRX transfer decode. Tasks 4-5 cover BTC provider parsing, Esplora client paths, balance decode, transaction delta classification, URL redaction, and BTC event drafts. Task 1 covers the storage helper needed for TRC20 asset lookup. Tasks 6-8 cover worker dispatch, cursors, scan helpers, event insertion, and notification enqueue. Task 9 covers final regression verification.
- Scope control: The plan does not add WebSocket/Telegram delivery, provider failover, health checks, a chain indexer, BTC mempool scanning, TRON subscriptions, token discovery, frontend pages, or new tables.
- Type consistency: `DecodedTronTransfer`, `DecodedBtcTransfer`, `TronClient`, `BtcClient`, cursor constants, and helper names are used consistently across provider and worker tasks.
- Checkpoint behavior: Every checkpoint first runs `git status --short` and skips commit when the directory is not a git repository.
