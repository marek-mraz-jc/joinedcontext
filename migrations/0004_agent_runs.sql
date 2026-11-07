-- Agent builder run lifecycle and append-only event stream (AG-33..AG-46, AP-51).
CREATE TABLE IF NOT EXISTS agent_runs (
  id              text        PRIMARY KEY,
  project         text        NOT NULL,
  app_name        text        NOT NULL,
  endpoint_name   text        NOT NULL,
  endpoint_slug   text        NOT NULL,
  profile         text        NOT NULL,
  app_class       text        NOT NULL,
  visibility      text        NOT NULL,
  prompt          text        NOT NULL,
  prompt_digest   text        NOT NULL,
  data_needs      jsonb       NOT NULL,
  allows_write    boolean     NOT NULL DEFAULT false,
  branch          text        NOT NULL,
  path_prefix     text        NOT NULL,
  status          text        NOT NULL,
  ticket_hash     text        NOT NULL,
  workspace       text,
  merge_request   integer,
  preview_url     text,
  steps           integer     NOT NULL DEFAULT 0,
  tokens_used     bigint      NOT NULL DEFAULT 0,
  created_by      text        NOT NULL,
  created_at      timestamptz NOT NULL DEFAULT now(),
  started_at      timestamptz,
  finished_at     timestamptz,
  expires_at      timestamptz NOT NULL,
  error           text
);

CREATE INDEX IF NOT EXISTS agent_runs_project_created ON agent_runs (project, created_at DESC);

CREATE TABLE IF NOT EXISTS agent_run_events (
  run_id     text        NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
  seq        bigint      NOT NULL,
  kind       text        NOT NULL,
  payload    jsonb       NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (run_id, seq)
);
