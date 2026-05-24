DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM custody_accounts
        GROUP BY tenant_id, address_normalized
        HAVING COUNT(*) > 1
    ) THEN
        RAISE EXCEPTION 'duplicate custody account normalized addresses must be merged before applying multi-chain custody migration';
    END IF;
END $$;

CREATE TABLE IF NOT EXISTS custody_account_chain_configs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    custody_account_id UUID NOT NULL,
    chain_id UUID NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, custody_account_id, chain_id),
    UNIQUE (id, tenant_id),
    FOREIGN KEY (custody_account_id, tenant_id)
        REFERENCES custody_accounts(id, tenant_id)
        ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS custody_account_chain_config_assets (
    chain_config_id UUID NOT NULL REFERENCES custody_account_chain_configs(id) ON DELETE CASCADE,
    asset_id UUID NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    PRIMARY KEY (chain_config_id, asset_id)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_custody_accounts_tenant_address_normalized
ON custody_accounts(tenant_id, address_normalized);

CREATE INDEX IF NOT EXISTS idx_custody_chain_configs_tenant_account
ON custody_account_chain_configs(tenant_id, custody_account_id);

INSERT INTO custody_account_chain_configs (tenant_id, custody_account_id, chain_id)
SELECT tenant_id, id, chain_id
FROM custody_accounts
ON CONFLICT (tenant_id, custody_account_id, chain_id) DO NOTHING;

INSERT INTO custody_account_chain_config_assets (chain_config_id, asset_id)
SELECT config.id, asset.id
FROM custody_account_chain_configs config
JOIN assets asset ON asset.chain_id = config.chain_id
WHERE asset.asset_type = 'native'
  AND asset.status = 'active'
ON CONFLICT DO NOTHING;
