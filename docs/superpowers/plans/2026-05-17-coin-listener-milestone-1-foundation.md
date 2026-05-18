# Coin Listener Milestone 1 Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the initial Rust workspace, React + Semi UI frontend, Docker Compose infrastructure, configuration, logging, migrations, and health checks for the Coin Listener platform.

**Architecture:** This milestone creates a monorepo with a Rust backend workspace and a React frontend. The backend has separate binary crates for `api-server`, `scheduler`, `worker`, and `notifier`, plus shared library crates for `core` and `storage`. PostgreSQL and Redis run through Docker Compose and are accessed by backend services through environment-driven configuration.

**Tech Stack:** Rust stable, Cargo workspace, Axum, Tokio, SQLx, Redis, Tracing, Figment, React, TypeScript, Vite, Semi UI, Docker Compose, PostgreSQL, Redis.

---

## Scope

This plan implements only Milestone 1 from `docs/superpowers/specs/2026-05-17-coin-listener-design.md`:

- Rust workspace.
- React + Semi UI project.
- Docker Compose.
- PostgreSQL / Redis.
- Configuration management.
- Migration framework.
- Basic logging.
- API health check.

It does not implement authentication, address management, chain providers, event ingestion, worker queues, or notifications. Those belong to later milestone plans.

## File Structure

Create this structure:

```text
coin-listener/
  .env.example
  .gitignore
  docker-compose.yml
  docker/
    backend.Dockerfile
  backend/
    Cargo.toml
    crates/
      core/
        Cargo.toml
        src/lib.rs
        src/config.rs
        src/error.rs
      storage/
        Cargo.toml
        src/lib.rs
        src/postgres.rs
        src/redis.rs
        migrations/0001_init.sql
      api-server/
        Cargo.toml
        src/main.rs
        src/routes.rs
      scheduler/
        Cargo.toml
        src/main.rs
      worker/
        Cargo.toml
        src/main.rs
      notifier/
        Cargo.toml
        src/main.rs
  frontend/
    package.json
    index.html
    vite.config.ts
    tsconfig.json
    tsconfig.node.json
    src/main.tsx
    src/App.tsx
    src/api/health.ts
    src/styles.css
```

Responsibilities:

- `backend/crates/core`: shared config and error types.
- `backend/crates/storage`: PostgreSQL and Redis connection helpers plus migrations.
- `backend/crates/api-server`: HTTP API and `/health` endpoint.
- `backend/crates/scheduler`: placeholder process with config and logging.
- `backend/crates/worker`: placeholder process with config and logging.
- `backend/crates/notifier`: placeholder process with config and logging.
- `frontend`: React + Semi UI shell and API health display.
- `docker-compose.yml`: local PostgreSQL, Redis, backend services, and frontend.

---

## Task 1: Create repository foundation files

**Files:**
- Create: `.gitignore`
- Create: `.env.example`
- Create: `docker-compose.yml`
- Create: `docker/backend.Dockerfile`

- [ ] **Step 1: Create `.gitignore`**

Write:

```gitignore
/target
/backend/target
/frontend/node_modules
/frontend/dist
.env
.env.local
*.log
.DS_Store
.idea
.vscode
```

- [ ] **Step 2: Create `.env.example`**

Write:

```bash
POSTGRES_USER=coin_listener
POSTGRES_PASSWORD=coin_listener_password
POSTGRES_DB=coin_listener
POSTGRES_HOST=postgres
POSTGRES_PORT=5432
DATABASE_URL=postgres://coin_listener:coin_listener_password@postgres:5432/coin_listener
REDIS_URL=redis://redis:6379
API_SERVER_HOST=0.0.0.0
API_SERVER_PORT=8080
RUST_LOG=info
VITE_API_BASE_URL=http://localhost:8080
```

- [ ] **Step 3: Create `docker-compose.yml`**

Write:

```yaml
services:
  postgres:
    image: postgres:16-alpine
    environment:
      POSTGRES_USER: ${POSTGRES_USER:-coin_listener}
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD:-coin_listener_password}
      POSTGRES_DB: ${POSTGRES_DB:-coin_listener}
    ports:
      - "5432:5432"
    volumes:
      - postgres_data:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U ${POSTGRES_USER:-coin_listener} -d ${POSTGRES_DB:-coin_listener}"]
      interval: 5s
      timeout: 3s
      retries: 10

  redis:
    image: redis:7-alpine
    ports:
      - "6379:6379"
    healthcheck:
      test: ["CMD", "redis-cli", "ping"]
      interval: 5s
      timeout: 3s
      retries: 10

  api-server:
    build:
      context: ./backend
      dockerfile: ../docker/backend.Dockerfile
    command: ["api-server"]
    env_file: .env
    ports:
      - "8080:8080"
    depends_on:
      postgres:
        condition: service_healthy
      redis:
        condition: service_healthy

  scheduler:
    build:
      context: ./backend
      dockerfile: ../docker/backend.Dockerfile
    command: ["scheduler"]
    env_file: .env
    depends_on:
      postgres:
        condition: service_healthy
      redis:
        condition: service_healthy

  worker:
    build:
      context: ./backend
      dockerfile: ../docker/backend.Dockerfile
    command: ["worker"]
    env_file: .env
    depends_on:
      postgres:
        condition: service_healthy
      redis:
        condition: service_healthy

  notifier:
    build:
      context: ./backend
      dockerfile: ../docker/backend.Dockerfile
    command: ["notifier"]
    env_file: .env
    depends_on:
      postgres:
        condition: service_healthy
      redis:
        condition: service_healthy

volumes:
  postgres_data:
```

- [ ] **Step 4: Create Dockerfile directory and backend Dockerfile**

Create `docker/backend.Dockerfile`:

```dockerfile
FROM rust:1-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --workspace

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/api-server /usr/local/bin/api-server
COPY --from=builder /app/target/release/scheduler /usr/local/bin/scheduler
COPY --from=builder /app/target/release/worker /usr/local/bin/worker
COPY --from=builder /app/target/release/notifier /usr/local/bin/notifier
ENTRYPOINT []
```

- [ ] **Step 5: Verify YAML parses**

Run:

```bash
docker compose config
```

Expected: Compose prints the merged configuration without syntax errors.

---

## Task 2: Create Rust workspace and shared core crate

**Files:**
- Create: `backend/Cargo.toml`
- Create: `backend/crates/core/Cargo.toml`
- Create: `backend/crates/core/src/lib.rs`
- Create: `backend/crates/core/src/config.rs`
- Create: `backend/crates/core/src/error.rs`

- [ ] **Step 1: Create `backend/Cargo.toml`**

Write:

```toml
[workspace]
members = [
    "crates/core",
    "crates/storage",
    "crates/api-server",
    "crates/scheduler",
    "crates/worker",
    "crates/notifier",
]
resolver = "2"

[workspace.package]
edition = "2021"
version = "0.1.0"
license = "UNLICENSED"

[workspace.dependencies]
anyhow = "1"
axum = { version = "0.7", features = ["ws"] }
figment = "0.10"
redis = { version = "0.25", features = ["tokio-comp"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sqlx = { version = "0.7", features = ["runtime-tokio-rustls", "postgres", "uuid", "chrono", "migrate"] }
thiserror = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal"] }
tower-http = { version = "0.5", features = ["cors", "trace"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
uuid = { version = "1", features = ["v4", "serde"] }
```

- [ ] **Step 2: Create `backend/crates/core/Cargo.toml`**

Write:

```toml
[package]
name = "coin-listener-core"
edition.workspace = true
version.workspace = true
license.workspace = true

[dependencies]
figment.workspace = true
serde.workspace = true
thiserror.workspace = true
```

- [ ] **Step 3: Create `backend/crates/core/src/lib.rs`**

Write:

```rust
pub mod config;
pub mod error;

pub use config::{AppConfig, PostgresConfig, RedisConfig, ServerConfig};
pub use error::{AppError, AppResult};
```

- [ ] **Step 4: Create `backend/crates/core/src/error.rs`**

Write:

```rust
use thiserror::Error;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("database error: {0}")]
    Database(String),
    #[error("redis error: {0}")]
    Redis(String),
}
```

- [ ] **Step 5: Create `backend/crates/core/src/config.rs`**

Write:

```rust
use figment::Figment;
use serde::Deserialize;
use std::env;

use crate::{AppError, AppResult};

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub postgres: PostgresConfig,
    pub redis: RedisConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PostgresConfig {
    pub database_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RedisConfig {
    pub redis_url: String,
}

impl AppConfig {
    pub fn from_env() -> AppResult<Self> {
        Figment::new()
            .merge(("server.host", env::var("API_SERVER_HOST").unwrap_or_else(|_| "0.0.0.0".to_string())))
            .merge(("server.port", env::var("API_SERVER_PORT").unwrap_or_else(|_| "8080".to_string())))
            .merge(("postgres.database_url", env::var("DATABASE_URL").unwrap_or_else(|_| {
                "postgres://coin_listener:coin_listener_password@localhost:5432/coin_listener".to_string()
            })))
            .merge(("redis.redis_url", env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string())))
            .extract()
            .map_err(|error| AppError::Config(error.to_string()))
    }

    pub fn server_addr(&self) -> String {
        format!("{}:{}", self.server.host, self.server.port)
    }
}
```

- [ ] **Step 6: Run Rust check for core crate**

Run:

```bash
cd backend && cargo check -p coin-listener-core
```

Expected: command exits successfully.

---

## Task 3: Create storage crate with PostgreSQL, Redis, and migration

**Files:**
- Create: `backend/crates/storage/Cargo.toml`
- Create: `backend/crates/storage/src/lib.rs`
- Create: `backend/crates/storage/src/postgres.rs`
- Create: `backend/crates/storage/src/redis.rs`
- Create: `backend/crates/storage/migrations/0001_init.sql`

- [ ] **Step 1: Create `backend/crates/storage/Cargo.toml`**

Write:

```toml
[package]
name = "coin-listener-storage"
edition.workspace = true
version.workspace = true
license.workspace = true

[dependencies]
coin-listener-core = { path = "../core" }
redis.workspace = true
sqlx.workspace = true
tracing.workspace = true
```

- [ ] **Step 2: Create `backend/crates/storage/src/lib.rs`**

Write:

```rust
pub mod postgres;
pub mod redis;

pub use postgres::{connect_postgres, run_migrations};
pub use redis::connect_redis;
```

- [ ] **Step 3: Create `backend/crates/storage/src/postgres.rs`**

Write:

```rust
use coin_listener_core::{AppError, AppResult, PostgresConfig};
use sqlx::{postgres::PgPoolOptions, PgPool};

pub async fn connect_postgres(config: &PostgresConfig) -> AppResult<PgPool> {
    PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn run_migrations(pool: &PgPool) -> AppResult<()> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}
```

- [ ] **Step 4: Create `backend/crates/storage/src/redis.rs`**

Write:

```rust
use coin_listener_core::{AppError, AppResult, RedisConfig};
use redis::Client;

pub fn connect_redis(config: &RedisConfig) -> AppResult<Client> {
    Client::open(config.redis_url.as_str()).map_err(|error| AppError::Redis(error.to_string()))
}
```

- [ ] **Step 5: Create initial migration**

Create `backend/crates/storage/migrations/0001_init.sql`:

```sql
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

CREATE TABLE IF NOT EXISTS schema_migrations_marker (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

INSERT INTO schema_migrations_marker (name)
VALUES ('0001_init')
ON CONFLICT (name) DO NOTHING;
```

- [ ] **Step 6: Run storage crate check**

Run:

```bash
cd backend && cargo check -p coin-listener-storage
```

Expected: command exits successfully.

---

## Task 4: Create API server with health check

**Files:**
- Create: `backend/crates/api-server/Cargo.toml`
- Create: `backend/crates/api-server/src/main.rs`
- Create: `backend/crates/api-server/src/routes.rs`

- [ ] **Step 1: Create `backend/crates/api-server/Cargo.toml`**

Write:

```toml
[package]
name = "api-server"
edition.workspace = true
version.workspace = true
license.workspace = true

[dependencies]
coin-listener-core = { path = "../core" }
coin-listener-storage = { path = "../storage" }
axum.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
tower-http.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
```

- [ ] **Step 2: Create `backend/crates/api-server/src/routes.rs`**

Write:

```rust
use axum::{extract::State, routing::get, Json, Router};
use serde::Serialize;
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Clone)]
pub struct ApiState {
    pub postgres: PgPool,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub service: &'static str,
}

pub fn build_router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .with_state(state)
}

async fn health(State(_state): State<Arc<ApiState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        service: "api-server",
    })
}
```

- [ ] **Step 3: Create `backend/crates/api-server/src/main.rs`**

Write:

```rust
mod routes;

use coin_listener_core::AppConfig;
use coin_listener_storage::{connect_postgres, run_migrations};
use routes::{build_router, ApiState};
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::info;
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

    let state = Arc::new(ApiState { postgres });
    let app = build_router(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let listener = TcpListener::bind(config.server_addr()).await?;
    info!(address = %listener.local_addr()?, "api server listening");

    axum::serve(listener, app).await?;
    Ok(())
}
```

- [ ] **Step 4: Run API server check**

Run:

```bash
cd backend && cargo check -p api-server
```

Expected: command exits successfully.

- [ ] **Step 5: Start dependencies and API server locally**

Run:

```bash
cp .env.example .env
docker compose up -d postgres redis
cd backend && DATABASE_URL=postgres://coin_listener:coin_listener_password@localhost:5432/coin_listener REDIS_URL=redis://localhost:6379 cargo run -p api-server
```

Expected: log contains `api server listening`.

- [ ] **Step 6: Verify health endpoint**

Run in another terminal:

```bash
curl http://localhost:8080/health
```

Expected:

```json
{"status":"ok","service":"api-server"}
```

---

## Task 5: Create scheduler, worker, and notifier placeholder services

**Files:**
- Create: `backend/crates/scheduler/Cargo.toml`
- Create: `backend/crates/scheduler/src/main.rs`
- Create: `backend/crates/worker/Cargo.toml`
- Create: `backend/crates/worker/src/main.rs`
- Create: `backend/crates/notifier/Cargo.toml`
- Create: `backend/crates/notifier/src/main.rs`

- [ ] **Step 1: Create `scheduler` crate**

Create `backend/crates/scheduler/Cargo.toml`:

```toml
[package]
name = "scheduler"
edition.workspace = true
version.workspace = true
license.workspace = true

[dependencies]
anyhow.workspace = true
coin-listener-core = { path = "../core" }
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
```

Create `backend/crates/scheduler/src/main.rs`:

```rust
use coin_listener_core::AppConfig;
use tokio::signal;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let _config = AppConfig::from_env()?;
    info!(service = "scheduler", "service started");
    signal::ctrl_c().await?;
    info!(service = "scheduler", "service stopped");
    Ok(())
}
```

- [ ] **Step 2: Run scheduler check**

Run:

```bash
cd backend && cargo check -p scheduler
```

Expected: scheduler compiles successfully.

- [ ] **Step 3: Create `worker` crate**

Create `backend/crates/worker/Cargo.toml`:

```toml
[package]
name = "worker"
edition.workspace = true
version.workspace = true
license.workspace = true

[dependencies]
anyhow.workspace = true
coin-listener-core = { path = "../core" }
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
```

Create `backend/crates/worker/src/main.rs`:

```rust
use coin_listener_core::AppConfig;
use tokio::signal;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let _config = AppConfig::from_env()?;
    info!(service = "worker", "service started");
    signal::ctrl_c().await?;
    info!(service = "worker", "service stopped");
    Ok(())
}
```

- [ ] **Step 4: Create `notifier` crate**

Create `backend/crates/notifier/Cargo.toml`:

```toml
[package]
name = "notifier"
edition.workspace = true
version.workspace = true
license.workspace = true

[dependencies]
anyhow.workspace = true
coin-listener-core = { path = "../core" }
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
```

Create `backend/crates/notifier/src/main.rs`:

```rust
use coin_listener_core::AppConfig;
use tokio::signal;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let _config = AppConfig::from_env()?;
    info!(service = "notifier", "service started");
    signal::ctrl_c().await?;
    info!(service = "notifier", "service stopped");
    Ok(())
}
```

- [ ] **Step 5: Run workspace check**

Run:

```bash
cd backend && cargo check --workspace
```

Expected: all crates compile successfully.

---

## Task 6: Create React + Semi UI frontend shell

**Files:**
- Create: `frontend/package.json`
- Create: `frontend/index.html`
- Create: `frontend/vite.config.ts`
- Create: `frontend/tsconfig.json`
- Create: `frontend/tsconfig.node.json`
- Create: `frontend/src/main.tsx`
- Create: `frontend/src/App.tsx`
- Create: `frontend/src/api/health.ts`
- Create: `frontend/src/styles.css`

- [ ] **Step 1: Create `frontend/package.json`**

Write:

```json
{
  "name": "coin-listener-frontend",
  "private": true,
  "version": "0.1.0",
  "type": "module",
  "scripts": {
    "dev": "vite --host 0.0.0.0",
    "build": "tsc && vite build",
    "preview": "vite preview --host 0.0.0.0"
  },
  "dependencies": {
    "@douyinfe/semi-icons": "latest",
    "@douyinfe/semi-ui": "latest",
    "@tanstack/react-query": "latest",
    "react": "latest",
    "react-dom": "latest"
  },
  "devDependencies": {
    "@types/react": "latest",
    "@types/react-dom": "latest",
    "@vitejs/plugin-react": "latest",
    "typescript": "latest",
    "vite": "latest"
  }
}
```

- [ ] **Step 2: Create Vite and TypeScript config files**

Create `frontend/index.html`:

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Coin Listener</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/main.tsx"></script>
  </body>
</html>
```

Create `frontend/vite.config.ts`:

```ts
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
  },
});
```

Create `frontend/tsconfig.json`:

```json
{
  "compilerOptions": {
    "target": "ES2020",
    "useDefineForClassFields": true,
    "lib": ["DOM", "DOM.Iterable", "ES2020"],
    "allowJs": false,
    "skipLibCheck": true,
    "esModuleInterop": true,
    "allowSyntheticDefaultImports": true,
    "strict": true,
    "forceConsistentCasingInFileNames": true,
    "module": "ESNext",
    "moduleResolution": "Node",
    "resolveJsonModule": true,
    "isolatedModules": true,
    "noEmit": true,
    "jsx": "react-jsx"
  },
  "include": ["src"],
  "references": [{ "path": "./tsconfig.node.json" }]
}
```

Create `frontend/tsconfig.node.json`:

```json
{
  "compilerOptions": {
    "composite": true,
    "module": "ESNext",
    "moduleResolution": "Node",
    "allowSyntheticDefaultImports": true
  },
  "include": ["vite.config.ts"]
}
```

- [ ] **Step 3: Create health API client**

Create `frontend/src/api/health.ts`:

```ts
export type HealthResponse = {
  status: string;
  service: string;
};

const apiBaseUrl = import.meta.env.VITE_API_BASE_URL ?? 'http://localhost:8080';

export async function fetchHealth(): Promise<HealthResponse> {
  const response = await fetch(`${apiBaseUrl}/health`);

  if (!response.ok) {
    throw new Error(`Health check failed with status ${response.status}`);
  }

  return response.json();
}
```

- [ ] **Step 4: Create React entrypoint**

Create `frontend/src/main.tsx`:

```tsx
import React from 'react';
import ReactDOM from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import '@douyinfe/semi-ui/dist/css/semi.min.css';
import './styles.css';
import { App } from './App';

const queryClient = new QueryClient();

ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <App />
    </QueryClientProvider>
  </React.StrictMode>,
);
```

- [ ] **Step 5: Create `frontend/src/App.tsx`**

Write:

```tsx
import { useQuery } from '@tanstack/react-query';
import { Banner, Button, Card, Layout, Nav, Space, Tag, Typography } from '@douyinfe/semi-ui';
import { IconPulse, IconServer } from '@douyinfe/semi-icons';
import { fetchHealth } from './api/health';

const { Header, Sider, Content } = Layout;
const { Title, Text } = Typography;

export function App() {
  const healthQuery = useQuery({
    queryKey: ['health'],
    queryFn: fetchHealth,
    retry: 1,
  });

  return (
    <Layout className="app-shell">
      <Sider className="app-sider">
        <div className="brand">Coin Listener</div>
        <Nav
          defaultSelectedKeys={['dashboard']}
          items={[
            { itemKey: 'dashboard', text: '仪表盘', icon: <IconPulse /> },
            { itemKey: 'system', text: '系统状态', icon: <IconServer /> },
          ]}
        />
      </Sider>
      <Layout>
        <Header className="app-header">
          <Title heading={4}>多链地址监听平台</Title>
        </Header>
        <Content className="app-content">
          <Space vertical align="start" spacing="large" className="content-stack">
            <Banner
              type="info"
              title="Milestone 1"
              description="当前版本提供工程骨架、API 健康检查、PostgreSQL、Redis 和 React + Semi UI 前端基础。"
            />
            <Card title="API 健康状态" className="status-card">
              {healthQuery.isLoading ? <Text>正在检查 API...</Text> : null}
              {healthQuery.isError ? (
                <Space vertical align="start">
                  <Tag color="red">API 不可用</Tag>
                  <Text type="danger">{healthQuery.error.message}</Text>
                  <Button onClick={() => healthQuery.refetch()}>重新检查</Button>
                </Space>
              ) : null}
              {healthQuery.data ? (
                <Space vertical align="start">
                  <Tag color="green">{healthQuery.data.status}</Tag>
                  <Text>服务：{healthQuery.data.service}</Text>
                  <Button onClick={() => healthQuery.refetch()}>刷新</Button>
                </Space>
              ) : null}
            </Card>
          </Space>
        </Content>
      </Layout>
    </Layout>
  );
}
```

- [ ] **Step 6: Create `frontend/src/styles.css`**

Write:

```css
html,
body,
#root {
  width: 100%;
  height: 100%;
  margin: 0;
}

.app-shell {
  min-height: 100vh;
  background: #f5f7fa;
}

.app-sider {
  min-height: 100vh;
  background: #ffffff;
  border-right: 1px solid #edf0f5;
}

.brand {
  height: 56px;
  display: flex;
  align-items: center;
  padding: 0 20px;
  font-weight: 700;
  font-size: 18px;
}

.app-header {
  height: 56px;
  display: flex;
  align-items: center;
  padding: 0 24px;
  background: #ffffff;
  border-bottom: 1px solid #edf0f5;
}

.app-content {
  padding: 24px;
}

.content-stack {
  width: 100%;
}

.status-card {
  width: 520px;
  max-width: 100%;
}
```

- [ ] **Step 7: Install frontend dependencies**

Run:

```bash
cd frontend && npm install
```

Expected: dependencies install successfully and `package-lock.json` is created.

- [ ] **Step 8: Build frontend**

Run:

```bash
cd frontend && npm run build
```

Expected: TypeScript and Vite build complete successfully.

---

## Task 7: Final local verification

**Files:**
- Modify only if previous tasks reveal compile or configuration errors.

- [ ] **Step 1: Check backend workspace**

Run:

```bash
cd backend && cargo fmt --all && cargo check --workspace
```

Expected: formatting succeeds and all backend crates compile.

- [ ] **Step 2: Check frontend build**

Run:

```bash
cd frontend && npm run build
```

Expected: frontend builds successfully.

- [ ] **Step 3: Run infrastructure**

Run:

```bash
cp .env.example .env
docker compose up -d postgres redis
```

Expected: PostgreSQL and Redis become healthy.

- [ ] **Step 4: Run API server**

Run:

```bash
cd backend && DATABASE_URL=postgres://coin_listener:coin_listener_password@localhost:5432/coin_listener REDIS_URL=redis://localhost:6379 cargo run -p api-server
```

Expected: server listens on `0.0.0.0:8080`.

- [ ] **Step 5: Verify API health**

Run:

```bash
curl http://localhost:8080/health
```

Expected:

```json
{"status":"ok","service":"api-server"}
```

- [ ] **Step 6: Run frontend dev server**

Run:

```bash
cd frontend && VITE_API_BASE_URL=http://localhost:8080 npm run dev
```

Expected: Vite prints a local URL and the page shows API status `ok`.

- [ ] **Step 7: Commit if repository has been initialized**

If this directory is initialized as a git repository, run:

```bash
git add .
git commit -m "chore: bootstrap coin listener foundation"
```

Expected: commit succeeds. If the directory is not a git repository, skip this step.
