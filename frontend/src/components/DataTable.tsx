import { useEffect, useMemo, useState } from 'react';
import { Table } from '@douyinfe/semi-ui';
import type { ColumnProps, TableProps } from '@douyinfe/semi-ui/lib/es/table/interface';

const STORAGE_PREFIX = 'coin-listener:data-table-widths:';

type RowData = Record<string, unknown>;

type WidthMap = Record<string, number>;

type DataTableProps<RecordType extends RowData> = Omit<TableProps<RecordType>, 'columns' | 'resizable'> & {
  tableId: string;
  columns: ColumnProps<RecordType>[];
  actionColumnKeys?: Array<string | number>;
};

function storageKey(tableId: string) {
  return `${STORAGE_PREFIX}${tableId}`;
}

function readWidths(tableId: string): WidthMap {
  try {
    const raw = localStorage.getItem(storageKey(tableId));
    if (!raw) return {};
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    const widths: WidthMap = {};
    for (const [key, value] of Object.entries(parsed)) {
      if (typeof value === 'number' && Number.isFinite(value)) {
        widths[key] = value;
      }
    }
    return widths;
  } catch {
    return {};
  }
}

function writeWidths(tableId: string, widths: WidthMap) {
  try {
    localStorage.setItem(storageKey(tableId), JSON.stringify(widths));
  } catch {
    // Ignore storage failures so table interaction still works in restricted browsers.
  }
}

function normalizeKey<RecordType extends RowData>(column: ColumnProps<RecordType>, indexPath: string) {
  if (column.key !== undefined && column.key !== null) return String(column.key);
  if (typeof column.dataIndex === 'string') return column.dataIndex;
  return `column-${indexPath}`;
}

function numericWidth(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0;
}

function prepareColumns<RecordType extends RowData>(
  columns: ColumnProps<RecordType>[],
  widths: WidthMap,
  actionColumnKeys: Array<string | number>,
  parentPath = '',
): ColumnProps<RecordType>[] {
  const actionKeySet = new Set(actionColumnKeys.map(String));

  return columns.map((column, index) => {
    const indexPath = parentPath ? `${parentPath}-${index}` : String(index);
    const key = normalizeKey(column, indexPath);
    const isActionColumn = actionKeySet.has(key);
    const width = widths[key] ?? column.width;
    const nextColumn: ColumnProps<RecordType> = {
      ...column,
      key,
      width,
      fixed: isActionColumn ? ('right' as const) : column.fixed,
      resize: isActionColumn ? false : column.resize,
    };

    if (Array.isArray(column.children) && column.children.length > 0) {
      nextColumn.children = prepareColumns(column.children, widths, actionColumnKeys, indexPath);
    }

    return nextColumn;
  });
}

function totalWidth<RecordType extends RowData>(columns: ColumnProps<RecordType>[]): number {
  return columns.reduce((sum, column) => {
    const childWidth = Array.isArray(column.children) ? totalWidth(column.children) : 0;
    return sum + numericWidth(column.width) + childWidth;
  }, 0);
}

export function DataTable<RecordType extends RowData>({
  tableId,
  columns,
  actionColumnKeys = ['operation', 'operations', 'actions'],
  className,
  scroll,
  ...props
}: DataTableProps<RecordType>) {
  const [widths, setWidths] = useState<WidthMap>(() => readWidths(tableId));

  useEffect(() => {
    setWidths(readWidths(tableId));
  }, [tableId]);

  const preparedColumns = useMemo(
    () => prepareColumns(columns, widths, actionColumnKeys),
    [actionColumnKeys, columns, widths],
  );

  const scrollX = scroll?.x ?? Math.max(totalWidth(preparedColumns), 720);

  return (
    <div className="data-table-surface">
      <Table<RecordType>
        {...props}
        className={['data-table', className].filter(Boolean).join(' ')}
        columns={preparedColumns}
        scroll={{ ...scroll, x: scrollX }}
        resizable={{
          onResizeStop: column => {
            const key = column.key === undefined || column.key === null ? undefined : String(column.key);
            const width = numericWidth(column.width);
            if (!key || !width) return column;
            const nextWidths = { ...widths, [key]: width };
            setWidths(nextWidths);
            writeWidths(tableId, nextWidths);
            return column;
          },
        }}
      />
    </div>
  );
}
