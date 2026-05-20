# Coin Listener Watched Address Asset Selection Design

日期：2026-05-21

## 1. 目标

支持同一个地址字符串按多条链分别监听，并为每条链显式选择至少一个资产。例如同一个 EVM 地址可以在 Ethereum 上监听 ETH、USDT，同时在 Base 上监听 USDC。

## 2. 当前依据

- `backend/crates/storage/migrations/0002_config_management.sql` 中 `watched_addresses` 已按 `UNIQUE (tenant_id, chain_id, address)` 建模，同一个地址字符串天然可以在不同链上分别存在。
- `backend/crates/core/src/models.rs` 的 `CreateWatchedAddressRequest` 当前只有单个 `chain_id`，没有资产选择字段。
- `backend/crates/storage/migrations/0003_events.sql` 中 `balance_snapshots` 与 `address_events` 已引用 `address_id` 和 `asset_id`，事件层已经具备按资产归属能力。
- `backend/crates/worker/src/lib.rs` 当前会为 EVM 地址扫描 native asset 和该链全部 active ERC20，为 TRON 地址扫描 TRX 和全部 active TRC20，为 BTC 地址扫描 BTC native。
- `frontend/src/pages/AddressesPage.tsx` 当前新增地址表单只选择单条链，没有资产多选。

## 3. 用户确认的规则

每个监听配置必须显式选择至少一个资产。空资产列表无效，不表示“监听全部资产”，也不表示“只监听原生币”。

## 4. 推荐方案

保留 `watched_addresses` 作为“租户 + 链 + 地址”的扫描调度实体，新增规范化关联表 `watched_address_assets` 表达该地址在该链上要监听哪些资产。

该方案避免重复创建同一链同一地址的多条调度记录，也避免 JSON 数组无法使用外键约束的问题。扫描器通过关联表获取该地址实际选择的资产，只扫描所选资产。

## 5. 数据模型

新增迁移 `backend/crates/storage/migrations/0013_watched_address_assets.sql`：

```sql
CREATE TABLE IF NOT EXISTS watched_address_assets (
    address_id UUID NOT NULL REFERENCES watched_addresses(id) ON DELETE CASCADE,
    asset_id UUID NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (address_id, asset_id)
);

CREATE INDEX IF NOT EXISTS idx_watched_address_assets_asset
    ON watched_address_assets(asset_id);
```

现有 `watched_addresses` 不增加 `asset_ids` 列，继续保持 `UNIQUE (tenant_id, chain_id, address)`。

创建和更新时必须校验：

1. `asset_ids` 非空。
2. 每个 asset 存在。
3. 每个 asset 的 `chain_id` 等于 watched address 的 `chain_id`。
4. 重复 asset id 会被去重或返回 validation error；推荐后端去重后写入，前端不生成重复项。
5. 删除 watched address 时通过 `ON DELETE CASCADE` 删除资产关联。

## 6. API Contract

### 6.1 请求

`CreateWatchedAddressRequest` 增加字段：

```json
{
  "chain_id": "...",
  "address": "0x...",
  "label": "主钱包",
  "priority": "normal",
  "scan_interval_seconds": 300,
  "transfer_filter_enabled": true,
  "balance_change_filter_enabled": true,
  "status": "active",
  "asset_ids": ["eth-asset-id", "usdt-asset-id"]
}
```

`POST /api/addresses` 和 `PUT /api/addresses/:id` 都使用同一请求结构。

### 6.2 响应

新增 API 聚合类型，而不是把数据库 row model 和聚合字段混在一起：

```rust
pub struct WatchedAddressResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub chain_id: Uuid,
    pub address: String,
    pub label: Option<String>,
    pub priority: String,
    pub scan_interval_seconds: i32,
    pub transfer_filter_enabled: bool,
    pub balance_change_filter_enabled: bool,
    pub status: String,
    pub asset_ids: Vec<Uuid>,
}
```

`GET /api/addresses` 返回 `Vec<WatchedAddressResponse>`。创建和更新也返回 `WatchedAddressResponse`。

## 7. Repository 设计

新增 repository helpers：

- `selected_assets_for_address(pool, address_id) -> Vec<Asset>`
- `asset_ids_for_address(pool, address_id) -> Vec<Uuid>`
- `validate_assets_for_chain(pool, chain_id, asset_ids) -> AppResult<Vec<Uuid>>`
- `replace_watched_address_assets(transaction, address_id, asset_ids)`

创建地址流程：

1. 读取 chain 并校验 address shape。
2. 校验 watched address 字段。
3. 校验 `asset_ids` 非空且全部属于 chain。
4. 在 transaction 中插入 `watched_addresses`。
5. 插入 `watched_address_assets`。
6. 提交后返回 `WatchedAddressResponse`。

更新地址流程：

1. 读取 chain 并校验 address shape。
2. 校验 watched address 字段。
3. 校验 `asset_ids` 非空且全部属于 chain。
4. 在 transaction 中更新 `watched_addresses`。
5. 删除旧 `watched_address_assets`。
6. 插入新 `watched_address_assets`。
7. 提交后返回 `WatchedAddressResponse`。

列表流程应避免 N+1；推荐一次查询地址列表，再一次查询这些地址的 asset associations，按 `address_id` 聚合。

## 8. Worker 扫描行为

Worker 必须只扫描监听地址显式选择的资产。

### 8.1 EVM

- 获取 `selected_assets_for_address(context.id)`。
- 如果 selected assets 包含 native asset，才执行 `eth_getBalance` 并生成 native balance change。
- ERC20 transfer 扫描只遍历 selected assets 中 `asset_type = 'erc20'` 的资产。
- 如果只选 USDT，不扫描 ETH native balance，也不扫描 USDC logs。
- 如果选 ETH + USDT，则扫描 ETH balance 和 USDT transfer logs。

### 8.2 TRON

- 如果 selected assets 包含 TRX native asset，才扫描 TRX transfers。
- TRC20 transfer 扫描只遍历 selected assets 中 `asset_type = 'trc20'` 的资产。

### 8.3 BTC

- BTC 只有 native asset。选择 BTC 后按现有 BTC balance 和 transaction scan 逻辑执行。

### 8.4 Cursor

现有 cursor 仍按 `address_id + cursor_type` 维护。资产选择变更会影响后续扫描范围：

- 新增资产后，从现有 cursor 后继续扫，不回溯历史区块。
- 移除资产后，不再扫描该资产；历史事件保留。

这是第一版的明确行为，避免新增“按资产回溯 cursor”的复杂度。

## 9. 前端设计

### 9.1 地址新增

`frontend/src/pages/AddressesPage.tsx` 支持一次为同一个地址添加多条链配置。表单结构：

- 地址：输入一次。
- 标签、优先级、扫描间隔、转账过滤、余额变化过滤、状态：作为所有链配置的默认值。
- 链配置列表：每行包含：
  - 链：单选。
  - 资产：多选，选项只显示该链下的 active assets。

提交时前端把每一行拆成一次 `createWatchedAddress` 调用。每行必须选择至少一个资产。

示例：

| 地址 | 链 | 资产 |
|---|---|---|
| `0xabc...` | Ethereum | ETH, USDT |
| `0xabc...` | Base | USDC |

前端提交两个请求：

1. Ethereum + `0xabc...` + `[ETH, USDT]`
2. Base + `0xabc...` + `[USDC]`

### 9.2 地址编辑

编辑单条 watched address 时，只编辑该链上的资产多选和原有字段，不在同一个编辑弹窗中切换成多链批量编辑。这样保持编辑语义清晰。

### 9.3 地址列表

地址表格新增“监听资产”列，按 `asset_ids` 映射到 `symbol` 展示：

| 链 | 标签 | 地址 | 监听资产 | 优先级 | 状态 |
|---|---|---|---|---|---|
| Ethereum | 主钱包 | `0xabc...` | ETH, USDT | normal | active |
| Base | 主钱包 | `0xabc...` | USDC | normal | active |

## 10. 错误处理

- `asset_ids` 为空：HTTP 400，错误信息 `asset_ids must not be empty`。
- asset 不存在：HTTP 400，错误信息包含 `asset does not exist`。
- asset 不属于 selected chain：HTTP 400，错误信息包含 `asset must belong to watched address chain`。
- 同一 tenant + chain + address 已存在：保持唯一约束；后续可优化为友好的 409。第一版只要求不破坏现有行为。
- 多链批量创建部分失败：前端显示每条链的成功/失败结果并刷新地址列表；已成功的记录保留，不做自动回滚。

## 11. 测试策略

必须遵循 TDD。

### 11.1 Backend storage tests

覆盖：

1. 创建 watched address 时写入 asset associations。
2. `asset_ids` 为空返回 validation error。
3. asset 不属于 chain 返回 validation error。
4. 更新 watched address 会替换 asset associations。
5. list watched addresses 返回 asset_ids。

### 11.2 Worker tests

覆盖：

1. EVM native balance 只在 native asset 被选择时扫描。
2. EVM ERC20 scan 只遍历 selected ERC20 assets。
3. TRON TRC20 scan 只遍历 selected TRC20 assets。
4. BTC native asset 选择后仍按现有逻辑执行。

### 11.3 API tests

覆盖：

1. `POST /api/addresses` 接受 non-empty `asset_ids`。
2. `PUT /api/addresses/:id` 替换 `asset_ids`。
3. missing or empty `asset_ids` 返回 400。

### 11.4 Frontend tests

覆盖：

1. 地址新增表单存在资产多选。
2. 资产选项按链过滤。
3. 多链配置提交会拆成多个 create 请求。
4. 地址表格展示监听资产。

## 12. 不包含

- 不做按资产独立 scan cursor 回溯。
- 不新增资产管理页面。
- 不做批量创建事务型后端 endpoint。
- 不修改通知规则的数据模型；通知规则已有 `asset_id`，后续自然可按具体资产过滤事件。
- 不自动把空资产选择解释为全部资产或 native-only。
