-- When the link of the last invitation the Portal sent a person stops working (PF-108, T-3237).
--
-- The realm does not say when its execute-actions e-mail went out, so the Portal remembers the
-- expiry it asked for; the people page shows it beside a pending invitation, and an invitation
-- sent again leads into the same project. The row holds the realm's user id, an instant and a
-- project name, never a name, an e-mail or the link.
CREATE TABLE IF NOT EXISTS person_invitations (
  person_id  text        PRIMARY KEY,
  expires_at timestamptz NOT NULL,
  project    text
);
