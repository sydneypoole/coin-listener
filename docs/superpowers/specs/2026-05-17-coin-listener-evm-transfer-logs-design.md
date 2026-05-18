# Coin Listener EVM Transfer Logs Design

日期：2026-05-17

## 1. 目标

在已完成的真实 EVM 原生余额扫描基础上，增加基于 `eth_getLogs` 的 EVM / BASE ERC20 `Transfer` 事件扫描能力。系统应能从已配置的 active ERC20 assets 中扫描 watched address 的转入和转出日志，归一化为 `address_events`，并复用现有通知队列。

本阶段目标是提供稳定、可增量执行的 polling path，不引入 WebSocket subscription、provider failover 或全历史回溯。

## 2. 范围

### 2.1 包含

- 为 EVM JSON-RPC client 增加 `eth_getLogs` 支持。
- 构造 ERC20 `Transfer(address,address,uint256)` topic filter。
- 分别扫描 watched address 作为 `from` 和 `to` 的 Transfer logs。
- 解码 EVM log：`tx_hash`、`log_index`、`block_number`、`block_hash`、`from_address`、`to_address`、`amount_raw`。
- 使用 asset decimals 生成 `amount_decimal`。
- 将 decoded transfer 转为 `AddressEventDraft`，并设置 `is_transfer = true`。
- 为 EVM ERC20 transfer scan 增加区块 cursor。
- 使用 confirmations 计算 confirmed block range。
- 使用数据库唯一约束或 insert-if-not-exists 语义防止重复事件。
- worker 在同一个 EVM scan task 中执行 native balance scan 和 ERC20 transfer log scan。
- 只有新插入的 events 才 enqueue notify tasks。
- 保持现有 event center 和 notification API 不变，直接消费新事件。

### 2.2 不包含

- Native ETH / BASE 普通交易扫描。
- Internal transaction 扫描。
- WebSocket subscription。
- ERC20 token 自动发现。
- 任意合约 ABI 解析。
- Provider failover。
- 复杂 reorg 回滚。
- 历史全量回溯 UI。
- 前端新页面或筛选器改造。

## 3. 架构

```text
worker scan task
  -> load watched address context
  -> run existing native balance snapshot scan
  -> load active ERC20 assets for chain
  -> load or initialize scan cursor
  -> latest = eth_blockNumber
  -> confirmed_to = latest - chain.default_confirmations
  -> from_block = cursor.last_scanned_block + 1 or initial window start
  -> for each active ERC20 asset:
       eth_getLogs Transfer(to = watched)
       eth_getLogs Transfer(from = watched)
       decode logs
       classify direction
       insert event if not exists
       enqueue notify task for inserted event
  -> update cursor to confirmed_to
  -> finish address scan
```

`chain-providers` 只负责 JSON-RPC payload、log decode 和 event draft creation，不访问数据库。

`storage` 负责 cursor、asset lookup、event dedupe insert。

`worker` 负责 orchestration、错误传播、通知入队顺序和 scan completion。

## 4. RPC 设计

### 4.1 `eth_getLogs`

请求格式：

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "eth_getLogs",
  "params": [
    {
      "address": "0xTokenContract",
      "fromBlock": "0x...",
      "toBlock": "0x...",
      "topics": ["0xddf252ad...", null, "0x000000000000000000000000WatchedAddress"]
    }
  ]
}
```

`EvmRpcClient` 新增：

```rust
pub async fn eth_get_logs(&self, filter: EvmLogFilter) -> AppResult<Vec<EvmLog>>
```

### 4.2 Transfer topics

`topic0` 固定为：

```text
keccak256("Transfer(address,address,uint256)")
= 0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef
```

watched address topic 编码：

```text
0x + 24 zero hex chars + lowercase 20-byte address without 0x
```

扫描两个方向：

```text
incoming: topics = [TRANSFER_TOPIC0, null, watched_topic]
outgoing: topics = [TRANSFER_TOPIC0, watched_topic, null]
```

如果同一 log 同时命中 incoming 和 outgoing，分类为 `self`，去重 insert 只保留一条 event。

## 5. Log decode 规则

输入 log 必须满足：

- `address` 是 token contract。
- `topics.len() >= 3`。
- `topics[0] == TRANSFER_TOPIC0`。
- `topics[1]` 和 `topics[2]` 是 32-byte encoded address topic。
- `data` 是 32-byte U256 hex。
- `transactionHash` 存在。
- `logIndex` 存在。
- `blockNumber` 存在。

输出字段：

| Event 字段 | 来源 |
|---|---|
| `event_type` | `transfer` |
| `direction` | from/to 和 watched address 比较得到 `in` / `out` / `self` |
| `is_transfer` | `true` |
| `tx_hash` | `transactionHash` |
| `log_index` | parsed `logIndex` |
| `block_number` | parsed `blockNumber` |
| `block_hash` | `blockHash` |
| `from_address` | topic1 decoded address |
| `to_address` | topic2 decoded address |
| `amount_raw` | data parsed U256 decimal string |
| `amount_decimal` | amount_raw + asset.decimals |
| `metadata.source` | `evm_erc20_transfer_log` |
| `metadata.token_contract` | log.address |

Invalid log payloads return `AppError::Validation` and fail the scan. The first version does not silently skip malformed provider data because silent skips can hide provider/API incompatibility.

## 6. Cursor 和区块范围

### 6.1 Cursor 表

新增最小 cursor 表：

```sql
CREATE TABLE scan_cursors (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    chain_id UUID NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
    address_id UUID NOT NULL REFERENCES watched_addresses(id) ON DELETE CASCADE,
    cursor_type TEXT NOT NULL,
    last_scanned_block BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(address_id, cursor_type)
);
```

本阶段使用：

```text
cursor_type = "evm_erc20_transfer"
```

### 6.2 Range 计算

```text
latest = eth_blockNumber()
confirmations = chain.default_confirmations
confirmed_to = latest - confirmations
```

如果没有 cursor：

```text
from_block = max(0, confirmed_to - initial_window + 1)
```

`initial_window` 第一版固定为：

```text
1000 blocks
```

如果已有 cursor：

```text
from_block = cursor.last_scanned_block + 1
```

如果：

```text
confirmed_to < from_block
```

则本轮 ERC20 transfer scan no-op，正常完成 scan，不更新 cursor。

成功扫描完整 range 后，将 cursor 更新为 `confirmed_to`。

## 7. 去重和插入语义

现有 migration 已包含 transfer 去重索引：

```sql
CREATE UNIQUE INDEX IF NOT EXISTS idx_address_events_unique_transfer ON address_events(
    chain_id,
    tx_hash,
    COALESCE(log_index, -1),
    address_id,
    asset_id,
    event_type
) WHERE tx_hash IS NOT NULL;
```

本阶段复用该索引，不新增重复唯一索引。

新增 repository：

```rust
pub async fn insert_event_if_not_exists(
    pool: &PgPool,
    draft: AddressEventDraft,
) -> AppResult<Option<AddressEvent>>
```

语义：

- 插入成功返回 `Some(event)`。
- 唯一冲突返回 `Ok(None)`。
- 其他 DB 错误返回 `AppError::Database`。

只有 `Some(event)` 才创建 notify task。

## 8. Worker 集成

当前 `scan_evm_native_balance` 保留其职责。新增上层 orchestration：

```rust
pub async fn scan_evm_address(
    pool: &PgPool,
    task: &ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<Vec<AddressEvent>>
```

流程：

1. 执行 native balance scan，返回 0 或 1 个 balance-change event。
2. 执行 ERC20 transfer log scan，返回 0 到 N 个 transfer events。
3. 合并 events。
4. `process_locked_scan_task` 对每个 event enqueue notify task。
5. 所有 notify enqueue 成功后调用 `finish_address_scan`。

错误语义：

| 错误点 | 行为 |
|---|---|
| provider missing | 返回错误，不 finish scan |
| RPC request/status/body/json error | 返回错误，不 finish scan |
| malformed log | 返回 validation error，不 finish scan |
| DB insert snapshot/event/cursor error | 返回错误，不 finish scan |
| duplicate event | 返回 None，继续处理 |
| no new confirmed range | 正常 finish scan |
| no active ERC20 assets | 正常 finish scan |

## 9. Testing Strategy

### 9.1 `chain-providers`

- Transfer topic constant equals known hash.
- Address topic encoding lowercases and pads correctly.
- Address topic decode rejects malformed topics.
- `eth_getLogs` request body contains address, fromBlock, toBlock, topics.
- JSON-RPC logs parser rejects error payloads.
- Transfer log decode extracts from/to/amount/block/tx/log index.
- Decoded event draft classifies incoming/outgoing/self.
- Amount decimal respects token decimals.

### 9.2 `storage`

- `scan_cursors` migration has unique `(address_id, cursor_type)`.
- Cursor upsert query updates `last_scanned_block` and `updated_at`.
- Cursor lookup query filters by address and type.
- `insert_event_if_not_exists` query uses conflict handling on existing `idx_address_events_unique_transfer` semantics.
- Active ERC20 asset query filters `asset_type = 'erc20'`, active status, and non-null contract address.

### 9.3 `worker`

- Range calculation without cursor uses 1000-block window.
- Range calculation with cursor starts at `last_scanned_block + 1`.
- Confirmed range before from block is no-op.
- No active ERC20 assets produces no transfer events.
- Duplicate events do not enqueue notify.
- Inserted transfer events enqueue notify before `finish_address_scan`.

### 9.4 Final verification

- `cargo fmt --all --check --manifest-path backend/Cargo.toml`
- `cargo check --workspace --manifest-path backend/Cargo.toml`
- `cargo test --workspace --manifest-path backend/Cargo.toml`
- `npm run build --prefix frontend`
- `docker compose -f docker-compose.yml config`

## 10. Acceptance Criteria

- A watched EVM / BASE address with active ERC20 assets can scan confirmed Transfer logs via RPC polling.
- Incoming and outgoing token transfers create normalized `address_events` with `is_transfer = true`.
- Repeated scans do not create duplicate events for the same `(chain, address, asset, tx_hash, log_index)`.
- Cursor advances only after the log range is processed successfully.
- Notification tasks are enqueued only for newly inserted events.
- Existing native balance snapshot and balance-change behavior remains working.
- Existing event center can list the new transfer events without frontend changes.
