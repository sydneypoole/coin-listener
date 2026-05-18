# Coin Listener Scheduler / Worker Scan Queue Design

日期：2026-05-17

## 1. 目标

本设计补齐 Coin Listener 的扫描调度与消费骨架，让系统从“手动开发模拟扫描”推进到“由 scheduler 定期入队、由 worker 消费并生成事件”的可运行链路。

本阶段优先建设可复用基础设施，不接真实链上 RPC。EVM / Base 地址继续复用 Milestone 3 的 deterministic mock scanner；BTC / TRON provider、通知系统和运维 UI 在后续里程碑实现。

## 2. 范围

### 2.1 包含

- 定义 `ScanAddressTask` 队列消息。
- 增加 Redis scan queue 封装。
- 增加 Redis 地址级扫描锁，避免多 worker 重复处理同一地址。
- 增加到期地址查询：`status = active` 且 `next_scan_at <= NOW()`。
- 增加扫描状态更新：成功后写入 `last_scanned_at` 和新的 `next_scan_at`。
- scheduler 周期性查询到期地址并写入 Redis 队列。
- worker 从 Redis 队列消费任务，获取锁，执行扫描，更新扫描时间。
- EVM / Base 地址通过现有 mock scanner 生成 `address_events`。
- 非 EVM 地址在 worker 中标记为暂不支持，并按失败策略延后。
- 后端测试覆盖队列消息序列化、扫描时间计算、worker 锁语义的可测试部分。

### 2.2 不包含

- 真实 EVM RPC / WebSocket / Alloy 集成。
- BTC provider、TRON provider。
- 通知规则、站内通知、WebSocket、Telegram。
- Provider token bucket 限流完整实现。
- 多 provider fallback。
- all-in-one 单体打包。
- 运维状态前端页面。

## 3. 架构

```text
PostgreSQL watched_addresses
  -> scheduler queries due active addresses
  -> Redis list scan:address:queue
  -> worker BRPOP consumes ScanAddressTask
  -> Redis SET NX EX scan:address:lock:{address_id}
  -> storage mock EVM scan creates address_events
  -> watched_addresses last_scanned_at / next_scan_at updated
```

### 3.1 设计原则

- API server 不执行重扫描任务，保持请求响应轻量。
- Redis queue 是 scheduler 和 worker 的唯一任务交接边界。
- 地址级 Redis lock 是防重入口；PostgreSQL 事件唯一索引继续兜底事件幂等。
- 先实现可运行的单队列骨架，后续再扩展每链队列、provider 限流和多 provider fallback。
- worker 对不支持的链不崩溃，记录失败并延后扫描。

## 4. 数据模型与消息

### 4.1 ScanAddressTask

新增 Rust 模型：

```rust
pub struct ScanAddressTask {
    pub task_id: Uuid,
    pub address_id: Uuid,
    pub tenant_id: Uuid,
    pub chain_id: Uuid,
    pub attempt: u16,
    pub enqueued_at: DateTime<Utc>,
}
```

字段含义：

- `task_id`：单次队列任务标识，用于日志和排障。
- `address_id`：监听地址 ID。
- `tenant_id`：租户 ID，后续通知与隔离复用。
- `chain_id`：链 ID，worker 可按链路由。
- `attempt`：当前尝试次数，首次为 `1`。
- `enqueued_at`：入队时间。

### 4.2 Redis key

```text
scan:address:queue
scan:address:lock:{address_id}
```

- queue 使用 Redis list。
- lock 使用 `SET key value NX EX <ttl>`。
- lock value 使用 `task_id`，便于日志定位。
- 默认 lock TTL 为 120 秒，避免 worker 异常退出后永久阻塞。

## 5. Scheduler 行为

scheduler 启动后按固定间隔运行 tick：

1. 查询到期 active 地址。
2. 每个地址构造一个 `ScanAddressTask`。
3. 写入 Redis queue。
4. 将该地址 `next_scan_at` 延后到 `NOW() + scan_interval_seconds`，避免 scheduler 在 worker 消费前重复入队。
5. 输出入队数量日志。

第一版默认：

- tick 间隔：30 秒。
- 每次最多入队：100 条地址。
- Redis queue：`scan:address:queue`。

这些值通过环境变量配置：

```text
SCHEDULER_TICK_SECONDS=30
SCHEDULER_BATCH_SIZE=100
SCAN_QUEUE_KEY=scan:address:queue
SCAN_LOCK_TTL_SECONDS=120
```

## 6. Worker 行为

worker 启动后循环消费 Redis queue：

1. `BRPOP scan:address:queue 5` 等待任务。
2. 反序列化 `ScanAddressTask`。
3. 获取 `scan:address:lock:{address_id}`。
4. 如果未获取锁，跳过任务并记录日志。
5. 读取 watched address 和 chain。
6. 如果 `chain_type = evm`，调用现有 `create_mock_evm_event`。
7. 如果链类型不是 `evm`，记录暂不支持并延后扫描。
8. 成功或可控失败后更新 `last_scanned_at` 与 `next_scan_at`。
9. 释放锁。

第一版 worker 不做无限重试。无法解析任务、地址不存在、链不支持、mock scan 验证失败时，记录日志并继续处理下一条任务。

## 7. 错误处理

| 场景 | 行为 |
|---|---|
| Redis 连接失败 | 进程启动失败，返回错误 |
| PostgreSQL 连接失败 | 进程启动失败，返回错误 |
| queue payload 不是合法 JSON | 丢弃该消息并记录错误 |
| 地址不存在 | 丢弃该消息并记录错误 |
| 未获取扫描锁 | 跳过该消息，不重入队 |
| 非 EVM 链 | 更新下次扫描时间，记录暂不支持 |
| mock scan 重复或失败 | 记录错误，更新下次扫描时间 |
| worker 中途崩溃 | lock TTL 到期后允许后续任务处理 |

## 8. 测试策略

### 8.1 单元测试

- `ScanAddressTask` JSON 序列化和反序列化。
- `next_scan_at` 计算：当前时间 + `scan_interval_seconds`。
- Redis lock 命令参数构造或 lock value 生成。
- worker 对非 EVM 链的决策函数返回“不支持”。

### 8.2 集成验证

本阶段使用编译和可解析配置作为默认验证：

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
cargo check --workspace --manifest-path backend/Cargo.toml
cargo test --workspace --manifest-path backend/Cargo.toml
npm run build --prefix frontend
docker compose -f docker-compose.yml config
```

如果本地 Docker daemon 可用，可额外运行：

```bash
docker compose up -d postgres redis
DATABASE_URL=postgres://coin_listener:coin_listener_password@localhost:5432/coin_listener REDIS_URL=redis://localhost:6379 cargo run --manifest-path backend/Cargo.toml -p scheduler
DATABASE_URL=postgres://coin_listener:coin_listener_password@localhost:5432/coin_listener REDIS_URL=redis://localhost:6379 cargo run --manifest-path backend/Cargo.toml -p worker
```

## 9. 验收标准

1. scheduler 能查询 active 且到期的 watched addresses。
2. scheduler 能将每个到期地址序列化成 `ScanAddressTask` 并写入 Redis queue。
3. scheduler 入队后会延后 `next_scan_at`，避免同一 tick 反复入队。
4. worker 能从 Redis queue 读取任务。
5. worker 能对同一 address 使用 Redis lock 防止并发重复扫描。
6. worker 能对 EVM / Base 地址调用 mock scanner 并生成事件。
7. worker 能在扫描后更新 `last_scanned_at` 与 `next_scan_at`。
8. 非 EVM 地址不会导致 worker 崩溃。
9. 后端格式检查、编译和测试通过。
10. Docker Compose 配置仍可解析。

## 10. 后续扩展

- Milestone 4：接入 BTC / TRON provider 后，worker 按 chain type 分发到真实 provider。
- Milestone 5 后续增强：每链队列、每链并发池、provider token bucket、失败重试队列、死信队列。
- Milestone 6：事件入库后投递通知任务到 notifier queue。
- Milestone 7：暴露队列积压、worker 状态、provider 健康状态和错误统计。
