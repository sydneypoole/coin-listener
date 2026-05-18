# Coin Listener Milestone 2 Config Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add users, tenants, chains, providers, assets, watched addresses, basic auth, and React + Semi UI management screens.

**Architecture:** Extend the existing Rust workspace with SQLx migrations, storage repositories, Axum routes, and React pages. This milestone keeps authentication intentionally simple: a seeded admin user and token-style login response for frontend flow, with full password/JWT hardening deferred to later security hardening.

**Tech Stack:** Rust, Axum, SQLx, PostgreSQL, Serde, Uuid, React, TypeScript, Semi UI, TanStack Query.

---

## Scope

Implements Milestone 2 from the design spec:

- Login.
- Default tenant.
- Chain configuration.
- Provider configuration.
- Asset configuration.
- Address management.
- Address format validation.

Does not implement real chain scanning, event generation, notifications, or worker queue behavior.

## File Structure

Modify existing files:

```text
backend/Cargo.toml
backend/crates/core/src/lib.rs
backend/crates/core/src/error.rs
backend/crates/storage/src/lib.rs
backend/crates/api-server/src/main.rs
backend/crates/api-server/src/routes.rs
frontend/src/App.tsx
frontend/src/styles.css
```

Create files:

```text
backend/crates/core/src/models.rs
backend/crates/storage/src/repositories.rs
backend/crates/storage/migrations/0002_config_management.sql
frontend/src/api/client.ts
frontend/src/api/types.ts
frontend/src/pages/LoginPage.tsx
frontend/src/pages/ChainsPage.tsx
frontend/src/pages/AssetsPage.tsx
frontend/src/pages/ProvidersPage.tsx
frontend/src/pages/AddressesPage.tsx
```

---

## Task 1: Add domain models and migration

**Files:**
- Create: `backend/crates/core/src/models.rs`
- Modify: `backend/crates/core/src/lib.rs`
- Modify: `backend/crates/core/src/error.rs`
- Modify: `backend/crates/storage/Cargo.toml`
- Create: `backend/crates/storage/migrations/0002_config_management.sql`

- [ ] Add shared Rust models for users, tenants, chains, providers, assets, and watched addresses.
- [ ] Add `NotFound`, `Validation`, and `Unauthorized` variants to `AppError`.
- [ ] Add SQLx migration with tables and seed data for default tenant, admin user, BTC/ETH/TRON/BASE chains, and builtin assets.
- [ ] Run `cargo fmt --all`.
- [ ] Run backend `cargo check --workspace` with local `CARGO_HOME`.

## Task 2: Add storage repositories

**Files:**
- Create: `backend/crates/storage/src/repositories.rs`
- Modify: `backend/crates/storage/src/lib.rs`

- [ ] Add repository functions for login lookup, listing chains, providers, assets, and watched addresses.
- [ ] Add create/update/delete watched address functions.
- [ ] Add create/update provider functions.
- [ ] Validate address format by chain type before insert/update.
- [ ] Run backend `cargo check --workspace` with local `CARGO_HOME`.

## Task 3: Add API routes

**Files:**
- Modify: `backend/crates/api-server/src/routes.rs`
- Modify: `backend/crates/api-server/src/main.rs` if needed

- [ ] Add `POST /api/auth/login`.
- [ ] Add `GET /api/chains`.
- [ ] Add `GET /api/assets`.
- [ ] Add `GET /api/providers` and `POST /api/providers`.
- [ ] Add `GET /api/addresses`, `POST /api/addresses`, `PUT /api/addresses/:id`, `DELETE /api/addresses/:id`.
- [ ] Return JSON error responses for validation and not found cases.
- [ ] Run backend `cargo check --workspace` with local `CARGO_HOME`.

## Task 4: Add frontend API client and pages

**Files:**
- Create: `frontend/src/api/client.ts`
- Create: `frontend/src/api/types.ts`
- Create: `frontend/src/pages/LoginPage.tsx`
- Create: `frontend/src/pages/ChainsPage.tsx`
- Create: `frontend/src/pages/AssetsPage.tsx`
- Create: `frontend/src/pages/ProvidersPage.tsx`
- Create: `frontend/src/pages/AddressesPage.tsx`
- Modify: `frontend/src/App.tsx`
- Modify: `frontend/src/styles.css`

- [ ] Add typed API client.
- [ ] Add login page.
- [ ] Add chains, assets, providers, and addresses pages using Semi UI Table/Form/Modal.
- [ ] Add sidebar navigation between pages.
- [ ] Run frontend build.

## Task 5: Verify Milestone 2

**Files:**
- Modify only if verification reveals issues.

- [ ] Run `cargo fmt --all`.
- [ ] Run backend `cargo check --workspace` with local `CARGO_HOME`.
- [ ] Run frontend `npm run build`.
- [ ] Run `docker compose config`.
- [ ] Record Docker daemon limitations if full container startup is blocked by local permissions.
