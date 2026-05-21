CREATE TABLE IF NOT EXISTS telegram_bots (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    bot_token TEXT NOT NULL,
    token_preview TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    verification_status TEXT NOT NULL DEFAULT 'unverified',
    last_verified_at TIMESTAMPTZ,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT telegram_bots_status_check CHECK (status IN ('active', 'inactive')),
    CONSTRAINT telegram_bots_verification_status_check CHECK (verification_status IN ('unverified', 'verified', 'failed'))
);

CREATE INDEX IF NOT EXISTS idx_telegram_bots_tenant_status
    ON telegram_bots(tenant_id, status, created_at DESC);

CREATE TABLE IF NOT EXISTS watched_address_import_tasks (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'pending',
    chain_id UUID NOT NULL REFERENCES chains(id),
    asset_ids UUID[] NOT NULL,
    priority TEXT NOT NULL,
    scan_interval_seconds INTEGER NOT NULL,
    transfer_filter_enabled BOOLEAN NOT NULL,
    balance_change_filter_enabled BOOLEAN NOT NULL,
    address_status TEXT NOT NULL,
    total_rows INTEGER NOT NULL DEFAULT 0,
    processed_rows INTEGER NOT NULL DEFAULT 0,
    success_rows INTEGER NOT NULL DEFAULT 0,
    failed_rows INTEGER NOT NULL DEFAULT 0,
    locked_at TIMESTAMPTZ,
    locked_by TEXT,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT watched_address_import_tasks_status_check CHECK (status IN ('pending', 'running', 'completed', 'failed', 'cancelled'))
);

CREATE INDEX IF NOT EXISTS idx_watched_address_import_tasks_tenant_created
    ON watched_address_import_tasks(tenant_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_watched_address_import_tasks_claim
    ON watched_address_import_tasks(status, created_at)
    WHERE status IN ('pending', 'running');

CREATE TABLE IF NOT EXISTS watched_address_import_rows (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    import_task_id UUID NOT NULL REFERENCES watched_address_import_tasks(id) ON DELETE CASCADE,
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    row_number INTEGER NOT NULL,
    raw_text TEXT NOT NULL,
    address TEXT NOT NULL,
    label TEXT,
    priority TEXT,
    scan_interval_seconds INTEGER,
    transfer_filter_enabled BOOLEAN,
    balance_change_filter_enabled BOOLEAN,
    address_status TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    watched_address_id UUID REFERENCES watched_addresses(id) ON DELETE SET NULL,
    error_code TEXT,
    error_message TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT watched_address_import_rows_status_check CHECK (status IN ('pending', 'success', 'failed', 'skipped')),
    CONSTRAINT watched_address_import_rows_unique_row_number UNIQUE (import_task_id, row_number)
);

CREATE INDEX IF NOT EXISTS idx_watched_address_import_rows_task_status
    ON watched_address_import_rows(import_task_id, status, row_number);
