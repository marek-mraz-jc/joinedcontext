-- The tenancy key on the Portal's project tables, phase two (ARCH/STO-02, STO-03, TEN-04, TEN-05;
-- T-7201 PK-2). Phase one (0069-0073, 0076-0078) put nullable org_id and project_id, the
-- `tenant` policy and the per-minute `project_ids::backfill` on these tables. This migration
-- refuses to run while one row still has no key, naming each table and project and the next
-- step, because a forced table would hide that row from every reader. Then the key columns are
-- NOT NULL and row-level security is enabled and forced: a statement outside `db::tenant_tx`
-- reads none of these rows and writes none. There is no default Organization to key a row
-- with (TEN-05): an operator places the project or removes its rows.
--
-- A row whose project the registry never minted an id for (no `project_ids` row: the previous
-- release's backfill found no Project of that name in the mirror, e.g. an MCP call that named
-- `platform`) can never be keyed, and no operator step places it. Such rows are moved, whole and
-- with the rows that would cascade from them, to `project_rows_set_aside` (installation-level,
-- read by an operator only), instead of stopping every start of this release (T-7201, jc-dev
-- 2026-11-18). A project the registry holds keeps refusing: its Organization can still be added.

CREATE TABLE IF NOT EXISTS project_rows_set_aside (
    source_table text NOT NULL,
    project text,
    content jsonb NOT NULL,
    set_aside_at timestamptz NOT NULL DEFAULT now()
);

DO $$
DECLARE
    tables text[] := ARRAY[
        'grafana_visualizations',
        'alert_subscriptions', 'alert_incidents', 'alert_notices', 'health_check',
        'health_check_history', 'space_quality', 'endpoint_usage_daily', 'link_check',
        'app_service_usage', 'email_unsubscribes', 'mqtt_capture', 'entity_trash',
        'entity_comments', 'data_views',
        'pipeline_log', 'pipeline_runs', 'pipeline_rejected', 'pipeline_status',
        'pipeline_templates', 'pipeline_record_hashes', 'pipeline_source_ledger',
        'pipeline_source_holds', 'pipeline_source_complete', 'pipeline_source_removed',
        'erasures', 'access_requests',
        'drafts', 'activity', 'workspaces', 'agent_runs'];
    t text;
    col text;
    unplaceable text;
    child record;
    names text;
    waiting text[] := ARRAY[]::text[];
BEGIN
    -- Children first (alert_notices before alert_subscriptions), so no set-aside row cascades.
    FOR i IN REVERSE cardinality(tables)..1 LOOP
        t := tables[i];
        col := CASE t WHEN 'access_requests' THEN 'endpoint_project' ELSE 'project' END;
        unplaceable := format(
            '(r.org_id IS NULL OR r.project_id IS NULL) AND (r.%1$I IS NULL OR (r.%1$I <> ''org'' '
            'AND NOT EXISTS (SELECT 1 FROM project_ids p WHERE p.name = r.%1$I)))', col);
        -- Rows of other tables that ON DELETE CASCADE would take with a set-aside row.
        FOR child IN
            SELECT c.conrelid::regclass::text AS tbl, a.attname AS fk, pa.attname AS pk
            FROM pg_constraint c
            JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = c.conkey[1]
            JOIN pg_attribute pa ON pa.attrelid = c.confrelid AND pa.attnum = c.confkey[1]
            WHERE c.contype = 'f' AND c.confrelid = t::regclass AND c.confdeltype = 'c'
                AND cardinality(c.conkey) = 1
        LOOP
            EXECUTE format(
                'WITH gone AS (DELETE FROM %1$s ch USING %2$I r WHERE ch.%3$I = r.%4$I AND %5$s '
                'RETURNING to_jsonb(ch) AS content, r.%6$I AS project) '
                'INSERT INTO project_rows_set_aside (source_table, project, content) '
                'SELECT %7$L, project, content FROM gone',
                child.tbl, t, child.fk, child.pk, unplaceable, col, child.tbl);
        END LOOP;
        EXECUTE format(
            'WITH gone AS (DELETE FROM %1$I r WHERE %2$s RETURNING to_jsonb(r) AS content, r.%3$I AS project) '
            'INSERT INTO project_rows_set_aside (source_table, project, content) '
            'SELECT %1$L, project, content FROM gone',
            t, unplaceable, col);
    END LOOP;
    FOREACH t IN ARRAY tables LOOP
        -- The column naming the project whose key a row takes (project_ids::key_column).
        EXECUTE format(
            'SELECT string_agg(DISTINCT coalesce(%2$I, ''(none)''), '', '') FROM %1$I
                 WHERE org_id IS NULL OR project_id IS NULL',
            t, CASE t WHEN 'access_requests' THEN 'endpoint_project' ELSE 'project' END)
            INTO names;
        IF names IS NOT NULL THEN
            waiting := waiting || format('%s (projects %s)', t, names);
        END IF;
    END LOOP;
    IF cardinality(waiting) > 0 THEN
        -- The next step is in the message: the boot log shows the message, not a hint.
        RAISE EXCEPTION 'project rows without a key: %. Run the previous release until its log no '
            'longer says "project rows keep no key": add each project''s Organization on the '
            'Organizations page (the per-minute backfill then keys its rows), or remove the rows of '
            'a project that no longer exists; then start this release again',
            array_to_string(waiting, '; ')
            USING ERRCODE = 'check_violation';
    END IF;
    FOREACH t IN ARRAY tables LOOP
        EXECUTE format(
            'ALTER TABLE %1$I ALTER COLUMN org_id SET NOT NULL, ALTER COLUMN project_id SET NOT NULL', t);
        EXECUTE format('DROP INDEX IF EXISTS %1$I', t || '_unkeyed');
        EXECUTE format('ALTER TABLE %1$I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %1$I FORCE ROW LEVEL SECURITY', t);
    END LOOP;
END
$$;
