-- Alerts a person chooses (API/01 §37, PL-71, T-3261): what each person subscribed to, the
-- incidents the Portal opened and closed, and the notices it left each subscriber.
CREATE TABLE IF NOT EXISTS alert_subscriptions (
  id          bigserial   PRIMARY KEY,
  subject     text        NOT NULL,
  project     text        NOT NULL,
  scope       text        NOT NULL CHECK (scope IN ('pipeline', 'space', 'type')),
  target      text        NOT NULL,
  events      text[]      NOT NULL,
  delivery    text        NOT NULL CHECK (delivery IN ('portal', 'digest')),
  muted_until timestamptz,
  digest_at   timestamptz,
  created_at  timestamptz NOT NULL DEFAULT now(),
  UNIQUE (subject, project, scope, target)
);

-- One open incident per pipeline and event: opened when its signal starts, closed when it clears.
CREATE TABLE IF NOT EXISTS alert_incidents (
  id           bigserial   PRIMARY KEY,
  project      text        NOT NULL,
  pipeline     text        NOT NULL,
  event        text        NOT NULL CHECK (event IN ('failure', 'stale', 'zero')),
  detail       text        NOT NULL DEFAULT '',
  opened_at    timestamptz NOT NULL DEFAULT now(),
  recovered_at timestamptz
);
CREATE UNIQUE INDEX IF NOT EXISTS alert_incidents_open ON alert_incidents (project, pipeline, event) WHERE recovered_at IS NULL;

CREATE TABLE IF NOT EXISTS alert_notices (
  id           bigserial   PRIMARY KEY,
  recipient    text        NOT NULL,
  subscription bigint      NOT NULL REFERENCES alert_subscriptions (id) ON DELETE CASCADE,
  project      text        NOT NULL,
  pipeline     text        NOT NULL,
  event        text        NOT NULL,
  change       text        NOT NULL CHECK (change IN ('opened', 'recovered', 'digest')),
  detail       text        NOT NULL DEFAULT '',
  created_at   timestamptz NOT NULL DEFAULT now(),
  read_at      timestamptz
);
CREATE INDEX IF NOT EXISTS alert_notices_recipient ON alert_notices (recipient, id DESC);
