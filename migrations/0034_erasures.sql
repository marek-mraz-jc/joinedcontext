-- The erasure log (API/01 §46, DM-126, DM-127, ADR-N-054 §2.6, T-4668): one row per data
-- subject's erasure in a space, kept outside the broker's database. It holds the subject's keyed
-- hash, never the value, the Entities it named and one state per store the erasure reaches; the
-- erasure stays open until every store reports done, and is asked again each minute until then.
CREATE TABLE IF NOT EXISTS erasures (
  id           text        PRIMARY KEY,
  project      text        NOT NULL,
  space        text        NOT NULL,
  subject      text        NOT NULL CHECK (subject ~ '^[0-9a-f]{32}$'),
  reason       text        NOT NULL CHECK (char_length(reason) BETWEEN 1 AND 500),
  actor        text        NOT NULL,
  requested_at timestamptz NOT NULL DEFAULT now(),
  completed_at timestamptz,
  -- [{"entities": [urn…], "attrs": [term…] | null}], one group per class of the model.
  groups       jsonb       NOT NULL,
  -- {"broker": {"state": "pending"|"done"|"failed", "count": n, "attempts": n, "error": …}, …}
  stores       jsonb       NOT NULL
);
CREATE INDEX IF NOT EXISTS erasures_open ON erasures (requested_at) WHERE completed_at IS NULL;
CREATE INDEX IF NOT EXISTS erasures_by_space ON erasures (project, space, requested_at DESC);
