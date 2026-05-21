# Telegram Channel Binding Design

**Goal:** Replace manual Telegram Chat ID entry with a verification-code binding flow for private chats and group chats.

## Approved decisions

- Support both Telegram webhook and `getUpdates` polling.
- Reuse one shared Telegram update processor for both ingestion paths.
- Generate both a private-chat deep-link token and a short group-friendly code.
- Use 15 minutes as the binding request expiry window.
- Binding resolves `chat_id` and display alias first; admins still review and save the notification channel manually.
- Send a Telegram confirmation message after a chat is bound.

## Current context

The project already has Telegram bot management and notification channel storage:

- `telegram_bots` stores bot tokens server-side and only exposes token previews to the frontend.
- Telegram bot verification uses Telegram `getMe`.
- Telegram notification channels currently store config as `telegram_bot_id`, `chat_id`, `chat_alias`, and `message_template`.
- Notification delivery already reads this config and sends Telegram messages through `sendMessage`.
- `NotificationChannelsPage` and the notification rule quick-create flow still ask admins to type Chat ID manually.

This design keeps the existing delivery config shape so current send logic remains compatible.

## Architecture

Add a Telegram binding module with three responsibilities:

1. Create and track binding requests.
2. Process Telegram updates from webhook or polling.
3. Resolve a binding request into trusted Telegram chat metadata.

A binding request stores:

- `id`
- `tenant_id`
- `telegram_bot_id`
- `bind_token`, used by private chat `/start bind_xxx`
- `short_code`, used by group messages such as `@bot CL-7K2P9Q`
- `status`: `pending`, `bound`, `expired`, `cancelled`
- `chat_id`, set only after Telegram confirms it through an update
- `chat_type`
- `chat_title`
- `chat_username`
- `expires_at`
- `bound_at`
- `created_at`, `updated_at`

Webhook and polling both call the same processor:

```text
Telegram update
  -> process_telegram_update(bot_id, update)
  -> extract bind_token or short_code
  -> find pending non-expired binding request
  -> save chat metadata and mark bound
  -> send confirmation message
```

The processor must be idempotent because the same update can arrive through webhook and polling.

## Backend APIs

Protected admin APIs:

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/api/telegram-bindings` | Create a pending binding request for a selected Telegram bot. |
| `GET` | `/api/telegram-bindings/:id` | Fetch binding status for frontend polling. |
| `POST` | `/api/telegram-bindings/:id/cancel` | Cancel a pending binding request. |

Public Telegram ingestion API:

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/api/telegram/webhook/:bot_id` | Receive Telegram webhook updates for a bot. |

The create response returns the data needed by the frontend:

```json
{
  "id": "...",
  "telegram_bot_id": "...",
  "status": "pending",
  "bind_token": "bind_xxx",
  "short_code": "CL-7K2P9Q",
  "deep_link_url": "https://t.me/<bot_username>?start=bind_xxx",
  "expires_at": "..."
}
```

The status response returns bound chat metadata after success:

```json
{
  "id": "...",
  "status": "bound",
  "chat_id": "-1001234567890",
  "chat_type": "supergroup",
  "chat_title": "Ops Alerts",
  "chat_username": null,
  "bound_at": "..."
}
```

## Polling

Add a polling path that periodically calls Telegram `getUpdates` for active verified bots. It should reuse the same update processor as webhook.

Polling needs persistent offset tracking per bot so old updates are not replayed forever. Store `last_update_id` in a dedicated `telegram_bot_update_offsets` table keyed by `tenant_id` and `telegram_bot_id`. The implementation must avoid concurrent pollers processing the same bot at the same time.

Webhook support and polling support can coexist. If a deployment configures webhook, polling can still be used as a fallback, but duplicate updates must not create duplicate bindings.

## Frontend flow

Replace manual Chat ID entry in Telegram notification channel forms with a binding panel.

For `NotificationChannelsPage`:

1. Admin selects `TG机器人`.
2. Admin clicks `生成绑定码`.
3. The panel shows:
   - private chat: deep link button and `/start bind_xxx` copy text;
   - group chat: instructions to add or mention the bot and send `CL-7K2P9Q`.
4. The frontend polls `/api/telegram-bindings/:id` every two seconds while status is `pending`.
5. When status becomes `bound`, the panel shows the resolved chat destination and writes `chat_id` and `chat_alias` into the form state.
6. Admin clicks `保存` to create or update the notification channel.

For `NotificationRulesPage` quick-create channel flow:

- Reuse the same binding panel and client API functions.
- Do not keep a separate manual Chat ID implementation.

The notification channel config saved by the frontend remains:

```json
{
  "telegram_bot_id": "...",
  "chat_id": "...",
  "chat_alias": "...",
  "message_template": "..."
}
```

## Edge cases and validation

- A binding request expires after 15 minutes.
- Expired or cancelled requests cannot be bound.
- A request can only transition from `pending` to `bound` once.
- Creating a binding request requires an active, verified Telegram bot.
- Short codes must be unique among pending requests for the tenant.
- `chat_id` must only come from Telegram updates, not manual user input.
- Group messages should bind if they contain the short code, even if the message does not include an explicit bot mention.
- Private messages should bind using `/start bind_xxx` and can also accept the short code as fallback.
- Reprocessing the same update must be safe.
- If sending the confirmation message fails after binding, the binding should remain bound and store/report the confirmation error separately rather than rolling back the trusted chat metadata.

## Security

- Bot tokens remain server-side only.
- Binding requests are tenant-scoped.
- Admin APIs require existing auth middleware.
- The webhook route should validate an optional Telegram secret token header when configured.
- The webhook path includes a bot id but must not expose the bot token.
- Frontend should not allow manual Chat ID edits for normal Telegram channel creation.

## Testing plan

Backend tests should cover:

- binding request creation validation;
- short code and deep-link token generation;
- private `/start bind_xxx` update binding;
- group short-code update binding;
- expired request rejection;
- cancelled request rejection;
- idempotent duplicate update processing;
- notification channel delivery compatibility with the existing config shape.

Frontend regression tests should cover:

- Notification channel forms no longer expose manual Chat ID entry for Telegram channels;
- the binding panel exposes `生成绑定码`, private deep-link instructions, and group short-code instructions;
- Telegram binding API client functions and frontend types are exposed;
- rule quick-create channel flow reuses the binding panel;
- bound chat metadata fills the form before saving.
