-- A key asked for over MCP, waiting for its person in the Portal (PF-104, T-2359). What the claim
-- will do and who may use it, never a secret: nothing is minted until that person confirms it. A
-- claim lives 15 minutes and is deleted when it is used or found expired.
CREATE TABLE IF NOT EXISTS service_account_key_claims (
    id             text        PRIMARY KEY,
    project        text        NOT NULL,
    account        text        NOT NULL,
    action         text        NOT NULL CHECK (action IN ('mint', 'rotate')),
    credential     text        NOT NULL,
    key_id         text,
    key_expires_at timestamptz,
    overlap_hours  integer,
    created_by     text        NOT NULL,
    created_at     timestamptz NOT NULL,
    expires_at     timestamptz NOT NULL,
    CHECK (action = 'mint' OR key_id IS NOT NULL)
);

CREATE INDEX IF NOT EXISTS service_account_key_claims_expiry
    ON service_account_key_claims (expires_at);
