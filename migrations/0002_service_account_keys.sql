-- API keys of a ServiceAccount (PF-36, PF-37). The secret is never here: `secret_hash` is the
-- Argon2id PHC string the Context Gateway verifies against, and a revoked key keeps its row so
-- the audit trail survives the revocation.
CREATE TABLE IF NOT EXISTS service_account_keys (
    key_id       text        PRIMARY KEY,
    project      text        NOT NULL,
    account      text        NOT NULL,
    credential   text        NOT NULL,
    secret_hash  text        NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    created_by   text        NOT NULL,
    expires_at   timestamptz,
    last_used_at timestamptz,
    revoked_at   timestamptz
);

CREATE INDEX IF NOT EXISTS service_account_keys_by_account
    ON service_account_keys (project, account, created_at DESC);
