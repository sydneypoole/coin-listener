# Blockchain Console Frontend Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild the frontend into a blockchain operations console with Tailwind + Semi CSS Layer, light/dark theme switching, reusable layout primitives, and resizable/persisted/fixed-action data tables.

**Architecture:** Keep Semi Design as the component foundation, add Tailwind for layout and visual composition, and introduce a small project design-system layer under `frontend/src/components`. All business tables migrate through a `DataTable` wrapper so overflow, fixed action columns, resizing, and localStorage persistence are solved once.

**Tech Stack:** React, TypeScript, Vite, Semi Design, TailwindCSS v4, `@tailwindcss/vite`, `@douyinfe/semi-vite-plugin`, Node test runner.

---

## Parallel execution map

Sequential foundation:

1. Task 1 regression tests.
2. Task 2 Tailwind + Semi CSS Layer.
3. Task 3 theme system and shell.
4. Task 4 layout primitives.
5. Task 5 `DataTable`.

After Task 5, use parallel agents:

| Agent | Tasks | Files |
| --- | --- | --- |
| A | Task 6 simple configuration pages | `ChainsPage.tsx`, `AssetsPage.tsx`, `ProvidersPage.tsx`, `AddressesPage.tsx` |
| B | Task 7 operational pages | `EventsPage.tsx`, `NotificationRulesPage.tsx`, `InAppNotificationsPage.tsx` |
| C | Task 8 dashboard/status/notification operations/login polish | `App.tsx`, `SystemStatusPage.tsx`, `NotificationOperationsPage.tsx`, `LoginPage.tsx` |

Task 9 runs after all agents merge.

---

## File structure

Create:

- `frontend/src/semi-layer.css` — CSS layer order bootstrap for Semi + Tailwind.
- `frontend/src/tailwind.css` — Tailwind v4 entry and Semi token mappings.
- `frontend/src/themeMode.ts` — theme preference loading, applying, and system-mode handling.
- `frontend/src/components/ThemeToggle.tsx` — header theme selector.
- `frontend/src/components/AppShell.tsx` — authenticated app chrome.
- `frontend/src/components/PageScaffold.tsx` — standard page header/content layout.
- `frontend/src/components/FilterPanel.tsx` — consistent filter panel.
- `frontend/src/components/MetricGrid.tsx` — metric card grid.
- `frontend/src/components/DataSurface.tsx` — bounded table/card surface.
- `frontend/src/components/DataTable.tsx` — Semi Table wrapper.

Modify:

- `frontend/package.json`
- `frontend/package-lock.json`
- `frontend/vite.config.ts`
- `frontend/src/main.tsx`
- `frontend/src/styles.css`
- `frontend/src/App.tsx`
- `frontend/src/ui-regression.test.ts`
- `frontend/src/pages/LoginPage.tsx`
- `frontend/src/pages/ChainsPage.tsx`
- `frontend/src/pages/AssetsPage.tsx`
- `frontend/src/pages/ProvidersPage.tsx`
- `frontend/src/pages/AddressesPage.tsx`
- `frontend/src/pages/EventsPage.tsx`
- `frontend/src/pages/SystemStatusPage.tsx`
- `frontend/src/pages/NotificationRulesPage.tsx`
- `frontend/src/pages/NotificationOperationsPage.tsx`
- `frontend/src/pages/InAppNotificationsPage.tsx`

---

## Task 1: Add failing UI regression checks

**Files:**

- Modify: `frontend/src/ui-regression.test.ts`

- [ ] **Step 1: Add regression tests before implementation**

Append these tests inside the existing `describe('frontend UI regressions', () => { ... })` block:

```ts
  test('tailwind and semi css layers are wired before app styles', () => {
    const packageJson = readSource('../package.json');
    const viteConfig = readSource('../vite.config.ts');
    const main = readSource('main.tsx');
    const semiLayer = readSource('semi-layer.css');
    const tailwind = readSource('tailwind.css');

    expectContains(packageJson, 'tailwindcss');
    expectContains(packageJson, '@tailwindcss/vite');
    expectContains(packageJson, '@douyinfe/semi-vite-plugin');
    expectContains(viteConfig, 'tailwindcss()');
    expectContains(viteConfig, 'semiTheming({ cssLayer: true })');
    expectContains(semiLayer, '@layer theme, base, semi, utilities;');
    expectContains(tailwind, '@import "tailwindcss";');
    expectContains(main, "import './semi-layer.css';");
    expectContains(main, "import './tailwind.css';");
  });

  test('theme mode persists and uses semi dark mode contract', () => {
    const themeMode = readSource('themeMode.ts');
    const app = readSource('App.tsx');
    const toggle = readSource('components/ThemeToggle.tsx');

    expectContains(themeMode, 'coin-listener:theme-mode');
    expectContains(themeMode, "document.body.setAttribute('theme-mode', 'dark')");
    expectContains(themeMode, "document.body.removeAttribute('theme-mode')");
    expectContains(themeMode, "matchMedia('(prefers-color-scheme: dark)')");
    expectContains(toggle, 'ThemeToggle');
    expectContains(app, '<ThemeToggle');
  });

  test('frontend design system components exist', () => {
    for (const componentPath of [
      'components/AppShell.tsx',
      'components/PageScaffold.tsx',
      'components/FilterPanel.tsx',
      'components/MetricGrid.tsx',
      'components/DataSurface.tsx',
      'components/DataTable.tsx',
    ]) {
      const source = readSource(componentPath);
      expectContains(source, 'export');
    }
  });

  test('data table wrapper persists resized widths and fixes action columns', () => {
    const table = readSource('components/DataTable.tsx');

    expectContains(table, 'coin-listener:data-table-widths:');
    expectContains(table, 'localStorage');
    expectContains(table, 'onResizeStop');
    expectContains(table, "fixed: 'right'");
    expectContains(table, 'resizable=');
    expectContains(table, 'data-table-surface');
  });

  test('business pages use DataTable for table overflow control', () => {
    const pagePaths = [
      'pages/ChainsPage.tsx',
      'pages/AssetsPage.tsx',
      'pages/ProvidersPage.tsx',
      'pages/AddressesPage.tsx',
      'pages/EventsPage.tsx',
      'pages/SystemStatusPage.tsx',
      'pages/NotificationRulesPage.tsx',
      'pages/NotificationOperationsPage.tsx',
      'pages/InAppNotificationsPage.tsx',
    ];

    for (const pagePath of pagePaths) {
      const page = readSource(pagePath);
      expectContains(page, 'DataTable');
      expectContains(page, 'tableId=');
      expectNotContains(page, ' Table,');
      expectNotContains(page, '<Table<');
    }
  });
```

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
npm --prefix frontend run test:ui-regression
```

Expected: FAIL because `semi-layer.css`, `tailwind.css`, `themeMode.ts`, and `components/DataTable.tsx` do not exist yet.

- [ ] **Step 3: Commit regression tests**

```bash
git add frontend/src/ui-regression.test.ts
git commit -m "添加前端重设计回归测试"
```

---

## Task 2: Integrate Tailwind with Semi CSS Layer

**Files:**

- Modify: `frontend/package.json`
- Modify: `frontend/package-lock.json`
- Modify: `frontend/vite.config.ts`
- Modify: `frontend/src/main.tsx`
- Modify: `frontend/src/styles.css`
- Create: `frontend/src/semi-layer.css`
- Create: `frontend/src/tailwind.css`

- [ ] **Step 1: Install dependencies**

Run:

```bash
npm --prefix frontend install -D tailwindcss @tailwindcss/vite @douyinfe/semi-vite-plugin
```

Expected: `frontend/package.json` gains the three dev dependencies and `frontend/package-lock.json` updates.

- [ ] **Step 2: Update Vite config**

Replace `frontend/vite.config.ts` with:

```ts
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import semi from '@douyinfe/semi-vite-plugin';

const { semiTheming } = semi;

export default defineConfig({
  plugins: [
    tailwindcss(),
    semiTheming({ cssLayer: true }),
    react(),
  ],
  server: {
    port: 5173,
  },
});
```

- [ ] **Step 3: Create CSS layer bootstrap**

Create `frontend/src/semi-layer.css`:

```css
@layer theme, base, semi, utilities;
```

- [ ] **Step 4: Create Tailwind entry**

Create `frontend/src/tailwind.css`:

```css
@import "tailwindcss";

@theme {
  --color-semi-color-primary: var(--semi-color-primary);
  --color-semi-color-primary-hover: var(--semi-color-primary-hover);
  --color-semi-color-bg-0: var(--semi-color-bg-0);
  --color-semi-color-bg-1: var(--semi-color-bg-1);
  --color-semi-color-bg-2: var(--semi-color-bg-2);
  --color-semi-color-border: var(--semi-color-border);
  --color-semi-color-text-0: var(--semi-color-text-0);
  --color-semi-color-text-1: var(--semi-color-text-1);
  --color-semi-color-text-2: var(--semi-color-text-2);
  --radius-semi-border-radius-small: var(--semi-border-radius-small);
  --radius-semi-border-radius-medium: var(--semi-border-radius-medium);
  --radius-semi-border-radius-large: var(--semi-border-radius-large);
}

body {
  --color-semi-color-primary: var(--semi-color-primary);
  --color-semi-color-primary-hover: var(--semi-color-primary-hover);
  --color-semi-color-bg-0: var(--semi-color-bg-0);
  --color-semi-color-bg-1: var(--semi-color-bg-1);
  --color-semi-color-bg-2: var(--semi-color-bg-2);
  --color-semi-color-border: var(--semi-color-border);
  --color-semi-color-text-0: var(--semi-color-text-0);
  --color-semi-color-text-1: var(--semi-color-text-1);
  --color-semi-color-text-2: var(--semi-color-text-2);
  --radius-semi-border-radius-small: var(--semi-border-radius-small);
  --radius-semi-border-radius-medium: var(--semi-border-radius-medium);
  --radius-semi-border-radius-large: var(--semi-border-radius-large);
}
```

- [ ] **Step 5: Fix import order**

Replace the import section of `frontend/src/main.tsx` with:

```ts
import './semi-layer.css';
import './tailwind.css';
import React from 'react';
import ReactDOM from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import '@douyinfe/semi-ui/lib/es/_base/base.css';
import './styles.css';
import { App } from './App';
```

Keep the existing `QueryClient` creation and render code unchanged.

- [ ] **Step 6: Replace global shell CSS base**

In `frontend/src/styles.css`, replace lines 1-44 with:

```css
html,
body,
#root {
  width: 100%;
  height: 100%;
  margin: 0;
  overflow: hidden;
}

body {
  color: var(--semi-color-text-0);
  background:
    radial-gradient(circle at 12% 0%, rgba(20, 184, 166, 0.12), transparent 32%),
    radial-gradient(circle at 88% 10%, rgba(59, 130, 246, 0.14), transparent 30%),
    var(--semi-color-bg-0);
}

.app-shell {
  height: 100vh;
  min-width: 0;
  background: transparent;
}

.app-sider {
  min-height: 100vh;
  background: linear-gradient(180deg, #06111f 0%, #0b1728 54%, #101827 100%);
  border-right: 1px solid rgba(148, 163, 184, 0.18);
}

.brand {
  height: 64px;
  display: flex;
  align-items: center;
  padding: 0 20px;
  color: #e5f7ff;
  font-weight: 800;
  font-size: 18px;
  letter-spacing: 0.02em;
}

.app-header {
  height: 64px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 0 24px;
  background: color-mix(in srgb, var(--semi-color-bg-0) 88%, transparent);
  border-bottom: 1px solid var(--semi-color-border);
  backdrop-filter: blur(16px);
}

.app-content {
  min-width: 0;
  height: calc(100vh - 64px);
  padding: 24px;
  overflow: auto;
}
```

Keep existing `.content-stack`, `.status-card`, `.filter-card`, `.table-cell-mono`, `.form-help-text`, `.login-page`, `.login-card`, and `.login-form` definitions for now. Later tasks refine them.

- [ ] **Step 7: Run regression and build**

Run:

```bash
npm --prefix frontend run test:ui-regression
npm --prefix frontend run build
```

Expected: tests still fail until later tasks create components and migrate pages; build should pass or fail only on missing later files if imports were added early. Do not add imports for missing files in this task.

- [ ] **Step 8: Commit Tailwind/Semi integration**

```bash
git add frontend/package.json frontend/package-lock.json frontend/vite.config.ts frontend/src/main.tsx frontend/src/semi-layer.css frontend/src/tailwind.css frontend/src/styles.css
git commit -m "接入 Tailwind 与 Semi CSS Layer"
```

---

## Task 3: Add theme system and AppShell

**Files:**

- Create: `frontend/src/themeMode.ts`
- Create: `frontend/src/components/ThemeToggle.tsx`
- Create: `frontend/src/components/AppShell.tsx`
- Modify: `frontend/src/App.tsx`
- Modify: `frontend/src/styles.css`

- [ ] **Step 1: Write theme utility**

Create `frontend/src/themeMode.ts`:

```ts
export type ThemeMode = 'light' | 'dark' | 'system';

export const THEME_MODE_STORAGE_KEY = 'coin-listener:theme-mode';

function systemPrefersDark() {
  return window.matchMedia('(prefers-color-scheme: dark)').matches;
}

export function loadThemeMode(): ThemeMode {
  const value = localStorage.getItem(THEME_MODE_STORAGE_KEY);
  return value === 'light' || value === 'dark' || value === 'system' ? value : 'system';
}

export function resolveThemeMode(mode: ThemeMode): 'light' | 'dark' {
  if (mode === 'system') {
    return systemPrefersDark() ? 'dark' : 'light';
  }
  return mode;
}

export function applyThemeMode(mode: ThemeMode) {
  const resolvedMode = resolveThemeMode(mode);
  document.documentElement.dataset.theme = resolvedMode;
  if (resolvedMode === 'dark') {
    document.body.setAttribute('theme-mode', 'dark');
    return;
  }
  document.body.removeAttribute('theme-mode');
}

export function saveThemeMode(mode: ThemeMode) {
  localStorage.setItem(THEME_MODE_STORAGE_KEY, mode);
  applyThemeMode(mode);
}

export function subscribeSystemTheme(callback: () => void) {
  const media = window.matchMedia('(prefers-color-scheme: dark)');
  media.addEventListener('change', callback);
  return () => media.removeEventListener('change', callback);
}
```

- [ ] **Step 2: Write ThemeToggle**

Create `frontend/src/components/ThemeToggle.tsx`:

```tsx
import { Select } from '@douyinfe/semi-ui';
import type { ThemeMode } from '../themeMode';

type ThemeToggleProps = {
  value: ThemeMode;
  onChange: (mode: ThemeMode) => void;
};

const options = [
  { label: '跟随系统', value: 'system' },
  { label: '浅色', value: 'light' },
  { label: '暗色', value: 'dark' },
];

export function ThemeToggle({ value, onChange }: ThemeToggleProps) {
  return (
    <Select
      value={value}
      optionList={options}
      size="small"
      style={{ width: 112 }}
      onChange={nextValue => onChange(nextValue as ThemeMode)}
    />
  );
}
```

- [ ] **Step 3: Write AppShell**

Create `frontend/src/components/AppShell.tsx`:

```tsx
import type { ReactNode } from 'react';
import { Layout, Nav, Space, Typography } from '@douyinfe/semi-ui';
import type { NavItemProps } from '@douyinfe/semi-ui/lib/es/navigation';
import { ThemeToggle } from './ThemeToggle';
import type { ThemeMode } from '../themeMode';

const { Header, Sider, Content } = Layout;
const { Text, Title } = Typography;

type AppShellProps<PageKey extends string> = {
  page: PageKey;
  navItems: NavItemProps[];
  userLabel: string;
  tenantLabel: string;
  themeMode: ThemeMode;
  onThemeModeChange: (mode: ThemeMode) => void;
  onSelectPage: (page: PageKey) => void;
  onLogout: () => void;
  children: ReactNode;
};

export function AppShell<PageKey extends string>({
  page,
  navItems,
  userLabel,
  tenantLabel,
  themeMode,
  onThemeModeChange,
  onSelectPage,
  onLogout,
  children,
}: AppShellProps<PageKey>) {
  return (
    <Layout className="app-shell">
      <Sider className="app-sider">
        <div className="brand">
          <span className="brand-mark">CL</span>
          <span>Coin Listener</span>
        </div>
        <Nav
          className="chain-nav"
          selectedKeys={[page]}
          onSelect={({ itemKey }) => onSelectPage(itemKey as PageKey)}
          items={navItems}
        />
      </Sider>
      <Layout className="min-w-0">
        <Header className="app-header">
          <div>
            <Title heading={4} style={{ margin: 0 }}>链上监控控制台</Title>
            <Text type="tertiary">多链资产、事件与通知运维工作台</Text>
          </div>
          <Space>
            <Text type="tertiary">{userLabel} / {tenantLabel}</Text>
            <ThemeToggle value={themeMode} onChange={onThemeModeChange} />
            <button className="shell-logout-button" type="button" onClick={onLogout}>退出登录</button>
          </Space>
        </Header>
        <Content className="app-content">{children}</Content>
      </Layout>
    </Layout>
  );
}
```

- [ ] **Step 4: Wire theme and shell in App**

In `frontend/src/App.tsx`:

1. Add imports:

```ts
import { useCallback, useEffect, useMemo, useState } from 'react';
import { AppShell } from './components/AppShell';
import { applyThemeMode, loadThemeMode, saveThemeMode, subscribeSystemTheme, type ThemeMode } from './themeMode';
```

2. Remove `Layout` from the Semi import and remove `const { Header, Sider, Content } = Layout;`.

3. Add state inside `App`:

```ts
  const [themeMode, setThemeMode] = useState<ThemeMode>(() => loadThemeMode());
```

4. Add effects and handler inside `App`:

```ts
  useEffect(() => {
    applyThemeMode(themeMode);
    if (themeMode !== 'system') return undefined;
    return subscribeSystemTheme(() => applyThemeMode('system'));
  }, [themeMode]);

  function handleThemeModeChange(nextMode: ThemeMode) {
    setThemeMode(nextMode);
    saveThemeMode(nextMode);
  }
```

5. Extract nav items before return:

```ts
  const navItems = useMemo(() => [
    { itemKey: 'dashboard', text: '仪表盘', icon: <IconPulse /> },
    { itemKey: 'system-status', text: '系统状态', icon: <IconPulse /> },
    { itemKey: 'chains', text: '链配置', icon: <IconServer /> },
    { itemKey: 'assets', text: '资产配置', icon: <IconSetting /> },
    { itemKey: 'providers', text: 'Provider', icon: <IconServer /> },
    { itemKey: 'addresses', text: '监听地址', icon: <IconUser /> },
    { itemKey: 'events', text: '事件中心', icon: <IconBell /> },
    { itemKey: 'notification-rules', text: '通知规则', icon: <IconBell /> },
    { itemKey: 'notification-operations', text: '通知运维', icon: <IconBell /> },
    {
      itemKey: 'in-app-notifications',
      text: realtimeUnreadCount > 0 ? `站内通知 (${realtimeUnreadCount})` : '站内通知',
      icon: <IconBell />,
    },
  ], [realtimeUnreadCount]);
```

6. Replace the authenticated `return` block with:

```tsx
  return (
    <AppShell<PageKey>
      page={page}
      navItems={navItems}
      userLabel={session.user.display_name}
      tenantLabel={session.tenant.name}
      themeMode={themeMode}
      onThemeModeChange={handleThemeModeChange}
      onSelectPage={setPage}
      onLogout={handleLogout}
    >
      {renderPage(page, healthQuery, setRealtimeUnreadCount)}
    </AppShell>
  );
```

- [ ] **Step 5: Add shell CSS**

Append to `frontend/src/styles.css`:

```css
.brand-mark {
  width: 28px;
  height: 28px;
  margin-right: 10px;
  border-radius: 10px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: #05111f;
  background: linear-gradient(135deg, #22d3ee, #60a5fa);
  font-size: 12px;
  font-weight: 900;
}

.chain-nav .semi-navigation-item {
  color: rgba(226, 232, 240, 0.78);
}

.chain-nav .semi-navigation-item-selected {
  color: #e0f2fe;
  background: rgba(14, 165, 233, 0.18);
}

.shell-logout-button {
  border: 1px solid var(--semi-color-border);
  border-radius: 999px;
  padding: 6px 12px;
  color: var(--semi-color-text-1);
  background: var(--semi-color-bg-1);
  cursor: pointer;
}
```

- [ ] **Step 6: Verify this task**

Run:

```bash
npm --prefix frontend run build
npm --prefix frontend run test:ui-regression
```

Expected: build passes. Regression still fails on missing later design components and page migrations.

- [ ] **Step 7: Commit theme and shell**

```bash
git add frontend/src/themeMode.ts frontend/src/components/ThemeToggle.tsx frontend/src/components/AppShell.tsx frontend/src/App.tsx frontend/src/styles.css
git commit -m "添加前端主题切换与应用外壳"
```

---

## Task 4: Add layout primitives

**Files:**

- Create: `frontend/src/components/PageScaffold.tsx`
- Create: `frontend/src/components/FilterPanel.tsx`
- Create: `frontend/src/components/MetricGrid.tsx`
- Create: `frontend/src/components/DataSurface.tsx`
- Modify: `frontend/src/styles.css`

- [ ] **Step 1: Create PageScaffold**

Create `frontend/src/components/PageScaffold.tsx`:

```tsx
import type { ReactNode } from 'react';
import { Space, Typography } from '@douyinfe/semi-ui';

const { Text, Title } = Typography;

type PageScaffoldProps = {
  title: string;
  description?: string;
  actions?: ReactNode;
  children: ReactNode;
};

export function PageScaffold({ title, description, actions, children }: PageScaffoldProps) {
  return (
    <section className="page-scaffold">
      <div className="page-heading">
        <div>
          <Title heading={3} style={{ margin: 0 }}>{title}</Title>
          {description ? <Text type="tertiary">{description}</Text> : null}
        </div>
        {actions ? <div className="page-actions">{actions}</div> : null}
      </div>
      <Space vertical align="start" spacing={16} className="content-stack">
        {children}
      </Space>
    </section>
  );
}
```

- [ ] **Step 2: Create FilterPanel**

Create `frontend/src/components/FilterPanel.tsx`:

```tsx
import type { ReactNode } from 'react';
import { Card } from '@douyinfe/semi-ui';

type FilterPanelProps = {
  title: string;
  children: ReactNode;
};

export function FilterPanel({ title, children }: FilterPanelProps) {
  return (
    <Card title={title} className="filter-panel">
      <div className="filter-panel-body">{children}</div>
    </Card>
  );
}
```

- [ ] **Step 3: Create MetricGrid**

Create `frontend/src/components/MetricGrid.tsx`:

```tsx
import type { ReactNode } from 'react';
import { Card, Typography } from '@douyinfe/semi-ui';

const { Text, Title } = Typography;

type MetricGridProps = {
  children: ReactNode;
};

type MetricCardProps = {
  title: string;
  value: string | number;
  hint: string;
  tone?: 'neutral' | 'success' | 'warning' | 'danger';
};

export function MetricGrid({ children }: MetricGridProps) {
  return <div className="metric-grid">{children}</div>;
}

export function MetricCard({ title, value, hint, tone = 'neutral' }: MetricCardProps) {
  return (
    <Card className={`metric-card metric-card-${tone}`}>
      <Text type="tertiary">{title}</Text>
      <Title heading={3} style={{ margin: '8px 0 4px' }}>{value}</Title>
      <Text type="tertiary">{hint}</Text>
    </Card>
  );
}
```

- [ ] **Step 4: Create DataSurface**

Create `frontend/src/components/DataSurface.tsx`:

```tsx
import type { ReactNode } from 'react';
import { Card } from '@douyinfe/semi-ui';

type DataSurfaceProps = {
  title: string;
  actions?: ReactNode;
  children: ReactNode;
};

export function DataSurface({ title, actions, children }: DataSurfaceProps) {
  return (
    <Card title={title} headerExtraContent={actions} className="data-surface">
      <div className="data-surface-body">{children}</div>
    </Card>
  );
}
```

- [ ] **Step 5: Add primitive CSS**

Append to `frontend/src/styles.css`:

```css
.page-scaffold {
  width: 100%;
  min-width: 0;
}

.page-heading {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
  margin-bottom: 20px;
}

.page-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.filter-panel,
.data-surface {
  width: 100%;
  min-width: 0;
  border: 1px solid var(--semi-color-border);
  background: color-mix(in srgb, var(--semi-color-bg-1) 92%, transparent);
  box-shadow: 0 20px 60px rgba(15, 23, 42, 0.06);
}

.filter-panel-body {
  min-width: 0;
}

.filter-panel-body .semi-form-horizontal {
  display: flex;
  flex-wrap: wrap;
  align-items: flex-end;
  gap: 12px 16px;
}

.filter-panel-body .semi-form-field {
  margin-bottom: 0;
}

.data-surface-body {
  min-width: 0;
  overflow: hidden;
}

.metric-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: 16px;
  width: 100%;
}

.metric-card {
  border: 1px solid var(--semi-color-border);
  background: linear-gradient(180deg, var(--semi-color-bg-1), var(--semi-color-bg-0));
}

.metric-card-success { border-color: rgba(34, 197, 94, 0.32); }
.metric-card-warning { border-color: rgba(245, 158, 11, 0.34); }
.metric-card-danger { border-color: rgba(239, 68, 68, 0.34); }
```

- [ ] **Step 6: Verify and commit**

```bash
npm --prefix frontend run build
git add frontend/src/components/PageScaffold.tsx frontend/src/components/FilterPanel.tsx frontend/src/components/MetricGrid.tsx frontend/src/components/DataSurface.tsx frontend/src/styles.css
git commit -m "添加前端布局组件"
```

---

## Task 5: Add DataTable wrapper

**Files:**

- Create: `frontend/src/components/DataTable.tsx`
- Modify: `frontend/src/styles.css`

- [ ] **Step 1: Create DataTable**

Create `frontend/src/components/DataTable.tsx`:

```tsx
import { useEffect, useMemo, useState } from 'react';
import { Table } from '@douyinfe/semi-ui';
import type { ColumnProps, TableProps } from '@douyinfe/semi-ui/lib/es/table/interface';

const STORAGE_PREFIX = 'coin-listener:data-table-widths:';

type RowData = Record<string, unknown>;

type WidthMap = Record<string, number>;

type DataTableProps<RecordType extends RowData> = Omit<TableProps<RecordType>, 'columns' | 'resizable'> & {
  tableId: string;
  columns: ColumnProps<RecordType>[];
  actionColumnKeys?: Array<string | number>;
};

function storageKey(tableId: string) {
  return `${STORAGE_PREFIX}${tableId}`;
}

function readWidths(tableId: string): WidthMap {
  try {
    const raw = localStorage.getItem(storageKey(tableId));
    if (!raw) return {};
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    return Object.fromEntries(
      Object.entries(parsed).filter(([, value]) => typeof value === 'number' && Number.isFinite(value)),
    );
  } catch {
    return {};
  }
}

function writeWidths(tableId: string, widths: WidthMap) {
  try {
    localStorage.setItem(storageKey(tableId), JSON.stringify(widths));
  } catch {
    // Ignore storage failures so table interaction still works in restricted browsers.
  }
}

function normalizeKey<RecordType extends RowData>(column: ColumnProps<RecordType>, indexPath: string) {
  if (column.key !== undefined && column.key !== null) return String(column.key);
  if (typeof column.dataIndex === 'string') return column.dataIndex;
  return `column-${indexPath}`;
}

function numericWidth(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0;
}

function prepareColumns<RecordType extends RowData>(
  columns: ColumnProps<RecordType>[],
  widths: WidthMap,
  actionColumnKeys: Array<string | number>,
  parentPath = '',
): ColumnProps<RecordType>[] {
  const actionKeySet = new Set(actionColumnKeys.map(String));

  return columns.map((column, index) => {
    const indexPath = parentPath ? `${parentPath}-${index}` : String(index);
    const key = normalizeKey(column, indexPath);
    const isActionColumn = actionKeySet.has(key);
    const width = widths[key] ?? column.width;
    const nextColumn: ColumnProps<RecordType> = {
      ...column,
      key,
      width,
      fixed: isActionColumn ? 'right' : column.fixed,
      resize: isActionColumn ? false : column.resize,
    };

    if (Array.isArray(column.children) && column.children.length > 0) {
      nextColumn.children = prepareColumns(column.children, widths, actionColumnKeys, indexPath);
    }

    return nextColumn;
  });
}

function totalWidth<RecordType extends RowData>(columns: ColumnProps<RecordType>[]) {
  return columns.reduce((sum, column) => {
    const childWidth = Array.isArray(column.children) ? totalWidth(column.children) : 0;
    return sum + numericWidth(column.width) + childWidth;
  }, 0);
}

export function DataTable<RecordType extends RowData>({
  tableId,
  columns,
  actionColumnKeys = ['operation', 'operations', 'actions'],
  className,
  scroll,
  ...props
}: DataTableProps<RecordType>) {
  const [widths, setWidths] = useState<WidthMap>(() => readWidths(tableId));

  useEffect(() => {
    setWidths(readWidths(tableId));
  }, [tableId]);

  const preparedColumns = useMemo(
    () => prepareColumns(columns, widths, actionColumnKeys),
    [actionColumnKeys, columns, widths],
  );

  const scrollX = scroll?.x ?? Math.max(totalWidth(preparedColumns), 720);

  return (
    <div className="data-table-surface">
      <Table<RecordType>
        {...props}
        className={['data-table', className].filter(Boolean).join(' ')}
        columns={preparedColumns}
        scroll={{ ...scroll, x: scrollX }}
        resizable={{
          onResizeStop: column => {
            const key = column.key === undefined || column.key === null ? undefined : String(column.key);
            const width = numericWidth(column.width);
            if (!key || !width) return column;
            const nextWidths = { ...widths, [key]: width };
            setWidths(nextWidths);
            writeWidths(tableId, nextWidths);
            return column;
          },
        }}
      />
    </div>
  );
}
```

- [ ] **Step 2: Add DataTable CSS**

Append to `frontend/src/styles.css`:

```css
.data-table-surface {
  width: 100%;
  min-width: 0;
  overflow: hidden;
}

.data-table .semi-table-container {
  min-width: 0;
}

.data-table .semi-table-thead > .semi-table-row > .semi-table-row-head {
  color: var(--semi-color-text-1);
  background: color-mix(in srgb, var(--semi-color-bg-2) 86%, transparent);
  font-size: 12px;
  font-weight: 700;
}

.data-table .semi-table-row:hover .semi-table-row-cell {
  background: color-mix(in srgb, var(--semi-color-primary) 7%, var(--semi-color-bg-1));
}
```

- [ ] **Step 3: Verify and commit**

```bash
npm --prefix frontend run build
git add frontend/src/components/DataTable.tsx frontend/src/styles.css
git commit -m "添加可伸缩数据表格封装"
```

Expected: build passes. Regression still fails until pages migrate.

---

## Task 6: Migrate simple configuration pages

**Files:**

- Modify: `frontend/src/pages/ChainsPage.tsx`
- Modify: `frontend/src/pages/AssetsPage.tsx`
- Modify: `frontend/src/pages/ProvidersPage.tsx`
- Modify: `frontend/src/pages/AddressesPage.tsx`

- [ ] **Step 1: Migrate ChainsPage**

Replace `Card, Table, Tag` import with:

```ts
import { Tag } from '@douyinfe/semi-ui';
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { PageScaffold } from '../components/PageScaffold';
```

Replace the return with:

```tsx
  return (
    <PageScaffold title="链配置" description="维护多链监听所需的链基础信息。">
      <DataSurface title="链列表">
        <DataTable<Chain>
          tableId="chains"
          loading={query.isLoading}
          dataSource={query.data ?? []}
          rowKey="id"
          pagination={false}
          columns={[
            { title: '名称', dataIndex: 'name', width: 180, ellipsis: { showTitle: true } },
            { title: 'Key', dataIndex: 'key', width: 160, ellipsis: { showTitle: true }, className: 'table-cell-mono' },
            { title: '类型', dataIndex: 'chain_type', width: 120 },
            { title: '原生资产', dataIndex: 'native_asset_symbol', width: 120 },
            { title: '确认数', dataIndex: 'default_confirmations', width: 100 },
            { title: '状态', dataIndex: 'status', width: 120, render: value => <Tag color="green">{String(value)}</Tag> },
          ]}
        />
      </DataSurface>
    </PageScaffold>
  );
```

- [ ] **Step 2: Migrate AssetsPage**

Use imports:

```ts
import { Tag } from '@douyinfe/semi-ui';
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { PageScaffold } from '../components/PageScaffold';
```

Wrap the table with:

```tsx
<PageScaffold title="资产配置" description="查看各链原生资产与合约资产。">
  <DataSurface title="资产列表">
    <DataTable<Asset>
      tableId="assets"
      loading={assetsQuery.isLoading}
      dataSource={assetsQuery.data ?? []}
      rowKey="id"
      pagination={{ pageSize: 10 }}
      columns={[
        { title: '链', dataIndex: 'chain_id', width: 160, render: value => chainMap.get(String(value)) ?? String(value) },
        { title: '符号', dataIndex: 'symbol', width: 120, ellipsis: { showTitle: true } },
        { title: '名称', dataIndex: 'name', width: 180, ellipsis: { showTitle: true } },
        { title: '类型', dataIndex: 'asset_type', width: 120 },
        { title: '合约地址', dataIndex: 'contract_address', width: 320, ellipsis: { showTitle: true }, className: 'table-cell-mono', render: value => value ? String(value) : '-' },
        { title: '精度', dataIndex: 'decimals', width: 90 },
        { title: '内置', dataIndex: 'is_builtin', width: 90, render: value => <Tag color={value ? 'blue' : 'grey'}>{value ? '是' : '否'}</Tag> },
      ]}
    />
  </DataSurface>
</PageScaffold>
```

- [ ] **Step 3: Migrate ProvidersPage**

Keep existing mutation/test logic. Change imports to remove `Card` and `Table`, and add:

```ts
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { PageScaffold } from '../components/PageScaffold';
```

Wrap with:

```tsx
<PageScaffold
  title="Provider 配置"
  description="管理链 RPC、WebSocket 与 REST Provider。"
  actions={<Button type="primary" onClick={openCreateModal}>新增 Provider</Button>}
>
  <DataSurface title="Provider 列表">
    <DataTable<Provider>
      tableId="providers"
      loading={providersQuery.isLoading}
      dataSource={providersQuery.data ?? []}
      rowKey="id"
      pagination={{ pageSize: 10 }}
      actionColumnKeys={['operations']}
      columns={[
        { title: '链', dataIndex: 'chain_id', width: 140, render: value => chainMap.get(String(value)) ?? String(value) },
        { title: '名称', dataIndex: 'name', width: 160, ellipsis: { showTitle: true } },
        { title: '类型', dataIndex: 'provider_type', width: 120 },
        { title: 'URL', dataIndex: 'base_url', width: 300, ellipsis: { showTitle: true }, className: 'table-cell-mono' },
        { title: '优先级', dataIndex: 'priority', width: 100 },
        { title: 'QPS', dataIndex: 'qps_limit', width: 100 },
        { title: '超时', dataIndex: 'timeout_ms', width: 110 },
        { title: '状态', dataIndex: 'status', width: 100, render: value => <Tag color={String(value) === 'active' ? 'green' : 'grey'}>{String(value)}</Tag> },
        {
          key: 'operations',
          title: '操作',
          width: 170,
          render: (_, provider) => {
            const testDisabled = !canTestProvider(provider);
            return (
              <Space>
                <Button size="small" onClick={() => openEditModal(provider)}>编辑</Button>
                <Button size="small" disabled={testDisabled} loading={testingProviderId === provider.id} onClick={() => handleTestProvider(provider)}>
                  {testDisabled ? '仅 EVM RPC 可测' : '测试'}
                </Button>
              </Space>
            );
          },
        },
      ]}
    />
  </DataSurface>
  <Modal title={editingProvider ? '编辑 Provider' : '新增 Provider'} visible={visible} onCancel={closeModal} footer={null}>
    <p className="form-help-text">当前仅支持 EVM/Base RPC 测试；WebSocket 与 REST API Provider 可保存，但暂不提供连通性测试。</p>
    <Form initValues={initialValues()} onSubmit={handleSubmit} labelPosition="left" labelWidth={110}>
      <Form.Select field="chain_id" label="链" rules={[{ required: true, message: '请选择链' }]}>
        {(chainsQuery.data ?? []).map(chain => <Form.Select.Option key={chain.id} value={chain.id}>{chain.name}</Form.Select.Option>)}
      </Form.Select>
      <Form.Select field="provider_type" label="类型" rules={[{ required: true, message: '请选择类型' }]}>
        <Form.Select.Option value="rpc">RPC</Form.Select.Option>
        <Form.Select.Option value="websocket">WebSocket</Form.Select.Option>
        <Form.Select.Option value="rest_api">REST API</Form.Select.Option>
      </Form.Select>
      <Form.Input field="name" label="名称" rules={[{ required: true, message: '请输入名称' }]} />
      <Form.Input field="base_url" label="Base URL" rules={[{ required: true, message: '请输入 URL' }]} />
      <Form.Input field="api_key_ref" label="API Key 引用" />
      <Form.InputNumber field="priority" label="优先级" min={1} rules={[{ required: true, message: '请输入优先级' }]} />
      <Form.InputNumber field="qps_limit" label="QPS 限制" min={1} rules={[{ required: true, message: '请输入 QPS 限制' }]} />
      <Form.InputNumber field="timeout_ms" label="超时毫秒" min={1} rules={[{ required: true, message: '请输入超时毫秒' }]} />
      <Form.Select field="status" label="状态" rules={[{ required: true, message: '请选择状态' }]}>
        <Form.Select.Option value="active">active</Form.Select.Option>
        <Form.Select.Option value="disabled">disabled</Form.Select.Option>
      </Form.Select>
      <Space>
        <Button htmlType="submit" type="primary" loading={mutation.isPending}>保存</Button>
        <Button onClick={closeModal}>取消</Button>
      </Space>
    </Form>
  </Modal>
</PageScaffold>
```

- [ ] **Step 4: Migrate AddressesPage**

Keep all existing form and mutation logic. Remove `Card` and `Table` imports, add `DataSurface`, `DataTable`, `PageScaffold`. Use `PageScaffold` action for the create button and `DataTable` with `tableId="addresses"`, `actionColumnKeys={['operations']}`. Rename the operation column from implicit render-only to:

```ts
{
  key: 'operations',
  title: '操作',
  width: 150,
  render: (_, record) => (
    <Space>
      <Button theme="borderless" onClick={() => openEditModal(record)}>编辑</Button>
      <Popconfirm title="确认删除该地址？" onConfirm={() => deleteMutation.mutate(record.id)}>
        <Button type="danger" theme="borderless">删除</Button>
      </Popconfirm>
    </Space>
  ),
}
```

- [ ] **Step 5: Verify and commit**

```bash
npm --prefix frontend run build
npm --prefix frontend run test:ui-regression
git add frontend/src/pages/ChainsPage.tsx frontend/src/pages/AssetsPage.tsx frontend/src/pages/ProvidersPage.tsx frontend/src/pages/AddressesPage.tsx
git commit -m "迁移基础配置页到新表格布局"
```

Expected: build passes. Regression may still fail for pages not migrated in Tasks 7 and 8.

---

## Task 7: Migrate events and notification rule pages

**Files:**

- Modify: `frontend/src/pages/EventsPage.tsx`
- Modify: `frontend/src/pages/NotificationRulesPage.tsx`
- Modify: `frontend/src/pages/InAppNotificationsPage.tsx`

- [ ] **Step 1: Migrate EventsPage wrappers**

Replace outer `<Space vertical ...>` with:

```tsx
<PageScaffold title="事件中心" description="查看链上监听事件、转账方向与资产变化。">
  {eventsQuery.isError ? (
    <Banner
      type="danger"
      title="事件列表加载失败"
      description={eventsQuery.error instanceof Error ? eventsQuery.error.message : '请求失败'}
    />
  ) : null}

  <FilterPanel title="事件筛选">
    <Form<FilterForm> layout="horizontal" onSubmit={handleFilterSubmit} labelPosition="left">
      {({ formApi }) => (
        <>
          <Form.Select field="chain_id" label="链" showClear placeholder="全部链" filter>
            {(chainsQuery.data ?? []).map(chain => <Form.Select.Option key={chain.id} value={chain.id}>{chain.name}</Form.Select.Option>)}
          </Form.Select>
          <Form.Select field="address_id" label="地址" showClear placeholder="全部地址" filter>
            {(addressesQuery.data ?? []).map(address => (
              <Form.Select.Option key={address.id} value={address.id}>
                {address.label ? `${address.label} / ${address.address}` : address.address}
              </Form.Select.Option>
            ))}
          </Form.Select>
          <Form.Select field="asset_id" label="资产" showClear placeholder="全部资产" filter>
            {(assetsQuery.data ?? []).map(asset => <Form.Select.Option key={asset.id} value={asset.id}>{asset.symbol}</Form.Select.Option>)}
          </Form.Select>
          <Form.Select field="event_type" label="事件类型" showClear placeholder="全部类型" optionList={eventTypeOptions} />
          <Form.Select field="direction" label="方向" showClear placeholder="全部方向" optionList={directionOptions} />
          <Form.Select field="is_transfer" label="是否转账" showClear placeholder="全部">
            <Form.Select.Option value="true">是</Form.Select.Option>
            <Form.Select.Option value="false">否</Form.Select.Option>
          </Form.Select>
          <Space>
            <Button htmlType="submit" type="primary">查询</Button>
            <Button onClick={() => resetFilters(formApi)}>重置</Button>
          </Space>
        </>
      )}
    </Form>
  </FilterPanel>

  <DataSurface title="开发模拟扫描">
    <Space vertical align="start">
      {devRouteUnavailable ? (
        <Banner
          type="warning"
          title="开发模拟扫描未启用"
          description="后端仅在 ENABLE_DEV_ROUTES=true 时开放 /api/dev/scan-address，用于本地调试。"
        />
      ) : null}
      <Space>
        <Select
          value={scanAddressId}
          onChange={value => setScanAddressId(value as string | undefined)}
          showClear
          filter
          placeholder="选择 EVM/Base 监听地址"
          style={{ width: 360 }}
          disabled={evmAddresses.length === 0}
        >
          {evmAddresses.map(address => (
            <Select.Option key={address.id} value={address.id}>
              {address.label ? `${address.label} / ${address.address}` : address.address}
            </Select.Option>
          ))}
        </Select>
        <Button
          type="primary"
          loading={scanMutation.isPending}
          disabled={Boolean(simulateDisabledReason)}
          onClick={() => scanAddressId && scanMutation.mutate(scanAddressId)}
        >
          生成模拟事件
        </Button>
      </Space>
      <Text type={simulateDisabledReason ? 'warning' : 'tertiary'}>
        {simulateDisabledReason ?? '仅支持 EVM / Base 地址；如接口返回 404，请设置 ENABLE_DEV_ROUTES=true 后重启后端。'}
      </Text>
    </Space>
  </DataSurface>

  <DataSurface title="事件流水">
    <DataTable<AddressEvent>
      tableId="events"
      loading={eventsQuery.isLoading}
      dataSource={eventsQuery.data ?? []}
      rowKey="id"
      pagination={{ pageSize: 10 }}
      columns={[
        { title: '时间', dataIndex: 'created_at', width: 180, render: value => new Date(String(value)).toLocaleString() },
        { title: '链', dataIndex: 'chain_id', width: 120, render: value => chainMap.get(String(value)) ?? String(value) },
        { title: '地址', dataIndex: 'address_id', width: 280, render: value => renderAddress(String(value)) },
        { title: '资产', dataIndex: 'asset_id', width: 100, render: value => assetMap.get(String(value)) ?? String(value) },
        { title: '类型', dataIndex: 'event_type', width: 150, render: value => <Tag>{String(value)}</Tag> },
        { title: '转账', dataIndex: 'is_transfer', width: 90, render: value => <Tag color={value ? 'green' : 'grey'}>{value ? '是' : '否'}</Tag> },
        { title: '方向', dataIndex: 'direction', width: 90 },
        { title: '金额', dataIndex: 'amount_decimal', width: 120, render: value => value ? String(value) : '-' },
        { title: '余额变化', dataIndex: 'balance_delta_raw', width: 140, render: value => value ? String(value) : '-' },
        { title: '确认数', dataIndex: 'confirmations', width: 90 },
        { title: '通知状态', width: 100, render: () => <Tag color="grey">待接入</Tag> },
        { title: '交易哈希', dataIndex: 'tx_hash', width: 260, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
      ]}
    />
  </DataSurface>
</PageScaffold>
```

Imports must remove `Card` and `Table`, then add:

```ts
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { FilterPanel } from '../components/FilterPanel';
import { PageScaffold } from '../components/PageScaffold';
```

- [ ] **Step 2: Migrate NotificationRulesPage**

Use `PageScaffold` with create action, `DataSurface`, and `DataTable<NotificationRule>` with `tableId="notification-rules"` and `actionColumnKeys={['operations']}`. Rename the operation column key to `operations`:

```ts
{
  key: 'operations',
  title: '操作',
  width: 150,
  render: (_, rule) => (
    <Space>
      <Button size="small" onClick={() => openEditModal(rule)}>编辑</Button>
      <Button size="small" type="danger" loading={deleteMutation.isPending} onClick={() => deleteMutation.mutate(rule.id)}>删除</Button>
    </Space>
  ),
}
```

- [ ] **Step 3: Migrate InAppNotificationsPage**

Use:

```tsx
<PageScaffold title="站内通知" description="查看并处理平台内通知。">
  {notificationsQuery.isError ? (
    <Banner
      type="danger"
      title="站内通知加载失败"
      description={notificationsQuery.error instanceof Error ? notificationsQuery.error.message : '请求失败'}
    />
  ) : null}
  <FilterPanel title="站内通知筛选">
    <Space>
      <Switch checked={unreadOnly} onChange={checked => setUnreadOnly(Boolean(checked))} />
      <span>只看未读</span>
      <Button onClick={() => notificationsQuery.refetch()}>刷新</Button>
    </Space>
  </FilterPanel>
  <DataSurface title="通知列表">
    <DataTable<InAppNotification>
      tableId="in-app-notifications"
      loading={notificationsQuery.isLoading}
      dataSource={notificationsQuery.data ?? []}
      rowKey="id"
      pagination={{ pageSize: 10 }}
      actionColumnKeys={['operations']}
      columns={[
        { title: '时间', dataIndex: 'created_at', width: 180, render: value => new Date(String(value)).toLocaleString() },
        { title: '标题', dataIndex: 'title', width: 180 },
        { title: '内容', dataIndex: 'body', width: 420, ellipsis: { showTitle: true } },
        {
          title: '状态',
          dataIndex: 'read_at',
          width: 100,
          render: value => value ? <Tag color="grey">已读</Tag> : <Tag color="red">未读</Tag>,
        },
        {
          key: 'operations',
          title: '操作',
          width: 120,
          render: (_, notification) => (
            <Button
              size="small"
              disabled={Boolean(notification.read_at)}
              loading={markReadMutation.isPending}
              onClick={() => markReadMutation.mutate(notification.id)}
            >
              标记已读
            </Button>
          ),
        },
      ]}
    />
  </DataSurface>
</PageScaffold>
```

- [ ] **Step 4: Verify and commit**

```bash
npm --prefix frontend run build
npm --prefix frontend run test:ui-regression
git add frontend/src/pages/EventsPage.tsx frontend/src/pages/NotificationRulesPage.tsx frontend/src/pages/InAppNotificationsPage.tsx
git commit -m "迁移事件与通知规则页面布局"
```

Expected: build passes. Regression may still fail for `SystemStatusPage.tsx` and `NotificationOperationsPage.tsx` until Task 8.

---

## Task 8: Redesign dashboard, status, notification operations, and login

**Files:**

- Modify: `frontend/src/App.tsx`
- Modify: `frontend/src/pages/SystemStatusPage.tsx`
- Modify: `frontend/src/pages/NotificationOperationsPage.tsx`
- Modify: `frontend/src/pages/LoginPage.tsx`
- Modify: `frontend/src/styles.css`

- [ ] **Step 1: Replace dashboard return in App**

In `renderPage`, replace the default dashboard return with:

```tsx
  return (
    <PageScaffold title="仪表盘" description="多链监听、事件与通知系统总览。">
      <Banner
        type="info"
        title="链上监控工作台"
        description="当前版本提供登录、链配置、资产配置、Provider 配置、监听地址管理、事件中心、通知规则、通知运维、站内通知和系统状态。"
      />
      <DataSurface title="API 健康状态">
        {healthQuery.isLoading ? <Text>正在检查 API...</Text> : null}
        {healthQuery.isError ? (
          <Space vertical align="start">
            <Tag color="red">API 不可用</Tag>
            <Text type="danger">{healthQuery.error instanceof Error ? healthQuery.error.message : '请求失败'}</Text>
            <Button onClick={() => healthQuery.refetch()}>重新检查</Button>
          </Space>
        ) : null}
        {healthQuery.data ? (
          <MetricGrid>
            <MetricCard title="API 状态" value={healthQuery.data.status} hint={healthQuery.data.service} tone="success" />
            <MetricCard title="监听模块" value="Ready" hint="进入左侧模块查看详情" />
            <MetricCard title="通知链路" value="Online" hint="通知运维提供详细积压状态" />
          </MetricGrid>
        ) : null}
      </DataSurface>
    </PageScaffold>
  );
```

Add imports in `App.tsx`:

```ts
import { DataSurface } from './components/DataSurface';
import { MetricCard, MetricGrid } from './components/MetricGrid';
import { PageScaffold } from './components/PageScaffold';
```

- [ ] **Step 2: Migrate SystemStatusPage**

Remove `Card`, `Col`, `Row`, and `Table` usage. Add `PageScaffold`, `DataSurface`, `DataTable`, `MetricGrid`, `MetricCard`. Convert metric rows to:

```tsx
<PageScaffold title="系统状态" description="扫描队列、通知队列、Provider 与服务心跳状态。">
  {statusQuery.isError ? (
    <Banner
      type="danger"
      title="系统状态加载失败"
      description={statusQuery.error instanceof Error ? statusQuery.error.message : '请求失败'}
    />
  ) : null}

  {status?.queues.queue_errors.length ? (
    <Banner
      type="warning"
      title="队列状态部分不可用"
      description={status.queues.queue_errors.join('；')}
    />
  ) : null}

  <DataSurface title="运维状态总览">
    <MetricGrid>
      <MetricCard title="Scan Queue" value={formatDepth(status?.queues.scan_queue_depth)} hint={status?.queues.scan_queue_key ?? '-'} />
      <MetricCard title="Notify Queue" value={formatDepth(status?.queues.notify_queue_depth)} hint={status?.queues.notify_queue_key ?? '-'} />
      <MetricCard title="Active 地址" value={status?.scans.active_addresses ?? 0} hint="status = active" />
      <MetricCard title="Due 地址" value={status?.scans.due_addresses ?? 0} hint="next_scan_at <= now" />
      <MetricCard title="24h 事件" value={status?.events.last_24h_total ?? 0} hint={`transfer ${status?.events.last_24h_transfers ?? 0}`} />
      <MetricCard title="Outbox Failed" value={status?.notifications.outbox.failed ?? 0} hint={`24h delivery failed ${status?.notifications.last_24h_failed ?? 0}`} tone="danger" />
      <MetricCard title="服务在线" value={status?.services.online ?? 0} hint={`stale ${status?.services.stale ?? 0}`} tone={status?.services.stale ? 'warning' : 'success'} />
    </MetricGrid>
  </DataSurface>

  <DataSurface title="扫描与通知摘要">
    <Space vertical align="start">
      <Text>生成时间：{formatTime(status?.generated_at)}</Text>
      <Text>最近扫描时间：{formatTime(status?.scans.last_scanned_at)}</Text>
      <Text>过期未扫描地址：{status?.scans.overdue_addresses ?? 0}</Text>
      <Text>24h 转账事件：{status?.events.last_24h_transfers ?? 0}</Text>
      <Text>24h 非转账事件：{status?.events.last_24h_non_transfers ?? 0}</Text>
      <Text>
        24h 通知：sent {status?.notifications.last_24h_sent ?? 0} / skipped {status?.notifications.last_24h_skipped ?? 0} / failed{' '}
        {status?.notifications.last_24h_failed ?? 0} / unread {status?.notifications.unread_in_app ?? 0}
      </Text>
      <Text>
        Outbox：pending {status?.notifications.outbox.pending ?? 0} / retryable {status?.notifications.outbox.retryable ?? 0} / processing{' '}
        {status?.notifications.outbox.processing ?? 0} / failed {status?.notifications.outbox.failed ?? 0} / stale{' '}
        {status?.notifications.outbox.stale_processing ?? 0} / next due {formatTime(status?.notifications.outbox.next_due_at)}
      </Text>
      <Text>Provider：active {status?.providers.active ?? 0} / inactive {status?.providers.inactive ?? 0}</Text>
      <Text>服务：online {status?.services.online ?? 0} / stale {status?.services.stale ?? 0}</Text>
    </Space>
  </DataSurface>

  <DataSurface title="服务心跳">
    <DataTable<ServiceHeartbeatStatusItem>
      tableId="service-heartbeats"
      dataSource={status?.services.items ?? []}
      rowKey={serviceHeartbeatRowKey}
      pagination={false}
      columns={[
        { title: '服务', dataIndex: 'service_name', width: 140 },
        { title: '状态', dataIndex: 'status', width: 120, render: (_value, record) => <Tag color={serviceStatusColor(record)}>{record.is_stale ? 'stale' : record.status}</Tag> },
        { title: '实例', dataIndex: 'instance_id', width: 140, render: value => shortInstanceId(String(value)) },
        { title: '启动时间', dataIndex: 'started_at', width: 190, render: value => formatTime(String(value)) },
        { title: '最后心跳', dataIndex: 'last_seen_at', width: 190, render: value => formatTime(String(value)) },
        { title: '超时阈值', dataIndex: 'stale_after_seconds', width: 110, render: value => `${String(value)}s` },
        { title: '运行信息', dataIndex: 'metadata', width: 160, render: value => metadataText(value as Record<string, unknown>) },
      ]}
    />
  </DataSurface>

  <DataSurface title="Provider 按链状态">
    <DataTable<ProviderChainStatus>
      tableId="provider-chain-status"
      dataSource={status?.providers.by_chain ?? []}
      rowKey="chain_id"
      pagination={false}
      columns={[
        { title: '链', dataIndex: 'chain_name', width: 180 },
        { title: 'Active', dataIndex: 'active', width: 120 },
        { title: 'Inactive', dataIndex: 'inactive', width: 120 },
      ]}
    />
  </DataSurface>

  <DataSurface title="Provider 明细">
    <DataTable<ProviderStatusItem>
      tableId="provider-status"
      dataSource={status?.providers.items ?? []}
      rowKey="id"
      pagination={{ pageSize: 10 }}
      columns={providerStatusColumns}
    />
  </DataSurface>
</PageScaffold>
```

- [ ] **Step 3: Migrate NotificationOperationsPage**

Remove `Card`, `Col`, `Row`, and `Table`; add `PageScaffold`, `FilterPanel`, `DataSurface`, `DataTable`, `MetricGrid`, `MetricCard`. Use `tableId="notification-outbox"` for the main outbox table with `actionColumnKeys={['operations']}`. In `OutboxDetailModal`, replace the deliveries `<Table>` with:

```tsx
<DataTable<NotificationDeliveryListItem>
  tableId="notification-deliveries"
  dataSource={detail.deliveries}
  rowKey="id"
  pagination={{ pageSize: 5 }}
  columns={[
    { title: '创建时间', dataIndex: 'created_at', width: 180, render: value => formatTime(String(value)) },
    { title: '渠道', dataIndex: 'channel_type', width: 110, render: value => value ? <Tag>{String(value)}</Tag> : '-' },
    { title: '状态', dataIndex: 'status', width: 110, render: value => <Tag color={deliveryStatusColor(String(value))}>{String(value)}</Tag> },
    { title: 'Attempt', dataIndex: 'attempt_count', width: 90 },
    { title: 'Rule ID', dataIndex: 'rule_id', width: 240, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
    { title: 'Channel ID', dataIndex: 'channel_id', width: 240, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
    { title: 'Idempotency Key', dataIndex: 'idempotency_key', width: 320, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
    { title: 'Provider Message', dataIndex: 'provider_message_id', width: 180, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
    { title: 'Provider Status', dataIndex: 'provider_status_code', width: 130, render: value => value ?? '-' },
    { title: 'Provider Response', dataIndex: 'provider_response', width: 260, ellipsis: { showTitle: true }, render: value => truncate(value ? String(value) : null, 120) },
    { title: 'Last Error', dataIndex: 'last_error', width: 260, ellipsis: { showTitle: true }, render: value => truncate(value ? String(value) : null, 120) },
  ]}
/>
```

- [ ] **Step 4: Redesign LoginPage**

Replace the JSX inside `LoginPage` return with:

```tsx
    <div className="login-page">
      <div className="login-orbit" />
      <Card className="login-card">
        <div className="login-brand-mark">CL</div>
        <Title heading={3}>Coin Listener</Title>
        <Text type="tertiary">链上监听、事件与通知运维控制台</Text>
        <Form onSubmit={handleSubmit} className="login-form">
          <Form.Input field="email" label="邮箱" initValue="admin@example.com" rules={[{ required: true, message: '请输入邮箱' }]} />
          <Form.Input field="password" label="密码" mode="password" rules={[{ required: true, message: '请输入密码' }]} />
          <Button htmlType="submit" type="primary" loading={loading} block>
            进入控制台
          </Button>
        </Form>
      </Card>
    </div>
```

- [ ] **Step 5: Add login CSS**

Replace login CSS block in `frontend/src/styles.css` with:

```css
.login-page {
  min-height: 100vh;
  position: relative;
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  background:
    radial-gradient(circle at 30% 20%, rgba(34, 211, 238, 0.28), transparent 28%),
    radial-gradient(circle at 70% 70%, rgba(59, 130, 246, 0.2), transparent 30%),
    #06111f;
}

.login-orbit {
  position: absolute;
  width: 520px;
  height: 520px;
  border: 1px solid rgba(125, 211, 252, 0.22);
  border-radius: 999px;
  box-shadow: inset 0 0 80px rgba(14, 165, 233, 0.1);
}

.login-card {
  width: 430px;
  z-index: 1;
  border: 1px solid rgba(148, 163, 184, 0.26);
  background: color-mix(in srgb, var(--semi-color-bg-1) 92%, transparent);
  backdrop-filter: blur(18px);
}

.login-brand-mark {
  width: 42px;
  height: 42px;
  margin-bottom: 16px;
  border-radius: 14px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: #06111f;
  background: linear-gradient(135deg, #22d3ee, #60a5fa);
  font-weight: 900;
}

.login-form {
  margin-top: 24px;
}
```

- [ ] **Step 6: Verify and commit**

```bash
npm --prefix frontend run build
npm --prefix frontend run test:ui-regression
git add frontend/src/App.tsx frontend/src/pages/SystemStatusPage.tsx frontend/src/pages/NotificationOperationsPage.tsx frontend/src/pages/LoginPage.tsx frontend/src/styles.css
git commit -m "重设计仪表盘状态页与登录页"
```

Expected: build passes. UI regression passes if all pages migrated.

---

## Task 9: Final verification and browser review

**Files:**

- Modify if needed: files touched by Tasks 1-8.

- [ ] **Step 1: Run static regression**

```bash
npm --prefix frontend run test:ui-regression
```

Expected: all tests pass, including Tailwind/Semi integration, theme persistence, design-system components, DataTable persistence, and page migration checks.

- [ ] **Step 2: Run production build**

```bash
npm --prefix frontend run build
```

Expected: exit code 0. Existing third-party warnings from `lottie-web` or bundle size are acceptable if no new errors appear.

- [ ] **Step 3: Manual browser checks**

Start dev server:

```bash
npm --prefix frontend run dev
```

Verify in browser:

- Login page renders with blockchain console branding.
- Header theme selector switches light/dark/system.
- Refresh preserves selected theme.
- Body receives `theme-mode="dark"` only in dark mode.
- All pages are reachable from sidebar.
- Wide pages do not create browser-level horizontal scroll.
- Table containers scroll internally when columns exceed width.
- Providers, addresses, notification rules, notification outbox, and in-app notifications keep operation column fixed on the right.
- Drag at least one table column width, refresh, and confirm width persists.
- Existing create/edit/delete/test/filter flows still open and submit as before.

- [ ] **Step 4: Inspect git diff**

```bash
git status --short
git diff --stat
git diff -- frontend/src frontend/package.json frontend/package-lock.json frontend/vite.config.ts
```

Expected: only frontend redesign files changed.

- [ ] **Step 5: Final commit**

If Step 4 shows uncommitted verification fixes:

```bash
git add frontend
git commit -m "完善区块链前端重设计验证"
```

If Step 4 shows a clean tree, skip this commit.

---

## Self-review notes

Spec coverage:

- Tailwind + Semi CSS Layer: Task 2.
- Light/dark/system theme persistence: Task 3.
- Design-system component layer: Tasks 3-5.
- DataTable overflow/fixed action/resizable/localStorage: Task 5 plus migrations in Tasks 6-8.
- Full page coverage: Tasks 6-8.
- Regression/build/manual verification: Task 9.

Parallelization:

- Do not start Tasks 6-8 until Task 5 is complete.
- After Task 5, Tasks 6, 7, and 8 can run in parallel agents because they touch mostly separate page files.
- Task 9 must run after all page migrations are integrated.
