import type { ReactNode } from 'react';
import { Space, Typography } from '@douyinfe/semi-ui';

const { Text, Title } = Typography;

type PageScaffoldProps = {
  title: string;
  description?: string;
  actions?: ReactNode;
  children: ReactNode;
};

export function PageScaffold({ title, description, actions, children }: PageScaffoldProps) {
  return (
    <section className="page-scaffold">
      <div className="page-heading">
        <div className="page-title-block">
          <Text className="page-eyebrow">Control Plane</Text>
          <Title heading={3} style={{ margin: 0 }}>{title}</Title>
          {description ? <Text type="tertiary" className="page-description">{description}</Text> : null}
        </div>
        {actions ? <div className="page-actions">{actions}</div> : null}
      </div>
      <Space vertical align="start" spacing={16} className="content-stack">
        {children}
      </Space>
    </section>
  );
}
