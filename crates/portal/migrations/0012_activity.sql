-- What is happening in a project (UI-31, OPS-48). A projection of the logs and traces, built for
-- reading and trimmed after seven days (OPS-49).
CREATE TABLE IF NOT EXISTS activity (
  id             bigserial   PRIMARY KEY,
  time           timestamptz NOT NULL,
  project        text        NOT NULL,
  space          text,
  kind           text        NOT NULL,
  source         text        NOT NULL,
  summary        text        NOT NULL,
  severity       text        NOT NULL,
  correlation_id text,
  details        jsonb       NOT NULL DEFAULT '{}'::jsonb
);

-- The one query the read API makes: a project's events, newest first, paged by (time, id).
CREATE INDEX IF NOT EXISTS activity_project_time ON activity (project, time DESC, id DESC);
CREATE INDEX IF NOT EXISTS activity_object ON activity ((details->>'object'));
