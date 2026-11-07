-- What a person deleted from a data view, kept for them for 30 days (API/01 §31, ADR-N-042 §3.3,
-- T-3107). The row is its keeper's own copy of the entity as they read it; restoring it is their
-- own create through the gateway, so the copy grants nothing.
CREATE TABLE IF NOT EXISTS entity_trash (
  id         bigserial   PRIMARY KEY,
  project    text        NOT NULL,
  space      text        NOT NULL,
  owner      text        NOT NULL,
  urn        text        NOT NULL,
  type       text        NOT NULL,
  entity     jsonb       NOT NULL,
  deleted_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS entity_trash_keeper ON entity_trash (project, space, owner, id DESC);
