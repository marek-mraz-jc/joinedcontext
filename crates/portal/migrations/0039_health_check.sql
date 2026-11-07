-- The health log (T-5067): one row per project, kind, name and check with its latest status, since
-- when it holds, when it was last ok, what it said and how to fix it; and the statuses over time,
-- one row per change or per hour, kept 90 days. Masked texts only, never a record or a secret.
CREATE TABLE IF NOT EXISTS health_check (
  project      text        NOT NULL,
  kind         text        NOT NULL CHECK (kind IN ('pipeline', 'space', 'endpoint', 'model', 'datasource')),
  name         text        NOT NULL,
  "check"      text        NOT NULL,
  status       text        NOT NULL CHECK (status IN ('ok', 'warn', 'error')),
  since        timestamptz NOT NULL,
  last_ok      timestamptz,
  checked_at   timestamptz NOT NULL,
  detail       text        NOT NULL DEFAULT '',
  fix_hint     text        NOT NULL DEFAULT '',
  evidence_url text        NOT NULL DEFAULT '',
  -- The detail and the fix as the UI translates them: {"code": key under health.say, "args": {...}}.
  detail_say   jsonb       NOT NULL DEFAULT '{}',
  fix_say      jsonb       NOT NULL DEFAULT '{}',
  PRIMARY KEY (project, kind, name, "check")
);

CREATE TABLE IF NOT EXISTS health_check_history (
  project text        NOT NULL,
  kind    text        NOT NULL,
  name    text        NOT NULL,
  "check" text        NOT NULL,
  at      timestamptz NOT NULL,
  status  text        NOT NULL,
  PRIMARY KEY (project, kind, name, "check", at)
);

CREATE INDEX IF NOT EXISTS health_check_history_at ON health_check_history (at);
