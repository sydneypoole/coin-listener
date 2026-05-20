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
          <span>Coin Listener</span>
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
          <div>
            <Title heading={4} style={{ margin: 0 }}>链上监控控制台</Title>
            <Text type="tertiary">多链资产、事件与通知运维工作台</Text>
          </div>
          <Space>
            <Text type="tertiary">{userLabel} / {tenantLabel}</Text>
            <ThemeToggle value={themeMode} onChange={onThemeModeChange} />
            <Button className="shell-logout-button" theme="borderless" type="tertiary" onClick={onLogout}>退出登录</Button>
          </Space>
        </Header>
        <Content className="app-content">{children}</Content>
      </Layout>
    </Layout>
  );
}
