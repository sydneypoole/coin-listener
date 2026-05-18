ALTER TABLE notification_deliveries
    ADD COLUMN IF NOT EXISTS channel_type TEXT,
    ADD COLUMN IF NOT EXISTS idempotency_key TEXT,
    ADD COLUMN IF NOT EXISTS provider_message_id TEXT,
    ADD COLUMN IF NOT EXISTS provider_status_code INTEGER,
    ADD COLUMN IF NOT EXISTS provider_response TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_notification_deliveries_idempotency
    ON notification_deliveries(event_id, rule_id, channel_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
