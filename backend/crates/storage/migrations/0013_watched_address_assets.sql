CREATE TABLE IF NOT EXISTS watched_address_assets (
    address_id UUID NOT NULL REFERENCES watched_addresses(id) ON DELETE CASCADE,
    asset_id UUID NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (address_id, asset_id)
);

CREATE INDEX IF NOT EXISTS idx_watched_address_assets_asset
    ON watched_address_assets(asset_id);
