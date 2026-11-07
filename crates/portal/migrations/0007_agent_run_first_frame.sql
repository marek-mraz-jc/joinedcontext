-- The milliseconds from a run's creation to its first preview, set once (AP-57). NULL until then.
ALTER TABLE agent_runs ADD COLUMN IF NOT EXISTS first_frame_ms bigint;
