-- The daily data-quality run's last report per space (DM-74, T-5066), so a restarted Portal shows
-- it and does not run again before it is due. `new_rules` lists the `rule path` pairs the report
-- breaks that the one before did not; they open an `invalid` alert. Counts and entity ids only.
CREATE TABLE IF NOT EXISTS space_quality (
  project   text        NOT NULL,
  space     text        NOT NULL,
  report    jsonb       NOT NULL,
  new_rules text[]      NOT NULL DEFAULT '{}',
  at        timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (project, space)
);

ALTER TABLE alert_incidents DROP CONSTRAINT IF EXISTS alert_incidents_event_check;
ALTER TABLE alert_incidents ADD CONSTRAINT alert_incidents_event_check
  CHECK (event IN ('failure', 'stale', 'zero', 'invalid'));
