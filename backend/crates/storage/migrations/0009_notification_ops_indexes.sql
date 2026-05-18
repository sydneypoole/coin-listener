CREATE INDEX IF NOT EXISTS idx_notification_outbox_tenant_status_created
    ON notification_outbox(tenant_id, status, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_notification_outbox_tenant_next_attempt
    ON notification_outbox(tenant_id, next_attempt_at);

CREATE INDEX IF NOT EXISTS idx_notification_deliveries_tenant_status_created
    ON notification_deliveries(tenant_id, status, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_notification_deliveries_tenant_channel_type_created
    ON notification_deliveries(tenant_id, channel_type, created_at DESC);
