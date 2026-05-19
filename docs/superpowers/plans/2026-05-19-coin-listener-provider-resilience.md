# Coin Listener Provider Resilience Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make worker scans resilient to provider outages by tracking provider health, temporarily skipping unhealthy providers, falling back to lower-priority providers, enforcing per-provider QPS limits, and exposing runtime health in system status.

**Architecture:** Provider runtime state lives in a new PostgreSQL `provider_health` table keyed by `provider_id`. Storage helpers return active provider candidates with open circuits excluded, record success/failure state, sanitize persisted errors, and enforce simple Redis QPS counters. Worker scan paths fetch ordered candidates and try them until one succeeds, while `/api/system/status` and the existing Semi Design status page display each provider's runtime circuit state.

**Tech Stack:** Rust workspace, SQLx/PostgreSQL migrations, Redis counters, worker async scan pipeline, Axum system status DTOs, React + TypeScript + Semi Design frontend.

---

## File Structure

- Create `backend/crates/storage/migrations/0011_provider_health.sql` for durable runtime provider health rows and indexes.
- Create `backend/crates/storage/src/provider_health.rs` for health DTO rows, query strings, sanitizer helpers, circuit helper functions, Redis QPS helpers, and repository functions.
- Modify `backend/crates/storage/src/lib.rs` to export `provider_health`.
- Modify `backend/crates/core/src/models.rs` to add `ProviderHealthStatus` and extend `ProviderStatusItem`.
- Modify `backend/crates/storage/src/system_status.rs` to left join provider health and map default healthy values for providers with no health row.
- Modify `backend/crates/worker/src/lib.rs` to route EVM/TRON/BTC scans through provider candidate failover and QPS checks using the existing Redis connection.
- Modify frontend:
  - `frontend/src/api/types.ts` for `ProviderHealthStatus` and extended `ProviderStatusItem`.
  - `frontend/src/pages/SystemStatusPage.tsx` for config status and runtime circuit status columns.

## Constraints

- Do not add provider management buttons, manual reset APIs, alert delivery, uptime charts, API key rotation, or scheduler-level capacity planning.
- Do not persist provider `base_url`, `api_key_ref`, tokens, API keys, or full query strings in `provider_health.last_error`.
- Provider request/status failures are treated as availability failures only when the scan returns `AppError::Config(_)`; validation, database, Redis, unsupported-chain, and auth errors do not trigger fallback.
- Redis QPS limiter errors fail the scan task rather than silently exceeding configured provider limits.
- Health write failures must not hide the original scan result. Log them and keep returning the scan success or original scan error.
- Preserve unrelated `frontend/package-lock.json` changes; do not stage or commit it unless package dependencies are deliberately changed.
- Follow TDD: each production behavior gets a failing test before implementation.

---

### Task 1: Add provider health DTOs

**Files:**
- Modify: `backend/crates/core/src/models.rs`

- [ ] **Step 1: Write the failing DTO tests**

In `backend/crates/core/src/models.rs`, extend the `use super::{ ... }` list inside `#[cfg(test)] mod tests` to include `ProviderHealthStatus`, then add this test near `service_health_status_round_trips_as_json`:

```rust
#[test]
fn provider_health_status_round_trips_as_json() {
    let health = ProviderHealthStatus {
        consecutive_failures: 3,
        last_success_at: Some(Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 0).unwrap()),
        last_failure_at: Some(Utc.with_ymd_and_hms(2026, 5, 19, 10, 4, 0).unwrap()),
        disabled_until: Some(Utc.with_ymd_and_hms(2026, 5, 19, 10, 9, 0).unwrap()),
        last_error: Some("provider request failed: timeout".to_string()),
        is_circuit_open: true,
    };

    let payload = serde_json::to_string(&health).expect("serialize provider health");
    let decoded: ProviderHealthStatus =
        serde_json::from_str(&payload).expect("deserialize provider health");

    assert_eq!(decoded, health);
    assert!(payload.contains("\"consecutive_failures\":3"));
    assert!(payload.contains("\"is_circuit_open\":true"));
}
```

In `system_status_round_trips_as_json`, extend the existing `ProviderStatusItem` literal with:

```rust
health: ProviderHealthStatus {
    consecutive_failures: 0,
    last_success_at: None,
    last_failure_at: None,
    disabled_until: None,
    last_error: None,
    is_circuit_open: false,
},
```

Add this assertion after the existing payload checks:

```rust
assert!(payload.contains("\"health\""));
```

- [ ] **Step 2: Run DTO tests to verify they fail**

Run:

```bash
cargo test -p coin-listener-core provider_health_status_round_trips_as_json --manifest-path backend/Cargo.toml
```

Expected: FAIL to compile with missing `ProviderHealthStatus`.

- [ ] **Step 3: Add minimal DTOs**

Add this struct near the other system status DTOs in `backend/crates/core/src/models.rs`, before `ProviderStatus`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderHealthStatus {
    pub consecutive_failures: i32,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub disabled_until: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub is_circuit_open: bool,
}
```

Add this field to `ProviderStatusItem`:

```rust
pub health: ProviderHealthStatus,
```

- [ ] **Step 4: Run DTO tests to verify they pass**

Run:

```bash
cargo test -p coin-listener-core provider_health_status_round_trips_as_json --manifest-path backend/Cargo.toml
cargo test -p coin-listener-core system_status_round_trips_as_json --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 5: Commit Task 1**

```bash
git add backend/crates/core/src/models.rs
git commit -m "Add provider health DTOs"
```

---

### Task 2: Add provider health migration and base helpers

**Files:**
- Create: `backend/crates/storage/migrations/0011_provider_health.sql`
- Create: `backend/crates/storage/src/provider_health.rs`
- Modify: `backend/crates/storage/src/lib.rs`

- [ ] **Step 1: Write failing migration and helper tests**

Create `backend/crates/storage/src/provider_health.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use crate::provider_health::{
        is_provider_circuit_open, provider_disabled_until, provider_qps_key,
        sanitize_provider_error, PROVIDER_CIRCUIT_COOLDOWN_SECONDS,
        PROVIDER_CIRCUIT_FAILURE_THRESHOLD, PROVIDER_LAST_ERROR_MAX_CHARS,
    };

    #[test]
    fn provider_health_migration_defines_table_and_indexes() {
        let migration = include_str!("../migrations/0011_provider_health.sql");

        assert!(migration.contains("CREATE TABLE IF NOT EXISTS provider_health"));
        assert!(migration.contains("provider_id UUID PRIMARY KEY REFERENCES providers(id) ON DELETE CASCADE"));
        assert!(migration.contains("disabled_until TIMESTAMPTZ"));
        assert!(migration.contains("idx_provider_health_disabled_until"));
        assert!(migration.contains("idx_provider_health_last_failure"));
    }

    #[test]
    fn provider_qps_key_uses_provider_id_and_epoch_second() {
        let provider_id = uuid::Uuid::from_u128(42);

        assert_eq!(
            provider_qps_key(provider_id, 1_779_123_600),
            "provider:qps:00000000-0000-0000-0000-00000000002a:1779123600"
        );
    }

    #[test]
    fn provider_disabled_until_uses_five_minute_cooldown() {
        let now = Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 0).unwrap();

        assert_eq!(PROVIDER_CIRCUIT_FAILURE_THRESHOLD, 3);
        assert_eq!(PROVIDER_CIRCUIT_COOLDOWN_SECONDS, 300);
        assert_eq!(
            provider_disabled_until(PROVIDER_CIRCUIT_FAILURE_THRESHOLD, now),
            Some(Utc.with_ymd_and_hms(2026, 5, 19, 10, 5, 0).unwrap())
        );
        assert_eq!(provider_disabled_until(PROVIDER_CIRCUIT_FAILURE_THRESHOLD - 1, now), None);
    }

    #[test]
    fn circuit_is_open_only_while_disabled_until_is_future() {
        let now = Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 0).unwrap();
        let past = Utc.with_ymd_and_hms(2026, 5, 19, 9, 59, 59).unwrap();
        let future = Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 1).unwrap();

        assert!(!is_provider_circuit_open(None, now));
        assert!(!is_provider_circuit_open(Some(past), now));
        assert!(is_provider_circuit_open(Some(future), now));
    }

    #[test]
    fn provider_error_sanitizer_redacts_query_secrets_and_truncates() {
        let secret = format!(
            "provider request failed: https://example.invalid/rpc?token=abc&api_key=def&key=ghi&safe=ok {}",
            "x".repeat(PROVIDER_LAST_ERROR_MAX_CHARS + 20)
        );

        let sanitized = sanitize_provider_error(&secret);

        assert!(!sanitized.contains("abc"));
        assert!(!sanitized.contains("def"));
        assert!(!sanitized.contains("ghi"));
        assert!(sanitized.contains("token=<redacted>"));
        assert!(sanitized.contains("api_key=<redacted>"));
        assert!(sanitized.contains("key=<redacted>"));
        assert!(sanitized.len() <= PROVIDER_LAST_ERROR_MAX_CHARS);
    }
}
```

Add `pub mod provider_health;` to `backend/crates/storage/src/lib.rs` so the new module is compiled.

- [ ] **Step 2: Run storage tests to verify they fail**

Run:

```bash
cargo test -p coin-listener-storage provider_health --manifest-path backend/Cargo.toml
```

Expected: FAIL with missing migration file and missing helper constants/functions.

- [ ] **Step 3: Add migration**

Create `backend/crates/storage/migrations/0011_provider_health.sql`:

```sql
CREATE TABLE IF NOT EXISTS provider_health (
    provider_id UUID PRIMARY KEY REFERENCES providers(id) ON DELETE CASCADE,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    last_success_at TIMESTAMPTZ,
    last_failure_at TIMESTAMPTZ,
    disabled_until TIMESTAMPTZ,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_provider_health_disabled_until
    ON provider_health(disabled_until)
    WHERE disabled_until IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_provider_health_last_failure
    ON provider_health(last_failure_at DESC);
```

- [ ] **Step 4: Add minimal helper implementation**

Above the test module in `backend/crates/storage/src/provider_health.rs`, add:

```rust
use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

pub const PROVIDER_CIRCUIT_FAILURE_THRESHOLD: i32 = 3;
pub const PROVIDER_CIRCUIT_COOLDOWN_SECONDS: i64 = 300;
pub const PROVIDER_LAST_ERROR_MAX_CHARS: usize = 500;

pub fn provider_qps_key(provider_id: Uuid, epoch_second: i64) -> String {
    format!("provider:qps:{provider_id}:{epoch_second}")
}

pub fn provider_disabled_until(failure_count: i32, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    (failure_count >= PROVIDER_CIRCUIT_FAILURE_THRESHOLD)
        .then(|| now + Duration::seconds(PROVIDER_CIRCUIT_COOLDOWN_SECONDS))
}

pub fn is_provider_circuit_open(disabled_until: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    disabled_until.is_some_and(|value| value > now)
}

pub fn sanitize_provider_error(error: &str) -> String {
    let mut sanitized = error
        .replace("token=", "token=<redacted>")
        .replace("api_key=", "api_key=<redacted>")
        .replace("key=", "key=<redacted>");

    sanitized = redact_query_value(&sanitized, "token=<redacted>");
    sanitized = redact_query_value(&sanitized, "api_key=<redacted>");
    sanitized = redact_query_value(&sanitized, "key=<redacted>");

    sanitized.chars().take(PROVIDER_LAST_ERROR_MAX_CHARS).collect()
}

fn redact_query_value(input: &str, marker: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut remaining = input;

    while let Some(index) = remaining.find(marker) {
        output.push_str(&remaining[..index + marker.len()]);
        let after_marker = &remaining[index + marker.len()..];
        let next_separator = after_marker
            .find('&')
            .or_else(|| after_marker.find(' '))
            .unwrap_or(after_marker.len());
        remaining = &after_marker[next_separator..];
    }

    output.push_str(remaining);
    output
}
```

- [ ] **Step 5: Run storage helper tests to verify they pass**

Run:

```bash
cargo test -p coin-listener-storage provider_health --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 6: Commit Task 2**

```bash
git add backend/crates/storage/migrations/0011_provider_health.sql backend/crates/storage/src/provider_health.rs backend/crates/storage/src/lib.rs
git commit -m "Add provider health storage foundation"
```

---

### Task 3: Add provider candidate and health write storage helpers

**Files:**
- Modify: `backend/crates/storage/src/provider_health.rs`

- [ ] **Step 1: Write failing query and row mapping tests**

Extend the test imports in `backend/crates/storage/src/provider_health.rs` to include:

```rust
use coin_listener_core::models::Provider;

use crate::provider_health::{
    provider_candidate_health, provider_qps_permits, ProviderCandidate, ProviderHealthRow,
    ACTIVE_RPC_PROVIDER_CANDIDATES_QUERY, RECORD_PROVIDER_FAILURE_QUERY,
    RECORD_PROVIDER_SUCCESS_QUERY,
};
```

Add these tests inside the existing `tests` module:

```rust
#[test]
fn active_candidate_query_excludes_open_circuits_and_orders_by_priority() {
    assert!(ACTIVE_RPC_PROVIDER_CANDIDATES_QUERY.contains("LEFT JOIN provider_health ph"));
    assert!(ACTIVE_RPC_PROVIDER_CANDIDATES_QUERY.contains("provider_type = 'rpc'"));
    assert!(ACTIVE_RPC_PROVIDER_CANDIDATES_QUERY.contains("p.status = 'active'"));
    assert!(ACTIVE_RPC_PROVIDER_CANDIDATES_QUERY.contains("(ph.disabled_until IS NULL OR ph.disabled_until <= $2)"));
    assert!(ACTIVE_RPC_PROVIDER_CANDIDATES_QUERY.contains("ORDER BY p.priority ASC, p.name ASC"));
    assert!(!ACTIVE_RPC_PROVIDER_CANDIDATES_QUERY.contains("LIMIT 1"));
}

#[test]
fn success_query_resets_failures_and_clears_error_state() {
    assert!(RECORD_PROVIDER_SUCCESS_QUERY.contains("consecutive_failures = 0"));
    assert!(RECORD_PROVIDER_SUCCESS_QUERY.contains("last_success_at = EXCLUDED.last_success_at"));
    assert!(RECORD_PROVIDER_SUCCESS_QUERY.contains("disabled_until = NULL"));
    assert!(RECORD_PROVIDER_SUCCESS_QUERY.contains("last_error = NULL"));
}

#[test]
fn failure_query_increments_failures_and_sets_disabled_until() {
    assert!(RECORD_PROVIDER_FAILURE_QUERY.contains("consecutive_failures + 1"));
    assert!(RECORD_PROVIDER_FAILURE_QUERY.contains("last_failure_at"));
    assert!(RECORD_PROVIDER_FAILURE_QUERY.contains("disabled_until"));
    assert!(RECORD_PROVIDER_FAILURE_QUERY.contains("last_error"));
}

#[test]
fn provider_candidate_health_defaults_missing_health_to_closed_circuit() {
    let candidate = ProviderCandidate {
        provider: provider(1, 10),
        health: None,
    };
    let now = Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 0).unwrap();

    let health = provider_candidate_health(&candidate, now);

    assert_eq!(health.consecutive_failures, 0);
    assert_eq!(health.disabled_until, None);
    assert!(!health.is_circuit_open);
}

#[test]
fn provider_candidate_health_marks_future_disabled_until_open() {
    let disabled_until = Utc.with_ymd_and_hms(2026, 5, 19, 10, 5, 0).unwrap();
    let candidate = ProviderCandidate {
        provider: provider(1, 10),
        health: Some(ProviderHealthRow {
            provider_id: Uuid::from_u128(1),
            consecutive_failures: 3,
            last_success_at: None,
            last_failure_at: Some(Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 0).unwrap()),
            disabled_until: Some(disabled_until),
            last_error: Some("provider request failed: timeout".to_string()),
        }),
    };
    let now = Utc.with_ymd_and_hms(2026, 5, 19, 10, 1, 0).unwrap();

    let health = provider_candidate_health(&candidate, now);

    assert_eq!(health.consecutive_failures, 3);
    assert_eq!(health.disabled_until, Some(disabled_until));
    assert!(health.is_circuit_open);
}

#[test]
fn provider_qps_permits_counts_within_limit_only() {
    assert!(provider_qps_permits(1, 1));
    assert!(provider_qps_permits(10, 10));
    assert!(!provider_qps_permits(11, 10));
    assert!(!provider_qps_permits(1, 0));
}

fn provider(id: u128, priority: i32) -> Provider {
    Provider {
        id: Uuid::from_u128(id),
        chain_id: Uuid::from_u128(100),
        provider_type: "rpc".to_string(),
        name: format!("provider-{id}"),
        base_url: "https://example.invalid".to_string(),
        api_key_ref: None,
        priority,
        qps_limit: 10,
        timeout_ms: 5000,
        status: "active".to_string(),
    }
}
```

- [ ] **Step 2: Run provider health tests to verify they fail**

Run:

```bash
cargo test -p coin-listener-storage provider_health --manifest-path backend/Cargo.toml
```

Expected: FAIL with missing query constants, structs, and helper functions.

- [ ] **Step 3: Add row structs, query constants, and helper mappings**

Add imports at the top of `backend/crates/storage/src/provider_health.rs`:

```rust
use coin_listener_core::{
    models::{Provider, ProviderHealthStatus},
    AppError, AppResult,
};
use redis::aio::MultiplexedConnection;
use sqlx::{FromRow, PgPool};
```

Add these constants and structs above the helper functions:

```rust
pub const ACTIVE_RPC_PROVIDER_CANDIDATES_QUERY: &str = r#"
SELECT
    p.id,
    p.chain_id,
    p.provider_type,
    p.name,
    p.base_url,
    p.api_key_ref,
    p.priority,
    p.qps_limit,
    p.timeout_ms,
    p.status,
    ph.consecutive_failures,
    ph.last_success_at,
    ph.last_failure_at,
    ph.disabled_until,
    ph.last_error
FROM providers p
LEFT JOIN provider_health ph ON ph.provider_id = p.id
WHERE p.chain_id = $1
  AND p.provider_type = 'rpc'
  AND p.status = 'active'
  AND (ph.disabled_until IS NULL OR ph.disabled_until <= $2)
ORDER BY p.priority ASC, p.name ASC
"#;

pub const RECORD_PROVIDER_SUCCESS_QUERY: &str = r#"
INSERT INTO provider_health (provider_id, consecutive_failures, last_success_at, disabled_until, last_error)
VALUES ($1, 0, $2, NULL, NULL)
ON CONFLICT (provider_id) DO UPDATE
SET consecutive_failures = 0,
    last_success_at = EXCLUDED.last_success_at,
    disabled_until = NULL,
    last_error = NULL,
    updated_at = NOW()
"#;

pub const RECORD_PROVIDER_FAILURE_QUERY: &str = r#"
INSERT INTO provider_health (
    provider_id, consecutive_failures, last_failure_at, disabled_until, last_error
)
VALUES ($1, 1, $2, NULL, $3)
ON CONFLICT (provider_id) DO UPDATE
SET consecutive_failures = provider_health.consecutive_failures + 1,
    last_failure_at = EXCLUDED.last_failure_at,
    disabled_until = CASE
        WHEN provider_health.consecutive_failures + 1 >= $4 THEN $5
        ELSE provider_health.disabled_until
    END,
    last_error = EXCLUDED.last_error,
    updated_at = NOW()
"#;

#[derive(Debug, Clone, FromRow)]
pub struct ProviderCandidateRow {
    pub id: Uuid,
    pub chain_id: Uuid,
    pub provider_type: String,
    pub name: String,
    pub base_url: String,
    pub api_key_ref: Option<String>,
    pub priority: i32,
    pub qps_limit: i32,
    pub timeout_ms: i32,
    pub status: String,
    pub consecutive_failures: Option<i32>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub disabled_until: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderHealthRow {
    pub provider_id: Uuid,
    pub consecutive_failures: i32,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub disabled_until: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCandidate {
    pub provider: Provider,
    pub health: Option<ProviderHealthRow>,
}
```

Add helper functions:

```rust
impl From<ProviderCandidateRow> for ProviderCandidate {
    fn from(row: ProviderCandidateRow) -> Self {
        let health = row.consecutive_failures.map(|consecutive_failures| ProviderHealthRow {
            provider_id: row.id,
            consecutive_failures,
            last_success_at: row.last_success_at,
            last_failure_at: row.last_failure_at,
            disabled_until: row.disabled_until,
            last_error: row.last_error.clone(),
        });

        Self {
            provider: Provider {
                id: row.id,
                chain_id: row.chain_id,
                provider_type: row.provider_type,
                name: row.name,
                base_url: row.base_url,
                api_key_ref: row.api_key_ref,
                priority: row.priority,
                qps_limit: row.qps_limit,
                timeout_ms: row.timeout_ms,
                status: row.status,
            },
            health,
        }
    }
}

pub fn provider_candidate_health(
    candidate: &ProviderCandidate,
    now: DateTime<Utc>,
) -> ProviderHealthStatus {
    let Some(health) = &candidate.health else {
        return ProviderHealthStatus {
            consecutive_failures: 0,
            last_success_at: None,
            last_failure_at: None,
            disabled_until: None,
            last_error: None,
            is_circuit_open: false,
        };
    };

    ProviderHealthStatus {
        consecutive_failures: health.consecutive_failures,
        last_success_at: health.last_success_at,
        last_failure_at: health.last_failure_at,
        disabled_until: health.disabled_until,
        last_error: health.last_error.clone(),
        is_circuit_open: is_provider_circuit_open(health.disabled_until, now),
    }
}

pub fn provider_qps_permits(current_count: i64, qps_limit: i32) -> bool {
    qps_limit > 0 && current_count <= i64::from(qps_limit)
}
```

- [ ] **Step 4: Add async storage and Redis helpers**

Add repository functions after the pure helpers:

```rust
pub async fn active_rpc_provider_candidates(
    pool: &PgPool,
    chain_id: Uuid,
    now: DateTime<Utc>,
) -> AppResult<Vec<ProviderCandidate>> {
    sqlx::query_as::<_, ProviderCandidateRow>(ACTIVE_RPC_PROVIDER_CANDIDATES_QUERY)
        .bind(chain_id)
        .bind(now)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
        .map(|rows| rows.into_iter().map(ProviderCandidate::from).collect())
}

pub async fn record_provider_success(
    pool: &PgPool,
    provider_id: Uuid,
    now: DateTime<Utc>,
) -> AppResult<()> {
    sqlx::query(RECORD_PROVIDER_SUCCESS_QUERY)
        .bind(provider_id)
        .bind(now)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    Ok(())
}

pub async fn record_provider_failure(
    pool: &PgPool,
    provider_id: Uuid,
    now: DateTime<Utc>,
    error: &AppError,
) -> AppResult<()> {
    let sanitized = sanitize_provider_error(&error.to_string());
    let disabled_until = now + Duration::seconds(PROVIDER_CIRCUIT_COOLDOWN_SECONDS);

    sqlx::query(RECORD_PROVIDER_FAILURE_QUERY)
        .bind(provider_id)
        .bind(now)
        .bind(sanitized)
        .bind(PROVIDER_CIRCUIT_FAILURE_THRESHOLD)
        .bind(disabled_until)
        .execute(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    Ok(())
}

pub async fn try_acquire_provider_qps(
    redis: &mut MultiplexedConnection,
    provider_id: Uuid,
    qps_limit: i32,
    now: DateTime<Utc>,
) -> AppResult<bool> {
    let key = provider_qps_key(provider_id, now.timestamp());
    let current_count: i64 = redis::cmd("INCR")
        .arg(&key)
        .query_async(redis)
        .await
        .map_err(|error| AppError::Redis(error.to_string()))?;

    let _: bool = redis::cmd("EXPIRE")
        .arg(&key)
        .arg(2)
        .query_async(redis)
        .await
        .map_err(|error| AppError::Redis(error.to_string()))?;

    Ok(provider_qps_permits(current_count, qps_limit))
}
```

- [ ] **Step 5: Run provider health tests to verify they pass**

Run:

```bash
cargo test -p coin-listener-storage provider_health --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 6: Commit Task 3**

```bash
git add backend/crates/storage/src/provider_health.rs
git commit -m "Add provider health repository helpers"
```

---

### Task 4: Expose provider health in system status

**Files:**
- Modify: `backend/crates/storage/src/system_status.rs`
- Modify: `backend/crates/core/src/models.rs`

- [ ] **Step 1: Write failing system status query tests**

In `backend/crates/storage/src/system_status.rs`, extend the test imports to include `PROVIDER_STATUS_DEFAULT_FAILURES`, then add these tests inside the existing `tests` module:

```rust
#[test]
fn provider_items_query_includes_runtime_health_left_join() {
    assert!(PROVIDER_ITEMS_QUERY.contains("LEFT JOIN provider_health ph"));
    assert!(PROVIDER_ITEMS_QUERY.contains("COALESCE(ph.consecutive_failures, 0)"));
    assert!(PROVIDER_ITEMS_QUERY.contains("last_success_at"));
    assert!(PROVIDER_ITEMS_QUERY.contains("last_failure_at"));
    assert!(PROVIDER_ITEMS_QUERY.contains("disabled_until"));
    assert!(PROVIDER_ITEMS_QUERY.contains("last_error"));
}

#[test]
fn provider_status_defaults_missing_health_to_zero_failures() {
    assert_eq!(PROVIDER_STATUS_DEFAULT_FAILURES, 0);
}
```

- [ ] **Step 2: Run system status tests to verify they fail**

Run:

```bash
cargo test -p coin-listener-storage provider_items_query_includes_runtime_health_left_join --manifest-path backend/Cargo.toml
```

Expected: FAIL because `PROVIDER_ITEMS_QUERY` does not include provider health fields.

- [ ] **Step 3: Extend provider status rows and query**

In `backend/crates/storage/src/system_status.rs`, update imports:

```rust
use coin_listener_core::{
    models::{
        EventStatus, NotificationStatus, ProviderChainStatus, ProviderHealthStatus,
        ProviderStatus, ProviderStatusItem, ScanStatus,
    },
    AppError, AppResult,
};
```

Add this constant near `NOTIFICATION_STATUS_STALE_MINUTES`:

```rust
pub const PROVIDER_STATUS_DEFAULT_FAILURES: i32 = 0;
```

Replace `PROVIDER_ITEMS_QUERY` with:

```rust
pub const PROVIDER_ITEMS_QUERY: &str = r#"
    SELECT
        p.id,
        p.chain_id,
        c.name AS chain_name,
        p.provider_type,
        p.name,
        p.base_url,
        p.priority,
        p.qps_limit,
        p.timeout_ms,
        p.status,
        COALESCE(ph.consecutive_failures, 0) AS consecutive_failures,
        ph.last_success_at,
        ph.last_failure_at,
        ph.disabled_until,
        ph.last_error
    FROM providers p
    JOIN chains c ON c.id = p.chain_id
    LEFT JOIN provider_health ph ON ph.provider_id = p.id
    ORDER BY c.name ASC, p.priority ASC, p.name ASC
"#;
```

Extend `ProviderStatusItemRow`:

```rust
consecutive_failures: i32,
last_success_at: Option<DateTime<Utc>>,
last_failure_at: Option<DateTime<Utc>>,
disabled_until: Option<DateTime<Utc>>,
last_error: Option<String>,
```

In the `ProviderStatusItem` mapping, add:

```rust
health: ProviderHealthStatus {
    consecutive_failures: row.consecutive_failures,
    last_success_at: row.last_success_at,
    last_failure_at: row.last_failure_at,
    disabled_until: row.disabled_until,
    last_error: row.last_error,
    is_circuit_open: row.disabled_until.is_some_and(|disabled_until| disabled_until > Utc::now()),
},
```

- [ ] **Step 4: Run system status tests to verify they pass**

Run:

```bash
cargo test -p coin-listener-storage provider_items_query_includes_runtime_health_left_join --manifest-path backend/Cargo.toml
cargo test -p coin-listener-storage provider_status_defaults_missing_health_to_zero_failures --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 5: Run core system status test after adding the required health field**

Run:

```bash
cargo test -p coin-listener-core system_status_round_trips_as_json --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 6: Commit Task 4**

```bash
git add backend/crates/storage/src/system_status.rs backend/crates/core/src/models.rs
git commit -m "Expose provider health in system status"
```

---

### Task 5: Add worker failover classification helpers

**Files:**
- Modify: `backend/crates/worker/src/lib.rs`

- [ ] **Step 1: Write failing worker helper tests**

In `backend/crates/worker/src/lib.rs`, add this test module inside `#[cfg(test)] mod tests`:

```rust
mod provider_failover_helpers {
    use coin_listener_core::{models::Provider, AppError};
    use uuid::Uuid;

    use crate::{
        is_provider_availability_error, provider_capacity_error, provider_timeout_duration,
    };

    #[test]
    fn config_errors_are_provider_availability_errors() {
        assert!(is_provider_availability_error(&AppError::Config(
            "provider request failed: timeout".to_string()
        )));
    }

    #[test]
    fn validation_database_and_redis_errors_do_not_fallback() {
        assert!(!is_provider_availability_error(&AppError::Validation(
            "bad decoded data".to_string()
        )));
        assert!(!is_provider_availability_error(&AppError::Database(
            "db unavailable".to_string()
        )));
        assert!(!is_provider_availability_error(&AppError::Redis(
            "redis unavailable".to_string()
        )));
    }

    #[test]
    fn provider_capacity_error_names_chain() {
        let error = provider_capacity_error(Uuid::from_u128(7));

        assert!(matches!(error, AppError::Config(message) if message.contains("no active rpc provider capacity for chain 00000000-0000-0000-0000-000000000007")));
    }

    #[test]
    fn provider_timeout_duration_rejects_zero_or_negative_values() {
        let provider = provider_with_timeout(0);
        let error = provider_timeout_duration(&provider).unwrap_err();
        assert!(matches!(error, AppError::Validation(message) if message == "timeout_ms must be positive"));

        let provider = provider_with_timeout(-1);
        let error = provider_timeout_duration(&provider).unwrap_err();
        assert!(matches!(error, AppError::Validation(message) if message == "timeout_ms must be positive"));
    }

    #[test]
    fn provider_timeout_duration_accepts_positive_values() {
        let provider = provider_with_timeout(1500);

        assert_eq!(provider_timeout_duration(&provider).unwrap().as_millis(), 1500);
    }

    fn provider_with_timeout(timeout_ms: i32) -> Provider {
        Provider {
            id: Uuid::from_u128(1),
            chain_id: Uuid::from_u128(2),
            provider_type: "rpc".to_string(),
            name: "provider".to_string(),
            base_url: "https://example.invalid".to_string(),
            api_key_ref: None,
            priority: 1,
            qps_limit: 10,
            timeout_ms,
            status: "active".to_string(),
        }
    }
}
```

- [ ] **Step 2: Run helper tests to verify they fail**

Run:

```bash
cargo test -p worker provider_failover_helpers --manifest-path backend/Cargo.toml
```

Expected: FAIL with missing helper functions.

- [ ] **Step 3: Add minimal worker helpers**

Near the other worker helper functions in `backend/crates/worker/src/lib.rs`, add:

```rust
pub fn is_provider_availability_error(error: &AppError) -> bool {
    matches!(error, AppError::Config(_))
}

pub fn provider_capacity_error(chain_id: uuid::Uuid) -> AppError {
    AppError::Config(format!("no active rpc provider capacity for chain {chain_id}"))
}

pub fn provider_timeout_duration(provider: &Provider) -> AppResult<Duration> {
    let timeout_ms = u64::try_from(provider.timeout_ms)
        .map_err(|_| AppError::Validation("timeout_ms must be positive".to_string()))?;
    if timeout_ms == 0 {
        return Err(AppError::Validation(
            "timeout_ms must be positive".to_string(),
        ));
    }
    Ok(Duration::from_millis(timeout_ms))
}
```

- [ ] **Step 4: Replace duplicate timeout parsing in current scan functions**

In `scan_evm_native_balance`, `scan_evm_address`, `scan_tron_address`, and `scan_btc_address`, replace the repeated `u64::try_from(provider.timeout_ms)` blocks with:

```rust
let timeout = provider_timeout_duration(&provider)?;
```

Then update client creation to use `timeout`:

```rust
let rpc = EvmRpcClient::new(provider.base_url.clone(), timeout);
let client = TronClient::new(provider.base_url.clone(), timeout);
let client = BtcClient::new(provider.base_url.clone(), timeout);
```

- [ ] **Step 5: Run helper tests to verify they pass**

Run:

```bash
cargo test -p worker provider_failover_helpers --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 6: Commit Task 5**

```bash
git add backend/crates/worker/src/lib.rs
git commit -m "Add provider failover worker helpers"
```

---

### Task 6: Route worker scan processing through Redis-aware scan functions

**Files:**
- Modify: `backend/crates/worker/src/lib.rs`

- [ ] **Step 1: Write failing source-level worker integration tests**

Add these tests inside `#[cfg(test)] mod tests` in `backend/crates/worker/src/lib.rs`:

```rust
#[test]
fn process_locked_scan_task_receives_redis_for_provider_qps() {
    let source = include_str!("lib.rs");

    assert!(source.contains("process_locked_scan_task(pool, redis, &task, now).await"));
    assert!(source.contains("redis: &mut MultiplexedConnection"));
}

#[test]
fn scan_entrypoints_receive_redis_for_provider_qps() {
    let source = include_str!("lib.rs");

    assert!(source.contains("scan_evm_address(\n    pool: &PgPool,\n    redis: &mut MultiplexedConnection,"));
    assert!(source.contains("scan_tron_address(\n    pool: &PgPool,\n    redis: &mut MultiplexedConnection,"));
    assert!(source.contains("scan_btc_address(\n    pool: &PgPool,\n    redis: &mut MultiplexedConnection,"));
}
```

- [ ] **Step 2: Run worker integration tests to verify they fail**

Run:

```bash
cargo test -p worker process_locked_scan_task_receives_redis_for_provider_qps --manifest-path backend/Cargo.toml
```

Expected: FAIL because `process_locked_scan_task` and scan entrypoints do not yet accept Redis.

- [ ] **Step 3: Update process scan signatures**

In `process_scan_task`, replace:

```rust
let outcome = process_locked_scan_task(pool, &task, now).await;
```

with:

```rust
let outcome = process_locked_scan_task(pool, redis, &task, now).await;
```

Change `process_locked_scan_task` signature to:

```rust
async fn process_locked_scan_task(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    task: &ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<ScanTaskOutcome> {
```

Inside its match arms, update calls:

```rust
let _events = scan_evm_address(pool, redis, task, now).await?;
let _events = scan_tron_address(pool, redis, task, now).await?;
let _events = scan_btc_address(pool, redis, task, now).await?;
```

Change public scan entrypoint signatures to:

```rust
pub async fn scan_evm_address(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    task: &ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<Vec<AddressEvent>> {
```

```rust
pub async fn scan_tron_address(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    task: &ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<Vec<AddressEvent>> {
```

```rust
pub async fn scan_btc_address(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    task: &ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<Vec<AddressEvent>> {
```

Keep the new `redis` and `now` parameters unused for the moment by naming them `_redis` and `_now` if the compiler requires it during this intermediate task.

- [ ] **Step 4: Run worker integration tests to verify they pass**

Run:

```bash
cargo test -p worker process_locked_scan_task_receives_redis_for_provider_qps --manifest-path backend/Cargo.toml
cargo test -p worker scan_entrypoints_receive_redis_for_provider_qps --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 5: Commit Task 6**

```bash
git add backend/crates/worker/src/lib.rs
git commit -m "Pass Redis into worker scan paths"
```

---

### Task 7: Implement provider candidate failover in EVM scans

**Files:**
- Modify: `backend/crates/worker/src/lib.rs`

- [ ] **Step 1: Write failing EVM failover source tests**

Add these tests inside `#[cfg(test)] mod tests` in `backend/crates/worker/src/lib.rs`:

```rust
#[test]
fn evm_scan_uses_provider_candidates_and_health_records() {
    let source = include_str!("lib.rs");

    assert!(source.contains("active_rpc_provider_candidates(pool, context.chain_id, now).await?"));
    assert!(source.contains("try_acquire_provider_qps(redis, provider.id, provider.qps_limit, now).await?"));
    assert!(source.contains("record_provider_success(pool, provider.id, now).await"));
    assert!(source.contains("record_provider_failure(pool, provider.id, now, &error).await"));
}

#[test]
fn evm_scan_has_with_provider_function_for_single_candidate_attempt() {
    let source = include_str!("lib.rs");

    assert!(source.contains("scan_evm_address_with_provider"));
    assert!(source.contains("provider_capacity_error(context.chain_id)"));
}
```

- [ ] **Step 2: Run EVM failover tests to verify they fail**

Run:

```bash
cargo test -p worker evm_scan_uses_provider_candidates_and_health_records --manifest-path backend/Cargo.toml
```

Expected: FAIL because EVM scan still fetches a single active provider.

- [ ] **Step 3: Import provider health helpers**

Update worker imports:

```rust
use coin_listener_storage::{
    provider_health::{
        active_rpc_provider_candidates, record_provider_failure, record_provider_success,
        try_acquire_provider_qps,
    },
    repositories,
    scan_queue::ScanQueue,
};
```

- [ ] **Step 4: Split the existing EVM implementation into a supplied-provider function**

Replace the body of `scan_evm_address` with a candidate loop in the next step. First create this function by moving the current single-provider scan body into it and removing the provider lookup:

```rust
pub async fn scan_evm_address_with_provider(
    pool: &PgPool,
    task: &ScanAddressTask,
    provider: &Provider,
) -> AppResult<Vec<AddressEvent>> {
    let context = repositories::get_scan_address_context(pool, task.address_id).await?;
    let chain = repositories::chain_by_id(pool, context.chain_id).await?;
    let native_asset = repositories::native_asset_for_chain(pool, context.chain_id).await?;
    let timeout = provider_timeout_duration(provider)?;
    let rpc = EvmRpcClient::new(provider.base_url.clone(), timeout);
    let latest_block = rpc.eth_block_number().await?;

    let mut events = Vec::new();
    if let Some(event) = scan_evm_native_balance_with_context(
        pool,
        &rpc,
        &context,
        &native_asset,
        provider,
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

- [ ] **Step 5: Implement EVM candidate loop**

Replace `scan_evm_address` with:

```rust
pub async fn scan_evm_address(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    task: &ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<Vec<AddressEvent>> {
    let context = repositories::get_scan_address_context(pool, task.address_id).await?;
    let candidates = active_rpc_provider_candidates(pool, context.chain_id, now).await?;
    if candidates.is_empty() {
        return Err(provider_capacity_error(context.chain_id));
    }

    let mut last_provider_error = None;
    for candidate in candidates {
        let provider = candidate.provider;
        if !try_acquire_provider_qps(redis, provider.id, provider.qps_limit, now).await? {
            info!(provider_id = %provider.id, chain_id = %context.chain_id, "provider qps limit reached");
            continue;
        }

        match scan_evm_address_with_provider(pool, task, &provider).await {
            Ok(events) => {
                if let Err(error) = record_provider_success(pool, provider.id, now).await {
                    warn!(provider_id = %provider.id, error = %error, "failed to record provider success");
                }
                return Ok(events);
            }
            Err(error) if is_provider_availability_error(&error) => {
                if let Err(write_error) = record_provider_failure(pool, provider.id, now, &error).await {
                    warn!(provider_id = %provider.id, error = %write_error, "failed to record provider failure");
                }
                last_provider_error = Some(error);
            }
            Err(error) => return Err(error),
        }
    }

    Err(last_provider_error.unwrap_or_else(|| provider_capacity_error(context.chain_id)))
}
```

After this step, `scan_evm_address` must no longer call `repositories::active_rpc_provider_for_chain`; only `scan_evm_native_balance` may still contain the legacy lookup until Task 9 removes that unused helper.

- [ ] **Step 6: Run EVM failover tests to verify they pass**

Run:

```bash
cargo test -p worker evm_scan_uses_provider_candidates_and_health_records --manifest-path backend/Cargo.toml
cargo test -p worker evm_scan_has_with_provider_function_for_single_candidate_attempt --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 7: Commit Task 7**

```bash
git add backend/crates/worker/src/lib.rs
git commit -m "Add EVM provider failover"
```

---

### Task 8: Implement provider candidate failover in TRON and BTC scans

**Files:**
- Modify: `backend/crates/worker/src/lib.rs`

- [ ] **Step 1: Write failing TRON/BTC failover source tests**

Add these tests inside `#[cfg(test)] mod tests` in `backend/crates/worker/src/lib.rs`:

```rust
#[test]
fn tron_and_btc_scans_have_with_provider_functions() {
    let source = include_str!("lib.rs");

    assert!(source.contains("scan_tron_address_with_provider"));
    assert!(source.contains("scan_btc_address_with_provider"));
}

#[test]
fn tron_and_btc_scan_entrypoints_record_provider_health() {
    let source = include_str!("lib.rs");

    let tron_start = source.find("pub async fn scan_tron_address(").expect("tron scan function");
    let btc_start = source.find("pub async fn scan_btc_address(").expect("btc scan function");
    let tron_body = &source[tron_start..btc_start];
    let btc_body = &source[btc_start..];

    assert!(tron_body.contains("active_rpc_provider_candidates(pool, context.chain_id, now).await?"));
    assert!(tron_body.contains("try_acquire_provider_qps(redis, provider.id, provider.qps_limit, now).await?"));
    assert!(tron_body.contains("record_provider_success(pool, provider.id, now).await"));
    assert!(tron_body.contains("record_provider_failure(pool, provider.id, now, &error).await"));

    assert!(btc_body.contains("active_rpc_provider_candidates(pool, context.chain_id, now).await?"));
    assert!(btc_body.contains("try_acquire_provider_qps(redis, provider.id, provider.qps_limit, now).await?"));
    assert!(btc_body.contains("record_provider_success(pool, provider.id, now).await"));
    assert!(btc_body.contains("record_provider_failure(pool, provider.id, now, &error).await"));
}
```

- [ ] **Step 2: Run TRON/BTC tests to verify they fail**

Run:

```bash
cargo test -p worker tron_and_btc_scans_have_with_provider_functions --manifest-path backend/Cargo.toml
```

Expected: FAIL because TRON and BTC scans still use one provider inline.

- [ ] **Step 3: Split TRON implementation into a supplied-provider function**

Create `scan_tron_address_with_provider` with the current TRON scan body and a supplied `provider`:

```rust
pub async fn scan_tron_address_with_provider(
    pool: &PgPool,
    task: &ScanAddressTask,
    provider: &Provider,
) -> AppResult<Vec<AddressEvent>> {
    let context = repositories::get_scan_address_context(pool, task.address_id).await?;
    let native_asset = repositories::native_asset_for_chain(pool, context.chain_id).await?;
    let timeout = provider_timeout_duration(provider)?;
    let client = TronClient::new(provider.base_url.clone(), timeout);

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
        ensure_provider_page_limit(
            "TRON account transactions",
            trx_pages_processed,
            has_next_page,
        )?;

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

    let trx_cursor_value = tron_cursor_value(&trx_transfers);

    let trc20_assets =
        repositories::active_assets_for_chain_by_type(pool, context.chain_id, "trc20").await?;
    let trc20_cursor =
        repositories::scan_cursor(pool, context.id, TRON_TRC20_TRANSFER_CURSOR).await?;
    let trc20_from = trc20_cursor
        .as_ref()
        .map(|cursor| cursor.last_scanned_block + 1)
        .unwrap_or(0);
    let mut trc20_cursor_value: Option<i64> = None;
    let mut trc20_transfers = Vec::new();
    for asset in trc20_assets {
        let Some(contract_address) = asset.contract_address.clone() else {
            continue;
        };
        let mut trc20_fingerprint: Option<String> = None;
        let mut trc20_pages_processed = 0usize;

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
            ensure_provider_page_limit(
                "TRON TRC20 transfers",
                trc20_pages_processed,
                has_next_page,
            )?;

            for (index, payload) in data.into_iter().enumerate() {
                let transfer = tron::decode_trc20_transfer_at_index(
                    &payload,
                    &contract_address,
                    asset.decimals,
                    paged_log_index(page_index, index)?,
                )?;
                collect_matching_tron_transfer(
                    &asset,
                    transfer,
                    &mut trc20_cursor_value,
                    &mut trc20_transfers,
                );
            }

            let Some(next) = next_fingerprint else {
                break;
            };
            trc20_fingerprint = Some(next);
        }
    }

    let mut events = Vec::new();
    for transfer in trx_transfers {
        if !tron_transfer_should_scan(&native_asset, &transfer) {
            continue;
        }
        let draft = tron::tron_transfer_event_draft(&context, &native_asset, transfer);
        if let Some(event) =
            repositories::insert_event_and_outbox_if_not_exists(pool, draft).await?
        {
            events.push(event);
        }
    }
    for (asset, transfer) in trc20_transfers {
        let draft = tron::tron_transfer_event_draft(&context, &asset, transfer);
        if let Some(event) =
            repositories::insert_event_and_outbox_if_not_exists(pool, draft).await?
        {
            events.push(event);
        }
    }

    if let Some(cursor_value) = trx_cursor_value {
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
    if let Some(cursor_value) = trc20_cursor_value {
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

- [ ] **Step 4: Replace TRON entrypoint with candidate loop**

Replace `scan_tron_address` with:

```rust
pub async fn scan_tron_address(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    task: &ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<Vec<AddressEvent>> {
    let context = repositories::get_scan_address_context(pool, task.address_id).await?;
    let candidates = active_rpc_provider_candidates(pool, context.chain_id, now).await?;
    if candidates.is_empty() {
        return Err(provider_capacity_error(context.chain_id));
    }

    let mut last_provider_error = None;
    for candidate in candidates {
        let provider = candidate.provider;
        if !try_acquire_provider_qps(redis, provider.id, provider.qps_limit, now).await? {
            info!(provider_id = %provider.id, chain_id = %context.chain_id, "provider qps limit reached");
            continue;
        }

        match scan_tron_address_with_provider(pool, task, &provider).await {
            Ok(events) => {
                if let Err(error) = record_provider_success(pool, provider.id, now).await {
                    warn!(provider_id = %provider.id, error = %error, "failed to record provider success");
                }
                return Ok(events);
            }
            Err(error) if is_provider_availability_error(&error) => {
                if let Err(write_error) = record_provider_failure(pool, provider.id, now, &error).await {
                    warn!(provider_id = %provider.id, error = %write_error, "failed to record provider failure");
                }
                last_provider_error = Some(error);
            }
            Err(error) => return Err(error),
        }
    }

    Err(last_provider_error.unwrap_or_else(|| provider_capacity_error(context.chain_id)))
}
```

- [ ] **Step 5: Split BTC implementation into a supplied-provider function**

Create `scan_btc_address_with_provider` with the current BTC scan body and a supplied `provider`:

```rust
pub async fn scan_btc_address_with_provider(
    pool: &PgPool,
    task: &ScanAddressTask,
    provider: &Provider,
) -> AppResult<Vec<AddressEvent>> {
    let context = repositories::get_scan_address_context(pool, task.address_id).await?;
    let native_asset = repositories::native_asset_for_chain(pool, context.chain_id).await?;
    let timeout = provider_timeout_duration(provider)?;
    let client = BtcClient::new(provider.base_url.clone(), timeout);
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
    let from_block = btc_scan_from_block(cursor.as_ref());
    let mut events = Vec::new();
    let mut transfers = Vec::new();
    let mut next_last_seen_txid: Option<String> = None;
    let mut pages_processed = 0usize;

    loop {
        let page = client
            .address_transactions_page(&context.address, next_last_seen_txid.as_deref())
            .await?;
        if !should_process_btc_transaction_page(pages_processed, page.transactions.len())? {
            break;
        }
        pages_processed += 1;

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

    let cursor_value = btc_cursor_value(&transfers);
    for transfer in transfers {
        let draft = btc::btc_transfer_event_draft(&context, &native_asset, transfer);
        if let Some(event) =
            repositories::insert_event_and_outbox_if_not_exists(pool, draft).await?
        {
            events.push(event);
        }
    }

    if let Some(cursor_value) = cursor_value {
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

- [ ] **Step 6: Replace BTC entrypoint with candidate loop**

Replace `scan_btc_address` with:

```rust
pub async fn scan_btc_address(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    task: &ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<Vec<AddressEvent>> {
    let context = repositories::get_scan_address_context(pool, task.address_id).await?;
    let candidates = active_rpc_provider_candidates(pool, context.chain_id, now).await?;
    if candidates.is_empty() {
        return Err(provider_capacity_error(context.chain_id));
    }

    let mut last_provider_error = None;
    for candidate in candidates {
        let provider = candidate.provider;
        if !try_acquire_provider_qps(redis, provider.id, provider.qps_limit, now).await? {
            info!(provider_id = %provider.id, chain_id = %context.chain_id, "provider qps limit reached");
            continue;
        }

        match scan_btc_address_with_provider(pool, task, &provider).await {
            Ok(events) => {
                if let Err(error) = record_provider_success(pool, provider.id, now).await {
                    warn!(provider_id = %provider.id, error = %error, "failed to record provider success");
                }
                return Ok(events);
            }
            Err(error) if is_provider_availability_error(&error) => {
                if let Err(write_error) = record_provider_failure(pool, provider.id, now, &error).await {
                    warn!(provider_id = %provider.id, error = %write_error, "failed to record provider failure");
                }
                last_provider_error = Some(error);
            }
            Err(error) => return Err(error),
        }
    }

    Err(last_provider_error.unwrap_or_else(|| provider_capacity_error(context.chain_id)))
}
```

- [ ] **Step 7: Run TRON/BTC failover tests to verify they pass**

Run:

```bash
cargo test -p worker tron_and_btc_scans_have_with_provider_functions --manifest-path backend/Cargo.toml
cargo test -p worker tron_and_btc_scan_entrypoints_record_provider_health --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 8: Run worker tests to catch refactor regressions**

Run:

```bash
cargo test -p worker --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 9: Commit Task 8**

```bash
git add backend/crates/worker/src/lib.rs
git commit -m "Add TRON and BTC provider failover"
```

---

### Task 9: Remove legacy single-provider worker dependency from scan paths

**Files:**
- Modify: `backend/crates/worker/src/lib.rs`

- [ ] **Step 1: Write failing regression test for worker scan provider selection**

Add this test inside `#[cfg(test)] mod tests` in `backend/crates/worker/src/lib.rs`:

```rust
#[test]
fn worker_scan_entrypoints_do_not_call_single_active_provider_lookup() {
    let source = include_str!("lib.rs");

    assert!(!source.contains("active_rpc_provider_for_chain(pool, context.chain_id).await?"));
    assert!(source.matches("active_rpc_provider_candidates(pool, context.chain_id, now).await?").count() >= 3);
}
```

- [ ] **Step 2: Run regression test to verify it fails if any legacy lookup remains**

Run:

```bash
cargo test -p worker worker_scan_entrypoints_do_not_call_single_active_provider_lookup --manifest-path backend/Cargo.toml
```

Expected before cleanup: FAIL if `scan_evm_native_balance` or any scan entrypoint still contains the legacy lookup string.

- [ ] **Step 3: Remove the unused legacy EVM native balance entrypoint**

Delete the entire `scan_evm_native_balance` function from `backend/crates/worker/src/lib.rs`:

```rust
pub async fn scan_evm_native_balance(
    pool: &PgPool,
    task: &ScanAddressTask,
    _now: DateTime<Utc>,
) -> AppResult<Option<AddressEvent>> {
    let context = repositories::get_scan_address_context(pool, task.address_id).await?;
    let asset = repositories::native_asset_for_chain(pool, context.chain_id).await?;
    let provider = repositories::active_rpc_provider_for_chain(pool, context.chain_id).await?;
    let timeout = provider_timeout_duration(&provider)?;

    let rpc = EvmRpcClient::new(provider.base_url.clone(), timeout);
    let block_number = rpc.eth_block_number().await?;
    scan_evm_native_balance_with_context(pool, &rpc, &context, &asset, &provider, block_number)
        .await
}
```

Do not delete `scan_evm_native_balance_with_context`; `scan_evm_address_with_provider` still uses it. The final worker source must satisfy:

```rust
assert!(!include_str!("lib.rs").contains("active_rpc_provider_for_chain(pool, context.chain_id).await?"));
```

- [ ] **Step 4: Run regression test to verify it passes**

Run:

```bash
cargo test -p worker worker_scan_entrypoints_do_not_call_single_active_provider_lookup --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 5: Run worker package tests**

Run:

```bash
cargo test -p worker --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 6: Commit Task 9**

```bash
git add backend/crates/worker/src/lib.rs
git commit -m "Remove single-provider worker scan lookup"
```

---

### Task 10: Add frontend provider health types and system status columns

**Files:**
- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/pages/SystemStatusPage.tsx`

- [ ] **Step 1: Add frontend provider health types**

In `frontend/src/api/types.ts`, add this type above `ProviderStatusItem`:

```ts
export type ProviderHealthStatus = {
  consecutive_failures: number;
  last_success_at?: string | null;
  last_failure_at?: string | null;
  disabled_until?: string | null;
  last_error?: string | null;
  is_circuit_open: boolean;
};
```

Extend `ProviderStatusItem` with:

```ts
health: ProviderHealthStatus;
```

- [ ] **Step 2: Add UI helpers**

In `frontend/src/pages/SystemStatusPage.tsx`, add these helpers near `statusColor`:

```tsx
function circuitStatusColor(isOpen: boolean): 'red' | 'green' {
  return isOpen ? 'red' : 'green';
}

function circuitStatusText(isOpen: boolean) {
  return isOpen ? 'circuit-open' : 'healthy';
}

function truncateError(value?: string | null) {
  if (!value) {
    return '-';
  }
  return value.length > 80 ? `${value.slice(0, 80)}…` : value;
}
```

- [ ] **Step 3: Update Provider detail table columns**

In the `Provider 明细` table, change `scroll={{ x: 1000 }}` to:

```tsx
scroll={{ x: 1500 }}
```

Replace the current status column:

```tsx
{ title: '状态', dataIndex: 'status', width: 100, render: value => <Tag color={statusColor(String(value))}>{String(value)}</Tag> },
```

with:

```tsx
{
  title: '配置状态',
  dataIndex: 'status',
  width: 110,
  render: value => <Tag color={statusColor(String(value))}>{String(value)}</Tag>,
},
{
  title: '运行状态',
  dataIndex: 'health',
  width: 130,
  render: (_value, record) => {
    const item = record as ProviderStatusItem;
    return <Tag color={circuitStatusColor(item.health.is_circuit_open)}>{circuitStatusText(item.health.is_circuit_open)}</Tag>;
  },
},
{
  title: '连续失败',
  dataIndex: 'health',
  width: 110,
  render: (_value, record) => String((record as ProviderStatusItem).health.consecutive_failures),
},
{
  title: '最后成功',
  dataIndex: 'health',
  width: 190,
  render: (_value, record) => formatTime((record as ProviderStatusItem).health.last_success_at),
},
{
  title: '最后失败',
  dataIndex: 'health',
  width: 190,
  render: (_value, record) => formatTime((record as ProviderStatusItem).health.last_failure_at),
},
{
  title: '禁用至',
  dataIndex: 'health',
  width: 190,
  render: (_value, record) => formatTime((record as ProviderStatusItem).health.disabled_until),
},
{
  title: '最后错误',
  dataIndex: 'health',
  width: 260,
  ellipsis: { showTitle: true },
  render: (_value, record) => truncateError((record as ProviderStatusItem).health.last_error),
},
```

Keep the existing columns for chain, name, type, priority, QPS, timeout, and URL.

- [ ] **Step 4: Run frontend build to verify TypeScript**

Run:

```bash
npm run build --prefix frontend
```

Expected: PASS. Existing Vite warnings about `lottie-web` direct eval or chunk size are acceptable if the build exits 0.

- [ ] **Step 5: Commit Task 10**

```bash
git add frontend/src/api/types.ts frontend/src/pages/SystemStatusPage.tsx
git commit -m "Show provider health in system status"
```

---

### Task 11: Run final formatting, backend, frontend, and migration verification

**Files:**
- Verify only unless formatting changes are produced by `cargo fmt`.

- [ ] **Step 1: Run Rust formatting check**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 2: If formatting fails, apply formatting and commit**

Run only if Step 1 reports formatting diffs:

```bash
cargo fmt --all --manifest-path backend/Cargo.toml
git add backend
git commit -m "Format provider resilience backend"
```

Then rerun:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 3: Run backend compile check**

Run:

```bash
cargo check --workspace --manifest-path backend/Cargo.toml
```

Expected: PASS. Existing future-incompatibility warnings from dependencies are acceptable if the command exits 0.

- [ ] **Step 4: Run backend tests**

Run:

```bash
cargo test --workspace --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 5: Run frontend build**

Run:

```bash
npm run build --prefix frontend
```

Expected: PASS. Existing Vite warnings about `lottie-web` direct eval or chunk size are acceptable if the command exits 0.

- [ ] **Step 6: Verify unrelated package-lock diff is not staged**

Run:

```bash
git status --short
```

Expected: no staged `frontend/package-lock.json`. If `frontend/package-lock.json` is modified from earlier unrelated work, leave it unstaged.

- [ ] **Step 7: Commit final verification notes only if files changed**

If Step 2 created a formatting commit, no extra commit is needed. If no files changed during final verification, do not create an empty commit.

---

## Self-Review Checklist

- Spec coverage:
  - Data model: Task 2 creates `provider_health` migration and indexes.
  - Core DTOs: Task 1 adds `ProviderHealthStatus` and extends `ProviderStatusItem`.
  - Storage helpers: Tasks 2 and 3 add constants, sanitizer, candidate query, success/failure writes, QPS key, and Redis limiter.
  - System status API shape: Task 4 extends provider item health fields through existing `/api/system/status`.
  - Worker failover: Tasks 5 through 9 classify provider availability errors, pass Redis, enforce QPS, try ordered candidates, record success/failure, and remove single-provider scan lookup.
  - Frontend display: Task 10 adds types and Semi table columns for config status, runtime circuit status, failures, last success/failure, disabled-until, and last error.
  - Verification: Task 11 runs fmt, check, tests, and frontend build.
- Placeholder scan: no placeholder markers, no incomplete sections, and no deferred implementation text.
- Type consistency: `ProviderHealthStatus`, `ProviderCandidate`, `ProviderHealthRow`, `active_rpc_provider_candidates`, `record_provider_success`, `record_provider_failure`, `try_acquire_provider_qps`, and `is_provider_availability_error` names match across tasks.
