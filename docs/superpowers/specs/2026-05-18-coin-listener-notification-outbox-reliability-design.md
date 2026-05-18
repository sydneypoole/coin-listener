# Coin Listener Notification Outbox Reliability Design

日期：2026-05-18

## 1. 目标

消除当前通知链路中“`address_events` 已写入数据库，但 Redis notify task 未成功入队或已出队后丢失”的一致性缺口。新链路使用 PostgreSQL notification outbox 作为可靠任务源，让通知处理可以在 worker、notifier 或 Redis 故障后恢复。

本阶段只聚焦 notification reliable delivery。Telegram/Webhook 真实发送、前端 outbox 管理页、复杂指标告警和分布式任务系统不进入本阶段。

## 2. 当前状态依据

- `backend/crates/worker/src/lib.rs` 当前扫描成功后返回本轮新插入的 `AddressEvent`，随后在 `process_locked_scan_task` 中逐条调用 `NotifyQueue::enqueue`。
- `backend/crates/storage/src/repositories.rs` 的 `insert_event_if_not_exists` 使用 `ON CONFLICT DO NOTHING RETURNING id`，事件已存在时返回 `None`。
- 如果 event insert 成功但 Redis enqueue 失败，后续 retry 会因为 event 已存在而不再返回该事件，通知任务可能永久丢失。
- `backend/crates/storage/src/notify_queue.rs` 使用 Redis list：worker `LPUSH`，notifier `BRPOP`。`BRPOP` 后 notifier 崩溃会导致该 Redis 消息丢失。
- `backend/crates/notifier/src/lib.rs` 已有通知规则匹配、channel 解析、delivery 创建和 in-app notification 创建逻辑，可复用为 outbox item 的处理逻辑。
- `backend/crates/storage/migrations/0005_notifications.sql` 已有 `notification_channels`、`notification_rules`、`notification_deliveries` 和 `in_app_notifications`，但还没有可靠任务状态表。

## 3. 范围

### 3.1 包含

1. 新增 PostgreSQL `notification_outbox` 表：
   - 每个新插入的 `address_events` 对应一个 outbox row。
   - `event_id` 唯一，防止重复任务。
   - 支持 pending、processing、retryable、delivered、failed 状态。
   - 支持 attempt count、next attempt time、lock owner、lock time、last error 和 delivered time。
2. Worker 事件写入集成：
   - 新增 repository helper，在同一个 DB transaction 中插入 event 和 outbox。
   - 只有 event 是新插入时才创建 outbox。
   - 已存在 event 不重复创建 outbox。
   - worker 不再依赖 Redis enqueue 作为可靠投递主路径。
3. Notifier outbox dispatcher：
   - notifier 从 DB claim due outbox rows，而不是以 Redis list 作为主任务源。
   - claim 使用 `FOR UPDATE SKIP LOCKED`，支持多个 notifier 实例并发。
   - 单条 outbox 处理成功后标记 delivered。
   - 处理失败后按最大重试次数标记 retryable 或 failed。
4. Stale processing recovery：
   - 启动或循环中将超时的 processing row 释放为 retryable。
   - 防止 notifier crash 后任务永久卡在 processing。
5. 幂等和重复处理：
   - outbox `event_id` 唯一。
   - delivery / in-app 创建在本阶段保持现有语义，不引入外部 channel exactly-once。
   - outbox delivered 表示该 event 的通知规则处理流程完成；无匹配规则也可标记 delivered。
6. 测试与验证：
   - migration/query 字符串测试。
   - claim/mark/retry 状态迁移 helper 测试。
   - worker event+outbox repository helper 测试。
   - notifier outbox processing tests。
   - 保持 backend workspace、frontend build 和 docker compose config 不回归。

### 3.2 不包含

- Telegram、Webhook、Email 等真实外部发送。
- 外部 channel exactly-once guarantee。
- 前端 outbox 管理页面。
- 运维告警、metrics dashboard。
- 用 Kafka/RabbitMQ 替换 Redis 或 PostgreSQL。
- 对历史 `address_events` 自动批量补 outbox。
- 重构全部 notification delivery schema。

## 4. 推荐方案

采用 PostgreSQL outbox pattern：`address_events` 和 `notification_outbox` 在同一数据库事务中创建；notifier 通过数据库状态机 claim、处理、重试和完成任务。

Redis notify queue 可以保留代码兼容，但不再作为新事件通知可靠投递的主路径。这样核心可靠性不依赖“DB insert 成功后再写 Redis”这种跨系统非原子操作，也避免 Redis `BRPOP` 出队后进程崩溃造成任务丢失。

## 5. 数据模型

新增 migration：`backend/crates/storage/migrations/0007_notification_outbox.sql`。

```sql
CREATE TABLE IF NOT EXISTS notification_outbox (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    event_id UUID NOT NULL REFERENCES address_events(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'pending',
    attempt_count INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    locked_at TIMESTAMPTZ,
    locked_by TEXT,
    last_error TEXT,
    delivered_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(event_id)
);

CREATE INDEX IF NOT EXISTS idx_notification_outbox_claim
    ON notification_outbox(status, next_attempt_at, created_at)
    WHERE status IN ('pending', 'retryable');

CREATE INDEX IF NOT EXISTS idx_notification_outbox_processing_stale
    ON notification_outbox(status, locked_at)
    WHERE status = 'processing';

CREATE INDEX IF NOT EXISTS idx_notification_outbox_event
    ON notification_outbox(event_id);
```

状态语义：

| status | 含义 |
|---|---|
| `pending` | 新创建，等待处理 |
| `processing` | 已被某个 notifier claim，正在处理 |
| `retryable` | 上次处理失败，等待下一次重试 |
| `delivered` | 通知规则处理已完成 |
| `failed` | 超过最大重试次数，停止自动重试 |

## 6. Repository 设计

### 6.1 Event + outbox 原子写入

新增 helper，例如：

```rust
pub async fn insert_event_and_outbox_if_not_exists(
    pool: &PgPool,
    draft: AddressEventDraft,
) -> AppResult<Option<AddressEvent>>
```

行为：

1. 开启 DB transaction。
2. 执行现有 `INSERT_EVENT_IF_NOT_EXISTS_QUERY`。
3. 如果返回 `Some(event)`：插入 `notification_outbox(tenant_id, event_id, status)`。
4. 如果返回 `None`：不创建 outbox。
5. commit transaction。
6. 返回 `Option<AddressEvent>`。

这个 helper 替换 worker 中需要通知的 `insert_event_if_not_exists` 调用。对于明确不需要通知的未来事件类型，可以保留原 helper。

### 6.2 Claim due outbox rows

新增 query/helper：

```rust
pub async fn claim_due_notification_outbox(
    pool: &PgPool,
    now: DateTime<Utc>,
    worker_id: &str,
    limit: i64,
) -> AppResult<Vec<NotificationOutboxItem>>
```

SQL 形态：

```sql
WITH due AS (
    SELECT id
    FROM notification_outbox
    WHERE status IN ('pending', 'retryable')
      AND next_attempt_at <= $1
    ORDER BY next_attempt_at ASC, created_at ASC
    LIMIT $2
    FOR UPDATE SKIP LOCKED
)
UPDATE notification_outbox o
SET status = 'processing',
    locked_at = $1,
    locked_by = $3,
    attempt_count = attempt_count + 1,
    updated_at = NOW()
FROM due
WHERE o.id = due.id
RETURNING o.*;
```

### 6.3 Mark delivered / retryable / failed

新增 helpers：

```rust
pub async fn mark_notification_outbox_delivered(
    pool: &PgPool,
    id: Uuid,
    now: DateTime<Utc>,
) -> AppResult<()>;

pub async fn mark_notification_outbox_retryable(
    pool: &PgPool,
    id: Uuid,
    next_attempt_at: DateTime<Utc>,
    last_error: &str,
) -> AppResult<()>;

pub async fn mark_notification_outbox_failed(
    pool: &PgPool,
    id: Uuid,
    last_error: &str,
) -> AppResult<()>;
```

状态更新必须带当前状态约束，例如 `WHERE id = $1 AND status = 'processing'`，避免误改已被其他 worker 处理的 row。

### 6.4 Recover stale processing rows

新增 helper：

```rust
pub async fn release_stale_notification_outbox(
    pool: &PgPool,
    stale_before: DateTime<Utc>,
    next_attempt_at: DateTime<Utc>,
) -> AppResult<u64>;
```

将 `status = 'processing' AND locked_at < stale_before` 的 rows 改为 `retryable`，清空 lock 字段，保留 `attempt_count`。

## 7. Worker 集成

Worker 侧目标：扫描只负责创建事件和 outbox，不再负责把 notify task 写入 Redis。

现有：

```text
scan_*_address -> returns Vec<AddressEvent>
process_locked_scan_task -> for event enqueue Redis -> finish scan
```

调整为：

```text
scan_*_address -> insert event + outbox atomically -> returns Vec<AddressEvent> for logging/tests only
process_locked_scan_task -> finish scan
notifier -> claims notification_outbox rows and processes notifications
```

具体规则：

- EVM、BTC、TRON transfer events 都使用 `insert_event_and_outbox_if_not_exists`。
- Balance snapshot 不创建 notification outbox，除非它产生 `balance_change` address_event。
- 已存在 event 不创建 outbox，不重复投递。
- `process_locked_scan_task` 不再因为 Redis notify enqueue 失败而使 scan 失败。
- `NotifyQueue` 可以暂时保留，以降低删除范围；新路径不调用它。

## 8. Notifier 集成

### 8.1 处理入口

新增处理函数，例如：

```rust
pub async fn process_notification_outbox_item(
    pool: &PgPool,
    item: NotificationOutboxItem,
    now: DateTime<Utc>,
) -> AppResult<usize>
```

它复用现有规则匹配逻辑：

1. 读取 `address_events`。
2. 查询 enabled notification rules。
3. 匹配规则。
4. 解析 channels。
5. 创建 `notification_deliveries`。
6. 创建 `in_app_notifications`。

无匹配规则返回 `Ok(0)`，outbox 仍标记 delivered。

### 8.2 Dispatcher loop

`run_notifier` 改为：

1. 周期性 release stale processing rows。
2. claim due outbox rows，建议 batch size 初始为 `50`。
3. 对每条 item 调用 `process_notification_outbox_item`。
4. 成功：mark delivered。
5. 失败：按 retry policy mark retryable 或 failed。
6. 无任务：短暂等待或 sleep。
7. 收到 shutdown：停止 claim 新任务，当前 item 处理完成后退出。

### 8.3 Retry policy

初始参数：

```text
NOTIFICATION_OUTBOX_BATCH_SIZE=50
NOTIFICATION_OUTBOX_MAX_ATTEMPTS=10
NOTIFICATION_OUTBOX_STALE_LOCK_SECONDS=300
NOTIFICATION_OUTBOX_IDLE_SLEEP_MS=500
```

Backoff 使用确定性函数，便于测试：

```rust
pub fn notification_outbox_next_attempt_at(now: DateTime<Utc>, attempt_count: i32) -> DateTime<Utc> {
    let delay_seconds = match attempt_count {
        0 | 1 => 30,
        2 => 60,
        3 => 300,
        4 => 900,
        _ => 3600,
    };
    now + chrono::Duration::seconds(delay_seconds)
}
```

注意：`attempt_count` 在 claim 时递增，因此失败处理看到的是本次已消费后的 attempt count。

## 9. 错误处理与幂等

| 场景 | 行为 |
|---|---|
| event insert 成功 | 同事务创建 outbox |
| event 已存在 | 不创建重复 outbox |
| outbox insert 失败 | transaction 回滚，event 不落库 |
| notifier claim 后崩溃 | stale processing recovery 改回 retryable |
| event 不存在 | mark failed，记录 last_error |
| 无匹配规则 | mark delivered |
| channel 不存在或 inactive | 沿用现有 skipped delivery 语义，outbox delivered |
| unsupported channel | 沿用 skipped delivery，outbox delivered |
| in-app delivery 写入失败 | outbox retryable 或 failed |
| 超过最大重试次数 | outbox failed |

本阶段不承诺外部 channel exactly-once。未来加入 Telegram/Webhook 时，需要 channel-level idempotency key 或 provider-specific de-duplication。

## 10. 验收标准

1. 新增 `notification_outbox` migration，并包含 claim/stale recovery 所需索引。
2. 新插入 address_event 时，同一个 DB transaction 创建 outbox row。
3. event 已存在时不会重复创建 outbox。
4. worker 不再依赖 Redis notify enqueue 作为通知可靠投递主路径。
5. notifier 能 claim pending/retryable outbox rows，并用 `FOR UPDATE SKIP LOCKED` 支持并发实例。
6. notifier 成功处理 outbox 后标记 delivered。
7. notifier 处理失败后按 retry policy 标记 retryable 或 failed。
8. stale processing rows 能恢复为 retryable。
9. 无匹配 notification rule 的 outbox row 能标记 delivered。
10. 现有 notification rule matching、delivery、in-app notification 行为不回归。
11. EVM、BTC、TRON worker 扫描不因 Redis notify queue 故障而丢失通知任务。
12. 后端 workspace、前端 build 和 docker compose config 不回归。

## 11. 测试策略

### 11.1 Storage tests

- migration 字符串包含 `notification_outbox`、`UNIQUE(event_id)`、claim index 和 stale processing index。
- `insert_event_and_outbox_if_not_exists`：新 event 返回 `Some(event)` 并创建 outbox。
- `insert_event_and_outbox_if_not_exists`：重复 event 返回 `None` 且不创建第二个 outbox。
- claim query 包含 `FOR UPDATE SKIP LOCKED`。
- mark delivered/retryable/failed query 都约束 `status = 'processing'`。
- stale release query 只匹配 stale processing rows。

### 11.2 Worker tests

- `process_locked_scan_task` 不再调用 Redis notify enqueue。
- EVM/BTC/TRON event insert 路径使用 event+outbox helper。
- 已存在事件不生成重复 outbox。

### 11.3 Notifier tests

- `notification_outbox_next_attempt_at` backoff 稳定。
- 成功处理 item 后调用 mark delivered。
- 处理失败且未超过 max attempts 时 mark retryable。
- 处理失败且达到 max attempts 时 mark failed。
- 无匹配 rules 时仍视为 delivered。
- shutdown 时不 claim 新任务。

### 11.4 Final verification

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
cargo check --workspace --manifest-path backend/Cargo.toml
cargo test --workspace --manifest-path backend/Cargo.toml
npm run build --prefix frontend
docker compose -f docker-compose.yml config
```

## 12. 后续里程碑

完成 outbox 后，下一优先级可以是：

1. Telegram/Webhook 真实 channel 发送，并为外部发送增加 idempotency key。
2. Notification retry/dead-letter 前端运维页面。
3. TRON native balance snapshot / balance_change event。
4. Provider failover、限流、健康探测和熔断。
