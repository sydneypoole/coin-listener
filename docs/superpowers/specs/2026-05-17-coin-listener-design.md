# Coin Listener 多链地址监听平台设计

日期：2026-05-17

## 1. 背景和目标

本项目是一个使用 Rust 后端和 React + Semi UI 前端构建的多链区块链地址监听平台。系统用于监听指定地址的转账和余额变化，并在满足规则时发送通知。

第一版采用 MVP 方式落地，但架构预留后期 SaaS、多租户、多链扩展和横向扩容能力。

核心目标：

- 支持 BTC、ETH、TRON、BASE。
- 后期支持新增网络。
- 支持地址金额变化检测。
- 支持转账事件检测。
- 支持筛选事件是否为转账。
- 支持原生币和常见稳定币。
- 后期支持任意代币合约。
- 支持站内通知、WebSocket 实时推送和 Telegram 通知。
- 支持多线程并发扫描任务。
- 支持 Docker Compose 部署，架构兼容后期 K8s。
- 支持双运行模式：多进程平台模式和 `all-in-one` 单体应用模式。

## 2. 范围

### 2.1 MVP 包含

- 用户登录和默认租户。
- 地址添加、暂停、恢复、删除。
- BTC、ETH、TRON、BASE 链配置。
- Provider 配置和健康状态。
- 原生币和常见稳定币配置。
- 地址余额快照。
- 标准化事件表。
- `is_transfer` 事件分类。
- React + Semi UI 管理后台。
- 事件中心筛选。
- 通知规则。
- 站内通知。
- WebSocket 实时推送。
- Telegram 通知。
- Scheduler / Worker / Notifier 多进程。
- Worker 进程内多线程并发任务处理。
- Redis 队列、锁、限流。
- PostgreSQL 持久化。
- Docker Compose 部署。
- `all-in-one` 单体应用打包模式：单个 Rust 进程托管前端静态资源，并在进程内运行 API、scheduler、worker、notifier 组件。

### 2.2 MVP 不包含

- 复杂 RBAC。
- 计费套餐。
- 团队成员邀请。
- 高级审计日志。
- 自建完整链索引器。
- Kafka、ClickHouse、TimescaleDB。
- 任意 token 自动发现。
- 复杂风控系统。

## 3. 总体架构

项目采用 monorepo：

```text
coin-listener/
  backend/
    Cargo.toml
    crates/
      api-server/
      scheduler/
      worker/
      notifier/
      all-in-one/
      core/
      chain-providers/
      storage/
  frontend/
  docker-compose.yml
  docs/
```

后端采用 Rust workspace。核心进程包括：

- `api-server`：提供 HTTP API、WebSocket、认证、地址管理、事件查询、通知配置。
- `scheduler`：按地址优先级和扫描频率生成扫描任务，写入 Redis 队列。
- `worker`：消费扫描任务，调用链 provider，生成事件和余额快照。
- `notifier`：消费通知任务，发送站内、WebSocket、Telegram 通知。
- `all-in-one`：单体应用入口，在一个 Rust 进程中启动 API、scheduler、worker、notifier，并托管 React 构建后的静态资源。
- `core`：领域模型、trait、错误类型和事件分类。
- `chain-providers`：BTC、EVM、TRON、BASE 的链适配和解析。
- `storage`：PostgreSQL repository、migration、Redis 队列和锁封装。

基础设施：

- PostgreSQL：主数据存储。
- Redis：任务队列、分布式锁、限流状态。
- React + Semi UI：前端管理后台。
- Docker Compose：第一版部署。

## 4. 后端服务职责

### 4.1 api-server

职责：

- 用户登录。
- 默认租户管理。
- 地址管理。
- 资产管理。
- 网络和 Provider 配置。
- 事件查询和筛选。
- 通知规则配置。
- 通知渠道配置。
- WebSocket 实时推送入口。

原则：API 服务不直接执行大量链上扫描任务，避免慢任务阻塞用户请求。

### 4.2 scheduler

职责：

- 查询 `next_scan_at <= now` 的监听地址。
- 按链、优先级、扫描频率分批。
- 生成 `ScanAddressTask`。
- 写入 Redis 队列。
- 更新调度状态。
- 初步考虑 Provider 限流。

优先级策略：

- 普通地址：分钟级扫描。
- 高优先级地址：更短扫描间隔。
- 关键地址：准实时或高频扫描。

### 4.3 worker

职责：

- 从 Redis 队列领取扫描任务。
- 使用 Redis 锁避免重复处理同一地址。
- 调用对应链的 `ChainProvider`。
- 获取余额、交易、token transfer。
- 与上次余额快照对比。
- 生成标准化 `AddressEvent`。
- 判断 `is_transfer`。
- 写入事件和余额快照。
- 投递通知任务。
- 更新地址扫描时间。

并发模型：

- 每个 worker 进程使用 Tokio async runtime。
- 进程内按链维护并发池。
- 支持每条链配置最大并发数。
- 支持 Provider 级 QPS 限流。
- 支持多个 worker 进程横向扩容。

示例并发配置：

```text
BTC pool     max_concurrency = 20
ETH pool     max_concurrency = 100
BASE pool    max_concurrency = 100
TRON pool    max_concurrency = 50
NOTIFY pool  max_concurrency = 50
```

### 4.4 notifier

职责：

- 消费通知任务。
- 匹配通知渠道。
- 写入站内通知。
- 推送 WebSocket 消息。
- 发送 Telegram 消息。
- 记录通知发送结果。
- 处理失败重试和渠道限流。

### 4.5 all-in-one

职责：

- 作为单体应用入口，打包为一个 Rust 可执行文件或单个 Docker 镜像。
- 复用 `api-server` 的 HTTP 路由和 WebSocket 能力。
- 在同一个 Tokio runtime 中启动 scheduler、worker、notifier 后台任务。
- 托管 React + Semi UI 的前端构建产物。
- 支持小规模部署、本地测试、私有化交付。

约束：

- `all-in-one` 模式仍默认使用 PostgreSQL 和 Redis，避免第一版同时维护两套存储语义。
- 后期可以增加 SQLite / embedded queue 的轻量模式，但不纳入当前 MVP。
- 单体模式优先保证部署简单；平台级高并发仍推荐多进程模式。

## 5. 数据模型

### 5.1 用户与租户

#### users

- `id`
- `email`
- `password_hash`
- `display_name`
- `status`
- `created_at`
- `updated_at`

#### tenants

- `id`
- `name`
- `status`
- `created_at`
- `updated_at`

#### tenant_members

- `tenant_id`
- `user_id`
- `role`
- `created_at`

第一版可以只有一个默认租户和管理员用户。

### 5.2 链和 Provider

#### chains

- `id`
- `key`
- `name`
- `chain_type`：`utxo`、`evm`、`tron`
- `native_asset_symbol`
- `status`
- `default_confirmations`
- `created_at`

内置链：

- `btc`
- `ethereum`
- `tron`
- `base`

#### providers

- `id`
- `chain_id`
- `provider_type`：`rpc`、`websocket`、`rest_api`
- `name`
- `base_url`
- `api_key_ref`
- `priority`
- `qps_limit`
- `timeout_ms`
- `status`

`api_key_ref` 不保存明文密钥，第一版可引用环境变量名，后期接密钥管理系统。

### 5.3 资产

#### assets

- `id`
- `chain_id`
- `asset_type`：`native`、`erc20`、`trc20`
- `symbol`
- `name`
- `contract_address`
- `decimals`
- `is_builtin`
- `status`

第一版内置：

- BTC / BTC
- Ethereum / ETH
- Ethereum / USDT
- Ethereum / USDC
- TRON / TRX
- TRON / USDT
- BASE / ETH
- BASE / USDC
- BASE / USDT 如需要

### 5.4 监听地址

#### watched_addresses

- `id`
- `tenant_id`
- `chain_id`
- `address`
- `label`
- `priority`：`normal`、`high`、`critical`
- `scan_interval_seconds`
- `transfer_filter_enabled`
- `balance_change_filter_enabled`
- `status`
- `last_scanned_at`
- `next_scan_at`
- `created_by`
- `created_at`
- `updated_at`

### 5.5 余额快照

#### balance_snapshots

- `id`
- `tenant_id`
- `chain_id`
- `address_id`
- `asset_id`
- `balance_raw`
- `balance_decimal`
- `block_number`
- `block_hash`
- `observed_at`
- `source_provider_id`

余额计算以 `balance_raw + decimals` 为准，避免浮点精度问题。

### 5.6 标准化事件

#### address_events

- `id`
- `tenant_id`
- `chain_id`
- `address_id`
- `asset_id`
- `event_type`
- `direction`
- `is_transfer`
- `tx_hash`
- `log_index`
- `block_number`
- `block_hash`
- `confirmations`
- `from_address`
- `to_address`
- `amount_raw`
- `amount_decimal`
- `balance_before_raw`
- `balance_after_raw`
- `balance_delta_raw`
- `metadata`
- `detected_at`
- `created_at`

`event_type` 可选值：

- `transfer`
- `balance_change`
- `fee_only_change`
- `contract_interaction`
- `unknown`

`direction` 可选值：

- `in`
- `out`
- `self`
- `unknown`

`is_transfer` 是前端筛选“是不是转账”的直接字段。

### 5.7 通知

#### notification_channels

- `id`
- `tenant_id`
- `channel_type`：`in_app`、`telegram`、`email`、`webhook`
- `name`
- `config`
- `status`
- `created_at`

#### notification_rules

- `id`
- `tenant_id`
- `name`
- `chain_id`
- `address_id`
- `asset_id`
- `event_type`
- `is_transfer`
- `min_amount_raw`
- `direction`
- `channel_ids`
- `enabled`
- `created_at`

#### notification_deliveries

- `id`
- `tenant_id`
- `event_id`
- `channel_id`
- `status`
- `attempt_count`
- `last_error`
- `sent_at`
- `created_at`

## 6. 多链监听设计

### 6.1 统一接口

后端定义统一 `ChainProvider` trait：

```rust
trait ChainProvider {
    async fn get_latest_height(&self) -> Result<BlockHeight>;

    async fn get_balances(
        &self,
        address: &WatchedAddress,
        assets: &[Asset],
    ) -> Result<Vec<AssetBalance>>;

    async fn get_recent_transfers(
        &self,
        address: &WatchedAddress,
        from_height: Option<BlockHeight>,
        assets: &[Asset],
    ) -> Result<Vec<RawTransfer>>;

    async fn get_transaction_detail(
        &self,
        tx_hash: &str,
    ) -> Result<RawTransaction>;
}
```

实现：

- `BtcProvider`
- `EvmProvider`
- `TronProvider`
- `BaseProvider` 或 EVM 配置实例

后期新增网络时，新增 provider 并注册到 provider factory。

### 6.2 ETH / BASE

ETH 和 BASE 采用 EVM 统一策略：

- WebSocket 订阅新区块。
- 查询 native transaction。
- 查询 ERC20 `Transfer(address,address,uint256)` 日志。
- 不可用时退回轮询。
- Base 复用 EVM parser。

事件来源：

- native transfer
- ERC20 Transfer log
- gas fee 导致余额减少
- 合约交互导致余额变化

### 6.3 TRON

TRON 初版采用：

- TronGrid / FullNode REST API 轮询。
- 查询 TRX transfer。
- 查询 TRC20 transfer events。

后期可加入 webhook 或事件订阅。

### 6.4 BTC

BTC 采用 UTXO 策略：

- 查询地址 UTXO。
- 查询地址交易历史。
- 查询 mempool 或最近区块交易。
- 计算余额变化。
- 根据 inputs / outputs 判断方向。

判断：

```text
received = sum(outputs to watched address)
spent = sum(inputs from watched address)
delta = received - spent
```

- `delta > 0`：入账。
- `delta < 0`：出账。
- `delta = 0`：可能是自转或无净变化。

## 7. 事件分类

### 7.1 transfer

条件：

- BTC 地址在交易 input/output 中出现并产生净变化。
- EVM native tx 的 from/to 命中监听地址。
- ERC20 Transfer 日志 from/to 命中监听地址。
- TRON TRX/TRC20 transfer 命中监听地址。

字段：

- `event_type = transfer`
- `is_transfer = true`
- `direction = in / out / self`

### 7.2 balance_change

条件：

- 当前余额与上次快照不同。
- 找不到明确 transfer 事件。
- 或 provider 只返回余额，不返回完整交易细节。

字段：

- `event_type = balance_change`
- `is_transfer = false`

### 7.3 fee_only_change

条件：

- 地址余额减少。
- 地址是交易发起方。
- 减少金额主要来自 gas 或 fee。
- 没有对应资产转出。

字段：

- `event_type = fee_only_change`
- `is_transfer = false`

### 7.4 contract_interaction

条件：

- 地址参与合约调用。
- 余额变化或 token 变化由合约产生。
- 无法简单归类为普通转账。

`is_transfer` 根据是否存在 token transfer 设置。

### 7.5 unknown

条件：

- 检测到变化。
- Provider 数据不足以分类。

字段：

- `event_type = unknown`
- `is_transfer = false`

## 8. 幂等、限流和失败处理

### 8.1 幂等

- Redis `SETNX scan-lock:{address_id}` 防止同一地址重复扫描。
- PostgreSQL 唯一约束兜底。
- 事件唯一键建议：`chain_id + tx_hash + log_index + address_id + asset_id + event_type`。

### 8.2 限流

Provider 级限流：

- token bucket。
- 超时控制。
- 指数退避。
- provider 熔断。
- 多 provider fallback。

### 8.3 失败处理

Provider 失败：

- 单次失败重试。
- 多次失败切换 provider。
- 全部失败则任务延后。
- 记录错误指标。

数据不完整：

- 可写入 `unknown` 事件。
- 后续扫描补充确认数或分类。
- 单个 token 查询失败不应导致整个地址任务失败。

区块重组：

- 事件先以低确认状态写入。
- 达到确认数后标记 confirmed。
- 如果后续发现 block hash 不一致，标记 reorged。

## 9. 通知流程

通知在事件入库后触发：

```text
address_events inserted
  -> match notification_rules
  -> enqueue notify task
  -> notifier sends in-app / websocket / telegram
  -> notification_deliveries records result
```

通知规则支持：

- 链。
- 地址。
- 资产。
- 事件类型。
- `is_transfer`。
- 方向。
- 最小金额。
- 地址优先级。
- 通知渠道。

第一版通知渠道：

- 站内通知。
- WebSocket 实时推送。
- Telegram Bot。

后期扩展：

- Email。
- Webhook。
- 企业微信。
- Discord。

## 10. React + Semi UI 前端设计

### 10.1 技术栈

- React。
- TypeScript。
- Vite。
- `@douyinfe/semi-ui`。
- `@douyinfe/semi-icons`。
- React Router。
- TanStack Query。
- Zustand。
- ECharts 或 Recharts。
- WebSocket client。

### 10.2 页面

- 登录页。
- 仪表盘。
- 地址监听。
- 事件中心。
- 通知规则。
- 通知渠道。
- 资产配置。
- 网络 / Provider 配置。
- Worker / 系统状态。
- 设置。

### 10.3 事件中心

事件中心使用 Semi `Table` 和顶部筛选表单。

筛选项：

- 链。
- 地址 / 标签。
- 资产。
- 事件类型。
- 是否转账：全部、是、否。
- 方向。
- 金额区间。
- 时间范围。
- 确认状态。
- 通知状态。

接口示例：

```http
GET /api/events?chain=eth&is_transfer=true&direction=in&asset=USDT
```

表格列：

- 时间。
- 链。
- 地址标签。
- 地址。
- 资产。
- 事件类型。
- 是否转账。
- 方向。
- 金额。
- 余额变化。
- 确认数。
- 通知状态。
- 交易哈希。
- 操作。

### 10.4 地址监听

功能：

- 添加单个地址。
- 批量导入地址。
- 编辑标签。
- 选择链。
- 选择资产范围。
- 设置优先级。
- 设置扫描频率。
- 启用 / 关闭转账通知。
- 启用 / 关闭余额变化通知。
- 暂停 / 恢复监听。

### 10.5 实时通知

WebSocket 消息示例：

```json
{
  "type": "address_event.created",
  "payload": {
    "event_id": "...",
    "chain": "ethereum",
    "address": "0x...",
    "asset": "USDT",
    "event_type": "transfer",
    "is_transfer": true,
    "direction": "in",
    "amount": "1000.00",
    "tx_hash": "0x..."
  }
}
```

前端收到后：

- 弹出 Semi `Notification`。
- 增加未读 `Badge`。
- 事件中心列表顶部插入。
- 仪表盘统计刷新。

## 11. 技术栈

### 11.1 后端

- Rust stable。
- `axum`：HTTP API 和 WebSocket。
- `tokio`：异步运行时。
- `sqlx`：PostgreSQL 和 migration。
- `serde` / `serde_json`。
- `reqwest`。
- `alloy`：EVM RPC / WebSocket / log 解析。
- `redis`。
- `tracing` / `tracing-subscriber`。
- `thiserror` / `anyhow`。
- `config` 或 `figment`。
- `jsonwebtoken`。
- `argon2`。

新项目建议优先选择 `alloy` 作为 EVM 基础库。

### 11.2 前端

- React。
- TypeScript。
- Vite。
- Semi UI。
- Semi Icons。
- React Router。
- TanStack Query。
- Zustand。
- ECharts 或 Recharts。

## 12. 部署与打包

### 12.1 Docker Compose

第一版服务：

```text
postgres
redis
api-server
scheduler
worker
notifier
frontend
nginx optional
```

配置文件：

```text
.env
config/default.toml
config/local.toml
```

关键配置：

- 数据库连接。
- Redis 连接。
- JWT secret。
- Provider URL / API key 引用。
- 每条链并发数。
- 每个 provider QPS。
- 默认扫描频率。
- Telegram bot token。
- 日志级别。

### 12.2 all-in-one 单体应用

单体应用模式提供两种交付物：

- 单个 Rust 可执行文件：`coin-listener-all-in-one`。
- 单个 Docker 镜像：包含 `coin-listener-all-in-one` 和前端静态资源。

运行方式：

```bash
coin-listener-all-in-one --config config/local.toml
```

内部结构：

```text
all-in-one process
  -> HTTP API / WebSocket
  -> Static frontend serving
  -> Scheduler background task
  -> Worker background task pool
  -> Notifier background task
  -> PostgreSQL
  -> Redis
```

打包策略：

- 前端先执行 `npm run build` 生成 `frontend/dist`。
- `all-in-one` 使用 `tower-http` 或等价静态文件服务托管 `frontend/dist`。
- Docker 单镜像构建时先构建前端，再构建 Rust binary，最后复制前端产物和 binary 到 runtime image。
- 二进制单文件打包可以在后期使用 `rust-embed` 把前端资源嵌入 binary；MVP 可先采用 binary + dist 目录，再演进到真正单文件。

适用场景：

- 本地开发演示。
- 小规模私有化部署。
- 单机部署环境。
- 客户希望最少组件交付的场景。

不适用场景：

- 几万地址以上的高并发平台部署。
- 需要独立扩容 worker / scheduler / notifier 的环境。

## 13. 里程碑

### Milestone 1：工程骨架和基础设施

- Rust workspace。
- React + Semi UI。
- Docker Compose。
- PostgreSQL / Redis。
- 配置管理。
- migration 框架。
- 基础日志。
- API health check。

### Milestone 2：用户、地址、资产、链配置

- 登录。
- 默认租户。
- 链配置。
- Provider 配置。
- 资产配置。
- 地址管理。
- 地址格式校验。

### Milestone 3：事件模型和 EVM / BASE 监听

- EVM provider。
- Base 复用 EVM provider。
- Native transfer。
- ERC20 transfer。
- 余额快照。
- `is_transfer` 分类。
- 事件中心筛选。

### Milestone 4：BTC / TRON 监听

- BTC provider。
- TRON provider。
- BTC UTXO 变化。
- TRX / TRC20 transfer。
- 统一事件归一化。

### Milestone 5：Scheduler / Worker 并发化

- Redis scan queue。
- 多 worker 消费。
- 每链并发池。
- Provider 限流。
- 锁和幂等。
- 失败重试。

### Milestone 6：通知系统

- 通知规则。
- 站内通知。
- WebSocket 实时推送。
- Telegram Bot。
- 通知失败重试。

### Milestone 7：运维状态和部署完善

- Worker 状态。
- 队列积压。
- Provider 健康状态。
- 错误统计。
- Docker Compose 完善。
- 部署说明。

### Milestone 8：all-in-one 单体应用打包

- 新增 `all-in-one` crate。
- 复用 API 路由和后台任务模块。
- 托管前端静态构建产物。
- 提供单体可执行文件构建命令。
- 提供单镜像 Dockerfile。
- 明确单体模式和多进程模式的配置差异。

## 14. 验收标准

MVP 完成时应满足：

1. 用户能登录后台。
2. 用户能添加 BTC / ETH / TRON / BASE 地址。
3. 系统能按配置周期扫描地址。
4. 系统能记录余额快照。
5. 系统能生成统一 `address_events`。
6. 事件能区分 `is_transfer = true/false`。
7. 前端能筛选“是不是转账”。
8. 前端能筛选链、地址、资产、方向、金额、时间。
9. 满足通知规则时，系统能生成站内通知。
10. 在线用户能通过 WebSocket 收到事件。
11. Telegram 能收到通知。
12. Worker 支持多线程并发扫描。
13. 多 worker 运行时不会重复处理同一地址。
14. Provider 失败时有重试和 fallback。
15. Docker Compose 能启动完整系统。
16. `all-in-one` 模式能构建并启动单体应用，提供 API、前端静态页面和后台任务。

## 15. 推荐实施顺序

1. 工程骨架。
2. 数据库和事件模型。
3. React + Semi UI 基础后台。
4. ETH / BASE EVM provider。
5. 事件中心和 `is_transfer` 筛选。
6. Scheduler / Worker 并发。
7. TRON。
8. BTC。
9. 通知系统。
10. 系统状态和部署完善。
11. `all-in-one` 单体应用打包。

优先做 ETH / BASE，是因为两者共享 EVM 逻辑，能最快跑通端到端链路。BTC 的 UTXO 模型差异最大，建议后做。
