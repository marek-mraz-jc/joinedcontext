-- A scheduled backup holds a lease while it runs and is stamped run only when it succeeded
-- (CMP-09, LOP-06; T-7557). `lease_token` names the run holding the target and `lease_until`
-- how long it holds: the other replicas leave the target alone, and when the holder dies the
-- lease lapses and another replica retries. After a failure the token is cleared and
-- `lease_until` holds the retry back for the backoff. `last_run_at` is the end of the last
-- successful run.
ALTER TABLE organization_backup_targets
    ADD COLUMN IF NOT EXISTS lease_until timestamptz,
    ADD COLUMN IF NOT EXISTS lease_token text;
