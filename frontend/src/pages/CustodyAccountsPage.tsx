import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Banner, Button, Form, Popconfirm, Space, Tag, Toast, Typography } from '@douyinfe/semi-ui';
import {
  assignCustodyAccount,
  createCustodyAccount,
  listChains,
  listCustodyAccounts,
  listCustodyAssignments,
  releaseCustodyAssignment,
} from '../api/client';
import type {
  AssignCustodyAccountRequest,
  CreateCustodyAccountRequest,
  CustodyAccount,
  CustodyAccountAssignment,
  CustodyAccountQuery,
  CustodyAssignmentQuery,
} from '../api/types';
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { FilterPanel } from '../components/FilterPanel';
import { FormModal } from '../components/FormModal';
import { PageScaffold } from '../components/PageScaffold';

const { Text } = Typography;

type AccountFilterForm = {
  chain_id?: string;
  source?: string;
  status?: string;
};

type AssignmentFilterForm = {
  chain_id?: string;
  status?: string;
  business_ref?: string;
};

function custodySourceText(source: string) {
  if (source === 'pool') return '系统地址池';
  if (source === 'user') return '用户自带';
  return source;
}

function custodyAccountStatusColor(status: string): 'green' | 'blue' | 'red' | 'grey' {
  if (status === 'available') return 'green';
  if (status === 'assigned') return 'blue';
  if (status === 'disabled') return 'red';
  return 'grey';
}

function custodyAssignmentStatusColor(status: string): 'blue' | 'green' | 'red' | 'grey' {
  if (status === 'active') return 'blue';
  if (status === 'released') return 'green';
  if (status === 'cancelled') return 'red';
  return 'grey';
}

function formatTime(value?: string | null) {
  return value ? new Date(value).toLocaleString() : '-';
}

function optionalString(value: unknown) {
  const text = value === undefined || value === null ? '' : String(value).trim();
  return text || undefined;
}

function validateAssignCustodyAccountForm(values: Record<string, unknown>) {
  if (String(values.source) === 'user' && !optionalString(values.address)) {
    Toast.warning('用户自带地址需填写地址');
    return false;
  }
  return true;
}

export function CustodyAccountsPage() {
  const [accountFilters, setAccountFilters] = useState<CustodyAccountQuery>({});
  const [assignmentFilters, setAssignmentFilters] = useState<CustodyAssignmentQuery>({ status: 'active' });
  const [createVisible, setCreateVisible] = useState(false);
  const [assignVisible, setAssignVisible] = useState(false);
  const queryClient = useQueryClient();

  const chainsQuery = useQuery({ queryKey: ['chains'], queryFn: listChains });
  const accountsQuery = useQuery({
    queryKey: ['custody-accounts', accountFilters],
    queryFn: () => listCustodyAccounts(accountFilters),
  });
  const assignmentsQuery = useQuery({
    queryKey: ['custody-assignments', assignmentFilters],
    queryFn: () => listCustodyAssignments(assignmentFilters),
  });

  const chainOptions = (chainsQuery.data ?? []).map(chain => ({ label: chain.name, value: chain.id }));

  const createMutation = useMutation({
    mutationFn: (values: Record<string, unknown>) => {
      const payload: CreateCustodyAccountRequest = {
        chain_id: String(values.chain_id),
        address: String(values.address).trim(),
        label: optionalString(values.label) ?? null,
        source: 'pool',
        status: 'available',
      };
      return createCustodyAccount(payload);
    },
    onSuccess: () => {
      Toast.success('托管地址已创建');
      setCreateVisible(false);
      queryClient.invalidateQueries({ queryKey: ['custody-accounts'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '创建托管地址失败'),
  });

  const assignMutation = useMutation({
    mutationFn: (values: Record<string, unknown>) => {
      if (!validateAssignCustodyAccountForm(values)) {
        return Promise.reject(new Error('用户自带地址需填写地址'));
      }
      const source = String(values.source);
      const payload: AssignCustodyAccountRequest = {
        chain_id: String(values.chain_id),
        source,
        address: source === 'user' ? optionalString(values.address) : null,
        applicant_type: String(values.applicant_type),
        business_ref: String(values.business_ref).trim(),
        purpose: optionalString(values.purpose) ?? null,
      };
      return assignCustodyAccount(payload);
    },
    onSuccess: response => {
      Toast.success(`托管地址已申请并自动添加监听：${response.account.address}`);
      setAssignVisible(false);
      queryClient.invalidateQueries({ queryKey: ['custody-accounts'] });
      queryClient.invalidateQueries({ queryKey: ['custody-assignments'] });
      queryClient.invalidateQueries({ queryKey: ['addresses'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '申请托管地址失败'),
  });

  const releaseMutation = useMutation({
    mutationFn: releaseCustodyAssignment,
    onSuccess: () => {
      Toast.success('托管地址已释放');
      queryClient.invalidateQueries({ queryKey: ['custody-accounts'] });
      queryClient.invalidateQueries({ queryKey: ['custody-assignments'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '释放托管地址失败'),
  });

  function handleAccountFilter(values: Record<string, unknown>) {
    const form = values as AccountFilterForm;
    setAccountFilters({
      chain_id: form.chain_id || undefined,
      source: form.source || undefined,
      status: form.status || undefined,
    });
  }

  function handleAssignmentFilter(values: Record<string, unknown>) {
    const form = values as AssignmentFilterForm;
    setAssignmentFilters({
      chain_id: form.chain_id || undefined,
      status: form.status || undefined,
      business_ref: form.business_ref?.trim() || undefined,
    });
  }

  function resetAccountFilters(formApi: { reset: () => void }) {
    formApi.reset();
    setAccountFilters({});
  }

  function resetAssignmentFilters(formApi: { reset: () => void }) {
    formApi.reset();
    setAssignmentFilters({});
  }

  return (
    <PageScaffold
      title="托管账户"
      description="统一管理系统地址池和用户自带地址；申请成功后自动添加监听，并通过后端事务确保同一地址不能重复申请。"
      actions={(
        <Space>
          <Button onClick={() => setAssignVisible(true)} type="primary">申请托管地址</Button>
          <Button onClick={() => setCreateVisible(true)}>新增托管地址</Button>
        </Space>
      )}
    >
      <Banner
        type="info"
        title="托管模式说明"
        description="系统地址池按 available 地址分配；用户自带地址按链和地址归一化复用或创建。申请会自动添加监听，活跃分配期间不能重复申请。"
      />

      <FilterPanel title="托管地址筛选">
        <Form<AccountFilterForm> layout="horizontal" onSubmit={handleAccountFilter} labelPosition="left">
          {({ formApi }) => (
            <>
              <Form.Select field="chain_id" label="链" showClear placeholder="全部链" optionList={chainOptions} />
              <Form.Select field="source" label="来源" showClear placeholder="全部来源">
                <Form.Select.Option value="pool">系统地址池</Form.Select.Option>
                <Form.Select.Option value="user">用户自带</Form.Select.Option>
              </Form.Select>
              <Form.Select field="status" label="状态" showClear placeholder="全部状态">
                <Form.Select.Option value="available">available</Form.Select.Option>
                <Form.Select.Option value="assigned">assigned</Form.Select.Option>
                <Form.Select.Option value="disabled">disabled</Form.Select.Option>
              </Form.Select>
              <Space>
                <Button htmlType="submit" type="primary">查询</Button>
                <Button onClick={() => resetAccountFilters(formApi)}>重置</Button>
                <Button loading={accountsQuery.isFetching} onClick={() => accountsQuery.refetch()}>刷新</Button>
              </Space>
            </>
          )}
        </Form>
      </FilterPanel>

      <DataSurface title="托管地址池" actions={<Text type="tertiary">自动添加监听 / 不能重复申请</Text>}>
        <DataTable<CustodyAccount>
          tableId="custody-accounts"
          loading={accountsQuery.isLoading}
          dataSource={accountsQuery.data ?? []}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          scroll={{ x: 1600 }}
          columns={[
            { title: '链', dataIndex: 'chain_name', width: 150, ellipsis: { showTitle: true } },
            { title: '来源', dataIndex: 'source', width: 130, render: value => custodySourceText(String(value)) },
            { title: '标签', dataIndex: 'label', width: 160, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
            { title: '地址', dataIndex: 'address', width: 340, ellipsis: { showTitle: true }, render: value => <span className="table-cell-mono">{String(value)}</span> },
            { title: '状态', dataIndex: 'status', width: 120, render: value => <Tag color={custodyAccountStatusColor(String(value))}>{String(value)}</Tag> },
            { title: '业务引用', dataIndex: 'current_business_ref', width: 180, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
            { title: '监听地址ID', dataIndex: 'watched_address_id', width: 260, ellipsis: { showTitle: true }, render: value => value ? <span className="table-cell-mono">{String(value)}</span> : '-' },
            { title: '创建时间', dataIndex: 'created_at', width: 180, render: value => formatTime(String(value)) },
          ]}
        />
      </DataSurface>

      {accountsQuery.isError ? (
        <Banner type="danger" title="托管地址加载失败" description={accountsQuery.error instanceof Error ? accountsQuery.error.message : '请求失败'} />
      ) : null}

      <FilterPanel title="申请记录筛选">
        <Form<AssignmentFilterForm> layout="horizontal" onSubmit={handleAssignmentFilter} labelPosition="left" initValues={{ status: 'active' }}>
          {({ formApi }) => (
            <>
              <Form.Select field="chain_id" label="链" showClear placeholder="全部链" optionList={chainOptions} />
              <Form.Select field="status" label="状态" showClear placeholder="全部状态">
                <Form.Select.Option value="active">active</Form.Select.Option>
                <Form.Select.Option value="released">released</Form.Select.Option>
                <Form.Select.Option value="cancelled">cancelled</Form.Select.Option>
              </Form.Select>
              <Form.Input field="business_ref" label="业务引用" placeholder="外部订单号或内部业务ID" style={{ width: 240 }} />
              <Space>
                <Button htmlType="submit" type="primary">查询</Button>
                <Button onClick={() => resetAssignmentFilters(formApi)}>重置</Button>
                <Button loading={assignmentsQuery.isFetching} onClick={() => assignmentsQuery.refetch()}>刷新</Button>
              </Space>
            </>
          )}
        </Form>
      </FilterPanel>

      <DataSurface title="托管申请记录">
        <DataTable<CustodyAccountAssignment>
          tableId="custody-assignments"
          actionColumnKeys={['operations']}
          loading={assignmentsQuery.isLoading}
          dataSource={assignmentsQuery.data ?? []}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          scroll={{ x: 1800 }}
          columns={[
            { title: '申请时间', dataIndex: 'assigned_at', width: 180, render: value => formatTime(String(value)) },
            { title: '链', dataIndex: 'chain_name', width: 150, ellipsis: { showTitle: true } },
            { title: '地址', dataIndex: 'address', width: 340, ellipsis: { showTitle: true }, render: value => <span className="table-cell-mono">{String(value)}</span> },
            { title: '申请方', dataIndex: 'applicant_type', width: 110, render: value => <Tag>{String(value)}</Tag> },
            { title: '业务引用', dataIndex: 'business_ref', width: 180, ellipsis: { showTitle: true } },
            { title: '用途', dataIndex: 'purpose', width: 180, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
            { title: '状态', dataIndex: 'status', width: 120, render: value => <Tag color={custodyAssignmentStatusColor(String(value))}>{String(value)}</Tag> },
            { title: '释放时间', dataIndex: 'released_at', width: 180, render: value => formatTime(value ? String(value) : null) },
            { title: '监听地址ID', dataIndex: 'watched_address_id', width: 260, ellipsis: { showTitle: true }, render: value => value ? <span className="table-cell-mono">{String(value)}</span> : '-' },
            {
              title: '操作',
              key: 'operations',
              width: 120,
              render: (_, row) => row.status === 'active' ? (
                <Popconfirm title="确认释放该托管地址？" onConfirm={() => releaseMutation.mutate(row.id)}>
                  <Button size="small" type="danger" theme="borderless" loading={releaseMutation.isPending}>释放</Button>
                </Popconfirm>
              ) : '-',
            },
          ]}
        />
      </DataSurface>

      {assignmentsQuery.isError ? (
        <Banner type="danger" title="申请记录加载失败" description={assignmentsQuery.error instanceof Error ? assignmentsQuery.error.message : '请求失败'} />
      ) : null}

      <FormModal title="新增托管地址" visible={createVisible} onCancel={() => setCreateVisible(false)} size="large">
        <Banner type="info" title="系统地址池地址" description="新增入口仅维护可被系统池自动分配的地址，来源固定为 pool，状态固定为 available。用户自带地址请通过申请流程录入。" />
        <Form
          onSubmit={values => createMutation.mutate(values)}
          labelPosition="left"
          labelWidth={120}
          initValues={{ source: 'pool', status: 'available' }}
        >
          <Form.Select field="chain_id" label="链" rules={[{ required: true, message: '请选择链' }]} optionList={chainOptions} />
          <Form.Input field="address" label="地址" rules={[{ required: true, message: '请输入地址' }]} />
          <Form.Input field="label" label="标签" />
          <Form.Select field="source" label="来源" disabled rules={[{ required: true, message: '请选择来源' }]}>
            <Form.Select.Option value="pool">系统地址池</Form.Select.Option>
            <Form.Select.Option value="user">用户自带</Form.Select.Option>
          </Form.Select>
          <Form.Select field="status" label="状态" disabled rules={[{ required: true, message: '请选择状态' }]}>
            <Form.Select.Option value="available">available</Form.Select.Option>
          </Form.Select>
          <Space className="form-modal-actions">
            <Button htmlType="submit" type="primary" loading={createMutation.isPending}>保存</Button>
            <Button htmlType="button" onClick={() => setCreateVisible(false)}>取消</Button>
          </Space>
        </Form>
      </FormModal>

      <FormModal title="申请托管地址" visible={assignVisible} onCancel={() => setAssignVisible(false)} size="large">
        <Banner type="info" title="申请说明" description="系统地址池无需填写地址；用户自带地址需填写地址。申请成功后自动添加监听，同一业务引用和同一活跃地址不能重复申请。" />
        <Form
          onSubmit={values => assignMutation.mutate(values)}
          labelPosition="left"
          labelWidth={120}
          initValues={{ source: 'pool', applicant_type: 'api' }}
        >
          <Form.Select field="chain_id" label="链" rules={[{ required: true, message: '请选择链' }]} optionList={chainOptions} />
          <Form.Select field="source" label="来源" rules={[{ required: true, message: '请选择来源' }]}>
            <Form.Select.Option value="pool">系统地址池</Form.Select.Option>
            <Form.Select.Option value="user">用户自带</Form.Select.Option>
          </Form.Select>
          <Form.Input field="address" label="用户地址" placeholder="source=user 时填写" />
          <Form.Select field="applicant_type" label="申请方" rules={[{ required: true, message: '请选择申请方' }]}>
            <Form.Select.Option value="api">外部 API</Form.Select.Option>
            <Form.Select.Option value="internal">平台内部</Form.Select.Option>
          </Form.Select>
          <Form.Input field="business_ref" label="业务引用" rules={[{ required: true, message: '请输入业务引用' }]} />
          <Form.Input field="purpose" label="用途" placeholder="deposit_address" />
          <Space className="form-modal-actions">
            <Button htmlType="submit" type="primary" loading={assignMutation.isPending}>申请</Button>
            <Button htmlType="button" onClick={() => setAssignVisible(false)}>取消</Button>
          </Space>
        </Form>
      </FormModal>
    </PageScaffold>
  );
}
