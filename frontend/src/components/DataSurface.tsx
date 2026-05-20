import type { ReactNode } from 'react';
import { Card } from '@douyinfe/semi-ui';

type DataSurfaceProps = {
  title: string;
  actions?: ReactNode;
  children: ReactNode;
};

export function DataSurface({ title, actions, children }: DataSurfaceProps) {
  return (
    <Card title={title} headerExtraContent={actions} className="data-surface">
      <div className="data-surface-body">{children}</div>
    </Card>
  );
}
