# Coin Listener External Notification Channels Design

日期：2026-05-18

## 1. 目标

让已经在数据模型和 API 中开放的 `telegram` 与 `webhook` notification channels 从“记录为 skipped”升级为真实外部发送，并为每个 event/rule/channel 组合建立稳定的 channel-level idempotency 记录。

本阶段承接 notification outbox reliability。Outbox 仍然是唯一可靠任务源；本设计只补齐 notifier 对外部 channel 的处理能力，不改变 worker 扫描、事件写入或 outbox claim/retry 基础架构。

## 2. 当前状态依据

- `notification_channels.channel_type` 已允许 `in_app`、`telegram`、`webhook`。
- API 已有 notification channel、notification rule 和 in-app notification 相关路由。
- `notification_channels.config` 是 JSONB，当前没有 Telegram/Webhook 的 typed config validation。
- Notifier 当前只对 `in_app` 创建站内通知；其他 channel 类型会写入 skipped delivery，错误为 `channel type not implemented`。
- `notification_deliveries` 当前只记录基础字段：tenant/event/rule/channel/status/attempt_count/last_error/sent_at/created_at。
- Notifier crate 当前没有可复用 HTTP client；Telegram/Webhook 发送需要新增外部 HTTP 发送能力。

## 3. 范围

### 3.1 包含

1. Telegram 真实发送：
   - 解析 Telegram channel config。
   - 调用 Telegram Bot API `sendMessage`。
   - 成功后记录 provider message id、HTTP status 和截断响应。
2. Webhook 真实发送：
   - 解析 Webhook channel config。
   - 向配置 URL 发送 JSON payload。
   - 在 header 和 payload 中携带稳定 idempotency key。
   - 成功后记录 HTTP status 和截断响应。
3. Channel-level idempotency：
   - 对每个 `(tenant_id, event_id, rule_id, channel_id)` 生成稳定 idempotency key。
   - 在 `notification_deliveries` 中保存 idempotency key。
   - 已记录为 `sent` 或 `skipped` 的同一 key 不重复发送。
   - `failed` 或 `processing` 的同一 key 可被后续 outbox retry 更新并重试。
4. Delivery metadata 扩展：
   - 保存外部 channel 类型、idempotency key、provider message id、provider HTTP status 和 provider response 摘要。
   - 对 token、secret、带 query 的 webhook URL 做脱敏后再进入日志或错误文本。
5. 错误分类：
   - 配置错误和 4xx 永久失败记录 failed delivery，不触发 outbox retry。
   - 429、408、5xx、网络错误和 timeout 记录 failed attempt，并触发 outbox retry。
   - delivery metadata 写入失败必须触发 outbox retry。
6. 测试与验证：
   - Config parser 测试。
   - Idempotency key 测试。
   - Telegram/Webhook sender 结果分类测试。
   - Notifier channel decision 和 outbox retry 语义测试。
   - Backend workspace、frontend build 和 docker compose config 不回归。

### 3.2 不包含

- Email、Discord、企业微信或其他外部 channel。
- 前端 retry/dead-letter 运维页面。
- 前端复杂 channel config 表单改造；本阶段以 API/后端能力为准。
- 通用 secret manager；本阶段通过环境变量名引用 secret。
- 外部 provider 的严格 exactly-once guarantee。
- 历史 skipped telegram/webhook delivery 自动重放。
- Notification 模板系统。
- WebSocket 实时推送。

## 4. 推荐方案

采用共享 external notification sender 基础层，然后分别实现 Telegram 和 Webhook adapter。

Notifier 的主流程保持不变：outbox item 被 claim 后加载 event、匹配 rules、解析 channels。`in_app` 继续使用现有本地写入路径；`telegram` 和 `webhook` 进入 external sender。External sender 在发送前创建或复用带 idempotency key 的 delivery row，发送成功后更新该 row；可重试失败返回错误给 outbox dispatcher；永久失败只记录 failed delivery，不阻塞同一 outbox item 的其他 channel。

这样可以避免 Telegram 与 Webhook 各自实现一套 delivery/idempotency/retry 逻辑，也为后续 retry/dead-letter 运维页提供统一数据来源。

## 5. 数据模型

新增 migration：`backend/crates/storage/migrations/0008_external_notification_deliveries.sql`。

扩展 `notification_deliveries`：

```sql
ALTER TABLE notification_deliveries
    ADD COLUMN IF NOT EXISTS channel_type TEXT,
    ADD COLUMN IF NOT EXISTS idempotency_key TEXT,
    ADD COLUMN IF NOT EXISTS provider_message_id TEXT,
    ADD COLUMN IF NOT EXISTS provider_status_code INTEGER,
    ADD COLUMN IF NOT EXISTS provider_response TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_notification_deliveries_idempotency
    ON notification_deliveries(event_id, rule_id, channel_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
```

字段语义：

| 字段 | 语义 |
|---|---|
| `channel_type` | 写入 delivery 时的 channel type 快照，便于 channel 被删除后仍能审计 |
| `idempotency_key` | event/rule/channel 级别稳定键 |
| `provider_message_id` | Telegram message id 或未来 provider 返回的消息标识 |
| `provider_status_code` | 外部 HTTP status；网络错误为空 |
| `provider_response` | 截断后的 provider response 或错误摘要 |

Delivery status 继续使用 text，并扩展语义：

| status | 语义 |
|---|---|
| `processing` | 已创建外部发送尝试，尚未得到最终结果 |
| `sent` | 外部发送成功 |
| `skipped` | 业务性跳过，例如 inactive/missing channel |
| `failed` | 本次外部发送失败；是否重试由 outbox 错误分类决定 |

## 6. Idempotency 设计

Idempotency key 使用稳定字符串：

```text
notification:v1:{tenant_id}:{event_id}:{rule_id}:{channel_id}
```

行为规则：

1. 同一 event/rule/channel 每次重试生成完全相同的 key。
2. 不同 channel 或不同 rule 生成不同 key。
3. 发送前调用 repository helper 创建或复用 delivery：
   - 如果已有 `sent` 或 `skipped` delivery，直接视为该 channel 已完成，不再调用外部 provider。
   - 如果已有 `processing` 或 `failed` delivery，更新 attempt_count、last_error/provider fields 后继续本次发送。
   - 如果不存在 delivery，插入 `processing` delivery。
4. Webhook 请求必须携带：
   - header `X-Coin-Listener-Idempotency-Key`
   - payload 字段 `idempotency_key`
5. Telegram Bot API 没有 provider-side idempotency。系统只能保证成功写入 `sent` delivery 后不重复发送；如果进程在 Telegram 返回成功后、DB 更新 sent 前崩溃，后续重试可能造成 Telegram 侧重复消息。该限制必须在代码注释和测试命名中明确。

## 7. Channel config

### 7.1 Telegram config

`notification_channels.config`：

```json
{
  "bot_token_env": "TELEGRAM_BOT_TOKEN",
  "chat_id": "123456789"
}
```

字段规则：

- `bot_token_env` 必填，值是环境变量名，不是 token 明文。
- `chat_id` 必填，按字符串保存。
- Notifier 从进程环境读取 bot token。
- 日志、delivery last_error 和 provider_response 不得包含 token。

Telegram 请求：

```text
POST https://api.telegram.org/bot{token}/sendMessage
```

请求 body：

```json
{
  "chat_id": "123456789",
  "text": "<rendered notification text>"
}
```

### 7.2 Webhook config

`notification_channels.config`：

```json
{
  "url": "https://example.com/coin-listener-hook",
  "secret_env": "COIN_LISTENER_WEBHOOK_SECRET",
  "timeout_ms": 5000
}
```

字段规则：

- `url` 必填，只允许 `http` 或 `https`。
- `secret_env` 可选，值是环境变量名。
- `timeout_ms` 可选，默认 5000，允许范围 1000 到 30000。
- 日志和错误文本中的 URL 必须去掉 query string。

Webhook payload：

```json
{
  "idempotency_key": "notification:v1:<tenant>:<event>:<rule>:<channel>",
  "event_id": "<uuid>",
  "tenant_id": "<uuid>",
  "chain_id": "<uuid>",
  "address_id": "<uuid>",
  "asset_id": "<uuid>",
  "event_type": "transfer",
  "direction": "inbound",
  "is_transfer": true,
  "tx_hash": "<hash-or-null>",
  "block_number": 123,
  "from_address": "<address-or-null>",
  "to_address": "<address-or-null>",
  "amount_raw": "1000000000000000000",
  "amount_decimal": "1.0",
  "detected_at": "<rfc3339>"
}
```

Webhook headers：

```text
Content-Type: application/json
X-Coin-Listener-Event-Id: <event_id>
X-Coin-Listener-Idempotency-Key: <idempotency_key>
X-Coin-Listener-Signature: <hmac-sha256-hex>   # only when secret_env is configured
```

Signature input is the raw JSON request body bytes. The signature value is lowercase hex HMAC-SHA256 using the secret loaded from `secret_env`.

## 8. Notifier flow

For each matched rule/channel:

1. Resolve channel as current notifier does today.
2. Build delivery plan:
   - `in_app` -> existing in-app path。
   - `telegram` -> external Telegram path。
   - `webhook` -> external Webhook path。
   - unknown type -> skipped delivery。
3. For external path:
   - Parse typed config。
   - Build idempotency key。
   - Begin or reuse external delivery row。
   - If existing row is already `sent` or `skipped`, return success without provider call。
   - Render notification content。
   - Send through shared HTTP client。
   - Update delivery metadata to `sent` on success。
   - Update delivery metadata to `failed` on permanent failure and continue。
   - Update delivery metadata to `failed` on transient failure and return `Err` so outbox retries。

Outbox item semantics：

- 所有 matched channels 都成功、skipped 或永久 failed 后，outbox 标记 `delivered`。
- 任一 channel 出现 transient failure 或 DB metadata write failure，outbox 标记 `retryable` 或最终 `failed`。
- 已经 `sent` 的 channel 在下一次 retry 中通过 idempotency key 跳过，不重复调用 provider。

## 9. HTTP client

Notifier 初始化一个可复用 `reqwest::Client`，并注入 external sender。禁止每次发送创建新 client。

默认行为：

- Webhook timeout 使用 channel config 的 `timeout_ms`。
- Telegram timeout 使用 5000ms。
- Redirect 使用 reqwest 默认策略。
- Response body 最多保存前 2048 bytes。
- Runtime error 中不得包含 Telegram token、webhook secret、带 query string 的 URL。

## 10. 错误分类

| 来源 | 条件 | Delivery | Outbox |
|---|---|---|---|
| Config | 缺必填字段、URL scheme 非法、env var 缺失 | `failed` | `delivered` |
| Telegram | 2xx 且 body 可解析 | `sent` | `delivered` |
| Telegram | 400、401、403、404 | `failed` | `delivered` |
| Telegram | 408、429、5xx、timeout、network error | `failed` | `retryable` 或最终 `failed` |
| Webhook | 2xx | `sent` | `delivered` |
| Webhook | 400、401、403、404、410 | `failed` | `delivered` |
| Webhook | 408、429、5xx、timeout、network error | `failed` | `retryable` 或最终 `failed` |
| DB | delivery begin/update 失败 | 不保证写入 | `retryable` 或最终 `failed` |

永久失败不重试，因为重复发送不会修复配置或认证问题。Transient failure 交给既有 notification outbox retry policy。

## 11. Repository 设计

新增或扩展 notification repository helpers：

```rust
pub async fn begin_external_notification_delivery(
    pool: &PgPool,
    tenant_id: Uuid,
    event_id: Uuid,
    rule_id: Uuid,
    channel_id: Uuid,
    channel_type: &str,
    idempotency_key: &str,
    attempt_count: i32,
) -> AppResult<ExternalDeliveryStart>;

pub async fn mark_external_notification_delivery_sent(
    pool: &PgPool,
    delivery_id: Uuid,
    sent_at: DateTime<Utc>,
    provider_message_id: Option<&str>,
    provider_status_code: Option<i32>,
    provider_response: Option<&str>,
) -> AppResult<()>;

pub async fn mark_external_notification_delivery_failed(
    pool: &PgPool,
    delivery_id: Uuid,
    last_error: &str,
    provider_status_code: Option<i32>,
    provider_response: Option<&str>,
) -> AppResult<()>;
```

`ExternalDeliveryStart`：

```rust
pub enum ExternalDeliveryStart {
    AlreadyComplete { delivery_id: Uuid },
    ReadyToSend { delivery_id: Uuid },
}
```

`begin_external_notification_delivery` must use the idempotency unique index. It must not create a second row for the same idempotency key.

## 12. Testing plan

Backend tests：

1. `telegram_channel_config_requires_token_env_and_chat_id`。
2. `webhook_channel_config_requires_http_url`。
3. `webhook_channel_config_defaults_timeout`。
4. `notification_idempotency_key_is_stable_for_same_rule_channel`。
5. `notification_idempotency_key_changes_for_different_channel`。
6. `external_delivery_start_skips_already_sent_delivery`。
7. `external_delivery_start_reuses_failed_delivery_for_retry`。
8. `telegram_sender_classifies_success_as_sent`。
9. `telegram_sender_classifies_rate_limit_as_retryable`。
10. `webhook_sender_includes_idempotency_headers`。
11. `webhook_sender_signs_payload_when_secret_env_is_set`。
12. `webhook_sender_redacts_query_string_from_errors`。
13. `notifier_treats_telegram_and_webhook_as_sendable_channels`。
14. `transient_external_send_error_keeps_outbox_retryable`。
15. `permanent_external_send_error_records_failed_delivery_without_outbox_retry`。

Verification commands：

```bash
cargo fmt --all --check --manifest-path backend/Cargo.toml
cargo check --workspace --manifest-path backend/Cargo.toml
cargo test --workspace --manifest-path backend/Cargo.toml
npm run build --prefix frontend
if [ -f .env ]; then docker compose -f docker-compose.yml config >/tmp/coin-listener-compose-config.txt; else touch .env && rc=0; docker compose -f docker-compose.yml config >/tmp/coin-listener-compose-config.txt || rc=$?; rm .env; exit $rc; fi
```

The final compose verification preserves any existing `.env`; when `.env` is absent, it creates a temporary empty file and removes it after the command.

## 13. Rollout

1. Add migration and model fields first; existing in-app behavior remains unchanged.
2. Add config parsing and validation while keeping telegram/webhook skipped until sender tests exist.
3. Add external sender with HTTP mocked tests.
4. Switch notifier channel decision for telegram/webhook from skipped to sendable.
5. Run full verification.

Rollback path：

- Reverting notifier channel decision to skipped stops external sends while preserving delivery metadata columns.
- The migration is additive and does not break existing in-app notification reads.

## 14. Follow-up milestones

After this milestone, the next priorities are：

1. Notification retry/dead-letter frontend operations page using outbox and delivery metadata。
2. Secret storage beyond process environment variables。
3. Webhook delivery replay endpoint for manually retriggering failed permanent deliveries。
4. Provider health/rate-limit/circuit-breaker work for chain RPC providers。
