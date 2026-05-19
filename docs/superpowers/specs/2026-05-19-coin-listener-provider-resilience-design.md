# Coin Listener Provider Resilience Design

**Goal:** Make scans resilient to provider outages by tracking provider health, skipping temporarily unhealthy providers, falling back to the next configured provider, and enforcing provider QPS limits during worker scans.

**Scope:** This milestone covers backend provider health storage, provider candidate selection, worker failover, Redis-backed provider QPS limiting, `/api/system/status` provider health fields, and frontend display on the existing system status page. It does not add provider auto-discovery, external alert delivery, manual provider management actions, historical uptime charts, or provider-specific paid API configuration.

## 1. Current context

The MVP design requires Provider configuration and health state, provider-level QPS limiting, and retry/fallback when a provider fails. Current code has provider configuration fields (`priority`, `qps_limit`, `timeout_ms`, `status`) and the operations status page lists providers. Worker scan paths still call `active_rpc_provider_for_chain`, which selects a single active RPC provider ordered by priority. If that provider request fails, the scan task fails without trying a lower-priority provider.

Relevant current files:

- `backend/crates/storage/src/repositories.rs`: `ACTIVE_RPC_PROVIDER_QUERY` returns one active provider with `LIMIT 1`.
- `backend/crates/worker/src/lib.rs`: EVM, TRON, and BTC scan paths fetch one provider and construct one client.
- `backend/crates/storage/src/system_status.rs`: provider status counts configured active/inactive providers but does not expose runtime health.
- `frontend/src/pages/SystemStatusPage.tsx`: provider status table shows static provider fields only.

## 2. Approach options

### Option A: Failover only

Fetch all active providers by chain, try them in priority order, and return success from the first provider that completes the scan.

- Pros: small and immediately improves availability.
- Cons: does not remember failures; every scan keeps hitting a known-bad primary provider.

### Option B: Health-tracked failover with circuit breaker

Add a `provider_health` table keyed by provider id. On provider request failure, increment consecutive failures and open a circuit for five minutes after three consecutive failures. On success, reset the failure counter. Candidate selection excludes providers whose circuit is currently open.

- Pros: avoids repeatedly hammering known-bad providers, gives operators visibility, uses existing provider priorities.
- Cons: adds one migration and health write paths.

### Option C: Health-tracked failover plus QPS limiter

Build Option B and enforce existing `providers.qps_limit` with Redis counters per provider per second before using a candidate.

- Pros: closes the MVP provider reliability gap most directly: health, fallback, and rate limiting.
- Cons: touches worker scan signatures because scan execution already has the Redis connection at task-processing level.

**Selected approach:** Option C. It is the smallest milestone that satisfies provider health, fallback, and QPS requirements without introducing new queues or external systems.

## 3. Data model

Create migration `backend/crates/storage/migrations/0011_provider_health.sql`:

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

This table tracks runtime health only. It does not change `providers.status`; operators can still disable a provider explicitly through provider config.

## 4. Core models

Extend provider status DTOs in `backend/crates/core/src/models.rs`:

```rust
pub struct ProviderHealthStatus {
    pub consecutive_failures: i32,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub disabled_until: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub is_circuit_open: bool,
}
```

Add to `ProviderStatusItem`:

```rust
pub health: ProviderHealthStatus,
```

Worker-internal storage rows may use a `ProviderCandidate` struct that combines `Provider` plus health state for selection. This does not need to be exposed through public API unless the frontend needs it.

## 5. Storage design

Add focused provider runtime helpers in `backend/crates/storage/src/provider_health.rs` and export the module from `backend/crates/storage/src/lib.rs`.

Constants:

```rust
pub const PROVIDER_CIRCUIT_FAILURE_THRESHOLD: i32 = 3;
pub const PROVIDER_CIRCUIT_COOLDOWN_SECONDS: i64 = 300;
pub const PROVIDER_LAST_ERROR_MAX_CHARS: usize = 500;
```

Queries and helper behavior:

1. `active_rpc_provider_candidates(pool, chain_id, now)`
   - Selects configured active RPC providers for a chain.
   - Left joins `provider_health`.
   - Excludes rows where `disabled_until > now`.
   - Orders by `providers.priority ASC, providers.name ASC`.
   - Returns all usable candidates, not just one.
2. `record_provider_success(pool, provider_id, now)`
   - Upserts `provider_health`.
   - Sets `consecutive_failures = 0`.
   - Sets `last_success_at = now`.
   - Clears `disabled_until` and `last_error`.
3. `record_provider_failure(pool, provider_id, now, error)`
   - Upserts and increments `consecutive_failures`.
   - Sets `last_failure_at = now`.
   - Stores a truncated safe `last_error` string.
   - If the new failure count is at least `PROVIDER_CIRCUIT_FAILURE_THRESHOLD`, sets `disabled_until = now + 300 seconds`.
4. `provider_health_status(pool)` or a system-status-specific query
   - Returns health fields for provider status display.

`last_error` must not include provider base URLs, API keys, tokens, or full webhook-style query strings. Current chain-provider request/status errors already avoid provider URLs. The storage helper still truncates messages and redacts obvious `token=`, `api_key=`, and `key=` query values before persistence.

## 6. Provider QPS limiter

Add Redis-backed limiter helpers near scan queue code or in `provider_health.rs` if Redis dependency stays in storage:

```rust
pub fn provider_qps_key(provider_id: Uuid, epoch_second: i64) -> String
```

Key format:

```text
provider:qps:<provider_id>:<epoch_second>
```

Behavior:

- Before a worker uses a provider, it increments the current second key.
- The key expires after two seconds.
- If the incremented value is `<= provider.qps_limit`, the provider can be used.
- If the value is greater than `qps_limit`, the worker skips this provider for the current scan attempt and tries the next candidate.
- A rate-limit skip does not increment provider failure count and does not open the circuit.
- If every candidate is rate-limited or circuit-open, the scan returns a configuration error: `no provider capacity available for chain`.

This limiter is intentionally simple. It is approximate across workers but shared through Redis and respects the existing `qps_limit` field without introducing a scheduler-level rate planner.

## 7. Worker failover design

Worker scan task flow changes from “fetch one provider and scan” to “fetch candidates and try providers.”

High-level behavior:

1. Load scan context and chain plan as today.
2. Fetch active provider candidates for the chain with open circuits excluded.
3. For each candidate:
   - Check the provider QPS limiter.
   - If rate-limited, log and try the next candidate.
   - Run the chain-specific scan using that provider.
   - On success, record provider success and finish the address scan.
   - On provider request/status/body failure, record provider failure and try the next candidate.
   - On database, validation, or unsupported-chain errors, stop immediately; these are not provider availability failures.
4. If all candidates fail due to provider availability, return the last provider error.
5. If no usable candidate exists, return `AppError::Config("no active rpc provider capacity for chain")`.

Provider availability failures are classified narrowly:

```rust
pub fn is_provider_availability_error(error: &AppError) -> bool {
    matches!(error, AppError::Config(_))
}
```

This matches current chain-provider clients where request failures and HTTP status failures are mapped to `AppError::Config`, while invalid decoded data remains `Validation` and should not blindly fall back.

Chain-specific scan functions should be split so existing behavior can run with a supplied provider:

- `scan_evm_address_with_provider(pool, task, provider, now)`
- `scan_tron_address_with_provider(pool, task, provider, now)`
- `scan_btc_address_with_provider(pool, task, provider, now)`

The public `scan_evm_address`, `scan_tron_address`, and `scan_btc_address` functions may become thin wrappers around the candidate loop or may accept Redis if needed for QPS. Keep the smallest signature change that keeps tests readable.

## 8. API and frontend behavior

`GET /api/system/status` remains the endpoint for provider runtime visibility. `providers.items[]` gains:

```json
{
  "health": {
    "consecutive_failures": 3,
    "last_success_at": "2026-05-19T10:00:00Z",
    "last_failure_at": "2026-05-19T10:04:00Z",
    "disabled_until": "2026-05-19T10:09:00Z",
    "last_error": "provider request failed: timeout",
    "is_circuit_open": true
  }
}
```

Frontend updates:

- Add `ProviderHealthStatus` to `frontend/src/api/types.ts`.
- Extend `ProviderStatusItem` with `health`.
- Update `SystemStatusPage.tsx` provider table:
  - show config status and runtime circuit status separately;
  - render circuit-open rows with a red tag;
  - show consecutive failure count;
  - show last success/failure time;
  - show `disabled_until` when present;
  - show truncated `last_error` with ellipsis.

Do not add new provider management buttons in this milestone.

## 9. Error handling and safety

- Provider config errors such as invalid `timeout_ms` or `qps_limit` remain validation/configuration errors and should not mark a provider unhealthy unless they happen during provider availability checks.
- Provider request and HTTP status failures mark only the provider used for that scan.
- Database write failure while recording provider health should not hide the original scan error. Log the health write failure and continue with the original error handling path.
- Redis QPS limiter failure should fail the scan task rather than silently exceeding configured limits.
- Provider `base_url` and `api_key_ref` must not be copied into provider health errors.
- Open circuits are temporary. A provider becomes eligible again automatically after `disabled_until` passes.

## 10. Testing plan

Backend tests:

1. Migration test verifies `provider_health` table, primary key, `disabled_until`, and indexes.
2. DTO serde test verifies `ProviderHealthStatus` and extended `ProviderStatusItem` round-trip.
3. Candidate query test verifies active RPC providers are selected by priority and open circuits are excluded.
4. Success query test verifies success resets failures and clears `disabled_until`/`last_error`.
5. Failure query test verifies failure increments count and opens circuit at threshold.
6. Error sanitizer test verifies URL query secrets and long messages are redacted/truncated.
7. QPS helper tests verify Redis key shape and permit comparison behavior.
8. Worker helper tests verify provider availability error classification and failover decision order.
9. Existing worker scan tests continue to pass.

Frontend checks:

1. `npm run build --prefix frontend` validates TypeScript for provider health fields.
2. System status page compiles with circuit status tags and health columns.

Final verification:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
cargo check --workspace --manifest-path backend/Cargo.toml
cargo test --workspace --manifest-path backend/Cargo.toml
npm run build --prefix frontend
```

## 11. Out of scope

- Manual “reset provider health” API.
- Historical provider uptime charts.
- Provider auto-discovery or scoring beyond priority and temporary circuit state.
- Paid API key rotation.
- Scheduler-level provider capacity planning.
- Alert delivery for circuit-open providers.
