# Coin Listener Scheduler Worker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the scheduler and worker scan queue foundation so active due watched addresses are enqueued in Redis, consumed by workers, locked per address, scanned through the existing mock EVM scanner, and rescheduled.

**Architecture:** Add shared scan task/config models in `core`, due-scan persistence functions in `storage`, and a Redis list/lock wrapper as the scheduler-worker boundary. The scheduler periodically queries PostgreSQL for active due addresses and pushes `ScanAddressTask` JSON into Redis; the worker blocks on Redis, obtains an address lock, routes EVM chains to the existing deterministic mock scanner, marks scan timestamps, and safely skips unsupported chains.

**Tech Stack:** Rust, Tokio, SQLx, PostgreSQL, Redis, Serde, Chrono, Uuid, tracing, Docker Compose.

---

## Scope

Implements `docs/superpowers/specs/2026-05-17-coin-listener-scheduler-worker-design.md`.

Included:

- `ScanAddressTask` queue message.
- Scheduler/worker scan queue environment config.
- PostgreSQL migration for due-scan scheduling defaults and index.
- Repository functions for due-address query and scan timestamp updates.
- Redis list queue wrapper and Redis address lock wrapper.
- Scheduler tick loop.
- Worker consume loop with EVM mock scan and unsupported-chain handling.
- Backend tests for serialization, time calculation, queue payload helpers, lock key helpers, scheduler task construction, and worker chain decision logic.

Not included:

- Real EVM RPC / WebSocket / Alloy integration.
- BTC/TRON providers.
- Notification delivery.
- Provider token bucket/fallback.
- Operations UI.
- Frontend changes.

## Git note

The current project directory has been observed as not being a git repository. Each task ends with a verification checkpoint instead of a required commit. If a future worker runs this plan inside a git repository, commit after the checkpoint with the exact message shown in that task.

## File Structure

Modify:

```text
backend/Cargo.toml
backend/crates/core/src/config.rs
backend/crates/core/src/lib.rs
backend/crates/core/src/models.rs
backend/crates/storage/Cargo.toml
backend/crates/storage/src/lib.rs
backend/crates/storage/src/repositories.rs
backend/crates/scheduler/Cargo.toml
backend/crates/scheduler/src/main.rs
backend/crates/worker/Cargo.toml
backend/crates/worker/src/main.rs
```

Create:

```text
backend/crates/storage/migrations/0004_scheduler_worker_scan.sql
backend/crates/storage/src/scan_queue.rs
backend/crates/scheduler/src/lib.rs
backend/crates/worker/src/lib.rs
```

## Task 1: Add shared scan task and scan config

**Files:**

- Modify: `backend/crates/core/src/models.rs`
- Modify: `backend/crates/core/src/config.rs`
- Modify: `backend/crates/core/src/lib.rs`
- Verify: `backend/crates/core/src/models.rs` unit tests

- [ ] **Step 1: Write the failing queue message serialization test**

Add this test module to the bottom of `backend/crates/core/src/models.rs` before adding `ScanAddressTask`:

```rust
#[cfg(test)]
mod tests {
    use super::ScanAddressTask;
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    #[test]
    fn scan_address_task_round_trips_as_json() {
        let task = ScanAddressTask {
            task_id: Uuid::from_u128(1),
            address_id: Uuid::from_u128(2),
            tenant_id: Uuid::from_u128(3),
            chain_id: Uuid::from_u128(4),
            attempt: 1,
            enqueued_at: Utc.with_ymd_and_hms(2026, 5, 17, 12, 0, 0).unwrap(),
        };

        let payload = serde_json::to_string(&task).expect("serialize scan task");
        let decoded: ScanAddressTask = serde_json::from_str(&payload).expect("deserialize scan task");

        assert_eq!(decoded, task);
        assert!(payload.contains("\"attempt\":1"));
    }
}
```

- [ ] **Step 2: Run the test to verify RED**

Run:

```bash
cargo test -p coin-listener-core scan_address_task_round_trips_as_json --manifest-path backend/Cargo.toml
```

Expected: FAIL with an unresolved import or missing type for `ScanAddressTask`.

- [ ] **Step 3: Add `ScanAddressTask` to shared models**

Add this struct after `EventQuery` in `backend/crates/core/src/models.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanAddressTask {
    pub task_id: Uuid,
    pub address_id: Uuid,
    pub tenant_id: Uuid,
    pub chain_id: Uuid,
    pub attempt: u16,
    pub enqueued_at: DateTime<Utc>,
}
```

- [ ] **Step 4: Add scan queue config**

Update `backend/crates/core/src/config.rs`.

Change `AppConfig` to include `scan`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub postgres: PostgresConfig,
    pub redis: RedisConfig,
    pub scan: ScanConfig,
}
```

Add this config struct after `RedisConfig`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct ScanConfig {
    pub scheduler_tick_seconds: u64,
    pub scheduler_batch_size: i64,
    pub queue_key: String,
    pub lock_ttl_seconds: usize,
}
```

Add these merges to `AppConfig::from_env()` after the `redis.redis_url` merge:

```rust
.merge((
    "scan.scheduler_tick_seconds",
    env::var("SCHEDULER_TICK_SECONDS").unwrap_or_else(|_| "30".to_string()),
))
.merge((
    "scan.scheduler_batch_size",
    env::var("SCHEDULER_BATCH_SIZE").unwrap_or_else(|_| "100".to_string()),
))
.merge((
    "scan.queue_key",
    env::var("SCAN_QUEUE_KEY").unwrap_or_else(|_| "scan:address:queue".to_string()),
))
.merge((
    "scan.lock_ttl_seconds",
    env::var("SCAN_LOCK_TTL_SECONDS").unwrap_or_else(|_| "120".to_string()),
))
```

Format the existing long `merge` calls if `cargo fmt` changes this file.

- [ ] **Step 5: Export `ScanConfig`**

Update `backend/crates/core/src/lib.rs`:

```rust
pub use config::{AppConfig, PostgresConfig, RedisConfig, ScanConfig, ServerConfig};
```

- [ ] **Step 6: Run the core test to verify GREEN**

Run:

```bash
cargo test -p coin-listener-core scan_address_task_round_trips_as_json --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 7: Checkpoint**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
cargo check -p coin-listener-core --manifest-path backend/Cargo.toml
```

Expected: both commands exit 0. If running inside git, commit with:

```bash
git add backend/crates/core/src/models.rs backend/crates/core/src/config.rs backend/crates/core/src/lib.rs
git commit -m "feat: add scan task config"
```

## Task 2: Add due-scan persistence functions

**Files:**

- Create: `backend/crates/storage/migrations/0004_scheduler_worker_scan.sql`
- Modify: `backend/crates/core/src/models.rs`
- Modify: `backend/crates/storage/Cargo.toml`
- Modify: `backend/crates/storage/src/repositories.rs`
- Verify: repository unit tests and storage check

- [ ] **Step 1: Write the failing scan time calculation test**

Add this test module to the bottom of `backend/crates/storage/src/repositories.rs` before adding `next_scan_at_from`:

```rust
#[cfg(test)]
mod tests {
    use super::next_scan_at_from;
    use chrono::{TimeZone, Utc};

    #[test]
    fn next_scan_at_from_adds_scan_interval_seconds() {
        let now = Utc.with_ymd_and_hms(2026, 5, 17, 12, 0, 0).unwrap();

        let next_scan_at = next_scan_at_from(now, 300);

        assert_eq!(next_scan_at, Utc.with_ymd_and_hms(2026, 5, 17, 12, 5, 0).unwrap());
    }
}
```

- [ ] **Step 2: Run the test to verify RED**

Run:

```bash
cargo test -p coin-listener-storage next_scan_at_from_adds_scan_interval_seconds --manifest-path backend/Cargo.toml
```

Expected: FAIL with `no next_scan_at_from in repositories` or unresolved import.

- [ ] **Step 3: Add storage dependencies**

Update `backend/crates/storage/Cargo.toml` dependencies:

```toml
[dependencies]
chrono.workspace = true
coin-listener-chain-providers = { path = "../chain-providers" }
coin-listener-core = { path = "../core" }
redis.workspace = true
serde_json.workspace = true
sqlx.workspace = true
tracing.workspace = true
uuid.workspace = true
```

- [ ] **Step 4: Create the scan scheduling migration**

Create `backend/crates/storage/migrations/0004_scheduler_worker_scan.sql`:

```sql
UPDATE watched_addresses
SET next_scan_at = NOW()
WHERE status = 'active'
  AND next_scan_at IS NULL;

ALTER TABLE watched_addresses
ALTER COLUMN next_scan_at SET DEFAULT NOW();

CREATE INDEX IF NOT EXISTS idx_watched_addresses_due_scan
ON watched_addresses(status, next_scan_at, priority);
```

This makes existing active addresses eligible for the first scheduler tick and gives newly inserted rows a scan time without changing the existing `create_watched_address` insert statement.

- [ ] **Step 5: Add scan repository row models**

Add these structs after `ScanAddressTask` in `backend/crates/core/src/models.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ScanAddressCandidate {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub chain_id: Uuid,
    pub scan_interval_seconds: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ScanAddressContext {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub chain_id: Uuid,
    pub address: String,
    pub scan_interval_seconds: i32,
    pub chain_type: String,
}
```

- [ ] **Step 6: Import scan models and chrono in repositories**

Update the imports at the top of `backend/crates/storage/src/repositories.rs`:

```rust
use chrono::{DateTime, Duration, Utc};
use coin_listener_chain_providers::evm;
use coin_listener_core::{
    models::{
        AddressEvent, AddressEventDraft, Asset, Chain, CreateProviderRequest,
        CreateWatchedAddressRequest, EventQuery, Provider, ScanAddressCandidate,
        ScanAddressContext, Tenant, User, WatchedAddress,
    },
    AppError, AppResult,
};
use sqlx::PgPool;
use uuid::Uuid;
```

- [ ] **Step 7: Implement due-scan repository functions**

Add these public functions after `create_mock_evm_event` in `backend/crates/storage/src/repositories.rs`:

```rust
pub fn next_scan_at_from(now: DateTime<Utc>, scan_interval_seconds: i32) -> DateTime<Utc> {
    now + Duration::seconds(i64::from(scan_interval_seconds))
}

pub async fn claim_one_due_scan_address_for_update(
    transaction: &mut Transaction<'_, Postgres>,
    now: DateTime<Utc>,
) -> AppResult<Option<ScanAddressCandidate>> {
    sqlx::query_as::<_, ScanAddressCandidate>(
        r#"
        SELECT id, tenant_id, chain_id, scan_interval_seconds
        FROM watched_addresses
        WHERE status = 'active'
          AND next_scan_at <= $1
        ORDER BY next_scan_at ASC, created_at ASC
        FOR UPDATE SKIP LOCKED
        LIMIT 1
        "#,
    )
    .bind(now)
    .fetch_optional(transaction.as_mut())
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn list_due_scan_addresses(
    pool: &PgPool,
    limit: i64,
) -> AppResult<Vec<ScanAddressCandidate>> {
    sqlx::query_as::<_, ScanAddressCandidate>(
        r#"
        SELECT id, tenant_id, chain_id, scan_interval_seconds
        FROM watched_addresses
        WHERE status = 'active'
          AND next_scan_at <= NOW()
        ORDER BY next_scan_at ASC, created_at ASC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn get_scan_address_context(
    pool: &PgPool,
    address_id: Uuid,
) -> AppResult<ScanAddressContext> {
    sqlx::query_as::<_, ScanAddressContext>(
        r#"
        SELECT wa.id,
               wa.tenant_id,
               wa.chain_id,
               wa.address,
               wa.scan_interval_seconds,
               c.chain_type
        FROM watched_addresses wa
        INNER JOIN chains c ON c.id = wa.chain_id
        WHERE wa.id = $1
          AND wa.status = 'active'
        "#,
    )
    .bind(address_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?
    .ok_or_else(|| AppError::NotFound("watched address".to_string()))
}

pub async fn mark_claimed_scan_enqueued(
    transaction: &mut Transaction<'_, Postgres>,
    address_id: Uuid,
    next_scan_at: DateTime<Utc>,
) -> AppResult<()> {
    let result = sqlx::query(
        r#"
        UPDATE watched_addresses
        SET next_scan_at = $2,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(address_id)
    .bind(next_scan_at)
    .execute(transaction.as_mut())
    .await
    .map_err(|error| AppError::Database(error.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("watched address".to_string()));
    }

    Ok(())
}

pub async fn finish_address_scan(
    pool: &PgPool,
    address_id: Uuid,
    last_scanned_at: DateTime<Utc>,
    next_scan_at: DateTime<Utc>,
) -> AppResult<()> {
    let result = sqlx::query(
        r#"
        UPDATE watched_addresses
        SET last_scanned_at = $2,
            next_scan_at = $3,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(address_id)
    .bind(last_scanned_at)
    .bind(next_scan_at)
    .execute(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("watched address".to_string()));
    }

    Ok(())
}
```

- [ ] **Step 8: Run the repository test to verify GREEN**

Run:

```bash
cargo test -p coin-listener-storage next_scan_at_from_adds_scan_interval_seconds --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 9: Checkpoint**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
cargo check -p coin-listener-storage --manifest-path backend/Cargo.toml
```

Expected: both commands exit 0. If running inside git, commit with:

```bash
git add backend/crates/core/src/models.rs backend/crates/storage/Cargo.toml backend/crates/storage/src/repositories.rs backend/crates/storage/migrations/0004_scheduler_worker_scan.sql
git commit -m "feat: add due scan persistence"
```

## Task 3: Add Redis scan queue wrapper

**Files:**

- Create: `backend/crates/storage/src/scan_queue.rs`
- Modify: `backend/crates/storage/src/lib.rs`
- Verify: scan queue unit tests and storage check

- [ ] **Step 1: Write failing scan queue helper tests**

Create `backend/crates/storage/src/scan_queue.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::{deserialize_scan_task, scan_lock_key, serialize_scan_task};
    use chrono::{TimeZone, Utc};
    use coin_listener_core::models::ScanAddressTask;
    use uuid::Uuid;

    #[test]
    fn scan_task_payload_round_trips() {
        let task = ScanAddressTask {
            task_id: Uuid::from_u128(11),
            address_id: Uuid::from_u128(12),
            tenant_id: Uuid::from_u128(13),
            chain_id: Uuid::from_u128(14),
            attempt: 1,
            enqueued_at: Utc.with_ymd_and_hms(2026, 5, 17, 13, 0, 0).unwrap(),
        };

        let payload = serialize_scan_task(&task).expect("serialize task");
        let decoded = deserialize_scan_task(&payload).expect("deserialize task");

        assert_eq!(decoded, task);
    }

    #[test]
    fn malformed_scan_task_payload_returns_error() {
        let result = deserialize_scan_task("not-json");

        assert!(result.is_err());
    }

    #[test]
    fn scan_lock_key_uses_address_id() {
        let address_id = Uuid::from_u128(42);

        assert_eq!(
            scan_lock_key(address_id),
            "scan:address:lock:00000000-0000-0000-0000-00000000002a"
        );
    }
}
```

- [ ] **Step 2: Export the module so the tests compile path is active**

Add this line to `backend/crates/storage/src/lib.rs`:

```rust
pub mod scan_queue;
```

- [ ] **Step 3: Run the tests to verify RED**

Run:

```bash
cargo test -p coin-listener-storage scan_task_payload_round_trips --manifest-path backend/Cargo.toml
```

Expected: FAIL with unresolved imports for the scan queue helper functions.

- [ ] **Step 4: Implement the Redis queue wrapper**

Replace `backend/crates/storage/src/scan_queue.rs` with this implementation, keeping the tests at the bottom:

```rust
use coin_listener_core::{models::ScanAddressTask, AppError, AppResult};
use redis::{aio::MultiplexedConnection, Client};
use uuid::Uuid;

const LOCK_KEY_PREFIX: &str = "scan:address:lock";
const RELEASE_LOCK_SCRIPT: &str = r#"
if redis.call("GET", KEYS[1]) == ARGV[1] then
    return redis.call("DEL", KEYS[1])
else
    return 0
end
"#;

#[derive(Debug, Clone)]
pub struct ScanQueue {
    queue_key: String,
    lock_ttl_seconds: usize,
}

impl ScanQueue {
    pub fn new(queue_key: String, lock_ttl_seconds: usize) -> Self {
        Self {
            queue_key,
            lock_ttl_seconds,
        }
    }

    pub fn queue_key(&self) -> &str {
        &self.queue_key
    }

    pub async fn enqueue(
        &self,
        connection: &mut MultiplexedConnection,
        task: &ScanAddressTask,
    ) -> AppResult<()> {
        let payload = serialize_scan_task(task)?;
        let _: usize = redis::cmd("LPUSH")
            .arg(&self.queue_key)
            .arg(payload)
            .query_async(connection)
            .await
            .map_err(|error| AppError::Redis(error.to_string()))?;
        Ok(())
    }

    pub async fn dequeue(
        &self,
        connection: &mut MultiplexedConnection,
        timeout_seconds: usize,
    ) -> AppResult<Option<ScanAddressTask>> {
        let result: Option<(String, String)> = redis::cmd("BRPOP")
            .arg(&self.queue_key)
            .arg(timeout_seconds)
            .query_async(connection)
            .await
            .map_err(|error| AppError::Redis(error.to_string()))?;

        result
            .map(|(_, payload)| deserialize_scan_task(&payload))
            .transpose()
    }

    pub async fn acquire_lock(
        &self,
        connection: &mut MultiplexedConnection,
        address_id: Uuid,
        task_id: Uuid,
    ) -> AppResult<bool> {
        let result: Option<String> = redis::cmd("SET")
            .arg(scan_lock_key(address_id))
            .arg(task_id.to_string())
            .arg("NX")
            .arg("EX")
            .arg(self.lock_ttl_seconds)
            .query_async(connection)
            .await
            .map_err(|error| AppError::Redis(error.to_string()))?;

        Ok(result.is_some())
    }

    pub async fn release_lock(
        &self,
        connection: &mut MultiplexedConnection,
        address_id: Uuid,
        task_id: Uuid,
    ) -> AppResult<bool> {
        let released: i32 = redis::Script::new(RELEASE_LOCK_SCRIPT)
            .key(scan_lock_key(address_id))
            .arg(task_id.to_string())
            .invoke_async(connection)
            .await
            .map_err(|error| AppError::Redis(error.to_string()))?;

        Ok(released > 0)
    }
}

pub async fn connect_scan_queue(client: &Client) -> AppResult<MultiplexedConnection> {
    client
        .get_multiplexed_async_connection()
        .await
        .map_err(|error| AppError::Redis(error.to_string()))
}

pub fn scan_lock_key(address_id: Uuid) -> String {
    format!("{LOCK_KEY_PREFIX}:{address_id}")
}

pub fn serialize_scan_task(task: &ScanAddressTask) -> AppResult<String> {
    serde_json::to_string(task).map_err(|error| AppError::Validation(error.to_string()))
}

pub fn deserialize_scan_task(payload: &str) -> AppResult<ScanAddressTask> {
    serde_json::from_str(payload).map_err(|error| AppError::Validation(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{deserialize_scan_task, scan_lock_key, serialize_scan_task};
    use chrono::{TimeZone, Utc};
    use coin_listener_core::models::ScanAddressTask;
    use uuid::Uuid;

    #[test]
    fn scan_task_payload_round_trips() {
        let task = ScanAddressTask {
            task_id: Uuid::from_u128(11),
            address_id: Uuid::from_u128(12),
            tenant_id: Uuid::from_u128(13),
            chain_id: Uuid::from_u128(14),
            attempt: 1,
            enqueued_at: Utc.with_ymd_and_hms(2026, 5, 17, 13, 0, 0).unwrap(),
        };

        let payload = serialize_scan_task(&task).expect("serialize task");
        let decoded = deserialize_scan_task(&payload).expect("deserialize task");

        assert_eq!(decoded, task);
    }

    #[test]
    fn malformed_scan_task_payload_returns_error() {
        let result = deserialize_scan_task("not-json");

        assert!(result.is_err());
    }

    #[test]
    fn scan_lock_key_uses_address_id() {
        let address_id = Uuid::from_u128(42);

        assert_eq!(
            scan_lock_key(address_id),
            "scan:address:lock:00000000-0000-0000-0000-00000000002a"
        );
    }
}
```

- [ ] **Step 5: Run queue tests to verify GREEN**

Run:

```bash
cargo test -p coin-listener-storage scan_ --manifest-path backend/Cargo.toml
```

Expected: PASS for the scan queue helper tests and the scan time calculation test.

- [ ] **Step 6: Checkpoint**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
cargo check -p coin-listener-storage --manifest-path backend/Cargo.toml
```

Expected: both commands exit 0. If running inside git, commit with:

```bash
git add backend/crates/storage/src/scan_queue.rs backend/crates/storage/src/lib.rs
git commit -m "feat: add redis scan queue wrapper"
```

## Task 4: Implement scheduler tick loop

**Files:**

- Modify: `backend/Cargo.toml`
- Modify: `backend/crates/scheduler/Cargo.toml`
- Modify: `backend/crates/storage/src/repositories.rs`
- Create: `backend/crates/scheduler/src/lib.rs`
- Modify: `backend/crates/scheduler/src/main.rs`
- Verify: scheduler unit tests and scheduler check

- [ ] **Step 1: Write the failing scheduler task construction test**

Create `backend/crates/scheduler/src/lib.rs` with this test first:

```rust
#[cfg(test)]
mod tests {
    use super::build_scan_task;
    use chrono::{TimeZone, Utc};
    use coin_listener_core::models::ScanAddressCandidate;
    use uuid::Uuid;

    #[test]
    fn build_scan_task_uses_candidate_ids_and_first_attempt() {
        let candidate = ScanAddressCandidate {
            id: Uuid::from_u128(2),
            tenant_id: Uuid::from_u128(3),
            chain_id: Uuid::from_u128(4),
            scan_interval_seconds: 300,
        };
        let now = Utc.with_ymd_and_hms(2026, 5, 17, 14, 0, 0).unwrap();

        let task = build_scan_task(&candidate, now);

        assert_eq!(task.address_id, candidate.id);
        assert_eq!(task.tenant_id, candidate.tenant_id);
        assert_eq!(task.chain_id, candidate.chain_id);
        assert_eq!(task.attempt, 1);
        assert_eq!(task.enqueued_at, now);
    }
}
```

- [ ] **Step 2: Run the test to verify RED**

Run:

```bash
cargo test -p scheduler build_scan_task_uses_candidate_ids_and_first_attempt --manifest-path backend/Cargo.toml
```

Expected: FAIL with unresolved import for `build_scan_task`.

- [ ] **Step 3: Enable Tokio time support**

Update the `tokio` workspace dependency in `backend/Cargo.toml`:

```toml
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal", "time"] }
```

- [ ] **Step 4: Add scheduler crate dependencies**

Update `backend/crates/scheduler/Cargo.toml` dependencies:

```toml
[dependencies]
anyhow.workspace = true
chrono.workspace = true
coin-listener-core = { path = "../core" }
coin-listener-storage = { path = "../storage" }
redis.workspace = true
sqlx.workspace = true
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
uuid.workspace = true
```

- [ ] **Step 5: Add transactional due-address claim helpers**

Add a storage helper that selects one due active watched address inside a caller-owned SQL transaction using `FOR UPDATE SKIP LOCKED`, without updating `next_scan_at`. Add a second helper that updates that same locked row's `next_scan_at` after enqueue succeeds. The scheduler must hold the transaction open between these two calls so a Redis enqueue failure rolls back and leaves the row due.

- [ ] **Step 6: Implement scheduler library functions**

Replace `backend/crates/scheduler/src/lib.rs` with:

```rust
use chrono::{DateTime, Utc};
use coin_listener_core::{
    models::{ScanAddressCandidate, ScanAddressTask},
    AppResult,
};
use coin_listener_storage::{repositories, scan_queue::ScanQueue};
use redis::aio::MultiplexedConnection;
use sqlx::PgPool;
use uuid::Uuid;

pub fn build_scan_task(candidate: &ScanAddressCandidate, now: DateTime<Utc>) -> ScanAddressTask {
    ScanAddressTask {
        task_id: Uuid::new_v4(),
        address_id: candidate.id,
        tenant_id: candidate.tenant_id,
        chain_id: candidate.chain_id,
        attempt: 1,
        enqueued_at: now,
    }
}

pub async fn enqueue_due_addresses(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    queue: &ScanQueue,
    batch_size: i64,
    now: DateTime<Utc>,
) -> AppResult<usize> {
    let mut enqueued = 0usize;

    for _ in 0..batch_size {
        let mut transaction = pool
            .begin()
            .await
            .map_err(|error| coin_listener_core::AppError::Database(error.to_string()))?;
        let Some(candidate) =
            repositories::claim_one_due_scan_address_for_update(&mut transaction, now).await?
        else {
            transaction
                .rollback()
                .await
                .map_err(|error| coin_listener_core::AppError::Database(error.to_string()))?;
            break;
        };

        let task = build_scan_task(&candidate, now);
        queue.enqueue(redis, &task).await?;
        repositories::mark_claimed_scan_enqueued(
            &mut transaction,
            candidate.id,
            repositories::next_scan_at_from(now, candidate.scan_interval_seconds),
        )
        .await?;
        transaction
            .commit()
            .await
            .map_err(|error| coin_listener_core::AppError::Database(error.to_string()))?;
        enqueued += 1;
    }

    Ok(enqueued)
}

#[cfg(test)]
mod tests {
    use super::build_scan_task;
    use chrono::{TimeZone, Utc};
    use coin_listener_core::models::ScanAddressCandidate;
    use uuid::Uuid;

    #[test]
    fn build_scan_task_uses_candidate_ids_and_first_attempt() {
        let candidate = ScanAddressCandidate {
            id: Uuid::from_u128(2),
            tenant_id: Uuid::from_u128(3),
            chain_id: Uuid::from_u128(4),
            scan_interval_seconds: 300,
        };
        let now = Utc.with_ymd_and_hms(2026, 5, 17, 14, 0, 0).unwrap();

        let task = build_scan_task(&candidate, now);

        assert_eq!(task.address_id, candidate.id);
        assert_eq!(task.tenant_id, candidate.tenant_id);
        assert_eq!(task.chain_id, candidate.chain_id);
        assert_eq!(task.attempt, 1);
        assert_eq!(task.enqueued_at, now);
    }
}
```

- [ ] **Step 7: Replace scheduler binary placeholder with the tick loop**

Replace `backend/crates/scheduler/src/main.rs` with:

```rust
use chrono::Utc;
use coin_listener_core::AppConfig;
use coin_listener_storage::{
    connect_postgres, connect_redis, run_migrations,
    scan_queue::{connect_scan_queue, ScanQueue},
};
use scheduler::enqueue_due_addresses;
use std::time::Duration;
use tokio::{signal, time};
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = AppConfig::from_env()?;
    let postgres = connect_postgres(&config.postgres).await?;
    run_migrations(&postgres).await?;
    let redis_client = connect_redis(&config.redis)?;
    let mut redis = connect_scan_queue(&redis_client).await?;
    let queue = ScanQueue::new(config.scan.queue_key.clone(), config.scan.lock_ttl_seconds);
    let mut ticker = time::interval(Duration::from_secs(config.scan.scheduler_tick_seconds));

    info!(
        service = "scheduler",
        queue_key = queue.queue_key(),
        batch_size = config.scan.scheduler_batch_size,
        tick_seconds = config.scan.scheduler_tick_seconds,
        "service started"
    );

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                match enqueue_due_addresses(
                    &postgres,
                    &mut redis,
                    &queue,
                    config.scan.scheduler_batch_size,
                    Utc::now(),
                ).await {
                    Ok(enqueued) => info!(service = "scheduler", enqueued, "scheduler tick completed"),
                    Err(error) => {
                        error!(service = "scheduler", error = %error, "scheduler tick failed");
                        return Err(error.into());
                    }
                }
            }
            result = signal::ctrl_c() => {
                result?;
                break;
            }
        }
    }

    info!(service = "scheduler", "service stopped");
    Ok(())
}
```

- [ ] **Step 8: Run scheduler test to verify GREEN**

Run:

```bash
cargo test -p scheduler build_scan_task_uses_candidate_ids_and_first_attempt --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 9: Checkpoint**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
cargo check -p scheduler --manifest-path backend/Cargo.toml
```

Expected: both commands exit 0. If running inside git, commit with:

```bash
git add backend/Cargo.toml backend/crates/scheduler/Cargo.toml backend/crates/scheduler/src/lib.rs backend/crates/scheduler/src/main.rs
git commit -m "feat: enqueue due scan addresses"
```

## Task 5: Implement worker consume loop

**Files:**

- Modify: `backend/crates/worker/Cargo.toml`
- Create: `backend/crates/worker/src/lib.rs`
- Modify: `backend/crates/worker/src/main.rs`
- Verify: worker unit tests and worker check

- [ ] **Step 1: Write failing worker chain decision tests**

Create `backend/crates/worker/src/lib.rs` with these tests first:

```rust
#[cfg(test)]
mod tests {
    use super::{scan_plan_for_chain, ScanPlan};

    #[test]
    fn evm_chain_uses_mock_evm_scan() {
        assert_eq!(scan_plan_for_chain("evm"), ScanPlan::MockEvm);
    }

    #[test]
    fn non_evm_chain_is_unsupported() {
        assert_eq!(
            scan_plan_for_chain("utxo"),
            ScanPlan::Unsupported("utxo".to_string())
        );
    }
}
```

- [ ] **Step 2: Run the test to verify RED**

Run:

```bash
cargo test -p worker scan_plan_for_chain --manifest-path backend/Cargo.toml
```

Expected: FAIL with unresolved imports for `scan_plan_for_chain` and `ScanPlan`.

- [ ] **Step 3: Add worker crate dependencies**

Update `backend/crates/worker/Cargo.toml` dependencies:

```toml
[dependencies]
anyhow.workspace = true
chrono.workspace = true
coin-listener-core = { path = "../core" }
coin-listener-storage = { path = "../storage" }
redis.workspace = true
sqlx.workspace = true
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
```

- [ ] **Step 4: Implement worker library functions**

Replace `backend/crates/worker/src/lib.rs` with:

```rust
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use chrono::{DateTime, Utc};
use coin_listener_core::{models::ScanAddressTask, AppResult};
use coin_listener_storage::{repositories, scan_queue::ScanQueue};
use redis::aio::MultiplexedConnection;
use sqlx::PgPool;
use tracing::{info, warn};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanPlan {
    MockEvm,
    Unsupported(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanTaskOutcome {
    Locked,
    Scanned,
    UnsupportedChain(String),
}

pub fn scan_plan_for_chain(chain_type: &str) -> ScanPlan {
    match chain_type {
        "evm" => ScanPlan::MockEvm,
        other => ScanPlan::Unsupported(other.to_string()),
    }
}

pub fn worker_shutdown_requested(shutdown: &AtomicBool) -> bool {
    shutdown.load(Ordering::Relaxed)
}

pub async fn process_scan_task(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    queue: &ScanQueue,
    task: ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<ScanTaskOutcome> {
    let acquired = queue
        .acquire_lock(redis, task.address_id, task.task_id)
        .await?;
    if !acquired {
        return Ok(ScanTaskOutcome::Locked);
    }

    let outcome = process_locked_scan_task(pool, &task, now).await;
    if let Err(error) = queue
        .release_lock(redis, task.address_id, task.task_id)
        .await
    {
        warn!(
            task_id = %task.task_id,
            address_id = %task.address_id,
            error = %error,
            "failed to release scan lock"
        );
    }

    outcome
}

async fn process_locked_scan_task(
    pool: &PgPool,
    task: &ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<ScanTaskOutcome> {
    let context = repositories::get_scan_address_context(pool, task.address_id).await?;
    let next_scan_at = repositories::next_scan_at_from(now, context.scan_interval_seconds);

    match scan_plan_for_chain(&context.chain_type) {
        ScanPlan::MockEvm => {
            repositories::create_mock_evm_event(pool, task.address_id).await?;
            repositories::finish_address_scan(pool, task.address_id, now, next_scan_at).await?;
            Ok(ScanTaskOutcome::Scanned)
        }
        ScanPlan::Unsupported(chain_type) => {
            warn!(
                task_id = %task.task_id,
                address_id = %task.address_id,
                chain_type,
                "chain type is not supported by worker scan"
            );
            repositories::finish_address_scan(pool, task.address_id, now, next_scan_at).await?;
            Ok(ScanTaskOutcome::UnsupportedChain(chain_type))
        }
    }
}

pub async fn run_worker(
    pool: PgPool,
    mut redis: MultiplexedConnection,
    queue: ScanQueue,
    shutdown: Arc<AtomicBool>,
) -> AppResult<()> {
    while !worker_shutdown_requested(&shutdown) {
        match queue.dequeue(&mut redis, 5).await {
            Ok(Some(task)) => {
                let task_id = task.task_id;
                let address_id = task.address_id;
                match process_scan_task(&pool, &mut redis, &queue, task, Utc::now()).await {
                    Ok(outcome) => info!(
                        task_id = %task_id,
                        address_id = %address_id,
                        ?outcome,
                        "scan task processed"
                    ),
                    Err(error) => warn!(
                        task_id = %task_id,
                        address_id = %address_id,
                        error = %error,
                        "scan task failed"
                    ),
                }
            }
            Ok(None) => {}
            Err(error) => warn!(error = %error, "discarded invalid or failed scan queue message"),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{scan_plan_for_chain, ScanPlan};

    #[test]
    fn evm_chain_uses_mock_evm_scan() {
        assert_eq!(scan_plan_for_chain("evm"), ScanPlan::MockEvm);
    }

    #[test]
    fn non_evm_chain_is_unsupported() {
        assert_eq!(
            scan_plan_for_chain("utxo"),
            ScanPlan::Unsupported("utxo".to_string())
        );
    }
}
```

- [ ] **Step 5: Replace worker binary placeholder with the consume loop**

Replace `backend/crates/worker/src/main.rs` with:

```rust
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use coin_listener_core::AppConfig;
use coin_listener_storage::{
    connect_postgres, connect_redis, run_migrations,
    scan_queue::{connect_scan_queue, ScanQueue},
};
use tokio::signal;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use worker::run_worker;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = AppConfig::from_env()?;
    let postgres = connect_postgres(&config.postgres).await?;
    run_migrations(&postgres).await?;
    let redis_client = connect_redis(&config.redis)?;
    let redis = connect_scan_queue(&redis_client).await?;
    let queue = ScanQueue::new(config.scan.queue_key.clone(), config.scan.lock_ttl_seconds);

    info!(
        service = "worker",
        queue_key = queue.queue_key(),
        lock_ttl_seconds = config.scan.lock_ttl_seconds,
        "service started"
    );

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_signal = Arc::clone(&shutdown);
    tokio::spawn(async move {
        if signal::ctrl_c().await.is_ok() {
            shutdown_signal.store(true, Ordering::Relaxed);
        }
    });

    run_worker(postgres, redis, queue, shutdown).await?;

    info!(service = "worker", "service stopped");
    Ok(())
}
```

- [ ] **Step 6: Run worker tests to verify GREEN**

Run:

```bash
cargo test -p worker scan_plan_for_chain --manifest-path backend/Cargo.toml
```

Expected: PASS.

- [ ] **Step 7: Checkpoint**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
cargo check -p worker --manifest-path backend/Cargo.toml
```

Expected: both commands exit 0. If running inside git, commit with:

```bash
git add backend/crates/worker/Cargo.toml backend/crates/worker/src/lib.rs backend/crates/worker/src/main.rs
git commit -m "feat: consume scan queue tasks"
```

## Task 6: Verify scheduler worker milestone

**Files:**

- Verify: backend workspace
- Verify: frontend build remains unaffected
- Verify: Docker Compose config

- [ ] **Step 1: Run Rust formatting check**

Run:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
```

Expected: exit 0 with no formatting diff.

- [ ] **Step 2: Run Rust workspace check**

Run:

```bash
cargo check --workspace --manifest-path backend/Cargo.toml
```

Expected: exit 0. This proves the scheduler and worker binaries compile with the shared storage/core changes.

- [ ] **Step 3: Run Rust workspace tests**

Run:

```bash
cargo test --workspace --manifest-path backend/Cargo.toml
```

Expected: exit 0. The relevant new tests are:

```text
scan_address_task_round_trips_as_json
next_scan_at_from_adds_scan_interval_seconds
scan_task_payload_round_trips
malformed_scan_task_payload_returns_error
scan_lock_key_uses_address_id
build_scan_task_uses_candidate_ids_and_first_attempt
evm_chain_uses_mock_evm_scan
non_evm_chain_is_unsupported
```

- [ ] **Step 4: Run frontend build regression check**

Run:

```bash
npm run build --prefix frontend
```

Expected: exit 0. Existing Vite dependency warnings from `lottie-web` or chunk-size warnings may appear; they are not failures if the command exits 0.

- [ ] **Step 5: Validate Compose config**

Run:

```bash
docker compose -f docker-compose.yml config
```

Expected: exit 0 and rendered Compose YAML.

- [ ] **Step 6: Optional local integration smoke test when Docker daemon is available**

Run only when Docker daemon is available and local ports `5432` and `6379` are free:

```bash
docker compose up -d postgres redis
DATABASE_URL=postgres://coin_listener:coin_listener_password@localhost:5432/coin_listener REDIS_URL=redis://localhost:6379 cargo run --manifest-path backend/Cargo.toml -p scheduler
DATABASE_URL=postgres://coin_listener:coin_listener_password@localhost:5432/coin_listener REDIS_URL=redis://localhost:6379 cargo run --manifest-path backend/Cargo.toml -p worker
```

Expected:

```text
scheduler logs service started and scheduler tick completed
worker logs service started and either waits for queue messages or processes scan task messages
```

Stop the scheduler/worker with `Ctrl-C`. Leave Postgres/Redis running only if the next task needs them; otherwise run:

```bash
docker compose down
```

- [ ] **Step 7: Final checkpoint**

Record which commands were run and their exit status. If running inside git, commit with:

```bash
git add backend/Cargo.toml backend/crates/core backend/crates/storage backend/crates/scheduler backend/crates/worker
git commit -m "feat: add scheduler worker scan queue"
```

## Acceptance Checklist

After Task 6, verify against the design spec:

- [ ] `ScanAddressTask` exists with `task_id`, `address_id`, `tenant_id`, `chain_id`, `attempt`, and `enqueued_at`.
- [ ] Scheduler reads `SCHEDULER_TICK_SECONDS`, `SCHEDULER_BATCH_SIZE`, `SCAN_QUEUE_KEY`, and `SCAN_LOCK_TTL_SECONDS` through `AppConfig`.
- [ ] Active addresses with `next_scan_at <= NOW()` can be queried in batches.
- [ ] Scheduler serializes due addresses into Redis queue messages.
- [ ] Scheduler postpones `next_scan_at` after enqueue.
- [ ] Worker consumes `BRPOP` messages from `scan:address:queue` or the configured queue key.
- [ ] Worker uses `SET key value NX EX ttl` for address locks.
- [ ] Worker releases locks with compare-and-delete semantics.
- [ ] Worker routes EVM chains to `create_mock_evm_event`.
- [ ] Worker marks unsupported chains as scanned forward without crashing.
- [ ] Worker updates `last_scanned_at` and `next_scan_at` after controlled scan outcomes.
- [ ] Backend formatting, compile, and tests pass.
- [ ] Docker Compose config parses.
