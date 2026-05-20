import type { ReactNode } from 'react';
import { Card, Typography } from '@douyinfe/semi-ui';

const { Text, Title } = Typography;

type MetricGridProps = {
  children: ReactNode;
};

type MetricCardProps = {
  title: string;
  value: string | number;
  hint: string;
  tone?: 'neutral' | 'success' | 'warning' | 'danger';
};

export function MetricGrid({ children }: MetricGridProps) {
  return <div className="metric-grid">{children}</div>;
}

export function MetricCard({ title, value, hint, tone = 'neutral' }: MetricCardProps) {
  return (
    <Card className={`metric-card metric-card-${tone}`}>
      <Text type="tertiary">{title}</Text>
      <Title heading={3} style={{ margin: '8px 0 4px' }}>{value}</Title>
      <Text type="tertiary">{hint}</Text>
    </Card>
  );
}
