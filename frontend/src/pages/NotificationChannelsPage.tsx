import { useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Banner, Button, Form, Popconfirm, Space, Tag, Toast } from '@douyinfe/semi-ui';
import {
  createNotificationChannel,
  deleteNotificationChannel,
  listNotificationChannels,
  listTelegramBots,
  testNotificationChannel,
  updateNotificationChannel,
  verifyNotificationChannel,
} from '../api/client';
import type {
  CreateNotificationChannelRequest,
  NotificationChannel,
  UpdateNotificationChannelRequest,
} from '../api/types';
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { FormModal } from '../components/FormModal';
import { PageScaffold } from '../components/PageScaffold';

type ChannelForm = {
  name?: string;
  channel_type?: string;
  status?: string;
  telegram_bot_id?: string;
  chat_id?: string;
  chat_alias?: string;
  message_template?: string;
  config_json?: string;
};

function parseConfigJson(value?: string) {
  if (!value?.trim()) return {};
  return JSON.parse(value) as Record<string, unknown>;
}

function channelPayload(values: ChannelForm): CreateNotificationChannelRequest | UpdateNotificationChannelRequest {
  const base = {
    channel_type: values.channel_type ?? 'telegram',
    name: values.name ?? '',
    status: values.status ?? 'active',
  };
  if (base.channel_type === 'telegram') {
    return {
      ...base,
      config: {
        telegram_bot_id: values.telegram_bot_id,
        chat_id: values.chat_id,
        chat_alias: values.chat_alias || undefined,
        message_template: values.message_template || undefined,
      },
    };
  }
  return { ...base, config: parseConfigJson(values.config_json) };
}

function initialChannelValues(channel: NotificationChannel | null): ChannelForm {
  if (!channel) return { channel_type: 'telegram', status: 'active' };
  const config = channel.config ?? {};
  return {
    name: channel.name,
    channel_type: channel.channel_type,
    status: channel.status,
    telegram_bot_id: typeof config.telegram_bot_id === 'string' ? config.telegram_bot_id : undefined,
    chat_id: typeof config.chat_id === 'string' ? config.chat_id : undefined,
    chat_alias: typeof config.chat_alias === 'string' ? config.chat_alias : undefined,
    message_template: typeof config.message_template === 'string' ? config.message_template : undefined,
    config_json: channel.channel_type === 'telegram' ? undefined : JSON.stringify(config, null, 2),
  };
}

function destinationSummary(channel: NotificationChannel) {
  if (channel.channel_type === 'telegram') {
    const chatId = typeof channel.config.chat_id === 'string' ? channel.config.chat_id : '-';
    const alias = typeof channel.config.chat_alias === 'string' ? channel.config.chat_alias : '';
    return alias ? `${alias} / ${chatId}` : chatId;
  }
  if (channel.channel_type === 'email') return String(channel.config.email ?? channel.config.recipient ?? '-');
  if (channel.channel_type === 'webhook') return String(channel.config.url ?? '-');
  return JSON.stringify(channel.config);
}

export function NotificationChannelsPage() {
  const [visible, setVisible] = useState(false);
  const [editingChannel, setEditingChannel] = useState<NotificationChannel | null>(null);
  const queryClient = useQueryClient();
  const channelsQuery = useQuery({ queryKey: ['notification-channels'], queryFn: listNotificationChannels });
  const botsQuery = useQuery({ queryKey: ['telegram-bots'], queryFn: listTelegramBots });
  const botMap = useMemo(() => new Map((botsQuery.data ?? []).map(bot => [bot.id, bot.name])), [botsQuery.data]);

  const saveMutation = useMutation({
    mutationFn: (values: ChannelForm) => {
      const payload = channelPayload(values);
      return editingChannel
        ? updateNotificationChannel(editingChannel.id, payload as UpdateNotificationChannelRequest)
        : createNotificationChannel(payload as CreateNotificationChannelRequest);
    },
    onSuccess: () => {
      Toast.success(editingChannel ? '通知渠道已更新' : '通知渠道已创建');
      setVisible(false);
      setEditingChannel(null);
      queryClient.invalidateQueries({ queryKey: ['notification-channels'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '通知渠道保存失败'),
  });

  const verifyMutation = useMutation({
    mutationFn: verifyNotificationChannel,
    onSuccess: response => Toast[response.ok ? 'success' : 'error'](response.message),
    onError: error => Toast.error(error instanceof Error ? error.message : '通知渠道验证失败'),
  });

  const testMutation = useMutation({
    mutationFn: testNotificationChannel,
    onSuccess: response => Toast[response.ok ? 'success' : 'error'](response.message),
    onError: error => Toast.error(error instanceof Error ? error.message : '测试发送失败'),
  });

  const deleteMutation = useMutation({
    mutationFn: deleteNotificationChannel,
    onSuccess: () => {
      Toast.success('通知渠道已删除');
      queryClient.invalidateQueries({ queryKey: ['notification-channels'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '通知渠道删除失败'),
  });

  function openCreateModal() {
    setEditingChannel(null);
    setVisible(true);
  }

  function openEditModal(channel: NotificationChannel) {
    setEditingChannel(channel);
    setVisible(true);
  }

  function closeModal() {
    setVisible(false);
    setEditingChannel(null);
  }

  return (
    <PageScaffold title="通知渠道" actions={<Button type="primary" onClick={openCreateModal}>新增渠道</Button>}>
      {channelsQuery.isError ? (
        <Banner
          type="danger"
          title="通知渠道加载失败"
          description={channelsQuery.error instanceof Error ? channelsQuery.error.message : '请求失败'}
        />
      ) : null}

      <DataSurface title="通知渠道列表">
        <DataTable<NotificationChannel>
          tableId="notification-channels"
          actionColumnKeys={['operations']}
          loading={channelsQuery.isLoading}
          dataSource={channelsQuery.data ?? []}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          scroll={{ x: 1300 }}
          columns={[
            { title: '名称', dataIndex: 'name', width: 180, ellipsis: { showTitle: true } },
            { title: '类型', dataIndex: 'channel_type', width: 120, render: value => <Tag>{String(value)}</Tag> },
            {
              title: '状态',
              dataIndex: 'status',
              width: 100,
              render: value => <Tag color={String(value) === 'active' ? 'green' : 'grey'}>{String(value)}</Tag>,
            },
            { title: '目的地', dataIndex: 'config', width: 260, ellipsis: { showTitle: true }, render: (_, channel) => destinationSummary(channel) },
            {
              title: 'TG机器人',
              dataIndex: 'config',
              width: 180,
              render: (_, channel) => {
                const botId = typeof channel.config.telegram_bot_id === 'string' ? channel.config.telegram_bot_id : '';
                return botId ? botMap.get(botId) ?? botId : '-';
              },
            },
            { title: '更新时间', dataIndex: 'updated_at', width: 190 },
            {
              title: '操作',
              key: 'operations',
              width: 260,
              render: (_, channel) => (
                <Space>
                  <Button size="small" onClick={() => openEditModal(channel)}>编辑</Button>
                  <Button size="small" disabled={channel.channel_type !== 'telegram'} loading={verifyMutation.isPending} onClick={() => verifyMutation.mutate(channel.id)}>验证</Button>
                  <Button size="small" disabled={channel.channel_type !== 'telegram'} loading={testMutation.isPending} onClick={() => testMutation.mutate(channel.id)}>测试发送</Button>
                  <Popconfirm title="确认删除该通知渠道？" onConfirm={() => deleteMutation.mutate(channel.id)}>
                    <Button size="small" type="danger">删除</Button>
                  </Popconfirm>
                </Space>
              ),
            },
          ]}
        />
      </DataSurface>

      <FormModal title={editingChannel ? '编辑通知渠道' : '新增通知渠道'} visible={visible} onCancel={closeModal} size="large">
        <Form<ChannelForm>
          initValues={initialChannelValues(editingChannel)}
          onSubmit={values => saveMutation.mutate(values)}
          labelPosition="left"
          labelWidth={120}
        >
          {({ formState }) => {
            const channelType = formState.values?.channel_type ?? 'telegram';
            return (
              <>
                <Form.Input field="name" label="名称" rules={[{ required: true, message: '请输入渠道名称' }]} />
                <Form.Select field="channel_type" label="类型" rules={[{ required: true, message: '请选择渠道类型' }]}>
                  <Form.Select.Option value="telegram">telegram</Form.Select.Option>
                  <Form.Select.Option value="in_app">in_app</Form.Select.Option>
                  <Form.Select.Option value="webhook">webhook</Form.Select.Option>
                  <Form.Select.Option value="email">email</Form.Select.Option>
                </Form.Select>
                <Form.Select field="status" label="状态" rules={[{ required: true, message: '请选择状态' }]}>
                  <Form.Select.Option value="active">active</Form.Select.Option>
                  <Form.Select.Option value="inactive">inactive</Form.Select.Option>
                </Form.Select>
                {channelType === 'telegram' ? (
                  <>
                    <Form.Select field="telegram_bot_id" label="TG机器人" filter rules={[{ required: true, message: '请选择TG机器人' }]}>
                      {(botsQuery.data ?? []).map(bot => <Form.Select.Option key={bot.id} value={bot.id}>{bot.name} / {bot.token_preview}</Form.Select.Option>)}
                    </Form.Select>
                    <Form.Input field="chat_id" label="Chat ID" rules={[{ required: true, message: '请输入 Chat ID' }]} />
                    <Form.Input field="chat_alias" label="会话别名" />
                    <Form.TextArea field="message_template" label="消息模板" autosize />
                  </>
                ) : (
                  <Form.TextArea field="config_json" label="配置 JSON" autosize rules={[{ required: channelType !== 'in_app', message: '请输入配置 JSON' }]} />
                )}
                <Space className="form-modal-actions">
                  <Button htmlType="submit" type="primary" loading={saveMutation.isPending}>保存</Button>
                  <Button htmlType="button" onClick={closeModal}>取消</Button>
                </Space>
              </>
            );
          }}
        </Form>
      </FormModal>
    </PageScaffold>
  );
}
