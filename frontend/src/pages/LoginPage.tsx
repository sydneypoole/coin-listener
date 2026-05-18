import { useState } from 'react';
import { Button, Card, Form, Toast, Typography } from '@douyinfe/semi-ui';
import { login } from '../api/client';
import type { LoginResponse } from '../api/types';

const { Title, Text } = Typography;

type LoginPageProps = {
  onLogin: (session: LoginResponse) => void;
};

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
      <Card className="login-card">
        <Title heading={3}>Coin Listener</Title>
        <Text type="tertiary">默认账号：admin@example.com / admin</Text>
        <Form onSubmit={handleSubmit} className="login-form">
          <Form.Input field="email" label="邮箱" initValue="admin@example.com" rules={[{ required: true, message: '请输入邮箱' }]} />
          <Form.Input field="password" label="密码" mode="password" initValue="admin" rules={[{ required: true, message: '请输入密码' }]} />
          <Button htmlType="submit" type="primary" loading={loading} block>
            登录
          </Button>
        </Form>
      </Card>
    </div>
  );
}
