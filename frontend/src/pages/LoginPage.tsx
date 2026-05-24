import { useState } from 'react';
import { Button, Card, Form, Space, Tag, Toast, Typography } from '@douyinfe/semi-ui';
import { login } from '../api/client';
import type { LoginResponse } from '../api/types';

const { Text, Title } = Typography;

type LoginPageProps = {
  onLogin: (session: LoginResponse) => void;
};

const loginHighlights = ['Tenant scoped', 'Provider aware', 'Auditable outbox'];
const loginProofs = ['多链监听目标集中管理', 'Provider 与事件账本统一审计', '通知出站和站内回执闭环追踪'];

export function LoginPage({ onLogin }: LoginPageProps) {
  const [loading, setLoading] = useState(false);

  async function handleSubmit(values: Record<string, unknown>) {
    setLoading(true);
    try {
      const session = await login(String(values.email), String(values.password));
      onLogin(session);
      Toast.success('登录成功');
    } catch (error) {
      Toast.error(error instanceof Error ? error.message : '登录失败');
    } finally {
      setLoading(false);
    }
  }

  return (
    <div className="login-page">
      <div className="login-shell">
        <section className="login-hero-panel">
          <div className="login-brand-row">
            <span className="brand-mark">CL</span>
            <Text strong>Coin Listener</Text>
          </div>
          <Title heading={1} className="login-hero-title">Commercial control for on-chain operations</Title>
          <Text className="login-hero-copy">
            面向区块链运维的监听控制台：地址、Provider、事件账本与通知出站保持在同一个可信工作面。
          </Text>
          <Space wrap className="login-highlight-row">
            {loginHighlights.map(item => <Tag key={item}>{item}</Tag>)}
          </Space>
          <ul className="login-proof-list">
            {loginProofs.map(item => <li key={item}>{item}</li>)}
          </ul>
        </section>

        <Card className="login-card">
          <Text type="tertiary">Secure console entry</Text>
          <Title heading={3} style={{ marginTop: 6 }}>登录工作台</Title>
          <Text type="tertiary">使用管理员账号进入多链运维控制面</Text>
          <Form onSubmit={handleSubmit} className="login-form" labelPosition="top">
            <Form.Input field="email" label="邮箱" initValue="admin@example.com" rules={[{ required: true, message: '请输入邮箱' }]} />
            <Form.Input field="password" label="密码" mode="password" rules={[{ required: true, message: '请输入密码' }]} />
            <Button htmlType="submit" type="primary" loading={loading} block>
              进入控制台
            </Button>
          </Form>
        </Card>
      </div>
    </div>
  );
}
