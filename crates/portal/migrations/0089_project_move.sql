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
-- serving role may call it (db.rs grants it), and it takes no project or Organization of its own:
-- it moves what one proposed `ProjectMove` Change names, approved by an operator other than its
-- proposer, marks that Change applied and writes its audit row, all in the caller's transaction.
-- A replay finds the Change applied and is refused. Ceiling: the serving role writes
-- operator_changes, so the database binds the move to a recorded two-person Change and leaves an
-- audit row it cannot delete; who is an operator is the realm's answer, which the Portal checks.

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

CREATE OR REPLACE FUNCTION project_move(change uuid, approver text, source_ip text)
RETURNS TABLE (moved_table text, moved_rows bigint)
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp AS $$
DECLARE
    asked operator_changes%ROWTYPE;
    moved uuid;
    from_org text;
    target text;
    from_key uuid;
    to_key uuid;
    t text;
    forced boolean;
    held boolean;
    n bigint;
    counts jsonb := '[]'::jsonb;
    total bigint := 0;
BEGIN
    SELECT * INTO asked FROM operator_changes WHERE id = change FOR UPDATE;
    IF NOT FOUND OR asked.kind <> 'ProjectMove' THEN
        RAISE EXCEPTION 'operator Change % is no ProjectMove', change
            USING ERRCODE = 'no_data_found';
    END IF;
    IF asked.state <> 'proposed' THEN
        RAISE EXCEPTION 'operator Change % is % already', change, asked.state
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    IF coalesce(approver, '') = '' OR approver = asked.proposed_by THEN
        RAISE EXCEPTION 'operator Change % is approved by an operator other than its proposer', change
            USING ERRCODE = 'insufficient_privilege';
    END IF;
    target := asked.payload->>'organization';
    SELECT p.id, p.organization, o.key INTO moved, from_org, from_key
        FROM project_ids p JOIN organizations o ON o.name = p.organization
        WHERE p.name = asked.payload->>'project' AND p.parent IS NULL AND NOT p.reserved
        FOR UPDATE OF p;
    IF from_key IS NULL THEN
        RAISE EXCEPTION 'project % is no live project of the registry with an Organization',
            asked.payload->>'project'
            USING ERRCODE = 'no_data_found';
    END IF;
    SELECT key INTO to_key FROM organizations WHERE name = target AND state = 'active';
    IF to_key IS NULL THEN
        RAISE EXCEPTION 'Organization % is no active Organization of the registry', target
            USING ERRCODE = 'no_data_found';
    END IF;
    IF from_key <> to_key THEN
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
            counts := counts || jsonb_build_object('table', t, 'rows', n);
            total := total + n;
            RETURN NEXT;
        END LOOP;
        PERFORM set_config('jc.project_move', moved::text, true);
        UPDATE project_ids SET organization = target WHERE id = moved;
        PERFORM set_config('jc.org', '', true), set_config('jc.project', '', true),
                set_config('jc.project_move', '', true);
    END IF;
    UPDATE operator_changes SET state = 'applied', decided_by = approver, decided_at = now()
        WHERE id = change;
    -- Counts per table, never a row's content.
    INSERT INTO operator_audit (actor, change_id, action, before, after, source_ip)
    VALUES (approver, change, 'applied ProjectMove',
            jsonb_build_object('project', asked.payload->>'project', 'organization', from_org),
            jsonb_build_object('project', asked.payload->>'project', 'organization', target,
                               'rows', total, 'tables', counts),
            source_ip);
END
$$;

REVOKE ALL ON FUNCTION project_move(uuid, text, text) FROM PUBLIC;
