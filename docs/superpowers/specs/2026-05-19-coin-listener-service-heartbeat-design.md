# Coin Listener Service Heartbeat Runtime Health Design

**Goal:** Add durable service heartbeat visibility so operators can tell whether api-server, scheduler, worker, and notifier processes are alive, stale, or stopped.

**Scope:** This milestone covers backend heartbeat storage, service runtime heartbeat writes, `/api/system/status` health summary fields, and frontend display on the existing system status page. It does not add alert delivery, auto-restart, provider failover, queue retry/dead-lettering, or external monitoring integrations.

## 1. Current context

Coin Listener now has scan status, queue depth, provider config, notification outbox counts, and notification operations pages. The remaining operations gap is process liveness: a healthy database and queue can still look normal while the scheduler, worker, notifier, or API process is no longer running.

Current services:

- `api-server`: serves dashboard/API requests and already has `/api/system/status`.
- `scheduler`: periodically enqueues due scan addresses.
- `worker`: consumes scan tasks and writes events/notification outbox rows.
- `notifier`: claims notification outbox rows and writes deliveries.

Current status data is inferred from data tables and queues. There is no durable row saying when a runtime last checked in, what binary kind it is, or whether that timestamp is stale.

## 2. Approach

Add a small `service_heartbeats` table keyed by `(service_name, instance_id)`. Each process writes an upserted heartbeat row at startup and on a fixed interval. System status reads these rows, classifies each row as `online` or `stale`, and returns a summary plus row details to the frontend.

This keeps the milestone narrow:

- No separate heartbeat daemon.
- No distributed lease ownership.
- No alerting side effects.
- No destructive cleanup of old instances.

## 3. Data model

Create migration `backend/crates/storage/migrations/0010_service_heartbeats.sql`:

```sql
CREATE TABLE IF NOT EXISTS service_heartbeats (
    service_name TEXT NOT NULL,
    instance_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'online',
    started_at TIMESTAMPTZ NOT NULL,
    last_seen_at TIMESTAMPTZ NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (service_name, instance_id)
);

CREATE INDEX IF NOT EXISTS idx_service_heartbeats_last_seen
    ON service_heartbeats(last_seen_at DESC);

CREATE INDEX IF NOT EXISTS idx_service_heartbeats_service
    ON service_heartbeats(service_name, last_seen_at DESC);
```

`service_name` accepted values for this milestone:

- `api-server`
- `scheduler`
- `worker`
- `notifier`

`instance_id` is generated at process startup as a UUID string. It is not persisted across restarts.

`metadata` stores a small JSON object with runtime information that is safe to expose:

```json
{
  "pid": 12345,
  "version": "0.1.0"
}
```

No secrets, URLs, tokens, or environment variable values are stored.

## 4. Backend model and repository design

Add core models:

```rust
pub struct ServiceHeartbeat {
    pub service_name: String,
    pub instance_id: String,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct ServiceHeartbeatStatusItem {
    pub service_name: String,
    pub instance_id: String,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub stale_after_seconds: i64,
    pub is_stale: bool,
    pub metadata: serde_json::Value,
}

pub struct ServiceHealthStatus {
    pub online: i64,
    pub stale: i64,
    pub items: Vec<ServiceHeartbeatStatusItem>,
}
```

Extend `SystemStatus` with:

```rust
pub services: ServiceHealthStatus,
```

Add storage helpers in a focused module, either `backend/crates/storage/src/service_heartbeats.rs` or a small section in `system_status.rs` plus repository functions. Prefer a new `service_heartbeats.rs` if implementation would otherwise make `system_status.rs` too broad.

Repository operations:

1. `upsert_service_heartbeat(pool, service_name, instance_id, started_at, now, metadata)`
   - validates known service name and non-empty instance id.
   - upserts by `(service_name, instance_id)`.
   - sets `status = 'online'`.
   - updates `last_seen_at`, `metadata`, `updated_at`.
   - preserves original `started_at` on conflict.
2. `list_service_heartbeats(pool, stale_before)`
   - returns all rows ordered by `service_name ASC, last_seen_at DESC`.
   - computes stale in Rust for deterministic tests.
3. `system_service_health(pool, now)`
   - uses `now - 90 seconds` as the stale threshold.
   - returns counts and items.

Stale threshold is a constant:

```rust
pub const SERVICE_HEARTBEAT_STALE_SECONDS: i64 = 90;
```

## 5. Runtime heartbeat writer

Add a small async loop shared by binaries, for example in `coin-listener-storage`:

```rust
pub async fn run_service_heartbeat(
    pool: PgPool,
    service_name: &'static str,
    instance_id: String,
    started_at: DateTime<Utc>,
    shutdown: Arc<AtomicBool>,
) -> AppResult<()>
```

Behavior:

- Write one heartbeat immediately before the first sleep.
- Repeat every 30 seconds while shutdown is not requested.
- Log write failures and keep looping; a transient database error should not crash the service.
- Stop when the existing process shutdown flag is set.

If a binary does not currently expose a shared shutdown flag shape, keep the integration minimal: spawn the loop with an `Arc<AtomicBool>` local to that binary and set it during signal shutdown where already available.

Runtime integration:

- `api-server/src/main.rs`: starts heartbeat after PostgreSQL pool is available.
- `scheduler/src/main.rs`: starts heartbeat after PostgreSQL pool is available.
- `worker/src/main.rs`: starts heartbeat after PostgreSQL pool is available.
- `notifier/src/main.rs`: starts heartbeat after PostgreSQL pool is available.

## 6. API and frontend behavior

`GET /api/system/status` remains the only endpoint needed. It adds:

```json
{
  "services": {
    "online": 3,
    "stale": 1,
    "items": [
      {
        "service_name": "worker",
        "instance_id": "8d1d...",
        "status": "online",
        "started_at": "2026-05-19T10:00:00Z",
        "last_seen_at": "2026-05-19T10:01:00Z",
        "stale_after_seconds": 90,
        "is_stale": false,
        "metadata": { "pid": 12345, "version": "0.1.0" }
      }
    ]
  }
}
```

Frontend updates:

- Add `ServiceHealthStatus` and `ServiceHeartbeatStatusItem` types in `frontend/src/api/types.ts`.
- Update `SystemStatusPage.tsx` with a service health card/table.
- Display counts for online/stale.
- Mark stale rows visually using existing Semi tags.
- Show service name, instance id (shortened), started time, last seen time, and metadata pid/version when present.

## 7. Error handling

- Invalid service names return `AppError::Validation` in repository functions.
- Heartbeat loop logs transient database errors and retries on the next interval.
- System status should still fail if PostgreSQL query fails, matching existing system status behavior.
- Frontend handles missing `services` defensively only if TypeScript requires it; backend responses include it after this milestone.

## 8. Testing plan

Backend tests:

1. Core serde round-trip for `ServiceHealthStatus` and extended `SystemStatus`.
2. Migration string test verifies primary key and indexes.
3. Repository validation rejects unknown service names and empty instance ids.
4. Upsert query preserves `started_at` and updates `last_seen_at`.
5. Status query/list classifies rows stale when `last_seen_at < now - 90 seconds`.
6. API route/system status model test includes `services` in JSON.
7. Heartbeat loop helper test covers immediate write timing through a factored single-tick function, not a long sleep.

Frontend checks:

1. `npm run build --prefix frontend` validates TypeScript.
2. System status page compiles with service health table and stale tag rendering.

Final verification:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
cargo check --workspace --manifest-path backend/Cargo.toml
cargo test --workspace --manifest-path backend/Cargo.toml
npm run build --prefix frontend
```

## 9. Follow-up milestones

1. Alerting rules for stale services.
2. Provider health/failover/rate-limit/circuit breaker.
3. Durable scan queue retry/dead-letter operations.
4. Historical service uptime charts.
