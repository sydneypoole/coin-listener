CREATE TABLE IF NOT EXISTS scan_cursors (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    chain_id UUID NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
    address_id UUID NOT NULL REFERENCES watched_addresses(id) ON DELETE CASCADE,
    cursor_type TEXT NOT NULL,
    last_scanned_block BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(address_id, cursor_type)
);
