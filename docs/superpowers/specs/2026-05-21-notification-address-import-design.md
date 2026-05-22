# Notification Channels and Address Import Design

## Context

Coin Listener needs three related product improvements: larger and more consistent dialogs, full notification-channel management with Telegram support, and scalable batch watched-address import. The existing frontend already has notification rule selection, watched-address CRUD, and a redesigned table/layout system, but notification channel administration and high-volume address import need clearer product flows and backend support.

## Goals

- Standardize form modal sizing so dense forms are usable on desktop and responsive on smaller screens.
- Add Telegram bot management so administrators configure and verify bots once instead of repeating bot tokens on every channel.
- Add notification channel management so rules can target Telegram chats, in-app channels, webhooks, or email channels through a clear page.
- Support rule-page quick channel creation and refresh without forcing users to leave the rule workflow.
- Add backend task-style batch watched-address import with progress, partial failure handling, cancellation, and persisted error details.
- Preserve existing notification rules, watched-address CRUD behavior, and table layout conventions.

## Non-goals

- No direct calls to the real Telegram API from automated tests.
- No frontend storage of Telegram bot tokens outside transient form state.
- No replacement of Semi Design core components.
- No unrelated refactor of existing listener, scanner, or notification delivery behavior.

## Architecture

Use a layered design:

1. `FormModal` standardizes Semi `Modal` sizing, responsive width, body scrolling, and footer spacing.
2. Telegram bot management owns bot credentials and bot verification.
3. Notification channel management owns destinations such as Telegram bot plus chat ID, webhook endpoint, email recipient, or in-app fallback.
4. Notification rules select notification channels and never handle bot tokens directly.
5. Watched-address batch import uses a backend import-task API instead of frontend-only loops.

## Modal System

Create a small frontend wrapper around Semi `Modal` for form-heavy dialogs.

| Size | Width | Use |
| --- | --- | --- |
| `medium` | 720px | Short forms and confirmations with fields. |
| `large` | 920px | Dense create/edit forms such as watched addresses and notification rules. |
| `wide` | 1120px | Details, previews, import results, and log-heavy views. |

Responsive behavior:

- Desktop uses the configured max width.
- Smaller screens use `calc(100vw - 32px)`.
- Long content scrolls inside the modal body.
- Existing details modals that already require wide display can keep or migrate to `wide`.

Initial migrations:

- `frontend/src/pages/AddressesPage.tsx` create/edit dialog uses `large`.
- `frontend/src/pages/NotificationRulesPage.tsx` create/edit dialog uses `large`.
- Batch import dialog uses `wide` because it includes preview and progress tables.

## Telegram Bot Manager

Add a Telegram bot management module for reusable bots.

### Data model

A Telegram bot record should include:

- `id`
- `tenant_id`
- `name`
- `token_secret` or equivalent backend-only encrypted secret reference
- `token_preview` for safe frontend display
- `status`: `active` or `paused`
- `verification_status`: `unverified`, `verified`, or `failed`
- `last_verified_at`
- `last_error`
- `created_at`
- `updated_at`

The frontend never receives the raw token after create/update. Lists and detail responses expose only `token_preview`.

### UI

Add a `Telegram 机器人` management page or section under notification settings.

Actions:

- Create bot with name and Bot Token.
- Edit name and replace token.
- Enable or pause bot.
- Delete bot after confirmation.
- Verify bot token through the backend.

Verification uses the backend to call Telegram `getMe` or an equivalent adapter method. The UI shows success, failure reason, and last verification time.

## Notification Channel Management

Add a dedicated `通知渠道` page, suggested route `/notifications/channels`.

### Supported channel types

- `in_app`
- `telegram`
- `webhook`
- `email`
- Unknown existing types remain visible as read-only or generic rows so backend data is not hidden.

### Table columns

- Name
- Type
- Status
- Verification status
- Destination summary
- Config summary
- Updated time
- Actions

### Actions

- Create channel.
- Edit channel.
- Enable or pause channel.
- Delete channel with confirmation.
- Verify destination when supported.
- Send test notification when supported.

### Telegram channel configuration

A Telegram notification channel stores destination information, not bot credentials:

- Channel name
- Telegram bot ID
- Chat ID or conversation ID
- Optional conversation alias
- Optional message template
- Status

Verification confirms the selected bot can send to the configured chat. Test send sends a backend-generated test message to that chat and reports the result.

### Rule-page integration

`NotificationRulesPage` keeps channel selection but adds:

- Refresh channels action.
- Quick create channel action.
- Telegram quick-create path that can select an existing bot and create a destination channel.
- Automatic selection of the newly created channel after successful creation.

When no channel is selected, the existing default in-app behavior remains available and visible.

## Backend API Design

### Telegram bots

Suggested endpoints:

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/telegram-bots` | List bots. |
| `POST` | `/api/telegram-bots` | Create bot. |
| `PUT` | `/api/telegram-bots/:id` | Update name, token, or status. |
| `DELETE` | `/api/telegram-bots/:id` | Delete bot. |
| `POST` | `/api/telegram-bots/:id/verify` | Verify token. |

### Notification channels

Suggested endpoints:

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/notification-channels` | List channels. |
| `POST` | `/api/notification-channels` | Create channel. |
| `PUT` | `/api/notification-channels/:id` | Update channel. |
| `DELETE` | `/api/notification-channels/:id` | Delete channel. |
| `POST` | `/api/notification-channels/:id/verify` | Verify destination. |
| `POST` | `/api/notification-channels/:id/test` | Send test notification. |

If some endpoints already exist, implementation should reuse their current shape. Missing endpoints should be added through explicit backend models, services, route handlers, and contract tests.

## Batch Watched-Address Import

Batch import should be backend-task based instead of frontend-only loops.

### Frontend flow

1. User opens `批量添加` from the watched-address page.
2. User chooses default chain, assets, priority, scan interval, filters, and status.
3. User pastes one address per line or CSV.
4. Frontend performs lightweight parsing and preflight validation.
5. User reviews a preview table.
6. User submits and receives an import task ID.
7. UI switches to progress view.
8. User can refresh progress, wait for completion, cancel a running task, or inspect failed rows.
9. Completed imports refresh the watched-address list.

### Input formats

One address per line:

```text
0xabc...
0xdef...
```

CSV:

```text
address,label,priority
0xabc...,Hot wallet,critical
0xdef...,Cold wallet,normal
```

Supported CSV fields:

- `address`
- `label`
- `priority`
- `scan_interval_seconds`
- `transfer_filter_enabled`
- `balance_change_filter_enabled`
- `status`

Unknown CSV fields are reported in the preview. Valid rows can still be submitted after the user confirms the warning.

### Backend APIs

| Method | Path | Purpose |
| --- | --- | --- |
| `POST` | `/api/addresses/imports` | Create import task. |
| `GET` | `/api/addresses/imports/:id` | Get progress and summary. |
| `GET` | `/api/addresses/imports/:id/errors` | Get failed row details. |
| `POST` | `/api/addresses/imports/:id/cancel` | Cancel pending or running task. |

Create request contains:

- Default watched-address configuration.
- Parsed rows with `address`, optional per-row overrides, original row number, and original text.

Task response contains:

- `id`
- `status`: `pending`, `running`, `completed`, `failed`, or `cancelled`
- `total_rows`
- `processed_rows`
- `success_rows`
- `failed_rows`
- `created_at`
- `started_at`
- `completed_at`
- `last_error`

Error rows contain:

- `row_number`
- `address`
- `raw_text`
- `error_code`
- `error_message`

### Backend behavior

- Creating an import returns quickly and does not block the HTTP request for row processing.
- A worker processes rows in the background.
- Row failures do not stop the whole task.
- Duplicate rows in the same import are rejected with row-level errors.
- Existing watched-address conflicts are recorded as row-level errors unless the backend already has a defined upsert behavior.
- Task state and failed-row details are persisted so refreshes can recover progress.
- Cancel requests stop unprocessed rows and leave already-created addresses intact.

## Error Handling

- Bot token verification failure shows backend-provided reason without exposing the token.
- Telegram chat verification failure distinguishes invalid Chat ID, bot not joined, and Telegram API failure when the backend can identify the cause.
- Test send failure displays a concise message and optional details.
- Batch import preview blocks submission for missing required defaults or duplicate rows.
- Import task failure keeps progress data visible.
- Failed import rows are copyable as CSV for correction and retry.

## Testing and Verification

### Frontend tests

Add or update UI regression tests for:

- `FormModal` size definitions and migration usage.
- Notification rules page quick channel creation and refresh entries.
- Telegram bot/channel management pages or routes.
- Batch import parser handling line-based input, CSV input, empty lines, duplicate rows, and unknown CSV fields.
- Import progress states and failed-row rendering.

### Backend tests

Add contract or service tests for:

- Telegram bot create/update/delete/list.
- Telegram bot token verification using a mock Telegram client.
- Telegram channel create/update/delete/list.
- Channel verify and test-send using a mock sender.
- Address import task creation.
- Import task progress state transitions.
- Partial success with row-level failures.
- Duplicate row handling.
- Cancellation.
- Failed-row detail retrieval.

### Verification commands

Frontend:

```bash
npm --prefix frontend run test:ui-regression
npm --prefix frontend run build
```

Backend:

```bash
cargo test --locked --manifest-path backend/Cargo.toml
```

## Acceptance Criteria

- Address and notification rule dialogs use the new modal sizing system.
- Administrators can manage Telegram bots without exposing stored tokens in list/detail views.
- Administrators can verify a Telegram bot token.
- Administrators can create Telegram notification channels that reference a bot and Chat ID.
- Administrators can verify and test-send to a Telegram channel.
- Notification rules can select Telegram channels and can refresh or quick-create channels from the rule workflow.
- Watched-address batch import supports line input and CSV paste.
- Batch imports run as backend tasks with progress, cancellation, persisted failures, and partial success handling.
- Existing watched-address, notification rule, and default in-app notification behavior remains intact.
