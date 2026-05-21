import type { WatchedAddressImportRowRequest } from './api/types';

export type ParsedAddressImportRow = WatchedAddressImportRowRequest & {
  error?: string;
};

export type ParsedAddressImport = {
  rows: ParsedAddressImportRow[];
  warnings: string[];
};

const supportedCsvFields = new Set([
  'address',
  'label',
  'priority',
  'scan_interval_seconds',
  'transfer_filter_enabled',
  'balance_change_filter_enabled',
  'status',
]);

export function parseAddressImportInput(input: string): ParsedAddressImport {
  const lines = input
    .split(/\r?\n/)
    .map((line, index) => ({ text: line.trim(), lineNumber: index + 1 }))
    .filter(line => line.text.length > 0);

  if (lines.length === 0) return { rows: [], warnings: [] };

  const headers = splitCsvLine(lines[0].text).map(header => header.trim().toLowerCase());
  if (headers.includes('address')) return parseCsv(lines, headers);

  return markDuplicateRows(lines.map(line => ({
    row_number: line.lineNumber,
    raw_text: line.text,
    address: line.text,
  })));
}

function parseCsv(lines: { text: string; lineNumber: number }[], headers: string[]): ParsedAddressImport {
  const warnings = headers
    .filter(header => header.length > 0 && !supportedCsvFields.has(header))
    .map(header => `unknown CSV field: ${header}`);

  const rows = lines.slice(1).map(line => {
    const values = splitCsvLine(line.text);
    const record = Object.fromEntries(headers.map((header, index) => [header, values[index]?.trim() ?? '']));

    return {
      row_number: line.lineNumber,
      raw_text: line.text,
      address: record.address ?? '',
      label: record.label || null,
      priority: record.priority || null,
      scan_interval_seconds: parseOptionalNumber(record.scan_interval_seconds),
      transfer_filter_enabled: parseOptionalBoolean(record.transfer_filter_enabled),
      balance_change_filter_enabled: parseOptionalBoolean(record.balance_change_filter_enabled),
      status: record.status || null,
    } satisfies ParsedAddressImportRow;
  });

  const parsed = markDuplicateRows(rows);
  return { rows: parsed.rows, warnings: [...warnings, ...parsed.warnings] };
}

function markDuplicateRows(rows: ParsedAddressImportRow[]): ParsedAddressImport {
  const seen = new Set<string>();
  return {
    rows: rows.map(row => {
      const key = row.address.trim().toLowerCase();
      if (!key) return { ...row, error: '地址不能为空' };
      if (seen.has(key)) return { ...row, error: '重复地址' };
      seen.add(key);
      return row;
    }),
    warnings: [],
  };
}

function splitCsvLine(line: string): string[] {
  return line.split(',');
}

function parseOptionalNumber(value: string | undefined): number | null {
  if (!value) return null;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : null;
}

function parseOptionalBoolean(value: string | undefined): boolean | null {
  if (!value) return null;
  const normalized = value.trim().toLowerCase();
  if (normalized === 'true') return true;
  if (normalized === 'false') return false;
  return null;
}
