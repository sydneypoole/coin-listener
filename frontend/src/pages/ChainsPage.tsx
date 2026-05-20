import { useQuery } from '@tanstack/react-query';
import { Card, Table, Tag } from '@douyinfe/semi-ui';
import { listChains } from '../api/client';
import type { Chain } from '../api/types';

export function ChainsPage() {
  const query = useQuery({ queryKey: ['chains'], queryFn: listChains });

  return (
    <Card title="链配置">
      <Table<Chain>
        loading={query.isLoading}
        dataSource={query.data ?? []}
        rowKey="id"
        pagination={false}
        scroll={{ x: 820 }}
        columns={[
          { title: '名称', dataIndex: 'name', width: 180, ellipsis: { showTitle: true } },
          { title: 'Key', dataIndex: 'key', width: 160, ellipsis: { showTitle: true }, className: 'table-cell-mono' },
          { title: '类型', dataIndex: 'chain_type', width: 120 },
          { title: '原生资产', dataIndex: 'native_asset_symbol', width: 120 },
          { title: '确认数', dataIndex: 'default_confirmations', width: 100 },
          { title: '状态', dataIndex: 'status', width: 120, render: value => <Tag color="green">{String(value)}</Tag> },
        ]}
      />
    </Card>
  );
}
