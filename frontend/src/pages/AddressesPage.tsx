import { useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Button, Form, Popconfirm, Select, Space, Tag, Toast } from '@douyinfe/semi-ui';
import {
  createWatchedAddress,
  deleteWatchedAddress,
  listAssets,
  listChains,
  listWatchedAddresses,
  updateWatchedAddress,
} from '../api/client';
import type { Asset, CreateWatchedAddressRequest, WatchedAddress } from '../api/types';
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { FormModal } from '../components/FormModal';
import { PageScaffold } from '../components/PageScaffold';

type ChainRow = {
  id: string;
  chain_id: string;
  asset_ids: string[];
};

let chainRowIdSequence = 0;

function createChainRowId() {
  chainRowIdSequence += 1;
  return `address-chain-row-${chainRowIdSequence}`;
}

export function AddressesPage() {
  const [visible, setVisible] = useState(false);
  const [editingAddress, setEditingAddress] = useState<WatchedAddress | null>(null);
  const [chainRows, setChainRows] = useState<ChainRow[]>([emptyChainRow()]);
  const queryClient = useQueryClient();
  const addressesQuery = useQuery({ queryKey: ['addresses'], queryFn: listWatchedAddresses });
  const chainsQuery = useQuery({ queryKey: ['chains'], queryFn: listChains });
  const assetsQuery = useQuery({ queryKey: ['assets'], queryFn: listAssets });
  const chainMap = useMemo(() => new Map((chainsQuery.data ?? []).map(chain => [chain.id, chain.name])), [chainsQuery.data]);
  const assetMap = useMemo(() => new Map((assetsQuery.data ?? []).map(asset => [asset.id, asset])), [assetsQuery.data]);

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

  function addChainRow() {
    setChainRows(rows => [...rows, emptyChainRow()]);
  }

  function removeChainRow(rowId: string) {
    setChainRows(rows => rows.length === 1 ? rows : rows.filter(row => row.id !== rowId));
  }

  function updateChainRow(rowId: string, patch: Partial<Pick<ChainRow, 'chain_id' | 'asset_ids'>>) {
    setChainRows(rows => rows.map(row => row.id === rowId ? { ...row, ...patch } : row));
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
    <PageScaffold title="监听地址" actions={<Button onClick={openCreateModal}>新增地址</Button>}>
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
    </PageScaffold>
  );
}
