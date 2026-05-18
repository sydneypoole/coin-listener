CREATE TABLE IF NOT EXISTS notification_outbox (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    event_id UUID NOT NULL REFERENCES address_events(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'pending',
    attempt_count INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    locked_at TIMESTAMPTZ,
    locked_by TEXT,
    last_error TEXT,
    delivered_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(event_id)
);

CREATE INDEX IF NOT EXISTS idx_notification_outbox_claim
    ON notification_outbox(status, next_attempt_at, created_at)
    WHERE status IN ('pending', 'retryable');

CREATE INDEX IF NOT EXISTS idx_notification_outbox_processing_stale
    ON notification_outbox(status, locked_at)
    WHERE status = 'processing';

CREATE INDEX IF NOT EXISTS idx_notification_outbox_event
    ON notification_outbox(event_id);
