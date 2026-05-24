CREATE TABLE IF NOT EXISTS custody_accounts (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    chain_id UUID NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
    address TEXT NOT NULL,
    address_normalized TEXT NOT NULL,
    label TEXT,
    source TEXT NOT NULL CHECK (source IN ('pool', 'user')),
    status TEXT NOT NULL CHECK (status IN ('available', 'assigned', 'disabled')),
    watched_address_id UUID REFERENCES watched_addresses(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, chain_id, address_normalized),
    UNIQUE (id, tenant_id)
);

CREATE TABLE IF NOT EXISTS custody_account_assignments (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    custody_account_id UUID NOT NULL REFERENCES custody_accounts(id) ON DELETE CASCADE,
    applicant_type TEXT NOT NULL CHECK (applicant_type IN ('api', 'internal')),
    business_ref TEXT NOT NULL,
    purpose TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'released', 'cancelled')),
    watched_address_id UUID REFERENCES watched_addresses(id) ON DELETE SET NULL,
    assigned_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    released_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, business_ref),
    FOREIGN KEY (custody_account_id, tenant_id) REFERENCES custody_accounts(id, tenant_id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_custody_assignments_one_active
ON custody_account_assignments(custody_account_id)
WHERE status = 'active';

CREATE INDEX IF NOT EXISTS idx_custody_accounts_tenant_chain_status
ON custody_accounts(tenant_id, chain_id, status, created_at ASC);

CREATE INDEX IF NOT EXISTS idx_custody_assignments_tenant_status
ON custody_account_assignments(tenant_id, status, assigned_at DESC);
