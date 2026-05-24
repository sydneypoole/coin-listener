import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Banner, Button, Card, Form, Space, Tag, Toast, Typography } from '@douyinfe/semi-ui';
import { getNotificationOutbox, getSystemStatus, listNotificationOutbox, retryNotificationOutbox } from '../api/client';
import type { NotificationDeliveryListItem, NotificationOutboxDetail, NotificationOutboxListItem, NotificationOutboxQuery } from '../api/types';
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { FilterPanel } from '../components/FilterPanel';
import { FormModal } from '../components/FormModal';
import { MetricCard, MetricGrid } from '../components/MetricGrid';
import { PageScaffold } from '../components/PageScaffold';

const { Text } = Typography;

type FilterForm = {
  status?: string;
  event_id?: string;
};

const outboxStatusOptions = [
  { label: 'pending', value: 'pending' },
  { label: 'processing', value: 'processing' },
  { label: 'retryable', value: 'retryable' },
  { label: 'delivered', value: 'delivered' },
  { label: 'failed', value: 'failed' },
];

function formatTime(value?: string | null) {
  return value ? new Date(value).toLocaleString() : '-';
}

function truncate(value?: string | null, maxLength = 160) {
  if (!value) return '-';
  return value.length > maxLength ? `${value.slice(0, maxLength)}...` : value;
}

function outboxStatusColor(status: string): 'green' | 'red' | 'orange' | 'blue' | 'grey' {
  if (status === 'delivered') return 'green';
  if (status === 'failed') return 'red';
  if (status === 'retryable') return 'orange';
  if (status === 'processing') return 'blue';
  return 'grey';
}

function deliveryStatusColor(status: string): 'green' | 'red' | 'orange' | 'blue' | 'grey' {
  if (status === 'sent') return 'green';
  if (status === 'failed') return 'red';
  if (status === 'skipped') return 'orange';
  if (status === 'processing') return 'blue';
  return 'grey';
}

function retryableOutbox(status: string) {
  return status === 'failed' || status === 'retryable';
}

export function NotificationOperationsPage() {
  const [filters, setFilters] = useState<NotificationOutboxQuery>({ limit: 50, offset: 0 });
  const [selectedOutboxId, setSelectedOutboxId] = useState<string>();
  const queryClient = useQueryClient();

  const statusQuery = useQuery({
    queryKey: ['system-status'],
    queryFn: getSystemStatus,
    refetchInterval: 10_000,
  });

  const outboxQuery = useQuery({
    queryKey: ['notification-outbox', filters],
    queryFn: () => listNotificationOutbox(filters),
  });

  const detailQuery = useQuery({
    queryKey: ['notification-outbox-detail', selectedOutboxId],
    queryFn: () => getNotificationOutbox(selectedOutboxId ?? ''),
    enabled: Boolean(selectedOutboxId),
  });

  const retryMutation = useMutation({
    mutationFn: retryNotificationOutbox,
    onSuccess: () => {
      Toast.success('通知任务已重新进入重试队列');
      queryClient.invalidateQueries({ queryKey: ['notification-outbox'] });
      queryClient.invalidateQueries({ queryKey: ['notification-outbox-detail'] });
      queryClient.invalidateQueries({ queryKey: ['system-status'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '重试通知任务失败'),
  });

  function handleFilterSubmit(values: Record<string, unknown>) {
    const form = values as FilterForm;
    const eventId = form.event_id?.trim();
    setFilters({
      status: form.status || undefined,
      event_id: eventId || undefined,
      limit: 50,
      offset: 0,
    });
  }

  function resetFilters(formApi: { reset: () => void }) {
    formApi.reset();
    setFilters({ limit: 50, offset: 0 });
  }

  const outbox = statusQuery.data?.notifications.outbox;

  return (
    <PageScaffold
      title="通知运维"
      description="跟踪 Notification Outbox 积压、重试窗口与投递明细。"
      actions={<Tag color={statusQuery.isFetching ? 'blue' : 'green'}>{statusQuery.isFetching ? 'refreshing' : '10s auto refresh'}</Tag>}
    >
      {outboxQuery.isError ? (
        <Banner
          type="danger"
          title="通知任务加载失败"
          description={outboxQuery.error instanceof Error ? outboxQuery.error.message : '请求失败'}
        />
      ) : null}

      {detailQuery.isError ? (
        <Banner
          type="danger"
          title="通知任务详情加载失败"
          description={detailQuery.error instanceof Error ? detailQuery.error.message : '请求失败'}
        />
      ) : null}

      <MetricGrid>
        <MetricCard title="Pending" value={outbox?.pending ?? 0} hint="等待 notifier claim" />
        <MetricCard title="Retryable" value={outbox?.retryable ?? 0} hint="等待自动重试" tone={outbox?.retryable ? 'warning' : 'neutral'} />
        <MetricCard title="Processing" value={outbox?.processing ?? 0} hint={`stale ${outbox?.stale_processing ?? 0}`} />
        <MetricCard title="Failed" value={outbox?.failed ?? 0} hint="可人工重试" tone={outbox?.failed ? 'danger' : 'neutral'} />
        <MetricCard title="Stale Processing" value={outbox?.stale_processing ?? 0} hint="locked_at 超过 15 分钟" tone={outbox?.stale_processing ? 'danger' : 'neutral'} />
        <MetricCard title="Next Due" value={formatTime(outbox?.next_due_at)} hint="pending/retryable due" />
      </MetricGrid>

      <FilterPanel title="Outbox 筛选">
        <Form<FilterForm> layout="horizontal" onSubmit={handleFilterSubmit} labelPosition="left">
          {({ formApi }) => (
            <>
              <Form.Select field="status" label="状态" showClear placeholder="全部状态" optionList={outboxStatusOptions} />
              <Form.Input field="event_id" label="Event ID" placeholder="按 event UUID 查询" style={{ width: 360 }} />
              <Space>
                <Button htmlType="submit" type="primary">查询</Button>
                <Button onClick={() => resetFilters(formApi)}>重置</Button>
                <Button loading={outboxQuery.isFetching} onClick={() => outboxQuery.refetch()}>刷新</Button>
              </Space>
            </>
          )}
        </Form>
      </FilterPanel>

      <DataSurface title="Notification Outbox" actions={<Text type="tertiary">limit {filters.limit ?? 50} / offset {filters.offset ?? 0}</Text>}>
        <DataTable<NotificationOutboxListItem>
          tableId="notification-outbox"
          loading={outboxQuery.isLoading}
          dataSource={outboxQuery.data?.items ?? []}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          scroll={{ x: 1700 }}
          columns={[
            { title: '创建时间', dataIndex: 'created_at', width: 180, render: value => formatTime(String(value)) },
            {
              title: '状态',
              dataIndex: 'status',
              width: 130,
              render: (_, row) => (
                <Space>
                  <Tag color={outboxStatusColor(row.status)}>{row.status}</Tag>
                  {row.is_stale_processing ? <Tag color="red">stale</Tag> : null}
                </Space>
              ),
            },
            { title: 'Event', dataIndex: 'event_id', width: 260, ellipsis: { showTitle: true } },
            { title: '事件类型', dataIndex: 'event_type', width: 130, render: value => value ? <Tag>{String(value)}</Tag> : '-' },
            { title: '方向', dataIndex: 'direction', width: 90, render: value => value ? String(value) : '-' },
            { title: '交易哈希', dataIndex: 'tx_hash', width: 240, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
            { title: 'Attempt', dataIndex: 'attempt_count', width: 100 },
            { title: 'Next Attempt', dataIndex: 'next_attempt_at', width: 180, render: value => formatTime(String(value)) },
            { title: 'Locked By', dataIndex: 'locked_by', width: 160, render: value => value ? String(value) : '-' },
            { title: 'Locked At', dataIndex: 'locked_at', width: 180, render: value => formatTime(value ? String(value) : null) },
            {
              title: 'Delivery',
              width: 170,
              render: (_, row) => `${row.delivery_sent}/${row.delivery_failed}/${row.delivery_skipped} / total ${row.delivery_total}`,
            },
            { title: 'Last Error', dataIndex: 'last_error', width: 280, ellipsis: { showTitle: true }, render: value => truncate(value ? String(value) : null) },
            {
              title: '操作',
              key: 'operations',
              width: 170,
              render: (_, row) => (
                <Space>
                  <Button size="small" onClick={() => setSelectedOutboxId(row.id)}>详情</Button>
                  <Button
                    size="small"
                    type="primary"
                    disabled={!retryableOutbox(row.status)}
                    loading={retryMutation.isPending}
                    onClick={() => retryMutation.mutate(row.id)}
                  >
                    重试
                  </Button>
                </Space>
              ),
            },
          ]}
        />
      </DataSurface>

      <OutboxDetailModal
        visible={Boolean(selectedOutboxId)}
        loading={detailQuery.isLoading}
        detail={detailQuery.data}
        onClose={() => setSelectedOutboxId(undefined)}
      />
    </PageScaffold>
  );
}

function OutboxDetailModal({
  visible,
  loading,
  detail,
  onClose,
}: {
  visible: boolean;
  loading: boolean;
  detail?: NotificationOutboxDetail;
  onClose: () => void;
}) {
  return (
    <FormModal title="通知任务详情" visible={visible} onCancel={onClose} size="wide">
      {loading ? <Text>正在加载详情...</Text> : null}
      {detail ? (
        <Space vertical align="start" spacing={16} style={{ width: '100%' }}>
          <div className="notification-detail-grid">
            <Card title="Outbox" className="notification-detail-card">
              <DetailLine label="ID" value={detail.outbox.id} mono />
              <DetailLine label="Event" value={detail.outbox.event_id} mono />
              <div className="detail-line">
                <Text type="tertiary">状态</Text>
                <Tag color={outboxStatusColor(detail.outbox.status)}>{detail.outbox.status}</Tag>
              </div>
              <DetailLine label="Attempt" value={String(detail.outbox.attempt_count)} />
              <DetailLine label="Next Attempt" value={formatTime(detail.outbox.next_attempt_at)} />
              <DetailLine label="Lock" value={`${detail.outbox.locked_by ?? '-'} / ${formatTime(detail.outbox.locked_at)}`} />
              <DetailLine label="Last Error" value={detail.outbox.last_error ?? '-'} />
            </Card>

            <Card title="Event" className="notification-detail-card">
              <DetailLine label="类型" value={detail.event.event_type} />
              <DetailLine label="方向" value={detail.event.direction} />
              <DetailLine label="交易哈希" value={detail.event.tx_hash ?? '-'} mono />
              <div className="detail-line detail-line-vertical">
                <Text type="tertiary">Metadata</Text>
                <pre className="detail-json">{JSON.stringify(detail.event.metadata, null, 2)}</pre>
              </div>
            </Card>
          </div>

          <DataSurface title="Deliveries">
            <DataTable<NotificationDeliveryListItem>
              tableId="notification-deliveries"
              dataSource={detail.deliveries}
              rowKey="id"
              pagination={{ pageSize: 5 }}
              scroll={{ x: 1500 }}
              columns={[
                { title: '创建时间', dataIndex: 'created_at', width: 180, render: value => formatTime(String(value)) },
                { title: '渠道', dataIndex: 'channel_type', width: 110, render: value => value ? <Tag>{String(value)}</Tag> : '-' },
                { title: '状态', dataIndex: 'status', width: 110, render: value => <Tag color={deliveryStatusColor(String(value))}>{String(value)}</Tag> },
                { title: 'Attempt', dataIndex: 'attempt_count', width: 90 },
                { title: 'Rule ID', dataIndex: 'rule_id', width: 240, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
                { title: 'Channel ID', dataIndex: 'channel_id', width: 240, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
                { title: 'Idempotency Key', dataIndex: 'idempotency_key', width: 320, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
                { title: 'Provider Message', dataIndex: 'provider_message_id', width: 180, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
                { title: 'Provider Status', dataIndex: 'provider_status_code', width: 130, render: value => value ?? '-' },
                { title: 'Provider Response', dataIndex: 'provider_response', width: 260, ellipsis: { showTitle: true }, render: value => truncate(value ? String(value) : null, 120) },
                { title: 'Last Error', dataIndex: 'last_error', width: 260, ellipsis: { showTitle: true }, render: value => truncate(value ? String(value) : null, 120) },
              ]}
            />
          </DataSurface>
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
