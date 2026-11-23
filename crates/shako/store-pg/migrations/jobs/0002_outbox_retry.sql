-- T-7552: an event whose webhook delivery failed waits until retry_at and is claimed again,
-- attempts counting the failed passes. Job state beside the store log, never replayed from it;
-- an earlier release ignores both columns and sends a waiting event at once.
ALTER TABLE shako.outbox ADD COLUMN IF NOT EXISTS attempts INTEGER NOT NULL DEFAULT 0;
ALTER TABLE shako.outbox ADD COLUMN IF NOT EXISTS retry_at TIMESTAMPTZ;
