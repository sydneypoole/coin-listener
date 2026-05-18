UPDATE watched_addresses SET next_scan_at = NOW() WHERE status = 'active' AND next_scan_at IS NULL;
ALTER TABLE watched_addresses ALTER COLUMN next_scan_at SET DEFAULT NOW();
CREATE INDEX IF NOT EXISTS idx_watched_addresses_due_scan ON watched_addresses(status, next_scan_at, priority);
