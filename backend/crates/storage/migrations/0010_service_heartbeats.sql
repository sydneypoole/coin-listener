CREATE TABLE IF NOT EXISTS service_heartbeats (
    service_name TEXT NOT NULL,
    instance_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'online',
    started_at TIMESTAMPTZ NOT NULL,
    last_seen_at TIMESTAMPTZ NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (service_name, instance_id)
);

CREATE INDEX IF NOT EXISTS idx_service_heartbeats_last_seen
    ON service_heartbeats(last_seen_at DESC);

CREATE INDEX IF NOT EXISTS idx_service_heartbeats_service
    ON service_heartbeats(service_name, last_seen_at DESC);
