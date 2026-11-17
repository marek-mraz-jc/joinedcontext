-- Branch projects (ARCH/TEN-12, CHG-15; T-7212): a branch is a project_ids row with a `parent`
-- and this row, its lifetime. It lives until `expires_at`; its creator or an approver of the
-- parent renews it by at most 7 days at a time, never past 30 days from creation, and only while
-- it lives: an expired branch is never renewed. No row is deleted, so an expired branch's id
-- stays a tombstone beside its project_ids row and is never reused (0066). The Portal names a
-- branch `b-{ProjectId}` (path form), a name no person can choose (RTE-05).
CREATE TABLE IF NOT EXISTS project_branches (
    id uuid PRIMARY KEY REFERENCES project_ids (id),
    created_by text NOT NULL CHECK (created_by <> ''),
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    CHECK (expires_at > created_at AND expires_at <= created_at + interval '30 days')
);

CREATE OR REPLACE FUNCTION project_branches_keep() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NOT EXISTS (SELECT 1 FROM project_ids p WHERE p.id = NEW.id AND p.parent IS NOT NULL) THEN
            RAISE EXCEPTION 'project % has no parent, so it is no branch', NEW.id
                USING ERRCODE = 'integrity_constraint_violation';
        END IF;
        RETURN NEW;
    END IF;
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'branch % stays as a tombstone', OLD.id
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    IF NEW.id <> OLD.id OR NEW.created_by <> OLD.created_by OR NEW.created_at <> OLD.created_at THEN
        RAISE EXCEPTION 'branch % keeps its creation', OLD.id
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    IF NEW.expires_at <> OLD.expires_at AND (OLD.expires_at <= now()
        OR NEW.expires_at < OLD.expires_at OR NEW.expires_at > OLD.expires_at + interval '7 days') THEN
        RAISE EXCEPTION 'branch % is renewed only while it lives, by at most 7 days at a time', OLD.id
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    RETURN NEW;
END
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS project_branches_keep ON project_branches;
CREATE TRIGGER project_branches_keep BEFORE INSERT OR UPDATE OR DELETE ON project_branches
    FOR EACH ROW EXECUTE FUNCTION project_branches_keep();
