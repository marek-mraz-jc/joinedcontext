-- The source ledger of a snapshot pipeline (PL-97, PL-100, T-4891): one row per entity id the
-- pipeline wrote, the run that last read it and how many complete runs in a row missed it. Ids
-- only, never a record. A held run is kept per pipeline until a complete run, a release or a keep
-- clears it.
CREATE TABLE IF NOT EXISTS pipeline_source_ledger (
  project    text        NOT NULL,
  pipeline   text        NOT NULL,
  entity_id  text        NOT NULL,
  first_seen timestamptz NOT NULL DEFAULT now(),
  last_seen  timestamptz NOT NULL DEFAULT now(),
  last_run   text        NOT NULL,
  misses     smallint    NOT NULL DEFAULT 0,
  PRIMARY KEY (project, pipeline, entity_id)
);

CREATE TABLE IF NOT EXISTS pipeline_source_holds (
  project  text        NOT NULL,
  pipeline text        NOT NULL,
  run      text        NOT NULL,
  reason   text        NOT NULL CHECK (reason IN ('share-exceeded', 'empty', 'incomplete')),
  would    integer     NOT NULL,
  ledger   integer     NOT NULL,
  at       timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (project, pipeline)
);
