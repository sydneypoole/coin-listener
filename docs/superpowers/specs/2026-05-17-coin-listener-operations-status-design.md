# Coin Listener Operations Status Design

日期：2026-05-17

## 1. 目标

新增轻量运维状态中心，让当前多进程 Coin Listener 可以从管理后台查看 Redis 队列积压、扫描状态、事件生成状态、通知处理状态和 Provider 配置状态。

本阶段优先使用现有 PostgreSQL 与 Redis 数据做快照式状态聚合，不引入 Prometheus、服务心跳表、真实 RPC ping 或 WebSocket 实时状态推送。

## 2. 范围

### 2.1 包含

- 新增共享状态模型：`SystemStatus` 及其子结构。
- 新增只读 API：`GET /api/system/status`。
- 增加 Redis queue depth 查询能力：scan queue 与 notify queue。
- 增加 PostgreSQL 聚合查询：扫描地址、事件、通知、Provider 配置状态。
- 前端新增 `SystemStatusPage`，使用 Semi Design 展示状态卡片和 Provider 状态表。
- 前端使用 TanStack Query 每 10 秒轮询系统状态。
- 后端测试覆盖状态模型序列化、队列 depth helper、聚合 SQL 关键约束、API route 暴露。
- 前端构建验证。

### 2.2 不包含

- 真实服务心跳表。
- Prometheus / Grafana / OpenTelemetry。
- 真实 RPC Provider 健康探测。
- WebSocket 实时状态推送。
- 告警规则、邮件、Telegram 运维告警。
- 历史指标趋势图。
- 多租户级运维权限模型。

## 3. 架构

```text
frontend SystemStatusPage
  -> GET /api/system/status
  -> api-server aggregates status
      -> Redis LLEN scan queue / notify queue
      -> PostgreSQL watched_addresses scan summary
      -> PostgreSQL address_events 24h summary
      -> PostgreSQL notification_deliveries / in_app_notifications summary
      -> PostgreSQL providers grouped by chain
```

### 3.1 设计原则

- API 只读，不改变 scheduler、worker、notifier 行为。
- Redis queue depth 查询失败不让整个接口失败；对应 depth 返回 `null`，错误放入 `queue_errors`。
- PostgreSQL 聚合查询失败返回 500，沿用现有 `ApiError`。
- 无事件、无通知、无 provider 时返回 0 或空数组。
- 先提供可运行的运维快照，后续再扩展服务心跳与指标时间序列。

## 4. API 设计

### 4.1 Endpoint

```http
GET /api/system/status
```

该接口是正式状态接口，不受 `ENABLE_DEV_ROUTES` 控制。

### 4.2 Response

```json
{
  "generated_at": "2026-05-17T10:00:00Z",
  "queues": {
    "scan_queue_key": "scan:address:queue",
    "scan_queue_depth": 3,
    "notify_queue_key": "notify:event:queue",
    "notify_queue_depth": 1,
    "queue_errors": []
  },
  "scans": {
    "active_addresses": 12,
    "due_addresses": 2,
    "overdue_addresses": 1,
    "last_scanned_at": "2026-05-17T09:58:00Z"
  },
  "events": {
    "last_24h_total": 40,
    "last_24h_transfers": 35,
    "last_24h_non_transfers": 5
  },
  "notifications": {
    "last_24h_sent": 20,
    "last_24h_skipped": 2,
    "last_24h_failed": 1,
    "unread_in_app": 4
  },
  "providers": {
    "active": 4,
    "inactive": 1,
    "by_chain": [
      {
        "chain_id": "00000000-0000-0000-0000-000000000001",
        "chain_name": "Ethereum",
        "active": 2,
        "inactive": 0
      }
    ],
    "items": [
      {
        "id": "00000000-0000-0000-0000-000000000010",
        "chain_id": "00000000-0000-0000-0000-000000000001",
        "chain_name": "Ethereum",
        "provider_type": "rpc",
        "name": "Ethereum RPC",
        "base_url": "https://example.invalid",
        "priority": 1,
        "qps_limit": 10,
        "timeout_ms": 5000,
        "status": "active"
      }
    ]
  }
}
```

### 4.3 Rust Models

新增到 `backend/crates/core/src/models.rs`：

```rust
pub struct SystemStatus {
    pub generated_at: DateTime<Utc>,
    pub queues: QueueStatus,
    pub scans: ScanStatus,
    pub events: EventStatus,
    pub notifications: NotificationStatus,
    pub providers: ProviderStatus,
}

pub struct QueueStatus {
    pub scan_queue_key: String,
    pub scan_queue_depth: Option<i64>,
    pub notify_queue_key: String,
    pub notify_queue_depth: Option<i64>,
    pub queue_errors: Vec<String>,
}

pub struct ScanStatus {
    pub active_addresses: i64,
    pub due_addresses: i64,
    pub overdue_addresses: i64,
    pub last_scanned_at: Option<DateTime<Utc>>,
}

pub struct EventStatus {
    pub last_24h_total: i64,
    pub last_24h_transfers: i64,
    pub last_24h_non_transfers: i64,
}

pub struct NotificationStatus {
    pub last_24h_sent: i64,
    pub last_24h_skipped: i64,
    pub last_24h_failed: i64,
    pub unread_in_app: i64,
}

pub struct ProviderStatus {
    pub active: i64,
    pub inactive: i64,
    pub by_chain: Vec<ProviderChainStatus>,
    pub items: Vec<ProviderStatusItem>,
}
```

## 5. 后端实现边界

### 5.1 Redis Queue Depth

在现有 queue wrapper 增加方法：

```rust
pub async fn depth(&self, connection: &mut MultiplexedConnection) -> AppResult<i64>
```

适用文件：

- `backend/crates/storage/src/scan_queue.rs`
- `backend/crates/storage/src/notify_queue.rs`

实现使用 Redis `LLEN <queue_key>`。

### 5.2 PostgreSQL 聚合查询

在 storage 层新增只读聚合函数：

- `system_scan_status(pool)`：统计 active、due、overdue、last scanned。
- `system_event_status(pool)`：统计最近 24h event 总数、transfer 数、non-transfer 数。
- `system_notification_status(pool)`：统计最近 24h delivery sent/skipped/failed 和 unread in-app 数。
- `system_provider_status(pool)`：统计 provider active/inactive、按 chain 聚合、provider 明细列表。

建议放在新文件：

```text
backend/crates/storage/src/system_status.rs
```

并在 `backend/crates/storage/src/lib.rs` 导出。

### 5.3 API State

当前 `ApiState` 只有 PostgreSQL。为了查询 Redis queue depth，扩展为：

```rust
pub struct ApiState {
    pub postgres: PgPool,
    pub redis: Option<redis::Client>,
    pub scan_queue_key: String,
    pub notify_queue_key: String,
    pub enable_dev_routes: bool,
}
```

API server main 初始化 Redis client 后注入 state。测试可用 `redis: None`，此时 queue depth 为 `null` 且 `queue_errors` 包含 Redis 未配置说明。

## 6. 前端设计

### 6.1 页面

新增：

```text
frontend/src/pages/SystemStatusPage.tsx
```

导航新增：`系统状态`。

### 6.2 页面布局

- 顶部状态卡片区：
  - Scan Queue 积压。
  - Notify Queue 积压。
  - Active 地址数。
  - Due 地址数。
  - 24h 事件数。
  - 24h 通知失败数。
- 中部摘要区：
  - 24h transfer / non-transfer。
  - 24h sent / skipped / failed。
  - unread in-app count。
- Provider 表格：
  - chain。
  - name。
  - type。
  - status。
  - priority。
  - qps limit。
  - timeout。
- 错误提示：
  - `queue_errors` 非空时显示 Semi `Banner`。
  - API 请求失败时显示 Semi `Banner type="danger"`。

### 6.3 Query

```ts
useQuery({
  queryKey: ['system-status'],
  queryFn: getSystemStatus,
  refetchInterval: 10_000,
});
```

## 7. 错误处理

| 场景 | 行为 |
|---|---|
| Redis queue depth 查询失败 | 对应 depth 为 `null`，错误文本加入 `queue_errors` |
| Redis client 未注入 | 两个 depth 均为 `null`，`queue_errors` 标记 Redis unavailable |
| PostgreSQL 聚合失败 | API 返回 500 |
| 没有 watched addresses | scan 统计返回 0，`last_scanned_at = null` |
| 没有 events | event 统计返回 0 |
| 没有 notification deliveries | notification delivery 统计返回 0 |
| 没有 providers | provider active/inactive 为 0，数组为空 |
| 前端请求失败 | 页面显示错误 Banner，保留页面结构 |

## 8. 测试策略

### 8.1 后端单元测试

- `SystemStatus` JSON round-trip。
- `QueueStatus` 支持 `null` depth 和 `queue_errors`。
- queue depth helper 使用稳定 key 构造或命令封装测试。
- scan 聚合 SQL 包含：
  - `status = 'active'`
  - `next_scan_at <= NOW()`
- event 聚合 SQL 包含：
  - `created_at >= NOW() - INTERVAL '24 hours'`
  - `is_transfer = TRUE`
- notification 聚合 SQL 包含：
  - `status = 'sent'`
  - `status = 'skipped'`
  - `status = 'failed'`
- provider 聚合 SQL 连接 `chains`，返回 chain name。

### 8.2 API 测试

- `GET /api/system/status` route 存在。
- 不支持方法返回 `METHOD_NOT_ALLOWED`。
- 测试 state 可无 Redis client，route 仍可构建。

### 8.3 前端验证

- TypeScript 类型覆盖 `SystemStatus`。
- `SystemStatusPage` 空数据和 `null` queue depth 不崩溃。
- `npm run build --prefix frontend` 通过。

### 8.4 最终验证

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
cargo check --workspace --manifest-path backend/Cargo.toml
cargo test --workspace --manifest-path backend/Cargo.toml
npm run build --prefix frontend
docker compose -f docker-compose.yml config
```

## 9. 验收标准

1. 前端可以打开“系统状态”页面。
2. 页面能显示 scan queue 与 notify queue 积压；Redis 不可用时显示 `null` 和错误提示。
3. 页面能显示 active/due/overdue 地址统计。
4. 页面能显示最近 24h event transfer/non-transfer 统计。
5. 页面能显示最近 24h notification delivery sent/skipped/failed 统计。
6. 页面能显示 unread in-app notification 数量。
7. 页面能展示 provider 明细和按 chain 聚合状态。
8. `GET /api/system/status` 不依赖 dev route 开关。
9. 后端格式检查、编译和测试通过。
10. 前端构建通过。
11. Docker Compose 配置仍可解析。

## 10. 后续扩展

- 增加 scheduler / worker / notifier 服务心跳表。
- 增加错误事件表和错误详情页。
- 增加 provider RPC ping 和延迟统计。
- 增加 Prometheus `/metrics`。
- 增加队列积压阈值告警。
- 增加 WebSocket 推送系统状态变化。
