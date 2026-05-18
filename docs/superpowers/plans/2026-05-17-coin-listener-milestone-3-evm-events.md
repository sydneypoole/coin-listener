# Coin Listener Milestone 3 EVM Events Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add event models, balance snapshots, EVM/Base event normalization, event query APIs, and an event center frontend with `is_transfer` filtering.

**Architecture:** Add database tables for balance snapshots and address events, shared Rust event models, storage repositories, API filters, and a lightweight EVM provider abstraction. Since real RPC integration requires external provider keys, this milestone creates provider boundaries and a deterministic mock scanner path that later workers can replace with real Alloy RPC calls.

**Tech Stack:** Rust, Axum, SQLx, PostgreSQL, Serde, Uuid, React, TypeScript, Semi UI, TanStack Query.

---

## Scope

Implements Milestone 3 from the design spec:

- Event model.
- Balance snapshot model.
- EVM/Base provider boundary.
- Native transfer / ERC20 transfer normalized shapes.
- `is_transfer` classification.
- Event query API with filters.
- Event center frontend.

Does not implement live RPC scanning, real block subscriptions, Redis scan queues, or notification sending.

## File Structure

Modify:

```text
backend/Cargo.toml
backend/crates/core/Cargo.toml
backend/crates/core/src/models.rs
backend/crates/storage/src/repositories.rs
backend/crates/api-server/src/routes.rs
frontend/src/App.tsx
frontend/src/api/client.ts
frontend/src/api/types.ts
frontend/src/styles.css
```

Create:

```text
backend/crates/storage/migrations/0003_events.sql
backend/crates/chain-providers/Cargo.toml
backend/crates/chain-providers/src/lib.rs
backend/crates/chain-providers/src/evm.rs
frontend/src/pages/EventsPage.tsx
```

## Task 1: Add event schema and models

- [ ] Add `balance_snapshots` table.
- [ ] Add `address_events` table.
- [ ] Add event indexes for tenant/time, transfer/time, chain/time, address/time.
- [ ] Add Rust models and filter/request types.
- [ ] Check backend.

## Task 2: Add EVM/Base provider abstraction

- [ ] Create `chain-providers` crate.
- [ ] Add EVM raw transfer structs.
- [ ] Add classifier converting raw transfers and balance deltas into normalized `AddressEventDraft`.
- [ ] Add deterministic mock scan function for development without RPC credentials.
- [ ] Check backend.

## Task 3: Add event storage and API

- [ ] Add repository functions to list events with filters.
- [ ] Add repository function to create mock EVM/Base event for a watched address.
- [ ] Add `GET /api/events` with `is_transfer`, chain, address, asset, event_type, direction filters.
- [ ] Add `POST /api/dev/scan-address/:id` to generate deterministic mock event for development.
- [ ] Check backend.

## Task 4: Add event center frontend

- [ ] Add typed event API client.
- [ ] Add event center page with filters.
- [ ] Add `is_transfer` filter.
- [ ] Add dev scan action on address page or event page.
- [ ] Build frontend.

## Task 5: Verify Milestone 3

- [ ] Run backend fmt/check.
- [ ] Run frontend build.
- [ ] Run compose config.
- [ ] Document any Docker daemon limitation.
