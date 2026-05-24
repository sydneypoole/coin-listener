import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Banner, Button, Card, Form, Space, Tag, Toast, Typography } from '@douyinfe/semi-ui';
import { getScanRun, listChains, listScanRuns, listWatchedAddresses, retryScanRun } from '../api/client';
import type { ScanRunDetail, ScanRunListItem, ScanRunQuery, ScanRunStatus } from '../api/types';
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { FilterPanel } from '../components/FilterPanel';
import { FormModal } from '../components/FormModal';
import { PageScaffold } from '../components/PageScaffold';

const { Text } = Typography;

type FilterForm = {
  chain_id?: string;
  address_id?: string;
  status?: string;
  started_after?: string;
  started_before?: string;
};

const scanRunStatusOptions = [
  { label: '扫描中', value: 'running' },
  { label: '成功', value: 'success' },
  { label: '失败', value: 'failed' },
  { label: '跳过：锁占用', value: 'locked' },
  { label: '不支持', value: 'unsupported' },
];

function formatTime(value?: string | null) {
  return value ? new Date(value).toLocaleString() : '-';
}

function formatDuration(value?: number | null) {
  return value === null || value === undefined ? '-' : `${value}ms`;
}

function truncate(value?: string | null, maxLength = 120) {
  if (!value) return '-';
  return value.length > maxLength ? `${value.slice(0, maxLength)}…` : value;
}

function scanRunStatusText(status: ScanRunStatus) {
  if (status === 'running') return '扫描中';
  if (status === 'success') return '成功';
  if (status === 'failed') return '失败';
  if (status === 'locked') return '跳过：锁占用';
  if (status === 'unsupported') return '不支持';
  return status;
}

function scanRunStatusColor(status: ScanRunStatus): 'blue' | 'green' | 'red' | 'orange' | 'grey' {
  if (status === 'running') return 'blue';
  if (status === 'success') return 'green';
  if (status === 'failed') return 'red';
  if (status === 'locked') return 'grey';
  if (status === 'unsupported') return 'orange';
  return 'grey';
}

function retryableScanRun(status: ScanRunStatus) {
  return status === 'failed' || status === 'unsupported';
}

function currentPage(filters: ScanRunQuery) {
  const limit = filters.limit ?? 50;
  const offset = filters.offset ?? 0;
  return Math.floor(offset / limit) + 1;
}

export function ScanAuditPage() {
  const [filters, setFilters] = useState<ScanRunQuery>({ limit: 50, offset: 0 });
  const [selectedRunId, setSelectedRunId] = useState<string>();
  const queryClient = useQueryClient();

  const chainsQuery = useQuery({ queryKey: ['chains'], queryFn: listChains });
  const addressesQuery = useQuery({ queryKey: ['addresses'], queryFn: listWatchedAddresses });

  const scanRunsQuery = useQuery({
    queryKey: ['scan-runs', filters],
    queryFn: () => listScanRuns(filters),
  });

  const detailQuery = useQuery({
    queryKey: ['scan-run-detail', selectedRunId],
    queryFn: () => getScanRun(selectedRunId ?? ''),
    enabled: Boolean(selectedRunId),
  });

  const retryMutation = useMutation({
    mutationFn: retryScanRun,
    onSuccess: () => {
      Toast.success('扫描任务已重新入队');
      queryClient.invalidateQueries({ queryKey: ['scan-runs'] });
      queryClient.invalidateQueries({ queryKey: ['scan-run-detail'] });
      queryClient.invalidateQueries({ queryKey: ['system-status'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '重试扫描任务失败'),
  });

  function handleFilterSubmit(values: Record<string, unknown>) {
    const form = values as FilterForm;
    setFilters({
      chain_id: form.chain_id || undefined,
      address_id: form.address_id || undefined,
      status: form.status || undefined,
      started_after: form.started_after?.trim() || undefined,
      started_before: form.started_before?.trim() || undefined,
      limit: 50,
      offset: 0,
    });
  }

  function resetFilters(formApi: { reset: () => void }) {
    formApi.reset();
    setFilters({ limit: 50, offset: 0 });
  }

  function handlePageChange(page: number) {
    const pageSize = filters.limit ?? 50;
    setFilters(current => ({
      ...current,
      limit: pageSize,
      offset: (page - 1) * pageSize,
    }));
  }

  const chainOptions = (chainsQuery.data ?? []).map(chain => ({ label: chain.name, value: chain.id }));
  const addressOptions = (addressesQuery.data ?? []).map(address => ({
    label: `${address.label ?? address.address} / ${address.address.slice(0, 10)}…`,
    value: address.id,
  }));

  return (
    <PageScaffold
      title="扫描审计"
      description="查询 Worker 扫描尝试历史、失败原因与可重试任务。"
      actions={<Tag color={scanRunsQuery.isFetching ? 'blue' : 'green'}>{scanRunsQuery.isFetching ? 'refreshing' : 'manual refresh'}</Tag>}
    >
      {scanRunsQuery.isError ? (
        <Banner
          type="danger"
          title="扫描记录加载失败"
          description={scanRunsQuery.error instanceof Error ? scanRunsQuery.error.message : '请求失败'}
        />
      ) : null}

      {detailQuery.isError ? (
        <Banner
          type="danger"
          title="扫描详情加载失败"
          description={detailQuery.error instanceof Error ? detailQuery.error.message : '请求失败'}
        />
      ) : null}

      <FilterPanel title="扫描记录筛选">
        <Form<FilterForm> layout="horizontal" onSubmit={handleFilterSubmit} labelPosition="left">
          {({ formApi }) => (
            <>
              <Form.Select field="chain_id" label="链" showClear placeholder="全部链" optionList={chainOptions} />
              <Form.Select field="address_id" label="地址" showClear placeholder="全部地址" optionList={addressOptions} style={{ width: 280 }} />
              <Form.Select field="status" label="状态" showClear placeholder="全部状态" optionList={scanRunStatusOptions} />
              <Form.Input field="started_after" label="开始于" placeholder="2026-05-24T00:00:00Z" style={{ width: 220 }} />
              <Form.Input field="started_before" label="结束于" placeholder="2026-05-25T00:00:00Z" style={{ width: 220 }} />
              <Space>
                <Button htmlType="submit" type="primary">查询</Button>
                <Button onClick={() => resetFilters(formApi)}>重置</Button>
                <Button loading={scanRunsQuery.isFetching} onClick={() => scanRunsQuery.refetch()}>刷新</Button>
              </Space>
            </>
          )}
        </Form>
      </FilterPanel>

      <DataSurface title="扫描历史" actions={<Text type="tertiary">limit {filters.limit ?? 50} / offset {filters.offset ?? 0}</Text>}>
        <DataTable<ScanRunListItem>
          tableId="scan-runs"
          loading={scanRunsQuery.isLoading}
          dataSource={scanRunsQuery.data?.items ?? []}
          rowKey="id"
          pagination={{ pageSize: filters.limit ?? 50, currentPage: currentPage(filters), onPageChange: handlePageChange }}
          scroll={{ x: 1700 }}
          columns={[
            { title: '开始时间', dataIndex: 'started_at', width: 180, render: value => formatTime(String(value)) },
            { title: '结束时间', dataIndex: 'finished_at', width: 180, render: value => formatTime(value ? String(value) : null) },
            { title: '链', dataIndex: 'chain_name', width: 150, ellipsis: { showTitle: true } },
            { title: '地址', dataIndex: 'address', width: 260, ellipsis: { showTitle: true }, render: value => <span className="table-cell-mono">{String(value)}</span> },
            { title: '标签', dataIndex: 'address_label', width: 140, render: value => value ? String(value) : '-' },
            { title: '类型', dataIndex: 'chain_type', width: 100, render: value => <Tag>{String(value)}</Tag> },
            { title: '状态', dataIndex: 'status', width: 140, render: value => <Tag color={scanRunStatusColor(String(value))}>{scanRunStatusText(String(value))}</Tag> },
            { title: '耗时', dataIndex: 'duration_ms', width: 110, render: value => formatDuration(value as number | null) },
            { title: '事件数', dataIndex: 'event_count', width: 100 },
            { title: 'Task ID', dataIndex: 'task_id', width: 240, ellipsis: { showTitle: true } },
            { title: '错误摘要', dataIndex: 'error_message', width: 260, ellipsis: { showTitle: true }, render: value => truncate(value ? String(value) : null) },
            {
              title: '操作',
              key: 'operations',
              width: 170,
              render: (_, row) => (
                <Space>
                  <Button size="small" onClick={() => setSelectedRunId(row.id)}>详情</Button>
                  {retryableScanRun(row.status) ? (
                    <Button size="small" type="primary" loading={retryMutation.isPending} onClick={() => retryMutation.mutate(row.id)}>
                      重试
                    </Button>
                  ) : null}
                </Space>
              ),
            },
          ]}
        />
      </DataSurface>

      <ScanRunDetailModal
        visible={Boolean(selectedRunId)}
        loading={detailQuery.isLoading}
        detail={detailQuery.data}
        onClose={() => setSelectedRunId(undefined)}
      />
    </PageScaffold>
  );
}

function ScanRunDetailModal({
  visible,
  loading,
  detail,
  onClose,
}: {
  visible: boolean;
  loading: boolean;
  detail?: ScanRunDetail;
  onClose: () => void;
}) {
  return (
    <FormModal title="扫描记录详情" visible={visible} onCancel={onClose} size="wide">
      {loading ? <Text>正在加载详情...</Text> : null}
      {detail ? (
        <Space vertical align="start" spacing={16} style={{ width: '100%' }}>
          <div className="notification-detail-grid">
            <Card title="扫描尝试" className="notification-detail-card">
              <DetailLine label="Run ID" value={detail.id} mono />
              <DetailLine label="Task ID" value={detail.task_id} mono />
              <DetailLine label="链" value={`${detail.chain_name} / ${detail.chain_type}`} />
              <DetailLine label="地址" value={detail.address} mono />
              <div className="detail-line">
                <Text type="tertiary">状态</Text>
                <Tag color={scanRunStatusColor(detail.status)}>{scanRunStatusText(detail.status)}</Tag>
              </div>
              <DetailLine label="事件数" value={String(detail.event_count)} />
              <DetailLine label="耗时" value={formatDuration(detail.duration_ms)} />
              <DetailLine label="开始" value={formatTime(detail.started_at)} />
              <DetailLine label="结束" value={formatTime(detail.finished_at)} />
              <DetailLine label="错误" value={detail.error_message ?? '-'} />
            </Card>

            <Card title="Metadata" className="notification-detail-card">
              <div className="detail-line detail-line-vertical">
                <Text type="tertiary">运行元数据</Text>
                <pre className="detail-json">{JSON.stringify(detail.metadata, null, 2)}</pre>
              </div>
            </Card>
          </div>
        </Space>
      ) : null}
    </FormModal>
  );
}

function DetailLine({ label, value, mono = false }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="detail-line">
      <Text type="tertiary">{label}</Text>
      <span className={mono ? 'table-cell-mono detail-value' : 'detail-value'}>{value}</span>
    </div>
  );
}
