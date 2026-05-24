import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Banner, Button, Form, Popconfirm, Space, Tag, Toast, Typography } from '@douyinfe/semi-ui';
import {
  createTelegramBot,
  deleteTelegramBot,
  getTelegramSettings,
  listTelegramBots,
  updateTelegramBot,
  updateTelegramSettings,
  verifyTelegramBot,
} from '../api/client';
import type { TelegramBot, UpdateTelegramBotRequest, UpdateTelegramSettingsRequest } from '../api/types';
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { FormModal } from '../components/FormModal';
import { PageScaffold } from '../components/PageScaffold';

const { Text } = Typography;

type ProxyMode = 'global' | 'bot';

type BotForm = {
  name?: string;
  bot_token?: string;
  status?: string;
  proxy_mode?: ProxyMode;
  proxy_url?: string;
};

type TelegramSettingsForm = {
  proxy_url?: string;
};

function proxySourceLabel(source: string) {
  if (source === 'bot') return '机器人代理';
  if (source === 'global') return '全局代理';
  return '直连';
}

function proxySourceColor(source: string): 'blue' | 'green' | 'grey' {
  if (source === 'bot') return 'blue';
  if (source === 'global') return 'green';
  return 'grey';
}

function buildProxyUrlPayload(values: BotForm, editing: boolean): Pick<UpdateTelegramBotRequest, 'proxy_url'> | Record<string, never> {
  if (values.proxy_mode === 'global') return { proxy_url: null };

  const proxyUrl = values.proxy_url?.trim();
  if (proxyUrl) return { proxy_url: proxyUrl };

  return editing ? {} : { proxy_url: null };
}

function buildTelegramSettingsPayload(values: TelegramSettingsForm): UpdateTelegramSettingsRequest {
  const proxyUrl = values.proxy_url?.trim();
  return { proxy_url: proxyUrl || null };
}

export function TelegramBotsPage() {
  const [visible, setVisible] = useState(false);
  const [editingBot, setEditingBot] = useState<TelegramBot | null>(null);
  const queryClient = useQueryClient();
  const botsQuery = useQuery({ queryKey: ['telegram-bots'], queryFn: listTelegramBots });
  const settingsQuery = useQuery({ queryKey: ['telegram-settings'], queryFn: getTelegramSettings });

  const settingsMutation = useMutation({
    mutationFn: (values: TelegramSettingsForm) => updateTelegramSettings(buildTelegramSettingsPayload(values)),
    onSuccess: () => {
      Toast.success('Telegram 全局代理已保存');
      queryClient.invalidateQueries({ queryKey: ['telegram-settings'] });
      queryClient.invalidateQueries({ queryKey: ['telegram-bots'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : 'Telegram 全局代理保存失败'),
  });

  const saveMutation = useMutation({
    mutationFn: (values: BotForm) => {
      const proxyPayload = buildProxyUrlPayload(values, Boolean(editingBot));

      if (editingBot) {
        return updateTelegramBot(editingBot.id, {
          name: values.name ?? '',
          bot_token: values.bot_token || null,
          status: values.status ?? 'active',
          ...proxyPayload,
        } satisfies UpdateTelegramBotRequest);
      }
      return createTelegramBot({
        name: values.name ?? '',
        bot_token: values.bot_token ?? '',
        status: values.status ?? 'active',
        ...proxyPayload,
      });
    },
    onSuccess: () => {
      Toast.success(editingBot ? 'TG机器人已更新' : 'TG机器人已创建');
      closeModal();
      queryClient.invalidateQueries({ queryKey: ['telegram-bots'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : 'TG机器人保存失败'),
  });

  const verifyMutation = useMutation({
    mutationFn: verifyTelegramBot,
    onSuccess: response => {
      Toast[response.ok ? 'success' : 'error'](response.message);
      queryClient.invalidateQueries({ queryKey: ['telegram-bots'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : 'TG机器人验证失败'),
  });

  const deleteMutation = useMutation({
    mutationFn: deleteTelegramBot,
    onSuccess: () => {
      Toast.success('TG机器人已删除');
      queryClient.invalidateQueries({ queryKey: ['telegram-bots'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : 'TG机器人删除失败'),
  });

  function openCreateModal() {
    setEditingBot(null);
    setVisible(true);
  }

  function openEditModal(bot: TelegramBot) {
    setEditingBot(bot);
    setVisible(true);
  }

  function closeModal() {
    setVisible(false);
    setEditingBot(null);
  }

  return (
    <PageScaffold
      title="TG机器人"
      description="配置 Telegram Bot Token、全局/单机器人代理，并验证机器人连通性。"
      actions={<Button type="primary" onClick={openCreateModal}>新增机器人</Button>}
    >
      {settingsQuery.isError ? (
        <Banner
          type="danger"
          title="Telegram 全局代理加载失败"
          description={settingsQuery.error instanceof Error ? settingsQuery.error.message : '请求失败'}
        />
      ) : null}

      {botsQuery.isError ? (
        <Banner
          type="danger"
          title="TG机器人加载失败"
          description={botsQuery.error instanceof Error ? botsQuery.error.message : '请求失败'}
        />
      ) : null}

      <DataSurface
        title="Telegram 全局代理"
        actions={<Tag color={settingsQuery.data?.has_proxy ? 'green' : 'grey'}>{settingsQuery.data?.has_proxy ? '已配置' : '直连'}</Tag>}
      >
        <Space vertical align="start" spacing="medium" style={{ width: '100%' }}>
          <Space>
            <Text type="secondary">当前代理</Text>
            <Text className="table-cell-mono">{settingsQuery.data?.proxy_url_preview ?? '直连'}</Text>
          </Space>
          <Form<TelegramSettingsForm>
            layout="horizontal"
            labelPosition="left"
            labelWidth={100}
            initValues={{ proxy_url: '' }}
            onSubmit={values => settingsMutation.mutate(values)}
          >
            <Form.Input field="proxy_url" label="proxy_url" placeholder="留空保存会清除全局代理" style={{ width: 420 }} />
            <Space>
              <Button htmlType="submit" type="primary" loading={settingsMutation.isPending}>保存全局代理</Button>
              <Button htmlType="button" loading={settingsMutation.isPending} onClick={() => settingsMutation.mutate({ proxy_url: '' })}>清除全局代理</Button>
            </Space>
          </Form>
        </Space>
      </DataSurface>

      <DataSurface title="TG机器人列表">
        <DataTable<TelegramBot>
          tableId="telegram-bots"
          actionColumnKeys={['operations']}
          loading={botsQuery.isLoading}
          dataSource={botsQuery.data ?? []}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          scroll={{ x: 1450 }}
          columns={[
            { title: '名称', dataIndex: 'name', width: 180, ellipsis: { showTitle: true } },
            { title: 'Token', dataIndex: 'token_preview', width: 180, className: 'table-cell-mono' },
            {
              title: '代理来源',
              dataIndex: 'proxy_source',
              width: 130,
              render: value => <Tag color={proxySourceColor(String(value))}>{proxySourceLabel(String(value))}</Tag>,
            },
            {
              title: '代理',
              dataIndex: 'proxy_url_preview',
              width: 220,
              className: 'table-cell-mono',
              ellipsis: { showTitle: true },
              render: value => value ? String(value) : '直连',
            },
            {
              title: '状态',
              dataIndex: 'status',
              width: 100,
              render: value => <Tag color={value === 'active' ? 'green' : 'grey'}>{String(value)}</Tag>,
            },
            {
              title: '验证',
              dataIndex: 'verification_status',
              width: 120,
              render: value => <Tag color={value === 'verified' ? 'green' : value === 'failed' ? 'red' : 'orange'}>{String(value)}</Tag>,
            },
            { title: '最后验证', dataIndex: 'last_verified_at', width: 190, render: value => value ? String(value) : '-' },
            { title: '错误', dataIndex: 'last_error', width: 260, ellipsis: { showTitle: true }, render: value => value ? String(value) : '-' },
            {
              title: '操作',
              key: 'operations',
              width: 220,
              render: (_, bot) => (
                <Space>
                  <Button size="small" onClick={() => openEditModal(bot)}>编辑</Button>
                  <Button size="small" loading={verifyMutation.isPending} onClick={() => verifyMutation.mutate(bot.id)}>验证</Button>
                  <Popconfirm title="确认删除该TG机器人？" onConfirm={() => deleteMutation.mutate(bot.id)}>
                    <Button size="small" type="danger">删除</Button>
                  </Popconfirm>
                </Space>
              ),
            },
          ]}
        />
      </DataSurface>

      <FormModal title={editingBot ? '编辑TG机器人' : '新增TG机器人'} visible={visible} onCancel={closeModal} size="large">
        <Form<BotForm>
          initValues={editingBot ? {
            name: editingBot.name,
            status: editingBot.status,
            proxy_mode: editingBot.proxy_source === 'bot' ? 'bot' : 'global',
            proxy_url: '',
          } : { status: 'active', proxy_mode: 'global', proxy_url: '' }}
          onSubmit={values => saveMutation.mutate(values)}
          labelPosition="left"
          labelWidth={110}
        >
          <Form.Input field="name" label="名称" rules={[{ required: true, message: '请输入机器人名称' }]} />
          <Form.Input
            field="bot_token"
            label="Bot Token"
            mode="password"
            rules={editingBot ? [] : [{ required: true, message: '请输入 Bot Token' }]}
            placeholder={editingBot ? '留空表示不更换 Token' : '请输入 Telegram Bot Token'}
          />
          <Form.Select field="status" label="状态">
            <Form.Select.Option value="active">active</Form.Select.Option>
            <Form.Select.Option value="inactive">inactive</Form.Select.Option>
          </Form.Select>
          <Form.Select field="proxy_mode" label="代理来源" rules={[{ required: true, message: '请选择代理来源' }]}>
            <Form.Select.Option value="global">使用全局代理</Form.Select.Option>
            <Form.Select.Option value="bot">此机器人单独配置代理</Form.Select.Option>
          </Form.Select>
          <Form.Input
            field="proxy_url"
            label="proxy_url"
            placeholder={editingBot?.proxy_source === 'bot' ? '留空表示不更换代理 URL' : '仅单独代理模式填写'}
          />
          <Space className="form-modal-actions">
            <Button htmlType="submit" type="primary" loading={saveMutation.isPending}>保存</Button>
            <Button htmlType="button" onClick={closeModal}>取消</Button>
          </Space>
        </Form>
      </FormModal>
    </PageScaffold>
  );
}
