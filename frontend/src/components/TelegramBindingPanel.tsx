import { useEffect, useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Banner, Button, Card, Space, Tag, Toast, Typography } from '@douyinfe/semi-ui';
import {
  cancelTelegramBinding,
  createTelegramBinding,
  getTelegramBinding,
} from '../api/client';
import type { TelegramBindingRequest } from '../api/types';

const { Text } = Typography;

type TelegramBindingPanelProps = {
  telegramBotId: string | null | undefined;
  onBound?: (binding: TelegramBindingRequest) => void;
};

const statusColorMap: Record<string, 'green' | 'orange' | 'grey' | 'red' | 'blue'> = {
  pending: 'orange',
  bound: 'green',
  expired: 'grey',
  cancelled: 'grey',
};

function bindingChatAlias(binding: TelegramBindingRequest) {
  return binding.chat_title || binding.chat_username || binding.chat_id || '-';
}

function statusColor(status: string) {
  return statusColorMap[status] ?? 'blue';
}

export function TelegramBindingPanel({ telegramBotId, onBound }: TelegramBindingPanelProps) {
  const [bindingId, setBindingId] = useState<string | null>(null);
  const [notifiedBoundId, setNotifiedBoundId] = useState<string | null>(null);
  const queryClient = useQueryClient();
  const normalizedBotId = telegramBotId?.trim() ?? '';

  const createMutation = useMutation({
    mutationFn: () => createTelegramBinding({ telegram_bot_id: normalizedBotId }),
    onSuccess: binding => {
      setBindingId(binding.id);
      setNotifiedBoundId(null);
      queryClient.setQueryData(['telegram-binding', binding.id], binding);
      Toast.success('绑定码已生成');
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '生成绑定码失败'),
  });

  const bindingQuery = useQuery({
    queryKey: ['telegram-binding', bindingId],
    queryFn: () => getTelegramBinding(bindingId ?? ''),
    enabled: Boolean(bindingId),
    refetchInterval: query => (query.state.data?.status === 'pending' ? 5000 : false),
  });

  const binding = bindingQuery.data;
  const isPending = binding?.status === 'pending';
  const chatAlias = useMemo(() => (binding ? bindingChatAlias(binding) : '-'), [binding]);

  const cancelMutation = useMutation({
    mutationFn: () => cancelTelegramBinding(binding?.id ?? ''),
    onSuccess: updatedBinding => {
      queryClient.setQueryData(['telegram-binding', updatedBinding.id], updatedBinding);
      Toast.success('绑定请求已取消');
    },
    onError: error => Toast.error(error instanceof Error ? error.message : '取消绑定失败'),
  });

  useEffect(() => {
    if (!binding || binding.status !== 'bound' || notifiedBoundId === binding.id) return;
    setNotifiedBoundId(binding.id);
    onBound?.(binding);
  }, [binding, notifiedBoundId, onBound]);

  return (
    <Card className="telegram-binding-panel" title="TG会话绑定">
      <Space vertical align="start" spacing={16} style={{ width: '100%' }}>
        <Space>
          <Button
            type="primary"
            disabled={!normalizedBotId}
            loading={createMutation.isPending}
            onClick={() => createMutation.mutate()}
          >
            生成绑定码
          </Button>
          <Button
            disabled={!bindingId}
            loading={bindingQuery.isFetching}
            onClick={() => bindingId && queryClient.invalidateQueries({ queryKey: ['telegram-binding', bindingId] })}
          >
            刷新状态
          </Button>
        </Space>

        {!normalizedBotId ? <Text type="warning">请选择 TG 机器人后生成绑定码。</Text> : null}

        {bindingQuery.isError ? (
          <Banner
            fullMode={false}
            type="danger"
            bordered
            title="绑定状态加载失败"
            description={bindingQuery.error instanceof Error ? bindingQuery.error.message : '请求失败'}
          />
        ) : null}

        {binding ? (
          <Space vertical align="start" spacing={14} style={{ width: '100%' }}>
            <Space>
              <Text type="tertiary">状态</Text>
              <Tag color={statusColor(binding.status)}>{binding.status}</Tag>
              <Text type="tertiary">过期时间 {binding.expires_at}</Text>
            </Space>

            {binding.status === 'bound' ? (
              <Banner
                fullMode={false}
                type="success"
                bordered
                title="Telegram 会话已绑定"
                description={`已绑定 ${chatAlias} / ${binding.chat_id ?? '-'}。`}
              />
            ) : null}

            {binding.status === 'expired' ? (
              <Banner
                fullMode={false}
                type="warning"
                bordered
                title="绑定码已过期"
                description="请重新生成绑定码后再次发送 Telegram 验证消息。"
              />
            ) : null}

            <div className="telegram-binding-steps">
              <div className="telegram-binding-step">
                <Text strong>私聊通知</Text>
                {binding.deep_link_url ? (
                  <Text link={{ href: binding.deep_link_url, target: '_blank', rel: 'noopener noreferrer' }}>打开 Telegram 深链</Text>
                ) : null}
                <Text type="tertiary">向机器人发送命令：</Text>
                <code>/start {binding.bind_token}</code>
              </div>
              <div className="telegram-binding-step">
                <Text strong>群聊通知</Text>
                <Text type="tertiary">将机器人加入群聊，并在群内发送 short_code：</Text>
                <code>{binding.short_code}</code>
              </div>
            </div>

            {binding.confirmation_error ? (
              <Banner fullMode={false} type="warning" bordered title="确认异常" description={binding.confirmation_error} />
            ) : null}

            {isPending ? (
              <Button
                type="danger"
                theme="light"
                loading={cancelMutation.isPending}
                onClick={() => cancelMutation.mutate()}
              >
                取消绑定
              </Button>
            ) : null}
          </Space>
        ) : null}
      </Space>
    </Card>
  );
}
