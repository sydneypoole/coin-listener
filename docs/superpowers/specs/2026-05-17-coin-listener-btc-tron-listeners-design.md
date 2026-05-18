# Coin Listener BTC / TRON Listener Design

日期：2026-05-17

## 1. 目标

补齐 Milestone 4：让已配置的 BTC 与 TRON 监听地址进入真实扫描链路，并将 BTC、TRX、TRC20 转账与余额变化归一化写入现有 `address_events`、`balance_snapshots`、`scan_cursors` 与通知队列。

本里程碑延续已完成的 EVM / BASE 扫描结构：链 provider 只负责请求构造、响应解析与链特定 decode；worker 负责 cursor、资产选择、事件草稿构造、幂等写入、通知投递和扫描完成时间更新。

## 2. 当前状态依据

- `docs/superpowers/specs/2026-05-17-coin-listener-design.md:900` 定义 Milestone 4 为 BTC / TRON 监听。
- `backend/crates/storage/migrations/0002_config_management.sql` 已内置：
  - `btc`：`chain_type = 'utxo'`，native asset `BTC`，8 decimals。
  - `tron`：`chain_type = 'tron'`，native asset `TRX`，6 decimals。
  - TRON USDT：`asset_type = 'trc20'`，contract `TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t`。
- `backend/crates/chain-providers/src/lib.rs` 目前只暴露 `evm`。
- `backend/crates/worker/src/lib.rs` 目前只支持 `chain_type = 'evm'`，非 EVM 进入 `Unsupported`。
- 已存在可复用的统一持久化能力：`balance_snapshots`、`address_events`、`scan_cursors`、`insert_event_if_not_exists`、通知队列。

## 3. 范围

### 3.1 本里程碑包含

1. 新增 TRON provider：
   - TRON 地址格式校验与标准化。
   - TRX transfer 响应解析。
   - TRC20 transfer 响应解析。
   - TRON block / timestamp cursor 计算所需字段解析。
2. 新增 BTC provider：
   - BTC 地址格式基本校验。
   - BTC 地址余额响应解析。
   - BTC 地址交易历史响应解析。
   - 从交易 input/output 中计算 watched address 净变化。
3. Worker 支持：
   - `chain_type = 'tron'` 分派到 TRON scan helper。
   - `chain_type = 'utxo'` 分派到 BTC scan helper。
   - TRON 与 BTC 都复用 `scan_cursors`，cursor 单调前进。
   - 新事件复用 `insert_event_if_not_exists` 幂等写入。
   - 所有新事件生成 `NotifyEventTask` 后再完成地址扫描。
4. 事件归一化：
   - 转账事件统一 `event_type = 'transfer'`、`is_transfer = true`。
   - 余额变化事件统一 `event_type = 'balance_change'`、`is_transfer = false`。
   - `direction` 使用现有语义：`in`、`out`、`self`、`unknown`。

### 3.2 本里程碑不包含

- WebSocket / Telegram 真实发送增强。
- Provider failover、健康探测或 QPS 限流重构。
- 自建 BTC / TRON 索引器。
- BTC mempool 准实时监听。
- TRON webhook 或事件订阅。
- 任意 token 自动发现。
- 前端新页面；现有事件中心、通知、系统状态继续消费统一数据模型。

## 4. 推荐实施顺序

采用“统一设计，分阶段实现”的方案：

1. **阶段 A：TRON 优先**
   - TRON 的 account transaction / TRC20 transfer 模型与 EVM Transfer 最接近。
   - 已有 TRON USDT seed asset，可最快验证稳定币转账路径。
2. **阶段 B：BTC 接入**
   - BTC UTXO 净变化和 tx history 去重更复杂，放在 TRON 后实现。
   - BTC 只输出 watched address 视角的净变化事件，不尝试构建完整交易图谱。

两个阶段共享同一套 cursor、event draft、notification 和 verification 约定。

## 5. TRON 设计

### 5.1 Provider 边界

新增 `backend/crates/chain-providers/src/tron.rs`，职责限定为：

- 构造 TRON HTTP API 请求。
- 解析 TRX 与 TRC20 响应 payload。
- 将链特定字段 decode 为中间结构，例如 `DecodedTronTransfer`。
- 不访问数据库，不创建 `AddressEventDraft`。

Provider 错误必须满足：运行时错误不泄露 `base_url` 中的 API key 或敏感路径。

### 5.2 TRON cursor

使用两个 cursor type：

- `tron_trx_transfer`
- `tron_trc20_transfer`

cursor 的 `last_scanned_block` 存储已确认处理到的 block number。若 provider 响应只提供 timestamp 而不稳定提供 block number，provider decode 层必须显式返回 `block_number = None`，worker 不得推进 block cursor；该情况在当前实现计划中视为 provider payload 不满足扫描要求并返回 validation error。

### 5.3 TRX transfer

Worker 对 TRON native asset 执行：

1. 加载 watched address context、TRON active RPC provider、TRX native asset。
2. 读取 `tron_trx_transfer` cursor。
3. 请求 watched address 的 TRX transactions。
4. 解析交易中 watched address 的净变化：
   - watched address 收到 TRX：`direction = 'in'`。
   - watched address 发出 TRX：`direction = 'out'`。
   - from 与 to 都是 watched address：`direction = 'self'`。
5. 写入 `address_events`，并保存 TRX balance snapshot。
6. cursor 前进到本次成功处理的最高 confirmed block。

### 5.4 TRC20 transfer

Worker 对 `asset_type = 'trc20'` 且 active 的资产执行：

1. 按 watched address 请求 TRC20 transfer 列表。
2. 只接受 contract address 与 asset 配置匹配的 transfer。
3. 使用 asset decimals 计算 `amount_decimal`。
4. 生成统一 transfer event。
5. 幂等写入后推进 `tron_trc20_transfer` cursor。

TRON USDT 合约地址按 seed 配置使用 base58 地址；provider decode 层输出时保留 TRON 地址原始格式，比较时使用规范化后的同一格式。

## 6. BTC 设计

### 6.1 Provider 边界

新增 `backend/crates/chain-providers/src/btc.rs`，职责限定为：

- 构造 BTC provider HTTP 请求。
- 解析地址余额、交易历史、input/output。
- 计算 watched address 在单笔交易中的净 satoshi 变化。
- 输出 `DecodedBtcTransfer` 或 `DecodedBtcBalance` 中间结构。
- 不访问数据库。

Provider 错误同样不得泄露 `base_url` 中的 API key 或敏感路径。

### 6.2 BTC cursor

使用 cursor type：

- `btc_transaction`

cursor 的 `last_scanned_block` 存储已确认处理到的 block height。未确认交易不进入本里程碑的事件写入范围。

### 6.3 BTC transfer 归一化

BTC 没有 account model。Worker 以 watched address 视角计算每笔 confirmed transaction：

- `received = sum(outputs to watched address)`
- `spent = sum(inputs from watched address)`
- `delta = received - spent`

事件规则：

- `delta > 0`：`direction = 'in'`，`amount_raw = delta satoshi`。
- `delta < 0`：`direction = 'out'`，`amount_raw = abs(delta) satoshi`。
- `delta = 0` 且 watched address 同时出现在 input/output：`direction = 'self'`，`amount_raw = 0`。
- watched address 未出现在 input/output：忽略。

BTC event 字段约定：

- `tx_hash` 使用 BTC txid。
- `log_index = None`。
- `block_number = block height`。
- `block_hash` 若 provider 返回则保存，否则为 `None`。
- `from_address` / `to_address` 对多输入多输出不强行挑选单一地址；可为 `None`，详细 input/output 摘要进入 `metadata`。
- `metadata.source = 'btc_transaction'`。

### 6.4 BTC balance snapshot

每次 BTC scan 同步写入 native BTC balance snapshot。若 provider 返回 confirmed balance，则使用 confirmed balance；不将 unconfirmed balance 计入本里程碑事件和余额变化判断。

## 7. Storage 设计

现有表结构足够承载本里程碑，不新增表。

需要新增或泛化的 repository helper：

- `active_assets_for_chain_by_type(chain_id, asset_type)`：供 TRC20 资产查询复用。
- 现有 `insert_event_if_not_exists` 依赖 `address_events` 的 transfer 唯一索引：`chain_id + tx_hash + COALESCE(log_index, -1) + address_id + asset_id + event_type`，可覆盖 BTC `log_index = None` 的一 tx 一 address 一 asset 一 transfer event 去重，不新增迁移。
- BTC 同一 watched address 同一 tx 保持一条净变化 transfer event；不拆分为多 input / output 明细事件。

`scan_cursors` 继续使用 `UNIQUE(address_id, cursor_type)`。cursor upsert 必须保持单调前进。

## 8. Worker 设计

### 8.1 ScanPlan

`scan_plan_for_chain` 扩展为：

- `evm` → 现有 EVM scan。
- `tron` → TRON scan。
- `utxo` → BTC scan。
- 其他 → Unsupported。

### 8.2 scan helpers

新增：

- `scan_tron_address(pool, task, now) -> AppResult<Vec<AddressEvent>>`
- `scan_btc_address(pool, task, now) -> AppResult<Vec<AddressEvent>>`

两个 helper 都必须遵守：

1. 加载 context、chain、provider、native asset。
2. provider timeout 必须为正数。
3. 调用链 provider 获取 confirmed 数据。
4. 写入 balance snapshot。
5. 写入 transfer events。
6. 推进 cursor。
7. 返回本次新插入的 events，由 `process_locked_scan_task` 统一 enqueue notification。

## 9. 错误处理和幂等

- Provider HTTP request / body / status 错误返回 `AppError::Config`，错误消息不包含完整 provider URL。
- Payload 缺字段、非法金额、非法 block height、非法地址返回 `AppError::Validation`。
- 单个 scan task 内发生 provider 或 decode 错误时，不推进 cursor，不调用 `finish_address_scan`，让后续重试保留机会。
- event insert 使用幂等写入；重复扫描不会重复生成通知任务。
- cursor 仅在该 cursor 对应范围内的 events 全部处理成功后推进。

## 10. 测试策略

### 10.1 Provider tests

- TRON：地址规范化、TRX payload decode、TRC20 payload decode、malformed payload error、URL redaction。
- BTC：地址校验、balance decode、tx input/output delta 计算、多输入多输出、malformed payload error、URL redaction。

### 10.2 Worker tests

- `scan_plan_for_chain` 覆盖 `tron` 与 `utxo`。
- TRON cursor range / no-op / negative confirmation tests。
- BTC cursor range / no-op / negative confirmation tests。
- BTC delta classification tests：in、out、self、ignore。
- notification regression：新插入事件逐条 enqueue。

### 10.3 Final verification

每个阶段完成时运行：

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
cargo check --workspace --manifest-path backend/Cargo.toml
cargo test --workspace --manifest-path backend/Cargo.toml
npm run build --prefix frontend
docker compose -f docker-compose.yml config
```

## 11. 验收标准

1. `chain_type = 'tron'` 的 watched address 不再走 Unsupported。
2. TRX 与已配置 TRC20 资产 transfer 能写入 `address_events`。
3. `chain_type = 'utxo'` 的 BTC watched address 不再走 Unsupported。
4. BTC confirmed tx 能按 watched address 净变化写入 transfer event。
5. BTC 与 TRON 扫描都会写入 native balance snapshot。
6. 重复扫描不会重复插入同一 transfer event，也不会重复投递通知。
7. Provider 错误不会泄露 API key 或完整 base URL。
8. 当前 EVM / BASE 扫描、通知、事件 API、前端 build 不回归。
