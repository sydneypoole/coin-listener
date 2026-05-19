import { useQuery } from '@tanstack/react-query';
import { Banner, Card, Col, Row, Space, Table, Tag, Typography } from '@douyinfe/semi-ui';
import { getSystemStatus } from '../api/client';
import type { ProviderChainStatus, ProviderStatusItem, ServiceHeartbeatStatusItem } from '../api/types';

const { Text, Title } = Typography;

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
    <Space vertical align="start" spacing={16} className="content-stack">
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

      <Card title="运维状态总览" loading={statusQuery.isLoading}>
        <Row gutter={[16, 16]}>
          <Col span={8}>
            <Metric title="Scan Queue" value={formatDepth(status?.queues.scan_queue_depth)} hint={status?.queues.scan_queue_key ?? '-'} />
          </Col>
          <Col span={8}>
            <Metric title="Notify Queue" value={formatDepth(status?.queues.notify_queue_depth)} hint={status?.queues.notify_queue_key ?? '-'} />
          </Col>
          <Col span={8}>
            <Metric title="Active 地址" value={status?.scans.active_addresses ?? 0} hint="status = active" />
          </Col>
          <Col span={8}>
            <Metric title="Due 地址" value={status?.scans.due_addresses ?? 0} hint="next_scan_at <= now" />
          </Col>
          <Col span={8}>
            <Metric title="24h 事件" value={status?.events.last_24h_total ?? 0} hint={`transfer ${status?.events.last_24h_transfers ?? 0}`} />
          </Col>
          <Col span={8}>
            <Metric
              title="Outbox Failed"
              value={status?.notifications.outbox.failed ?? 0}
              hint={`24h delivery failed ${status?.notifications.last_24h_failed ?? 0}`}
            />
          </Col>
          <Col span={8}>
            <Metric title="服务在线" value={status?.services.online ?? 0} hint={`stale ${status?.services.stale ?? 0}`} />
          </Col>
        </Row>
      </Card>

      <Card title="扫描与通知摘要" loading={statusQuery.isLoading}>
        <Space vertical align="start">
          <Text>生成时间：{formatTime(status?.generated_at)}</Text>
          <Text>最近扫描时间：{formatTime(status?.scans.last_scanned_at)}</Text>
          <Text>过期未扫描地址：{status?.scans.overdue_addresses ?? 0}</Text>
          <Text>24h 转账事件：{status?.events.last_24h_transfers ?? 0}</Text>
          <Text>24h 非转账事件：{status?.events.last_24h_non_transfers ?? 0}</Text>
          <Text>
            24h 通知：sent {status?.notifications.last_24h_sent ?? 0} / skipped {status?.notifications.last_24h_skipped ?? 0} / failed{' '}
            {status?.notifications.last_24h_failed ?? 0} / unread {status?.notifications.unread_in_app ?? 0}
          </Text>
          <Text>
            Outbox：pending {status?.notifications.outbox.pending ?? 0} / retryable {status?.notifications.outbox.retryable ?? 0} / processing{' '}
            {status?.notifications.outbox.processing ?? 0} / failed {status?.notifications.outbox.failed ?? 0} / stale{' '}
            {status?.notifications.outbox.stale_processing ?? 0} / next due {formatTime(status?.notifications.outbox.next_due_at)}
          </Text>
          <Text>Provider：active {status?.providers.active ?? 0} / inactive {status?.providers.inactive ?? 0}</Text>
          <Text>服务：online {status?.services.online ?? 0} / stale {status?.services.stale ?? 0}</Text>
        </Space>
      </Card>

      <Card title="服务心跳" loading={statusQuery.isLoading}>
        <Table<ServiceHeartbeatStatusItem>
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
              render: (_value, record) => {
                const item = record as ServiceHeartbeatStatusItem;
                return <Tag color={serviceStatusColor(item)}>{item.is_stale ? 'stale' : item.status}</Tag>;
              },
            },
            { title: '实例', dataIndex: 'instance_id', width: 140, render: value => shortInstanceId(String(value)) },
            { title: '启动时间', dataIndex: 'started_at', width: 190, render: value => formatTime(String(value)) },
            { title: '最后心跳', dataIndex: 'last_seen_at', width: 190, render: value => formatTime(String(value)) },
            { title: '超时阈值', dataIndex: 'stale_after_seconds', width: 110, render: value => `${String(value)}s` },
            { title: '运行信息', dataIndex: 'metadata', width: 160, render: value => metadataText(value as Record<string, unknown>) },
          ]}
        />
      </Card>

      <Card title="Provider 按链状态" loading={statusQuery.isLoading}>
        <Table<ProviderChainStatus>
          dataSource={status?.providers.by_chain ?? []}
          rowKey="chain_id"
          pagination={false}
          columns={[
            { title: '链', dataIndex: 'chain_name' },
            { title: 'Active', dataIndex: 'active' },
            { title: 'Inactive', dataIndex: 'inactive' },
          ]}
        />
      </Card>

      <Card title="Provider 明细" loading={statusQuery.isLoading}>
        <Table<ProviderStatusItem>
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
              render: (_value, record) => {
                const item = record as ProviderStatusItem;
                return <Tag color={circuitStatusColor(item.health.is_circuit_open)}>{circuitStatusText(item.health.is_circuit_open)}</Tag>;
              },
            },
            {
              title: '连续失败',
              dataIndex: 'health',
              width: 110,
              render: (_value, record) => String((record as ProviderStatusItem).health.consecutive_failures),
            },
            {
              title: '最后成功',
              dataIndex: 'health',
              width: 190,
              render: (_value, record) => formatTime((record as ProviderStatusItem).health.last_success_at),
            },
            {
              title: '最后失败',
              dataIndex: 'health',
              width: 190,
              render: (_value, record) => formatTime((record as ProviderStatusItem).health.last_failure_at),
            },
            {
              title: '禁用至',
              dataIndex: 'health',
              width: 190,
              render: (_value, record) => formatTime((record as ProviderStatusItem).health.disabled_until),
            },
            {
              title: '最后错误',
              dataIndex: 'health',
              width: 260,
              ellipsis: { showTitle: true },
              render: (_value, record) => truncateError((record as ProviderStatusItem).health.last_error),
            },
            { title: '优先级', dataIndex: 'priority', width: 100 },
            { title: 'QPS', dataIndex: 'qps_limit', width: 100 },
            { title: '超时(ms)', dataIndex: 'timeout_ms', width: 120 },
            { title: 'URL', dataIndex: 'base_url', width: 260, ellipsis: { showTitle: true } },
          ]}
        />
      </Card>
    </Space>
  );
}

function Metric({ title, value, hint }: { title: string; value: string | number; hint: string }) {
  return (
    <Card className="status-card">
      <Space vertical align="start">
        <Text type="tertiary">{title}</Text>
        <Title heading={3}>{value}</Title>
        <Text type="tertiary">{hint}</Text>
      </Space>
    </Card>
  );
}
