# BTC/TRON Pagination Reliability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make BTC Esplora and TRON TronGrid scans process paginated confirmed transfer history without silently dropping events.

**Architecture:** Keep pagination parsing inside `coin-listener-chain-providers` and keep scan control inside `worker`. Providers return one page plus a next-page token; worker loops pages, writes events idempotently, and advances cursors only after all processed pages succeed.

**Tech Stack:** Rust 2021, Tokio, reqwest, serde/serde_json, SQLx, PostgreSQL, Redis queues, chrono, uuid, num-bigint.

---

## Scope and Constraints

Implement only the approved pagination reliability scope from `docs/superpowers/specs/2026-05-18-coin-listener-btc-tron-pagination-reliability-design.md`.

Do not implement notification outbox, TRON balance snapshots, provider failover, schema changes, mempool support, or frontend UI.

The project directory has returned `fatal: not a git repository`. Each checkpoint must run `git status --short`; if it fails, skip commit and report changed files.

---

## File Structure

Modify:

```text
backend/crates/chain-providers/src/btc.rs
backend/crates/chain-providers/src/tron.rs
backend/crates/worker/src/lib.rs
```

No new files and no database migrations.

Responsibilities:

- `backend/crates/chain-providers/src/btc.rs`: Esplora page model, page path construction, page response parsing, non-2xx body redaction.
- `backend/crates/chain-providers/src/tron.rs`: TronGrid page model, fingerprint parsing, fingerprint query construction, non-2xx body redaction.
- `backend/crates/worker/src/lib.rs`: page loop limits, BTC cursor overlap, page-aware TRON log index offsets, BTC/TRON paginated scan loops.

---

### Task 1: Add BTC provider pagination primitives

**Files:**
- Modify: `backend/crates/chain-providers/src/btc.rs:12-170`
- Test: `backend/crates/chain-providers/src/btc.rs:402-620`

- [ ] **Step 1: Write failing BTC pagination tests**

In `backend/crates/chain-providers/src/btc.rs`, update the test import block inside `#[cfg(test)] mod tests` from:

```rust
use super::{
    btc_transfer_event_draft, classify_btc_transaction, decode_btc_confirmed_balance,
    normalize_btc_address, BtcTransaction, DecodedBtcTransfer,
};
```

to:

```rust
use super::{
    btc_transfer_event_draft, classify_btc_transaction, decode_btc_confirmed_balance,
    decode_btc_transaction_page, normalize_btc_address, BtcTransaction, DecodedBtcTransfer,
};
```

Add these tests after `btc_client_builds_address_paths_without_double_slashes`:

```rust
#[test]
fn btc_client_builds_paginated_transaction_paths() {
    let client = super::BtcClient::new(
        "https://mempool.space/api/".to_string(),
        std::time::Duration::from_secs(5),
    );
    let address = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080";
    let txid = btc_hash(9);

    assert_eq!(
        client.address_txs_page_path(address, None).unwrap(),
        format!("/address/{address}/txs/chain")
    );
    assert_eq!(
        client
            .address_txs_page_path(address, Some(txid.as_str()))
            .unwrap(),
        format!("/address/{address}/txs/chain/{txid}")
    );
}

#[test]
fn btc_transaction_page_uses_last_txid_as_next_page_token() {
    let page = decode_btc_transaction_page(json!([
        btc_tx_json(1, 840_000),
        btc_tx_json(2, 840_001)
    ]))
    .unwrap();

    assert_eq!(page.transactions.len(), 2);
    assert_eq!(page.next_last_seen_txid, Some(btc_hash(2)));
}

#[test]
fn btc_transaction_page_without_transactions_has_no_next_token() {
    let page = decode_btc_transaction_page(json!([])).unwrap();

    assert!(page.transactions.is_empty());
    assert_eq!(page.next_last_seen_txid, None);
}

#[test]
fn btc_status_errors_do_not_include_provider_url() {
    let message = super::format_btc_status_error(
        "address transactions",
        "https://btc.example.com/provider-key/",
        reqwest::StatusCode::BAD_GATEWAY,
        "upstream https://btc.example.com/provider-key/ failed",
    );

    assert!(message.contains("address transactions"));
    assert!(message.contains("502 Bad Gateway"));
    assert!(message.contains("[redacted provider url]"));
    assert!(!message.contains("btc.example.com"));
    assert!(!message.contains("provider-key"));
}
```

Add this helper after the existing `btc_tx` helper:

```rust
fn btc_tx_json(seed: u8, block_height: i64) -> Value {
    json!({
        "txid": btc_hash(seed),
        "status": {
            "confirmed": true,
            "block_height": block_height,
            "block_hash": btc_hash(seed + 20)
        },
        "vin": [input(OTHER, 10_000)],
        "vout": [output(WATCHED, 9_000)]
    })
}
```

- [ ] **Step 2: Run BTC pagination tests to verify RED**

Run:

```bash
cargo test -p coin-listener-chain-providers btc_client_builds_paginated_transaction_paths --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers btc_transaction_page_uses_last_txid_as_next_page_token --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers btc_status_errors_do_not_include_provider_url --manifest-path backend/Cargo.toml
```

Expected: FAIL because `address_txs_page_path`, `decode_btc_transaction_page`, and `format_btc_status_error` do not exist.

- [ ] **Step 3: Add BTC page model and path construction**

In `backend/crates/chain-providers/src/btc.rs`, add this struct after `BtcBalance`:

```rust
#[derive(Debug, Clone)]
pub struct BtcTransactionPage {
    pub transactions: Vec<BtcTransaction>,
    pub next_last_seen_txid: Option<String>,
}
```

Replace the existing `address_txs_path` method with these methods:

```rust
pub fn address_txs_path(&self, address: &str) -> AppResult<String> {
    self.address_txs_page_path(address, None)
}

pub fn address_txs_page_path(
    &self,
    address: &str,
    last_seen_txid: Option<&str>,
) -> AppResult<String> {
    let address = normalize_btc_address(address)?;
    match last_seen_txid {
        Some(txid) => {
            validate_btc_hash(txid, "last_seen_txid")?;
            Ok(format!("/address/{address}/txs/chain/{txid}"))
        }
        None => Ok(format!("/address/{address}/txs/chain")),
    }
}
```

- [ ] **Step 4: Add BTC page request and decode functions**

Replace the existing `address_transactions` method with these methods:

```rust
pub async fn address_transactions(&self, address: &str) -> AppResult<Vec<BtcTransaction>> {
    self.address_transactions_page(address, None)
        .await
        .map(|page| page.transactions)
}

pub async fn address_transactions_page(
    &self,
    address: &str,
    last_seen_txid: Option<&str>,
) -> AppResult<BtcTransactionPage> {
    let path = self.address_txs_page_path(address, last_seen_txid)?;
    let body = self.get_json_body("address transactions", &path).await?;
    decode_btc_transaction_page(body)
}
```

Add this public function after the `impl BtcClient` block:

```rust
pub fn decode_btc_transaction_page(body: Value) -> AppResult<BtcTransactionPage> {
    let transactions: Vec<BtcTransaction> = serde_json::from_value(body).map_err(|error| {
        AppError::Validation(format!(
            "invalid BTC address transactions response json: {error}"
        ))
    })?;
    let next_last_seen_txid = transactions.last().map(|transaction| transaction.txid.clone());
    if let Some(txid) = next_last_seen_txid.as_deref() {
        validate_btc_hash(txid, "last_seen_txid")?;
    }

    Ok(BtcTransactionPage {
        transactions,
        next_last_seen_txid,
    })
}
```

- [ ] **Step 5: Redact BTC non-2xx response bodies**

Replace this block in `get_json_body`:

```rust
if !status.is_success() {
    return Err(AppError::Config(format!(
        "BTC {operation} returned http {status}: {body}"
    )));
}
```

with:

```rust
if !status.is_success() {
    return Err(AppError::Config(format_btc_status_error(
        operation,
        &self.base_url,
        status,
        &body,
    )));
}
```

Add this function after `format_btc_request_error`:

```rust
pub fn format_btc_status_error(
    operation: &str,
    base_url: &str,
    status: reqwest::StatusCode,
    body: &str,
) -> String {
    format!(
        "BTC {operation} returned http {status}: {}",
        redact_provider_url(body, base_url)
    )
}
```

- [ ] **Step 6: Run BTC provider tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-chain-providers btc_client_builds_paginated_transaction_paths --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers btc_transaction_page_uses_last_txid_as_next_page_token --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers btc_transaction_page_without_transactions_has_no_next_token --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers btc_status_errors_do_not_include_provider_url --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers btc_ --manifest-path backend/Cargo.toml
cargo check -p coin-listener-chain-providers --manifest-path backend/Cargo.toml
cargo fmt -p coin-listener-chain-providers --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 7: Checkpoint**

Run:

```bash
git status --short
```

If git status succeeds, run:

```bash
git add backend/crates/chain-providers/src/btc.rs
git commit -m "Add BTC provider pagination primitives"
```

Expected outside a git repository: skip commit and report changed files.

---

### Task 2: Add TRON provider pagination primitives

**Files:**
- Modify: `backend/crates/chain-providers/src/tron.rs:11-420`
- Test: `backend/crates/chain-providers/src/tron.rs:544-760`

- [ ] **Step 1: Write failing TRON pagination tests**

In `backend/crates/chain-providers/src/tron.rs`, add these tests after `tron_client_builds_account_paths_without_double_slashes`:

```rust
#[test]
fn tron_parse_page_reads_data_and_fingerprint() {
    let page = super::parse_tron_page(
        json!({
            "data": [{ "transaction_id": tron_hash(1) }],
            "meta": { "fingerprint": "fingerprint-1" }
        }),
        "account transactions",
    )
    .unwrap();

    assert_eq!(page.data.len(), 1);
    assert_eq!(page.next_fingerprint, Some("fingerprint-1".to_string()));
}

#[test]
fn tron_parse_page_treats_missing_or_empty_fingerprint_as_last_page() {
    let missing = super::parse_tron_page(
        json!({ "data": [] }),
        "account transactions",
    )
    .unwrap();
    let empty = super::parse_tron_page(
        json!({ "data": [], "meta": { "fingerprint": "  " } }),
        "account transactions",
    )
    .unwrap();

    assert_eq!(missing.next_fingerprint, None);
    assert_eq!(empty.next_fingerprint, None);
}

#[test]
fn tron_account_queries_append_fingerprint_when_present() {
    let without_fingerprint = super::account_transactions_query(1_710_000_000_000, None);
    let with_fingerprint =
        super::account_transactions_query(1_710_000_000_000, Some(" fingerprint-2 "));

    assert!(!without_fingerprint
        .iter()
        .any(|(key, _)| *key == "fingerprint"));
    assert!(with_fingerprint.contains(&("fingerprint", "fingerprint-2".to_string())));
}

#[test]
fn tron_trc20_queries_include_contract_and_fingerprint() {
    let query = super::account_trc20_transfers_query(
        TOKEN_CONTRACT.to_string(),
        1_710_000_000_000,
        Some("fingerprint-3"),
    );

    assert!(query.contains(&("contract_address", TOKEN_CONTRACT.to_string())));
    assert!(query.contains(&("fingerprint", "fingerprint-3".to_string())));
}

#[test]
fn tron_status_errors_do_not_include_provider_url() {
    let message = super::format_tron_status_error(
        "TRC20 transfers",
        "https://api.trongrid.io/provider-key/",
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        "upstream https://api.trongrid.io/provider-key/ failed",
    );

    assert!(message.contains("TRC20 transfers"));
    assert!(message.contains("429 Too Many Requests"));
    assert!(message.contains("[redacted provider url]"));
    assert!(!message.contains("trongrid.io"));
    assert!(!message.contains("provider-key"));
}
```

- [ ] **Step 2: Run TRON pagination tests to verify RED**

Run:

```bash
cargo test -p coin-listener-chain-providers tron_parse_page_reads_data_and_fingerprint --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers tron_account_queries_append_fingerprint_when_present --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers tron_status_errors_do_not_include_provider_url --manifest-path backend/Cargo.toml
```

Expected: FAIL because `parse_tron_page`, `account_transactions_query`, `account_trc20_transfers_query`, and `format_tron_status_error` do not exist.

- [ ] **Step 3: Add TRON page model and query helpers**

In `backend/crates/chain-providers/src/tron.rs`, add this struct after `TronBalance`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TronPage {
    pub data: Vec<Value>,
    pub next_fingerprint: Option<String>,
}
```

Add these functions before `impl TronClient`:

```rust
pub fn account_transactions_query(
    min_timestamp: i64,
    fingerprint: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mut query = vec![
        ("only_confirmed", "true".to_string()),
        ("limit", "200".to_string()),
        ("min_timestamp", min_timestamp.to_string()),
    ];
    push_fingerprint(&mut query, fingerprint);
    query
}

pub fn account_trc20_transfers_query(
    contract_address: String,
    min_timestamp: i64,
    fingerprint: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mut query = vec![
        ("only_confirmed", "true".to_string()),
        ("limit", "200".to_string()),
        ("contract_address", contract_address),
        ("min_timestamp", min_timestamp.to_string()),
    ];
    push_fingerprint(&mut query, fingerprint);
    query
}

fn push_fingerprint(query: &mut Vec<(&'static str, String)>, fingerprint: Option<&str>) {
    if let Some(fingerprint) = fingerprint
        .map(str::trim)
        .filter(|fingerprint| !fingerprint.is_empty())
    {
        query.push(("fingerprint", fingerprint.to_string()));
    }
}
```

- [ ] **Step 4: Add TRON paged client methods**

Replace the existing `account_transactions` and `account_trc20_transfers` methods with these methods:

```rust
pub async fn account_transactions(
    &self,
    address: &str,
    min_timestamp: i64,
) -> AppResult<Vec<Value>> {
    self.account_transactions_page(address, min_timestamp, None)
        .await
        .map(|page| page.data)
}

pub async fn account_transactions_page(
    &self,
    address: &str,
    min_timestamp: i64,
    fingerprint: Option<&str>,
) -> AppResult<TronPage> {
    let path = self.account_transactions_path(address)?;
    let query = account_transactions_query(min_timestamp, fingerprint);
    let body = self
        .get_json_body("account transactions", &path, &query)
        .await?;
    parse_tron_page(body, "account transactions")
}

pub async fn account_trc20_transfers(
    &self,
    address: &str,
    contract_address: &str,
    min_timestamp: i64,
) -> AppResult<Vec<Value>> {
    self.account_trc20_transfers_page(address, contract_address, min_timestamp, None)
        .await
        .map(|page| page.data)
}

pub async fn account_trc20_transfers_page(
    &self,
    address: &str,
    contract_address: &str,
    min_timestamp: i64,
    fingerprint: Option<&str>,
) -> AppResult<TronPage> {
    let path = self.account_trc20_path(address)?;
    let contract_address = normalize_tron_address(contract_address)?;
    let query = account_trc20_transfers_query(contract_address, min_timestamp, fingerprint);
    let body = self
        .get_json_body("TRC20 transfers", &path, &query)
        .await?;
    parse_tron_page(body, "TRC20 transfers")
}
```

- [ ] **Step 5: Add TRON page parser and status redaction**

Replace `parse_data_array` with:

```rust
pub fn parse_data_array(body: Value, operation: &str) -> AppResult<Vec<Value>> {
    parse_tron_page(body, operation).map(|page| page.data)
}

pub fn parse_tron_page(body: Value, operation: &str) -> AppResult<TronPage> {
    let data = body
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| {
            AppError::Validation(format!("TRON {operation} response missing data array"))
        })?;
    let next_fingerprint = body
        .get("meta")
        .and_then(|meta| meta.get("fingerprint"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|fingerprint| !fingerprint.is_empty())
        .map(ToString::to_string);

    Ok(TronPage {
        data,
        next_fingerprint,
    })
}
```

Replace this block in `get_json_body`:

```rust
if !status.is_success() {
    return Err(AppError::Config(format!(
        "TRON {operation} returned http {status}: {body}"
    )));
}
```

with:

```rust
if !status.is_success() {
    return Err(AppError::Config(format_tron_status_error(
        operation,
        &self.base_url,
        status,
        &body,
    )));
}
```

Add this function after `format_tron_request_error`:

```rust
pub fn format_tron_status_error(
    operation: &str,
    base_url: &str,
    status: reqwest::StatusCode,
    body: &str,
) -> String {
    format!(
        "TRON {operation} returned http {status}: {}",
        redact_provider_url(body, base_url)
    )
}
```

- [ ] **Step 6: Run TRON provider tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-chain-providers tron_parse_page_reads_data_and_fingerprint --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers tron_parse_page_treats_missing_or_empty_fingerprint_as_last_page --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers tron_account_queries_append_fingerprint_when_present --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers tron_trc20_queries_include_contract_and_fingerprint --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers tron_status_errors_do_not_include_provider_url --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers tron_ --manifest-path backend/Cargo.toml
cargo check -p coin-listener-chain-providers --manifest-path backend/Cargo.toml
cargo fmt -p coin-listener-chain-providers --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 7: Checkpoint**

Run:

```bash
git status --short
```

If git status succeeds, run:

```bash
git add backend/crates/chain-providers/src/tron.rs
git commit -m "Add TRON provider pagination primitives"
```

Expected outside a git repository: skip commit and report changed files.

---

### Task 3: Add worker pagination helper functions

**Files:**
- Modify: `backend/crates/worker/src/lib.rs:31-160`
- Test: `backend/crates/worker/src/lib.rs:968-1002`

- [ ] **Step 1: Write failing worker helper tests**

In `backend/crates/worker/src/lib.rs`, update the import block in `mod btc_worker_helpers` from:

```rust
use crate::{btc_cursor_value, BTC_TRANSACTION_CURSOR};
```

to:

```rust
use chrono::{TimeZone, Utc};
use coin_listener_core::models::ScanCursor;
use uuid::Uuid;

use crate::{
    btc_cursor_value, btc_scan_from_block, ensure_provider_page_limit, paged_log_index,
    BTC_CURSOR_OVERLAP_BLOCKS, BTC_TRANSACTION_CURSOR, MAX_PROVIDER_PAGES_PER_SCAN,
};
```

Add these tests inside `mod btc_worker_helpers` after `btc_cursor_constant_is_stable`:

```rust
#[test]
fn btc_scan_from_block_reprocesses_last_scanned_block_for_overlap() {
    let cursor = scan_cursor(800_000);

    assert_eq!(BTC_CURSOR_OVERLAP_BLOCKS, 1);
    assert_eq!(btc_scan_from_block(Some(&cursor)), 800_000);
}

#[test]
fn btc_scan_from_block_clamps_overlap_to_zero() {
    let cursor = scan_cursor(0);

    assert_eq!(btc_scan_from_block(Some(&cursor)), 0);
    assert_eq!(btc_scan_from_block(None), 0);
}

#[test]
fn provider_page_limit_errors_when_next_page_remains_after_maximum() {
    let error = ensure_provider_page_limit(
        "BTC address transactions",
        MAX_PROVIDER_PAGES_PER_SCAN,
        true,
    )
    .unwrap_err();

    assert!(error.to_string().contains("BTC address transactions"));
    assert!(error.to_string().contains("pagination exceeded"));
}

#[test]
fn provider_page_limit_allows_last_page_at_maximum() {
    assert!(ensure_provider_page_limit(
        "BTC address transactions",
        MAX_PROVIDER_PAGES_PER_SCAN,
        false,
    )
    .is_ok());
}

#[test]
fn paged_log_index_offsets_items_by_page() {
    assert_eq!(paged_log_index(0, 7).unwrap(), 7);
    assert_eq!(paged_log_index(2, 7).unwrap(), 20_007);
}
```

Add this helper near the existing `transfer` helper in `mod btc_worker_helpers`:

```rust
fn scan_cursor(last_scanned_block: i64) -> ScanCursor {
    ScanCursor {
        id: Uuid::from_u128(1),
        tenant_id: Uuid::from_u128(2),
        chain_id: Uuid::from_u128(3),
        address_id: Uuid::from_u128(4),
        cursor_type: BTC_TRANSACTION_CURSOR.to_string(),
        last_scanned_block,
        updated_at: Utc.with_ymd_and_hms(2026, 5, 18, 12, 0, 0).unwrap(),
    }
}
```

- [ ] **Step 2: Run worker helper tests to verify RED**

Run:

```bash
cargo test -p worker btc_scan_from_block_reprocesses_last_scanned_block_for_overlap --manifest-path backend/Cargo.toml
cargo test -p worker provider_page_limit_errors_when_next_page_remains_after_maximum --manifest-path backend/Cargo.toml
cargo test -p worker paged_log_index_offsets_items_by_page --manifest-path backend/Cargo.toml
```

Expected: FAIL because `btc_scan_from_block`, `ensure_provider_page_limit`, `paged_log_index`, `BTC_CURSOR_OVERLAP_BLOCKS`, and `MAX_PROVIDER_PAGES_PER_SCAN` do not exist.

- [ ] **Step 3: Add worker pagination constants**

In `backend/crates/worker/src/lib.rs`, add these constants after `BTC_INITIAL_BLOCK_WINDOW`:

```rust
pub const MAX_PROVIDER_PAGES_PER_SCAN: usize = 10;
pub const BTC_CURSOR_OVERLAP_BLOCKS: i64 = 1;
pub const PROVIDER_PAGE_LOG_INDEX_STRIDE: usize = 10_000;
```

- [ ] **Step 4: Add worker helper functions**

Add these functions after `btc_cursor_value`:

```rust
pub fn btc_scan_from_block(cursor: Option<&ScanCursor>) -> i64 {
    cursor
        .map(|cursor| {
            cursor
                .last_scanned_block
                .saturating_sub(BTC_CURSOR_OVERLAP_BLOCKS.saturating_sub(1))
                .max(0)
        })
        .unwrap_or(0)
}

pub fn paged_log_index(page_index: usize, item_index: usize) -> AppResult<i32> {
    let value = page_index
        .checked_mul(PROVIDER_PAGE_LOG_INDEX_STRIDE)
        .and_then(|base| base.checked_add(item_index))
        .ok_or_else(|| AppError::Validation("provider page item index overflow".to_string()))?;
    i32::try_from(value)
        .map_err(|_| AppError::Validation("provider page item index overflow".to_string()))
}

pub fn ensure_provider_page_limit(
    label: &str,
    pages_processed: usize,
    has_next_page: bool,
) -> AppResult<()> {
    if has_next_page && pages_processed >= MAX_PROVIDER_PAGES_PER_SCAN {
        return Err(AppError::Config(format!(
            "{label} pagination exceeded max page limit {MAX_PROVIDER_PAGES_PER_SCAN}"
        )));
    }
    Ok(())
}
```

- [ ] **Step 5: Run worker helper tests to verify GREEN**

Run:

```bash
cargo test -p worker btc_scan_from_block_reprocesses_last_scanned_block_for_overlap --manifest-path backend/Cargo.toml
cargo test -p worker btc_scan_from_block_clamps_overlap_to_zero --manifest-path backend/Cargo.toml
cargo test -p worker provider_page_limit_errors_when_next_page_remains_after_maximum --manifest-path backend/Cargo.toml
cargo test -p worker provider_page_limit_allows_last_page_at_maximum --manifest-path backend/Cargo.toml
cargo test -p worker paged_log_index_offsets_items_by_page --manifest-path backend/Cargo.toml
cargo test -p worker btc_worker_helpers --manifest-path backend/Cargo.toml
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
git commit -m "Add worker pagination helpers"
```

Expected outside a git repository: skip commit and report changed files.

---

### Task 4: Wire BTC paginated scanning into worker

**Files:**
- Modify: `backend/crates/worker/src/lib.rs:454-522`
- Test: `backend/crates/chain-providers/src/btc.rs`, `backend/crates/worker/src/lib.rs`

- [ ] **Step 1: Run focused tests before wiring**

Run:

```bash
cargo test -p coin-listener-chain-providers btc_transaction_page_uses_last_txid_as_next_page_token --manifest-path backend/Cargo.toml
cargo test -p worker btc_scan_from_block_reprocesses_last_scanned_block_for_overlap --manifest-path backend/Cargo.toml
```

Expected: PASS. These tests prove the provider exposes next-page tokens and the worker uses an overlap-safe cursor start.

- [ ] **Step 2: Replace the BTC single-page transaction block**

In `scan_btc_address`, replace this block:

```rust
let cursor = repositories::scan_cursor(pool, context.id, BTC_TRANSACTION_CURSOR).await?;
let from_block = cursor
    .as_ref()
    .map(|cursor| cursor.last_scanned_block + 1)
    .unwrap_or(0);
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
```

with this paginated block:

```rust
let cursor = repositories::scan_cursor(pool, context.id, BTC_TRANSACTION_CURSOR).await?;
let from_block = btc_scan_from_block(cursor.as_ref());
let mut events = Vec::new();
let mut transfers = Vec::new();
let mut next_last_seen_txid: Option<String> = None;
let mut pages_processed = 0usize;

loop {
    let page = client
        .address_transactions_page(&context.address, next_last_seen_txid.as_deref())
        .await?;
    pages_processed += 1;
    let has_next_page = page.next_last_seen_txid.is_some();
    ensure_provider_page_limit("BTC address transactions", pages_processed, has_next_page)?;

    for tx in page.transactions {
        let Some(transfer) = btc::classify_btc_transaction(&tx, &context.address)? else {
            continue;
        };
        if transfer.block_number < from_block {
            continue;
        }
        transfers.push(transfer);
    }

    let Some(next) = page.next_last_seen_txid else {
        break;
    };
    next_last_seen_txid = Some(next);
}

for transfer in &transfers {
    let draft = btc::btc_transfer_event_draft(&context, &native_asset, transfer.clone());
    if let Some(event) = repositories::insert_event_if_not_exists(pool, draft).await? {
        events.push(event);
    }
}
```

Leave the existing cursor update block unchanged:

```rust
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
```

- [ ] **Step 3: Run BTC worker compile and regression tests**

Run:

```bash
cargo test -p worker btc_worker_helpers --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers btc_ --manifest-path backend/Cargo.toml
cargo check -p worker --manifest-path backend/Cargo.toml
cargo fmt -p worker -p coin-listener-chain-providers --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 4: Checkpoint**

Run:

```bash
git status --short
```

If git status succeeds, run:

```bash
git add backend/crates/worker/src/lib.rs backend/crates/chain-providers/src/btc.rs
git commit -m "Wire BTC paginated transaction scanning"
```

Expected outside a git repository: skip commit and report changed files.

---

### Task 5: Wire TRON paginated scanning into worker

**Files:**
- Modify: `backend/crates/worker/src/lib.rs:340-452`
- Test: `backend/crates/chain-providers/src/tron.rs`, `backend/crates/worker/src/lib.rs`

- [ ] **Step 1: Run focused tests before wiring**

Run:

```bash
cargo test -p coin-listener-chain-providers tron_parse_page_reads_data_and_fingerprint --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers tron_account_queries_append_fingerprint_when_present --manifest-path backend/Cargo.toml
cargo test -p worker provider_page_limit_errors_when_next_page_remains_after_maximum --manifest-path backend/Cargo.toml
cargo test -p worker paged_log_index_offsets_items_by_page --manifest-path backend/Cargo.toml
```

Expected: PASS. These tests prove the provider exposes fingerprint pages and the worker can cap page loops and generate page-stable fallback log indexes.

- [ ] **Step 2: Replace the TRX single-page block**

In `scan_tron_address`, replace this block:

```rust
let trx_cursor = repositories::scan_cursor(pool, context.id, TRON_TRX_TRANSFER_CURSOR).await?;
let trx_from = trx_cursor
    .as_ref()
    .map(|cursor| cursor.last_scanned_block + 1)
    .unwrap_or(0);
let trx_payloads = client
    .account_transactions(&context.address, trx_from)
    .await?;
let mut trx_transfers = Vec::with_capacity(trx_payloads.len());
for (index, payload) in trx_payloads.iter().enumerate() {
    match tron::try_decode_trx_transfer_at_index(
        payload,
        native_asset.decimals,
        i32::try_from(index)
            .map_err(|_| AppError::Validation("TRON transfer index overflow".to_string()))?,
    )? {
        tron::TrxTransferDecode::Transfer(transfer) => trx_transfers.push(transfer),
        tron::TrxTransferDecode::Skip => continue,
    }
}
```

with this paginated block:

```rust
let trx_cursor = repositories::scan_cursor(pool, context.id, TRON_TRX_TRANSFER_CURSOR).await?;
let trx_from = trx_cursor
    .as_ref()
    .map(|cursor| cursor.last_scanned_block + 1)
    .unwrap_or(0);
let mut trx_transfers = Vec::new();
let mut trx_fingerprint: Option<String> = None;
let mut trx_pages_processed = 0usize;

loop {
    let page = client
        .account_transactions_page(&context.address, trx_from, trx_fingerprint.as_deref())
        .await?;
    let page_index = trx_pages_processed;
    trx_pages_processed += 1;
    let tron::TronPage {
        data,
        next_fingerprint,
    } = page;
    let has_next_page = next_fingerprint.is_some();
    ensure_provider_page_limit("TRON account transactions", trx_pages_processed, has_next_page)?;

    for (index, payload) in data.iter().enumerate() {
        match tron::try_decode_trx_transfer_at_index(
            payload,
            native_asset.decimals,
            paged_log_index(page_index, index)?,
        )? {
            tron::TrxTransferDecode::Transfer(transfer) => trx_transfers.push(transfer),
            tron::TrxTransferDecode::Skip => continue,
        }
    }

    let Some(next) = next_fingerprint else {
        break;
    };
    trx_fingerprint = Some(next);
}
```

Leave the existing `trx_cursor_value`, event insert, and cursor update code unchanged.

- [ ] **Step 3: Replace the TRC20 single-page block**

In `scan_tron_address`, replace this block inside `for asset in trc20_assets`:

```rust
let payloads = client
    .account_trc20_transfers(&context.address, &contract_address, trc20_from)
    .await?;
for (index, payload) in payloads.into_iter().enumerate() {
    let transfer = tron::decode_trc20_transfer_at_index(
        &payload,
        &contract_address,
        asset.decimals,
        i32::try_from(index).map_err(|_| {
            AppError::Validation("TRON transfer index overflow".to_string())
        })?,
    )?;
    trc20_cursor_value =
        Some(trc20_cursor_value.map_or(transfer.cursor_value, |current| {
            current.max(transfer.cursor_value)
        }));
    if !tron_transfer_should_scan(&asset, &transfer) {
        continue;
    }
    let draft = tron::tron_transfer_event_draft(&context, &asset, transfer);
    if let Some(event) = repositories::insert_event_if_not_exists(pool, draft).await? {
        events.push(event);
    }
}
```

with this paginated block:

```rust
let mut trc20_fingerprint: Option<String> = None;
let mut trc20_pages_processed = 0usize;
let mut asset_transfers = Vec::new();

loop {
    let page = client
        .account_trc20_transfers_page(
            &context.address,
            &contract_address,
            trc20_from,
            trc20_fingerprint.as_deref(),
        )
        .await?;
    let page_index = trc20_pages_processed;
    trc20_pages_processed += 1;
    let tron::TronPage {
        data,
        next_fingerprint,
    } = page;
    let has_next_page = next_fingerprint.is_some();
    ensure_provider_page_limit("TRON TRC20 transfers", trc20_pages_processed, has_next_page)?;

    for (index, payload) in data.into_iter().enumerate() {
        let transfer = tron::decode_trc20_transfer_at_index(
            &payload,
            &contract_address,
            asset.decimals,
            paged_log_index(page_index, index)?,
        )?;
        trc20_cursor_value = Some(
            trc20_cursor_value.map_or(transfer.cursor_value, |current| {
                current.max(transfer.cursor_value)
            }),
        );
        if tron_transfer_should_scan(&asset, &transfer) {
            asset_transfers.push(transfer);
        }
    }

    let Some(next) = next_fingerprint else {
        break;
    };
    trc20_fingerprint = Some(next);
}

for transfer in asset_transfers {
    let draft = tron::tron_transfer_event_draft(&context, &asset, transfer);
    if let Some(event) = repositories::insert_event_if_not_exists(pool, draft).await? {
        events.push(event);
    }
}
```

The cursor update remains after all assets finish. If any asset page fails, the `?` exits before `upsert_scan_cursor`, preserving the address-level TRC20 cursor.

- [ ] **Step 4: Run TRON worker compile and regression tests**

Run:

```bash
cargo test -p worker tron_worker_helpers --manifest-path backend/Cargo.toml
cargo test -p worker btc_worker_helpers --manifest-path backend/Cargo.toml
cargo test -p coin-listener-chain-providers tron_ --manifest-path backend/Cargo.toml
cargo check -p worker --manifest-path backend/Cargo.toml
cargo fmt -p worker -p coin-listener-chain-providers --check --manifest-path backend/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 5: Checkpoint**

Run:

```bash
git status --short
```

If git status succeeds, run:

```bash
git add backend/crates/worker/src/lib.rs backend/crates/chain-providers/src/tron.rs
git commit -m "Wire TRON fingerprint pagination scanning"
```

Expected outside a git repository: skip commit and report changed files.

---

### Task 6: Run final verification

**Files:**
- Verify: full backend workspace, frontend build, Docker Compose config

- [ ] **Step 1: Run backend format check**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: exit 0.

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

Expected: exit 0 with all Rust crate tests passing.

- [ ] **Step 4: Run frontend build regression**

Run:

```bash
npm run build --prefix frontend
```

Expected: exit 0. Existing lottie/chunk warnings may remain, but no build failure is allowed.

- [ ] **Step 5: Validate Docker Compose config**

Run:

```bash
docker compose -f docker-compose.yml config
```

Expected: exit 0.

- [ ] **Step 6: Final checkpoint**

Run:

```bash
git status --short
```

If git status succeeds, run:

```bash
git add backend/crates/chain-providers/src/btc.rs backend/crates/chain-providers/src/tron.rs backend/crates/worker/src/lib.rs
git commit -m "Add BTC and TRON pagination reliability"
```

Expected outside a git repository: skip commit and report changed files.

---

## Self-Review Checklist

Spec coverage:

- BTC Esplora `/txs/chain/:last_seen_txid` support: Task 1 and Task 4.
- TRON `meta.fingerprint` parsing and query support: Task 2 and Task 5.
- Worker page loops for BTC, TRX, and TRC20: Task 4 and Task 5.
- Max page limit with no cursor advancement on overflow: Task 3, Task 4, and Task 5.
- BTC cursor overlap without schema changes: Task 3 and Task 4.
- TRC20 address-level cursor only after all assets finish: Task 5.
- Provider non-2xx body redaction: Task 1 and Task 2.
- Full regression verification: Task 6.

No database migrations are required. No frontend code changes are required.
