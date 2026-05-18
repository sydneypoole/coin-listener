import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Button, Card, Form, Modal, Table, Toast } from '@douyinfe/semi-ui';
import { createProvider, listChains, listProviders } from '../api/client';
import type { CreateProviderRequest, Provider } from '../api/types';

export function ProvidersPage() {
  const [visible, setVisible] = useState(false);
  const queryClient = useQueryClient();
  const providersQuery = useQuery({ queryKey: ['providers'], queryFn: listProviders });
  const chainsQuery = useQuery({ queryKey: ['chains'], queryFn: listChains });
  const chainMap = new Map((chainsQuery.data ?? []).map(chain => [chain.id, chain.name]));

  const mutation = useMutation({
    mutationFn: createProvider,
    onSuccess: () => {
      Toast.success('Provider 已创建');
      setVisible(false);
      queryClient.invalidateQueries({ queryKey: ['providers'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '创建失败'),
  });

  function handleSubmit(values: Record<string, unknown>) {
    mutation.mutate({
      chain_id: String(values.chain_id),
      provider_type: String(values.provider_type),
      name: String(values.name),
      base_url: String(values.base_url),
      api_key_ref: values.api_key_ref ? String(values.api_key_ref) : null,
      priority: Number(values.priority),
      qps_limit: Number(values.qps_limit),
      timeout_ms: Number(values.timeout_ms),
      status: String(values.status),
    } satisfies CreateProviderRequest);
  }

  return (
    <Card title="Provider 配置" headerExtraContent={<Button onClick={() => setVisible(true)}>新增 Provider</Button>}>
      <Table<Provider>
        loading={providersQuery.isLoading}
        dataSource={providersQuery.data ?? []}
        rowKey="id"
        pagination={{ pageSize: 10 }}
        columns={[
          { title: '链', dataIndex: 'chain_id', render: value => chainMap.get(String(value)) ?? String(value) },
          { title: '名称', dataIndex: 'name' },
          { title: '类型', dataIndex: 'provider_type' },
          { title: 'URL', dataIndex: 'base_url' },
          { title: '优先级', dataIndex: 'priority' },
          { title: 'QPS', dataIndex: 'qps_limit' },
          { title: '超时', dataIndex: 'timeout_ms' },
          { title: '状态', dataIndex: 'status' },
        ]}
      />
      <Modal title="新增 Provider" visible={visible} onCancel={() => setVisible(false)} footer={null}>
        <Form onSubmit={handleSubmit}>
          <Form.Select field="chain_id" label="链" rules={[{ required: true, message: '请选择链' }]}>
            {(chainsQuery.data ?? []).map(chain => <Form.Select.Option key={chain.id} value={chain.id}>{chain.name}</Form.Select.Option>)}
          </Form.Select>
          <Form.Select field="provider_type" label="类型" initValue="rpc">
            <Form.Select.Option value="rpc">RPC</Form.Select.Option>
            <Form.Select.Option value="websocket">WebSocket</Form.Select.Option>
            <Form.Select.Option value="rest_api">REST API</Form.Select.Option>
          </Form.Select>
          <Form.Input field="name" label="名称" rules={[{ required: true, message: '请输入名称' }]} />
          <Form.Input field="base_url" label="Base URL" rules={[{ required: true, message: '请输入 URL' }]} />
          <Form.Input field="api_key_ref" label="API Key 引用" />
          <Form.InputNumber field="priority" label="优先级" initValue={100} />
          <Form.InputNumber field="qps_limit" label="QPS 限制" initValue={10} />
          <Form.InputNumber field="timeout_ms" label="超时毫秒" initValue={10000} />
          <Form.Select field="status" label="状态" initValue="active">
            <Form.Select.Option value="active">active</Form.Select.Option>
            <Form.Select.Option value="disabled">disabled</Form.Select.Option>
          </Form.Select>
          <Button htmlType="submit" type="primary" loading={mutation.isPending}>保存</Button>
        </Form>
      </Modal>
    </Card>
  );
}
