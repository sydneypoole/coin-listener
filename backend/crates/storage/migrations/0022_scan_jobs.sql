CREATE TABLE IF NOT EXISTS scan_jobs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    address_id UUID NOT NULL REFERENCES watched_addresses(id) ON DELETE CASCADE,
    chain_id UUID NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'retryable', 'processing', 'succeeded', 'dead_letter')),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    max_attempts INTEGER NOT NULL DEFAULT 10 CHECK (max_attempts > 0),
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    locked_at TIMESTAMPTZ,
    locked_by TEXT,
    lease_expires_at TIMESTAMPTZ,
    last_error TEXT,
    last_scan_run_id UUID REFERENCES scan_runs(id) ON DELETE SET NULL,
    retry_of_scan_run_id UUID REFERENCES scan_runs(id) ON DELETE SET NULL,
    succeeded_at TIMESTAMPTZ,
    dead_lettered_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_scan_jobs_active_address
    ON scan_jobs(address_id)
    WHERE status IN ('pending', 'retryable', 'processing');

CREATE INDEX IF NOT EXISTS idx_scan_jobs_claim
    ON scan_jobs(status, next_attempt_at, created_at)
    WHERE status IN ('pending', 'retryable');

CREATE INDEX IF NOT EXISTS idx_scan_jobs_processing_stale
    ON scan_jobs(lease_expires_at)
    WHERE status = 'processing';

CREATE INDEX IF NOT EXISTS idx_scan_jobs_tenant_status_created
    ON scan_jobs(tenant_id, status, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_scan_jobs_retry_of_scan_run
    ON scan_jobs(retry_of_scan_run_id);
