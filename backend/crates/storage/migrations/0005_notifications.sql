CREATE TABLE IF NOT EXISTS notification_channels (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    channel_type TEXT NOT NULL,
    name TEXT NOT NULL,
    config JSONB NOT NULL DEFAULT '{}'::jsonb,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_notification_channels_tenant_status
    ON notification_channels(tenant_id, status, channel_type);

CREATE UNIQUE INDEX IF NOT EXISTS idx_notification_channels_default_in_app
    ON notification_channels(tenant_id, channel_type, name)
    WHERE channel_type = 'in_app' AND name = 'Default In-App' AND status = 'active';

CREATE TABLE IF NOT EXISTS notification_rules (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    chain_id UUID REFERENCES chains(id) ON DELETE CASCADE,
    address_id UUID REFERENCES watched_addresses(id) ON DELETE CASCADE,
    asset_id UUID REFERENCES assets(id) ON DELETE CASCADE,
    event_type TEXT,
    is_transfer BOOLEAN,
    min_amount_raw TEXT,
    direction TEXT,
    channel_ids UUID[] NOT NULL DEFAULT '{}'::uuid[],
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_notification_rules_tenant_enabled
    ON notification_rules(tenant_id, enabled, created_at DESC);

CREATE TABLE IF NOT EXISTS notification_deliveries (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    event_id UUID NOT NULL REFERENCES address_events(id) ON DELETE CASCADE,
    rule_id UUID REFERENCES notification_rules(id) ON DELETE SET NULL,
    channel_id UUID REFERENCES notification_channels(id) ON DELETE SET NULL,
    status TEXT NOT NULL,
    attempt_count INTEGER NOT NULL DEFAULT 1,
    last_error TEXT,
    sent_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_notification_deliveries_event
    ON notification_deliveries(event_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_notification_deliveries_tenant
    ON notification_deliveries(tenant_id, created_at DESC);

CREATE TABLE IF NOT EXISTS in_app_notifications (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    event_id UUID NOT NULL REFERENCES address_events(id) ON DELETE CASCADE,
    delivery_id UUID REFERENCES notification_deliveries(id) ON DELETE SET NULL,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    read_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_in_app_notifications_tenant_created
    ON in_app_notifications(tenant_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_in_app_notifications_unread
    ON in_app_notifications(tenant_id, created_at DESC)
    WHERE read_at IS NULL;
