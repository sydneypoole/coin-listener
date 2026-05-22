# Telegram Proxy, Light Theme, and RPC Range Design

## Purpose

Add configurable proxy support for Telegram bots, fix unreadable light-mode text colors, and prevent EVM RPC providers from receiving `eth_getLogs` ranges that exceed free-tier block span limits.

## Approved Decisions

- Telegram proxy scope: support both a global Telegram proxy and a per-bot override.
- Telegram proxy input: use a single proxy URL string.
- Light-mode direction: mixed brand style with a dark brand sidebar and readable light main content.
- RPC range fix: split EVM ERC20 `eth_getLogs` requests before they exceed the default 10,000-block span.

## Current Context

Telegram calls currently share one `reqwest::Client` through `ExternalNotificationSender`. Bot storage and frontend forms do not include proxy fields. Telegram verification, notification sending, binding confirmation, and `getUpdates` polling all need to use the same final proxy selection rule.

Frontend styles mix Semi variables with hard-coded dark colors. This makes some light-mode content hard to read because colors such as `#e5f7ff` and `#f8fbff` are used outside reliably dark surfaces.

The EVM ERC20 scan path has a separate root cause. `backend/crates/worker/src/lib.rs` uses `evm_transfer_scan_range` to return the full cursor catch-up range. The initial scan is capped to `EVM_TRANSFER_INITIAL_WINDOW_BLOCKS = 1_000`, but an existing cursor scans from `last_scanned_block + 1` to the latest confirmed block without a max span. `scan_evm_erc20_transfers` then sends that full range to `eth_getLogs`, which can trigger provider errors such as `ranges over 10000 blocks are not supported on freetier`.

## Architecture

The work is split into three focused changes.

1. Telegram proxy configuration
   - Add global Telegram proxy configuration.
   - Add optional per-bot proxy URL override.
   - Resolve final proxy with this priority: bot proxy URL, then global Telegram proxy URL, then direct connection.
   - Apply the resolver to bot verification, notification send, binding confirmation, and update polling.

2. Light-mode token cleanup
   - Keep the current dark brand sidebar in light mode.
   - Move main content, cards, tables, dialogs, and forms to readable light tokens.
   - Replace scattered hard-coded dark-theme text colors with app-level CSS tokens backed by Semi theme variables.

3. EVM RPC range chunking
   - Add a max EVM log block span policy with default `10_000` blocks.
   - Split large ERC20 transfer scan ranges into chunks before calling `eth_getLogs`.
   - Preserve cursor safety by not advancing the scan cursor past failed chunks.

## Backend Design

### Telegram proxy storage

Add a global Telegram proxy value to the existing configuration surface. Add an optional `proxy_url` field to `telegram_bots` through a new migration. Bot list and detail APIs expose proxy source and masked proxy value for the UI, without exposing bot tokens.

The bot-level proxy URL is optional. A blank value means the bot uses the global Telegram proxy if configured. The global proxy is also optional. If neither exists, Telegram calls are direct.

### Telegram proxy validation

Persisted proxy values must be valid URL strings with a supported proxy scheme. The intended supported schemes are HTTP, HTTPS, and SOCKS5 when supported by the HTTP client stack. Save-time validation checks URL shape and scheme. Save-time validation does not require the proxy host to be reachable.

Actual network usability is tested by existing verification behavior. If an admin saves a bot proxy that cannot connect, `verifyTelegramBot` fails and records the last error.

### Proxy-aware Telegram client resolver

Replace the assumption that one fixed `reqwest::Client` covers all Telegram bots. Add a resolver that returns a Telegram HTTP client for the selected proxy URL. The implementation caches clients by normalized proxy URL to avoid rebuilding clients for every request.

All Telegram network methods must accept or derive the final proxy context:

- `verify_telegram_bot`
- `send_telegram`
- binding confirmation messages
- `get_telegram_updates`

Telegram proxy selection must not affect non-Telegram webhook notifications, database access, frontend requests, or chain RPC providers.

### EVM RPC range chunking

Add a helper that splits inclusive block ranges into bounded chunks. With a default max span of `10_000`, the range `1..25_000` becomes `1..10_000`, `10_001..20_000`, and `20_001..25_000`.

Apply chunking inside the EVM ERC20 transfer scan before constructing incoming and outgoing `EvmLogFilter` values. Each asset and direction uses the current chunk range. The worker processes chunks in ascending order.

Cursor behavior remains conservative. The cursor advances only after a chunk completes successfully. If a later chunk fails, the next run resumes from the last safely scanned block and does not skip events.

The first implementation uses a safe default of `10_000`. The design allows a future provider-level field for custom max spans, but provider-level configuration is not required for this delivery.

## Frontend Design

### Telegram bot proxy UI

Add a Telegram proxy section to the TG bot management experience.

- Global proxy card: lets admins set or clear the global Telegram proxy URL.
- Bot form proxy mode: defaults to using the global proxy; can switch to a bot-specific proxy URL.
- Bot list proxy source: shows direct, global proxy, or bot proxy.
- Proxy display: mask credentials in any proxy URL display.

The existing save-and-verify flow remains the main confidence check. Creating or updating a bot can save proxy settings; verification confirms whether the token and final proxy path work together.

The first version places the global Telegram proxy card on the TG bot management page because there is no dedicated system settings page in the current UI. The data boundary stays separate from bot records so it can later move to a system settings page without changing Telegram call behavior.

### Light-mode theme UI

Use the approved mixed brand direction.

- Sidebar stays dark in light mode.
- Main content uses light background and dark text.
- Cards, tables, dialogs, forms, banners, and list surfaces use light-mode readable tokens.
- Login page can keep branded dark atmosphere, but card contents, inputs, and helper text must remain readable.

Introduce app-level CSS tokens such as:

- `--app-shell-sidebar-bg`
- `--app-content-bg`
- `--app-card-bg`
- `--app-text-primary`
- `--app-text-secondary`
- `--app-border-subtle`

Map those tokens to Semi variables and override them for dark mode. Prefer token usage over hard-coded colors. Hard-coded light-on-dark text is acceptable only inside intentionally dark surfaces such as the brand sidebar.

## Error Handling

- Invalid global proxy URL: reject save with a clear validation error.
- Invalid bot proxy URL: reject bot save with a field-level error.
- Proxy unreachable: allow save, but verification or send operations fail with the network reason recorded in bot or notification error state.
- Proxy authentication failure: surface the Telegram request failure and keep the bot verification state accurate.
- `getUpdates` failure through proxy: do not advance the update offset.
- Telegram binding confirmation failure: preserve existing confirmation error behavior.
- RPC chunk failure: do not advance the scan cursor past the failed chunk.
- RPC range-limit response after chunking: record the provider error and make the max-span policy easy to lower in the implementation plan.
- Light-mode color miss: catch via source-level regression and build validation; use browser inspection if a specific surface remains unreadable.

## Testing Strategy

### Backend

- Unit-test proxy URL validation for accepted and rejected schemes.
- Unit-test proxy selection priority: bot override, global proxy, direct.
- Test Telegram verification, send, and `getUpdates` use the proxy-aware resolver path.
- Test bot create/update persists `proxy_url` and clears it when requested.
- Test EVM range chunking for exact boundaries, including a range larger than `10_000` blocks.
- Test ERC20 scan cursor safety: successful chunks advance; failed chunks do not skip unprocessed blocks.

### Frontend

- Extend source-level regression tests to assert the Telegram bot UI exposes global proxy and bot proxy configuration.
- Assert proxy displays are masked where shown.
- Assert light-mode CSS uses app tokens for main content, cards, dialogs, tables, and text.
- Assert known light-on-dark hard-coded colors are not used in light main content selectors.

### Verification Commands

Run these before declaring implementation complete:

```bash
cargo test --locked --manifest-path backend/Cargo.toml
npm --prefix frontend run test:ui-regression
npm --prefix frontend run build
```

## Acceptance Criteria

- Admins can configure a global Telegram proxy URL.
- A Telegram bot can override the global proxy with its own proxy URL.
- Telegram verification, notification send, binding confirmation, and `getUpdates` all use the same final proxy selection rule.
- EVM ERC20 scanning does not create default `eth_getLogs` requests larger than `10_000` blocks.
- Failed RPC chunks do not advance the cursor past unprocessed blocks.
- Light mode keeps the dark brand sidebar but makes main content, cards, tables, dialogs, and forms readable.
- Dark mode remains usable and keeps the existing product atmosphere.

## Out of Scope

- Provider-specific RPC max-span UI.
- Full frontend redesign beyond readability fixes.
- Proxy configuration for non-Telegram integrations.
- Changing Telegram channel binding semantics.
