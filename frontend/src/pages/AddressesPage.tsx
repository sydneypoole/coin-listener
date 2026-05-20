import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Button, Card, Form, Modal, Popconfirm, Table, Tag, Toast } from '@douyinfe/semi-ui';
import { createWatchedAddress, deleteWatchedAddress, listChains, listWatchedAddresses } from '../api/client';
import type { CreateWatchedAddressRequest, WatchedAddress } from '../api/types';

export function AddressesPage() {
  const [visible, setVisible] = useState(false);
  const queryClient = useQueryClient();
  const addressesQuery = useQuery({ queryKey: ['addresses'], queryFn: listWatchedAddresses });
  const chainsQuery = useQuery({ queryKey: ['chains'], queryFn: listChains });
  const chainMap = new Map((chainsQuery.data ?? []).map(chain => [chain.id, chain.name]));

  const createMutation = useMutation({
    mutationFn: createWatchedAddress,
    onSuccess: () => {
      Toast.success('地址已添加');
      setVisible(false);
      queryClient.invalidateQueries({ queryKey: ['addresses'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '添加失败'),
  });

  const deleteMutation = useMutation({
    mutationFn: deleteWatchedAddress,
    onSuccess: () => {
      Toast.success('地址已删除');
      queryClient.invalidateQueries({ queryKey: ['addresses'] });
    },
  });

  function handleSubmit(values: Record<string, unknown>) {
    createMutation.mutate({
      chain_id: String(values.chain_id),
      address: String(values.address),
      label: values.label ? String(values.label) : null,
      priority: String(values.priority),
      scan_interval_seconds: Number(values.scan_interval_seconds),
      transfer_filter_enabled: Boolean(values.transfer_filter_enabled),
      balance_change_filter_enabled: Boolean(values.balance_change_filter_enabled),
      status: String(values.status),
    } satisfies CreateWatchedAddressRequest);
  }

  return (
    <Card title="监听地址" headerExtraContent={<Button onClick={() => setVisible(true)}>新增地址</Button>}>
      <Table<WatchedAddress>
        loading={addressesQuery.isLoading}
        dataSource={addressesQuery.data ?? []}
        rowKey="id"
        pagination={{ pageSize: 10 }}
        scroll={{ x: 1280 }}
        columns={[
          { title: '链', dataIndex: 'chain_id', width: 150, render: value => chainMap.get(String(value)) ?? String(value) },
          { title: '标签', dataIndex: 'label', width: 150, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
          { title: '地址', dataIndex: 'address', width: 340, ellipsis: { showTitle: true }, className: 'table-cell-mono' },
          { title: '优先级', dataIndex: 'priority', width: 100, render: value => <Tag>{String(value)}</Tag> },
          { title: '扫描间隔', dataIndex: 'scan_interval_seconds', width: 110 },
          { title: '转账', dataIndex: 'transfer_filter_enabled', width: 90, render: value => value ? '开启' : '关闭' },
          { title: '余额变化', dataIndex: 'balance_change_filter_enabled', width: 110, render: value => value ? '开启' : '关闭' },
          { title: '状态', dataIndex: 'status', width: 100 },
          {
            title: '操作',
            width: 110,
            fixed: 'right',
            render: (_, record) => (
              <Popconfirm title="确认删除该地址？" onConfirm={() => deleteMutation.mutate(record.id)}>
                <Button type="danger" theme="borderless">删除</Button>
              </Popconfirm>
            ),
          },
        ]}
      />
      <Modal title="新增监听地址" visible={visible} onCancel={() => setVisible(false)} footer={null}>
        <Form onSubmit={handleSubmit}>
          <Form.Select field="chain_id" label="链" rules={[{ required: true, message: '请选择链' }]}>
            {(chainsQuery.data ?? []).map(chain => <Form.Select.Option key={chain.id} value={chain.id}>{chain.name}</Form.Select.Option>)}
          </Form.Select>
          <Form.Input field="address" label="地址" rules={[{ required: true, message: '请输入地址' }]} />
          <Form.Input field="label" label="标签" />
          <Form.Select field="priority" label="优先级" initValue="normal">
            <Form.Select.Option value="normal">normal</Form.Select.Option>
            <Form.Select.Option value="high">high</Form.Select.Option>
            <Form.Select.Option value="critical">critical</Form.Select.Option>
          </Form.Select>
          <Form.InputNumber field="scan_interval_seconds" label="扫描间隔秒" initValue={300} />
          <Form.Switch field="transfer_filter_enabled" label="关注转账" initValue={true} />
          <Form.Switch field="balance_change_filter_enabled" label="关注余额变化" initValue={true} />
          <Form.Select field="status" label="状态" initValue="active">
            <Form.Select.Option value="active">active</Form.Select.Option>
            <Form.Select.Option value="paused">paused</Form.Select.Option>
          </Form.Select>
          <Button htmlType="submit" type="primary" loading={createMutation.isPending}>保存</Button>
        </Form>
      </Modal>
    </Card>
  );
}
