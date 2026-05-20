import { useCallback, useEffect, useMemo, useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Button, Notification, Space, Tag, Typography } from '@douyinfe/semi-ui';
import { IconBell, IconPulse, IconServer, IconSetting, IconUser } from '@douyinfe/semi-icons';
import { request } from './api/client';
import { fetchHealth, type HealthResponse } from './api/health';
import { AppShell } from './components/AppShell';
import { DataSurface } from './components/DataSurface';
import { MetricCard, MetricGrid } from './components/MetricGrid';
import { PageScaffold } from './components/PageScaffold';
import type { LoginResponse } from './api/types';
import { clearSession, getSessionGeneration, loadStoredSession, saveSession, setUnauthorizedHandler } from './auth/session';
import { AddressesPage } from './pages/AddressesPage';
import { AssetsPage } from './pages/AssetsPage';
import { ChainsPage } from './pages/ChainsPage';
import { EventsPage } from './pages/EventsPage';
import { InAppNotificationsPage } from './pages/InAppNotificationsPage';
import { LoginPage } from './pages/LoginPage';
import { NotificationOperationsPage } from './pages/NotificationOperationsPage';
import { NotificationRulesPage } from './pages/NotificationRulesPage';
import { ProvidersPage } from './pages/ProvidersPage';
import { SystemStatusPage } from './pages/SystemStatusPage';
import { connectRealtimeNotifications } from './realtime/notifications';
import { applyThemeMode, loadThemeMode, saveThemeMode, subscribeSystemTheme, type ThemeMode } from './themeMode';

const { Text } = Typography;

type PageKey =
  | 'dashboard'
  | 'system-status'
  | 'chains'
  | 'assets'
  | 'providers'
  | 'addresses'
  | 'events'
  | 'notification-rules'
  | 'notification-operations'
  | 'in-app-notifications';

type HealthQuery = ReturnType<typeof useQuery<HealthResponse>>;

const dashboardSteps = [
  { title: '监听地址', label: 'Watch', description: '按链与资产集合维护扫描目标' },
  { title: 'Provider Mesh', label: 'RPC', description: '多 Provider 优先级、限流与熔断' },
  { title: '事件中心', label: 'Event', description: '归集转账、余额变更与合约交互' },
  { title: '通知出站', label: 'Notify', description: 'Outbox 重试、投递与站内通知闭环' },
];

export function App() {
  const queryClient = useQueryClient();
  const [session, setSession] = useState<LoginResponse | null>(() => loadStoredSession());
  const [page, setPage] = useState<PageKey>('dashboard');
  const [realtimeUnreadCount, setRealtimeUnreadCount] = useState(0);
  const [themeMode, setThemeMode] = useState<ThemeMode>(() => loadThemeMode());

  const resetAuthenticatedState = useCallback(() => {
    queryClient.clear();
    setPage('dashboard');
    setSession(null);
    setRealtimeUnreadCount(0);
  }, [queryClient]);

  const handleRealtimeUnauthorized = useCallback(() => {
    clearSession();
    resetAuthenticatedState();
  }, [resetAuthenticatedState]);

  useEffect(() => {
    setUnauthorizedHandler(resetAuthenticatedState);
    return () => setUnauthorizedHandler(null);
  }, [resetAuthenticatedState]);

  useEffect(() => {
    applyThemeMode(themeMode);
    if (themeMode !== 'system') return undefined;
    return subscribeSystemTheme(() => applyThemeMode('system'));
  }, [themeMode]);

  useEffect(() => {
    if (!session) return undefined;

    const generation = getSessionGeneration();
    return connectRealtimeNotifications(
      session,
      {
        onNotification: notification => {
          setRealtimeUnreadCount(count => count + 1);
          Notification.info({
            title: notification.title,
            content: notification.body,
          });
          queryClient.invalidateQueries({ queryKey: ['in-app-notifications'] });
          queryClient.invalidateQueries({ queryKey: ['events'] });
          queryClient.invalidateQueries({ queryKey: ['system-status'] });
        },
        onUnauthorized: handleRealtimeUnauthorized,
      },
      {
        generation,
        getGeneration: getSessionGeneration,
        verifyAuth: () => request('/api/system/status'),
      },
    );
  }, [handleRealtimeUnauthorized, queryClient, session]);

  const healthQuery = useQuery({
    queryKey: ['health'],
    queryFn: fetchHealth,
    retry: 1,
  });

  function handleLogin(nextSession: LoginResponse) {
    saveSession(nextSession);
    setSession(nextSession);
  }

  function handleLogout() {
    clearSession();
    resetAuthenticatedState();
  }

  function handleThemeModeChange(nextMode: ThemeMode) {
    setThemeMode(nextMode);
    saveThemeMode(nextMode);
  }

  const navItems = useMemo(() => [
    { itemKey: 'dashboard', text: '仪表盘', icon: <IconPulse /> },
    { itemKey: 'system-status', text: '系统状态', icon: <IconPulse /> },
    { itemKey: 'chains', text: '链配置', icon: <IconServer /> },
    { itemKey: 'assets', text: '资产配置', icon: <IconSetting /> },
    { itemKey: 'providers', text: 'Provider', icon: <IconServer /> },
    { itemKey: 'addresses', text: '监听地址', icon: <IconUser /> },
    { itemKey: 'events', text: '事件中心', icon: <IconBell /> },
    { itemKey: 'notification-rules', text: '通知规则', icon: <IconBell /> },
    { itemKey: 'notification-operations', text: '通知运维', icon: <IconBell /> },
    {
      itemKey: 'in-app-notifications',
      text: realtimeUnreadCount > 0 ? `站内通知 (${realtimeUnreadCount})` : '站内通知',
      icon: <IconBell />,
    },
  ], [realtimeUnreadCount]);

  if (!session) {
    return <LoginPage onLogin={handleLogin} />;
  }

  return (
    <AppShell<PageKey>
      page={page}
      navItems={navItems}
      userLabel={session.user.display_name}
      tenantLabel={session.tenant.name}
      themeMode={themeMode}
      onThemeModeChange={handleThemeModeChange}
      onSelectPage={setPage}
      onLogout={handleLogout}
    >
      {renderPage(page, healthQuery, realtimeUnreadCount, setRealtimeUnreadCount)}
    </AppShell>
  );
}

function renderPage(
  page: PageKey,
  healthQuery: HealthQuery,
  realtimeUnreadCount: number,
  setRealtimeUnreadCount: (count: number) => void,
) {
  if (page === 'system-status') return <SystemStatusPage />;
  if (page === 'chains') return <ChainsPage />;
  if (page === 'assets') return <AssetsPage />;
  if (page === 'providers') return <ProvidersPage />;
  if (page === 'addresses') return <AddressesPage />;
  if (page === 'events') return <EventsPage />;
  if (page === 'notification-rules') return <NotificationRulesPage />;
  if (page === 'notification-operations') return <NotificationOperationsPage />;
  if (page === 'in-app-notifications') {
    return <InAppNotificationsPage onUnreadSettled={setRealtimeUnreadCount} />;
  }

  return <DashboardOverview healthQuery={healthQuery} realtimeUnreadCount={realtimeUnreadCount} />;
}

function DashboardOverview({ healthQuery, realtimeUnreadCount }: { healthQuery: HealthQuery; realtimeUnreadCount: number }) {
  const healthLabel = healthStatusText(healthQuery);

  return (
    <PageScaffold
      title="链上运维总览"
      description="从 Provider、扫描队列、事件入库到通知出站的控制面入口。"
      actions={(
        <Button loading={healthQuery.isFetching} onClick={() => healthQuery.refetch()}>
          刷新 API 健康
        </Button>
      )}
    >
      <MetricGrid>
        <MetricCard title="API Gateway" value={healthLabel} hint={healthQuery.data?.service ?? 'health endpoint'} tone={healthTone(healthQuery)} />
        <MetricCard title="控制面" value="多链" hint="链、资产、Provider 统一配置" />
        <MetricCard title="实时通知" value={realtimeUnreadCount} hint="本会话新增站内通知" tone={realtimeUnreadCount > 0 ? 'warning' : 'neutral'} />
        <MetricCard title="刷新策略" value="手动" hint="健康检查保持原查询与重试行为" />
      </MetricGrid>

      <DataSurface title="控制面健康" actions={<Tag color={healthTagColor(healthQuery)}>{healthLabel}</Tag>}>
        <div className="dashboard-health-panel">
          <div>
            <Text type="tertiary">API Health</Text>
            <div className="dashboard-health-title">{healthQuery.data?.service ?? 'Coin Listener API'}</div>
            <Text type={healthQuery.isError ? 'danger' : 'tertiary'}>
              {healthDescription(healthQuery)}
            </Text>
          </div>
          <Button loading={healthQuery.isFetching} onClick={() => healthQuery.refetch()}>
            重新检查
          </Button>
        </div>
      </DataSurface>

      <DataSurface title="链上业务链路" actions={<Text type="tertiary">Watch → RPC → Event → Notify</Text>}>
        <div className="dashboard-chain-map">
          {dashboardSteps.map((step, index) => (
            <div className="dashboard-chain-step" key={step.title}>
              <div className="dashboard-step-index">0{index + 1}</div>
              <Tag color="blue">{step.label}</Tag>
              <div className="dashboard-step-title">{step.title}</div>
              <Text type="tertiary">{step.description}</Text>
            </div>
          ))}
        </div>
      </DataSurface>

      <DataSurface title="运维入口">
        <Space wrap>
          <Tag color="cyan">系统状态：队列、Provider、服务心跳</Tag>
          <Tag color="blue">事件中心：链上活动检索</Tag>
          <Tag color="orange">通知运维：Outbox 重试与详情</Tag>
          <Tag color="green">站内通知：实时消费反馈</Tag>
        </Space>
      </DataSurface>
    </PageScaffold>
  );
}

function healthStatusText(healthQuery: HealthQuery) {
  if (healthQuery.isLoading) return 'checking';
  if (healthQuery.isError) return 'degraded';
  return healthQuery.data?.status ?? 'unknown';
}

function healthTone(healthQuery: HealthQuery): 'neutral' | 'success' | 'warning' | 'danger' {
  if (healthQuery.isError) return 'danger';
  if (healthQuery.isLoading || healthQuery.isFetching) return 'warning';
  return healthQuery.data ? 'success' : 'neutral';
}

function healthTagColor(healthQuery: HealthQuery) {
  if (healthQuery.isError) return 'red';
  if (healthQuery.isLoading || healthQuery.isFetching) return 'blue';
  return healthQuery.data ? 'green' : 'grey';
}

function healthDescription(healthQuery: HealthQuery) {
  if (healthQuery.isLoading) return '正在检查 API 可用性...';
  if (healthQuery.isError) {
    return healthQuery.error instanceof Error ? healthQuery.error.message : '请求失败';
  }
  if (healthQuery.data) return `服务状态 ${healthQuery.data.status}`;
  return '等待健康检查结果';
}
