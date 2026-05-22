ALTER TABLE watched_address_import_tasks
    ADD COLUMN IF NOT EXISTS chain_configs JSONB NOT NULL DEFAULT '[]'::jsonb;

UPDATE watched_address_import_tasks
SET chain_configs = jsonb_build_array(
    jsonb_build_object(
        'chain_id', chain_id,
        'asset_ids', to_jsonb(asset_ids)
    )
)
WHERE chain_configs = '[]'::jsonb;

CREATE TABLE IF NOT EXISTS watched_address_import_attempts (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    import_task_id UUID NOT NULL REFERENCES watched_address_import_tasks(id) ON DELETE CASCADE,
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    row_number INTEGER NOT NULL,
    chain_id UUID NOT NULL REFERENCES chains(id),
    asset_ids UUID[] NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    watched_address_id UUID REFERENCES watched_addresses(id) ON DELETE SET NULL,
    error_code TEXT,
    error_message TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT watched_address_import_attempts_status_check CHECK (status IN ('pending', 'success', 'failed', 'skipped')),
    CONSTRAINT watched_address_import_attempts_unique_row_chain UNIQUE (import_task_id, row_number, chain_id),
    CONSTRAINT watched_address_import_attempts_source_row_fk
        FOREIGN KEY (import_task_id, row_number)
        REFERENCES watched_address_import_rows(import_task_id, row_number)
        ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_watched_address_import_attempts_task_status
    ON watched_address_import_attempts(import_task_id, status, row_number, chain_id);

CREATE INDEX IF NOT EXISTS idx_watched_address_import_attempts_tenant_task
    ON watched_address_import_attempts(tenant_id, import_task_id);

INSERT INTO watched_address_import_attempts (
    import_task_id, tenant_id, row_number, chain_id, asset_ids, status,
    watched_address_id, error_code, error_message, created_at, updated_at
)
SELECT import_row.import_task_id,
       import_row.tenant_id,
       import_row.row_number,
       task.chain_id,
       task.asset_ids,
       import_row.status,
       import_row.watched_address_id,
       import_row.error_code,
       import_row.error_message,
       import_row.created_at,
       import_row.updated_at
FROM watched_address_import_rows import_row
JOIN watched_address_import_tasks task
  ON task.id = import_row.import_task_id
 AND task.tenant_id = import_row.tenant_id
ON CONFLICT (import_task_id, row_number, chain_id) DO NOTHING;
