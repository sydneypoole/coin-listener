import { useQuery } from '@tanstack/react-query';
import { Tag } from '@douyinfe/semi-ui';
import { listChains } from '../api/client';
import type { Chain } from '../api/types';
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { PageScaffold } from '../components/PageScaffold';

export function ChainsPage() {
  const query = useQuery({ queryKey: ['chains'], queryFn: listChains });

  return (
    <PageScaffold title="链配置" description="查看当前可监听链、链类型、原生资产与确认数策略。">
      <DataSurface title="链列表">
        <DataTable<Chain>
          tableId="chains"
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
      </DataSurface>
    </PageScaffold>
  );
}
