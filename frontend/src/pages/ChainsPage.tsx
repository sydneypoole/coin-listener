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
        columns={[
          { title: '名称', dataIndex: 'name' },
          { title: 'Key', dataIndex: 'key' },
          { title: '类型', dataIndex: 'chain_type' },
          { title: '原生资产', dataIndex: 'native_asset_symbol' },
          { title: '确认数', dataIndex: 'default_confirmations' },
          { title: '状态', dataIndex: 'status', render: value => <Tag color="green">{String(value)}</Tag> },
        ]}
      />
    </Card>
  );
}
