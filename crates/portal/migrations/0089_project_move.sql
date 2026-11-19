-- Moving a project to another Organization (T-7305): an operator `ProjectMove` Change, approved by
-- a second operator, re-keys the project's registry row and every row keyed by its id in one
-- transaction. 0067 recorded a project's Organization once and never moved it; jc-dev's registry
-- filed four projects under the installation Organization while they belong to Organizations of
-- their own, and only a move puts them right.
--
-- The moving transaction sets `jc.org` and `jc.project` to the project's old key, as `tenant_tx`
-- does, and `jc.move_to` to the target Organization's key. The `project_move` policy gives such a
-- transaction the rights over (target, project) that `tenant` gives a transaction keyed with it: no
-- more than setting `jc.org` to the target would, and nothing over another project. An UPDATE of
-- `org_id` needs both, `tenant` to reach the old row and `project_move` to write and read back the
-- new one. `tenant` itself is unchanged.

ALTER TABLE operator_changes DROP CONSTRAINT IF EXISTS operator_changes_kind_check;
ALTER TABLE operator_changes ADD CONSTRAINT operator_changes_kind_check
    CHECK (kind IN ('HostAdd', 'HostRemove', 'OperatorAdd', 'OperatorRemove', 'SmtpSet',
                    'BrandingSet', 'ProjectMove'));

DO $$
DECLARE
    t text;
BEGIN
    FOR t IN SELECT tablename FROM pg_policies
             WHERE schemaname = current_schema() AND policyname = 'tenant' LOOP
        EXECUTE format('DROP POLICY IF EXISTS project_move ON %1$I', t);
        EXECUTE format(
            'CREATE POLICY project_move ON %1$I
                 USING (org_id = nullif(current_setting(''jc.move_to'', true), '''')::uuid
                        AND project_id = nullif(current_setting(''jc.project'', true), '''')::uuid)
                 WITH CHECK (org_id = nullif(current_setting(''jc.move_to'', true), '''')::uuid
                        AND project_id = nullif(current_setting(''jc.project'', true), '''')::uuid)',
            t);
    END LOOP;
END
$$;

-- An id is never renamed, released or reused (0066). A project's Organization is recorded once,
-- from none, and moves only inside a transaction keyed for that move: `jc.project` names the row
-- and `jc.move_to` the new Organization's key.
CREATE OR REPLACE FUNCTION project_ids_keep() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'DELETE' OR NEW.id <> OLD.id OR NEW.name <> OLD.name
        OR NEW.parent IS DISTINCT FROM OLD.parent OR NEW.minted_at <> OLD.minted_at
        OR NEW.reserved <> OLD.reserved THEN
        RAISE EXCEPTION 'project id % is never renamed, released or reused', OLD.id
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    IF OLD.organization IS NOT NULL AND NEW.organization IS DISTINCT FROM OLD.organization
        AND NOT (NOT OLD.reserved AND OLD.parent IS NULL
                 AND current_setting('jc.project', true) = OLD.id::text
                 AND current_setting('jc.move_to', true) =
                     (SELECT key::text FROM organizations WHERE name = NEW.organization)) THEN
        RAISE EXCEPTION 'project id % belongs to Organization % and moves only by an approved ProjectMove',
            OLD.id, OLD.organization
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    RETURN NEW;
END
$$ LANGUAGE plpgsql;
