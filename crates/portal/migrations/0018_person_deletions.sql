-- People whose removal waits for its Change (PF-93, ADR-N-031, T-2683).
--
-- Deleting a person proposes one Change that takes them out of every Group and RoleBinding; the
-- Keycloak user is deleted only once that Change is merged, so the pending removal has to outlive
-- a restart. The row holds the realm's user id and the pull request, never a name or an e-mail.
CREATE TABLE IF NOT EXISTS person_deletions (
  person_id    text        PRIMARY KEY,
  pull_request bigint      NOT NULL,
  change_name  text        NOT NULL,
  requested_by text        NOT NULL,
  requested_at timestamptz NOT NULL DEFAULT now()
);
