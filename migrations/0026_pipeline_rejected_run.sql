-- The run a rejected record belongs to (T-3252, PL-61, PL-62): the rejected list links each record
-- to its run's log. Records kept before this column, and the records a failed replay puts back,
-- have none.
ALTER TABLE pipeline_rejected ADD COLUMN IF NOT EXISTS run text;
