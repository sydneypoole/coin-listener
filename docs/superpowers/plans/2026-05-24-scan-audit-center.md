# Scan Audit Center Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a scan audit center that records worker scan attempts, exposes searchable history/detail/retry APIs, and displays scan health in the frontend.

**Architecture:** Persist scan attempts in a new PostgreSQL `scan_runs` table, record lifecycle updates from the worker, expose tenant-scoped API endpoints through the API server, and render a new React scan audit page plus system status summaries. Audit writes are observational and must not change scan cursor, event insertion, provider failover, or notification outbox behavior.

**Tech Stack:** Rust 2021, Axum, SQLx, PostgreSQL, Redis, Tokio, React, TypeScript, TanStack Query, Semi UI.

---

## Scope and Constraints

Implement only the approved scope from `docs/superpowers/specs/2026-05-24-scan-audit-center-design.md`.

In scope:

- Persist scan attempts in `scan_runs`.
- Record worker outcomes `running`, `success`, `failed`, `locked`, and `unsupported`.
- Expose tenant-scoped list, detail, and retry APIs.
- Extend `/api/system/status` scan summary with success/failure counts and recent runs.
- Add frontend API contracts, a `扫描审计` page, Chinese scan status labels, detail modal, and retry action for retryable rows.

Out of scope:

- Automatic retry policy engine.
- Alert routing from scan failures.
- Provider failover changes.
- Scan cursor movement changes.
- Event insertion or notification outbox behavior changes.

Preserve unrelated working-tree changes. This plan adds no new frontend dependencies.

---

## File Structure

Create:

```text
backend/crates/storage/migrations/0019_scan_runs.sql
backend/crates/storage/src/scan_runs.rs
frontend/src/pages/ScanAuditPage.tsx
```

Modify:

```text
backend/crates/core/src/models.rs
backend/crates/storage/src/lib.rs
backend/crates/storage/src/system_status.rs
backend/crates/worker/Cargo.toml
backend/crates/worker/src/lib.rs
backend/crates/api-server/src/routes.rs
frontend/src/api/types.ts
frontend/src/api/client.ts
frontend/src/App.tsx
frontend/src/pages/SystemStatusPage.tsx
frontend/src/ui-regression.test.ts
```

Responsibilities:

- `backend/crates/core/src/models.rs`: shared scan run DTOs, query types, retry response, and `ScanStatus` extension.
- `backend/crates/storage/migrations/0019_scan_runs.sql`: additive `scan_runs` table and indexes.
- `backend/crates/storage/src/scan_runs.rs`: scan run status validation, pagination helpers, insert/update/list/detail/retry/system summary storage helpers, and source-level query tests.
- `backend/crates/storage/src/lib.rs`: export the `scan_runs` module.
- `backend/crates/storage/src/system_status.rs`: attach scan run health summary and recent runs to existing scan status output.
- `backend/crates/worker/Cargo.toml`: add `serde_json` for audit metadata construction.
- `backend/crates/worker/src/lib.rs`: extend scan outcomes with event counts and record scan run lifecycle without changing scan semantics.
- `backend/crates/api-server/src/routes.rs`: protected scan run list/detail/retry routes and retry queue enqueue.
- `frontend/src/api/types.ts`: TypeScript scan audit DTOs and `ScanStatus` extension.
- `frontend/src/api/client.ts`: typed scan audit API helpers.
- `frontend/src/App.tsx`: `扫描审计` navigation entry and page routing.
- `frontend/src/pages/ScanAuditPage.tsx`: filter panel, table, Chinese status tags, detail modal, and retry action.
- `frontend/src/pages/SystemStatusPage.tsx`: scan health cards and compact recent-runs table.
- `frontend/src/ui-regression.test.ts`: frontend source-level regression coverage for contracts, navigation, Chinese labels, retry visibility, and DataTable usage.

---

### Task 1: Add scan audit core DTOs

**Files:**
- Modify: `backend/crates/core/src/models.rs`
- Test: `backend/crates/core/src/models.rs` existing `#[cfg(test)] mod tests`

- [ ] **Step 1: Write failing DTO serde tests**

In `backend/crates/core/src/models.rs`, extend the `use super::{ ... }` import list inside the test module to include scan audit DTOs:

```rust
use super::{
    AddressEvent, CreateBalanceSnapshotRequest, CreateTelegramBindingRequest,
    CreateTelegramBotRequest, CreateWatchedAddressImportRequest, CreateWatchedAddressRequest,
    EventStatus, EvmTransactionRescanRequest, EvmTransactionRescanResponse,
    EvmTransactionRescanSummary, EvmTransactionRescanTransferSummary, NotificationDelivery,
    NotificationDeliveryListItem, NotificationDeliveryListResponse, NotificationDeliveryQuery,
    NotificationOutboxDetail, NotificationOutboxItem, NotificationOutboxListItem,
    NotificationOutboxListResponse, NotificationOutboxQuery, NotificationStatus,
    NotifyEventTask, OutboxStatusCounts, ProviderChainStatus, ProviderHealthStatus,
    ProviderStatus, ProviderStatusItem, QueueStatus, RetryNotificationOutboxResponse,
    RetryScanRunResponse, ScanAddressTask, ScanCursor, ScanRun, ScanRunDetail,
    ScanRunListItem, ScanRunListResponse, ScanRunQuery, ScanStatus, ServiceHealthStatus,
    ServiceHeartbeatStatusItem, SystemStatus, TelegramBindingRequest, UpdateTelegramBotRequest,
    WatchedAddressImportChainConfig, WatchedAddressImportDefaults,
    WatchedAddressImportErrorRow, WatchedAddressImportTask, WatchedAddressResponse,
};
```

Append these tests after `system_status_round_trips_as_json`:

```rust
#[test]
fn scan_run_responses_round_trip_as_json() {
    let started_at = Utc.with_ymd_and_hms(2026, 5, 24, 8, 0, 0).unwrap();
    let finished_at = Utc.with_ymd_and_hms(2026, 5, 24, 8, 0, 2).unwrap();
    let list_item = ScanRunListItem {
        id: Uuid::from_u128(1),
        tenant_id: Uuid::from_u128(2),
        task_id: Uuid::from_u128(3),
        address_id: Uuid::from_u128(4),
        chain_id: Uuid::from_u128(5),
        chain_name: "Base".to_string(),
        address: "0x0000000000000000000000000000000000000001".to_string(),
        address_label: Some("Hot wallet".to_string()),
        chain_type: "evm".to_string(),
        status: "failed".to_string(),
        event_count: 0,
        started_at,
        finished_at: Some(finished_at),
        duration_ms: Some(2_000),
        error_message: Some("provider request failed: timeout".to_string()),
    };
    let detail = ScanRunDetail {
        id: list_item.id,
        tenant_id: list_item.tenant_id,
        task_id: list_item.task_id,
        address_id: list_item.address_id,
        chain_id: list_item.chain_id,
        chain_name: list_item.chain_name.clone(),
        address: list_item.address.clone(),
        address_label: list_item.address_label.clone(),
        chain_type: list_item.chain_type.clone(),
        status: list_item.status.clone(),
        event_count: list_item.event_count,
        started_at,
        finished_at: Some(finished_at),
        duration_ms: Some(2_000),
        error_message: list_item.error_message.clone(),
        metadata: serde_json::json!({ "outcome": "failed", "provider": "base-rpc" }),
        created_at: started_at,
        updated_at: finished_at,
    };
    let list = ScanRunListResponse {
        items: vec![list_item.clone()],
        limit: 50,
        offset: 0,
    };
    let retry = RetryScanRunResponse {
        task: ScanAddressTask {
            task_id: Uuid::from_u128(6),
            address_id: list_item.address_id,
            tenant_id: list_item.tenant_id,
            chain_id: list_item.chain_id,
            attempt: 1,
            enqueued_at: finished_at,
        },
    };

    let payload = serde_json::to_string(&(list, detail, retry)).expect("serialize scan run responses");
    let decoded: (ScanRunListResponse, ScanRunDetail, RetryScanRunResponse) =
        serde_json::from_str(&payload).expect("deserialize scan run responses");

    assert_eq!(decoded.0.items[0], list_item);
    assert_eq!(decoded.1.metadata["outcome"], "failed");
    assert_eq!(decoded.2.task.address_id, Uuid::from_u128(4));
    assert!(payload.contains("\"chain_name\":\"Base\""));
    assert!(payload.contains("\"duration_ms\":2000"));
}

#[test]
fn scan_run_query_deserializes_filters() {
    let query: ScanRunQuery = serde_json::from_str(
        r#"{
            "chain_id":"00000000-0000-0000-0000-000000000005",
            "address_id":"00000000-0000-0000-0000-000000000004",
            "status":"failed",
            "started_after":"2026-05-24T00:00:00Z",
            "started_before":"2026-05-25T00:00:00Z",
            "limit":25,
            "offset":50
        }"#,
    )
    .expect("deserialize scan run query");

    assert_eq!(query.chain_id, Some(Uuid::from_u128(5)));
    assert_eq!(query.address_id, Some(Uuid::from_u128(4)));
    assert_eq!(query.status.as_deref(), Some("failed"));
    assert_eq!(query.limit, Some(25));
    assert_eq!(query.offset, Some(50));
}

#[test]
fn scan_status_round_trips_scan_run_health() {
    let run_time = Utc.with_ymd_and_hms(2026, 5, 24, 8, 0, 0).unwrap();
    let status = ScanStatus {
        active_addresses: 12,
        due_addresses: 2,
        overdue_addresses: 1,
        last_scanned_at: Some(run_time),
        last_success_at: Some(run_time),
        last_failed_at: Some(run_time),
        last_24h_success: 7,
        last_24h_failed: 3,
        recent_runs: vec![ScanRunListItem {
            id: Uuid::from_u128(1),
            tenant_id: Uuid::from_u128(2),
            task_id: Uuid::from_u128(3),
            address_id: Uuid::from_u128(4),
            chain_id: Uuid::from_u128(5),
            chain_name: "Base".to_string(),
            address: "0x0000000000000000000000000000000000000001".to_string(),
            address_label: None,
            chain_type: "evm".to_string(),
            status: "success".to_string(),
            event_count: 2,
            started_at: run_time,
            finished_at: Some(run_time),
            duration_ms: Some(350),
            error_message: None,
        }],
    };

    let payload = serde_json::to_string(&status).expect("serialize scan status");
    let decoded: ScanStatus = serde_json::from_str(&payload).expect("deserialize scan status");

    assert_eq!(decoded, status);
    assert!(payload.contains("\"last_24h_success\":7"));
    assert!(payload.contains("\"recent_runs\""));
}
```

Also update the existing `system_status_round_trips_as_json` test so its `ScanStatus` literal includes the new fields:

```rust
scans: ScanStatus {
    active_addresses: 12,
    due_addresses: 2,
    overdue_addresses: 1,
    last_scanned_at: Some(Utc.with_ymd_and_hms(2026, 5, 17, 9, 58, 0).unwrap()),
    last_success_at: Some(Utc.with_ymd_and_hms(2026, 5, 17, 9, 58, 0).unwrap()),
    last_failed_at: None,
    last_24h_success: 3,
    last_24h_failed: 0,
    recent_runs: vec![],
},
```

- [ ] **Step 2: Run the core tests and verify they fail**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-core scan_run -- --nocapture
```

Expected: FAIL with unresolved imports or missing fields for `ScanRun`, `ScanRunListItem`, `ScanRunDetail`, `ScanRunQuery`, `ScanRunListResponse`, `RetryScanRunResponse`, and the new `ScanStatus` fields.

- [ ] **Step 3: Add the DTO structs**

In `backend/crates/core/src/models.rs`, insert these structs immediately after `ScanStatus`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct ScanRun {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub task_id: Uuid,
    pub address_id: Uuid,
    pub chain_id: Uuid,
    pub chain_type: String,
    pub status: String,
    pub event_count: i32,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<i64>,
    pub error_message: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct ScanRunListItem {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub task_id: Uuid,
    pub address_id: Uuid,
    pub chain_id: Uuid,
    pub chain_name: String,
    pub address: String,
    pub address_label: Option<String>,
    pub chain_type: String,
    pub status: String,
    pub event_count: i32,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<i64>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct ScanRunDetail {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub task_id: Uuid,
    pub address_id: Uuid,
    pub chain_id: Uuid,
    pub chain_name: String,
    pub address: String,
    pub address_label: Option<String>,
    pub chain_type: String,
    pub status: String,
    pub event_count: i32,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<i64>,
    pub error_message: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScanRunQuery {
    pub chain_id: Option<Uuid>,
    pub address_id: Option<Uuid>,
    pub status: Option<String>,
    pub started_after: Option<DateTime<Utc>>,
    pub started_before: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanRunListResponse {
    pub items: Vec<ScanRunListItem>,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryScanRunResponse {
    pub task: ScanAddressTask,
}
```

Then replace the existing `ScanStatus` struct with:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanStatus {
    pub active_addresses: i64,
    pub due_addresses: i64,
    pub overdue_addresses: i64,
    pub last_scanned_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failed_at: Option<DateTime<Utc>>,
    pub last_24h_success: i64,
    pub last_24h_failed: i64,
    pub recent_runs: Vec<ScanRunListItem>,
}
```

- [ ] **Step 4: Run the core tests and verify they pass**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-core scan_run -- --nocapture
```

Expected: PASS for the three scan-run tests.

- [ ] **Step 5: Run all core model tests**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-core
```

Expected: PASS.

- [ ] **Step 6: Commit Task 1**

Run:

```bash
git add backend/crates/core/src/models.rs
git commit -m "$(cat <<'EOF'
添加扫描审计核心 DTO
EOF
)"
```

---

### Task 2: Add scan run migration and storage helpers

**Files:**
- Create: `backend/crates/storage/migrations/0019_scan_runs.sql`
- Create: `backend/crates/storage/src/scan_runs.rs`
- Modify: `backend/crates/storage/src/lib.rs`
- Test: `backend/crates/storage/src/scan_runs.rs`

- [ ] **Step 1: Write failing storage tests**

Create `backend/crates/storage/src/scan_runs.rs` with this test-first skeleton:

```rust
use chrono::{DateTime, Utc};
use coin_listener_core::{
    models::{ScanAddressContext, ScanAddressTask, ScanRun, ScanRunDetail, ScanRunListItem, ScanRunQuery},
    AppError, AppResult,
};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

pub const SCAN_RUN_STATUS_RUNNING: &str = "running";
pub const SCAN_RUN_STATUS_SUCCESS: &str = "success";
pub const SCAN_RUN_STATUS_FAILED: &str = "failed";
pub const SCAN_RUN_STATUS_LOCKED: &str = "locked";
pub const SCAN_RUN_STATUS_UNSUPPORTED: &str = "unsupported";

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn scan_runs_migration_defines_table_statuses_and_indexes() {
        let migration = include_str!("../migrations/0019_scan_runs.sql");

        assert!(migration.contains("CREATE TABLE IF NOT EXISTS scan_runs"));
        for field in [
            "id UUID PRIMARY KEY",
            "tenant_id UUID NOT NULL",
            "task_id UUID NOT NULL",
            "address_id UUID NOT NULL",
            "chain_id UUID NOT NULL",
            "chain_type TEXT NOT NULL",
            "status TEXT NOT NULL",
            "event_count INTEGER NOT NULL DEFAULT 0",
            "started_at TIMESTAMPTZ NOT NULL",
            "finished_at TIMESTAMPTZ",
            "duration_ms BIGINT",
            "error_message TEXT",
            "metadata JSONB NOT NULL DEFAULT '{}'::jsonb",
            "created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()",
            "updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()",
        ] {
            assert!(migration.contains(field), "missing migration field {field}");
        }

        for status in ["running", "success", "failed", "locked", "unsupported"] {
            assert!(migration.contains(status), "missing status {status}");
        }

        assert!(migration.contains("idx_scan_runs_tenant_started_at"));
        assert!(migration.contains("ON scan_runs(tenant_id, started_at DESC)"));
        assert!(migration.contains("idx_scan_runs_tenant_status_started_at"));
        assert!(migration.contains("ON scan_runs(tenant_id, status, started_at DESC)"));
        assert!(migration.contains("idx_scan_runs_address_started_at"));
        assert!(migration.contains("ON scan_runs(address_id, started_at DESC)"));
        assert!(migration.contains("idx_scan_runs_task_id"));
        assert!(migration.contains("ON scan_runs(task_id)"));
    }

    #[test]
    fn scan_run_status_validation_and_retryability_are_explicit() {
        for status in ["running", "success", "failed", "locked", "unsupported"] {
            assert!(validate_scan_run_status(status).is_ok(), "status {status}");
        }
        assert!(validate_scan_run_status("unknown").is_err());
        assert!(scan_run_status_allows_retry("failed"));
        assert!(scan_run_status_allows_retry("unsupported"));
        assert!(!scan_run_status_allows_retry("running"));
        assert!(!scan_run_status_allows_retry("success"));
        assert!(!scan_run_status_allows_retry("locked"));
    }

    #[test]
    fn scan_run_pagination_defaults_and_clamps() {
        assert_eq!(scan_runs_limit(None), 50);
        assert_eq!(scan_runs_limit(Some(0)), 1);
        assert_eq!(scan_runs_limit(Some(100)), 100);
        assert_eq!(scan_runs_limit(Some(500)), 100);
        assert_eq!(scan_runs_offset(None), 0);
        assert_eq!(scan_runs_offset(Some(-10)), 0);
        assert_eq!(scan_runs_offset(Some(25)), 25);
    }

    #[test]
    fn scan_run_queries_are_tenant_scoped_and_filterable() {
        assert!(LIST_SCAN_RUNS_QUERY.contains("sr.tenant_id = $1"));
        assert!(LIST_SCAN_RUNS_QUERY.contains("$2::uuid IS NULL OR sr.chain_id = $2"));
        assert!(LIST_SCAN_RUNS_QUERY.contains("$3::uuid IS NULL OR sr.address_id = $3"));
        assert!(LIST_SCAN_RUNS_QUERY.contains("$4::text IS NULL OR sr.status = $4"));
        assert!(LIST_SCAN_RUNS_QUERY.contains("$5::timestamptz IS NULL OR sr.started_at >= $5"));
        assert!(LIST_SCAN_RUNS_QUERY.contains("$6::timestamptz IS NULL OR sr.started_at <= $6"));
        assert!(LIST_SCAN_RUNS_QUERY.contains("JOIN chains c"));
        assert!(LIST_SCAN_RUNS_QUERY.contains("JOIN watched_addresses wa"));
        assert!(GET_SCAN_RUN_DETAIL_QUERY.contains("sr.tenant_id = $1"));
        assert!(GET_SCAN_RUN_DETAIL_QUERY.contains("sr.id = $2"));
        assert!(SELECT_RETRY_SCAN_RUN_QUERY.contains("sr.tenant_id = $2"));
    }

    #[test]
    fn scan_run_completion_query_sets_duration_and_merges_metadata() {
        assert!(FINISH_SCAN_RUN_QUERY.contains("finished_at = $4"));
        assert!(FINISH_SCAN_RUN_QUERY.contains("duration_ms"));
        assert!(FINISH_SCAN_RUN_QUERY.contains("EXTRACT(EPOCH FROM ($4::timestamptz - started_at))"));
        assert!(FINISH_SCAN_RUN_QUERY.contains("metadata = scan_runs.metadata || $6::jsonb"));
        assert!(FINISH_SCAN_RUN_QUERY.contains("updated_at = NOW()"));
    }

    #[test]
    fn retry_scan_run_task_uses_new_task_id_and_attempt_one() {
        let now = Utc.with_ymd_and_hms(2026, 5, 24, 8, 0, 0).unwrap();
        let task = build_retry_scan_task(
            Uuid::from_u128(2),
            Uuid::from_u128(4),
            Uuid::from_u128(5),
            now,
        );

        assert_ne!(task.task_id, Uuid::nil());
        assert_eq!(task.tenant_id, Uuid::from_u128(2));
        assert_eq!(task.address_id, Uuid::from_u128(4));
        assert_eq!(task.chain_id, Uuid::from_u128(5));
        assert_eq!(task.attempt, 1);
        assert_eq!(task.enqueued_at, now);
    }
}
```

Add `pub mod scan_runs;` to `backend/crates/storage/src/lib.rs` so the new module compiles.

- [ ] **Step 2: Run storage scan run tests and verify they fail**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage scan_runs -- --nocapture
```

Expected: FAIL because `0019_scan_runs.sql`, query constants, validation helpers, pagination helpers, and retry task builder are missing.

- [ ] **Step 3: Create the migration**

Create `backend/crates/storage/migrations/0019_scan_runs.sql` with:

```sql
CREATE TABLE IF NOT EXISTS scan_runs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    task_id UUID NOT NULL,
    address_id UUID NOT NULL REFERENCES watched_addresses(id) ON DELETE CASCADE,
    chain_id UUID NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
    chain_type TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('running', 'success', 'failed', 'locked', 'unsupported')),
    event_count INTEGER NOT NULL DEFAULT 0 CHECK (event_count >= 0),
    started_at TIMESTAMPTZ NOT NULL,
    finished_at TIMESTAMPTZ,
    duration_ms BIGINT CHECK (duration_ms IS NULL OR duration_ms >= 0),
    error_message TEXT,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (finished_at IS NULL OR finished_at >= started_at)
);

CREATE INDEX IF NOT EXISTS idx_scan_runs_tenant_started_at
    ON scan_runs(tenant_id, started_at DESC);

CREATE INDEX IF NOT EXISTS idx_scan_runs_tenant_status_started_at
    ON scan_runs(tenant_id, status, started_at DESC);

CREATE INDEX IF NOT EXISTS idx_scan_runs_address_started_at
    ON scan_runs(address_id, started_at DESC);

CREATE INDEX IF NOT EXISTS idx_scan_runs_task_id
    ON scan_runs(task_id);
```

- [ ] **Step 4: Implement storage query constants and helpers**

Replace the skeleton in `backend/crates/storage/src/scan_runs.rs` above the tests with:

```rust
use chrono::{DateTime, Utc};
use coin_listener_core::{
    models::{
        ScanAddressContext, ScanAddressTask, ScanRun, ScanRunDetail, ScanRunListItem,
        ScanRunQuery,
    },
    AppError, AppResult,
};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

pub const SCAN_RUN_STATUS_RUNNING: &str = "running";
pub const SCAN_RUN_STATUS_SUCCESS: &str = "success";
pub const SCAN_RUN_STATUS_FAILED: &str = "failed";
pub const SCAN_RUN_STATUS_LOCKED: &str = "locked";
pub const SCAN_RUN_STATUS_UNSUPPORTED: &str = "unsupported";

pub const INSERT_SCAN_RUN_QUERY: &str = r#"
INSERT INTO scan_runs (
    tenant_id, task_id, address_id, chain_id, chain_type, status, started_at, metadata
)
VALUES ($1, $2, $3, $4, $5, 'running', $6, $7)
RETURNING id, tenant_id, task_id, address_id, chain_id, chain_type, status,
          event_count, started_at, finished_at, duration_ms, error_message,
          metadata, created_at, updated_at
"#;

pub const FINISH_SCAN_RUN_QUERY: &str = r#"
UPDATE scan_runs
SET status = $2,
    event_count = $3,
    finished_at = $4,
    duration_ms = GREATEST(0::double precision, EXTRACT(EPOCH FROM ($4::timestamptz - started_at)) * 1000)::BIGINT,
    error_message = $5,
    metadata = scan_runs.metadata || $6::jsonb,
    updated_at = NOW()
WHERE id = $1
RETURNING id, tenant_id, task_id, address_id, chain_id, chain_type, status,
          event_count, started_at, finished_at, duration_ms, error_message,
          metadata, created_at, updated_at
"#;

pub const LIST_SCAN_RUNS_QUERY: &str = r#"
SELECT sr.id,
       sr.tenant_id,
       sr.task_id,
       sr.address_id,
       sr.chain_id,
       c.name AS chain_name,
       wa.address,
       wa.label AS address_label,
       sr.chain_type,
       sr.status,
       sr.event_count,
       sr.started_at,
       sr.finished_at,
       sr.duration_ms,
       sr.error_message
FROM scan_runs sr
JOIN chains c ON c.id = sr.chain_id
JOIN watched_addresses wa ON wa.id = sr.address_id
WHERE sr.tenant_id = $1
  AND ($2::uuid IS NULL OR sr.chain_id = $2)
  AND ($3::uuid IS NULL OR sr.address_id = $3)
  AND ($4::text IS NULL OR sr.status = $4)
  AND ($5::timestamptz IS NULL OR sr.started_at >= $5)
  AND ($6::timestamptz IS NULL OR sr.started_at <= $6)
ORDER BY sr.started_at DESC
LIMIT $7 OFFSET $8
"#;

pub const GET_SCAN_RUN_DETAIL_QUERY: &str = r#"
SELECT sr.id,
       sr.tenant_id,
       sr.task_id,
       sr.address_id,
       sr.chain_id,
       c.name AS chain_name,
       wa.address,
       wa.label AS address_label,
       sr.chain_type,
       sr.status,
       sr.event_count,
       sr.started_at,
       sr.finished_at,
       sr.duration_ms,
       sr.error_message,
       sr.metadata,
       sr.created_at,
       sr.updated_at
FROM scan_runs sr
JOIN chains c ON c.id = sr.chain_id
JOIN watched_addresses wa ON wa.id = sr.address_id
WHERE sr.tenant_id = $1
  AND sr.id = $2
"#;

pub const SELECT_RETRY_SCAN_RUN_QUERY: &str = r#"
SELECT sr.tenant_id,
       sr.address_id,
       sr.chain_id,
       sr.status
FROM scan_runs sr
WHERE sr.id = $1
  AND sr.tenant_id = $2
"#;

pub const SCAN_RUN_CONTEXT_QUERY: &str = r#"
SELECT wa.id,
       wa.tenant_id,
       wa.chain_id,
       wa.address,
       wa.scan_interval_seconds,
       c.chain_type
FROM watched_addresses wa
JOIN chains c ON c.id = wa.chain_id
WHERE wa.id = $1
  AND wa.tenant_id = $2
  AND wa.chain_id = $3
"#;

pub const SCAN_RUN_HEALTH_SUMMARY_QUERY: &str = r#"
SELECT MAX(finished_at) FILTER (WHERE status = 'success') AS last_success_at,
       MAX(finished_at) FILTER (WHERE status = 'failed') AS last_failed_at,
       COUNT(*) FILTER (
           WHERE status = 'success'
             AND finished_at >= NOW() - INTERVAL '24 hours'
       ) AS last_24h_success,
       COUNT(*) FILTER (
           WHERE status = 'failed'
             AND finished_at >= NOW() - INTERVAL '24 hours'
       ) AS last_24h_failed
FROM scan_runs
"#;

pub const RECENT_SCAN_RUNS_QUERY: &str = r#"
SELECT sr.id,
       sr.tenant_id,
       sr.task_id,
       sr.address_id,
       sr.chain_id,
       c.name AS chain_name,
       wa.address,
       wa.label AS address_label,
       sr.chain_type,
       sr.status,
       sr.event_count,
       sr.started_at,
       sr.finished_at,
       sr.duration_ms,
       sr.error_message
FROM scan_runs sr
JOIN chains c ON c.id = sr.chain_id
JOIN watched_addresses wa ON wa.id = sr.address_id
ORDER BY sr.started_at DESC
LIMIT $1
"#;

#[derive(Debug, FromRow)]
pub struct ScanRunHealthSummary {
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failed_at: Option<DateTime<Utc>>,
    pub last_24h_success: i64,
    pub last_24h_failed: i64,
}

#[derive(Debug, FromRow)]
struct RetryScanRunRow {
    tenant_id: Uuid,
    address_id: Uuid,
    chain_id: Uuid,
    status: String,
}

pub fn validate_scan_run_status(status: &str) -> AppResult<()> {
    if !matches!(
        status,
        SCAN_RUN_STATUS_RUNNING
            | SCAN_RUN_STATUS_SUCCESS
            | SCAN_RUN_STATUS_FAILED
            | SCAN_RUN_STATUS_LOCKED
            | SCAN_RUN_STATUS_UNSUPPORTED
    ) {
        return Err(AppError::Validation(
            "scan run status must be running, success, failed, locked, or unsupported".to_string(),
        ));
    }
    Ok(())
}

pub fn scan_run_status_allows_retry(status: &str) -> bool {
    matches!(status, SCAN_RUN_STATUS_FAILED | SCAN_RUN_STATUS_UNSUPPORTED)
}

pub fn scan_runs_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(50).clamp(1, 100)
}

pub fn scan_runs_offset(offset: Option<i64>) -> i64 {
    offset.unwrap_or(0).max(0)
}

pub fn build_retry_scan_task(
    tenant_id: Uuid,
    address_id: Uuid,
    chain_id: Uuid,
    now: DateTime<Utc>,
) -> ScanAddressTask {
    ScanAddressTask {
        task_id: Uuid::new_v4(),
        address_id,
        tenant_id,
        chain_id,
        attempt: 1,
        enqueued_at: now,
    }
}

pub async fn scan_run_context(
    pool: &PgPool,
    task: &ScanAddressTask,
) -> AppResult<ScanAddressContext> {
    sqlx::query_as::<_, ScanAddressContext>(SCAN_RUN_CONTEXT_QUERY)
        .bind(task.address_id)
        .bind(task.tenant_id)
        .bind(task.chain_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("watched address".to_string()))
}

pub async fn create_scan_run(
    pool: &PgPool,
    task: &ScanAddressTask,
    context: &ScanAddressContext,
    started_at: DateTime<Utc>,
    metadata: serde_json::Value,
) -> AppResult<ScanRun> {
    sqlx::query_as::<_, ScanRun>(INSERT_SCAN_RUN_QUERY)
        .bind(context.tenant_id)
        .bind(task.task_id)
        .bind(context.id)
        .bind(context.chain_id)
        .bind(&context.chain_type)
        .bind(started_at)
        .bind(metadata)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn finish_scan_run(
    pool: &PgPool,
    scan_run_id: Uuid,
    status: &str,
    event_count: i32,
    finished_at: DateTime<Utc>,
    error_message: Option<&str>,
    metadata: serde_json::Value,
) -> AppResult<ScanRun> {
    validate_scan_run_status(status)?;
    sqlx::query_as::<_, ScanRun>(FINISH_SCAN_RUN_QUERY)
        .bind(scan_run_id)
        .bind(status)
        .bind(event_count)
        .bind(finished_at)
        .bind(error_message)
        .bind(metadata)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("scan run".to_string()))
}

pub async fn list_scan_runs(
    pool: &PgPool,
    tenant_id: Uuid,
    query: ScanRunQuery,
) -> AppResult<Vec<ScanRunListItem>> {
    if let Some(status) = query.status.as_deref() {
        validate_scan_run_status(status)?;
    }

    sqlx::query_as::<_, ScanRunListItem>(LIST_SCAN_RUNS_QUERY)
        .bind(tenant_id)
        .bind(query.chain_id)
        .bind(query.address_id)
        .bind(query.status)
        .bind(query.started_after)
        .bind(query.started_before)
        .bind(scan_runs_limit(query.limit))
        .bind(scan_runs_offset(query.offset))
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn get_scan_run_detail(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
) -> AppResult<ScanRunDetail> {
    sqlx::query_as::<_, ScanRunDetail>(GET_SCAN_RUN_DETAIL_QUERY)
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("scan run".to_string()))
}

pub async fn retry_scan_run_task(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
    now: DateTime<Utc>,
) -> AppResult<ScanAddressTask> {
    let row = sqlx::query_as::<_, RetryScanRunRow>(SELECT_RETRY_SCAN_RUN_QUERY)
        .bind(id)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .ok_or_else(|| AppError::NotFound("scan run".to_string()))?;

    if !scan_run_status_allows_retry(&row.status) {
        return Err(AppError::Validation(
            "only failed or unsupported scan runs can be retried".to_string(),
        ));
    }

    Ok(build_retry_scan_task(
        row.tenant_id,
        row.address_id,
        row.chain_id,
        now,
    ))
}

pub async fn scan_run_health_summary(pool: &PgPool) -> AppResult<ScanRunHealthSummary> {
    sqlx::query_as::<_, ScanRunHealthSummary>(SCAN_RUN_HEALTH_SUMMARY_QUERY)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn recent_scan_runs(pool: &PgPool, limit: i64) -> AppResult<Vec<ScanRunListItem>> {
    sqlx::query_as::<_, ScanRunListItem>(RECENT_SCAN_RUNS_QUERY)
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}
```

Keep the tests written in Step 1 at the bottom of this file.

- [ ] **Step 5: Run storage scan run tests and verify they pass**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage scan_runs -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Run all storage tests**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage
```

Expected: PASS.

- [ ] **Step 7: Commit Task 2**

Run:

```bash
git add backend/crates/storage/migrations/0019_scan_runs.sql backend/crates/storage/src/scan_runs.rs backend/crates/storage/src/lib.rs
git commit -m "$(cat <<'EOF'
添加扫描审计存储模型
EOF
)"
```

---

### Task 3: Extend system scan status from scan runs

**Files:**
- Modify: `backend/crates/storage/src/system_status.rs`
- Test: `backend/crates/storage/src/system_status.rs`

- [ ] **Step 1: Write failing system status tests**

In `backend/crates/storage/src/system_status.rs`, update the test imports to include the scan run summary constants:

```rust
use crate::{
    scan_runs::{RECENT_SCAN_RUNS_QUERY, SCAN_RUN_HEALTH_SUMMARY_QUERY},
    service_heartbeats::SERVICE_HEARTBEAT_STALE_SECONDS,
    system_status::{
        EVENT_STATUS_QUERY, NOTIFICATION_STATUS_QUERY, NOTIFICATION_STATUS_STALE_MINUTES,
        PROVIDER_CHAIN_STATUS_QUERY, PROVIDER_ITEMS_QUERY, PROVIDER_STATUS_DEFAULT_FAILURES,
        SCAN_STATUS_QUERY,
    },
};
```

Append these tests after `scan_status_query_counts_active_due_and_overdue_addresses`:

```rust
#[test]
fn scan_status_uses_scan_run_health_summary() {
    assert!(SCAN_RUN_HEALTH_SUMMARY_QUERY.contains("status = 'success'"));
    assert!(SCAN_RUN_HEALTH_SUMMARY_QUERY.contains("status = 'failed'"));
    assert!(SCAN_RUN_HEALTH_SUMMARY_QUERY.contains("last_success_at"));
    assert!(SCAN_RUN_HEALTH_SUMMARY_QUERY.contains("last_failed_at"));
    assert!(SCAN_RUN_HEALTH_SUMMARY_QUERY.contains("last_24h_success"));
    assert!(SCAN_RUN_HEALTH_SUMMARY_QUERY.contains("last_24h_failed"));
}

#[test]
fn scan_status_recent_runs_are_compact_and_limited() {
    assert!(RECENT_SCAN_RUNS_QUERY.contains("ORDER BY sr.started_at DESC"));
    assert!(RECENT_SCAN_RUNS_QUERY.contains("LIMIT $1"));
    assert!(RECENT_SCAN_RUNS_QUERY.contains("JOIN chains c"));
    assert!(RECENT_SCAN_RUNS_QUERY.contains("JOIN watched_addresses wa"));
}

#[test]
fn system_scan_status_attaches_recent_five_scan_runs() {
    let source = include_str!("system_status.rs")
        .split("#[cfg(test)]")
        .next()
        .expect("production source is present");

    assert!(source.contains("scan_runs::scan_run_health_summary(pool).await?"));
    assert!(source.contains("scan_runs::recent_scan_runs(pool, 5).await?"));
    assert!(source.contains("last_success_at: summary.last_success_at"));
    assert!(source.contains("last_failed_at: summary.last_failed_at"));
    assert!(source.contains("last_24h_success: summary.last_24h_success"));
    assert!(source.contains("last_24h_failed: summary.last_24h_failed"));
    assert!(source.contains("recent_runs"));
}
```

- [ ] **Step 2: Run the targeted system status tests and verify they fail**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage scan_status -- --nocapture
```

Expected: FAIL because `system_scan_status` does not yet call scan run summary helpers and `ScanStatus` construction lacks the new fields.

- [ ] **Step 3: Implement the scan status extension**

At the top of `backend/crates/storage/src/system_status.rs`, change:

```rust
use crate::repositories;
```

to:

```rust
use crate::{repositories, scan_runs};
```

Replace `system_scan_status` with:

```rust
pub async fn system_scan_status(pool: &PgPool) -> AppResult<ScanStatus> {
    let row = sqlx::query_as::<_, ScanStatusRow>(SCAN_STATUS_QUERY)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let summary = scan_runs::scan_run_health_summary(pool).await?;
    let recent_runs = scan_runs::recent_scan_runs(pool, 5).await?;

    Ok(ScanStatus {
        active_addresses: row.active_addresses,
        due_addresses: row.due_addresses,
        overdue_addresses: row.overdue_addresses,
        last_scanned_at: row.last_scanned_at,
        last_success_at: summary.last_success_at,
        last_failed_at: summary.last_failed_at,
        last_24h_success: summary.last_24h_success,
        last_24h_failed: summary.last_24h_failed,
        recent_runs,
    })
}
```

- [ ] **Step 4: Run the targeted system status tests and verify they pass**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage scan_status -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Run storage tests**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage
```

Expected: PASS.

- [ ] **Step 6: Commit Task 3**

Run:

```bash
git add backend/crates/storage/src/system_status.rs
git commit -m "$(cat <<'EOF'
扩展扫描状态汇总
EOF
)"
```

---

### Task 4: Record scan run lifecycle in the worker

**Files:**
- Modify: `backend/crates/worker/Cargo.toml`
- Modify: `backend/crates/worker/src/lib.rs`
- Test: `backend/crates/worker/src/lib.rs`

- [ ] **Step 1: Write failing worker audit tests**

In `backend/crates/worker/src/lib.rs`, update the existing scan task logging tests to the new `Scanned` shape and add scan-run lifecycle assertions:

```rust
mod scan_task_logging {
    use crate::{
        scan_task_event_count, scan_task_outcome_log_status, scan_task_update_metadata,
        ScanTaskOutcome,
    };

    #[test]
    fn scan_outcomes_have_explicit_log_statuses() {
        assert_eq!(
            scan_task_outcome_log_status(&ScanTaskOutcome::Scanned { event_count: 2 }),
            "success"
        );
        assert_eq!(
            scan_task_outcome_log_status(&ScanTaskOutcome::Locked),
            "locked"
        );
        assert_eq!(
            scan_task_outcome_log_status(&ScanTaskOutcome::UnsupportedChain(
                "solana".to_string()
            )),
            "unsupported"
        );
    }

    #[test]
    fn scan_outcomes_expose_event_counts_for_audit() {
        assert_eq!(
            scan_task_event_count(&ScanTaskOutcome::Scanned { event_count: 2 }),
            2
        );
        assert_eq!(scan_task_event_count(&ScanTaskOutcome::Locked), 0);
        assert_eq!(
            scan_task_event_count(&ScanTaskOutcome::UnsupportedChain("solana".to_string())),
            0
        );
    }

    #[test]
    fn scan_outcome_metadata_records_audit_outcome() {
        assert_eq!(
            scan_task_update_metadata(&ScanTaskOutcome::Scanned { event_count: 2 })["outcome"],
            "scanned"
        );
        assert_eq!(
            scan_task_update_metadata(&ScanTaskOutcome::Locked)["outcome"],
            "locked"
        );
        assert_eq!(
            scan_task_update_metadata(&ScanTaskOutcome::UnsupportedChain("solana".to_string()))
                ["chain_type"],
            "solana"
        );
    }

    #[test]
    fn worker_logs_explicit_scan_success_and_failure_status() {
        let source = include_str!("lib.rs");
        let start = source
            .find("pub async fn run_worker(")
            .expect("worker loop exists");
        let end = source[start..]
            .find("\n#[cfg(test)]")
            .expect("test module marker")
            + start;
        let worker = &source[start..end];

        assert!(worker.contains("scan_status = scan_task_outcome_log_status(&outcome)"));
        assert!(worker.contains("scan_status = \"failed\""));
        assert!(worker.contains("scan_run_id = ?scan_run_id"));
        assert!(worker.contains("\"scan task processed\""));
        assert!(worker.contains("\"scan task failed\""));
        assert!(!worker.contains("\"scan task succeeded\""));
    }

    #[test]
    fn worker_creates_and_finishes_scan_run_audit_records() {
        let source = include_str!("lib.rs");
        let start = source
            .find("pub async fn run_worker(")
            .expect("worker loop exists");
        let end = source[start..]
            .find("\n#[cfg(test)]")
            .expect("test module marker")
            + start;
        let worker = &source[start..end];

        assert!(worker.contains("create_scan_run_for_task"));
        assert!(worker.contains("finish_scan_run_for_result"));
        assert!(worker.contains("failed to create scan run audit"));
        assert!(worker.contains("failed to update scan run audit"));
    }

    #[test]
    fn process_locked_scan_task_reports_inserted_event_count() {
        let source = include_str!("lib.rs");
        let start = source
            .find("async fn process_locked_scan_task")
            .expect("process_locked_scan_task exists");
        let end = source[start..]
            .find("pub async fn run_worker")
            .expect("run_worker follows process_locked_scan_task")
            + start;
        let function = &source[start..end];

        assert!(function.contains("let events = scan_evm_address"));
        assert!(function.contains("let events = scan_tron_address"));
        assert!(function.contains("let events = scan_btc_address"));
        assert!(function.contains("ScanTaskOutcome::Scanned {"));
        assert!(function.contains("event_count: events.len()"));
    }
}
```

Update any existing test literals from `ScanTaskOutcome::Scanned` to `ScanTaskOutcome::Scanned { event_count: 0 }`.

- [ ] **Step 2: Run worker scan audit tests and verify they fail**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p worker scan_task_logging -- --nocapture
```

Expected: FAIL because the worker has no event-count outcome, no scan run metadata helper, and no scan run audit lifecycle calls.

- [ ] **Step 3: Add `serde_json` to the worker crate**

In `backend/crates/worker/Cargo.toml`, add:

```toml
serde_json.workspace = true
```

- [ ] **Step 4: Implement event-count outcomes and audit helpers**

In `backend/crates/worker/src/lib.rs`, update imports:

```rust
use coin_listener_storage::{
    address_imports,
    provider_health::{
        active_rpc_provider_candidates, record_provider_failure, record_provider_success,
        try_acquire_provider_qps,
    },
    repositories, scan_runs,
    scan_queue::ScanQueue,
};
```

Replace the `ScanTaskOutcome` enum with:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanTaskOutcome {
    Locked,
    Scanned { event_count: usize },
    UnsupportedChain(String),
}
```

Replace `scan_task_outcome_log_status` with:

```rust
pub fn scan_task_outcome_log_status(outcome: &ScanTaskOutcome) -> &'static str {
    match outcome {
        ScanTaskOutcome::Locked => "locked",
        ScanTaskOutcome::Scanned { .. } => "success",
        ScanTaskOutcome::UnsupportedChain(_) => "unsupported",
    }
}
```

Insert these helpers after `scan_task_outcome_log_status`:

```rust
pub fn scan_task_event_count(outcome: &ScanTaskOutcome) -> i32 {
    match outcome {
        ScanTaskOutcome::Scanned { event_count } => (*event_count).min(i32::MAX as usize) as i32,
        ScanTaskOutcome::Locked | ScanTaskOutcome::UnsupportedChain(_) => 0,
    }
}

pub fn scan_task_update_metadata(outcome: &ScanTaskOutcome) -> serde_json::Value {
    match outcome {
        ScanTaskOutcome::Scanned { .. } => serde_json::json!({ "outcome": "scanned" }),
        ScanTaskOutcome::Locked => serde_json::json!({ "outcome": "locked" }),
        ScanTaskOutcome::UnsupportedChain(chain_type) => {
            serde_json::json!({ "outcome": "unsupported", "chain_type": chain_type })
        }
    }
}

async fn create_scan_run_for_task(
    pool: &PgPool,
    task: &ScanAddressTask,
    started_at: DateTime<Utc>,
    worker_id: &str,
) -> Option<coin_listener_core::models::ScanRun> {
    let context = match scan_runs::scan_run_context(pool, task).await {
        Ok(context) => context,
        Err(error) => {
            warn!(
                task_id = %task.task_id,
                address_id = %task.address_id,
                error = %error,
                "failed to load scan run audit context"
            );
            return None;
        }
    };

    match scan_runs::create_scan_run(
        pool,
        task,
        &context,
        started_at,
        serde_json::json!({ "worker_id": worker_id }),
    )
    .await
    {
        Ok(run) => Some(run),
        Err(error) => {
            warn!(
                task_id = %task.task_id,
                address_id = %task.address_id,
                error = %error,
                "failed to create scan run audit"
            );
            None
        }
    }
}

async fn finish_scan_run_for_result(
    pool: &PgPool,
    scan_run_id: uuid::Uuid,
    result: &AppResult<ScanTaskOutcome>,
    finished_at: DateTime<Utc>,
) -> AppResult<()> {
    match result {
        Ok(outcome) => {
            scan_runs::finish_scan_run(
                pool,
                scan_run_id,
                scan_task_outcome_log_status(outcome),
                scan_task_event_count(outcome),
                finished_at,
                None,
                scan_task_update_metadata(outcome),
            )
            .await?;
        }
        Err(error) => {
            let error_message = error.to_string();
            scan_runs::finish_scan_run(
                pool,
                scan_run_id,
                scan_runs::SCAN_RUN_STATUS_FAILED,
                0,
                finished_at,
                Some(error_message.as_str()),
                serde_json::json!({ "outcome": "failed" }),
            )
            .await?;
        }
    }
    Ok(())
}
```

- [ ] **Step 5: Preserve scan semantics while reporting event counts**

In `process_locked_scan_task`, replace each supported scan branch.

EVM branch:

```rust
ScanPlan::EvmNativeBalance => {
    let events = scan_evm_address(pool, redis, task, now).await?;
    repositories::finish_address_scan(pool, task.address_id, now, next_scan_at).await?;
    Ok(ScanTaskOutcome::Scanned {
        event_count: events.len(),
    })
}
```

TRON branch:

```rust
ScanPlan::Tron => {
    let events = scan_tron_address(pool, redis, task, now).await?;
    repositories::finish_address_scan(pool, task.address_id, now, next_scan_at).await?;
    Ok(ScanTaskOutcome::Scanned {
        event_count: events.len(),
    })
}
```

BTC branch:

```rust
ScanPlan::Btc => {
    let events = scan_btc_address(pool, redis, task, now).await?;
    repositories::finish_address_scan(pool, task.address_id, now, next_scan_at).await?;
    Ok(ScanTaskOutcome::Scanned {
        event_count: events.len(),
    })
}
```

Do not change the unsupported branch cursor behavior; keep the existing `finish_address_scan` call and `UnsupportedChain` outcome.

- [ ] **Step 6: Wire audit recording into `run_worker`**

Replace the `Ok(Some(task))` branch in `run_worker` with:

```rust
Ok(Some(task)) => {
    let task_id = task.task_id;
    let address_id = task.address_id;
    let started_at = Utc::now();
    let scan_run = create_scan_run_for_task(&pool, &task, started_at, &worker_id).await;
    let scan_run_id = scan_run.as_ref().map(|run| run.id);
    let result = process_scan_task(&pool, &mut redis, &scan_queue, task, started_at).await;

    if let Some(scan_run_id) = scan_run_id {
        if let Err(error) = finish_scan_run_for_result(&pool, scan_run_id, &result, Utc::now()).await {
            warn!(
                task_id = %task_id,
                address_id = %address_id,
                scan_run_id = %scan_run_id,
                error = %error,
                "failed to update scan run audit"
            );
        }
    }

    match result {
        Ok(outcome) => info!(
            task_id = %task_id,
            address_id = %address_id,
            scan_run_id = ?scan_run_id,
            scan_status = scan_task_outcome_log_status(&outcome),
            ?outcome,
            "scan task processed"
        ),
        Err(error) => warn!(
            task_id = %task_id,
            address_id = %address_id,
            scan_run_id = ?scan_run_id,
            scan_status = "failed",
            error = %error,
            "scan task failed"
        ),
    }
}
```

- [ ] **Step 7: Run worker scan audit tests and verify they pass**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p worker scan_task_logging -- --nocapture
```

Expected: PASS.

- [ ] **Step 8: Run all worker tests**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p worker
```

Expected: PASS.

- [ ] **Step 9: Commit Task 4**

Run:

```bash
git add backend/crates/worker/Cargo.toml backend/crates/worker/src/lib.rs
git commit -m "$(cat <<'EOF'
记录扫描任务审计生命周期
EOF
)"
```

---

### Task 5: Add scan run API routes and retry enqueue

**Files:**
- Modify: `backend/crates/api-server/src/routes.rs`
- Test: `backend/crates/api-server/src/routes.rs`

- [ ] **Step 1: Write failing API route tests**

In `backend/crates/api-server/src/routes.rs`, append this test after `router_exposes_notification_operations_routes`:

```rust
#[tokio::test]
async fn router_exposes_scan_run_routes() {
    let app = build_router(test_state());

    for (method, uri, status) in [
        (
            Method::GET,
            "/api/scan-runs?status=failed&limit=50&offset=0",
            StatusCode::UNAUTHORIZED,
        ),
        (
            Method::GET,
            "/api/scan-runs/not-a-uuid",
            StatusCode::UNAUTHORIZED,
        ),
        (
            Method::POST,
            "/api/scan-runs/not-a-uuid/retry",
            StatusCode::UNAUTHORIZED,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), status, "{uri}");
    }
}

#[test]
fn scan_run_handlers_use_tenant_scope_and_scan_queue_retry() {
    let source = production_source();

    assert!(source.contains("Extension(auth): Extension<AuthContext>"));
    assert!(source.contains("scan_runs::list_scan_runs(&state.postgres, auth.tenant_id"));
    assert!(source.contains("scan_runs::get_scan_run_detail(&state.postgres, auth.tenant_id"));
    assert!(source.contains("scan_runs::retry_scan_run_task(&state.postgres, auth.tenant_id"));
    assert!(source.contains("ScanQueue::new(state.scan_queue_key.clone(), 1)"));
    assert!(source.contains("queue.enqueue(&mut connection, &task).await?"));
}
```

- [ ] **Step 2: Run API scan run tests and verify they fail**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p api-server scan_run -- --nocapture
```

Expected: FAIL because scan run routes and handlers do not exist.

- [ ] **Step 3: Add API imports**

In the `coin_listener_core::models` import list, add:

```rust
RetryScanRunResponse, ScanRunListResponse, ScanRunQuery,
```

In the `coin_listener_storage` import list, change:

```rust
repositories,
```

to:

```rust
repositories, scan_runs,
```

- [ ] **Step 4: Register protected routes**

In `build_router`, add these routes after `/api/events` and before `/api/evm/transactions/rescan`:

```rust
.route("/api/scan-runs", get(list_scan_runs))
.route("/api/scan-runs/:id", get(get_scan_run))
.route("/api/scan-runs/:id/retry", post(retry_scan_run))
```

- [ ] **Step 5: Implement handlers**

Insert these handlers near the notification operations handlers:

```rust
async fn list_scan_runs(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<ScanRunQuery>,
) -> Result<Response, ApiError> {
    let limit = scan_runs::scan_runs_limit(query.limit);
    let offset = scan_runs::scan_runs_offset(query.offset);
    let items = scan_runs::list_scan_runs(&state.postgres, auth.tenant_id, query).await?;

    Ok(Json(ScanRunListResponse {
        items,
        limit,
        offset,
    })
    .into_response())
}

async fn get_scan_run(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let detail = scan_runs::get_scan_run_detail(&state.postgres, auth.tenant_id, id).await?;
    Ok(Json(detail).into_response())
}

async fn retry_scan_run(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let task = scan_runs::retry_scan_run_task(&state.postgres, auth.tenant_id, id, Utc::now()).await?;
    let redis_client = state
        .redis
        .as_ref()
        .ok_or_else(|| AppError::Redis("redis unavailable".to_string()))?;
    let mut connection = connect_scan_queue(redis_client).await?;
    let queue = ScanQueue::new(state.scan_queue_key.clone(), 1);
    queue.enqueue(&mut connection, &task).await?;

    Ok(Json(RetryScanRunResponse { task }).into_response())
}
```

- [ ] **Step 6: Run API scan run tests and verify they pass**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p api-server scan_run -- --nocapture
```

Expected: PASS.

- [ ] **Step 7: Run all API server tests**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml -p api-server
```

Expected: PASS.

- [ ] **Step 8: Commit Task 5**

Run:

```bash
git add backend/crates/api-server/src/routes.rs
git commit -m "$(cat <<'EOF'
开放扫描审计 API
EOF
)"
```

---

### Task 6: Add frontend scan audit API contracts

**Files:**
- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/api/client.ts`
- Modify: `frontend/src/ui-regression.test.ts`

- [ ] **Step 1: Write failing frontend API regression test**

In `frontend/src/ui-regression.test.ts`, append this test before the final `});`:

```ts
test('scan audit API contracts are exposed to frontend', () => {
  const types = readSource('api/types.ts');
  const client = readSource('api/client.ts');

  for (const expected of [
    'export type ScanRunStatus',
    'export type ScanAddressTask',
    'export type ScanRunListItem',
    'export type ScanRunDetail',
    'export type ScanRunQuery',
    'export type ScanRunListResponse',
    'export type RetryScanRunResponse',
    'last_success_at?: string | null',
    'last_failed_at?: string | null',
    'last_24h_success: number',
    'last_24h_failed: number',
    'recent_runs: ScanRunListItem[]',
  ]) {
    expectContains(types, expected);
  }

  for (const expected of [
    'listScanRuns',
    'getScanRun',
    'retryScanRun',
    '/api/scan-runs',
    '/api/scan-runs/${id}',
    '/api/scan-runs/${id}/retry',
  ]) {
    expectContains(client, expected);
  }
});
```

- [ ] **Step 2: Run frontend UI regression tests and verify they fail**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: FAIL because scan audit frontend types and client functions are missing.

- [ ] **Step 3: Add TypeScript types**

In `frontend/src/api/types.ts`, insert these types before `QueueStatus`:

```ts
export type ScanRunStatus = 'running' | 'success' | 'failed' | 'locked' | 'unsupported' | string;

export type ScanAddressTask = {
  task_id: string;
  address_id: string;
  tenant_id: string;
  chain_id: string;
  attempt: number;
  enqueued_at: string;
};

export type ScanRunListItem = {
  id: string;
  tenant_id: string;
  task_id: string;
  address_id: string;
  chain_id: string;
  chain_name: string;
  address: string;
  address_label?: string | null;
  chain_type: string;
  status: ScanRunStatus;
  event_count: number;
  started_at: string;
  finished_at?: string | null;
  duration_ms?: number | null;
  error_message?: string | null;
};

export type ScanRunDetail = ScanRunListItem & {
  metadata: Record<string, unknown>;
  created_at: string;
  updated_at: string;
};

export type ScanRunQuery = {
  chain_id?: string;
  address_id?: string;
  status?: ScanRunStatus;
  started_after?: string;
  started_before?: string;
  limit?: number;
  offset?: number;
};

export type ScanRunListResponse = {
  items: ScanRunListItem[];
  limit: number;
  offset: number;
};

export type RetryScanRunResponse = {
  task: ScanAddressTask;
};
```

Replace the existing `ScanStatus` type with:

```ts
export type ScanStatus = {
  active_addresses: number;
  due_addresses: number;
  overdue_addresses: number;
  last_scanned_at?: string | null;
  last_success_at?: string | null;
  last_failed_at?: string | null;
  last_24h_success: number;
  last_24h_failed: number;
  recent_runs: ScanRunListItem[];
};
```

- [ ] **Step 4: Add client imports and functions**

In `frontend/src/api/client.ts`, add these imports from `./types`:

```ts
RetryScanRunResponse,
ScanRunDetail,
ScanRunListResponse,
ScanRunQuery,
```

Insert these functions after `listEvents`:

```ts
export function listScanRuns(filters: ScanRunQuery = {}): Promise<ScanRunListResponse> {
  return request<ScanRunListResponse>(`/api/scan-runs${buildQuery(filters)}`);
}

export function getScanRun(id: string): Promise<ScanRunDetail> {
  return request<ScanRunDetail>(`/api/scan-runs/${id}`);
}

export function retryScanRun(id: string): Promise<RetryScanRunResponse> {
  return request<RetryScanRunResponse>(`/api/scan-runs/${id}/retry`, {
    method: 'POST',
  });
}
```

- [ ] **Step 5: Run frontend UI regression tests and verify they pass**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: PASS.

- [ ] **Step 6: Run frontend build**

Run:

```bash
npm --prefix frontend run build
```

Expected: PASS. Existing Vite warnings about `lottie-web` direct eval or chunk size can be reported if they still appear.

- [ ] **Step 7: Commit Task 6**

Run:

```bash
git add frontend/src/api/types.ts frontend/src/api/client.ts frontend/src/ui-regression.test.ts
git commit -m "$(cat <<'EOF'
添加扫描审计前端 API 合约
EOF
)"
```

---

### Task 7: Add `ScanAuditPage` and navigation

**Files:**
- Create: `frontend/src/pages/ScanAuditPage.tsx`
- Modify: `frontend/src/App.tsx`
- Modify: `frontend/src/ui-regression.test.ts`

- [ ] **Step 1: Write failing page and navigation regression tests**

In `frontend/src/ui-regression.test.ts`, append this test before the final `});`:

```ts
test('scan audit page is wired into navigation with Chinese statuses and retry rules', () => {
  const app = readSource('App.tsx');
  const page = readSource('pages/ScanAuditPage.tsx');

  expectContains(app, "'scan-audit'");
  expectContains(app, 'ScanAuditPage');
  expectContains(app, '扫描审计');

  for (const expected of [
    'listScanRuns',
    'getScanRun',
    'retryScanRun',
    'tableId="scan-runs"',
    '扫描中',
    '成功',
    '失败',
    '跳过：锁占用',
    '不支持',
    'retryableScanRun(row.status) ? ',
    'JSON.stringify(detail.metadata, null, 2)',
    'queryClient.invalidateQueries({ queryKey: [\'scan-runs\'] })',
    'queryClient.invalidateQueries({ queryKey: [\'system-status\'] })',
  ]) {
    expectContains(page, expected);
  }
});
```

Also add `pages/ScanAuditPage.tsx` to the `pagePaths` array in the existing `business pages use DataTable for table overflow control` test:

```ts
'pages/ScanAuditPage.tsx',
```

- [ ] **Step 2: Run frontend UI regression tests and verify they fail**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: FAIL because `ScanAuditPage.tsx` and navigation wiring do not exist.

- [ ] **Step 3: Create `ScanAuditPage.tsx`**

Create `frontend/src/pages/ScanAuditPage.tsx` with:

```tsx
import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Banner, Button, Card, Form, Modal, Space, Tag, Toast, Typography } from '@douyinfe/semi-ui';
import { getScanRun, listChains, listScanRuns, listWatchedAddresses, retryScanRun } from '../api/client';
import type { ScanRunDetail, ScanRunListItem, ScanRunQuery, ScanRunStatus } from '../api/types';
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { FilterPanel } from '../components/FilterPanel';
import { PageScaffold } from '../components/PageScaffold';

const { Text } = Typography;

type FilterForm = {
  chain_id?: string;
  address_id?: string;
  status?: string;
  started_after?: string;
  started_before?: string;
};

const scanRunStatusOptions = [
  { label: '扫描中', value: 'running' },
  { label: '成功', value: 'success' },
  { label: '失败', value: 'failed' },
  { label: '跳过：锁占用', value: 'locked' },
  { label: '不支持', value: 'unsupported' },
];

function formatTime(value?: string | null) {
  return value ? new Date(value).toLocaleString() : '-';
}

function formatDuration(value?: number | null) {
  return value === null || value === undefined ? '-' : `${value}ms`;
}

function truncate(value?: string | null, maxLength = 120) {
  if (!value) return '-';
  return value.length > maxLength ? `${value.slice(0, maxLength)}…` : value;
}

function scanRunStatusText(status: ScanRunStatus) {
  if (status === 'running') return '扫描中';
  if (status === 'success') return '成功';
  if (status === 'failed') return '失败';
  if (status === 'locked') return '跳过：锁占用';
  if (status === 'unsupported') return '不支持';
  return status;
}

function scanRunStatusColor(status: ScanRunStatus): 'blue' | 'green' | 'red' | 'orange' | 'grey' {
  if (status === 'running') return 'blue';
  if (status === 'success') return 'green';
  if (status === 'failed') return 'red';
  if (status === 'locked') return 'grey';
  if (status === 'unsupported') return 'orange';
  return 'grey';
}

function retryableScanRun(status: ScanRunStatus) {
  return status === 'failed' || status === 'unsupported';
}

export function ScanAuditPage() {
  const [filters, setFilters] = useState<ScanRunQuery>({ limit: 50, offset: 0 });
  const [selectedRunId, setSelectedRunId] = useState<string>();
  const queryClient = useQueryClient();

  const chainsQuery = useQuery({ queryKey: ['chains'], queryFn: listChains });
  const addressesQuery = useQuery({ queryKey: ['addresses'], queryFn: listWatchedAddresses });

  const scanRunsQuery = useQuery({
    queryKey: ['scan-runs', filters],
    queryFn: () => listScanRuns(filters),
  });

  const detailQuery = useQuery({
    queryKey: ['scan-run-detail', selectedRunId],
    queryFn: () => getScanRun(selectedRunId ?? ''),
    enabled: Boolean(selectedRunId),
  });

  const retryMutation = useMutation({
    mutationFn: retryScanRun,
    onSuccess: () => {
      Toast.success('扫描任务已重新入队');
      queryClient.invalidateQueries({ queryKey: ['scan-runs'] });
      queryClient.invalidateQueries({ queryKey: ['scan-run-detail'] });
      queryClient.invalidateQueries({ queryKey: ['system-status'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '重试扫描任务失败'),
  });

  function handleFilterSubmit(values: Record<string, unknown>) {
    const form = values as FilterForm;
    setFilters({
      chain_id: form.chain_id || undefined,
      address_id: form.address_id || undefined,
      status: form.status || undefined,
      started_after: form.started_after?.trim() || undefined,
      started_before: form.started_before?.trim() || undefined,
      limit: 50,
      offset: 0,
    });
  }

  function resetFilters(formApi: { reset: () => void }) {
    formApi.reset();
    setFilters({ limit: 50, offset: 0 });
  }

  const chainOptions = (chainsQuery.data ?? []).map(chain => ({ label: chain.name, value: chain.id }));
  const addressOptions = (addressesQuery.data ?? []).map(address => ({
    label: `${address.label ?? address.address} / ${address.address.slice(0, 10)}…`,
    value: address.id,
  }));

  return (
    <PageScaffold
      title="扫描审计"
      description="查询 Worker 扫描尝试历史、失败原因与可重试任务。"
      actions={<Tag color={scanRunsQuery.isFetching ? 'blue' : 'green'}>{scanRunsQuery.isFetching ? 'refreshing' : 'manual refresh'}</Tag>}
    >
      {scanRunsQuery.isError ? (
        <Banner
          type="danger"
          title="扫描记录加载失败"
          description={scanRunsQuery.error instanceof Error ? scanRunsQuery.error.message : '请求失败'}
        />
      ) : null}

      {detailQuery.isError ? (
        <Banner
          type="danger"
          title="扫描详情加载失败"
          description={detailQuery.error instanceof Error ? detailQuery.error.message : '请求失败'}
        />
      ) : null}

      <FilterPanel title="扫描记录筛选">
        <Form<FilterForm> layout="horizontal" onSubmit={handleFilterSubmit} labelPosition="left">
          {({ formApi }) => (
            <>
              <Form.Select field="chain_id" label="链" showClear placeholder="全部链" optionList={chainOptions} />
              <Form.Select field="address_id" label="地址" showClear placeholder="全部地址" optionList={addressOptions} style={{ width: 280 }} />
              <Form.Select field="status" label="状态" showClear placeholder="全部状态" optionList={scanRunStatusOptions} />
              <Form.Input field="started_after" label="开始于" placeholder="2026-05-24T00:00:00Z" style={{ width: 220 }} />
              <Form.Input field="started_before" label="结束于" placeholder="2026-05-25T00:00:00Z" style={{ width: 220 }} />
              <Space>
                <Button htmlType="submit" type="primary">查询</Button>
                <Button onClick={() => resetFilters(formApi)}>重置</Button>
                <Button loading={scanRunsQuery.isFetching} onClick={() => scanRunsQuery.refetch()}>刷新</Button>
              </Space>
            </>
          )}
        </Form>
      </FilterPanel>

      <DataSurface title="扫描历史" actions={<Text type="tertiary">limit {filters.limit ?? 50} / offset {filters.offset ?? 0}</Text>}>
        <DataTable<ScanRunListItem>
          tableId="scan-runs"
          loading={scanRunsQuery.isLoading}
          dataSource={scanRunsQuery.data?.items ?? []}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          scroll={{ x: 1700 }}
          columns={[
            { title: '开始时间', dataIndex: 'started_at', width: 180, render: value => formatTime(String(value)) },
            { title: '结束时间', dataIndex: 'finished_at', width: 180, render: value => formatTime(value ? String(value) : null) },
            { title: '链', dataIndex: 'chain_name', width: 150, ellipsis: { showTitle: true } },
            { title: '地址', dataIndex: 'address', width: 260, ellipsis: { showTitle: true }, render: value => <span className="table-cell-mono">{String(value)}</span> },
            { title: '标签', dataIndex: 'address_label', width: 140, render: value => value ? String(value) : '-' },
            { title: '类型', dataIndex: 'chain_type', width: 100, render: value => <Tag>{String(value)}</Tag> },
            { title: '状态', dataIndex: 'status', width: 140, render: value => <Tag color={scanRunStatusColor(String(value))}>{scanRunStatusText(String(value))}</Tag> },
            { title: '耗时', dataIndex: 'duration_ms', width: 110, render: value => formatDuration(value as number | null) },
            { title: '事件数', dataIndex: 'event_count', width: 100 },
            { title: 'Task ID', dataIndex: 'task_id', width: 240, ellipsis: { showTitle: true } },
            { title: '错误摘要', dataIndex: 'error_message', width: 260, ellipsis: { showTitle: true }, render: value => truncate(value ? String(value) : null) },
            {
              title: '操作',
              key: 'operations',
              width: 170,
              render: (_, row) => (
                <Space>
                  <Button size="small" onClick={() => setSelectedRunId(row.id)}>详情</Button>
                  {retryableScanRun(row.status) ? (
                    <Button size="small" type="primary" loading={retryMutation.isPending} onClick={() => retryMutation.mutate(row.id)}>
                      重试
                    </Button>
                  ) : null}
                </Space>
              ),
            },
          ]}
        />
      </DataSurface>

      <ScanRunDetailModal
        visible={Boolean(selectedRunId)}
        loading={detailQuery.isLoading}
        detail={detailQuery.data}
        onClose={() => setSelectedRunId(undefined)}
      />
    </PageScaffold>
  );
}

function ScanRunDetailModal({
  visible,
  loading,
  detail,
  onClose,
}: {
  visible: boolean;
  loading: boolean;
  detail?: ScanRunDetail;
  onClose: () => void;
}) {
  return (
    <Modal title="扫描记录详情" visible={visible} onCancel={onClose} footer={null} width={1120}>
      {loading ? <Text>正在加载详情...</Text> : null}
      {detail ? (
        <Space vertical align="start" spacing={16} style={{ width: '100%' }}>
          <div className="notification-detail-grid">
            <Card title="扫描尝试" className="notification-detail-card">
              <DetailLine label="Run ID" value={detail.id} mono />
              <DetailLine label="Task ID" value={detail.task_id} mono />
              <DetailLine label="链" value={`${detail.chain_name} / ${detail.chain_type}`} />
              <DetailLine label="地址" value={detail.address} mono />
              <div className="detail-line">
                <Text type="tertiary">状态</Text>
                <Tag color={scanRunStatusColor(detail.status)}>{scanRunStatusText(detail.status)}</Tag>
              </div>
              <DetailLine label="事件数" value={String(detail.event_count)} />
              <DetailLine label="耗时" value={formatDuration(detail.duration_ms)} />
              <DetailLine label="开始" value={formatTime(detail.started_at)} />
              <DetailLine label="结束" value={formatTime(detail.finished_at)} />
              <DetailLine label="错误" value={detail.error_message ?? '-'} />
            </Card>

            <Card title="Metadata" className="notification-detail-card">
              <div className="detail-line detail-line-vertical">
                <Text type="tertiary">运行元数据</Text>
                <pre className="detail-json">{JSON.stringify(detail.metadata, null, 2)}</pre>
              </div>
            </Card>
          </div>
        </Space>
      ) : null}
    </Modal>
  );
}

function DetailLine({ label, value, mono = false }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="detail-line">
      <Text type="tertiary">{label}</Text>
      <span className={mono ? 'table-cell-mono detail-value' : 'detail-value'}>{value}</span>
    </div>
  );
}
```

- [ ] **Step 4: Wire navigation in `App.tsx`**

In `frontend/src/App.tsx`, add the import:

```tsx
import { ScanAuditPage } from './pages/ScanAuditPage';
```

Add `'scan-audit'` to `PageKey` after `'system-status'`:

```ts
| 'scan-audit'
```

Add this nav item after `系统状态`:

```tsx
{ itemKey: 'scan-audit', text: '扫描审计', icon: <IconPulse /> },
```

Add this render branch after the `system-status` branch:

```tsx
if (page === 'scan-audit') return <ScanAuditPage />;
```

- [ ] **Step 5: Run frontend UI regression tests and verify they pass**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: PASS.

- [ ] **Step 6: Run frontend build**

Run:

```bash
npm --prefix frontend run build
```

Expected: PASS. Existing Vite warnings about `lottie-web` direct eval or chunk size can be reported if they still appear.

- [ ] **Step 7: Commit Task 7**

Run:

```bash
git add frontend/src/pages/ScanAuditPage.tsx frontend/src/App.tsx frontend/src/ui-regression.test.ts
git commit -m "$(cat <<'EOF'
添加扫描审计页面
EOF
)"
```

---

### Task 8: Show scan audit summary on system status page

**Files:**
- Modify: `frontend/src/pages/SystemStatusPage.tsx`
- Modify: `frontend/src/ui-regression.test.ts`

- [ ] **Step 1: Write failing system status UI regression test**

In `frontend/src/ui-regression.test.ts`, append this test before the final `});`:

```ts
test('system status page shows scan audit health summary and recent runs', () => {
  const page = readSource('pages/SystemStatusPage.tsx');

  for (const expected of [
    'last_24h_success',
    'last_24h_failed',
    'last_success_at',
    'last_failed_at',
    'recent_runs',
    '扫描成功',
    '扫描失败',
    '最近扫描记录',
    'tableId="system-recent-scan-runs"',
    'scanRunStatusText',
    '跳过：锁占用',
  ]) {
    expectContains(page, expected);
  }
});
```

- [ ] **Step 2: Run frontend UI regression tests and verify they fail**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: FAIL because `SystemStatusPage.tsx` does not show scan audit summary fields or recent runs.

- [ ] **Step 3: Add scan run helpers**

In `frontend/src/pages/SystemStatusPage.tsx`, update the type import to include `ScanRunListItem`:

```tsx
import type { ProviderChainStatus, ProviderStatusItem, ScanRunListItem, ServiceHeartbeatStatusItem } from '../api/types';
```

Insert these helpers after `serviceStatusText`:

```tsx
function scanRunStatusText(status: string) {
  if (status === 'running') return '扫描中';
  if (status === 'success') return '成功';
  if (status === 'failed') return '失败';
  if (status === 'locked') return '跳过：锁占用';
  if (status === 'unsupported') return '不支持';
  return status;
}

function scanRunStatusColor(status: string): 'blue' | 'green' | 'red' | 'orange' | 'grey' {
  if (status === 'running') return 'blue';
  if (status === 'success') return 'green';
  if (status === 'failed') return 'red';
  if (status === 'locked') return 'grey';
  if (status === 'unsupported') return 'orange';
  return 'grey';
}
```

- [ ] **Step 4: Add scan health metric cards**

In the existing `<MetricGrid>`, insert these cards after the `Overdue 地址` card:

```tsx
<MetricCard
  title="扫描成功"
  value={status?.scans.last_24h_success ?? 0}
  hint={`last ${formatTime(status?.scans.last_success_at)}`}
  tone="success"
/>
<MetricCard
  title="扫描失败"
  value={status?.scans.last_24h_failed ?? 0}
  hint={`last ${formatTime(status?.scans.last_failed_at)}`}
  tone={status?.scans.last_24h_failed ? 'danger' : 'neutral'}
/>
```

- [ ] **Step 5: Add compact recent-runs table**

Insert this `DataSurface` after the existing `扫描与通知摘要` surface and before `服务心跳`:

```tsx
<DataSurface title="最近扫描记录" actions={<Text type="tertiary">最近 {status?.scans.recent_runs.length ?? 0} 条</Text>}>
  <DataTable<ScanRunListItem>
    tableId="system-recent-scan-runs"
    loading={statusQuery.isLoading}
    dataSource={status?.scans.recent_runs ?? []}
    rowKey="id"
    pagination={false}
    scroll={{ x: 1100 }}
    columns={[
      { title: '开始时间', dataIndex: 'started_at', width: 180, render: value => formatTime(String(value)) },
      { title: '链', dataIndex: 'chain_name', width: 150, ellipsis: { showTitle: true } },
      { title: '地址', dataIndex: 'address', width: 240, ellipsis: { showTitle: true }, render: value => <span className="table-cell-mono">{String(value)}</span> },
      { title: '状态', dataIndex: 'status', width: 140, render: value => <Tag color={scanRunStatusColor(String(value))}>{scanRunStatusText(String(value))}</Tag> },
      { title: '耗时', dataIndex: 'duration_ms', width: 100, render: value => value === null || value === undefined ? '-' : `${String(value)}ms` },
      { title: '事件数', dataIndex: 'event_count', width: 90 },
      { title: '错误', dataIndex: 'error_message', width: 260, ellipsis: { showTitle: true }, render: value => truncateError(value ? String(value) : null) },
    ]}
  />
</DataSurface>
```

- [ ] **Step 6: Run frontend UI regression tests and verify they pass**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: PASS.

- [ ] **Step 7: Run frontend build**

Run:

```bash
npm --prefix frontend run build
```

Expected: PASS. Existing Vite warnings about `lottie-web` direct eval or chunk size can be reported if they still appear.

- [ ] **Step 8: Commit Task 8**

Run:

```bash
git add frontend/src/pages/SystemStatusPage.tsx frontend/src/ui-regression.test.ts
git commit -m "$(cat <<'EOF'
展示扫描审计状态摘要
EOF
)"
```

---

### Task 9: Final integration verification

**Files:**
- Verify: backend workspace, frontend regression tests, frontend production build, git state

- [ ] **Step 1: Format backend code**

Run:

```bash
cargo fmt --manifest-path backend/Cargo.toml --all -- --check
```

Expected: PASS. If it fails with formatting diffs, run:

```bash
cargo fmt --manifest-path backend/Cargo.toml --all
```

Then rerun the check command and expect PASS.

- [ ] **Step 2: Run backend tests**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml
```

Expected: PASS for all backend crates.

- [ ] **Step 3: Run frontend source regression tests**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: PASS.

- [ ] **Step 4: Run frontend build**

Run:

```bash
npm --prefix frontend run build
```

Expected: PASS. Existing Vite warnings about `lottie-web` direct eval or chunk size can be reported if they still appear.

- [ ] **Step 5: Inspect changed files**

Run:

```bash
git status --short
git diff --stat
```

Expected: only scan audit center files from this plan are modified or newly created.

- [ ] **Step 6: Final commit**

If any verified changes remain uncommitted from fixups during Task 9, commit only the relevant scan-audit files:

```bash
git add backend/crates/core/src/models.rs \
  backend/crates/storage/migrations/0019_scan_runs.sql \
  backend/crates/storage/src/scan_runs.rs \
  backend/crates/storage/src/lib.rs \
  backend/crates/storage/src/system_status.rs \
  backend/crates/worker/Cargo.toml \
  backend/crates/worker/src/lib.rs \
  backend/crates/api-server/src/routes.rs \
  frontend/src/api/types.ts \
  frontend/src/api/client.ts \
  frontend/src/App.tsx \
  frontend/src/pages/ScanAuditPage.tsx \
  frontend/src/pages/SystemStatusPage.tsx \
  frontend/src/ui-regression.test.ts
git commit -m "$(cat <<'EOF'
完善扫描审计集成验证
EOF
)"
```

If `git status --short` is clean, do not create an empty commit.

---

## Implementation Notes

- Retry creates a new `ScanAddressTask` with `attempt: 1`; attempt history is tracked by separate `scan_runs` rows rather than mutating the old row.
- Worker audit insert/update failures are logged and do not change the result returned by `process_scan_task`.
- `success`, `locked`, and `unsupported` preserve existing `finish_address_scan` behavior. `failed` preserves existing failure behavior.
- `GET /api/scan-runs` and `GET /api/scan-runs/:id` are tenant scoped through `auth.tenant_id`.
- `POST /api/scan-runs/:id/retry` validates tenant scope and retryable status before Redis enqueue.
- Frontend retry action is rendered only for `failed` and `unsupported` rows.
