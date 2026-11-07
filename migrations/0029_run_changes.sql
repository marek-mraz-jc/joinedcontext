-- What a run's written records did (T-3304, PL-62): created, updated or unchanged, from the hash
-- the outcome sink sends with each id. The last hash per entity is kept and nothing of the record.
-- A run reported without hashes keeps the three counts NULL: unknown, never zero.
ALTER TABLE pipeline_runs ADD COLUMN IF NOT EXISTS created bigint;
ALTER TABLE pipeline_runs ADD COLUMN IF NOT EXISTS updated bigint;
ALTER TABLE pipeline_runs ADD COLUMN IF NOT EXISTS unchanged bigint;

CREATE TABLE IF NOT EXISTS pipeline_record_hashes (
  project  text NOT NULL,
  pipeline text NOT NULL,
  record   text NOT NULL,
  hash     text NOT NULL,
  PRIMARY KEY (project, pipeline, record)
);
