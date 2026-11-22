-- T-7549: a unique key is one Organization's, so a name one Organization holds is free in every
-- other. In a ledger of its own, which a previous release never reads, as the projection's is.
-- A previous release infers none of the indexes replaced here (STO-11). It does infer
-- (instance_id, idempotency_key) on shako.outbox and (id) on every table: those two are dropped
-- one release later, and every idempotency key names a repository id or an Organization id.
-- Rollback: nothing to run. The previous release works on these indexes.
-- The tables are small; a lock not had in 3 s fails the start and changes nothing.
SET LOCAL lock_timeout = '3s';

CREATE UNIQUE INDEX IF NOT EXISTS idx_shako_repo_org_owner_name
    ON shako.repo (org_id, instance_id, owner, name);
DROP INDEX IF EXISTS shako.idx_shako_repo_instance_owner_name;

CREATE UNIQUE INDEX IF NOT EXISTS idx_shako_protection_rule_key
    ON shako.protection_rule (org_id, instance_id, repo_id, kind, rule_name);
ALTER TABLE shako.protection_rule
    DROP CONSTRAINT IF EXISTS protection_rule_instance_id_repo_id_kind_rule_name_key;

CREATE UNIQUE INDEX IF NOT EXISTS idx_shako_pull_request_key
    ON shako.pull_request (org_id, instance_id, repo_id, number);
ALTER TABLE shako.pull_request
    DROP CONSTRAINT IF EXISTS pull_request_instance_id_repo_id_number_key;

CREATE UNIQUE INDEX IF NOT EXISTS idx_shako_commit_status_key
    ON shako.commit_status (org_id, instance_id, repo_id, sha, context);
ALTER TABLE shako.commit_status
    DROP CONSTRAINT IF EXISTS commit_status_instance_id_repo_id_sha_context_key;

CREATE UNIQUE INDEX IF NOT EXISTS idx_shako_push_mirror_key
    ON shako.push_mirror (org_id, instance_id, repo_id, remote_name);
ALTER TABLE shako.push_mirror
    DROP CONSTRAINT IF EXISTS push_mirror_instance_id_repo_id_remote_name_key;
