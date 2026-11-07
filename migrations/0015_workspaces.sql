-- The registry of workspaces: a named branch of the Organization repository with an owner,
-- a base revision, a scope and a TTL (CC-76, ADR-N-024). Beside the drafts, because it
-- describes a branch and so does not live on one.
CREATE TABLE IF NOT EXISTS workspaces (
  name          text        PRIMARY KEY,
  title         text,
  project       text        NOT NULL,
  owner         text        NOT NULL,
  base_revision text        NOT NULL,
  scope         jsonb       NOT NULL,
  preview_state text        NOT NULL DEFAULT 'none',
  created_at    timestamptz NOT NULL DEFAULT now(),
  expires_at    timestamptz NOT NULL
);

CREATE INDEX IF NOT EXISTS workspaces_project ON workspaces (project);
CREATE INDEX IF NOT EXISTS workspaces_expires ON workspaces (expires_at);
