-- Whether each Organization owns the domain it declares (PF-41, T-2377).
--
-- The challenge is minted once per Organization and published by its owner in DNS, so it has to
-- outlive a restart: a Portal that minted a new one on every start would fail every domain that
-- had verified. A changed `spec.domain` keeps the challenge and starts the state over.
CREATE TABLE IF NOT EXISTS domain_verifications (
  organization text        PRIMARY KEY,
  domain       text        NOT NULL,
  challenge    text        NOT NULL,
  state        text        NOT NULL CHECK (state IN ('pending', 'verified', 'failed')),
  method       text        CHECK (method IN ('dns-txt', 'did-web')),
  checked_at   timestamptz,
  reason       text
);
