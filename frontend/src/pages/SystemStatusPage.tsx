import { useQuery } from '@tanstack/react-query';
import { Banner, Space, Tag, Typography } from '@douyinfe/semi-ui';
import { getSystemStatus } from '../api/client';
import type { ProviderChainStatus, ProviderStatusItem, ServiceHeartbeatStatusItem } from '../api/types';
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { MetricCard, MetricGrid } from '../components/MetricGrid';
import { PageScaffold } from '../components/PageScaffold';

const { Text } = Typography;

function formatDepth(depth?: number | null) {
  return depth === null || depth === undefined ? '-' : String(depth);
}

function formatTime(value?: string | null) {
  return value ? new Date(value).toLocaleString() : '-';
}

function statusColor(status: string): 'green' | 'grey' {
  return status === 'active' ? 'green' : 'grey';
}

function circuitStatusColor(isOpen: boolean): 'red' | 'green' {
  return isOpen ? 'red' : 'green';
}

function circuitStatusText(isOpen: boolean) {
  return isOpen ? 'circuit-open' : 'healthy';
}

function truncateError(value?: string | null) {
  if (!value) {
    return '-';
  }
  return value.length > 80 ? `${value.slice(0, 80)}…` : value;
}

function serviceStatusColor(item: ServiceHeartbeatStatusItem): 'green' | 'red' {
  return item.is_stale ? 'red' : 'green';
}

function shortInstanceId(instanceId: string) {
  return instanceId.length > 12 ? `${instanceId.slice(0, 8)}…` : instanceId;
}

function metadataText(metadata: Record<string, unknown>) {
  const pid = typeof metadata.pid === 'number' || typeof metadata.pid === 'string' ? metadata.pid : '-';
  const version = typeof metadata.version === 'string' ? metadata.version : '-';
  return `pid ${pid} / v${version}`;
}

function serviceHeartbeatRowKey(record?: ServiceHeartbeatStatusItem) {
  return record ? `${record.service_name}:${record.instance_id}` : '';
}

export function SystemStatusPage() {
  const statusQuery = useQuery({
    queryKey: ['system-status'],
    queryFn: getSystemStatus,
    refetchInterval: 10_000,
  });

  const status = statusQuery.data;

  return (
    <PageScaffold
      title="系统状态"
      description="聚合扫描队列、通知出站、Provider 熔断与服务心跳。"
      actions={<Tag color={statusQuery.isFetching ? 'blue' : 'green'}>{statusQuery.isFetching ? 'refreshing' : '10s auto refresh'}</Tag>}
    >
      {statusQuery.isError ? (
        <Banner
          type="danger"
          title="系统状态加载失败"
          description={statusQuery.error instanceof Error ? statusQuery.error.message : '请求失败'}
        />
      ) : null}

      {status?.queues.queue_errors.length ? (
        <Banner
          type="warning"
          title="队列状态部分不可用"
          description={status.queues.queue_errors.join('；')}
        />
      ) : null}

      <MetricGrid>
        <MetricCard title="Scan Queue" value={formatDepth(status?.queues.scan_queue_depth)} hint={status?.queues.scan_queue_key ?? '-'} />
        <MetricCard title="Notify Queue" value={formatDepth(status?.queues.notify_queue_depth)} hint={status?.queues.notify_queue_key ?? '-'} />
        <MetricCard title="Active 地址" value={status?.scans.active_addresses ?? 0} hint={`due ${status?.scans.due_addresses ?? 0}`} />
        <MetricCard
          title="Overdue 地址"
          value={status?.scans.overdue_addresses ?? 0}
          hint={`last scan ${formatTime(status?.scans.last_scanned_at)}`}
          tone={status?.scans.overdue_addresses ? 'warning' : 'neutral'}
        />
        <MetricCard title="24h 事件" value={status?.events.last_24h_total ?? 0} hint={`transfer ${status?.events.last_24h_transfers ?? 0}`} />
        <MetricCard
          title="Outbox Failed"
          value={status?.notifications.outbox.failed ?? 0}
          hint={`24h delivery failed ${status?.notifications.last_24h_failed ?? 0}`}
          tone={status?.notifications.outbox.failed ? 'danger' : 'neutral'}
        />
        <MetricCard
          title="Provider Active"
          value={status?.providers.active ?? 0}
          hint={`inactive ${status?.providers.inactive ?? 0}`}
          tone={status?.providers.inactive ? 'warning' : 'success'}
        />
        <MetricCard
          title="服务在线"
          value={status?.services.online ?? 0}
          hint={`stale ${status?.services.stale ?? 0}`}
          tone={status?.services.stale ? 'danger' : 'success'}
        />
      </MetricGrid>

      <DataSurface title="扫描与通知摘要" actions={<Text type="tertiary">生成时间 {formatTime(status?.generated_at)}</Text>}>
        <div className="status-summary-grid">
          <SummaryItem label="扫描" value={`due ${status?.scans.due_addresses ?? 0} / overdue ${status?.scans.overdue_addresses ?? 0}`} />
          <SummaryItem label="事件" value={`transfer ${status?.events.last_24h_transfers ?? 0} / non-transfer ${status?.events.last_24h_non_transfers ?? 0}`} />
          <SummaryItem
            label="24h 通知"
            value={`sent ${status?.notifications.last_24h_sent ?? 0} / skipped ${status?.notifications.last_24h_skipped ?? 0} / failed ${status?.notifications.last_24h_failed ?? 0}`}
          />
          <SummaryItem label="站内未读" value={String(status?.notifications.unread_in_app ?? 0)} />
          <SummaryItem
            label="Outbox"
            value={`pending ${status?.notifications.outbox.pending ?? 0} / retryable ${status?.notifications.outbox.retryable ?? 0} / processing ${status?.notifications.outbox.processing ?? 0}`}
          />
          <SummaryItem label="下一次通知" value={formatTime(status?.notifications.outbox.next_due_at)} />
        </div>
      </DataSurface>

      <DataSurface title="服务心跳" actions={<Text type="tertiary">online {status?.services.online ?? 0} / stale {status?.services.stale ?? 0}</Text>}>
        <DataTable<ServiceHeartbeatStatusItem>
          tableId="system-service-heartbeats"
          loading={statusQuery.isLoading}
          dataSource={status?.services.items ?? []}
          rowKey={serviceHeartbeatRowKey}
          pagination={false}
          scroll={{ x: 900 }}
          columns={[
            { title: '服务', dataIndex: 'service_name', width: 140 },
            {
              title: '状态',
              dataIndex: 'status',
              width: 120,
              render: (_value, record) => <Tag color={serviceStatusColor(record)}>{record.is_stale ? 'stale' : record.status}</Tag>,
            },
            { title: '实例', dataIndex: 'instance_id', width: 140, render: value => shortInstanceId(String(value)) },
            { title: '启动时间', dataIndex: 'started_at', width: 190, render: value => formatTime(String(value)) },
            { title: '最后心跳', dataIndex: 'last_seen_at', width: 190, render: value => formatTime(String(value)) },
            { title: '超时阈值', dataIndex: 'stale_after_seconds', width: 110, render: value => `${String(value)}s` },
            { title: '运行信息', dataIndex: 'metadata', width: 160, render: value => metadataText(value as Record<string, unknown>) },
          ]}
        />
      </DataSurface>

      <DataSurface title="Provider 按链状态" actions={<Text type="tertiary">active {status?.providers.active ?? 0} / inactive {status?.providers.inactive ?? 0}</Text>}>
        <DataTable<ProviderChainStatus>
          tableId="system-provider-by-chain"
          loading={statusQuery.isLoading}
          dataSource={status?.providers.by_chain ?? []}
          rowKey="chain_id"
          pagination={false}
          columns={[
            { title: '链', dataIndex: 'chain_name', width: 220 },
            { title: 'Active', dataIndex: 'active', width: 120 },
            { title: 'Inactive', dataIndex: 'inactive', width: 120 },
          ]}
        />
      </DataSurface>

      <DataSurface title="Provider 明细">
        <DataTable<ProviderStatusItem>
          tableId="system-provider-items"
          loading={statusQuery.isLoading}
          dataSource={status?.providers.items ?? []}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          scroll={{ x: 1500 }}
          columns={[
            { title: '链', dataIndex: 'chain_name', width: 140 },
            { title: '名称', dataIndex: 'name', width: 160 },
            { title: '类型', dataIndex: 'provider_type', width: 120 },
            {
              title: '配置状态',
              dataIndex: 'status',
              width: 110,
              render: value => <Tag color={statusColor(String(value))}>{String(value)}</Tag>,
            },
            {
              title: '运行状态',
              dataIndex: 'health',
              width: 130,
              render: (_value, record) => <Tag color={circuitStatusColor(record.health.is_circuit_open)}>{circuitStatusText(record.health.is_circuit_open)}</Tag>,
            },
            {
              title: '连续失败',
              dataIndex: 'health',
              width: 110,
              render: (_value, record) => String(record.health.consecutive_failures),
            },
            {
              title: '最后成功',
              dataIndex: 'health',
              width: 190,
              render: (_value, record) => formatTime(record.health.last_success_at),
            },
            {
              title: '最后失败',
              dataIndex: 'health',
              width: 190,
              render: (_value, record) => formatTime(record.health.last_failure_at),
            },
            {
              title: '禁用至',
              dataIndex: 'health',
              width: 190,
              render: (_value, record) => formatTime(record.health.disabled_until),
            },
            {
              title: '最后错误',
              dataIndex: 'health',
              width: 260,
              ellipsis: { showTitle: true },
              render: (_value, record) => truncateError(record.health.last_error),
            },
            { title: '优先级', dataIndex: 'priority', width: 100 },
            { title: 'QPS', dataIndex: 'qps_limit', width: 100 },
            { title: '超时(ms)', dataIndex: 'timeout_ms', width: 120 },
            { title: 'URL', dataIndex: 'base_url', width: 260, ellipsis: { showTitle: true } },
          ]}
        />
      </DataSurface>
    </PageScaffold>
  );
}

function SummaryItem({ label, value }: { label: string; value: string }) {
  return (
    <div className="status-summary-item">
      <Text type="tertiary">{label}</Text>
      <div>{value}</div>
    </div>
  );
}
