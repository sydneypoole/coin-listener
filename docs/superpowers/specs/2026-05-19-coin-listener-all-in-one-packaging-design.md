# Coin Listener All-in-One Packaging Design

## 1. Goal

Build an all-in-one Coin Listener runtime that can be launched as one backend process while preserving the existing multi-process API, scheduler, worker, and notifier binaries. The all-in-one runtime should serve the API, serve the built React frontend, run scheduler/worker/notifier loops, write service heartbeats, and shut down cleanly from one signal.

This milestone closes the original MVP acceptance gap for an all-in-one mode. PostgreSQL and Redis remain external dependencies; this milestone does not embed databases, add authentication hardening, add process supervision, or remove existing Docker Compose services.

## 2. Current context

The backend workspace currently has separate binary crates:

- `api-server`: loads config, connects Postgres/Redis, runs migrations, builds Axum routes, runs `/health` and `/api/*`, and writes `api-server` heartbeats.
- `scheduler`: loads config, connects Postgres/Redis, runs migrations, periodically enqueues due scan tasks, and writes `scheduler` heartbeats.
- `worker`: loads config, connects Postgres/Redis, runs migrations, dequeues scan tasks, performs provider-resilient scans, and writes `worker` heartbeats.
- `notifier`: loads config, connects Postgres, runs migrations, processes notification outbox deliveries, and writes `notifier` heartbeats.

The React frontend builds into `frontend/dist`. Docker currently builds the backend workspace and runs one binary per service through `docker-compose.yml`. The all-in-one runtime should reuse these existing modules rather than fork scan, notification, provider, or API logic.

## 3. Approach options

### Option A: Shell-style supervisor image

Build the existing four binaries plus frontend assets, then start them from a shell script in one container.

- Pros: minimal Rust refactoring.
- Cons: signal handling and failure behavior are fragile; service health would represent multiple OS processes; harder to test in Rust; still does not provide a real single runtime.

### Option B: Monolithic Rust runtime with external frontend dist

Add one Rust crate/binary that imports existing service crates, starts API/scheduler/worker/notifier tasks inside one Tokio runtime, and serves `frontend/dist` through Axum. Keep Postgres and Redis external. Docker builds frontend assets and copies them into the image next to the binary.

- Pros: one process, testable runtime boundaries, shared graceful shutdown, reuses existing crates, satisfies MVP without overbuilding.
- Cons: requires extracting reusable API and scheduler runtime functions from current binary `main.rs` files.

### Option C: Fully embedded frontend bytes in the Rust binary

Compile frontend assets into the Rust binary with an embedding crate.

- Pros: closer to a literal single executable file.
- Cons: adds another asset embedding dependency and build-time coupling; makes frontend rebuild behavior more complex; unnecessary for the current MVP because binary + dist in one image is accepted by the original design notes.

**Selected approach:** Option B. It delivers a single all-in-one process and one Docker service while keeping the implementation narrow and maintainable. Option C can be a later hardening step if a true single-file artifact becomes required.

## 4. Runtime architecture

Add a backend workspace member `crates/all-in-one` with binary name `all-in-one`.

At startup it will:

1. Initialize tracing once.
2. Load `AppConfig::from_env()`.
3. Connect to Postgres once and run migrations once.
4. Connect Redis clients/connections needed by API, scheduler, and worker.
5. Create one shared `Arc<AtomicBool>` shutdown flag.
6. Spawn service heartbeat tasks for `api-server`, `scheduler`, `worker`, and `notifier` using distinct instance IDs prefixed or generated per service.
7. Spawn scheduler, worker, and notifier loops as Tokio tasks.
8. Bind the configured API address and serve one Axum router containing API routes plus frontend static routes.
9. On `ctrl_c`, set the shared shutdown flag and let background loops exit.

If the API server fails to bind, startup fails immediately. If scheduler, worker, or notifier returns an error, all-in-one logs the failing service, sets shutdown, and returns an error instead of silently running degraded. This keeps container health honest.

## 5. API and static frontend serving

Refactor `api-server` into a reusable library boundary:

- Keep `api-server/src/main.rs` as a thin binary entrypoint.
- Add `api-server/src/lib.rs` that exposes the existing `routes` module and reusable helpers for building the API router/state.
- Keep existing API route behavior unchanged for multi-process mode.

The all-in-one router will merge:

- Existing API router from `api-server` for `/health` and `/api/*`.
- Static file serving rooted at `COIN_LISTENER_FRONTEND_DIST` when set, otherwise `./frontend/dist`.
- SPA fallback to `index.html` for non-API GET routes so client-side navigation works.

API routes must keep priority over static routes. Missing frontend assets should not prevent API-only startup by default; the root route should return a clear 404 or static-file error if assets are absent. Docker all-in-one builds must include assets, so the packaged image serves the UI.

## 6. Scheduler, worker, and notifier reuse

The existing `worker::run_worker` and `notifier::run_notifier` functions are already reusable. Scheduler currently exposes `enqueue_due_addresses` but keeps the loop in `scheduler/src/main.rs`; this milestone should extract a reusable scheduler loop such as:

```rust
pub async fn run_scheduler(
    pool: PgPool,
    redis: MultiplexedConnection,
    queue: ScanQueue,
    batch_size: i64,
    tick_seconds: u64,
    shutdown: Arc<AtomicBool>,
) -> AppResult<()>;
```

The existing `scheduler` binary should call this helper, preserving current behavior. The all-in-one binary should call the same helper with its own Redis connection.

Each runtime loop gets its own Redis connection when needed:

- API keeps a Redis client for queue depth and dev scan routes.
- Scheduler owns one scan queue connection.
- Worker owns one scan queue/QPS connection.
- Notifier uses Postgres outbox and does not require Redis for delivery processing.

## 7. Configuration

Keep existing environment variables unchanged:

- `DATABASE_URL`
- `REDIS_URL`
- `API_SERVER_HOST`
- `API_SERVER_PORT`
- `ENABLE_DEV_ROUTES`
- `SCAN_QUEUE_KEY`
- `SCAN_LOCK_TTL_SECONDS`
- `SCHEDULER_TICK_SECONDS`
- `SCHEDULER_BATCH_SIZE`
- `NOTIFICATION_OUTBOX_*`

Add one optional variable:

- `COIN_LISTENER_FRONTEND_DIST`: path to built frontend assets for all-in-one static serving. Default: `./frontend/dist`.

Do not add mode-specific queue names or database URLs. Running all-in-one together with the separate scheduler/worker/notifier services is allowed only when operators intentionally want multiple workers/schedulers; Docker Compose should make the mode separation clear to avoid accidental duplicate scheduler loops.

## 8. Docker and Compose

Extend Docker packaging rather than replacing existing services:

- Keep `docker/backend.Dockerfile` capable of building the existing backend binaries.
- Add a build path for `all-in-one` that also builds frontend assets and copies `frontend/dist` into the runtime image.
- Add an `all-in-one` service to `docker-compose.yml` using a profile such as `all-in-one`.
- Keep the existing `api-server`, `scheduler`, `worker`, and `notifier` services for multi-process mode.

The all-in-one service exposes port `8080`, depends on healthy Postgres and Redis, reads `.env`, and sets `COIN_LISTENER_FRONTEND_DIST` to the copied frontend asset path inside the image.

## 9. Error handling and shutdown

Startup errors are fatal:

- invalid config
- Postgres connection failure
- migration failure
- Redis connection failure needed by API/scheduler/worker
- API bind failure

Runtime task errors are fatal to the all-in-one process:

1. Log the service name and error.
2. Set the shared shutdown flag.
3. Stop accepting API traffic through graceful shutdown.
4. Return a non-zero process result.

`ctrl_c` should set the shared shutdown flag and let scheduler, worker, notifier, and heartbeat loops exit. The API server should use Axum graceful shutdown.

## 10. Testing strategy

Use TDD for behavior changes.

Required backend tests:

- Scheduler loop observes shutdown and exits without requiring `ctrl_c`.
- All-in-one crate exposes stable startup/building helpers without duplicating service logic.
- All-in-one static path helper defaults to `./frontend/dist` and respects `COIN_LISTENER_FRONTEND_DIST`.
- All-in-one router serves API routes before static fallback.
- Existing API, scheduler, worker, notifier tests continue passing.

Required packaging verification:

- `cargo fmt --all --check --manifest-path backend/Cargo.toml`
- `cargo check --workspace --manifest-path backend/Cargo.toml`
- `cargo test --workspace --manifest-path backend/Cargo.toml`
- `npm run build --prefix frontend`
- Dockerfile/Compose text tests or source assertions confirm `all-in-one` is copied and exposed.

Docker image build can be verified if local Docker is available. If Docker is unavailable, the milestone must still verify Dockerfile and Compose structure through tests/source assertions and clearly report that image build was not run.

## 11. Acceptance criteria

This milestone is complete when:

1. `backend/Cargo.toml` includes `crates/all-in-one`.
2. `cargo build --release --manifest-path backend/Cargo.toml --bin all-in-one` can build the all-in-one binary.
3. Existing separate binaries still build and keep their current behavior.
4. The all-in-one runtime starts API, scheduler, worker, notifier, and service heartbeats in one process.
5. Built frontend assets can be served from the all-in-one HTTP server with SPA fallback.
6. Docker Compose includes an all-in-one service/profile without removing multi-process services.
7. Fresh backend and frontend verification passes.

## 12. Non-goals

- Embedding Postgres or Redis.
- Removing existing multi-process service binaries.
- Replacing the notification outbox, scan queue, provider failover, or service heartbeat designs.
- Building a process supervisor shell script.
- Adding production authentication hardening.
- Adding deployment documentation beyond the minimal Compose and env changes needed for the new service.
