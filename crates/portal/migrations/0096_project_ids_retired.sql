-- A deleted project's id is retired, never handed to the next project of its name (TEN-01,
-- TEN-16; T-7557). `seen_at` is set once, the first time a complete leader run finds the project
-- in its repository; `deleted_at` is set once, when a complete leader run no longer finds a
-- project it saw. A retired row keeps its id for the rows, copies, branches and backup folders
-- that name it, and leaves the live index, so a re-created name mints a fresh id. Neither column
-- is cleared or changed after it is set.
ALTER TABLE project_ids
    ADD COLUMN IF NOT EXISTS seen_at timestamptz,
    ADD COLUMN IF NOT EXISTS deleted_at timestamptz;
ALTER TABLE project_ids DROP CONSTRAINT IF EXISTS project_ids_deleted_seen;
ALTER TABLE project_ids ADD CONSTRAINT project_ids_deleted_seen
    CHECK (deleted_at IS NULL OR (seen_at IS NOT NULL AND parent IS NULL AND NOT reserved));

DROP INDEX IF EXISTS project_ids_live_project;
CREATE UNIQUE INDEX IF NOT EXISTS project_ids_live_project ON project_ids (name)
    WHERE parent IS NULL AND NOT reserved AND deleted_at IS NULL;

CREATE OR REPLACE FUNCTION project_ids_keep() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'DELETE' OR NEW.id <> OLD.id OR NEW.name <> OLD.name
        OR NEW.parent IS DISTINCT FROM OLD.parent OR NEW.minted_at <> OLD.minted_at
        OR NEW.reserved <> OLD.reserved
        OR (OLD.seen_at IS NOT NULL AND NEW.seen_at IS DISTINCT FROM OLD.seen_at)
        OR (OLD.deleted_at IS NOT NULL AND NEW.deleted_at IS DISTINCT FROM OLD.deleted_at) THEN
        RAISE EXCEPTION 'project id % is never renamed, released or reused', OLD.id
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    IF OLD.organization IS NOT NULL AND NEW.organization IS DISTINCT FROM OLD.organization
        AND NOT (NOT OLD.reserved AND OLD.parent IS NULL AND OLD.deleted_at IS NULL
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

-- project_move as 0093 left it, moving only the live row of the name.
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
    held boolean;
    n bigint;
    counts jsonb := '[]'::jsonb;
    total bigint := 0;
BEGIN
    -- Called by its owner the policies below do not apply: the one-role Portal cannot move.
    IF session_user = current_user THEN
        RAISE EXCEPTION 'project_move runs from the serving role only: set JC_PORTAL_MIGRATION_DATABASE_URL to the owning role and JC_PORTAL_DATABASE_URL to a role that owns nothing'
            USING ERRCODE = 'insufficient_privilege';
    END IF;
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
        WHERE p.name = asked.payload->>'project' AND p.parent IS NULL AND NOT p.reserved AND p.deleted_at IS NULL
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
        PERFORM set_config('jc.project_move', moved::text, true);
        FOR t IN
            SELECT c.relname::text FROM pg_class c
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
            EXECUTE format('UPDATE %I SET org_id = $1 WHERE org_id = $2 AND project_id = $3', t)
                USING to_key, from_key, moved;
            GET DIAGNOSTICS n = ROW_COUNT;
            moved_table := t;
            moved_rows := n;
            counts := counts || jsonb_build_object('table', t, 'rows', n);
            total := total + n;
            RETURN NEXT;
        END LOOP;
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
