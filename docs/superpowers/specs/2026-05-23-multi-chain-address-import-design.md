# Multi-Chain Address Import Design

## Goal

Batch watched-address import should support listening to the same imported address list on multiple chains in one task.

## Current behavior

- Single address creation already supports adding one address to multiple chains from the frontend by issuing one create request per chain configuration.
- Batch import creates one backend import task with one `chain_id` and one `asset_ids` list.
- The worker processes each import row once and creates exactly one watched address using the task-level chain and assets.
- Import progress and error rows are tied to source address rows, not expanded address-chain work items.

## Chosen approach

Use one backend import task with multiple task-level chain configurations.

The user selects chain configurations once in the batch import dialog. The imported address rows are expanded by the worker across all selected chain configurations.

Example:

```text
100 imported addresses x 3 chain configurations = 300 watched-address create attempts
```

This keeps one task, one progress view, one cancel action, and one error list.

## Backend contract

### Request shape

Extend batch import defaults with `chain_configs`:

```rust
pub struct WatchedAddressImportChainConfig {
    pub chain_id: Uuid,
    pub asset_ids: Vec<Uuid>,
}

pub struct WatchedAddressImportDefaults {
    pub chain_id: Uuid,
    pub asset_ids: Vec<Uuid>,
    pub chain_configs: Vec<WatchedAddressImportChainConfig>,
    pub priority: String,
    pub scan_interval_seconds: i32,
    pub transfer_filter_enabled: bool,
    pub balance_change_filter_enabled: bool,
    pub status: String,
}
```

Compatibility rule:

- New frontend requests send `chain_configs`.
- If an older client omits `chain_configs`, backend derives a single config from existing `chain_id` and `asset_ids`.
- Validation requires at least one effective chain config.
- Every config must have a non-empty `asset_ids` list.
- Duplicate `chain_id` values in one import are rejected.

### Storage

Add `chain_configs JSONB NOT NULL DEFAULT '[]'::jsonb` to `watched_address_import_tasks` through a new migration.

Continue returning legacy `chain_id` and `asset_ids` fields for response compatibility. For new multi-chain tasks, these fields mirror the first chain config.

`WatchedAddressImportTask` adds:

```rust
pub chain_configs: Vec<WatchedAddressImportChainConfig>
```

### Work items and progress

Do not create a second task per chain. The worker expands rows inside the same task.

The total unit of work becomes an address-chain attempt, not only a raw input row:

```text
total_rows = import_rows.len() * chain_configs.len()
processed_rows = success_rows + failed_rows + skipped_rows, counted per address-chain attempt
```

To track per-chain attempts, add an import-attempt table:

```text
watched_address_import_attempts
- id
- import_task_id
- tenant_id
- row_number
- chain_id
- asset_ids
- status: pending | success | failed | skipped
- watched_address_id
- error_code
- error_message
- created_at
- updated_at
```

Create attempts in the same transaction as the task and source rows. Source rows remain useful for original input and row-level metadata; attempts become the worker queue and error/progress source.

### Worker behavior

Worker flow changes from row-based to attempt-based:

1. Claim one pending/running import task.
2. Fetch pending attempts for that task.
3. Join attempt back to source row metadata.
4. Build `CreateWatchedAddressRequest` with:
   - `address`, `label`, and optional row overrides from the source row.
   - `chain_id` and `asset_ids` from the attempt.
   - priority, interval, filters, and status from row override or task defaults.
5. Mark each attempt success or failed independently.
6. Refresh task counts from attempts.
7. Complete when no pending attempts remain.

A failure on one chain does not block other chains for the same address.

Cancellation marks all pending attempts as skipped and recomputes counts.

## Error reporting

`WatchedAddressImportErrorRow` should include chain context:

```rust
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

The error list shows one row per failed address-chain attempt.

## Frontend behavior

Batch import modal replaces single default chain/asset fields with a chain configuration list, matching the create-address form pattern:

- Add chain config.
- Remove chain config.
- Select chain.
- Select assets for that chain.
- Require at least one chain config and at least one asset per config.
- Reset selected assets when a chain changes.
- Reject duplicate chains before creating a task.

Preview should show:

- valid imported address count.
- selected chain count.
- estimated create attempts: `validAddressCount * chainConfigCount`.

The request sends:

```ts
defaults: {
  chain_id: firstConfig.chain_id,
  asset_ids: firstConfig.asset_ids,
  chain_configs: chainConfigs,
  priority,
  scan_interval_seconds,
  transfer_filter_enabled,
  balance_change_filter_enabled,
  status,
}
```

Keep `chain_id` and `asset_ids` for backend compatibility while using `chain_configs` as the source of truth.

The import progress panel labels total/success/failure as address-chain attempts, not raw address rows.

The error table adds a chain column.

## CSV parsing

Do not add per-row chain fields in this change.

Supported CSV fields remain address-level metadata:

- `address`
- `label`
- `priority`
- `scan_interval_seconds`
- `transfer_filter_enabled`
- `balance_change_filter_enabled`
- `status`

Every valid row is applied to every selected chain config.

## Non-goals

- No per-row chain or per-row asset override.
- No separate import task per chain.
- No changes to scan scheduling semantics after watched addresses are created.
- No frontend-only bulk API loop for large imports.

## Testing strategy

Backend:

- Model serialization tests cover `chain_configs` request and response shape.
- Storage/source tests verify migrations create `watched_address_import_attempts` and `chain_configs`.
- Import validation tests cover empty configs, empty assets, and duplicate chains.
- Worker/source tests verify import processing uses pending attempts and creates one watched address per address-chain attempt.
- Error list query tests verify chain context is selected.

Frontend:

- Parser tests remain address-only and confirm unknown chain fields are still warnings.
- UI regression verifies batch import uses chain config rows, sends `chain_configs`, calculates estimated attempts, and shows chain in error table.

## Acceptance criteria

- Batch import can select multiple chains with chain-specific assets.
- One created import task represents the whole multi-chain import.
- Worker creates one watched address per valid address-chain combination.
- Progress counts address-chain attempts.
- Failed chain attempts are visible without hiding successes for other chains.
- Existing single-chain import clients remain compatible.
- Relevant backend tests, frontend UI regression, and frontend build pass.
