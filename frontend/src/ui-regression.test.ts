import { describe, test } from 'node:test';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const src = resolve(here);

function readSource(relativePath: string) {
  return readFileSync(resolve(src, relativePath), 'utf8');
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

describe('frontend UI regressions', () => {
  test('provider management exposes edit and connectivity test controls', () => {
    const page = readSource('pages/ProvidersPage.tsx');
    const client = readSource('api/client.ts');

    expectContains(client, 'export function updateProvider');
    expectContains(client, 'export function testProvider');
    expectContains(page, 'editingProvider');
    expectContains(page, 'updateProvider');
    expectContains(page, 'testProvider');
    expectContains(page, 'Provider 测试成功');
    expectContains(page, '编辑');
    expectContains(page, '测试');
    expectContains(page, 'testingProviderId');
    expectContains(page, "provider.provider_type === 'rpc'");
    expectContains(page, '当前仅支持 EVM/Base RPC 测试');
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

    expectContains(styles, 'overflow-x: hidden');
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
});
