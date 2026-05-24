import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Button, Form, Space, Tag, Toast } from '@douyinfe/semi-ui';
import { createProvider, listChains, listProviders, testProvider, updateProvider } from '../api/client';
import type { CreateProviderRequest, Provider } from '../api/types';
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { FormModal } from '../components/FormModal';
import { PageScaffold } from '../components/PageScaffold';

export function ProvidersPage() {
  const [visible, setVisible] = useState(false);
  const [editingProvider, setEditingProvider] = useState<Provider | null>(null);
  const [testingProviderId, setTestingProviderId] = useState<string | null>(null);
  const queryClient = useQueryClient();
  const providersQuery = useQuery({ queryKey: ['providers'], queryFn: listProviders });
  const chainsQuery = useQuery({ queryKey: ['chains'], queryFn: listChains });
  const chainMap = new Map((chainsQuery.data ?? []).map(chain => [chain.id, chain.name]));
  const chainTypeMap = new Map((chainsQuery.data ?? []).map(chain => [chain.id, chain.chain_type]));

  const mutation = useMutation({
    mutationFn: (payload: CreateProviderRequest) => (
      editingProvider ? updateProvider(editingProvider.id, payload) : createProvider(payload)
    ),
    onSuccess: () => {
      Toast.success(editingProvider ? 'Provider 已更新' : 'Provider 已创建');
      setVisible(false);
      setEditingProvider(null);
      queryClient.invalidateQueries({ queryKey: ['providers'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '保存失败'),
  });

  const testMutation = useMutation({
    mutationFn: testProvider,
    onSuccess: result => {
      Toast.success(result.latest_block === null || result.latest_block === undefined
        ? result.message
        : `${result.message}，最新区块 ${result.latest_block}`);
    },
    onError: error => Toast.error(error instanceof Error ? error.message : 'Provider 测试失败'),
    onSettled: () => setTestingProviderId(null),
  });

  function openCreateModal() {
    setEditingProvider(null);
    setVisible(true);
  }

  function openEditModal(provider: Provider) {
    setEditingProvider(provider);
    setVisible(true);
  }

  function closeModal() {
    setVisible(false);
    setEditingProvider(null);
  }

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

  function initialValues(): Partial<CreateProviderRequest> {
    return editingProvider ?? {
      provider_type: 'rpc',
      priority: 100,
      qps_limit: 10,
      timeout_ms: 10000,
      status: 'active',
    };
  }

  function providerTestSupported(provider: Provider) {
    const chainType = chainTypeMap.get(provider.chain_id);
    return (
      (chainType === 'evm' && provider.provider_type === 'rpc')
      || (chainType === 'tron' && ['rest_api', 'rpc'].includes(provider.provider_type))
      || (chainType === 'utxo' && provider.provider_type === 'rest_api')
    );
  }

  function handleTestProvider(provider: Provider) {
    setTestingProviderId(provider.id);
    testMutation.mutate(provider.id);
  }

  return (
    <PageScaffold
      title="Provider 配置"
      description="维护多链 RPC/REST Provider、优先级、限流与连通性验证。"
      actions={<Button onClick={openCreateModal}>新增 Provider</Button>}
    >
      <DataSurface title="Provider 列表">
        <DataTable<Provider>
          tableId="providers"
          actionColumnKeys={['operations']}
          loading={providersQuery.isLoading}
          dataSource={providersQuery.data ?? []}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          scroll={{ x: 1200 }}
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
              width: 150,
              render: (_, provider) => {
                const testDisabled = !providerTestSupported(provider);
                return (
                  <Space>
                    <Button size="small" onClick={() => openEditModal(provider)}>编辑</Button>
                    <Button
                      size="small"
                      disabled={testDisabled}
                      loading={testingProviderId === provider.id}
                      onClick={() => handleTestProvider(provider)}
                    >
                      {testDisabled ? '暂不支持测试' : '测试'}
                    </Button>
                  </Space>
                );
              },
            },
          ]}
        />
      </DataSurface>
      <FormModal title={editingProvider ? '编辑 Provider' : '新增 Provider'} visible={visible} onCancel={closeModal} size="large">
        <p className="form-help-text">支持 EVM RPC、TRON REST、BTC/UTXO REST Provider 连通性测试；WebSocket 暂不测试。</p>
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
          <Space className="form-modal-actions">
            <Button htmlType="submit" type="primary" loading={mutation.isPending}>保存</Button>
            <Button onClick={closeModal}>取消</Button>
          </Space>
        </Form>
      </FormModal>
    </PageScaffold>
  );
}
