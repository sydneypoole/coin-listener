import { useEffect, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Banner, Button, Card, Layout, Nav, Space, Tag, Typography } from '@douyinfe/semi-ui';
import { IconBell, IconPulse, IconServer, IconSetting, IconUser } from '@douyinfe/semi-icons';
import { fetchHealth, type HealthResponse } from './api/health';
import type { LoginResponse } from './api/types';
import { clearSession, loadStoredSession, saveSession, setUnauthorizedHandler } from './auth/session';
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

const { Header, Sider, Content } = Layout;
const { Title, Text } = Typography;

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

export function App() {
  const [session, setSession] = useState<LoginResponse | null>(() => loadStoredSession());
  const [page, setPage] = useState<PageKey>('dashboard');

  useEffect(() => {
    setUnauthorizedHandler(() => setSession(null));
    return () => setUnauthorizedHandler(null);
  }, []);

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
    setSession(null);
  }

  if (!session) {
    return <LoginPage onLogin={handleLogin} />;
  }

  return (
    <Layout className="app-shell">
      <Sider className="app-sider">
        <div className="brand">Coin Listener</div>
        <Nav
          selectedKeys={[page]}
          onSelect={({ itemKey }) => setPage(itemKey as PageKey)}
          items={[
            { itemKey: 'dashboard', text: '仪表盘', icon: <IconPulse /> },
            { itemKey: 'system-status', text: '系统状态', icon: <IconPulse /> },
            { itemKey: 'chains', text: '链配置', icon: <IconServer /> },
            { itemKey: 'assets', text: '资产配置', icon: <IconSetting /> },
            { itemKey: 'providers', text: 'Provider', icon: <IconServer /> },
            { itemKey: 'addresses', text: '监听地址', icon: <IconUser /> },
            { itemKey: 'events', text: '事件中心', icon: <IconBell /> },
            { itemKey: 'notification-rules', text: '通知规则', icon: <IconBell /> },
            { itemKey: 'notification-operations', text: '通知运维', icon: <IconBell /> },
            { itemKey: 'in-app-notifications', text: '站内通知', icon: <IconBell /> },
          ]}
        />
      </Sider>
      <Layout>
        <Header className="app-header">
          <Title heading={4}>多链地址监听平台</Title>
          <Space>
            <Text type="tertiary">{session.user.display_name} / {session.tenant.name}</Text>
            <Button onClick={handleLogout}>退出登录</Button>
          </Space>
        </Header>
        <Content className="app-content">{renderPage(page, healthQuery)}</Content>
      </Layout>
    </Layout>
  );
}

function renderPage(page: PageKey, healthQuery: ReturnType<typeof useQuery<HealthResponse>>) {
  if (page === 'system-status') return <SystemStatusPage />;
  if (page === 'chains') return <ChainsPage />;
  if (page === 'assets') return <AssetsPage />;
  if (page === 'providers') return <ProvidersPage />;
  if (page === 'addresses') return <AddressesPage />;
  if (page === 'events') return <EventsPage />;
  if (page === 'notification-rules') return <NotificationRulesPage />;
  if (page === 'notification-operations') return <NotificationOperationsPage />;
  if (page === 'in-app-notifications') return <InAppNotificationsPage />;

  return (
    <Space vertical align="start" spacing={24} className="content-stack">
      <Banner
        type="info"
        title="Milestone 3"
        description="当前版本提供登录、链配置、资产配置、Provider 配置、监听地址管理、事件中心、通知规则、通知运维、站内通知和系统状态。"
      />
      <Card title="API 健康状态" className="status-card">
        {healthQuery.isLoading ? <Text>正在检查 API...</Text> : null}
        {healthQuery.isError ? (
          <Space vertical align="start">
            <Tag color="red">API 不可用</Tag>
            <Text type="danger">{healthQuery.error instanceof Error ? healthQuery.error.message : '请求失败'}</Text>
            <Button onClick={() => healthQuery.refetch()}>重新检查</Button>
          </Space>
        ) : null}
        {healthQuery.data ? (
          <Space vertical align="start">
            <Tag color="green">{healthQuery.data.status}</Tag>
            <Text>服务：{healthQuery.data.service}</Text>
            <Button onClick={() => healthQuery.refetch()}>刷新</Button>
          </Space>
        ) : null}
      </Card>
    </Space>
  );
}
