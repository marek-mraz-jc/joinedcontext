-- Pipeline access requests (API/01 §47, PL-90…PL-93, T-4863): a pipeline's ask for its
-- ServiceAccount's read or write on an Endpoint of another space. A row grants nothing; the
-- grant is the Change an owner of the Endpoint's space proposes when approving it.
CREATE TABLE IF NOT EXISTS access_requests (
  id               text        PRIMARY KEY CHECK (id ~ '^ar-[0-9a-f]{12}$'),
  project          text        NOT NULL,
  pipeline         text        NOT NULL,
  endpoint_project text        NOT NULL,
  endpoint         text        NOT NULL,
  space            text        NOT NULL,
  operations       text        NOT NULL CHECK (operations IN ('read', 'write')),
  -- [{"type": "AirQualityObserved", "attrs": ["pm10", …]}, …]
  types            jsonb       NOT NULL,
  until            timestamptz,
  reason           text        NOT NULL CHECK (char_length(reason) BETWEEN 1 AND 500),
  state            text        NOT NULL CHECK (state IN ('pending', 'approved', 'declined', 'revoked')),
  requested_by     text        NOT NULL,
  requested_at     timestamptz NOT NULL DEFAULT now(),
  decided_by       text,
  decided_at       timestamptz,
  decision_reason  text CHECK (decision_reason IS NULL OR char_length(decision_reason) <= 500),
  change           text
);
-- One open or granted request per pipeline and Endpoint (PL-90).
CREATE UNIQUE INDEX IF NOT EXISTS access_requests_one_open
  ON access_requests (project, pipeline, endpoint_project, endpoint)
  WHERE state IN ('pending', 'approved');
CREATE INDEX IF NOT EXISTS access_requests_incoming
  ON access_requests (endpoint_project, requested_at DESC);
CREATE INDEX IF NOT EXISTS access_requests_outgoing
  ON access_requests (project, pipeline, requested_at DESC);
