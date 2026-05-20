# Blockchain Console Frontend Redesign

## Context

Coin Listener is a blockchain monitoring product. The current frontend works functionally, but the layout reads as a generic admin panel: cards are visually flat, filter areas are inconsistent, wide tables can push against the page boundary, and table behavior is configured page-by-page. The redesign should make the product feel like a chain monitoring console while fixing the table and layout issues systematically.

## Goals

- Redesign the full frontend: login, shell, sidebar, header, dashboard, business pages, tables, filters, metric cards, modals, and forms.
- Support light and dark themes with persisted user preference.
- Introduce TailwindCSS alongside Semi Design using Semi's official CSS Layer guidance.
- Add a project-level design-system component layer instead of styling each page independently.
- Ensure data tables stay inside the content container, keep right-side action columns visible, allow column resizing, and persist resized widths in `localStorage`.
- Keep Semi UI as the base component library; do not rebuild Button, Form, Modal, Select, Table, or Tag from scratch.

## Non-goals

- No backend API changes.
- No unrelated data model changes.
- No new charting library unless a page already has enough data to justify it during implementation planning.
- No full replacement of Semi components with custom primitives.

## Visual Direction

The product should feel like a blockchain operations console: precise, trustworthy, data-dense, and slightly terminal-like without sacrificing long-session readability.

### Light theme

- Light mist-gray work area.
- Deep, structured sidebar.
- White or translucent data surfaces with crisp borders.
- Chain-blue / cyan accents for active state, links, focus, and primary actions.

### Dark theme

- Deep blue-black background.
- Elevated glass-like panels using Semi tokens and Tailwind utilities.
- Cyan/blue accents for live chain activity.
- Status tags remain Semi-token driven so danger, warning, and success remain accessible.

## Architecture

### Style stack

1. Semi Design provides core components and tokens.
2. TailwindCSS provides layout, spacing, responsive utilities, and decorative composition.
3. Semi CSS Layer configuration prevents Tailwind Preflight and utilities from breaking Semi components.
4. Project design-system components enforce consistent page structure.

Tailwind must be configured using the Semi official Tailwind guidance:

- For Tailwind v4: declare `@layer theme, base, semi, utilities;` before Tailwind and Semi styles.
- For Tailwind v3: wrap Tailwind layers with `@layer tailwind-base,semi,tailwind-components,tailwind-utils;`.
- Map Semi tokens into Tailwind theme utilities where useful, especially text, background, border, and radius tokens.

The project currently has Vite and no Tailwind dependency. Implementation should add the smallest working Tailwind setup compatible with Vite and the installed package versions.

### Component layer

Create reusable frontend components under `frontend/src/components`:

| Component | Responsibility |
| --- | --- |
| `AppShell` | Own sidebar, top bar, theme switch, authenticated layout, and content scroll boundaries. |
| `PageScaffold` | Standard page title, subtitle, primary actions, and content stack. |
| `FilterPanel` | Consistent filter-card layout with wrapping fields and stable button alignment. |
| `DataSurface` | Bounded data container that prevents page-level horizontal overflow. |
| `DataTable` | Semi Table wrapper with resize, fixed operation column support, persisted column widths, and consistent visual defaults. |
| `MetricGrid` / `MetricCard` | Responsive operational metric cards for dashboard/status pages. |
| `ThemeToggle` | Light/dark/system mode control with persisted preference. |
| `StatusTag` | Optional thin wrapper for common status color mapping when repeated patterns justify it. |

## Table Design

All business tables should migrate to `DataTable`.

### Required behavior

- The page itself must not gain a horizontal scrollbar for table width.
- The table area may scroll horizontally inside `DataSurface`.
- Any table with row actions must keep the action column fixed on the right.
- Columns with explicit widths are resizable.
- Resized widths are persisted per table ID in `localStorage`.
- Long hashes, addresses, URLs, IDs, and errors use ellipsis plus monospace styling.
- Pagination, loading, empty states, and row keys remain handled by Semi Table.

### Semi Table constraints

Semi Table supports fixed columns through `column.fixed` and `scroll.x`. It supports resizing through `resizable`, but the docs note that resizing with `scroll.x` and fixed columns needs care. Implementation must:

- Use config-style `columns`, not JSX columns, because JSX columns do not support resizable.
- Provide stable `width` values for columns that should resize.
- Keep right action columns fixed and non-resizable unless Semi behavior is stable with resizing.
- Preserve at least one flexible non-fixed column where needed to reduce fixed-column alignment issues.
- Avoid inline object churn for heavy table props where it causes unwanted Semi resets.

### Column width persistence

`DataTable` should accept a required `tableId`. Widths are stored under a key such as:

```text
coin-listener:data-table-widths:<tableId>
```

The wrapper applies stored widths on render and updates storage on Semi `resizable.onResizeStop`. If stored data is malformed, it should be ignored and replaced on the next resize.

## Page Coverage

| Page | Redesign notes |
| --- | --- |
| Login | Convert from plain centered card to product-branded blockchain console entry. Keep authentication behavior unchanged. |
| Dashboard | Replace basic milestone/health view with a real overview using health, shortcuts, and operational cards. |
| System status | Use metric grid plus grouped data surfaces for service heartbeat and provider status tables. |
| Chains | Use `PageScaffold` and `DataTable`; keep simple read-only behavior. |
| Assets | Use `DataTable`; emphasize chain, symbol, contract address, built-in status. |
| Providers | Use `DataTable`; fixed action column with edit/test actions; preserve EVM RPC test rules. |
| Addresses | Use `DataTable`; fixed edit/delete action column; preserve multi-chain asset selection behavior. |
| Events | Redesign filters as `FilterPanel`; event table should feel like a chain event ledger. |
| Notification rules | Use `DataTable`; fixed action column; keep rule modal behavior. |
| Notification operations | Use metric grid, filter panel, `DataTable`, and a more readable detail modal. |
| In-app notifications | Use filter panel and fixed action column for mark-read action. |

## Theme Behavior

Theme state should support:

- `light`
- `dark`
- `system`

Preference is persisted in `localStorage`. The chosen theme should be applied through a root attribute or class, for example `data-theme="dark"`, and should cooperate with Semi token usage. The UI should include a compact switch in the header.

## Layout Rules

- The app shell owns viewport height and scrolling.
- Sidebar has stable width and clear active states.
- Header remains compact and shows user, tenant, and theme control.
- Content uses `min-width: 0` and bounded overflow to prevent wide children from expanding the viewport.
- Page cards and panels use consistent vertical rhythm.
- Filter panels wrap fields before they overflow.
- Modals use consistent width, help text, and action alignment.

## Testing and Verification

Add or update UI regression tests to check:

- Tailwind/Semi integration files exist and are imported in the correct order.
- The design-system components exist.
- Business pages use `DataTable` instead of direct scattered `Table` usage where applicable.
- Tables with actions include fixed right action columns through the wrapper or column config.
- `DataTable` includes localStorage-based column width persistence.
- Theme preference is persisted.

Manual/browser verification should cover:

- Light and dark theme switching.
- Wide tables do not create page-level horizontal overflow.
- Table internal horizontal scrolling works.
- Right action columns stay visible.
- Column resizing works and survives refresh.
- Existing create/edit/delete/test/filter behaviors still work.

Build verification:

```bash
npm --prefix frontend run test:ui-regression
npm --prefix frontend run build
```

## Acceptance Criteria

- Full frontend uses the new blockchain console visual language.
- Light/dark theme switch is available and persists.
- Tailwind is integrated without breaking Semi component styles.
- Tables are consistently wrapped by `DataTable` where appropriate.
- Wide tables no longer overflow the page container.
- Action columns remain fixed on the right for action-heavy tables.
- Column widths are draggable and persist across refreshes.
- Existing business behavior remains intact.
