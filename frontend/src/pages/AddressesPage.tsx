import { useEffect, useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Banner, Button, Form, Popconfirm, Progress, Select, Space, Tag, Toast } from '@douyinfe/semi-ui';
import { parseAddressImportInput } from '../addressImport';
import {
  cancelWatchedAddressImport,
  createWatchedAddress,
  createWatchedAddressImport,
  deleteWatchedAddress,
  getWatchedAddressImport,
  listAssets,
  listChains,
  listWatchedAddressImportErrors,
  listWatchedAddresses,
  updateWatchedAddress,
} from '../api/client';
import type { Asset, CreateWatchedAddressRequest, WatchedAddress, WatchedAddressImportErrorRow } from '../api/types';
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { FormModal } from '../components/FormModal';
import { PageScaffold } from '../components/PageScaffold';

type ChainRow = {
  id: string;
  chain_id: string;
  asset_ids: string[];
};

type BatchImportForm = Record<string, unknown>;

const terminalImportStatuses = ['completed', 'failed', 'cancelled'];

let chainRowIdSequence = 0;

function createChainRowId() {
  chainRowIdSequence += 1;
  return `address-chain-row-${chainRowIdSequence}`;
}

function importProgress(task: { total_rows: number; processed_rows: number }) {
  if (task.total_rows <= 0) return 0;
  return Math.round((task.processed_rows / task.total_rows) * 100);
}

function isTerminalImportStatus(status?: string) {
  return Boolean(status) && terminalImportStatuses.includes(status ?? '');
}

export function AddressesPage() {
  const [visible, setVisible] = useState(false);
  const [batchVisible, setBatchVisible] = useState(false);
  const [batchInput, setBatchInput] = useState('');
  const [importTaskId, setImportTaskId] = useState<string | null>(null);
  const [editingAddress, setEditingAddress] = useState<WatchedAddress | null>(null);
  const [chainRows, setChainRows] = useState<ChainRow[]>([emptyChainRow()]);
  const [batchChainRows, setBatchChainRows] = useState<ChainRow[]>([emptyChainRow()]);
  const queryClient = useQueryClient();
  const addressesQuery = useQuery({ queryKey: ['addresses'], queryFn: listWatchedAddresses });
  const chainsQuery = useQuery({ queryKey: ['chains'], queryFn: listChains });
  const assetsQuery = useQuery({ queryKey: ['assets'], queryFn: listAssets });
  const importTaskQuery = useQuery({
    queryKey: ['address-import', importTaskId],
    queryFn: () => getWatchedAddressImport(importTaskId ?? ''),
    enabled: Boolean(importTaskId),
    refetchInterval: query => {
      const status = query.state.data?.status;
      return status === 'pending' || status === 'running' ? 2000 : false;
    },
  });
  const importErrorsQuery = useQuery({
    queryKey: ['address-import-errors', importTaskId],
    queryFn: () => listWatchedAddressImportErrors(importTaskId ?? ''),
    enabled: Boolean(importTaskId) && isTerminalImportStatus(importTaskQuery.data?.status),
  });
  const chainMap = useMemo(() => new Map((chainsQuery.data ?? []).map(chain => [chain.id, chain.name])), [chainsQuery.data]);
  const assetMap = useMemo(() => new Map((assetsQuery.data ?? []).map(asset => [asset.id, asset])), [assetsQuery.data]);
  const parsedImport = useMemo(() => parseAddressImportInput(batchInput), [batchInput]);
  const importableRows = parsedImport.rows.filter(row => !row.error);
  const selectedBatchChainCount = batchChainRows.filter(row => row.chain_id && row.asset_ids.length > 0).length;
  const batchAttemptCount = importableRows.length * selectedBatchChainCount;

  useEffect(() => {
    if (!importTaskId || !isTerminalImportStatus(importTaskQuery.data?.status)) return;
    queryClient.invalidateQueries({ queryKey: ['addresses'] });
    queryClient.invalidateQueries({ queryKey: ['address-import-errors', importTaskId] });
  }, [importTaskId, importTaskQuery.data?.status, queryClient]);

  const saveMutation = useMutation({
    mutationFn: async (values: Record<string, unknown>) => {
      const base = basePayload(values);
      if (editingAddress) {
        const row = chainRows[0];
        if (!row?.asset_ids.length) {
          throw new Error('每条链至少选择一个资产');
        }
        return updateWatchedAddress(editingAddress.id, {
          ...base,
          chain_id: editingAddress.chain_id,
          asset_ids: row.asset_ids,
        } satisfies CreateWatchedAddressRequest);
      }

      for (const row of chainRows) {
        if (!row.chain_id) {
          throw new Error('请选择链');
        }
        if (!row.asset_ids.length) {
          throw new Error('每条链至少选择一个资产');
        }
      }

      const results = await Promise.allSettled(chainRows.map(row => createWatchedAddress({
        ...base,
        chain_id: row.chain_id,
        asset_ids: row.asset_ids,
      } satisfies CreateWatchedAddressRequest)));
      const failures = results.filter(result => result.status === 'rejected');
      if (failures.length > 0) {
        throw new Error(`部分链配置添加失败：${failures.length}/${results.length}`);
      }
      return results;
    },
    onSuccess: () => {
      Toast.success(editingAddress ? '地址已更新' : '地址已添加');
      closeModal();
      queryClient.invalidateQueries({ queryKey: ['addresses'] });
    },
    onError: error => {
      Toast.error(error instanceof Error ? error.message : '保存失败');
      queryClient.invalidateQueries({ queryKey: ['addresses'] });
    },
  });

  const createImportMutation = useMutation({
    mutationFn: (values: Record<string, unknown>) => {
      const chainConfigs = normalizedBatchChainConfigs();
      const firstConfig = chainConfigs[0];
      if (!firstConfig) {
        throw new Error('至少添加一条链配置');
      }
      return createWatchedAddressImport({
        defaults: {
          chain_id: firstConfig.chain_id,
          asset_ids: firstConfig.asset_ids,
          chain_configs: chainConfigs,
          priority: String(values.priority),
          scan_interval_seconds: Number(values.scan_interval_seconds),
          transfer_filter_enabled: Boolean(values.transfer_filter_enabled),
          balance_change_filter_enabled: Boolean(values.balance_change_filter_enabled),
          status: String(values.status),
        },
        rows: importableRows.map(row => ({
          row_number: row.row_number,
          raw_text: row.raw_text,
          address: row.address,
          label: row.label ?? null,
          priority: row.priority ?? null,
          scan_interval_seconds: row.scan_interval_seconds ?? null,
          transfer_filter_enabled: row.transfer_filter_enabled ?? null,
          balance_change_filter_enabled: row.balance_change_filter_enabled ?? null,
          status: row.status ?? null,
        })),
      });
    },
    onSuccess: task => {
      Toast.success('导入任务已创建');
      setImportTaskId(task.id);
      queryClient.invalidateQueries({ queryKey: ['addresses'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '导入任务创建失败'),
  });

  const cancelImportMutation = useMutation({
    mutationFn: cancelWatchedAddressImport,
    onSuccess: () => {
      Toast.success('导入任务已取消');
      queryClient.invalidateQueries({ queryKey: ['address-import', importTaskId] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '取消导入失败'),
  });

  const deleteMutation = useMutation({
    mutationFn: deleteWatchedAddress,
    onSuccess: () => {
      Toast.success('地址已删除');
      queryClient.invalidateQueries({ queryKey: ['addresses'] });
    },
  });

  function assetLabel(asset: Asset) {
    if (!asset.contract_address) {
      return asset.symbol;
    }
    const start = asset.contract_address.slice(0, 6);
    const end = asset.contract_address.slice(-4);
    return `${asset.symbol} (${asset.asset_type}, ${start}...${end})`;
  }

  function assetOptionsForChain(chainId: string) {
    return (assetsQuery.data ?? [])
      .filter(asset => asset.chain_id === chainId && asset.status === 'active')
      .map(asset => ({ value: asset.id, label: assetLabel(asset) }));
  }

  function selectedAssetSymbols(assetIds: string[] = []) {
    if (assetIds.length === 0) {
      return '-';
    }
    return assetIds.map(assetId => assetMap.get(assetId)?.symbol ?? assetId).join(', ');
  }

  function emptyChainRow(): ChainRow {
    return { id: createChainRowId(), chain_id: '', asset_ids: [] };
  }

  function resetCreateForm() {
    setEditingAddress(null);
    setChainRows([emptyChainRow()]);
  }

  function openCreateModal() {
    resetCreateForm();
    setVisible(true);
  }

  function openEditModal(address: WatchedAddress) {
    setEditingAddress(address);
    setChainRows([{ id: createChainRowId(), chain_id: address.chain_id, asset_ids: address.asset_ids }]);
    setVisible(true);
  }

  function closeModal() {
    setVisible(false);
    resetCreateForm();
  }

  function openBatchModal() {
    setBatchInput('');
    setImportTaskId(null);
    setBatchChainRows([emptyChainRow()]);
    setBatchVisible(true);
  }

  function closeBatchModal() {
    setBatchVisible(false);
  }

  function addChainRow() {
    setChainRows(rows => [...rows, emptyChainRow()]);
  }

  function removeChainRow(rowId: string) {
    setChainRows(rows => rows.length === 1 ? rows : rows.filter(row => row.id !== rowId));
  }

  function updateChainRow(rowId: string, patch: Partial<Pick<ChainRow, 'chain_id' | 'asset_ids'>>) {
    setChainRows(rows => rows.map(row => row.id === rowId ? { ...row, ...patch } : row));
  }

  function addBatchChainRow() {
    setBatchChainRows(rows => [...rows, emptyChainRow()]);
  }

  function removeBatchChainRow(rowId: string) {
    setBatchChainRows(rows => rows.length === 1 ? rows : rows.filter(row => row.id !== rowId));
  }

  function updateBatchChainRow(rowId: string, patch: Partial<Pick<ChainRow, 'chain_id' | 'asset_ids'>>) {
    setBatchChainRows(rows => rows.map(row => row.id === rowId ? { ...row, ...patch } : row));
  }

  function normalizedBatchChainConfigs() {
    if (batchChainRows.length === 0) {
      throw new Error('至少添加一条链配置');
    }
    const seen = new Set<string>();
    return batchChainRows.map(row => {
      if (!row.chain_id) {
        throw new Error('请选择链');
      }
      if (!row.asset_ids.length) {
        throw new Error('每条链至少选择一个资产');
      }
      if (seen.has(row.chain_id)) {
        throw new Error('不能重复选择链');
      }
      seen.add(row.chain_id);
      return { chain_id: row.chain_id, asset_ids: row.asset_ids };
    });
  }

  function basePayload(values: Record<string, unknown>) {
    return {
      address: editingAddress ? editingAddress.address : String(values.address),
      label: values.label ? String(values.label) : null,
      priority: String(values.priority),
      scan_interval_seconds: Number(values.scan_interval_seconds),
      transfer_filter_enabled: Boolean(values.transfer_filter_enabled),
      balance_change_filter_enabled: Boolean(values.balance_change_filter_enabled),
      status: String(values.status),
    };
  }

  function handleSubmit(values: Record<string, unknown>) {
    saveMutation.mutate(values);
  }

  return (
    <PageScaffold title="监听地址" actions={(
      <Space>
        <Button onClick={openBatchModal}>批量添加</Button>
        <Button onClick={openCreateModal}>新增地址</Button>
      </Space>
    )}>
      <DataSurface title="监听地址列表">
        <DataTable<WatchedAddress>
          tableId="addresses"
          actionColumnKeys={['operations']}
          loading={addressesQuery.isLoading}
          dataSource={addressesQuery.data ?? []}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          scroll={{ x: 1500 }}
          columns={[
            { title: '链', dataIndex: 'chain_id', width: 150, render: value => chainMap.get(String(value)) ?? String(value) },
            { title: '标签', dataIndex: 'label', width: 150, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
            { title: '地址', dataIndex: 'address', width: 340, ellipsis: { showTitle: true }, className: 'table-cell-mono' },
            { title: '监听资产', dataIndex: 'asset_ids', width: 180, ellipsis: { showTitle: true }, render: value => selectedAssetSymbols(value as string[]) },
            { title: '优先级', dataIndex: 'priority', width: 100, render: value => <Tag>{String(value)}</Tag> },
            { title: '扫描间隔', dataIndex: 'scan_interval_seconds', width: 110 },
            { title: '转账', dataIndex: 'transfer_filter_enabled', width: 90, render: value => value ? '开启' : '关闭' },
            { title: '余额变化', dataIndex: 'balance_change_filter_enabled', width: 110, render: value => value ? '开启' : '关闭' },
            { title: '状态', dataIndex: 'status', width: 100 },
            {
              key: 'operations',
              title: '操作',
              width: 140,
              render: (_, record) => (
                <Space>
                  <Button theme="borderless" onClick={() => openEditModal(record)}>编辑</Button>
                  <Popconfirm title="确认删除该地址？" onConfirm={() => deleteMutation.mutate(record.id)}>
                    <Button type="danger" theme="borderless">删除</Button>
                  </Popconfirm>
                </Space>
              ),
            },
          ]}
        />
      </DataSurface>
      <FormModal title={editingAddress ? '编辑监听地址' : '新增监听地址'} visible={visible} onCancel={closeModal} size="large">
        <Form
          key={editingAddress?.id ?? 'create'}
          onSubmit={handleSubmit}
          initValues={editingAddress ?? {
            priority: 'normal',
            scan_interval_seconds: 300,
            transfer_filter_enabled: true,
            balance_change_filter_enabled: true,
            status: 'active',
          }}
        >
          <Form.Input field="address" label="地址" disabled={Boolean(editingAddress)} rules={[{ required: true, message: '请输入地址' }]} />
          <Form.Input field="label" label="标签" />
          <Form.Select field="priority" label="优先级" initValue="normal">
            <Form.Select.Option value="normal">normal</Form.Select.Option>
            <Form.Select.Option value="high">high</Form.Select.Option>
            <Form.Select.Option value="critical">critical</Form.Select.Option>
          </Form.Select>
          <Form.InputNumber field="scan_interval_seconds" label="扫描间隔秒" initValue={300} min={10} />
          <Form.Switch field="transfer_filter_enabled" label="关注转账" initValue={true} />
          <Form.Switch field="balance_change_filter_enabled" label="关注余额变化" initValue={true} />
          <Form.Select field="status" label="状态" initValue="active">
            <Form.Select.Option value="active">active</Form.Select.Option>
            <Form.Select.Option value="paused">paused</Form.Select.Option>
          </Form.Select>

          <div className="address-chain-rows">
            {chainRows.map((row, index) => (
              <Space key={row.id} align="start" style={{ width: '100%', marginBottom: 12 }}>
                <div style={{ width: 180 }}>
                  <div style={{ marginBottom: 4 }}>{index === 0 ? '链配置' : '\u00a0'}</div>
                  <Select
                    disabled={Boolean(editingAddress)}
                    value={row.chain_id}
                    placeholder="选择链"
                    style={{ width: '100%' }}
                    onChange={value => updateChainRow(row.id, {
                      chain_id: typeof value === 'string' ? value : '',
                      asset_ids: [],
                    })}
                  >
                    {(chainsQuery.data ?? []).map(chain => <Select.Option key={chain.id} value={chain.id}>{chain.name}</Select.Option>)}
                  </Select>
                </div>
                <div style={{ width: 260 }}>
                  <div style={{ marginBottom: 4 }}>资产</div>
                  <Select
                    multiple
                    filter
                    value={row.asset_ids}
                    placeholder="选择资产"
                    optionList={assetOptionsForChain(row.chain_id)}
                    style={{ width: '100%' }}
                    onChange={value => updateChainRow(row.id, { asset_ids: Array.isArray(value) ? value.map(String) : [] })}
                  />
                </div>
                {!editingAddress && (
                  <Button htmlType="button" onClick={() => removeChainRow(row.id)} disabled={chainRows.length === 1}>移除</Button>
                )}
              </Space>
            ))}
          </div>
          {!editingAddress && <Button htmlType="button" onClick={addChainRow} theme="borderless">新增链配置</Button>}

          <Space className="form-modal-actions">
            <Button htmlType="submit" type="primary" loading={saveMutation.isPending}>保存</Button>
            <Button htmlType="button" onClick={closeModal}>取消</Button>
          </Space>
        </Form>
      </FormModal>
      <FormModal title="批量添加监听地址" visible={batchVisible} onCancel={closeBatchModal} size="wide">
        {parsedImport.warnings.length > 0 ? (
          <Banner type="warning" title="导入提示" description={parsedImport.warnings.join('；')} />
        ) : null}
        <Form<BatchImportForm>
          onSubmit={values => createImportMutation.mutate(values)}
          labelPosition="left"
          labelWidth={130}
          initValues={{
            priority: 'normal',
            scan_interval_seconds: 300,
            transfer_filter_enabled: true,
            balance_change_filter_enabled: true,
            status: 'active',
          }}
        >
          {() => (
            <>
              <div className="address-chain-rows">
                {batchChainRows.map((row, index) => (
                  <Space key={row.id} align="start" style={{ width: '100%', marginBottom: 12 }}>
                    <div style={{ width: 180 }}>
                      <div style={{ marginBottom: 4 }}>{index === 0 ? '链配置' : '\u00a0'}</div>
                      <Select
                        value={row.chain_id}
                        placeholder="选择链"
                        style={{ width: '100%' }}
                        onChange={value => updateBatchChainRow(row.id, {
                          chain_id: typeof value === 'string' ? value : '',
                          asset_ids: [],
                        })}
                      >
                        {(chainsQuery.data ?? []).map(chain => <Select.Option key={chain.id} value={chain.id}>{chain.name}</Select.Option>)}
                      </Select>
                    </div>
                    <div style={{ width: 260 }}>
                      <div style={{ marginBottom: 4 }}>资产</div>
                      <Select
                        multiple
                        filter
                        value={row.asset_ids}
                        placeholder="选择资产"
                        optionList={assetOptionsForChain(row.chain_id)}
                        style={{ width: '100%' }}
                        onChange={value => updateBatchChainRow(row.id, { asset_ids: Array.isArray(value) ? value.map(String) : [] })}
                      />
                    </div>
                    <Button htmlType="button" onClick={() => removeBatchChainRow(row.id)} disabled={batchChainRows.length === 1}>移除</Button>
                  </Space>
                ))}
              </div>
              <Button htmlType="button" onClick={addBatchChainRow} theme="borderless">新增链配置</Button>
              <Form.Select field="priority" label="默认优先级">
                <Form.Select.Option value="normal">normal</Form.Select.Option>
                <Form.Select.Option value="high">high</Form.Select.Option>
                <Form.Select.Option value="critical">critical</Form.Select.Option>
              </Form.Select>
              <Form.InputNumber field="scan_interval_seconds" label="扫描间隔秒" min={10} />
              <Form.Switch field="transfer_filter_enabled" label="关注转账" />
              <Form.Switch field="balance_change_filter_enabled" label="关注余额变化" />
              <Form.Select field="status" label="默认状态">
                <Form.Select.Option value="active">active</Form.Select.Option>
                <Form.Select.Option value="paused">paused</Form.Select.Option>
              </Form.Select>
              <Form.TextArea
                field="raw_input"
                label="地址或CSV"
                autosize={{ minRows: 5, maxRows: 10 }}
                placeholder="每行一个地址，或粘贴 address,label,priority CSV"
                onChange={value => setBatchInput(String(value))}
              />
              <Space wrap className="address-import-summary">
                <Tag>有效地址 {importableRows.length}</Tag>
                <Tag color="blue">链配置 {selectedBatchChainCount}</Tag>
                <Tag color="green">预计创建尝试 {batchAttemptCount}</Tag>
              </Space>
              <div className="address-import-section">
                <DataTable
                  tableId="address-import-preview"
                  dataSource={parsedImport.rows}
                  rowKey="row_number"
                  pagination={false}
                  scroll={{ x: 900 }}
                  columns={[
                    { title: '行号', dataIndex: 'row_number', width: 80 },
                    { title: '地址', dataIndex: 'address', width: 320, className: 'table-cell-mono', ellipsis: { showTitle: true } },
                    { title: '标签', dataIndex: 'label', width: 160, render: value => value ? String(value) : '-' },
                    { title: '优先级', dataIndex: 'priority', width: 120, render: value => value ? String(value) : '-' },
                    { title: '状态', dataIndex: 'error', width: 160, render: value => value ? <Tag color="red">{String(value)}</Tag> : <Tag color="green">可导入</Tag> },
                  ]}
                />
              </div>
              {importTaskQuery.data ? (
                <div className="address-import-progress">
                  <div className="address-import-progress-title">导入进度（按地址-链尝试计数）</div>
                  <Progress percent={importProgress(importTaskQuery.data)} />
                  <Space wrap>
                    <Tag>总尝试 {importTaskQuery.data.total_rows}</Tag>
                    <Tag color="blue">已处理 {importTaskQuery.data.processed_rows}</Tag>
                    <Tag color="green">成功 {importTaskQuery.data.success_rows}</Tag>
                    <Tag color="red">失败 {importTaskQuery.data.failed_rows}</Tag>
                    <Tag>{importTaskQuery.data.status}</Tag>
                  </Space>
                </div>
              ) : null}
              <div className="address-import-section">
                <DataTable<WatchedAddressImportErrorRow>
                  tableId="address-import-errors"
                  dataSource={importErrorsQuery.data ?? []}
                  rowKey={row => row ? `${row.row_number}-${row.chain_id}` : ''}
                  pagination={{ pageSize: 10 }}
                  scroll={{ x: 1050 }}
                  columns={[
                    { title: '行号', dataIndex: 'row_number', width: 80 },
                    { title: '链', dataIndex: 'chain_id', width: 160, render: (_, record) => record.chain_name ?? chainMap.get(String(record.chain_id)) ?? String(record.chain_id) },
                    { title: '地址', dataIndex: 'address', width: 320, className: 'table-cell-mono', ellipsis: { showTitle: true } },
                    { title: '原始内容', dataIndex: 'raw_text', width: 260, ellipsis: { showTitle: true } },
                    { title: '错误', dataIndex: 'error_message', width: 260, ellipsis: { showTitle: true } },
                  ]}
                />
              </div>
              <Space className="form-modal-actions">
                <Button htmlType="submit" type="primary" loading={createImportMutation.isPending} disabled={importableRows.length === 0}>创建导入任务</Button>
                {importTaskId ? (
                  <Button htmlType="button" type="danger" loading={cancelImportMutation.isPending} onClick={() => cancelImportMutation.mutate(importTaskId)}>取消导入</Button>
                ) : null}
                <Button htmlType="button" onClick={closeBatchModal}>关闭</Button>
              </Space>
            </>
          )}
        </Form>
      </FormModal>
    </PageScaffold>
  );
}
