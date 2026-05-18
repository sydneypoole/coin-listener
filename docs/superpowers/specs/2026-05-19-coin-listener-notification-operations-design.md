# Coin Listener Notification Operations Console Design

日期：2026-05-19

## 1. 目标

为已经落地的 PostgreSQL notification outbox 和 Telegram/Webhook external delivery 增加运维可视化与安全的手动重试能力，让操作者可以在前端查看通知任务积压、失败、重试、卡住的 processing lock、外部 provider 响应和 delivery 元数据。

本阶段不改变 worker 事件写入、notifier 自动重试、Telegram/Webhook 发送模型，也不追求 provider-side exactly-once。目标是把已有可靠性数据变成可查询、可诊断、可有限干预的运维台。

## 2. 当前状态依据

- `backend/crates/storage/migrations/0007_notification_outbox.sql` 已定义 `notification_outbox`，包含 `status`、`attempt_count`、`next_attempt_at`、`locked_at`、`locked_by`、`last_error`、`delivered_at`、`created_at`、`updated_at`。
- `backend/crates/storage/migrations/0008_external_notification_deliveries.sql` 已扩展 `notification_deliveries`，包含 `channel_type`、`idempotency_key`、`provider_message_id`、`provider_status_code`、`provider_response`。
- `backend/crates/storage/src/repositories.rs` 已有 outbox 写入、claim、delivered、retryable、failed 和 stale release helper，但没有运维列表、详情或手动 retry helper。
- `backend/crates/storage/src/notifications.rs` 已有 notification channel/rule CRUD、in-app notification 查询和 external delivery 写入 helper，但没有 delivery 运维查询。
- `backend/crates/storage/src/system_status.rs` 只统计 24h delivery status 和 unread in-app，不展示 outbox backlog、failed、retryable 或 stale processing。
- `backend/crates/api-server/src/routes.rs` 已暴露 notification channels、rules、in-app notifications 和 system status；未暴露 notification outbox 或 delivery 运维接口。
- `frontend/src/api/types.ts` 与 `frontend/src/api/client.ts` 没有 notification outbox/delivery 运维 DTO 或 API client。
- `frontend/src/pages/SystemStatusPage.tsx`、`InAppNotificationsPage.tsx`、`NotificationRulesPage.tsx` 已提供 Semi Design 页面、Table、Tag、filter、React Query、Toast 和 mutation 使用模式，可复用。

## 3. 范围

### 3.1 包含

1. Outbox 运维列表：
   - 查询 `notification_outbox` rows。
   - 支持按 `status`、`event_id`、分页参数过滤。
   - 展示 event 摘要、attempt、next attempt、lock、last error、delivery 计数。
2. Outbox 详情：
   - 查询单个 outbox row。
   - 同时返回关联 `address_events` 摘要和相关 `notification_deliveries`。
3. Delivery 运维列表：
   - 查询 `notification_deliveries` rows。
   - 支持按 `event_id`、`status`、`channel_type`、`rule_id`、`channel_id`、分页参数过滤。
   - 展示 idempotency key、provider message/status/response、last error。
4. 手动 outbox retry：
   - 新增 outbox-level retry endpoint。
   - 只允许对 `failed` 和 `retryable` outbox row 执行。
   - retry 后让 row 重新进入 notifier 可 claim 的状态。
5. 系统状态增强：
   - 在 system status 的 notification 部分增加 outbox status counts。
   - 展示 pending、retryable、processing、failed、stale processing 和 next due 信息。
6. 前端 Notification Operations 页面：
   - 新增导航入口。
   - 顶部展示 outbox summary cards。
   - 主表展示 outbox rows 和 retry 操作。
   - 详情区域或弹窗展示 event/delivery/provider metadata。
   - provider response 和长错误默认折叠或截断展示。
7. 测试与验证：
   - Storage query/helper 字符串测试。
   - API route 方法/参数/错误状态测试。
   - Frontend build 不回归。
   - Backend workspace fmt/check/test 不回归。
   - Docker Compose config 不回归。

### 3.2 不包含

- 单个 `notification_deliveries` row 的手动 retry。
- 对 `delivered` outbox row 的 replay。
- 对活跃 `processing` row 的强制 unlock。
- Provider health probing、RPC failover、rate limit 或 circuit breaker。
- Telegram/Webhook channel config 表单或 test-send UX。
- Prometheus、告警系统、WebSocket 实时推送。
- 解决 Telegram provider-side exactly-once。
- 引入新的队列系统。

## 4. 推荐方案

采用“只读诊断 + 收窄的 outbox-level retry”方案。

Outbox 仍然是唯一可靠任务源。运维台不直接修改 delivery row，也不绕过 notifier 调用外部 provider。手动 retry 只把符合条件的 outbox row 重新放回自动处理状态，让现有 notifier 逻辑复用 outbox attempt、delivery idempotency 和 error classification。

这样可以避免做一个独立的“手动发送器”，也避免 UI 对 Telegram/Webhook/in-app 等 channel 细节做重复实现。

## 5. 状态语义

### 5.1 Outbox status

| status | 运维含义 | UI 操作 |
|---|---|---|
| `pending` | 新任务等待 notifier claim | 只读 |
| `processing` | 某个 notifier 已 claim，可能正在处理 | 只读；标记是否 stale |
| `retryable` | 上次处理失败，等待自动 retry | 可手动 retry |
| `delivered` | event 的通知规则处理流程完成 | 只读 |
| `failed` | 超过最大自动尝试次数 | 可手动 retry |

### 5.2 Delivery status

| status | 运维含义 |
|---|---|
| `processing` | 外部发送尝试已创建但未完成；可能等待同一 outbox attempt 收尾 |
| `sent` | channel 发送或 in-app 创建成功 |
| `skipped` | 业务跳过，例如 inactive/missing/unsupported channel |
| `failed` | 单 channel 失败；是否导致 outbox retry 取决于 error classification |

重要区别：`notification_outbox.status = failed` 是 event-level 任务失败；`notification_deliveries.status = failed` 可能是单个外部 channel 永久失败，且 outbox 仍可为 `delivered`。

## 6. Backend API 设计

### 6.1 List outbox

```text
GET /api/notification-outbox?status=failed&event_id=<uuid>&limit=50&offset=0
```

响应：

```json
{
  "items": [
    {
      "id": "<uuid>",
      "tenant_id": "<uuid>",
      "event_id": "<uuid>",
      "status": "failed",
      "attempt_count": 5,
      "next_attempt_at": "2026-05-19T10:00:00Z",
      "locked_at": null,
      "locked_by": null,
      "last_error": "external notification error: webhook returned retryable status 500",
      "delivered_at": null,
      "created_at": "2026-05-19T09:00:00Z",
      "updated_at": "2026-05-19T09:20:00Z",
      "event_type": "transfer",
      "direction": "in",
      "tx_hash": "0xabc",
      "delivery_total": 2,
      "delivery_sent": 1,
      "delivery_failed": 1,
      "delivery_skipped": 0,
      "is_stale_processing": false
    }
  ],
  "limit": 50,
  "offset": 0
}
```

Rules:

- `limit` default 50, minimum 1, maximum 100。
- `offset` default 0。
- `status` 必须是已知 outbox status，否则返回 400。
- `event_id` 必须是 UUID，否则由 Axum 返回 400。

### 6.2 Get outbox detail

```text
GET /api/notification-outbox/:id
```

响应包含：

- outbox row。
- event 摘要。
- related deliveries。

如果 outbox id 不存在，返回 404。

### 6.3 Retry outbox

```text
POST /api/notification-outbox/:id/retry
```

允许状态：

- `failed`
- `retryable`

行为：

1. 检查 row 存在。
2. 如果 status 不是 `failed` 或 `retryable`，返回 validation error。
3. 更新 row：
   - `status = 'retryable'`
   - `next_attempt_at = now()`
   - `locked_at = NULL`
   - `locked_by = NULL`
   - `last_error = NULL`
   - `updated_at = now()`
4. 不重置 `attempt_count`。
5. 不修改任何 delivery row。
6. 返回更新后的 outbox row。

不重置 `attempt_count` 的原因：attempt history 是审计数据；下一次 notifier claim 会继续递增 attempt。后续如需要“reset attempt budget”，应作为单独运维操作设计。

### 6.4 List deliveries

```text
GET /api/notification-deliveries?event_id=<uuid>&status=failed&channel_type=webhook&limit=50&offset=0
```

响应：

```json
{
  "items": [
    {
      "id": "<uuid>",
      "tenant_id": "<uuid>",
      "event_id": "<uuid>",
      "rule_id": "<uuid>",
      "channel_id": "<uuid>",
      "channel_type": "webhook",
      "status": "failed",
      "attempt_count": 3,
      "last_error": "webhook returned retryable status 500",
      "sent_at": null,
      "created_at": "2026-05-19T09:00:00Z",
      "idempotency_key": "notification:v1:...",
      "provider_message_id": null,
      "provider_status_code": 500,
      "provider_response": "server error"
    }
  ],
  "limit": 50,
  "offset": 0
}
```

Rules:

- `status` 必须是 `sent`、`skipped`、`failed` 或 `processing`。
- `channel_type` 当前允许 `in_app`、`telegram`、`webhook`。
- Provider response 原样来自数据库，但前端默认折叠显示。

## 7. Core DTO 设计

新增或扩展 `backend/crates/core/src/models.rs`：

- `NotificationOutboxQuery`
- `NotificationDeliveryQuery`
- `NotificationOutboxListResponse`
- `NotificationOutboxListItem`
- `NotificationOutboxDetail`
- `NotificationDeliveryListResponse`
- `NotificationDeliveryListItem`
- `RetryNotificationOutboxResponse`
- `OutboxStatusCounts`

Query structs 使用 `serde::Deserialize`，response structs 使用 `serde::Serialize`。

## 8. Storage 设计

### 8.1 Outbox repositories

`backend/crates/storage/src/repositories.rs` 负责：

- `list_notification_outbox(pool, query) -> AppResult<Vec<NotificationOutboxListItem>>`
- `get_notification_outbox_detail(pool, id) -> AppResult<NotificationOutboxDetail>`
- `retry_notification_outbox(pool, id, now) -> AppResult<NotificationOutboxItem>`
- `notification_outbox_status_counts(pool, now, stale_before) -> AppResult<OutboxStatusCounts>`

List query 应 left join `address_events`，并用 subquery/group by 统计 delivery counts。避免 N+1 查询。

### 8.2 Delivery queries

`backend/crates/storage/src/notifications.rs` 负责：

- `list_notification_deliveries(pool, query) -> AppResult<Vec<NotificationDeliveryListItem>>`
- `list_notification_deliveries_for_event(pool, event_id) -> AppResult<Vec<NotificationDeliveryListItem>>`

### 8.3 Indexes

新增 migration：`backend/crates/storage/migrations/0009_notification_ops_indexes.sql`。

```sql
CREATE INDEX IF NOT EXISTS idx_notification_outbox_tenant_status_created
    ON notification_outbox(tenant_id, status, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_notification_outbox_tenant_next_attempt
    ON notification_outbox(tenant_id, next_attempt_at);

CREATE INDEX IF NOT EXISTS idx_notification_deliveries_tenant_status_created
    ON notification_deliveries(tenant_id, status, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_notification_deliveries_tenant_channel_type_created
    ON notification_deliveries(tenant_id, channel_type, created_at DESC);
```

本项目当前 API 仍沿用默认 tenant 语义；运维查询先与现有 notification APIs 保持一致，不在本阶段引入 auth-derived tenant scoping。

## 9. Frontend 设计

### 9.1 API client/types

修改：

- `frontend/src/api/types.ts`
- `frontend/src/api/client.ts`

新增：

- `NotificationOutboxListItem`
- `NotificationOutboxDetail`
- `NotificationDeliveryListItem`
- `NotificationOutboxQuery`
- `NotificationDeliveryQuery`
- `listNotificationOutbox(query)`
- `getNotificationOutbox(id)`
- `retryNotificationOutbox(id)`
- `listNotificationDeliveries(query)`

### 9.2 Page

新增：`frontend/src/pages/NotificationOperationsPage.tsx`。

页面结构：

1. Summary cards：
   - pending
   - retryable
   - processing
   - failed
   - stale processing
2. Outbox filters：
   - status select
   - event id input
   - refresh button
3. Outbox table：
   - status tag
   - event summary
   - attempt count
   - next attempt
   - locked by/locked at
   - last error preview
   - delivery counts
   - detail button
   - retry button for `failed` / `retryable`
4. Detail modal or drawer：
   - outbox fields
   - event fields
   - related deliveries table
   - provider response collapsed text
5. Delivery table：
   - can start as detail-only inside outbox detail。
   - Standalone delivery filters can be added if page complexity stays manageable。

### 9.3 Navigation

修改 `frontend/src/App.tsx`：

- 增加 page key，例如 `notification-operations`。
- 导航名称：`通知运维`。
- 渲染 `NotificationOperationsPage`。

## 10. Error handling and safety

1. Invalid query filter returns `AppError::Validation` -> HTTP 400。
2. Missing outbox id returns `AppError::NotFound` -> HTTP 404。
3. Retry non-retryable status returns validation error。
4. Retry does not directly send notification；只改变 outbox row，使 notifier 统一处理。
5. Retry does not reset attempt_count。
6. Processing rows are visible but not retryable in MVP。
7. Provider response may contain provider-side text；frontend default collapsed/truncated display。
8. Webhook query/token redaction remains handled by sender before errors are stored；ops UI does not perform additional secret parsing beyond safe display defaults。

## 11. Testing plan

Backend tests:

1. Core DTO serde round-trip tests。
2. Migration string tests for ops indexes。
3. Outbox list query includes status filters, event join, delivery counts, limit/offset。
4. Retry helper updates only `failed` / `retryable` and clears lock fields。
5. Retry helper rejects `pending` / `processing` / `delivered`。
6. Delivery list query filters by status and channel_type。
7. API route tests:
   - invalid status -> 400。
   - invalid UUID path -> 400。
   - unsupported method -> 405。
   - retry non-JSON body not required for POST retry。

Frontend checks:

1. TypeScript compile via `npm run build --prefix frontend`。
2. Page renders with empty data/loading/error states in existing app structure。
3. Retry mutation invalidates outbox query and shows Toast。

Final verification:

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
cargo check --workspace --manifest-path backend/Cargo.toml
cargo test --workspace --manifest-path backend/Cargo.toml
npm run build --prefix frontend
# docker compose config using a temporary .env copied from .env.example
```

## 12. Follow-up milestones

1. Telegram/Webhook channel configuration UI and test-send。
2. Provider health/failover/rate-limit/circuit breaker。
3. Full operations status dashboard with service heartbeat and alerting。
4. Carefully scoped replay/reset attempt budget operation。
5. Event page links into notification operations detail by event id。
