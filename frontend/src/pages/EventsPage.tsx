import { useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Banner, Button, Form, Input, Select, Space, Tag, Toast, Typography } from '@douyinfe/semi-ui';
import { ApiRequestError, listAssets, listChains, listEvents, listWatchedAddresses, rescanEvmTransaction, scanAddress } from '../api/client';
import type { AddressEvent, EventQuery, EvmTransactionRescanResponse } from '../api/types';
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { FilterPanel } from '../components/FilterPanel';
import { PageScaffold } from '../components/PageScaffold';

const { Text } = Typography;

type FilterForm = {
  chain_id?: string;
  address_id?: string;
  asset_id?: string;
  event_type?: string;
  direction?: string;
  is_transfer?: string;
};

const eventTypeOptions = [
  { label: 'transfer', value: 'transfer' },
  { label: 'balance_change', value: 'balance_change' },
  { label: 'fee_only_change', value: 'fee_only_change' },
  { label: 'contract_interaction', value: 'contract_interaction' },
  { label: 'unknown', value: 'unknown' },
];

const directionOptions = [
  { label: 'in', value: 'in' },
  { label: 'out', value: 'out' },
  { label: 'self', value: 'self' },
  { label: 'unknown', value: 'unknown' },
];

export function EventsPage() {
  const [filters, setFilters] = useState<EventQuery>({});
  const [scanAddressId, setScanAddressId] = useState<string>();
  const [rescanChainId, setRescanChainId] = useState<string>();
  const [rescanTxHash, setRescanTxHash] = useState('');
  const [rescanResult, setRescanResult] = useState<EvmTransactionRescanResponse>();
  const queryClient = useQueryClient();

  const eventsQuery = useQuery({
    queryKey: ['events', filters],
    queryFn: () => listEvents(filters),
  });
  const chainsQuery = useQuery({ queryKey: ['chains'], queryFn: listChains });
  const assetsQuery = useQuery({ queryKey: ['assets'], queryFn: listAssets });
  const addressesQuery = useQuery({ queryKey: ['addresses'], queryFn: listWatchedAddresses });

  const chainMap = useMemo(() => new Map((chainsQuery.data ?? []).map(chain => [chain.id, chain.name])), [chainsQuery.data]);
  const evmChainIds = useMemo(
    () => new Set((chainsQuery.data ?? []).filter(chain => chain.chain_type === 'evm').map(chain => chain.id)),
    [chainsQuery.data],
  );
  const evmChains = useMemo(
    () => (chainsQuery.data ?? []).filter(chain => chain.chain_type === 'evm'),
    [chainsQuery.data],
  );
  const evmAddresses = useMemo(
    () => (addressesQuery.data ?? []).filter(address => evmChainIds.has(address.chain_id)),
    [addressesQuery.data, evmChainIds],
  );
  const assetMap = useMemo(() => new Map((assetsQuery.data ?? []).map(asset => [asset.id, asset.symbol])), [assetsQuery.data]);
  const addressMap = useMemo(() => new Map((addressesQuery.data ?? []).map(address => [address.id, address])), [addressesQuery.data]);
  const [devRouteUnavailable, setDevRouteUnavailable] = useState(false);
  const hasLoadedSimulationInputs = !chainsQuery.isLoading && !addressesQuery.isLoading;
  const hasAnyAddress = (addressesQuery.data ?? []).length > 0;
  const simulateDisabledReason = !hasLoadedSimulationInputs
    ? '正在加载地址'
    : !hasAnyAddress
      ? '请先创建监听地址'
      : evmAddresses.length === 0
        ? '未找到 EVM/Base 地址，开发模拟扫描仅支持 EVM/Base'
        : !scanAddressId
          ? '请选择一个 EVM/Base 监听地址'
          : undefined;

  const scanMutation = useMutation({
    mutationFn: scanAddress,
    onSuccess: () => {
      setDevRouteUnavailable(false);
      Toast.success('已生成模拟事件');
      queryClient.invalidateQueries({ queryKey: ['events'] });
    },
    onError: error => {
      if (error instanceof ApiRequestError && error.status === 404) {
        setDevRouteUnavailable(true);
        Toast.error('开发模拟扫描未启用，请设置 ENABLE_DEV_ROUTES=true 后重启服务');
        return;
      }
      Toast.error(error instanceof Error ? error.message : '模拟扫描失败');
    },
  });

  const rescanMutation = useMutation({
    mutationFn: rescanEvmTransaction,
    onSuccess: result => {
      setRescanResult(result);
      Toast.success(`交易回填完成：新增 ${result.summary.inserted_event_count} 条，跳过 ${result.summary.skipped_event_count} 条`);
      queryClient.invalidateQueries({ queryKey: ['events'] });
    },
    onError: error => {
      Toast.error(error instanceof Error ? error.message : '交易回填失败');
    },
  });

  const normalizedRescanTxHash = rescanTxHash.trim();
  const rescanDisabledReason = !rescanChainId
    ? '请选择 EVM 链'
    : !/^0x[0-9a-fA-F]{64}$/.test(normalizedRescanTxHash)
      ? '请输入 0x 开头的 32 字节交易哈希'
      : undefined;

  function handleFilterSubmit(values: Record<string, unknown>) {
    const form = values as FilterForm;
    setFilters({
      chain_id: form.chain_id,
      address_id: form.address_id,
      asset_id: form.asset_id,
      event_type: form.event_type,
      direction: form.direction,
      is_transfer: form.is_transfer === undefined ? undefined : form.is_transfer === 'true',
    });
  }

  function resetFilters(formApi: { reset: () => void }) {
    formApi.reset();
    setFilters({});
  }

  function renderAddress(addressId: string) {
    const address = addressMap.get(addressId);
    if (!address) return addressId;
    return address.label ? `${address.label} / ${address.address}` : address.address;
  }

  function renderEventType(event: AddressEvent) {
    if (event.event_type === 'fee_only_change') return <Tag color="orange">Gas 消耗</Tag>;
    if (event.event_type === 'balance_change') return <Tag color="blue">余额变化</Tag>;
    if (event.event_type === 'transfer') return <Tag color="green">转账</Tag>;
    return <Tag>{event.event_type}</Tag>;
  }

  function renderEventSource(event: AddressEvent) {
    const source = typeof event.metadata?.source === 'string' ? event.metadata.source : undefined;
    if (source === 'evm_tx_rescan') return <Tag color="purple">交易回填</Tag>;
    if (source === 'evm_erc20_transfer_log') return <Tag color="green">EVM 日志</Tag>;
    if (source === 'evm_balance_snapshot') return <Tag color="blue">余额快照</Tag>;
    if (source === 'mock_evm_transfer') return <Tag color="grey">模拟</Tag>;
    return <Tag color="grey">-</Tag>;
  }

  return (
    <PageScaffold title="事件中心" description="按链、地址、资产和事件类型检索链上活动账本。">
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

      <FilterPanel title="开发模拟扫描">
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
      </FilterPanel>

      <FilterPanel title="EVM 交易回填">
        <Space vertical align="start">
          <Space>
            <Select
              value={rescanChainId}
              onChange={value => setRescanChainId(value as string | undefined)}
              showClear
              filter
              placeholder="选择 EVM 链"
              style={{ width: 220 }}
              disabled={evmChains.length === 0}
            >
              {evmChains.map(chain => (
                <Select.Option key={chain.id} value={chain.id}>{chain.name}</Select.Option>
              ))}
            </Select>
            <Input
              value={rescanTxHash}
              onChange={value => setRescanTxHash(String(value))}
              placeholder="0x...交易哈希"
              style={{ width: 520 }}
            />
            <Button
              type="primary"
              loading={rescanMutation.isPending}
              disabled={Boolean(rescanDisabledReason)}
              onClick={() => rescanChainId && rescanMutation.mutate({ chain_id: rescanChainId, tx_hash: normalizedRescanTxHash })}
            >
              重扫交易
            </Button>
          </Space>
          <Text type={rescanDisabledReason ? 'warning' : 'tertiary'}>
            {rescanDisabledReason ?? '按 tx hash 拉取交易和 receipt，解析 Transfer logs 并为命中的监听地址回填事件。'}
          </Text>
          {rescanResult ? (
            <Banner
              type="info"
              title={`回填结果：新增 ${rescanResult.summary.inserted_event_count} 条，跳过 ${rescanResult.summary.skipped_event_count} 条`}
              description={`Token transfers: ${rescanResult.summary.token_transfer_count}; native value raw: ${rescanResult.summary.native_value_raw}; block: ${rescanResult.summary.block_number}`}
            />
          ) : null}
        </Space>
      </FilterPanel>

      <DataSurface title="事件中心">
        <DataTable<AddressEvent>
          tableId="events"
          loading={eventsQuery.isLoading}
          dataSource={eventsQuery.data ?? []}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          scroll={{ x: 1400 }}
          columns={[
            { title: '时间', dataIndex: 'created_at', width: 180, render: value => new Date(String(value)).toLocaleString() },
            { title: '链', dataIndex: 'chain_id', width: 120, render: value => chainMap.get(String(value)) ?? String(value) },
            { title: '地址', dataIndex: 'address_id', width: 280, ellipsis: { showTitle: true }, className: 'table-cell-mono', render: value => renderAddress(String(value)) },
            { title: '资产', dataIndex: 'asset_id', width: 100, render: value => assetMap.get(String(value)) ?? String(value) },
            { title: '类型', width: 150, render: (_, event) => renderEventType(event) },
            { title: '来源', width: 110, render: (_, event) => renderEventSource(event) },
            { title: '转账', dataIndex: 'is_transfer', width: 90, render: value => <Tag color={value ? 'green' : 'grey'}>{value ? '是' : '否'}</Tag> },
            { title: '方向', dataIndex: 'direction', width: 90 },
            { title: '金额', dataIndex: 'amount_decimal', width: 120, render: value => value ? String(value) : '-' },
            { title: '余额变化', dataIndex: 'balance_delta_raw', width: 140, render: value => value ? String(value) : '-' },
            { title: '确认数', dataIndex: 'confirmations', width: 90 },
            { title: '通知状态', width: 100, render: () => <Tag color="grey">待接入</Tag> },
            { title: '交易哈希', dataIndex: 'tx_hash', width: 260, ellipsis: { showTitle: true }, className: 'table-cell-mono', render: value => value ? String(value) : '-' },
          ]}
        />
      </DataSurface>
    </PageScaffold>
  );
}
