import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Banner, Button, Card, Space, Switch, Table, Tag, Toast } from '@douyinfe/semi-ui';
import { listInAppNotifications, markInAppNotificationRead } from '../api/client';
import type { InAppNotification } from '../api/types';

type InAppNotificationsPageProps = {
  onUnreadSettled?: (count: number) => void;
};

export function InAppNotificationsPage({ onUnreadSettled }: InAppNotificationsPageProps) {
  const [unreadOnly, setUnreadOnly] = useState(false);
  const queryClient = useQueryClient();

  const notificationsQuery = useQuery({
    queryKey: ['in-app-notifications', unreadOnly],
    queryFn: () => listInAppNotifications({ unread_only: unreadOnly || undefined }),
  });

  useEffect(() => {
    if (!notificationsQuery.data) return;
    onUnreadSettled?.(notificationsQuery.data.filter(notification => !notification.read_at).length);
  }, [notificationsQuery.data, onUnreadSettled]);

  const markReadMutation = useMutation({
    mutationFn: markInAppNotificationRead,
    onSuccess: () => {
      Toast.success('已标记为已读');
      queryClient.invalidateQueries({ queryKey: ['in-app-notifications'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '标记已读失败'),
  });

  return (
    <Space vertical align="start" spacing={16} className="content-stack">
      {notificationsQuery.isError ? (
        <Banner
          type="danger"
          title="站内通知加载失败"
          description={notificationsQuery.error instanceof Error ? notificationsQuery.error.message : '请求失败'}
        />
      ) : null}

      <Card title="站内通知筛选" className="filter-card">
        <Space>
          <Switch checked={unreadOnly} onChange={checked => setUnreadOnly(Boolean(checked))} />
          <span>只看未读</span>
          <Button onClick={() => notificationsQuery.refetch()}>刷新</Button>
        </Space>
      </Card>

      <Card title="站内通知">
        <Table<InAppNotification>
          loading={notificationsQuery.isLoading}
          dataSource={notificationsQuery.data ?? []}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          scroll={{ x: 1000 }}
          columns={[
            { title: '时间', dataIndex: 'created_at', width: 180, render: value => new Date(String(value)).toLocaleString() },
            { title: '标题', dataIndex: 'title', width: 180 },
            { title: '内容', dataIndex: 'body', width: 420, ellipsis: { showTitle: true } },
            {
              title: '状态',
              dataIndex: 'read_at',
              width: 100,
              render: value => value ? <Tag color="grey">已读</Tag> : <Tag color="red">未读</Tag>,
            },
            {
              title: '操作',
              width: 120,
              render: (_, notification) => (
                <Button
                  size="small"
                  disabled={Boolean(notification.read_at)}
                  loading={markReadMutation.isPending}
                  onClick={() => markReadMutation.mutate(notification.id)}
                >
                  标记已读
                </Button>
              ),
            },
          ]}
        />
      </Card>
    </Space>
  );
}
