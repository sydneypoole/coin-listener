# Coin Listener Realtime In-App Notifications Design

日期：2026-05-19

## 1. 目标

补齐 Coin Listener MVP 中“在线用户能通过 WebSocket 收到事件”的实时通知能力。后端在站内通知创建后向同租户在线连接广播，前端在已有登录会话内建立 WebSocket 连接，收到消息后刷新站内通知和事件相关查询，并用 Semi Notification 提示用户。

## 2. 当前依据

- 总体设计的 MVP 验收标准包含“在线用户能通过 WebSocket 收到事件”。
- 通知系统骨架、外部通知渠道、通知运维和认证会话加固均把 WebSocket 实时推送列为后续范围。
- 当前 `axum` 已启用 `ws` feature，API 已有 JWT bearer 认证和租户上下文，具备 WebSocket 鉴权基础。
- 当前 notifier 会在 `in_app` channel 命中时创建 `in_app_notifications`，但没有实时广播路径。
- 当前前端有站内通知页和会话存储模块，但没有 WebSocket 客户端或未读实时 badge。

## 3. 推荐方案

采用 API 进程内 WebSocket hub + PostgreSQL `LISTEN/NOTIFY` 的轻量桥接方案。

每个 API 或 all-in-one 进程维护本进程 WebSocket 连接表，连接按 `tenant_id` 分组。站内通知写入成功后，storage/notifier 通过同一数据库事务之后发送 PostgreSQL `NOTIFY in_app_notifications` payload。每个 API/all-in-one 进程启动一个 listener 后台任务，收到 payload 后只向本进程中匹配 `tenant_id` 的连接广播。

该方案不要求 notifier 直接持有 API 内存 hub，也支持多 API 进程和 all-in-one 模式。相比 Redis Pub/Sub，它少引入一个运行时通道；相比前端轮询，它真正满足实时通知验收。

## 4. 范围

### 4.1 包含

1. 后端 WebSocket endpoint：`GET /api/realtime/notifications`。
2. WebSocket 鉴权：使用现有 JWT token，握手阶段验证 token、active user 和 active tenant membership。
3. WebSocket 消息模型：服务端发送 `in_app_notification.created` 和 `ping`，客户端无需发送业务消息。
4. 后端 hub：按 tenant 维护连接 sender，广播失败时清理断开连接。
5. PostgreSQL notification bridge：站内通知创建成功后发布最小 payload，API/all-in-one listener 订阅并广播。
6. 前端实时客户端：登录后连接，登出或 `401` 清理后断开。
7. 前端 UI：收到站内通知后刷新 `in-app-notifications`、`events`、`system-status` 查询，并显示 Semi Notification；侧边栏站内通知增加未读 badge。
8. 测试：token 提取、payload 序列化、tenant 分组广播、连接生命周期、前端 URL/token/reconnect 行为。

### 4.2 不包含

- WebSocket 推送系统状态或 provider 状态。
- 客户端上行订阅过滤 DSL。
- Redis Pub/Sub、Kafka、NATS 或独立 gateway。
- 消息持久化队列；离线用户继续通过现有站内通知列表补偿。
- 浏览器多 tab 去重。
- Role-based 权限扩展。

## 5. 后端设计

### 5.1 WebSocket 鉴权

WebSocket 入口接受查询参数 `token`：

```text
GET /api/realtime/notifications?token=<jwt>
```

握手处理复用现有 auth primitives：

1. `validate_token` 解析 claims。
2. `subject_uuid` 和 `tenant_uuid` 校验 UUID。
3. `active_user` 确认 inactive/deleted user 仍是 `401`。
4. `active_tenant_membership` 确认缺失或 inactive membership 是 `403`。
5. 成功后创建 `RealtimeClientContext { user_id, tenant_id, email }`。

查询参数 token 避免浏览器 WebSocket API 无法设置 `Authorization` header 的限制。服务端日志不得记录 token；错误响应只返回标准状态码。

### 5.2 Message contract

后端到前端消息使用 tagged JSON：

```json
{
  "type": "in_app_notification.created",
  "payload": {
    "id": "...",
    "tenant_id": "...",
    "event_id": "...",
    "delivery_id": "...",
    "title": "transfer in",
    "body": "address: ...; asset: ...; amount: ...; tx: ...",
    "read_at": null,
    "created_at": "2026-05-19T10:00:00Z"
  }
}
```

Heartbeat message：

```json
{ "type": "ping", "payload": { "sent_at": "2026-05-19T10:00:00Z" } }
```

未知消息类型必须被前端忽略。

### 5.3 Hub

新增 `api-server/src/realtime.rs`，职责：

- 定义 `RealtimeHub`。
- `subscribe(tenant_id) -> receiver`。
- `broadcast(notification)` 按 `tenant_id` 分发。
- 连接断开时释放 receiver。
- 限制单连接 channel buffer，满时丢弃旧消息并保留连接，避免慢客户端阻塞广播。

`ApiState` 新增 `realtime: RealtimeHub`。`build_router` 注册 WebSocket route 到受保护 router 之外，但 handler 内显式做 token 鉴权，因为 WebSocket 握手不能使用现有 bearer middleware。

### 5.4 PostgreSQL bridge

新增 `storage::notifications::publish_in_app_notification_created(pool, notification)`，发送 `NOTIFY coin_listener_in_app_notifications, '<json>'`。

`create_sent_in_app_delivery` 在 transaction commit 成功后发布 notification payload。发布失败返回数据库错误，让 outbox retry 覆盖实时广播失败；已写入的站内通知保证用户仍可在页面列表看到。

新增 `api-server::realtime::run_realtime_notification_listener(pool, hub, shutdown)`：

- 连接 PostgreSQL listener。
- `LISTEN coin_listener_in_app_notifications`。
- 解析 payload 为 `InAppNotification`。
- 调用 `hub.broadcast(notification)`。
- listener 断开时记录 warning 并短间隔重连，直到 shutdown。

API server 和 all-in-one 启动时都运行 listener。纯 notifier 进程只负责写通知并发布 database notification。

## 6. 前端设计

### 6.1 WebSocket client boundary

新增 `frontend/src/realtime/notifications.ts`，职责：

- 从 `LoginResponse.token` 构建 WebSocket URL。
- `http/https` 自动映射为 `ws/wss`。
- 建立连接，解析 JSON，忽略未知/非法消息。
- 暴露 `connectRealtimeNotifications(session, handlers) -> () => void`，返回 cleanup。
- 断线后指数退避重连；登出、401 handler 或 session generation 变化时停止重连。

### 6.2 UI behavior

`App.tsx` 登录后启动实时连接：

- 收到 `in_app_notification.created`：
  - `Notification.info({ title, content })`。
  - invalidate `['in-app-notifications']`。
  - invalidate `['events']`。
  - invalidate `['system-status']`。
  - 未读 badge +1。
- 用户进入站内通知页或标记已读成功后，未读 badge 从 query 数据或系统状态重新同步。
- 登出或 unauthorized reset 时断开连接并清空 badge。

## 7. 错误处理

- WebSocket token 缺失、过期、篡改：握手返回 `401`。
- active tenant membership 缺失：握手返回 `403`。
- listener payload JSON 解析失败：记录 warning，丢弃该 payload。
- 单个客户端发送失败：清理该连接，不影响其他连接。
- 前端连接失败：不阻塞页面使用；延迟重连。
- 实时消息丢失：站内通知列表仍是权威数据源。

## 8. 测试策略

后端：

- `realtime` unit tests 覆盖 WebSocket token query 提取、消息 JSON shape、tenant 分组广播和断开清理。
- `routes` tests 覆盖 `/api/realtime/notifications` 无 token 返回 `401`，login/public route 不受影响。
- `storage` tests 覆盖 NOTIFY channel 名称和 payload 字段。
- `cargo fmt --all --check` 与 `cargo test --workspace`。

前端：

- `notifications.ts` unit-level tests 可通过纯函数覆盖 WebSocket URL 构造、message parser、reconnect delay clamping。
- `npm run build --prefix frontend` 验证 TypeScript 与生产构建。
- 不引入端到端浏览器测试。

## 9. 验收标准

1. 登录用户能建立 `/api/realtime/notifications` WebSocket 连接。
2. 缺失/无效 token 连接失败为 `401`。
3. inactive/deleted user token 连接失败为 `401`。
4. 缺失/inactive tenant membership 连接失败为 `403`。
5. 创建站内通知后，同租户在线用户收到 `in_app_notification.created`。
6. 不同租户连接不会收到该通知。
7. 前端收到消息后显示 Semi Notification 并刷新站内通知数据。
8. 登出或 session 被清理后 WebSocket 断开且不再重连。
9. API server 与 all-in-one 模式都启动 realtime listener。
10. 后端测试和前端构建通过。
