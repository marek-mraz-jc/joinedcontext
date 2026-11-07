-- Source deletes of an events pipeline and the person-edited check (PL-98, PL-100, T-5360): the
-- run whose source marked a ledger id as deleted, until its delete stream takes it; when the
-- pipeline last wrote each id, so a delete skips an entity someone changed after that; and
-- whether a removal was a snapshot's or the source's own mark. Ids only, never a record.
ALTER TABLE pipeline_source_ledger ADD COLUMN IF NOT EXISTS last_written timestamptz;
ALTER TABLE pipeline_source_ledger ADD COLUMN IF NOT EXISTS marked_run text;
CREATE INDEX IF NOT EXISTS pipeline_source_ledger_marked
  ON pipeline_source_ledger (project, pipeline) WHERE marked_run IS NOT NULL;

ALTER TABLE pipeline_source_removed ADD COLUMN IF NOT EXISTS cause text NOT NULL DEFAULT 'source-snapshot'
  CHECK (cause IN ('source-snapshot', 'source-event'));
