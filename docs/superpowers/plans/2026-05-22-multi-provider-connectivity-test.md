# Multi-Provider Connectivity Test Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend manual Provider connectivity testing beyond EVM RPC so supported TRON and BTC/UTXO REST providers can be tested from the existing Provider page.

**Architecture:** Keep the existing `POST /api/providers/:id/test` endpoint and add a small backend dispatcher by `chain_type` + `provider_type`. Reuse chain-provider clients for protocol-specific probes: EVM keeps `eth_blockNumber`, TRON performs a lightweight account transactions request against a constant probe address, and BTC/UTXO reads Esplora `/blocks/tip/height`. Frontend support mirrors backend support logic and uses the backend response message for consistent feedback.

**Tech Stack:** Rust, Axum, reqwest, serde, SQLx model structs, React, TypeScript, TanStack Query, Semi Design, Node UI regression tests.

---

## File map

- Modify `backend/crates/chain-providers/src/tron.rs`
  - Add `TRON_CONNECTIVITY_PROBE_ADDRESS`.
  - Add `account_transactions_connectivity_query()` returning `limit=1` and `min_timestamp=0`.
  - Add `TronClient::test_connectivity()`.
  - Add unit tests for the probe address, query shape, and method source references.

- Modify `backend/crates/chain-providers/src/btc.rs`
  - Add `BtcClient::tip_height_path()`.
  - Add `BtcClient::tip_height()`.
  - Add `parse_btc_tip_height()`.
  - Add unit tests for path and parser behavior.

- Modify `backend/crates/api-server/src/routes.rs`
  - Import `BtcClient` and `TronClient`.
  - Extend `ProviderTestResponse` with `chain_type` and `provider_type`.
  - Replace `test_provider()` EVM-only checks with dispatcher helpers.
  - Add source-level tests for dispatch support and unsupported combinations.

- Modify `frontend/src/api/types.ts`
  - Add `chain_type` and `provider_type` to `ProviderTestResponse`.

- Modify `frontend/src/pages/ProvidersPage.tsx`
  - Replace EVM-only `canTestProvider` logic with supported-combination logic.
  - Update toast and disabled-label behavior.
  - Update modal help text.

- Modify `frontend/src/ui-regression.test.ts`
  - Update provider-management regression assertions for multi-provider testing.

---

### Task 1: Add TRON connectivity probe helper

**Files:**
- Modify: `backend/crates/chain-providers/src/tron.rs`

- [ ] **Step 1: Write failing tests for TRON probe query and client method source**

Add these tests inside `#[cfg(test)] mod tests` in `backend/crates/chain-providers/src/tron.rs`:

```rust
#[test]
fn tron_connectivity_query_uses_probe_address_and_minimal_limit() {
    let query = super::account_transactions_connectivity_query();

    assert_eq!(
        super::TRON_CONNECTIVITY_PROBE_ADDRESS,
        "TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh"
    );
    assert!(query.contains(&("only_confirmed", "true".to_string())));
    assert!(query.contains(&("limit", "1".to_string())));
    assert!(query.contains(&("min_timestamp", "0".to_string())));
    assert!(!query.iter().any(|(key, _)| *key == "fingerprint"));
}

#[test]
fn tron_connectivity_method_uses_account_transactions_probe() {
    let source = include_str!("tron.rs");
    let start = source
        .find("pub async fn test_connectivity(&self)")
        .expect("test_connectivity exists");
    let end = source[start..]
        .find("pub async fn account_transactions(")
        .expect("account_transactions follows connectivity method")
        + start;
    let method = &source[start..end];

    assert!(method.contains("TRON_CONNECTIVITY_PROBE_ADDRESS"));
    assert!(method.contains("account_transactions_path"));
    assert!(method.contains("account_transactions_connectivity_query()"));
    assert!(method.contains("parse_tron_page(body, \"connectivity probe\")"));
}
```

- [ ] **Step 2: Run TRON tests to verify they fail**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml -p coin-listener-chain-providers tron_connectivity
```

Expected: FAIL because `TRON_CONNECTIVITY_PROBE_ADDRESS`, `account_transactions_connectivity_query`, and `test_connectivity` do not exist.

- [ ] **Step 3: Implement TRON connectivity helper**

In `backend/crates/chain-providers/src/tron.rs`, add this constant near the existing top-level constants/types:

```rust
pub const TRON_CONNECTIVITY_PROBE_ADDRESS: &str = "TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh";
```

Add this function near `account_transactions_query`:

```rust
pub fn account_transactions_connectivity_query() -> Vec<(&'static str, String)> {
    vec![
        ("only_confirmed", "true".to_string()),
        ("limit", "1".to_string()),
        ("min_timestamp", "0".to_string()),
    ]
}
```

Add this method at the top of `impl TronClient`, before `account_transactions`:

```rust
pub async fn test_connectivity(&self) -> AppResult<()> {
    let path = self.account_transactions_path(TRON_CONNECTIVITY_PROBE_ADDRESS)?;
    let query = account_transactions_connectivity_query();
    let body = self
        .get_json_body("connectivity probe", &path, &query)
        .await?;
    parse_tron_page(body, "connectivity probe")?;
    Ok(())
}
```

- [ ] **Step 4: Run TRON tests to verify they pass**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml -p coin-listener-chain-providers tron_connectivity
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add backend/crates/chain-providers/src/tron.rs
git commit -m "添加TRON Provider连通性探针"
```

---

### Task 2: Add BTC/UTXO tip-height connectivity helper

**Files:**
- Modify: `backend/crates/chain-providers/src/btc.rs`

- [ ] **Step 1: Write failing BTC path and parser tests**

Add these tests inside `#[cfg(test)] mod tests` in `backend/crates/chain-providers/src/btc.rs`:

```rust
#[test]
fn btc_tip_height_path_targets_esplora_tip_height() {
    let client = super::BtcClient::new(
        "https://mempool.space/api/".to_string(),
        std::time::Duration::from_secs(5),
    );

    assert_eq!(client.tip_height_path(), "/blocks/tip/height");
}

#[test]
fn btc_tip_height_parser_accepts_non_negative_integer_text() {
    assert_eq!(super::parse_btc_tip_height("840000").unwrap(), 840000);
    assert_eq!(super::parse_btc_tip_height(" 0\n").unwrap(), 0);
}

#[test]
fn btc_tip_height_parser_rejects_invalid_or_negative_text() {
    for payload in ["", "not-a-number", "-1", "1.2"] {
        let error = super::parse_btc_tip_height(payload).unwrap_err().to_string();
        assert!(error.contains("invalid BTC tip height"), "{payload}: {error}");
    }
}
```

- [ ] **Step 2: Run BTC tests to verify they fail**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml -p coin-listener-chain-providers btc_tip_height
```

Expected: FAIL because `tip_height_path` and `parse_btc_tip_height` do not exist.

- [ ] **Step 3: Implement BTC helper**

In `impl BtcClient` in `backend/crates/chain-providers/src/btc.rs`, add:

```rust
pub fn tip_height_path(&self) -> &'static str {
    "/blocks/tip/height"
}

pub async fn tip_height(&self) -> AppResult<i64> {
    let path = self.tip_height_path();
    let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
    let response = self.client.get(&url).send().await.map_err(|error| {
        AppError::Config(format_btc_request_error(
            "tip height",
            &self.base_url,
            &error.without_url().to_string(),
        ))
    })?;
    let status = response.status();
    let body = response.text().await.map_err(|error| {
        AppError::Config(format!(
            "BTC tip height response body failed: {}",
            error.without_url()
        ))
    })?;
    if !status.is_success() {
        return Err(AppError::Config(format_btc_status_error(
            "tip height",
            &self.base_url,
            status,
            &body,
        )));
    }
    parse_btc_tip_height(&body)
}
```

Add this function near `decode_btc_transaction_page`:

```rust
pub fn parse_btc_tip_height(payload: &str) -> AppResult<i64> {
    let value = payload.trim().parse::<i64>().map_err(|error| {
        AppError::Validation(format!("invalid BTC tip height {payload:?}: {error}"))
    })?;
    if value < 0 {
        return Err(AppError::Validation(format!(
            "invalid BTC tip height {payload:?}: must be non-negative"
        )));
    }
    Ok(value)
}
```

- [ ] **Step 4: Run BTC tests to verify they pass**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml -p coin-listener-chain-providers btc_tip_height
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add backend/crates/chain-providers/src/btc.rs
git commit -m "添加BTC Provider连通性探针"
```

---

### Task 3: Dispatch Provider tests by chain and provider type in backend

**Files:**
- Modify: `backend/crates/api-server/src/routes.rs`

- [ ] **Step 1: Write failing backend source-level tests**

In `#[cfg(test)] mod tests` in `backend/crates/api-server/src/routes.rs`, add:

```rust
#[test]
fn provider_test_response_includes_chain_and_provider_type() {
    let response = super::ProviderTestResponse {
        ok: true,
        message: "TRON REST provider reachable".to_string(),
        latest_block: None,
        chain_type: "tron".to_string(),
        provider_type: "rest_api".to_string(),
    };
    let json = serde_json::to_value(response).unwrap();

    assert_eq!(json["ok"], true);
    assert_eq!(json["message"], "TRON REST provider reachable");
    assert!(json["latest_block"].is_null());
    assert_eq!(json["chain_type"], "tron");
    assert_eq!(json["provider_type"], "rest_api");
}

#[test]
fn provider_test_dispatch_supports_evm_tron_and_utxo_only() {
    assert_eq!(super::provider_test_kind("evm", "rpc").unwrap(), super::ProviderTestKind::EvmRpc);
    assert_eq!(super::provider_test_kind("tron", "rest_api").unwrap(), super::ProviderTestKind::TronRest);
    assert_eq!(super::provider_test_kind("tron", "rpc").unwrap(), super::ProviderTestKind::TronRest);
    assert_eq!(super::provider_test_kind("utxo", "rest_api").unwrap(), super::ProviderTestKind::BtcRest);

    let websocket = super::provider_test_kind("evm", "websocket").unwrap_err().to_string();
    assert!(websocket.contains("websocket providers"));

    let utxo_rpc = super::provider_test_kind("utxo", "rpc").unwrap_err().to_string();
    assert!(utxo_rpc.contains("rpc providers for utxo chains"));

    let evm_rest = super::provider_test_kind("evm", "rest_api").unwrap_err().to_string();
    assert!(evm_rest.contains("rest_api providers for evm chains"));
}

#[test]
fn provider_test_handler_source_uses_all_supported_clients() {
    let source = include_str!("routes.rs");
    assert!(source.contains("BtcClient"));
    assert!(source.contains("TronClient"));
    assert!(source.contains("ProviderTestKind::EvmRpc"));
    assert!(source.contains("ProviderTestKind::TronRest"));
    assert!(source.contains("ProviderTestKind::BtcRest"));
    assert!(source.contains("eth_block_number().await"));
    assert!(source.contains("test_connectivity().await"));
    assert!(source.contains("tip_height().await"));
}
```

- [ ] **Step 2: Run backend route tests to verify they fail**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml -p api-server provider_test
```

Expected: FAIL because `ProviderTestResponse` lacks fields, `ProviderTestKind` does not exist, and BTC/TRON imports are missing.

- [ ] **Step 3: Implement backend dispatcher**

Change the import near the top of `routes.rs` from:

```rust
use coin_listener_chain_providers::evm::EvmRpcClient;
```

to:

```rust
use coin_listener_chain_providers::{btc::BtcClient, evm::EvmRpcClient, tron::TronClient};
```

Change `ProviderTestResponse` to:

```rust
#[derive(Debug, Serialize)]
pub struct ProviderTestResponse {
    pub ok: bool,
    pub message: String,
    pub latest_block: Option<i64>,
    pub chain_type: String,
    pub provider_type: String,
}
```

Add this enum and helper near `ProviderTestResponse`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderTestKind {
    EvmRpc,
    TronRest,
    BtcRest,
}

pub fn provider_test_kind(chain_type: &str, provider_type: &str) -> AppResult<ProviderTestKind> {
    match (chain_type, provider_type) {
        (_, "websocket") => Err(AppError::Validation(
            "provider connectivity test does not support websocket providers".to_string(),
        )),
        ("evm", "rpc") => Ok(ProviderTestKind::EvmRpc),
        ("tron", "rest_api" | "rpc") => Ok(ProviderTestKind::TronRest),
        ("utxo", "rest_api") => Ok(ProviderTestKind::BtcRest),
        ("evm", other) => Err(AppError::Validation(format!(
            "provider connectivity test does not support {other} providers for evm chains"
        ))),
        ("tron", other) => Err(AppError::Validation(format!(
            "provider connectivity test does not support {other} providers for tron chains"
        ))),
        ("utxo", other) => Err(AppError::Validation(format!(
            "provider connectivity test does not support {other} providers for utxo chains"
        ))),
        (chain_type, _) => Err(AppError::Validation(format!(
            "provider connectivity test does not support {chain_type} chains"
        ))),
    }
}
```

Replace `test_provider` with:

```rust
async fn test_provider(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let provider = repositories::get_provider(&state.postgres, id).await?;
    let timeout_ms = u64::try_from(provider.timeout_ms)
        .map_err(|_| AppError::Validation("timeout_ms must be positive".to_string()))?;
    if timeout_ms == 0 {
        return Err(AppError::Validation("timeout_ms must be positive".to_string()).into());
    }

    let chain = repositories::chain_by_id(&state.postgres, provider.chain_id).await?;
    let timeout = StdDuration::from_millis(timeout_ms);
    let kind = provider_test_kind(&chain.chain_type, &provider.provider_type)?;

    let (message, latest_block) = match kind {
        ProviderTestKind::EvmRpc => {
            let client = EvmRpcClient::new(provider.base_url.clone(), timeout);
            let latest_block = client.eth_block_number().await?;
            ("EVM RPC reachable".to_string(), Some(latest_block))
        }
        ProviderTestKind::TronRest => {
            let client = TronClient::new(provider.base_url.clone(), timeout);
            client.test_connectivity().await?;
            ("TRON REST provider reachable".to_string(), None)
        }
        ProviderTestKind::BtcRest => {
            let client = BtcClient::new(provider.base_url.clone(), timeout);
            let latest_block = client.tip_height().await?;
            ("BTC REST provider reachable".to_string(), Some(latest_block))
        }
    };

    Ok(Json(ProviderTestResponse {
        ok: true,
        message,
        latest_block,
        chain_type: chain.chain_type,
        provider_type: provider.provider_type,
    })
    .into_response())
}
```

- [ ] **Step 4: Run backend route tests to verify they pass**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml -p api-server provider_test
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add backend/crates/api-server/src/routes.rs
git commit -m "支持多类型Provider连通性测试"
```

---

### Task 4: Update frontend Provider page and API type

**Files:**
- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/pages/ProvidersPage.tsx`
- Modify: `frontend/src/ui-regression.test.ts`

- [ ] **Step 1: Write failing UI regression assertions**

In `frontend/src/ui-regression.test.ts`, update the provider management test to assert the new behavior:

```ts
test('provider management exposes multi-provider connectivity test controls', () => {
  const page = readSource('pages/ProvidersPage.tsx');
  const client = readSource('api/client.ts');
  const types = readSource('api/types.ts');

  expectContains(client, 'export function updateProvider');
  expectContains(client, 'export function testProvider');
  expectContains(types, 'chain_type: string');
  expectContains(types, 'provider_type: string');
  expectContains(page, 'editingProvider');
  expectContains(page, 'updateProvider');
  expectContains(page, 'testProvider');
  expectContains(page, 'providerTestSupported');
  expectContains(page, "chainType === 'evm' && provider.provider_type === 'rpc'");
  expectContains(page, "chainType === 'tron' && ['rest_api', 'rpc'].includes(provider.provider_type)");
  expectContains(page, "chainType === 'utxo' && provider.provider_type === 'rest_api'");
  expectContains(page, 'result.message');
  expectContains(page, '最新区块');
  expectContains(page, '暂不支持测试');
  expectContains(page, '支持 EVM RPC、TRON REST、BTC/UTXO REST Provider 连通性测试；WebSocket 暂不测试。');
  expectContains(page, 'value="websocket"');
  expectContains(page, 'value="rest_api"');
  expectContains(page, 'rules={[{ required: true, message: \'请输入优先级\' }]');
  expectContains(page, 'min={1}');
});
```

- [ ] **Step 2: Run UI regression to verify it fails**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: FAIL because the frontend still contains the EVM-only assertions and `ProviderTestResponse` lacks the new fields.

- [ ] **Step 3: Update frontend API type**

Change `ProviderTestResponse` in `frontend/src/api/types.ts` to:

```ts
export type ProviderTestResponse = {
  ok: boolean;
  message: string;
  latest_block?: number | null;
  chain_type: string;
  provider_type: string;
};
```

- [ ] **Step 4: Update Provider page logic and messaging**

In `frontend/src/pages/ProvidersPage.tsx`, change `testMutation.onSuccess` to:

```tsx
onSuccess: result => {
  Toast.success(result.latest_block === null || result.latest_block === undefined
    ? result.message
    : `${result.message}，最新区块 ${result.latest_block}`);
},
```

Replace `canTestProvider` with:

```tsx
function providerTestSupported(provider: Provider) {
  const chainType = chainTypeMap.get(provider.chain_id);
  return (
    (chainType === 'evm' && provider.provider_type === 'rpc')
    || (chainType === 'tron' && ['rest_api', 'rpc'].includes(provider.provider_type))
    || (chainType === 'utxo' && provider.provider_type === 'rest_api')
  );
}
```

Update operation rendering:

```tsx
const testDisabled = !providerTestSupported(provider);
```

Update the button label:

```tsx
{testDisabled ? '暂不支持测试' : '测试'}
```

Replace modal help text with:

```tsx
<p className="form-help-text">支持 EVM RPC、TRON REST、BTC/UTXO REST Provider 连通性测试；WebSocket 暂不测试。</p>
```

- [ ] **Step 5: Run UI regression to verify it passes**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/api/types.ts frontend/src/pages/ProvidersPage.tsx frontend/src/ui-regression.test.ts
git commit -m "更新多Provider测试前端入口"
```

---

### Task 5: Final verification

**Files:**
- Verify only unless tests reveal required fixes.

- [ ] **Step 1: Run backend formatting for changed crates**

Run:

```bash
cargo fmt --manifest-path backend/crates/chain-providers/Cargo.toml --check
cargo fmt --manifest-path backend/crates/api-server/Cargo.toml --check
```

Expected: both commands exit 0.

- [ ] **Step 2: Run backend tests**

Run:

```bash
cargo test --locked --manifest-path backend/Cargo.toml
```

Expected: all backend tests pass.

- [ ] **Step 3: Run frontend UI regression**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: all UI regression tests pass.

- [ ] **Step 4: Run frontend build**

Run:

```bash
npm --prefix frontend run build
```

Expected: build exits 0. Existing Vite warnings about large chunks or known dependencies are acceptable only if the command succeeds.

- [ ] **Step 5: Confirm no verification-only changes remain**

Run:

```bash
git status --short
```

Expected: only the intentional commits from Tasks 1-4 are present in history, and no unstaged verification-only fixes remain. If any verification command failed, stop and diagnose that failure before creating another commit.
