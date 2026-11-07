-- The two scans that ran over whole tables (F-POR1-026, F-POR1-027, T-5728).
-- The lease sweep reads the live runs whose lease ran out, oldest first: a partial index holds
-- only the live runs, so it stays small however many finished runs the history keeps. Its
-- predicate is the sweep's own status list (db.rs list_expired_agent_runs), or the planner skips it.
CREATE INDEX IF NOT EXISTS agent_runs_live_expires
  ON agent_runs (expires_at)
  WHERE status NOT IN ('published', 'failed', 'cancelled', 'expired');
-- The retention trim deletes the rows older than its floor in batches; without an index on
-- `time` every batch read the whole table.
CREATE INDEX IF NOT EXISTS activity_time ON activity (time);
