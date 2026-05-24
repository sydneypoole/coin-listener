import type { ReactNode } from 'react';
import { Button, Layout, Nav, Space, Typography } from '@douyinfe/semi-ui';
import type { NavItemProps } from '@douyinfe/semi-ui/lib/es/navigation';
import { ThemeToggle } from './ThemeToggle';
import type { ThemeMode } from '../themeMode';

const { Header, Sider, Content } = Layout;
const { Text, Title } = Typography;

type AppShellProps<PageKey extends string> = {
  page: PageKey;
  navItems: NavItemProps[];
  userLabel: string;
  tenantLabel: string;
  themeMode: ThemeMode;
  onThemeModeChange: (mode: ThemeMode) => void;
  onSelectPage: (page: PageKey) => void;
  onLogout: () => void;
  children: ReactNode;
};

export function AppShell<PageKey extends string>({
  page,
  navItems,
  userLabel,
  tenantLabel,
  themeMode,
  onThemeModeChange,
  onSelectPage,
  onLogout,
  children,
}: AppShellProps<PageKey>) {
  return (
    <Layout className="app-shell">
      <Sider className="app-sider">
        <div className="brand">
          <span className="brand-mark">CL</span>
          <span className="shell-identity">
            <span>Coin Listener</span>
            <Text className="brand-subtitle">Chain Ops Console</Text>
          </span>
        </div>
        <div className="shell-meta">
          <Text className="shell-meta-label">workspace</Text>
          <Text className="shell-meta-value">{tenantLabel}</Text>
        </div>
        <Nav
          className="chain-nav"
          selectedKeys={[page]}
          onSelect={({ itemKey }) => onSelectPage(itemKey as PageKey)}
          items={navItems}
        />
      </Sider>
      <Layout className="min-w-0">
        <Header className="app-header">
          <div className="shell-header-copy">
            <Title heading={4} style={{ margin: 0 }}>链上监控控制台</Title>
            <Text type="tertiary">Watch · RPC Mesh · Event Ledger · Notify Outbox</Text>
          </div>
          <Space className="shell-actions" wrap>
            <div className="shell-session-pill">
              <Text className="shell-meta-label">operator</Text>
              <Text className="shell-session-value">{userLabel}</Text>
            </div>
            <ThemeToggle value={themeMode} onChange={onThemeModeChange} />
            <Button className="shell-logout-button" theme="borderless" type="tertiary" onClick={onLogout}>退出登录</Button>
          </Space>
        </Header>
        <Content className="app-content">{children}</Content>
      </Layout>
    </Layout>
  );
}
