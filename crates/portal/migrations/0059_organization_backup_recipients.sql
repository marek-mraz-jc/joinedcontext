-- The age recipients an Organization's data backup is encrypted to (T-7168): the working one
-- and the recovery one, public keys (`age1…`). The configuration backup keeps the list empty.
ALTER TABLE organization_backup_targets
    ADD COLUMN IF NOT EXISTS recipients text[] NOT NULL DEFAULT '{}';

-- A restore of a data backup is two administrators' act (T-7171): the first checks a version
-- and asks, a second approves with the identity again, and only then are rows written. The
-- identity is never stored; the plan is the dry run the request was asked on.
CREATE TABLE IF NOT EXISTS organization_data_restores (
    id bigserial PRIMARY KEY,
    organization text NOT NULL,
    version_id text NOT NULL,
    requested_by text NOT NULL,
    requested_at timestamptz NOT NULL DEFAULT now(),
    state text NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'applied', 'declined', 'failed')),
    plan jsonb NOT NULL,
    decided_by text,
    decided_at timestamptz,
    message text NOT NULL DEFAULT ''
);

CREATE INDEX IF NOT EXISTS organization_data_restores_by_organization
    ON organization_data_restores (organization, id DESC);

ALTER TABLE organization_backup_log DROP CONSTRAINT IF EXISTS organization_backup_log_action_check;
ALTER TABLE organization_backup_log ADD CONSTRAINT organization_backup_log_action_check
    CHECK (action IN ('run', 'delete', 'restore-plan', 'restore-request', 'restore-decline', 'restore'));
