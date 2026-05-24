import { useEffect, useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Banner, Button, Form, Space, Tag, Toast } from '@douyinfe/semi-ui';
import {
  createNotificationChannel,
  createNotificationRule,
  deleteNotificationRule,
  listAssets,
  listChains,
  listNotificationChannels,
  listNotificationRules,
  listTelegramBots,
  listWatchedAddresses,
  updateNotificationRule,
} from '../api/client';
import type { FormApi } from '@douyinfe/semi-ui/lib/es/form/interface';
import type { CreateNotificationRuleRequest, NotificationRule, TelegramBindingRequest } from '../api/types';
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { FormModal } from '../components/FormModal';
import { PageScaffold } from '../components/PageScaffold';
import { TelegramBindingPanel } from '../components/TelegramBindingPanel';

const eventTypeOptions = [
  { label: 'transfer', value: 'transfer' },
  { label: 'balance_change', value: 'balance_change' },
  { label: 'fee_only_change', value: 'fee_only_change' },
  { label: 'contract_interaction', value: 'contract_interaction' },
  { label: 'unknown', value: 'unknown' },
];

const directionOptions = [
  { label: 'in', value: 'in' },
  { label: 'out', value: 'out' },
  { label: 'self', value: 'self' },
  { label: 'unknown', value: 'unknown' },
];

type RuleForm = {
  name?: string;
  chain_id?: string;
  address_id?: string;
  asset_id?: string;
  event_type?: string;
  direction?: string;
  is_transfer?: string;
  min_amount_raw?: string;
  channel_ids?: string[];
  enabled?: boolean;
};

type QuickChannelForm = {
  name?: string;
  telegram_bot_id?: string;
  chat_id?: string;
  chat_alias?: string;
  message_template?: string;
};

type RuleFormApi = FormApi<RuleForm>;
type QuickChannelFormApi = FormApi<QuickChannelForm>;

export function NotificationRulesPage() {
  const [editingRule, setEditingRule] = useState<NotificationRule | null>(null);
  const [modalVisible, setModalVisible] = useState(false);
  const [quickChannelVisible, setQuickChannelVisible] = useState(false);
  const [quickCreatedChannelId, setQuickCreatedChannelId] = useState<string | null>(null);
  const [ruleFormApi, setRuleFormApi] = useState<RuleFormApi | null>(null);
  const [quickChannelFormApi, setQuickChannelFormApi] = useState<QuickChannelFormApi | null>(null);
  const queryClient = useQueryClient();

  const rulesQuery = useQuery({ queryKey: ['notification-rules'], queryFn: listNotificationRules });
  const channelsQuery = useQuery({ queryKey: ['notification-channels'], queryFn: listNotificationChannels });
  const telegramBotsQuery = useQuery({ queryKey: ['telegram-bots'], queryFn: listTelegramBots });
  const chainsQuery = useQuery({ queryKey: ['chains'], queryFn: listChains });
  const assetsQuery = useQuery({ queryKey: ['assets'], queryFn: listAssets });
  const addressesQuery = useQuery({ queryKey: ['addresses'], queryFn: listWatchedAddresses });

  const chainMap = useMemo(() => new Map((chainsQuery.data ?? []).map(chain => [chain.id, chain.name])), [chainsQuery.data]);
  const assetMap = useMemo(() => new Map((assetsQuery.data ?? []).map(asset => [asset.id, asset.symbol])), [assetsQuery.data]);
  const addressMap = useMemo(() => new Map((addressesQuery.data ?? []).map(address => [address.id, address])), [addressesQuery.data]);
  const channelMap = useMemo(() => new Map((channelsQuery.data ?? []).map(channel => [channel.id, channel.name])), [channelsQuery.data]);

  useEffect(() => {
    if (!quickCreatedChannelId || !ruleFormApi) return;
    const currentValue = ruleFormApi.getValue('channel_ids');
    const channelIds = Array.isArray(currentValue) ? currentValue.map(String) : [];
    if (!channelIds.includes(quickCreatedChannelId)) {
      ruleFormApi.setValue('channel_ids', [...channelIds, quickCreatedChannelId]);
    }
  }, [quickCreatedChannelId, ruleFormApi]);

  const saveMutation = useMutation({
    mutationFn: (payload: CreateNotificationRuleRequest) => (
      editingRule ? updateNotificationRule(editingRule.id, payload) : createNotificationRule(payload)
    ),
    onSuccess: () => {
      Toast.success(editingRule ? '通知规则已更新' : '通知规则已创建');
      setModalVisible(false);
      setEditingRule(null);
      queryClient.invalidateQueries({ queryKey: ['notification-rules'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '通知规则保存失败'),
  });

  const deleteMutation = useMutation({
    mutationFn: deleteNotificationRule,
    onSuccess: () => {
      Toast.success('通知规则已删除');
      queryClient.invalidateQueries({ queryKey: ['notification-rules'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '通知规则删除失败'),
  });

  const quickChannelMutation = useMutation({
    mutationFn: (values: Record<string, unknown>) => createNotificationChannel({
      channel_type: 'telegram',
      name: String(values.name),
      status: 'active',
      config: {
        telegram_bot_id: String(values.telegram_bot_id),
        chat_id: String(values.chat_id),
        chat_alias: values.chat_alias ? String(values.chat_alias) : undefined,
        message_template: values.message_template ? String(values.message_template) : undefined,
      },
    }),
    onSuccess: channel => {
      Toast.success('通知渠道已创建');
      setQuickCreatedChannelId(channel.id);
      setQuickChannelVisible(false);
      queryClient.invalidateQueries({ queryKey: ['notification-channels'] });
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '通知渠道创建失败'),
  });

  function openCreateModal() {
    setEditingRule(null);
    setQuickCreatedChannelId(null);
    setModalVisible(true);
  }

  function openEditModal(rule: NotificationRule) {
    setEditingRule(rule);
    setQuickCreatedChannelId(null);
    setModalVisible(true);
  }

  function refreshChannels() {
    queryClient.invalidateQueries({ queryKey: ['notification-channels'] });
  }

  function handleQuickTelegramBound(binding: TelegramBindingRequest) {
    const chatId = binding.chat_id ?? '';
    const alias = binding.chat_title || binding.chat_username || chatId || '';
    quickChannelFormApi?.setValue('chat_id', chatId);
    quickChannelFormApi?.setValue('chat_alias', alias);
  }

  function handleSubmit(values: Record<string, unknown>) {
    const form = values as RuleForm;
    saveMutation.mutate({
      name: form.name ?? '',
      chain_id: form.chain_id || null,
      address_id: form.address_id || null,
      asset_id: form.asset_id || null,
      event_type: form.event_type || null,
      direction: form.direction || null,
      is_transfer: form.is_transfer === undefined ? null : form.is_transfer === 'true',
      min_amount_raw: form.min_amount_raw || null,
      channel_ids: form.channel_ids ?? [],
      enabled: form.enabled ?? true,
    });
  }

  function initialValues(): RuleForm {
    if (!editingRule) {
      return { enabled: true, channel_ids: [] };
    }
    return {
      name: editingRule.name,
      chain_id: editingRule.chain_id ?? undefined,
      address_id: editingRule.address_id ?? undefined,
      asset_id: editingRule.asset_id ?? undefined,
      event_type: editingRule.event_type ?? undefined,
      direction: editingRule.direction ?? undefined,
      is_transfer: editingRule.is_transfer === null || editingRule.is_transfer === undefined ? undefined : String(editingRule.is_transfer),
      min_amount_raw: editingRule.min_amount_raw ?? undefined,
      channel_ids: editingRule.channel_ids,
      enabled: editingRule.enabled,
    };
  }

  function renderAddress(addressId?: string | null) {
    if (!addressId) return '-';
    const address = addressMap.get(addressId);
    if (!address) return addressId;
    return address.label ? `${address.label} / ${address.address}` : address.address;
  }

  return (
    <PageScaffold title="通知规则" description="用链、地址、资产、方向和金额条件定义通知路由，并快速绑定 Telegram 渠道。">
      {rulesQuery.isError ? (
        <Banner
          type="danger"
          title="通知规则加载失败"
          description={rulesQuery.error instanceof Error ? rulesQuery.error.message : '请求失败'}
        />
      ) : null}

      <DataSurface title="通知规则" actions={<Button type="primary" onClick={openCreateModal}>创建规则</Button>}>
        <DataTable<NotificationRule>
          tableId="notification-rules"
          actionColumnKeys={['operations']}
          loading={rulesQuery.isLoading}
          dataSource={rulesQuery.data ?? []}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          scroll={{ x: 1300 }}
          columns={[
            { title: '名称', dataIndex: 'name', width: 180, ellipsis: { showTitle: true } },
            { title: '启用', dataIndex: 'enabled', width: 80, render: value => <Tag color={value ? 'green' : 'grey'}>{value ? '启用' : '停用'}</Tag> },
            { title: '链', dataIndex: 'chain_id', width: 140, render: value => value ? chainMap.get(String(value)) ?? String(value) : '-' },
            { title: '地址', dataIndex: 'address_id', width: 260, ellipsis: { showTitle: true }, className: 'table-cell-mono', render: value => renderAddress(value ? String(value) : null) },
            { title: '资产', dataIndex: 'asset_id', width: 120, render: value => value ? assetMap.get(String(value)) ?? String(value) : '-' },
            { title: '事件类型', dataIndex: 'event_type', width: 150, render: value => value ? <Tag>{String(value)}</Tag> : '-' },
            { title: '方向', dataIndex: 'direction', width: 90, render: value => value ? String(value) : '-' },
            { title: '最小金额 raw', dataIndex: 'min_amount_raw', width: 150, ellipsis: { showTitle: true }, className: 'table-cell-mono', render: value => value ? String(value) : '-' },
            {
              title: '渠道',
              dataIndex: 'channel_ids',
              width: 220,
              render: value => {
                const channelIds = Array.isArray(value) ? value as string[] : [];
                if (channelIds.length === 0) return <Tag color="blue">默认站内</Tag>;
                return channelIds.map(id => <Tag key={id}>{channelMap.get(id) ?? id}</Tag>);
              },
            },
            {
              title: '操作',
              key: 'operations',
              width: 150,
              render: (_, rule) => (
                <Space>
                  <Button size="small" onClick={() => openEditModal(rule)}>编辑</Button>
                  <Button size="small" type="danger" loading={deleteMutation.isPending} onClick={() => deleteMutation.mutate(rule.id)}>删除</Button>
                </Space>
              ),
            },
          ]}
        />
      </DataSurface>

      <FormModal
        title={editingRule ? '编辑通知规则' : '创建通知规则'}
        visible={modalVisible}
        onCancel={() => {
          setModalVisible(false);
          setEditingRule(null);
        }}
        size="large"
      >
        <Form<RuleForm>
          initValues={initialValues()}
          onSubmit={handleSubmit}
          labelPosition="left"
          labelWidth={110}
          getFormApi={setRuleFormApi}
        >
          <Form.Input field="name" label="名称" rules={[{ required: true, message: '请输入规则名称' }]} />
          <Form.Select field="chain_id" label="链" showClear placeholder="不过滤链" filter>
            {(chainsQuery.data ?? []).map(chain => <Form.Select.Option key={chain.id} value={chain.id}>{chain.name}</Form.Select.Option>)}
          </Form.Select>
          <Form.Select field="address_id" label="地址" showClear placeholder="不过滤地址" filter>
            {(addressesQuery.data ?? []).map(address => (
              <Form.Select.Option key={address.id} value={address.id}>{address.label ? `${address.label} / ${address.address}` : address.address}</Form.Select.Option>
            ))}
          </Form.Select>
          <Form.Select field="asset_id" label="资产" showClear placeholder="不过滤资产" filter>
            {(assetsQuery.data ?? []).map(asset => <Form.Select.Option key={asset.id} value={asset.id}>{asset.symbol}</Form.Select.Option>)}
          </Form.Select>
          <Form.Select field="event_type" label="事件类型" showClear placeholder="不过滤类型" optionList={eventTypeOptions} />
          <Form.Select field="direction" label="方向" showClear placeholder="不过滤方向" optionList={directionOptions} />
          <Form.Select field="is_transfer" label="是否转账" showClear placeholder="不过滤">
            <Form.Select.Option value="true">是</Form.Select.Option>
            <Form.Select.Option value="false">否</Form.Select.Option>
          </Form.Select>
          <Form.Input field="min_amount_raw" label="最小金额 raw" placeholder="留空表示不过滤金额" />
          <div className="rule-channel-actions">
            <Form.Select field="channel_ids" label="渠道" multiple showClear placeholder="留空使用默认站内渠道" filter style={{ flex: 1 }}>
              {(channelsQuery.data ?? []).map(channel => <Form.Select.Option key={channel.id} value={channel.id}>{channel.name} / {channel.channel_type}</Form.Select.Option>)}
            </Form.Select>
            <Space>
              <Button htmlType="button" onClick={() => setQuickChannelVisible(true)}>新建渠道</Button>
              <Button htmlType="button" onClick={refreshChannels} loading={channelsQuery.isFetching}>刷新渠道</Button>
            </Space>
          </div>
          <Form.Switch field="enabled" label="启用" />
          <Space className="form-modal-actions">
            <Button htmlType="submit" type="primary" loading={saveMutation.isPending}>保存</Button>
            <Button onClick={() => setModalVisible(false)}>取消</Button>
          </Space>
        </Form>
      </FormModal>

      <FormModal title="快速新建TG通知渠道" visible={quickChannelVisible} onCancel={() => setQuickChannelVisible(false)} size="large">
        <Form<QuickChannelForm>
          onSubmit={values => quickChannelMutation.mutate(values)}
          labelPosition="left"
          labelWidth={120}
          getFormApi={setQuickChannelFormApi}
        >
          {({ formState }) => (
            <>
              <Form.Input field="name" label="渠道名称" rules={[{ required: true, message: '请输入渠道名称' }]} />
              <Form.Select field="telegram_bot_id" label="TG机器人" filter rules={[{ required: true, message: '请选择TG机器人' }]}>
                {(telegramBotsQuery.data ?? []).map(bot => <Form.Select.Option key={bot.id} value={bot.id}>{bot.name} / {bot.token_preview}</Form.Select.Option>)}
              </Form.Select>
              <TelegramBindingPanel telegramBotId={formState.values?.telegram_bot_id} onBound={handleQuickTelegramBound} />
              <Form.Input field="chat_alias" label="已绑定会话" disabled rules={[{ required: true, message: '请先绑定 Telegram 会话' }]} />
              <Form.Input field="chat_id" noLabel style={{ display: 'none' }} rules={[{ required: true, message: '请先绑定 Telegram 会话' }]} />
              <Form.TextArea field="message_template" label="消息模板" autosize />
              <Space className="form-modal-actions">
                <Button htmlType="submit" type="primary" loading={quickChannelMutation.isPending}>创建并选择</Button>
                <Button htmlType="button" onClick={() => setQuickChannelVisible(false)}>取消</Button>
              </Space>
            </>
          )}
        </Form>
      </FormModal>
    </PageScaffold>
  );
}
