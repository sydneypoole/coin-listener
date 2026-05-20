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
        <div>
          <Title heading={3} style={{ margin: 0 }}>{title}</Title>
          {description ? <Text type="tertiary">{description}</Text> : null}
        </div>
        {actions ? <div className="page-actions">{actions}</div> : null}
      </div>
      <Space vertical align="start" spacing={16} className="content-stack">
        {children}
      </Space>
    </section>
  );
}
