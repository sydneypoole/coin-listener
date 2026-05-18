# Coin Listener BTC/TRON Pagination Reliability Design

日期：2026-05-18

## 1. 目标

补齐 Milestone 4 后暴露的链上扫描可靠性缺口：BTC 与 TRON 扫描必须处理 provider 分页，避免因为 Esplora 单页交易历史或 TronGrid `meta.fingerprint` 截断而静默漏掉转账事件。

本阶段只聚焦“扫描完整性和 cursor 正确性”。通知 outbox、TRON 余额快照增强、provider failover 和自建索引器不进入本阶段，避免把分页可靠性与跨系统投递一致性耦合在同一个里程碑里。

## 2. 当前状态依据

- Milestone 4 已让 `chain_type = 'utxo'` 和 `chain_type = 'tron'` 进入真实 worker 扫描链路。
- `backend/crates/worker/src/lib.rs` 当前 BTC 扫描只调用一次 `BtcClient::address_transactions`，等价于 Esplora `/address/:address/txs/chain` 第一页。
- `backend/crates/worker/src/lib.rs` 当前 TRON 扫描只调用一次 `TronClient::account_transactions` 和每个 TRC20 asset 一次 `account_trc20_transfers`，等价于 TronGrid `limit=200` 第一页。
- `backend/crates/chain-providers/src/btc.rs` 当前只构造 `/address/{address}/txs/chain`，还没有 `/:last_seen_txid` 翻页 path。
- `backend/crates/chain-providers/src/tron.rs` 当前 `parse_data_array` 只取 `data`，没有读取 `meta.fingerprint`。
- `scan_cursors.last_scanned_block` 是 `BIGINT`，当前不支持复合 cursor state。
- `address_events` 已有转账幂等索引：`chain_id + tx_hash + COALESCE(log_index, -1) + address_id + asset_id + event_type`。

## 3. 范围

### 3.1 包含

1. BTC Esplora confirmed transaction history 分页：
   - 支持第一页 `/address/:address/txs/chain`。
   - 支持后续页 `/address/:address/txs/chain/:last_seen_txid`。
   - 以每页最后一笔交易的 `txid` 作为下一页游标。
2. TRON TronGrid 分页：
   - TRX transactions 读取 `meta.fingerprint`。
   - TRC20 transfers 读取 `meta.fingerprint`。
   - 后续请求带上 `fingerprint`。
3. Worker 分页扫描：
   - BTC、TRX、TRC20 都循环处理 provider pages。
   - 单次扫描设置最大页数上限，避免异常地址或 provider 行为导致 worker 长时间占用。
   - 任一页请求或 decode 失败时，不推进 cursor，不调用 `finish_address_scan`。
4. Cursor 策略：
   - BTC 继续用 `btc_transaction` 的 `last_scanned_block` 存 confirmed block height。
   - TRON 继续用 `tron_trx_transfer` / `tron_trc20_transfer` 的 `last_scanned_block` 存 timestamp watermark。
   - 只有完整分页扫描成功后才 upsert cursor。
5. 幂等与重复扫描：
   - 允许小范围重复读取已处理事件。
   - 依赖 `insert_event_if_not_exists` 防止重复写入同一 transfer event。
   - 只对本次新插入的 events enqueue notification。
6. Provider 错误脱敏补强：
   - HTTP request / body / status 错误不得泄露 provider base URL 或其中的 API key。

### 3.2 不包含

- Notification outbox / DB-backed reliable delivery。
- Redis enqueue 与 event insert 的跨系统原子性改造。
- TRON native balance snapshot 或 balance_change event 增强。
- BTC mempool 或未确认交易监听。
- Provider failover、健康探测、限流、熔断。
- 修改 `scan_cursors` 表结构为 JSONB 复合 cursor。
- 自建 BTC/TRON indexer。
- 前端页面改造。

## 4. 推荐方案

采用“provider 返回分页结果，worker 控制分页循环”的方案。

Provider 只负责：

- 构造当前页请求。
- 解析当前页响应。
- 返回当前页数据和下一页 token。

Worker 负责：

- 根据已有 scan cursor 计算本轮起点。
- 调用 provider page API。
- 对每页逐条 decode、filter、幂等写入。
- 统计本轮成功处理到的最大 cursor value。
- 在所有页处理成功后推进 cursor。

这样可以保持 `chain-providers` 无数据库依赖，也避免 provider 层隐式决定扫描策略。

## 5. 组件设计

### 5.1 BTC provider

新增分页数据结构，例如：

```rust
pub struct BtcTransactionPage {
    pub transactions: Vec<BtcTransaction>,
    pub next_last_seen_txid: Option<String>,
}
```

`BtcClient` 增加：

- `address_txs_page_path(address, last_seen_txid)`：
  - `None` 返回 `/address/{address}/txs/chain`。
  - `Some(txid)` 返回 `/address/{address}/txs/chain/{txid}`。
- `address_transactions_page(address, last_seen_txid)`：
  - 请求单页。
  - 解析为 `Vec<BtcTransaction>`。
  - 如果本页为空，`next_last_seen_txid = None`。
  - 如果本页非空，`next_last_seen_txid = Some(last txid)`。

BTC provider 不判断 block cursor，不访问数据库，不决定是否继续翻页。是否继续由 worker 根据返回页、最大页数和 cursor 策略决定。

### 5.2 TRON provider

新增分页数据结构，例如：

```rust
pub struct TronPage {
    pub data: Vec<serde_json::Value>,
    pub next_fingerprint: Option<String>,
}
```

响应解析规则：

- `data` 必须存在且为数组。
- `meta.fingerprint` 缺失或为空时表示没有下一页。
- `meta.fingerprint` 存在且非空时作为下一页请求参数。

`TronClient` 增加：

- `account_transactions_page(address, min_timestamp, fingerprint)`。
- `account_trc20_transfers_page(address, contract_address, min_timestamp, fingerprint)`。

TRON provider 保持当前 `only_confirmed=true`、`limit=200`、`min_timestamp` 行为，并在有 fingerprint 时追加 `fingerprint` query。

### 5.3 Worker 分页控制

新增共享 helper，避免 BTC/TRON 各自写无限循环：

- `MAX_PROVIDER_PAGES_PER_SCAN`：建议初始值 `10`。
- page loop 每处理一页后递增计数。
- 达到页数上限且仍存在下一页时返回错误，不推进 cursor，避免 provider 按新到旧排序时跳过未处理旧页。

停止条件：

1. provider 返回空页。
2. provider 返回无 next token。
3. 达到 `MAX_PROVIDER_PAGES_PER_SCAN` 且没有下一页。
4. 当前页所有 transaction/transfer 都早于 cursor 且 provider 顺序可判定已进入旧数据区间。

第 4 条只能在 provider 返回顺序明确且实现中可稳定验证时使用；否则不依赖它作为唯一停止条件。

### 5.4 BTC worker 策略

BTC 扫描流程：

1. 插入 BTC confirmed balance snapshot，沿用 Milestone 4 行为。
2. 读取 `btc_transaction` cursor。
3. 从第一页开始请求交易历史。
4. 每页对 transaction 调用 `classify_btc_transaction`。
5. 跳过 `transfer.block_number < from_block` 的旧事件。
6. 对保留事件调用 `btc_transfer_event_draft` 和 `insert_event_if_not_exists`。
7. 记录所有已成功处理 transfer 的最大 `block_number`。
8. 所有页面处理成功后，upsert `btc_transaction` cursor。

BTC 本阶段不新增复合 cursor。为降低同 block 边界风险，worker 可以在读取 cursor 后使用小范围 block overlap，例如从 `last_scanned_block - BTC_CURSOR_OVERLAP_BLOCKS + 1` 开始重新接受数据，再依赖事件幂等索引去重。初始 overlap 建议为 `1` 个 block。

### 5.5 TRON worker 策略

TRX 扫描流程：

1. 读取 `tron_trx_transfer` timestamp cursor。
2. 请求 TRX transaction page。
3. 对每页 payload 调用 `try_decode_trx_transfer_at_index`。
4. `Skip` 非 TransferContract payload，不视为错误。
5. 插入匹配 native asset 的 transfer event。
6. 记录最大 `cursor_value`。
7. 所有页面处理成功后推进 `tron_trx_transfer` cursor。

TRC20 扫描流程：

1. 读取 `tron_trc20_transfer` timestamp cursor。
2. 查询 active `trc20` assets。
3. 每个 asset 按 contract 独立分页请求。
4. 每页 payload 调用 `decode_trc20_transfer_at_index`。
5. 插入匹配 asset contract 的 transfer event。
6. 记录所有 assets 中最大 `cursor_value`。
7. 所有 assets 的所有页面处理成功后推进 `tron_trc20_transfer` cursor。

TRC20 cursor 是 address 级别，不是 asset 级别。因此只要任一 asset 分页失败，本轮不得推进 `tron_trc20_transfer` cursor，避免某个 contract 的失败被其他 contract 的高 timestamp 掩盖。

## 6. 错误处理与幂等

- Provider request 失败：返回 `AppError::Config`，消息脱敏，不推进 cursor。
- Provider non-2xx：返回 `AppError::Config`，body 中若包含 provider URL，也必须脱敏。
- Response JSON 结构错误：返回 `AppError::Validation`，不推进 cursor。
- 单条 BTC transaction decode 错误：当前阶段保持严格失败，整轮不推进 cursor。
- 单条 TRON TRX 非 TransferContract：返回 `Skip`，继续处理。
- 单条 TRON TRC20 malformed：严格失败，整轮不推进 cursor。
- `insert_event_if_not_exists` 返回 `None`：说明事件已存在，不加入通知任务列表。
- 只有新插入的事件参与当前 Redis notify enqueue。

## 7. 验收标准

1. BTC provider 能构造第一页和 `last_seen_txid` 后续页 path。
2. BTC worker 能处理超过一页的 confirmed transactions。
3. TRON provider 能解析 `meta.fingerprint`，并把 fingerprint 带入下一页请求。
4. TRX worker 能处理超过 200 条的多页 account transactions。
5. TRC20 worker 能处理超过 200 条的多页 account transfers。
6. 任一中间页失败时，对应 cursor 不推进，scan 不 finish。
7. 达到最大页数上限且仍有下一页时，scan 返回错误且 cursor 不推进。
8. 重复扫描不会重复插入 transfer event，也不会重复 enqueue 已存在事件。
9. Provider request、status、body 错误不泄露 base URL 或 API key。
10. 现有 EVM、BTC、TRON、notification、frontend build 和 docker compose config 不回归。

## 8. 测试策略

### 8.1 Provider tests

- BTC：
  - `address_txs_page_path(None)` 返回第一页 path。
  - `address_txs_page_path(Some(txid))` 返回后续页 path。
  - 单页 response 解析为空页时没有 next txid。
  - 单页 response 非空时 next txid 等于最后一笔 txid。
  - request/status/body 错误脱敏。
- TRON：
  - `parse_page` 提取 `data`。
  - `parse_page` 提取 `meta.fingerprint`。
  - 缺失 `data` 返回 validation error。
  - fingerprint query 只在存在 next token 时追加。
  - request/status/body 错误脱敏。

### 8.2 Worker helper tests

- page loop 达到 `MAX_PROVIDER_PAGES_PER_SCAN` 时停止。
- cursor 只基于成功处理过的 transfer 推进。
- BTC overlap start 不低于 0。
- TRC20 任一 asset 分页失败时不推进 address-level cursor。
- TRX `Skip` payload 不阻断同页其他 transfer。

### 8.3 Regression tests

- BTC 多页 fixtures：第一页与第二页都有 watched address transfer，最终都生成事件草稿。
- TRON TRX 多页 fixtures：第一页有 fingerprint，第二页无 fingerprint，最终两页都处理。
- TRON TRC20 多 asset fixtures：一个 asset 成功、另一个 asset 中间页失败时，cursor 不推进。
- 已存在事件不生成 notify task。

### 8.4 Final verification

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
cargo check --workspace --manifest-path backend/Cargo.toml
cargo test --workspace --manifest-path backend/Cargo.toml
npm run build --prefix frontend
docker compose -f docker-compose.yml config
```

## 9. 后续里程碑

完成本阶段后，下一优先级建议为 notification outbox / reliable delivery：用数据库 outbox 消除“事件已插入但 Redis notify enqueue 失败后无法补发”的一致性缺口。

TRON native balance snapshot 和 balance_change event 可以作为 outbox 之后的小型功能完整性里程碑实现。
