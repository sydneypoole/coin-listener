import { useQuery } from '@tanstack/react-query';
import { Tag } from '@douyinfe/semi-ui';
import { listAssets, listChains } from '../api/client';
import type { Asset } from '../api/types';
import { DataSurface } from '../components/DataSurface';
import { DataTable } from '../components/DataTable';
import { PageScaffold } from '../components/PageScaffold';

export function AssetsPage() {
  const assetsQuery = useQuery({ queryKey: ['assets'], queryFn: listAssets });
  const chainsQuery = useQuery({ queryKey: ['chains'], queryFn: listChains });
  const chainMap = new Map((chainsQuery.data ?? []).map(chain => [chain.id, chain.name]));

  return (
    <PageScaffold title="资产配置">
      <DataSurface title="资产列表">
        <DataTable<Asset>
          tableId="assets"
          loading={assetsQuery.isLoading}
          dataSource={assetsQuery.data ?? []}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          scroll={{ x: 980 }}
          columns={[
            { title: '链', dataIndex: 'chain_id', width: 160, render: value => chainMap.get(String(value)) ?? String(value) },
            { title: '符号', dataIndex: 'symbol', width: 120, ellipsis: { showTitle: true } },
            { title: '名称', dataIndex: 'name', width: 180, ellipsis: { showTitle: true } },
            { title: '类型', dataIndex: 'asset_type', width: 120 },
            { title: '合约地址', dataIndex: 'contract_address', width: 320, ellipsis: { showTitle: true }, className: 'table-cell-mono', render: value => value ? String(value) : '-' },
            { title: '精度', dataIndex: 'decimals', width: 90 },
            { title: '内置', dataIndex: 'is_builtin', width: 90, render: value => <Tag color={value ? 'blue' : 'grey'}>{value ? '是' : '否'}</Tag> },
          ]}
        />
      </DataSurface>
    </PageScaffold>
  );
}
