# Multi-Provider Connectivity Test Design

## Goal

Provider management should test every supported chain provider type that the product can scan, not only EVM RPC providers.

## Current behavior

- `POST /api/providers/:id/test` only accepts `provider_type = "rpc"` on EVM chains.
- The EVM test calls `eth_blockNumber` and returns `latest_block`.
- The frontend disables test buttons for non-EVM RPC providers and says only EVM/Base RPC can be tested.
- Worker scanning already uses dedicated clients for EVM, TRON, and BTC/UTXO providers.

## Scope

Included:

- EVM provider connectivity test.
- TRON REST provider connectivity test.
- BTC/UTXO Esplora-style REST provider connectivity test.
- Frontend Provider page button and messaging updates.
- Source/unit/UI-regression tests for supported and unsupported combinations.

Excluded:

- WebSocket handshake testing.
- Running full address scans from the provider test endpoint.
- Updating provider health, cursor, balances, events, or notification state from manual tests.
- API-key secret resolution through `api_key_ref`.

## Backend design

Create a focused provider connectivity dispatcher used by `POST /api/providers/:id/test`.

Supported combinations:

| `chain.chain_type` | `provider.provider_type` | Test action | Success data |
|---|---|---|---|
| `evm` | `rpc` | JSON-RPC `eth_blockNumber` | `latest_block` |
| `tron` | `rest_api` or `rpc` | TRON REST `GET /v1/accounts/{probe_address}/transactions?limit=1&min_timestamp=0` | message only |
| `utxo` | `rest_api` | Esplora REST `GET /blocks/tip/height` | `latest_block` |

Unsupported combinations return validation errors with explicit messages, for example:

- `provider connectivity test does not support websocket providers`
- `provider connectivity test does not support rpc providers for utxo chains`
- `provider connectivity test does not support rest_api providers for evm chains`

### Probe values

TRON needs a valid address shape to hit the existing account endpoint. Use a constant mainnet-shaped probe address only for connectivity testing:

```text
TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh
```

This endpoint may return empty data for the account; that is acceptable. The test succeeds if the provider returns a successful HTTP status and parseable TRON page shape.

BTC/UTXO uses `/blocks/tip/height`, so no address is needed.

### Response shape

Extend `ProviderTestResponse` without breaking existing frontend code:

```rust
pub struct ProviderTestResponse {
    pub ok: bool,
    pub message: String,
    pub latest_block: Option<i64>,
    pub chain_type: String,
    pub provider_type: String,
}
```

Message examples:

- EVM: `EVM RPC reachable`
- TRON: `TRON REST provider reachable`
- BTC/UTXO: `BTC REST provider reachable`

### Error handling

- Use existing `timeout_ms` validation.
- Network/request/status/body errors remain `AppError::Config` so the API returns a clear failed test response.
- Provider URLs and secrets must stay redacted in error strings.
- Manual provider tests must not record provider health success/failure. Health remains owned by worker scan attempts.

## Chain provider client changes

Add minimal reusable health methods rather than embedding URLs in routes:

- `TronClient::test_connectivity()` calls one lightweight account transactions page using the probe address, `min_timestamp = 0`, no fingerprint.
- `BtcClient::tip_height()` calls `/blocks/tip/height` and parses a non-negative integer.

EVM keeps `EvmRpcClient::eth_block_number()`.

## Frontend design

Provider page changes:

- Replace `canTestProvider` with supported-combination logic matching the backend.
- Enable `测试` for:
  - EVM + RPC
  - TRON + REST API/RPC
  - UTXO + REST API
- Disable unsupported combinations with label `暂不支持测试`.
- Replace modal help text with: `支持 EVM RPC、TRON REST、BTC/UTXO REST Provider 连通性测试；WebSocket 暂不测试。`
- Toast uses backend `message`; if `latest_block` is present, append latest block height.

## Testing strategy

Backend:

- Route/source-level tests verify the provider test route dispatches EVM, TRON, and UTXO branches.
- Chain-provider unit tests cover:
  - TRON connectivity uses the probe address path/query.
  - BTC tip height path is `/blocks/tip/height`.
  - BTC tip height parser rejects negative and non-integer responses.
- Unsupported combination tests verify explicit validation messages.

Frontend:

- UI regression verifies:
  - `canTestProvider` no longer limits tests to EVM RPC only.
  - User-facing text mentions EVM RPC, TRON REST, BTC/UTXO REST, and WebSocket unsupported.
  - Toast uses `result.message` and `latest_block`.

## Acceptance criteria

- EVM RPC provider test behavior remains compatible.
- TRON provider can be tested from the Provider page.
- BTC/UTXO REST provider can be tested from the Provider page.
- Unsupported provider combinations are disabled in the UI and rejected by the backend.
- Provider test endpoint does not mutate scan state or provider health.
- Backend tests and frontend UI regression pass.
