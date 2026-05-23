CREATE TABLE IF NOT EXISTS scan_runs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    task_id UUID NOT NULL,
    address_id UUID NOT NULL REFERENCES watched_addresses(id) ON DELETE CASCADE,
    chain_id UUID NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
    chain_type TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('running', 'success', 'failed', 'locked', 'unsupported')),
    event_count INTEGER NOT NULL DEFAULT 0 CHECK (event_count >= 0),
    started_at TIMESTAMPTZ NOT NULL,
    finished_at TIMESTAMPTZ,
    duration_ms BIGINT CHECK (duration_ms IS NULL OR duration_ms >= 0),
    error_message TEXT,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (finished_at IS NULL OR finished_at >= started_at)
);

CREATE INDEX IF NOT EXISTS idx_scan_runs_tenant_started_at
    ON scan_runs(tenant_id, started_at DESC);

CREATE INDEX IF NOT EXISTS idx_scan_runs_tenant_status_started_at
    ON scan_runs(tenant_id, status, started_at DESC);

CREATE INDEX IF NOT EXISTS idx_scan_runs_address_started_at
    ON scan_runs(address_id, started_at DESC);

CREATE INDEX IF NOT EXISTS idx_scan_runs_task_id
    ON scan_runs(task_id);
