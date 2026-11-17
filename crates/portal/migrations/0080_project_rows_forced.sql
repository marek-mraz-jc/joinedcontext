-- The tenancy key on the Portal's project tables, phase two (ARCH/STO-02, STO-03, TEN-04, TEN-05;
-- T-7201 PK-2). Phase one (0069-0073, 0076-0078) put nullable org_id and project_id, the
-- `tenant` policy and the per-minute `project_ids::backfill` on these tables. This migration
-- refuses to run while one row still has no key, naming each table and project and the next
-- step, because a forced table would hide that row from every reader. Then the key columns are
-- NOT NULL and row-level security is enabled and forced: a statement outside `db::tenant_tx`
-- reads none of these rows and writes none. There is no default Organization to key a row
-- with (TEN-05): an operator places the project or removes its rows.

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
    names text;
    waiting text[] := ARRAY[]::text[];
BEGIN
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
