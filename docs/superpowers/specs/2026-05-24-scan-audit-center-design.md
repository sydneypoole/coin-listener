# Scan Audit Center Design

## Goal

Build a scan audit center that records each worker scan attempt, exposes searchable scan history, supports failed-scan retry, and summarizes scan health in the existing system status page.

## Current Context

The current system status endpoint aggregates scan health from `watched_addresses` only. `backend/crates/storage/src/system_status.rs` reports active, due, overdue, and latest scanned timestamps, while `frontend/src/pages/SystemStatusPage.tsx` shows those aggregate values. The worker already emits structured scan logs with `task_id`, `address_id`, `scan_status`, and error details, but there is no persisted scan-attempt history that the UI can query.

## Scope

The first version implements a complete audit center, not a general-purpose scan orchestration platform. It includes persisted scan runs, list/detail APIs, failure retry, system status summaries, and a frontend audit page. It does not change scan cursor semantics, event insertion semantics, provider failover rules, or notification outbox behavior.

## Data Model

Add a `scan_runs` table through a new storage migration.

Fields:

| Field | Purpose |
| --- | --- |
| `id UUID PRIMARY KEY` | Scan run identifier |
| `tenant_id UUID NOT NULL` | Tenant scope |
| `task_id UUID NOT NULL` | Queue task that triggered the attempt |
| `address_id UUID NOT NULL` | Watched address |
| `chain_id UUID NOT NULL` | Chain scanned |
| `chain_type TEXT NOT NULL` | `evm`, `tron`, `utxo`, or unsupported type |
| `status TEXT NOT NULL` | `running`, `success`, `failed`, `locked`, `unsupported` |
| `event_count INTEGER NOT NULL DEFAULT 0` | New events inserted by this attempt |
| `started_at TIMESTAMPTZ NOT NULL` | Attempt start time |
| `finished_at TIMESTAMPTZ` | Attempt finish time |
| `duration_ms BIGINT` | Finish minus start in milliseconds |
| `error_message TEXT` | Failure detail for `failed` status |
| `metadata JSONB NOT NULL DEFAULT '{}'::jsonb` | Extensible runtime facts such as outcome and provider context |
| `created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()` | Row creation time |
| `updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()` | Row update time |

Indexes:

- `(tenant_id, started_at DESC)` for default history view.
- `(tenant_id, status, started_at DESC)` for status filtering.
- `(address_id, started_at DESC)` for address drill-down.
- `(task_id)` for queue-task lookup.

Status meaning:

| Status | Meaning |
| --- | --- |
| `running` | Worker created the record and scan has not completed yet |
| `success` | Worker completed a supported scan path |
| `failed` | Worker returned an error |
| `locked` | Worker skipped because another worker owns the lock |
| `unsupported` | Worker handled the task but the chain type is unsupported |

## Worker Behavior

`run_worker` creates a scan run record when a task is dequeued, then updates it when `process_scan_task` returns.

Lifecycle:

1. Dequeue scan task.
2. Load scan context needed for tenant, chain, and address metadata.
3. Insert `scan_runs` row with `running` status and `started_at`.
4. Call `process_scan_task`.
5. On `ScanTaskOutcome::Scanned`, update status to `success`, store `event_count`, `finished_at`, and `duration_ms`.
6. On `ScanTaskOutcome::Locked`, update status to `locked` with zero events.
7. On `ScanTaskOutcome::UnsupportedChain`, update status to `unsupported` with chain type in metadata.
8. On error, update status to `failed` with `error_message`.
9. Continue existing structured logs and add `scan_run_id` to success/failure log records.

The worker must not move scan cursors differently, enqueue notifications differently, or create events differently. The audit write is observational. If the audit update fails after a scan completes, the worker logs the audit error but does not change scan result semantics.

## API

Add protected APIs using the existing auth context.

### `GET /api/scan-runs`

Returns paginated scan history scoped to the authenticated tenant.

Query parameters:

| Parameter | Behavior |
| --- | --- |
| `chain_id` | Optional UUID filter |
| `address_id` | Optional UUID filter |
| `status` | Optional scan status filter |
| `started_after` | Optional ISO timestamp lower bound |
| `started_before` | Optional ISO timestamp upper bound |
| `limit` | Page size, default 50, max 100 |
| `offset` | Offset, default 0 |

Response contains `items`, `limit`, and `offset`. Each item includes chain/address display fields so the frontend does not need extra joins for table rendering.

### `GET /api/scan-runs/:id`

Returns one scan run scoped to the authenticated tenant. It includes full metadata and error message.

### `POST /api/scan-runs/:id/retry`

Retries a failed scan run by enqueueing a new `ScanAddressTask` for the same address. Only `failed` and `unsupported` runs are retryable in the first version. `locked`, `running`, and `success` are rejected with validation errors. The retry endpoint returns the newly enqueued task payload.

### `GET /api/system/status` extension

Extend `ScanStatus` with:

| Field | Purpose |
| --- | --- |
| `last_success_at` | Latest successful scan finish time |
| `last_failed_at` | Latest failed scan finish time |
| `last_24h_success` | Count of successful scan runs in last 24 hours |
| `last_24h_failed` | Count of failed scan runs in last 24 hours |
| `recent_runs` | Most recent 5 scan runs for dashboard display |

## Frontend

Add a new page named `ScanAuditPage` and a navigation item named `扫描审计`.

The page contains:

- Filter panel for chain, watched address, status, and time range.
- Data table with time, chain, address, status, duration, event count, error summary, and actions.
- Chinese status tags:
  - `running` → `扫描中`
  - `success` → `成功`
  - `failed` → `失败`
  - `locked` → `跳过：锁占用`
  - `unsupported` → `不支持`
- Detail action that shows full metadata and error message.
- Retry action for failed and unsupported rows.

The existing system status page adds scan summary cards for recent success/failure counts and a compact recent-runs table.

## Error Handling

- Invalid query UUIDs and timestamps return validation errors.
- Unknown scan statuses return validation errors.
- Detail and retry endpoints return not found when the row is outside the authenticated tenant.
- Retry rejects non-retryable statuses with a clear validation message.
- Worker audit-write failures are logged but do not cause a successfully processed scan to be reported as failed.

## Testing

Use TDD for implementation.

Required tests:

| Layer | Tests |
| --- | --- |
| Core | DTO JSON round-trip for scan run list/detail/status extension |
| Storage | Migration contains required table, indexes, status fields, and tenant-scoped queries |
| Worker | Scan success, failure, locked, and unsupported outcomes update scan run status |
| API | Routes exist, tenant scoping is enforced, filters validate, retry accepts only retryable statuses |
| Frontend | API contracts exist, navigation contains `扫描审计`, table uses Chinese statuses, retry action only appears for retryable rows |

## Rollout

1. Add data model and storage helpers.
2. Record scan runs from the worker.
3. Expose list/detail/retry APIs and system status summary fields.
4. Add frontend audit page and system status summary.
5. Verify with focused backend and frontend tests before any completion claim.

## Non-Goals

- No automatic retry policy engine.
- No alert routing from scan failures.
- No change to provider failover or circuit breaker rules.
- No change to scan cursor movement or event idempotency rules.
