CREATE TABLE IF NOT EXISTS provider_health (
    provider_id UUID PRIMARY KEY REFERENCES providers(id) ON DELETE CASCADE,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    last_success_at TIMESTAMPTZ,
    last_failure_at TIMESTAMPTZ,
    disabled_until TIMESTAMPTZ,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_provider_health_disabled_until
    ON provider_health(disabled_until)
    WHERE disabled_until IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_provider_health_last_failure
    ON provider_health(last_failure_at DESC);
