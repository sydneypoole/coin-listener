import { describe, test } from 'node:test';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const src = resolve(here);

function readSource(relativePath: string) {
  try {
    return readFileSync(resolve(src, relativePath), 'utf8');
  } catch (error) {
    throw new Error(`Missing source file: ${relativePath}`, { cause: error });
  }
}

function expectContains(source: string, expected: string) {
  if (!source.includes(expected)) {
    throw new Error(`Expected source to contain: ${expected}`);
  }
}

function expectNotContains(source: string, unexpected: string) {
  if (source.includes(unexpected)) {
    throw new Error(`Expected source not to contain: ${unexpected}`);
  }
}

function expectMatches(source: string, pattern: RegExp, label: string) {
  if (!pattern.test(source)) {
    throw new Error(`Expected source to match ${label}: ${pattern}`);
  }
}

function expectNotMatches(source: string, pattern: RegExp, label: string) {
  if (pattern.test(source)) {
    throw new Error(`Expected source not to match ${label}: ${pattern}`);
  }
}

function expectOrdered(source: string, first: string, second: string) {
  const firstIndex = source.indexOf(first);
  const secondIndex = source.indexOf(second);

  if (firstIndex === -1) {
    throw new Error(`Expected source to contain ordered item: ${first}`);
  }

  if (secondIndex === -1) {
    throw new Error(`Expected source to contain ordered item: ${second}`);
  }

  if (firstIndex > secondIndex) {
    throw new Error(`Expected ${first} to appear before ${second}`);
  }
}

describe('frontend UI regressions', () => {
  test('provider management exposes multi-provider connectivity test controls', () => {
    const page = readSource('pages/ProvidersPage.tsx');
    const client = readSource('api/client.ts');
    const types = readSource('api/types.ts');

    expectContains(client, 'export function updateProvider');
    expectContains(client, 'export function testProvider');
    expectContains(types, 'chain_type: string');
    expectContains(types, 'provider_type: string');
    expectContains(page, 'editingProvider');
    expectContains(page, 'updateProvider');
    expectContains(page, 'testProvider');
    expectContains(page, 'providerTestSupported');
    expectContains(page, "chainType === 'evm' && provider.provider_type === 'rpc'");
    expectContains(page, "chainType === 'tron' && ['rest_api', 'rpc'].includes(provider.provider_type)");
    expectContains(page, "chainType === 'utxo' && provider.provider_type === 'rest_api'");
    expectContains(page, 'result.message');
    expectContains(page, '最新区块');
    expectContains(page, '暂不支持测试');
    expectContains(page, '支持 EVM RPC、TRON REST、BTC/UTXO REST Provider 连通性测试；WebSocket 暂不测试。');
    expectContains(page, 'value="websocket"');
    expectContains(page, 'value="rest_api"');
    expectContains(page, 'rules={[{ required: true, message: \'请输入优先级\' }]');
    expectContains(page, 'min={1}');
  });

  test('simulate event panel explains disabled states and dev-route dependency', () => {
    const page = readSource('pages/EventsPage.tsx');

    expectContains(page, 'ENABLE_DEV_ROUTES=true');
    expectContains(page, 'devRouteUnavailable');
    expectContains(page, '未找到 EVM/Base 地址');
    expectContains(page, 'ApiRequestError');
  });

  test('wide data tables use horizontal scroll and ellipsis to avoid page overflow', () => {
    const styles = readSource('styles.css');
    const pages = [
      'pages/ChainsPage.tsx',
      'pages/AssetsPage.tsx',
      'pages/AddressesPage.tsx',
      'pages/ProvidersPage.tsx',
    ];

    expectMatches(styles, /overflow(?:-x)?:\s*hidden/, 'page horizontal overflow guard');
    expectContains(styles, '.table-cell-mono');

    for (const pagePath of pages) {
      const page = readSource(pagePath);
      expectContains(page, 'scroll={{ x:');
      expectContains(page, 'ellipsis: { showTitle: true }');
    }
  });

  test('watched address API types include selected asset ids', () => {
    const types = readSource('api/types.ts');
    const client = readSource('api/client.ts');

    expectContains(types, 'asset_ids: string[]');
    expectContains(types, "export type CreateWatchedAddressRequest = Omit<WatchedAddress, 'id' | 'tenant_id'>");
    expectContains(client, 'createWatchedAddress');
    expectContains(client, 'updateWatchedAddress');
  });

  test('watched address form supports multi-chain asset selection', () => {
    const page = readSource('pages/AddressesPage.tsx');

    expectContains(page, 'listAssets');
    expectContains(page, 'chainRows');
    expectContains(page, 'assetOptionsForChain');
    expectContains(page, 'multiple');
    expectContains(page, 'asset_ids');
    expectContains(page, '监听资产');
    expectContains(page, '新增链配置');
    expectContains(page, '编辑监听地址');
    expectContains(page, 'updateWatchedAddress');
    expectContains(page, 'Promise.allSettled');
    expectContains(page, '部分链配置添加失败');
    expectContains(page, 'scroll={{ x: 1500 }}');
    expectContains(page, 'slice(0, 6)');
    expectContains(page, 'slice(-4)');
  });

  test('watched address row ids do not require browser crypto.randomUUID', () => {
    const page = readSource('pages/AddressesPage.tsx');

    expectContains(page, 'function createChainRowId()');
    expectContains(page, 'address-chain-row-');
    expectNotContains(page, 'crypto.randomUUID');
  });

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
    expectMatches(viteConfig, /semiTheming\(\s*\{\s*cssLayer:\s*true\s*\}\s*\)/, 'Semi CSS layer plugin');
    expectContains(semiLayer, '@layer theme, base, semi, utilities;');
    expectContains(tailwind, '@import "tailwindcss";');
    expectOrdered(main, "import './semi-layer.css';", "import './tailwind.css';");
    expectOrdered(main, "import './tailwind.css';", "import './styles.css';");
  });

  test('theme mode persists and uses semi dark mode contract', () => {
    const themeMode = readSource('themeMode.ts');
    const app = readSource('App.tsx');
    const shell = readSource('components/AppShell.tsx');
    const toggle = readSource('components/ThemeToggle.tsx');

    expectContains(themeMode, 'coin-listener:theme-mode');
    expectContains(themeMode, 'localStorage.getItem');
    expectContains(themeMode, 'localStorage.setItem');
    expectContains(themeMode, "document.body.setAttribute('theme-mode', 'dark')");
    expectContains(themeMode, "document.body.removeAttribute('theme-mode')");
    expectContains(themeMode, "matchMedia('(prefers-color-scheme: dark)')");
    expectContains(toggle, 'ThemeToggle');
    expectContains(shell, '<ThemeToggle');
    expectContains(app, '<AppShell');
  });

  test('light mode uses app tokens for readable main content while keeping brand sidebar dark', () => {
    const styles = readSource('styles.css');

    for (const expected of [
      '--app-shell-sidebar-bg',
      '--app-content-bg',
      '--app-card-bg',
      '--app-text-primary',
      '--app-text-secondary',
      '--app-border-subtle',
      "body[theme-mode='dark']",
      'background: var(--app-content-bg)',
      'color: var(--app-text-primary)',
      '.app-sider',
    ]) {
      expectContains(styles, expected);
    }

    const mainContentSelectors = [
      '.app-content',
      '.filter-panel',
      '.data-surface',
      '.notification-detail-card',
      '.detail-json',
    ];
    for (const selector of mainContentSelectors) {
      const selectorIndex = styles.indexOf(selector);
      if (selectorIndex === -1) {
        throw new Error(`Missing selector ${selector}`);
      }
      const block = styles.slice(selectorIndex, styles.indexOf('}', selectorIndex));
      expectNotContains(block, '#e5f7ff');
      expectNotContains(block, '#f8fbff');
      expectNotContains(block, 'rgba(226, 232, 240');
    }
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

  test('console redesign guardrails keep tokens shell and motion accessible', () => {
    const styles = readSource('styles.css');
    const shell = readSource('components/AppShell.tsx');
    const table = readSource('components/DataTable.tsx');
    const formModal = readSource('components/FormModal.tsx');

    for (const expected of [
      '--app-console-blue',
      '--app-console-cyan',
      '--app-surface-base',
      '--app-surface-raised',
      '--app-surface-glass',
      '--app-focus-ring',
      ':focus-visible',
      '::selection',
      '@media (prefers-reduced-motion: reduce)',
    ]) {
      expectContains(styles, expected);
    }

    for (const expected of [
      'shell-identity',
      'shell-meta',
      '<Sider className="app-sider">',
      '<Header className="app-header">',
      '<Content className="app-content">',
      '<ThemeToggle',
    ]) {
      expectContains(shell, expected);
    }

    for (const expected of [
      'coin-listener:data-table-widths:',
      'onResizeStop',
      'resizable=',
      'fixed: isActionColumn ? (\'right\' as const)',
    ]) {
      expectContains(table, expected);
    }

    for (const expected of [
      'medium: 720',
      'large: 920',
      'wide: 1120',
      'calc(100vw - 32px)',
      'calc(100vh - 220px)',
      'form-modal',
    ]) {
      expectContains(formModal, expected);
    }
  });

  test('data table wrapper persists resized widths and fixes action columns', () => {
    const table = readSource('components/DataTable.tsx');

    expectContains(table, 'coin-listener:data-table-widths:');
    expectContains(table, 'localStorage');
    expectContains(table, 'onResizeStop');
    expectMatches(table, /fixed:\s*isActionColumn\s*\?\s*\(['"]right['"] as const\)/, 'fixed right action column');
    expectContains(table, 'resizable=');
    expectContains(table, 'data-table-surface');
  });

  test('provider health table columns have stable unique keys', () => {
    const page = readSource('pages/SystemStatusPage.tsx');

    for (const key of [
      "key: 'health-status'",
      "key: 'health-failures'",
      "key: 'health-last-success'",
      "key: 'health-last-failure'",
      "key: 'health-disabled-until'",
      "key: 'health-last-error'",
    ]) {
      expectContains(page, key);
    }
  });

  test('service heartbeat status renders Chinese labels', () => {
    const page = readSource('pages/SystemStatusPage.tsx');

    expectContains(page, 'function serviceStatusText');
    expectContains(page, "return '离线';");
    expectContains(page, "if (item.status === 'online') return '在线';");
    expectContains(page, 'serviceStatusText(record)');
    expectContains(page, 'hint={`离线 ${status?.services.stale ?? 0}`}');
    expectContains(page, '在线 {status?.services.online ?? 0} / 离线 {status?.services.stale ?? 0}');
    expectNotContains(page, "record.is_stale ? 'stale' : record.status");
    expectNotContains(page, 'stale {status?.services.stale ?? 0}');
  });

  test('business pages use DataTable for table overflow control', () => {
    const pagePaths = [
      'pages/ChainsPage.tsx',
      'pages/AssetsPage.tsx',
      'pages/ProvidersPage.tsx',
      'pages/AddressesPage.tsx',
      'pages/EventsPage.tsx',
      'pages/SystemStatusPage.tsx',
      'pages/ScanAuditPage.tsx',
      'pages/NotificationRulesPage.tsx',
      'pages/NotificationOperationsPage.tsx',
      'pages/InAppNotificationsPage.tsx',
    ];

    for (const pagePath of pagePaths) {
      const page = readSource(pagePath);
      expectContains(page, 'DataTable');
      expectContains(page, 'tableId=');
      expectNotMatches(
        page,
        /import\s*\{[^}]*\bTable\b[^}]*\}\s*from\s*['"]@douyinfe\/semi-ui['"]/,
        `${pagePath} Semi Table import`,
      );
      expectNotMatches(page, /<Table(?:\s|>|\/|<)/, `${pagePath} direct Semi Table JSX`);
    }
  });

  test('form modal sizes are centralized and used by dense forms', () => {
    const formModal = readSource('components/FormModal.tsx');
    const addressesPage = readSource('pages/AddressesPage.tsx');
    const notificationRulesPage = readSource('pages/NotificationRulesPage.tsx');

    expectContains(formModal, 'medium: 720');
    expectContains(formModal, 'large: 920');
    expectContains(formModal, 'wide: 1120');
    expectContains(formModal, "'footer' | 'width' | 'size'");
    expectContains(formModal, 'calc(100vw - 32px)');
    expectContains(addressesPage, '<FormModal');
    expectContains(addressesPage, 'size="large"');
    expectContains(notificationRulesPage, '<FormModal');
    expectContains(notificationRulesPage, 'size="large"');
  });

  test('telegram bot management page is wired into navigation', () => {
    const app = readSource('App.tsx');
    const page = readSource('pages/TelegramBotsPage.tsx');

    expectContains(app, "'telegram-bots'");
    expectContains(app, 'TelegramBotsPage');
    expectContains(app, 'TG机器人');
    expectContains(page, 'listTelegramBots');
    expectContains(page, 'createTelegramBot');
    expectContains(page, 'updateTelegramBot');
    expectContains(page, 'deleteTelegramBot');
    expectContains(page, 'verifyTelegramBot');
    expectContains(page, 'token_preview');
    expectContains(page, 'DataTable');
    expectContains(page, 'tableId="telegram-bots"');
  });

  test('telegram bot management exposes global and bot proxy configuration', () => {
    const types = readSource('api/types.ts');
    const client = readSource('api/client.ts');
    const page = readSource('pages/TelegramBotsPage.tsx');
    const combined = `${types}\n${client}\n${page}`;

    for (const expected of [
      'export type TelegramSettings',
      'proxy_url_preview?: string | null',
      'proxy_source: string',
      'getTelegramSettings',
      'updateTelegramSettings',
      'Telegram 全局代理',
      '代理来源',
      'proxy_mode',
      'proxy_url',
      '使用全局代理',
      '此机器人单独配置代理',
    ]) {
      expectContains(combined, expected);
    }
  });

  test('notification channel management page and rule quick actions exist', () => {
    const app = readSource('App.tsx');
    const page = readSource('pages/NotificationChannelsPage.tsx');
    const rules = readSource('pages/NotificationRulesPage.tsx');

    expectContains(app, "'notification-channels'");
    expectContains(app, 'NotificationChannelsPage');
    expectContains(app, '通知渠道');
    expectContains(page, 'listNotificationChannels');
    expectContains(page, 'listTelegramBots');
    expectContains(page, 'updateNotificationChannel');
    expectContains(page, 'deleteNotificationChannel');
    expectContains(page, 'verifyNotificationChannel');
    expectContains(page, 'testNotificationChannel');
    expectContains(page, 'tableId="notification-channels"');
    expectContains(page, 'isPlainConfigObject');
    expectContains(page, '配置 JSON 必须是对象');
    expectContains(page, 'safeChannelConfig');
    expectContains(page, 'TelegramBindingPanel');
    expectContains(page, 'handleTelegramBound');
    expectNotContains(page, 'label="Chat ID"');
    expectContains(rules, '新建渠道');
    expectContains(rules, '刷新渠道');
    expectContains(rules, 'quickCreatedChannelId');
    expectContains(rules, 'telegramBotsQuery');
    expectContains(rules, 'TelegramBindingPanel');
    expectContains(rules, 'handleQuickTelegramBound');
    expectNotContains(rules, 'label="Chat ID"');
  });

  test('notification and telegram API contracts are exposed to frontend', () => {
    const types = readSource('api/types.ts');
    const client = readSource('api/client.ts');

    for (const expected of [
      'export type TelegramBot',
      'export type CreateTelegramBotRequest',
      'export type UpdateTelegramBotRequest',
      'export type NotificationChannelTestResponse',
      'export type WatchedAddressImportChainConfig',
      'export type WatchedAddressImportTask',
      'export type CreateWatchedAddressImportRequest',
      'export type WatchedAddressImportErrorRow',
      'chain_configs: WatchedAddressImportChainConfig[]',
      'chain_name?: string | null',
    ]) {
      expectContains(types, expected);
    }

    for (const expected of [
      'listTelegramBots',
      'createTelegramBot',
      'updateTelegramBot',
      'deleteTelegramBot',
      'verifyTelegramBot',
      'updateNotificationChannel',
      'deleteNotificationChannel',
      'verifyNotificationChannel',
      'testNotificationChannel',
      'createWatchedAddressImport',
      'getWatchedAddressImport',
      'listWatchedAddressImportErrors',
      'cancelWatchedAddressImport',
    ]) {
      expectContains(client, expected);
    }
  });

  test('telegram binding API contracts and panel are exposed to frontend', () => {
    const types = readSource('api/types.ts');
    const client = readSource('api/client.ts');
    const panel = readSource('components/TelegramBindingPanel.tsx');
    const combined = `${types}\n${client}\n${panel}`;

    for (const expected of [
      'export type TelegramBindingRequest',
      'export type CreateTelegramBindingRequest',
      'createTelegramBinding',
      'getTelegramBinding',
      'cancelTelegramBinding',
      'TelegramBindingPanel',
      '生成绑定码',
      '/start',
      '群聊',
      'short_code',
      'deep_link_url',
      "binding.status === 'expired'",
      '绑定码已过期',
    ]) {
      expectContains(combined, expected);
    }
  });

  test('watched address page supports backend task batch import', () => {
    const page = readSource('pages/AddressesPage.tsx');

    for (const expected of [
      '批量添加',
      'parseAddressImportInput',
      'createWatchedAddressImport',
      'getWatchedAddressImport',
      'listWatchedAddressImportErrors',
      'cancelWatchedAddressImport',
      'tableId="address-import-preview"',
      'tableId="address-import-errors"',
      'importTaskId',
      'batchChainRows',
      'addBatchChainRow',
      'removeBatchChainRow',
      'updateBatchChainRow',
      'normalizedBatchChainConfigs',
      'chain_configs',
      '不能重复选择链',
      '预计创建尝试',
      '导入进度（按地址-链尝试计数）',
      '总尝试',
      'chain_name',
      '`${row.row_number}-${row.chain_id}`',
      "queryClient.invalidateQueries({ queryKey: ['address-import-errors', importTaskId] })",
    ]) {
      expectContains(page, expected);
    }

    expectNotContains(page, 'handleBatchChainChange');
    expectNotContains(page, "setValue('asset_ids', [])");
  });

  test('address import parser supports line and CSV input', async () => {
    const { parseAddressImportInput } = await import('./addressImport.ts');

    const lineResult = parseAddressImportInput('0x0000000000000000000000000000000000000001\n\n0x0000000000000000000000000000000000000002');
    if (lineResult.rows.length !== 2) throw new Error('line input should produce two rows');
    if (lineResult.rows[0].row_number !== 1) throw new Error('first line row number mismatch');
    if (lineResult.rows[1].row_number !== 3) throw new Error('second line row number mismatch');
    if (lineResult.rows[0].address !== '0x0000000000000000000000000000000000000001') throw new Error('line address mismatch');

    const csvResult = parseAddressImportInput('address,label,priority\n0x0000000000000000000000000000000000000003,Hot,critical');
    if (csvResult.rows[0].row_number !== 2) throw new Error('CSV row number mismatch');
    if (csvResult.rows[0].label !== 'Hot') throw new Error('CSV label mismatch');
    if (csvResult.rows[0].priority !== 'critical') throw new Error('CSV priority mismatch');
  });

  test('address import parser reports duplicates and unknown CSV fields', async () => {
    const { parseAddressImportInput } = await import('./addressImport.ts');

    const result = parseAddressImportInput('address,unknown\n0x0000000000000000000000000000000000000004,x\n0x0000000000000000000000000000000000000004,y');

    if (!result.warnings.some(warning => warning.includes('unknown'))) throw new Error('unknown CSV field warning missing');
    if (!result.rows.some(row => row.error === '重复地址')) throw new Error('duplicate row error missing');

    const chainFieldResult = parseAddressImportInput('address,chain_id\n0x0000000000000000000000000000000000000005,base');
    if (!chainFieldResult.warnings.some(warning => warning.includes('chain_id'))) throw new Error('chain_id CSV field should remain unknown');
  });

  test('scan audit API contracts are exposed to frontend', () => {
    const types = readSource('api/types.ts');
    const client = readSource('api/client.ts');

    for (const expected of [
      'export type ScanRunStatus',
      'export type ScanAddressTask',
      'export type ScanRunListItem',
      'export type ScanRunDetail',
      'export type ScanRunQuery',
      'export type ScanRunListResponse',
      'export type RetryScanRunResponse',
      'last_success_at?: string | null',
      'last_failed_at?: string | null',
      'last_24h_success: number',
      'last_24h_failed: number',
      'recent_runs: ScanRunListItem[]',
    ]) {
      expectContains(types, expected);
    }

    for (const expected of [
      'listScanRuns',
      'getScanRun',
      'retryScanRun',
      '/api/scan-runs',
      '/api/scan-runs/${id}',
      '/api/scan-runs/${id}/retry',
    ]) {
      expectContains(client, expected);
    }
  });

  test('custody account mode is wired into frontend contracts and navigation', () => {
    const types = readSource('api/types.ts');
    const client = readSource('api/client.ts');
    const app = readSource('App.tsx');
    const page = readSource('pages/CustodyAccountsPage.tsx');
    const combined = `${types}\n${client}\n${app}\n${page}`;

    for (const expected of [
      'export type CustodyAccount',
      'export type CustodyAccountAssignment',
      'export type CreateCustodyAccountRequest',
      'export type AssignCustodyAccountRequest',
      'export type AssignCustodyAccountResponse',
      'export type CustodyAccountChainConfigRequest',
      'export type CustodyAccountChainConfig',
      'export type CustodyAssignmentWatchedAddress',
      'chain_configs',
      'watched_addresses',
      'custodyChainRows',
      'assignChainRows',
      'assignSource',
      "assignSource === 'user'",
      'setAssignSource',
      'addCustodyChainRow',
      'assetOptionsForChain',
      'selectedAssetSymbols',
      '每条链至少选择一个资产',
      '不能重复选择链',
      '监听链配置',
      'multiple',
      'listCustodyAccounts',
      'createCustodyAccount',
      'assignCustodyAccount',
      'listCustodyAssignments',
      'releaseCustodyAssignment',
      '/api/custody/accounts',
      '/api/custody/accounts/assign',
      '/api/custody/assignments',
      '/api/custody/assignments/${id}/release',
      "'custody-accounts'",
      'CustodyAccountsPage',
      '托管账户',
      'tableId="custody-accounts"',
      'tableId="custody-assignments"',
      '新增托管地址',
      '申请托管地址',
      '自动添加监听',
      '不能重复申请',
      'validateAssignCustodyAccountForm',
      '用户自带地址需填写地址',
      '系统地址池地址',
      '状态固定为 available',
      '释放',
      "queryClient.invalidateQueries({ queryKey: ['custody-accounts'] })",
      "queryClient.invalidateQueries({ queryKey: ['custody-assignments'] })",
    ]) {
      expectContains(combined, expected);
    }
  });

  test('scan audit page is wired into navigation with Chinese statuses and retry rules', () => {
    const app = readSource('App.tsx');
    const page = readSource('pages/ScanAuditPage.tsx');

    expectContains(app, "'scan-audit'");
    expectContains(app, 'ScanAuditPage');
    expectContains(app, '扫描审计');

    for (const expected of [
      'listScanRuns',
      'getScanRun',
      'retryScanRun',
      'tableId="scan-runs"',
      '扫描中',
      '成功',
      '失败',
      '跳过：锁占用',
      '不支持',
      'retryableScanRun(row.status) ? ',
      'JSON.stringify(detail.metadata, null, 2)',
      'queryClient.invalidateQueries({ queryKey: [\'scan-runs\'] })',
      'queryClient.invalidateQueries({ queryKey: [\'system-status\'] })',
      'function handlePageChange(page: number)',
      'const pageSize = filters.limit ?? 50',
      'offset: (page - 1) * pageSize',
      'pagination={{ pageSize: filters.limit ?? 50, currentPage: currentPage(filters), onPageChange: handlePageChange }}',
    ]) {
      expectContains(page, expected);
    }
  });

  test('system status page shows scan audit health summary and recent runs', () => {
    const page = readSource('pages/SystemStatusPage.tsx');

    for (const expected of [
      'last_24h_success',
      'last_24h_failed',
      'last_success_at',
      'last_failed_at',
      'recent_runs',
      '扫描成功',
      '扫描失败',
      '最近扫描记录',
      'tableId="system-recent-scan-runs"',
      'scanRunStatusText',
      '跳过：锁占用',
    ]) {
      expectContains(page, expected);
    }
  });
});
