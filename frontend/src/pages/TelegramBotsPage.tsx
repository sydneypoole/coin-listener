import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Banner, Button, Form, Popconfirm, Space, Tag, Toast } from '@douyinfe/semi-ui';
import {
  createTelegramBot,
  deleteTelegramBot,
  listTelegramBots,
  updateTelegramBot,
  verifyTelegramBot,
} from '../api/client';
import type { TelegramBot, UpdateTelegramBotRequest } from '../api/types';
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { FormModal } from '../components/FormModal';
import { PageScaffold } from '../components/PageScaffold';

type BotForm = {
  name?: string;
  bot_token?: string;
  status?: string;
};

export function TelegramBotsPage() {
  const [visible, setVisible] = useState(false);
  const [editingBot, setEditingBot] = useState<TelegramBot | null>(null);
  const queryClient = useQueryClient();
  const botsQuery = useQuery({ queryKey: ['telegram-bots'], queryFn: listTelegramBots });

  const saveMutation = useMutation({
    mutationFn: (values: BotForm) => {
      if (editingBot) {
        return updateTelegramBot(editingBot.id, {
          name: values.name ?? '',
          bot_token: values.bot_token || null,
          status: values.status ?? 'active',
        } satisfies UpdateTelegramBotRequest);
      }
      return createTelegramBot({
        name: values.name ?? '',
        bot_token: values.bot_token ?? '',
        status: values.status ?? 'active',
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
    <PageScaffold title="TG机器人" actions={<Button type="primary" onClick={openCreateModal}>新增机器人</Button>}>
      {botsQuery.isError ? (
        <Banner
          type="danger"
          title="TG机器人加载失败"
          description={botsQuery.error instanceof Error ? botsQuery.error.message : '请求失败'}
        />
      ) : null}

      <DataSurface title="TG机器人列表">
        <DataTable<TelegramBot>
          tableId="telegram-bots"
          actionColumnKeys={['operations']}
          loading={botsQuery.isLoading}
          dataSource={botsQuery.data ?? []}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          scroll={{ x: 1100 }}
          columns={[
            { title: '名称', dataIndex: 'name', width: 180, ellipsis: { showTitle: true } },
            { title: 'Token', dataIndex: 'token_preview', width: 180, className: 'table-cell-mono' },
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
          initValues={editingBot ? { name: editingBot.name, status: editingBot.status } : { status: 'active' }}
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
          <Space className="form-modal-actions">
            <Button htmlType="submit" type="primary" loading={saveMutation.isPending}>保存</Button>
            <Button htmlType="button" onClick={closeModal}>取消</Button>
          </Space>
        </Form>
      </FormModal>
    </PageScaffold>
  );
}
