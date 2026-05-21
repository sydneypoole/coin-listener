CREATE UNIQUE INDEX IF NOT EXISTS idx_telegram_bots_id_tenant
    ON telegram_bots(id, tenant_id);

CREATE TABLE IF NOT EXISTS telegram_binding_requests (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    telegram_bot_id UUID NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    bind_token TEXT NOT NULL,
    short_code TEXT NOT NULL,
    deep_link_url TEXT,
    chat_id TEXT,
    chat_type TEXT,
    chat_title TEXT,
    chat_username TEXT,
    confirmation_error TEXT,
    expires_at TIMESTAMPTZ NOT NULL,
    bound_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT telegram_binding_requests_bot_tenant_fk FOREIGN KEY (telegram_bot_id, tenant_id)
        REFERENCES telegram_bots(id, tenant_id) ON DELETE CASCADE,
    CONSTRAINT telegram_binding_requests_status_check CHECK (status IN ('pending', 'bound', 'expired', 'cancelled')),
    CONSTRAINT telegram_binding_requests_pending_chat_check CHECK (
        status <> 'bound' OR chat_id IS NOT NULL
    )
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_telegram_binding_requests_bind_token
    ON telegram_binding_requests(bind_token);

CREATE UNIQUE INDEX IF NOT EXISTS idx_telegram_binding_requests_pending_short_code
    ON telegram_binding_requests(tenant_id, short_code)
    WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS idx_telegram_binding_requests_bot_status
    ON telegram_binding_requests(telegram_bot_id, status, expires_at);

CREATE TABLE IF NOT EXISTS telegram_bot_update_offsets (
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    telegram_bot_id UUID NOT NULL,
    last_update_id BIGINT NOT NULL DEFAULT 0,
    locked_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, telegram_bot_id),
    CONSTRAINT telegram_bot_update_offsets_bot_tenant_fk FOREIGN KEY (telegram_bot_id, tenant_id)
        REFERENCES telegram_bots(id, tenant_id) ON DELETE CASCADE
);
