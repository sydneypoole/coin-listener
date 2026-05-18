# Coin Listener Real EVM RPC Design

## Goal

Replace the worker's EVM mock scan path with a minimal real EVM JSON-RPC scan loop that records native balance snapshots and emits balance-change events when the watched address balance changes.

## Scope

Included:

- Use lightweight HTTP JSON-RPC for EVM providers stored in the existing `providers` table.
- Support `eth_blockNumber` and `eth_getBalance` for native asset balance scanning.
- Select the highest-priority active provider for the watched address chain.
- Insert a `balance_snapshots` row for every successful native balance scan.
- Compare the new snapshot with the previous snapshot for the same address and native asset.
- Insert an `address_events` balance-change event only when the balance changes.
- Enqueue notification work only when a new event is created.
- Keep the existing mock EVM dev scan route as an explicit development tool.

Excluded:

- WebSocket subscriptions.
- Transaction-list scanning.
- ERC20 `Transfer` log scanning.
- Provider failover across multiple providers.
- Frontend changes.
- Falling back to mock data when real RPC fails.

## Architecture

The worker remains the orchestration boundary for scheduled scans. Storage owns database reads and writes, `chain-providers` owns EVM JSON-RPC request/response handling, and the API server keeps the current dev-only mock scan endpoint unchanged.

The first real EVM slice is intentionally native-balance-only. This creates a verifiable live RPC loop without introducing transaction pagination, log filtering, or WebSocket lifecycle complexity.

## Data Flow

For each `ScanAddressTask` whose chain type is `evm`:

1. Load the watched address scan context from PostgreSQL.
2. Load the chain's native asset.
3. Select the active RPC provider for `address.chain_id` ordered by `priority ASC, name ASC`.
4. Create an EVM RPC client from `provider.base_url` and `provider.timeout_ms`.
5. Call `eth_blockNumber`.
6. Call `eth_getBalance(address.address, "latest")`.
7. Parse the returned hex quantity into a raw decimal string.
8. Format the native-asset decimal balance using the asset decimals.
9. Insert a `balance_snapshots` row with:
   - `tenant_id`
   - `chain_id`
   - `address_id`
   - `asset_id`
   - `balance_raw`
   - `balance_decimal`
   - `block_number`
   - `source_provider_id`
10. Load the previous snapshot for the same `address_id` and `asset_id`, excluding the row just inserted.
11. If there is no previous snapshot, finish the scan without creating an event.
12. If the raw balance is unchanged, finish the scan without creating an event.
13. If the raw balance changed, insert an `address_events` row with event type `balance_change` and enqueue a notify task for that event.
14. Finish the address scan and release the queue lock.

## Failure Behavior

Real RPC failures are surfaced as scan failures. The worker does not silently convert them into successful scans.

- No active RPC provider for the chain returns `AppError::NotFound` from the storage lookup.
- HTTP errors, network errors, and request timeouts return `AppError::Config` with enough context to identify the provider and method.
- Invalid JSON-RPC response shapes and JSON-RPC error payloads return `AppError::Validation` with enough context to identify the provider and method.
- Hex quantity parsing failures return `AppError::Validation`.
- Database failures return `AppError::Database`.
- A failed scan does not call `finish_address_scan` and does not enqueue notification work.
- The existing mock EVM event generator remains available only through the explicit dev API route guarded by `enable_dev_routes`.

## Component Boundaries

### `backend/crates/chain-providers/src/evm.rs`

Add a lightweight EVM RPC client and pure conversion helpers:

```rust
pub struct EvmRpcClient {
    base_url: String,
    timeout: std::time::Duration,
}

pub enum EvmBlockTag {
    Latest,
}

pub struct EvmBalance {
    pub block_number: i64,
    pub balance_raw: String,
    pub balance_decimal: String,
}

impl EvmRpcClient {
    pub fn new(base_url: String, timeout: std::time::Duration) -> Self;
    pub async fn eth_block_number(&self) -> AppResult<i64>;
    pub async fn eth_get_balance(&self, address: &str, block: EvmBlockTag) -> AppResult<String>;
}

pub fn parse_hex_quantity_to_i64(hex: &str) -> AppResult<i64>;
pub fn parse_hex_u256_to_decimal_string(hex: &str) -> AppResult<String>;
pub fn wei_to_decimal_string(raw: &str, decimals: i32) -> AppResult<String>;
```

The module should not depend on worker internals or database types beyond shared core models and errors.

### `backend/crates/storage/src/repositories.rs`

Add storage helpers for the worker:

```rust
pub async fn active_rpc_provider_for_chain(pool: &PgPool, chain_id: Uuid) -> AppResult<Provider>;
pub async fn latest_balance_snapshot(
    pool: &PgPool,
    address_id: Uuid,
    asset_id: Uuid,
    before_snapshot_id: Option<Uuid>,
) -> AppResult<Option<BalanceSnapshot>>;
pub async fn insert_balance_snapshot(
    pool: &PgPool,
    draft: CreateBalanceSnapshotRequest,
) -> AppResult<BalanceSnapshot>;
```

If the existing core model set does not contain a suitable create request, add a focused `CreateBalanceSnapshotRequest` model in `core::models` rather than passing loose parameters.

### `backend/crates/worker/src/lib.rs`

Replace the scheduled `MockEvm` branch with a real balance scan branch:

```rust
pub enum ScanPlan {
    EvmNativeBalance,
    Unsupported(String),
}

pub async fn scan_evm_native_balance(
    pool: &PgPool,
    task: &ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<Option<AddressEvent>>;
```

`run_worker` should enqueue notify work only when `scan_evm_native_balance` returns `Some(event)`.

### `backend/crates/api-server/src/routes.rs`

No behavior change is required. The dev scan route continues using `create_mock_evm_event` and remains hidden unless `enable_dev_routes` is true.

## Event Semantics

The first successful snapshot for an address/native asset establishes a baseline and does not create an event. Subsequent successful scans create a `balance_change` event only when `balance_raw` differs from the previous snapshot.

The event should preserve the current `AddressEvent` shape and include enough metadata to distinguish the old and new balance, provider, and observed block number in the existing JSON metadata field.

## Testing Strategy

Backend tests should cover behavior without requiring a live public RPC endpoint.

- Unit-test JSON-RPC request body construction and response parsing in `chain-providers`.
- Unit-test hex quantity parsing:
  - `0x0` -> `0`
  - `0x1` -> `1`
  - `0xde0b6b3a7640000` -> `1000000000000000000`
  - invalid hex returns validation error.
- Unit-test decimal formatting for native asset decimals.
- Repository tests should assert SQL query strings or behavior for:
  - active provider ordering and filtering.
  - latest snapshot lookup excluding the newly inserted snapshot.
  - snapshot insert field mapping.
- Worker tests should verify:
  - EVM chain type maps to `EvmNativeBalance`.
  - first snapshot produces no notification event.
  - unchanged balance produces no notification event.
  - changed balance produces one event and one notify enqueue.
  - RPC/provider errors return failure and do not finish the address scan.
- Existing API dev route tests should remain unchanged.

## Acceptance Criteria

- Scheduled worker scans for EVM watched addresses call real provider JSON-RPC instead of generating mock events.
- A successful scan always records a native balance snapshot.
- Balance-change events are emitted only after a baseline snapshot exists and the raw native balance changes.
- Notification queue entries are created only for emitted balance-change events.
- RPC/provider failures are visible as scan failures and never fall back to mock data.
- Existing mock dev scan route still works only when dev routes are enabled.
- Backend formatting, checks, tests, and frontend build remain green after implementation.
