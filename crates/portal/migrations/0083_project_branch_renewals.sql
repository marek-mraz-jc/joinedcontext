-- Branch renewals (ARCH/CHG-15; T-7213): one row per renewal of a branch project, who renewed it
-- and the expiry it set, written in the transaction that moves `project_branches.expires_at`. The
-- rows are the audit of CHG-15 and are never changed or removed.
CREATE TABLE IF NOT EXISTS project_branch_renewals (
    branch uuid NOT NULL REFERENCES project_branches (id),
    renewed_by text NOT NULL CHECK (renewed_by <> ''),
    renewed_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL
);

CREATE INDEX IF NOT EXISTS project_branch_renewals_branch ON project_branch_renewals (branch);

CREATE OR REPLACE FUNCTION project_branch_renewals_keep() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'a branch renewal is an audit row and stays as written'
        USING ERRCODE = 'integrity_constraint_violation';
END
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS project_branch_renewals_keep ON project_branch_renewals;
CREATE TRIGGER project_branch_renewals_keep BEFORE UPDATE OR DELETE ON project_branch_renewals
    FOR EACH ROW EXECUTE FUNCTION project_branch_renewals_keep();
