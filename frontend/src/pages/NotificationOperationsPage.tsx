import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Banner, Button, Card, Col, Form, Modal, Row, Space, Table, Tag, Toast, Typography } from '@douyinfe/semi-ui';
import { getNotificationOutbox, getSystemStatus, listNotificationOutbox, retryNotificationOutbox } from '../api/client';
import type { NotificationDeliveryListItem, NotificationOutboxListItem, NotificationOutboxQuery } from '../api/types';

const { Text, Title } = Typography;

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
    <Space vertical align="start" spacing={16} className="content-stack">
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

      <Card title="通知任务积压总览" loading={statusQuery.isLoading}>
        <Row gutter={[16, 16]}>
          <Col span={8}>
            <Metric title="Pending" value={outbox?.pending ?? 0} hint="等待 notifier claim" />
          </Col>
          <Col span={8}>
            <Metric title="Retryable" value={outbox?.retryable ?? 0} hint="等待自动重试" />
          </Col>
          <Col span={8}>
            <Metric title="Processing" value={outbox?.processing ?? 0} hint={`stale ${outbox?.stale_processing ?? 0}`} />
          </Col>
          <Col span={8}>
            <Metric title="Failed" value={outbox?.failed ?? 0} hint="可人工重试" />
          </Col>
          <Col span={8}>
            <Metric title="Stale Processing" value={outbox?.stale_processing ?? 0} hint="locked_at 超过 15 分钟" />
          </Col>
          <Col span={8}>
            <Metric title="Next Due" value={formatTime(outbox?.next_due_at)} hint="pending/retryable due" />
          </Col>
        </Row>
      </Card>

      <Card title="Outbox 筛选" className="filter-card">
        <Form<FilterForm> layout="horizontal" onSubmit={handleFilterSubmit} labelPosition="left">
          {({ formApi }) => (
            <>
              <Form.Select field="status" label="状态" showClear placeholder="全部状态" optionList={outboxStatusOptions} />
              <Form.Input field="event_id" label="Event ID" placeholder="按 event UUID 查询" style={{ width: 360 }} />
              <Space>
                <Button htmlType="submit" type="primary">查询</Button>
                <Button onClick={() => resetFilters(formApi)}>重置</Button>
                <Button onClick={() => outboxQuery.refetch()}>刷新</Button>
              </Space>
            </>
          )}
        </Form>
      </Card>

      <Card title="Notification Outbox">
        <Table<NotificationOutboxListItem>
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
              width: 170,
              fixed: 'right',
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
      </Card>

      <OutboxDetailModal
        visible={Boolean(selectedOutboxId)}
        loading={detailQuery.isLoading}
        detail={detailQuery.data}
        onClose={() => setSelectedOutboxId(undefined)}
      />
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

function OutboxDetailModal({
  visible,
  loading,
  detail,
  onClose,
}: {
  visible: boolean;
  loading: boolean;
  detail?: {
    outbox: NotificationOutboxListItem;
    event: { id: string; event_type: string; direction: string; tx_hash?: string | null; metadata: Record<string, unknown> };
    deliveries: NotificationDeliveryListItem[];
  };
  onClose: () => void;
}) {
  return (
    <Modal title="通知任务详情" visible={visible} onCancel={onClose} footer={null} width={1100}>
      {loading ? <Text>正在加载详情...</Text> : null}
      {detail ? (
        <Space vertical align="start" spacing={16} style={{ width: '100%' }}>
          <Card title="Outbox">
            <Space vertical align="start">
              <Text>ID：{detail.outbox.id}</Text>
              <Text>Event：{detail.outbox.event_id}</Text>
              <Text>状态：<Tag color={outboxStatusColor(detail.outbox.status)}>{detail.outbox.status}</Tag></Text>
              <Text>Attempt：{detail.outbox.attempt_count}</Text>
              <Text>Next Attempt：{formatTime(detail.outbox.next_attempt_at)}</Text>
              <Text>Lock：{detail.outbox.locked_by ?? '-'} / {formatTime(detail.outbox.locked_at)}</Text>
              <Text>Last Error：{detail.outbox.last_error ?? '-'}</Text>
            </Space>
          </Card>

          <Card title="Event">
            <Space vertical align="start">
              <Text>类型：{detail.event.event_type}</Text>
              <Text>方向：{detail.event.direction}</Text>
              <Text>交易哈希：{detail.event.tx_hash ?? '-'}</Text>
              <pre style={{ maxWidth: 1000, whiteSpace: 'pre-wrap', wordBreak: 'break-word' }}>
                {JSON.stringify(detail.event.metadata, null, 2)}
              </pre>
            </Space>
          </Card>

          <Card title="Deliveries">
            <Table<NotificationDeliveryListItem>
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
          </Card>
        </Space>
      ) : null}
    </Modal>
  );
}
