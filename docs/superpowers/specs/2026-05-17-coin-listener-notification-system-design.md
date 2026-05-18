# Coin Listener Notification System Skeleton Design

日期：2026-05-17

## 1. 目标

本设计补齐 Coin Listener 的第一版通知系统骨架，让已入库的 `address_events` 能按通知规则生成站内通知，并由独立 `notifier` 进程消费通知任务。

本阶段优先建设可运行链路：事件入库后投递通知任务，notifier 匹配规则，写入通知投递记录和站内通知。Telegram Bot、WebSocket 实时推送和复杂重试策略后续实现。

## 2. 范围

### 2.1 包含

- 新增通知相关数据表：`notification_channels`、`notification_rules`、`notification_deliveries`、`in_app_notifications`。
- 定义 `NotifyEventTask` Redis 队列消息。
- 增加 Redis notify queue 封装。
- 增加通知规则匹配逻辑。
- 增加 notification repository：规则、渠道、投递、站内通知。
- API 增加通知渠道、通知规则、站内通知查询和标记已读接口。
- notifier 进程消费通知任务，匹配规则并生成站内通知。
- worker 在成功生成 EVM mock event 后投递 notify task。
- 前端新增通知规则页和站内通知页。
- 后端测试覆盖任务序列化、规则匹配、金额阈值判断和 notifier 决策函数。

### 2.2 不包含

- Telegram Bot 真实发送。
- WebSocket 实时推送。
- Email、Webhook、企业微信、Discord。
- 通知失败重试队列和死信队列。
- 通知模板系统。
- 复杂 RBAC 或团队成员通知范围。
- 通知统计图表和运维状态页。

## 3. 架构

```text
worker creates address_events
  -> enqueue Redis notify:event:queue
  -> notifier BRPOP consumes NotifyEventTask
  -> load address_event
  -> match enabled notification_rules
  -> create notification_deliveries
  -> create in_app_notifications for in_app channels
  -> frontend lists/marks in-app notifications
```

### 3.1 设计原则

- 事件入库和通知发送解耦，worker 只投递 notify task，不直接生成通知。
- Redis notify queue 是 worker 和 notifier 的唯一任务交接边界。
- 第一版只保证 in-app channel 可运行；其他 channel 类型先保留数据模型，不做真实发送。
- 规则字段为 `NULL` 表示不过滤。
- notifier 处理单条任务失败不导致进程退出。
- delivery 是通知处理审计记录，in-app notification 是用户可见消息。

## 4. 数据模型与消息

### 4.1 notification_channels

```sql
CREATE TABLE notification_channels (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    channel_type TEXT NOT NULL,
    name TEXT NOT NULL,
    config JSONB NOT NULL DEFAULT '{}'::jsonb,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

第一版支持的 `channel_type`：

- `in_app`
- `telegram`（仅建模，不真实发送）
- `webhook`（仅建模，不真实发送）

### 4.2 notification_rules

```sql
CREATE TABLE notification_rules (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    chain_id UUID REFERENCES chains(id) ON DELETE CASCADE,
    address_id UUID REFERENCES watched_addresses(id) ON DELETE CASCADE,
    asset_id UUID REFERENCES assets(id) ON DELETE CASCADE,
    event_type TEXT,
    is_transfer BOOLEAN,
    min_amount_raw TEXT,
    direction TEXT,
    channel_ids UUID[] NOT NULL DEFAULT '{}',
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

匹配语义：

- `tenant_id` 必须等于事件租户。
- `enabled = true` 才参与匹配。
- `chain_id`、`address_id`、`asset_id`、`event_type`、`is_transfer`、`direction` 为 `NULL` 时不过滤。
- `min_amount_raw` 为 `NULL` 时不过滤金额。
- `min_amount_raw` 和事件 `amount_raw` 都按十进制非负整数字符串比较。
- 规则配置了 `min_amount_raw` 但事件 `amount_raw` 为空或不能解析时，该规则不匹配。
- `channel_ids` 为空时默认投递到租户的 active `in_app` channel。

### 4.3 notification_deliveries

```sql
CREATE TABLE notification_deliveries (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    event_id UUID NOT NULL REFERENCES address_events(id) ON DELETE CASCADE,
    rule_id UUID REFERENCES notification_rules(id) ON DELETE SET NULL,
    channel_id UUID REFERENCES notification_channels(id) ON DELETE SET NULL,
    status TEXT NOT NULL,
    attempt_count INTEGER NOT NULL DEFAULT 1,
    last_error TEXT,
    sent_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

`status` 第一版取值：

- `sent`：站内通知已写入。
- `skipped`：channel 类型暂未实现或 channel 不可用。
- `failed`：处理失败。

### 4.4 in_app_notifications

```sql
CREATE TABLE in_app_notifications (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    event_id UUID NOT NULL REFERENCES address_events(id) ON DELETE CASCADE,
    delivery_id UUID REFERENCES notification_deliveries(id) ON DELETE SET NULL,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    read_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

第一版 title/body 由事件字段生成：

- title：`{event_type} {direction}`。
- body：包含地址、资产、金额、交易哈希的简短文本；缺失字段用 `-`。

### 4.5 NotifyEventTask

新增 Rust 模型：

```rust
pub struct NotifyEventTask {
    pub task_id: Uuid,
    pub event_id: Uuid,
    pub tenant_id: Uuid,
    pub attempt: u16,
    pub enqueued_at: DateTime<Utc>,
}
```

字段含义：

- `task_id`：单次通知任务标识，用于日志定位。
- `event_id`：触发通知的事件 ID。
- `tenant_id`：租户 ID，用于规则匹配和隔离。
- `attempt`：当前尝试次数，首次为 `1`。
- `enqueued_at`：入队时间。

### 4.6 Redis key

```text
notify:event:queue
```

- queue 使用 Redis list。
- producer 使用 `LPUSH`。
- notifier 使用 `BRPOP notify:event:queue 5`。
- `LPUSH + BRPOP` 保持 FIFO。

环境变量：

```text
NOTIFY_QUEUE_KEY=notify:event:queue
```

## 5. API 设计

### 5.1 通知渠道

```http
GET /api/notification-channels
POST /api/notification-channels
```

第一版创建 channel 时支持：

- `channel_type`
- `name`
- `config`
- `status`

### 5.2 通知规则

```http
GET /api/notification-rules
POST /api/notification-rules
PUT /api/notification-rules/:id
DELETE /api/notification-rules/:id
```

删除采用硬删除。第一版不做规则历史版本。

### 5.3 站内通知

```http
GET /api/in-app-notifications?unread_only=true
POST /api/in-app-notifications/:id/read
```

列表按 `created_at DESC` 返回最多 200 条。标记已读写入 `read_at = NOW()`。

## 6. Notifier 行为

notifier 启动后：

1. 连接 PostgreSQL 和 Redis。
2. `BRPOP notify:event:queue 5` 等待任务。
3. 反序列化 `NotifyEventTask`。
4. 读取 `address_events`。
5. 查询同租户 enabled notification rules。
6. 对每条规则执行匹配。
7. 解析规则 channel：
   - `channel_ids` 非空：使用指定 active channels。
   - `channel_ids` 为空：使用租户 active `in_app` channels；如果不存在则创建默认 `in_app` channel。
8. 对 `in_app` channel：创建 delivery，创建 in-app notification，delivery 标记 `sent`。
9. 对暂未实现 channel：创建 delivery，状态为 `skipped`，`last_error = 'channel type not implemented'`。
10. 继续消费下一条任务。

无匹配规则是正常结果，记录 debug/info 日志，不创建 delivery。

## 7. Worker 集成

worker 处理 EVM mock scan 成功后：

1. `create_mock_evm_event` 返回 `AddressEvent`。
2. worker 构造 `NotifyEventTask`。
3. 写入 `notify:event:queue`。
4. notify 入队成功后继续 `finish_address_scan`。

如果 notify 入队失败：

- 本阶段将扫描视为失败，不推进 `last_scanned_at` / `next_scan_at`。
- Redis 恢复后 scheduler 会重新扫描并再次尝试通知任务。
- PostgreSQL 事件唯一索引兜底重复事件；重复事件导致 mock event insert 返回 validation error 时，worker 不推进扫描时间，等待后续策略优化。

## 8. 前端设计

### 8.1 通知规则页

新增页面：`NotificationRulesPage`。

功能：

- 展示规则列表。
- 创建规则。
- 编辑规则。
- 删除规则。
- 展示 enabled 状态。

字段控件：

- 名称：Semi `Input`。
- 链、地址、资产：Semi `Select`。
- 事件类型、方向、是否转账：Semi `Select`。
- 最小金额 raw：Semi `Input`。
- 启用：Semi `Switch`。

### 8.2 站内通知页

新增页面：`InAppNotificationsPage`。

功能：

- 展示通知列表。
- 按未读筛选。
- 标记单条已读。

表格列：

- 时间。
- 标题。
- 内容。
- 已读状态。
- 操作。

## 9. 错误处理

| 场景 | 行为 |
|---|---|
| Notify task JSON 非法 | 丢弃消息并记录错误 |
| event 不存在 | 丢弃消息并记录错误 |
| 无匹配规则 | 正常跳过 |
| channel 不存在或 inactive | delivery 记录 `skipped` |
| channel 类型未实现 | delivery 记录 `skipped` |
| in-app 写入失败 | delivery 记录 `failed` |
| Redis notify queue 连接失败 | worker/notifier 启动失败或当前扫描失败 |
| notifier 处理中单条规则失败 | 记录 failed delivery，继续其他规则 |

## 10. 测试策略

### 10.1 单元测试

- `NotifyEventTask` JSON 序列化和反序列化。
- `notification_rule_matches_event`：链、地址、资产、事件类型、方向、is_transfer。
- `min_amount_raw`：大于、等于、小于、缺失、非法字符串。
- `build_in_app_notification_content` 生成稳定 title/body。
- notify queue payload round-trip。
- notifier 对 unsupported channel 返回 skipped 决策。

### 10.2 集成验证

默认验证命令：

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
DATABASE_URL=postgres://coin_listener:coin_listener_password@localhost:5432/coin_listener REDIS_URL=redis://localhost:6379 ENABLE_DEV_ROUTES=true cargo run --manifest-path backend/Cargo.toml -p api-server
DATABASE_URL=postgres://coin_listener:coin_listener_password@localhost:5432/coin_listener REDIS_URL=redis://localhost:6379 cargo run --manifest-path backend/Cargo.toml -p notifier
```

再通过前端或 API 创建 in-app channel、notification rule，并用开发扫描接口生成 mock event，确认站内通知可查询。

## 11. 验收标准

1. 可以创建和查询 notification channels。
2. 可以创建、编辑、删除和查询 notification rules。
3. 可以查询站内通知并标记已读。
4. `NotifyEventTask` 能序列化进入 Redis queue。
5. worker 成功生成事件后会投递 notify task。
6. notifier 能消费 notify task。
7. notifier 能匹配 enabled notification rules。
8. in-app channel 能生成 notification delivery 和 in-app notification。
9. 暂未实现 channel 不会导致 notifier 崩溃。
10. 无匹配规则不会导致 notifier 报错。
11. 后端格式检查、编译和测试通过。
12. 前端构建通过。
13. Docker Compose 配置仍可解析。

## 12. 后续扩展

- 接入 WebSocket：in-app notification 创建后广播给在线用户。
- 接入 Telegram Bot：实现 telegram channel sender 和失败重试。
- 增加 notify retry queue 和 dead-letter queue。
- 增加 notification delivery 详情页和错误统计。
- 增加通知模板和多语言。
- 增加按用户/角色的通知范围。
