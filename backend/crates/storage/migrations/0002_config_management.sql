CREATE TABLE IF NOT EXISTS users (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    email TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    display_name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS tenants (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS tenant_members (
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role TEXT NOT NULL DEFAULT 'owner',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, user_id)
);

CREATE TABLE IF NOT EXISTS chains (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    key TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    chain_type TEXT NOT NULL,
    native_asset_symbol TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    default_confirmations INTEGER NOT NULL DEFAULT 12,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS providers (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    chain_id UUID NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
    provider_type TEXT NOT NULL,
    name TEXT NOT NULL,
    base_url TEXT NOT NULL,
    api_key_ref TEXT,
    priority INTEGER NOT NULL DEFAULT 100,
    qps_limit INTEGER NOT NULL DEFAULT 10,
    timeout_ms INTEGER NOT NULL DEFAULT 10000,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS assets (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    chain_id UUID NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
    asset_type TEXT NOT NULL,
    symbol TEXT NOT NULL,
    name TEXT NOT NULL,
    contract_address TEXT,
    decimals INTEGER NOT NULL,
    is_builtin BOOLEAN NOT NULL DEFAULT TRUE,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS watched_addresses (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    chain_id UUID NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
    address TEXT NOT NULL,
    label TEXT,
    priority TEXT NOT NULL DEFAULT 'normal',
    scan_interval_seconds INTEGER NOT NULL DEFAULT 300,
    transfer_filter_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    balance_change_filter_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    status TEXT NOT NULL DEFAULT 'active',
    last_scanned_at TIMESTAMPTZ,
    next_scan_at TIMESTAMPTZ,
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, chain_id, address)
);

CREATE INDEX IF NOT EXISTS idx_providers_chain_id ON providers(chain_id);
CREATE INDEX IF NOT EXISTS idx_assets_chain_id ON assets(chain_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_assets_unique_symbol_contract ON assets(chain_id, symbol, COALESCE(contract_address, ''));
CREATE INDEX IF NOT EXISTS idx_watched_addresses_tenant ON watched_addresses(tenant_id);
CREATE INDEX IF NOT EXISTS idx_watched_addresses_chain ON watched_addresses(chain_id);

WITH admin_user AS (
    INSERT INTO users (email, password_hash, display_name, status)
    VALUES ('admin@example.com', 'admin', 'Admin', 'active')
    ON CONFLICT (email) DO UPDATE SET display_name = EXCLUDED.display_name
    RETURNING id
), default_tenant AS (
    INSERT INTO tenants (id, name, status)
    VALUES ('00000000-0000-0000-0000-000000000001', 'Default Workspace', 'active')
    ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name
    RETURNING id
)
INSERT INTO tenant_members (tenant_id, user_id, role)
SELECT default_tenant.id, admin_user.id, 'owner'
FROM default_tenant, admin_user
ON CONFLICT (tenant_id, user_id) DO NOTHING;

INSERT INTO chains (id, key, name, chain_type, native_asset_symbol, default_confirmations, status)
VALUES
    ('10000000-0000-0000-0000-000000000001', 'btc', 'Bitcoin', 'utxo', 'BTC', 3, 'active'),
    ('10000000-0000-0000-0000-000000000002', 'ethereum', 'Ethereum', 'evm', 'ETH', 12, 'active'),
    ('10000000-0000-0000-0000-000000000003', 'tron', 'TRON', 'tron', 'TRX', 20, 'active'),
    ('10000000-0000-0000-0000-000000000004', 'base', 'Base', 'evm', 'ETH', 12, 'active')
ON CONFLICT (key) DO UPDATE SET name = EXCLUDED.name;

INSERT INTO assets (chain_id, asset_type, symbol, name, contract_address, decimals, is_builtin, status)
SELECT id, 'native', native_asset_symbol, native_asset_symbol, NULL, CASE key WHEN 'btc' THEN 8 WHEN 'tron' THEN 6 ELSE 18 END, TRUE, 'active'
FROM chains
ON CONFLICT (chain_id, symbol, COALESCE(contract_address, '')) DO NOTHING;

INSERT INTO assets (chain_id, asset_type, symbol, name, contract_address, decimals, is_builtin, status)
VALUES
    ('10000000-0000-0000-0000-000000000002', 'erc20', 'USDT', 'Tether USD', '0xdAC17F958D2ee523a2206206994597C13D831ec7', 6, TRUE, 'active'),
    ('10000000-0000-0000-0000-000000000002', 'erc20', 'USDC', 'USD Coin', '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48', 6, TRUE, 'active'),
    ('10000000-0000-0000-0000-000000000003', 'trc20', 'USDT', 'Tether USD', 'TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t', 6, TRUE, 'active'),
    ('10000000-0000-0000-0000-000000000004', 'erc20', 'USDC', 'USD Coin', '0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913', 6, TRUE, 'active')
ON CONFLICT (chain_id, symbol, COALESCE(contract_address, '')) DO NOTHING;
