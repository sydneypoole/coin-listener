CREATE TABLE IF NOT EXISTS balance_snapshots (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    chain_id UUID NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
    address_id UUID NOT NULL REFERENCES watched_addresses(id) ON DELETE CASCADE,
    asset_id UUID NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    balance_raw TEXT NOT NULL,
    balance_decimal TEXT NOT NULL,
    block_number BIGINT,
    block_hash TEXT,
    observed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    source_provider_id UUID REFERENCES providers(id) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS address_events (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    chain_id UUID NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
    address_id UUID NOT NULL REFERENCES watched_addresses(id) ON DELETE CASCADE,
    asset_id UUID NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    event_type TEXT NOT NULL,
    direction TEXT NOT NULL,
    is_transfer BOOLEAN NOT NULL DEFAULT FALSE,
    tx_hash TEXT,
    log_index INTEGER,
    block_number BIGINT,
    block_hash TEXT,
    confirmations INTEGER NOT NULL DEFAULT 0,
    from_address TEXT,
    to_address TEXT,
    amount_raw TEXT,
    amount_decimal TEXT,
    balance_before_raw TEXT,
    balance_after_raw TEXT,
    balance_delta_raw TEXT,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    detected_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_balance_snapshots_address_asset_time ON balance_snapshots(address_id, asset_id, observed_at DESC);
CREATE INDEX IF NOT EXISTS idx_address_events_tenant_time ON address_events(tenant_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_address_events_transfer_time ON address_events(tenant_id, is_transfer, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_address_events_chain_time ON address_events(tenant_id, chain_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_address_events_address_time ON address_events(tenant_id, address_id, created_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS idx_address_events_unique_transfer ON address_events(
    chain_id,
    tx_hash,
    COALESCE(log_index, -1),
    address_id,
    asset_id,
    event_type
) WHERE tx_hash IS NOT NULL;
