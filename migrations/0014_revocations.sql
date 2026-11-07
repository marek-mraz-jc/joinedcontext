-- Sessions logged out through OIDC back-channel logout (PF-11, T-0980).
--
-- A mark says: every session of this subject issued at or before `revoked_at` is refused. It
-- lived only in the process, so a restart re-accepted every session somebody had logged out,
-- for as long as the cookie's absolute timeout allowed. It is written here as well, and read
-- back at startup.
CREATE TABLE IF NOT EXISTS revocations (
  subject    text        PRIMARY KEY,
  revoked_at bigint      NOT NULL,
  -- When the mark stops mattering: no session issued before it can still be valid. Kept a day
  -- past the longest a session lives, so a clock that drifts does not resurrect one.
  expires_at timestamptz NOT NULL
);

CREATE INDEX IF NOT EXISTS revocations_expiry ON revocations (expires_at);
