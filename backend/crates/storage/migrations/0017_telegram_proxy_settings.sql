ALTER TABLE telegram_bots
    ADD COLUMN IF NOT EXISTS proxy_url TEXT;

CREATE TABLE IF NOT EXISTS telegram_settings (
    tenant_id UUID PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    proxy_url TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT telegram_settings_proxy_url_not_blank CHECK (
        proxy_url IS NULL OR btrim(proxy_url) <> ''
    )
);
