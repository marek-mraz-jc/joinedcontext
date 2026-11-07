-- The milliseconds from a run's creation to its first generated version, set once. NULL until then.
ALTER TABLE agent_runs ADD COLUMN IF NOT EXISTS first_version_ms bigint;
