-- Draft manifests shared across windows, the assistant, and MCP (AG-61, UI-47).
CREATE TABLE IF NOT EXISTS drafts (
  project      text        NOT NULL,
  kind         text        NOT NULL,
  name         text        NOT NULL,
  manifest     jsonb       NOT NULL,
  verdict      jsonb,
  touched_by   text        NOT NULL,
  touched_kind text        NOT NULL,
  version      bigint      NOT NULL DEFAULT 1,
  updated_at   timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (project, kind, name)
);

CREATE INDEX IF NOT EXISTS drafts_project_updated ON drafts (project, updated_at DESC);
