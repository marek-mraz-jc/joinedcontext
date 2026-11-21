-- One lock for every statement that rewrites `pg_class` rows of the public tables (T-7476):
-- `project_move` (0089) lifts and restores FORCE ROW LEVEL SECURITY per table, and the Portal's
-- serving-role grants (db.rs `grant_serving_role`, at every start) run GRANT ... ON ALL TABLES.
-- Both update the same catalog rows, and two sessions doing so at once fail with XX000 "tuple
-- concurrently updated". Each now takes the transaction-level advisory lock on the constant key
-- `hashtextextended('jc/pg_class', 0)` first, so they run one after the other. The key is a
-- constant, never caller input; db.rs names the same text (`CATALOG_LOCK`). The rest of
-- `project_move` is 0089's: FORCE is lifted and restored inside the caller's one transaction,
-- under the table's ACCESS EXCLUSIVE lock, so no other transaction sees a table unforced.

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
    PERFORM pg_advisory_xact_lock(hashtextextended('jc/pg_class', 0));
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
