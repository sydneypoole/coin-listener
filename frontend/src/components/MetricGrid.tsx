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
      <Text type="tertiary" className="metric-title">{title}</Text>
      <Title heading={3} className="metric-value" style={{ margin: '8px 0 4px' }}>{value}</Title>
      <Text type="tertiary" className="metric-hint">{hint}</Text>
    </Card>
  );
}
