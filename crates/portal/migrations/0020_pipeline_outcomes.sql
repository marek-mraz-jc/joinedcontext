-- What the pipeline runner reports about the records it did not write, and the log of every run
-- (PL-61, PL-62, ADR-N-034, T-2708, T-2710).
--
-- A refused record is kept with the rule it broke so a steward can fix the mapping or the model
-- and replay it; its secret-shaped values are masked before it is stored. Both tables are bounded
-- per pipeline by the Portal (the newest 1000 rejected records, the newest 5000 log lines, the
-- newest 200 runs), so a feed that breaks the model on every message cannot fill the database.
-- A run's counts outlive its lines: a run of 4000 records keeps its numbers after its lines age out.
CREATE TABLE IF NOT EXISTS pipeline_rejected (
  id        bigserial   PRIMARY KEY,
  project   text        NOT NULL,
  pipeline  text        NOT NULL,
  at        timestamptz NOT NULL DEFAULT now(),
  record    jsonb       NOT NULL,
  rule      text        NOT NULL,
  path      text        NOT NULL DEFAULT '',
  message   text        NOT NULL,
  step      integer
);
CREATE INDEX IF NOT EXISTS pipeline_rejected_by_pipeline ON pipeline_rejected (project, pipeline, id DESC);

CREATE TABLE IF NOT EXISTS pipeline_log (
  id        bigserial   PRIMARY KEY,
  project   text        NOT NULL,
  pipeline  text        NOT NULL,
  run       text        NOT NULL,
  at        timestamptz NOT NULL DEFAULT now(),
  record_id text        NOT NULL DEFAULT '',
  step      integer,
  outcome   text        NOT NULL CHECK (outcome IN ('sent', 'rejected', 'failed')),
  message   text        NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS pipeline_log_by_pipeline ON pipeline_log (project, pipeline, id DESC);
CREATE INDEX IF NOT EXISTS pipeline_log_by_run ON pipeline_log (project, pipeline, run);

CREATE TABLE IF NOT EXISTS pipeline_runs (
  project   text        NOT NULL,
  pipeline  text        NOT NULL,
  run       text        NOT NULL,
  first_at  timestamptz NOT NULL DEFAULT now(),
  last_at   timestamptz NOT NULL DEFAULT now(),
  sent      bigint      NOT NULL DEFAULT 0,
  rejected  bigint      NOT NULL DEFAULT 0,
  failed    bigint      NOT NULL DEFAULT 0,
  PRIMARY KEY (project, pipeline, run)
);
CREATE INDEX IF NOT EXISTS pipeline_runs_by_last ON pipeline_runs (project, pipeline, last_at DESC);
