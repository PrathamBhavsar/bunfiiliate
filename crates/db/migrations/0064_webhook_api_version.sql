-- Outbound body shape per endpoint: 'v1' (default, unchanged) or opt-in 'v2' envelope.
ALTER TABLE webhook_endpoints
    ADD COLUMN api_version text NOT NULL DEFAULT 'v1' CHECK (api_version IN ('v1', 'v2'));
