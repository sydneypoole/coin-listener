import type { ReactNode } from 'react';
import { Card } from '@douyinfe/semi-ui';

type FilterPanelProps = {
  title: string;
  children: ReactNode;
};

export function FilterPanel({ title, children }: FilterPanelProps) {
  return (
    <Card title={title} className="filter-panel">
      <div className="filter-panel-body">{children}</div>
    </Card>
  );
}
