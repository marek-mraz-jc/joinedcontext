-- D1 (RFC 8628): device authorizations. Forward-only migration 0003.
-- One row per device code; the user code is unique per realm. Both are SHA-256 hashes only.

CREATE TABLE kaisatsu.device_code (
    realm_id text NOT NULL COLLATE "C",
    token_hash text NOT NULL COLLATE "C",
    user_code_hash text NOT NULL COLLATE "C",
    consumed boolean NOT NULL DEFAULT false,
    doc jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, token_hash)
);
CREATE UNIQUE INDEX idx_device_code_user_code
    ON kaisatsu.device_code (realm_id, user_code_hash);
ALTER TABLE kaisatsu.device_code ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.device_code FORCE ROW LEVEL SECURITY;
CREATE POLICY device_code_rls ON kaisatsu.device_code
    USING (realm_id = kaisatsu.current_realm())
    WITH CHECK (realm_id = kaisatsu.current_realm());

GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE kaisatsu.device_code TO kaisatsu_app;
GRANT USAGE ON ALL SEQUENCES IN SCHEMA kaisatsu TO kaisatsu_app;
