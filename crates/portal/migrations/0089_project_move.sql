-- Moving a project to another Organization (T-7305): an operator `ProjectMove` Change, approved by
-- a second operator, re-keys the project's registry row and every row keyed by its id in one
-- transaction. 0067 recorded a project's Organization once and never moved it; jc-dev's registry
-- filed four projects under the installation Organization while they belong to Organizations of
-- their own, and only a move puts them right.
--
-- The policies stay as they are: every one reads only the key (STO-04). `project_move` runs as the
-- tables' owner (SECURITY DEFINER) and, inside the caller's transaction, lifts FORCE ROW LEVEL
-- SECURITY from each table that holds rows of the project for the one UPDATE that re-keys them,
-- then forces it again. ALTER TABLE is transactional and holds the table's ACCESS EXCLUSIVE lock
-- until the transaction ends, so no other transaction ever sees a table unforced. Only the
-- serving role may call it (db.rs grants it); the Portal calls it only for an approved
-- `ProjectMove` Change.

ALTER TABLE operator_changes DROP CONSTRAINT IF EXISTS operator_changes_kind_check;
ALTER TABLE operator_changes ADD CONSTRAINT operator_changes_kind_check
    CHECK (kind IN ('HostAdd', 'HostRemove', 'OperatorAdd', 'OperatorRemove', 'SmtpSet',
                    'BrandingSet', 'ProjectMove'));

-- An id is never renamed, released or reused (0066). A project's Organization is recorded once,
-- from none, and moves only inside `project_move`: as the table's owner, with the row named in
-- `jc.project_move`.
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
                 AND current_setting('jc.project_move', true) = OLD.id::text
                 AND (SELECT oid FROM pg_roles WHERE rolname = current_user)
                     = (SELECT relowner FROM pg_class WHERE oid = TG_RELID)) THEN
        RAISE EXCEPTION 'project id % belongs to Organization % and moves only by an approved ProjectMove',
            OLD.id, OLD.organization
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    RETURN NEW;
END
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION project_move(moved uuid, target text)
RETURNS TABLE (moved_table text, moved_rows bigint)
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp AS $$
DECLARE
    from_key uuid;
    to_key uuid;
    t text;
    forced boolean;
    held boolean;
    n bigint;
BEGIN
    SELECT o.key INTO from_key FROM project_ids p JOIN organizations o ON o.name = p.organization
        WHERE p.id = moved AND p.parent IS NULL AND NOT p.reserved
        FOR UPDATE OF p;
    IF from_key IS NULL THEN
        RAISE EXCEPTION 'project % is no live project of the registry with an Organization', moved
            USING ERRCODE = 'no_data_found';
    END IF;
    SELECT key INTO to_key FROM organizations WHERE name = target AND state = 'active';
    IF to_key IS NULL THEN
        RAISE EXCEPTION 'Organization % is no active Organization of the registry', target
            USING ERRCODE = 'no_data_found';
    END IF;
    IF from_key = to_key THEN
        RETURN;
    END IF;
    FOR t, forced IN
        SELECT c.relname::text, c.relforcerowsecurity FROM pg_class c
        JOIN pg_namespace ns ON ns.oid = c.relnamespace
        WHERE ns.nspname = 'public' AND c.relkind = 'r'
          AND EXISTS (SELECT 1 FROM pg_attribute a WHERE a.attrelid = c.oid AND a.attname = 'org_id' AND NOT a.attisdropped)
          AND EXISTS (SELECT 1 FROM pg_attribute a WHERE a.attrelid = c.oid AND a.attname = 'project_id' AND NOT a.attisdropped)
        ORDER BY 1
    LOOP
        -- Keyed with the old key, the policy shows the project's rows alone.
        PERFORM set_config('jc.org', from_key::text, true), set_config('jc.project', moved::text, true);
        EXECUTE format('SELECT EXISTS (SELECT 1 FROM %I WHERE org_id = $1 AND project_id = $2)', t)
            INTO held USING from_key, moved;
        CONTINUE WHEN NOT held;
        IF forced THEN
            EXECUTE format('ALTER TABLE %I NO FORCE ROW LEVEL SECURITY', t);
        END IF;
        EXECUTE format('UPDATE %I SET org_id = $1 WHERE org_id = $2 AND project_id = $3', t)
            USING to_key, from_key, moved;
        GET DIAGNOSTICS n = ROW_COUNT;
        IF forced THEN
            EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        END IF;
        moved_table := t;
        moved_rows := n;
        RETURN NEXT;
    END LOOP;
    PERFORM set_config('jc.project_move', moved::text, true);
    UPDATE project_ids SET organization = target WHERE id = moved;
    PERFORM set_config('jc.org', '', true), set_config('jc.project', '', true),
            set_config('jc.project_move', '', true);
END
$$;

REVOKE ALL ON FUNCTION project_move(uuid, text) FROM PUBLIC;
