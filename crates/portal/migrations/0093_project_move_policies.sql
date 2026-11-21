-- A move never lifts FORCE ROW LEVEL SECURITY (T-7476). 0089's `project_move` ran ALTER TABLE
-- ... NO FORCE / FORCE per table, which holds each table's ACCESS EXCLUSIVE lock to the end of the
-- transaction: a writer holding one keyed table and waiting for another the move had locked met
-- the move as 40P01 "deadlock detected" (ci 44860151279), and the ALTER rewrote the same pg_class
-- rows as the serving-role grants (XX000, 0092). Now FORCE stays on and the move takes only the
-- row locks of its UPDATEs.
--
-- Two permissive policies per keyed table (`org_id` and `project_id`, row-level security on)
-- let the re-keying UPDATE see the project's rows and write them under the new key:
-- `project_move_read` (SELECT, which an UPDATE with a WHERE also checks on the new row) and
-- `project_move` (UPDATE). Both apply only to the table's owner (`TO` the owning role) and only
-- while `session_user <> current_user`: inside a SECURITY DEFINER function the owner owns and
-- another role calls. `project_move` is the only such function (0089, 0092, here); the serving
-- role owns nothing and is no member of the owner (db.rs refuses both), and setting a session
-- variable opens nothing, since no session can SET current_user to the owner without being it.
-- `jc.project_move` narrows the rows to the one project the function moves. The tenant policies
-- are untouched. A one-role Portal (no JC_PORTAL_MIGRATION_DATABASE_URL) calls the function as
-- the owner, so session_user = current_user: the function refuses there with an error naming the
-- setting, instead of lifting FORCE.

DO $$
DECLARE
    t text;
    owner text;
BEGIN
    FOR t, owner IN
        SELECT c.relname::text, pg_get_userbyid(c.relowner) FROM pg_class c
        JOIN pg_namespace ns ON ns.oid = c.relnamespace
        WHERE ns.nspname = 'public' AND c.relkind = 'r' AND c.relrowsecurity
          AND EXISTS (SELECT 1 FROM pg_attribute a WHERE a.attrelid = c.oid AND a.attname = 'org_id' AND NOT a.attisdropped)
          AND EXISTS (SELECT 1 FROM pg_attribute a WHERE a.attrelid = c.oid AND a.attname = 'project_id' AND NOT a.attisdropped)
        ORDER BY 1
    LOOP
        EXECUTE format(
            'CREATE POLICY project_move_read ON %1$I FOR SELECT TO %2$I
                 USING (session_user <> current_user
                        AND project_id = nullif(current_setting(''jc.project_move'', true), '''')::uuid)',
            t, owner);
        EXECUTE format(
            'CREATE POLICY project_move ON %1$I FOR UPDATE TO %2$I
                 USING (session_user <> current_user
                        AND project_id = nullif(current_setting(''jc.project_move'', true), '''')::uuid)
                 WITH CHECK (session_user <> current_user
                        AND project_id = nullif(current_setting(''jc.project_move'', true), '''')::uuid)',
            t, owner);
    END LOOP;
END $$;

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

REVOKE ALL ON FUNCTION project_move(uuid, text, text) FROM PUBLIC;
