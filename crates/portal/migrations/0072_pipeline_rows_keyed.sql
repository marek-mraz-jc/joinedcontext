-- The tenancy key on the pipeline tables, phase one (ARCH/STO-02, STO-03, TEN-04; T-7198 PK-1f),
-- as 0069: nullable org_id and project_id defaulting to the key `db::tenant_tx` sets, the
-- `tenant` policy in place, row-level security off until `project_ids::backfill` has keyed
-- today's rows.

DO $$
DECLARE
    t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['pipeline_log', 'pipeline_runs', 'pipeline_rejected', 'pipeline_status',
                                 'pipeline_templates', 'pipeline_record_hashes', 'pipeline_source_ledger',
                                 'pipeline_source_holds', 'pipeline_source_complete',
                                 'pipeline_source_removed'] LOOP
        EXECUTE format(
            'ALTER TABLE %1$I
                 ADD COLUMN IF NOT EXISTS org_id uuid
                     DEFAULT nullif(current_setting(''jc.org'', true), '''')::uuid,
                 ADD COLUMN IF NOT EXISTS project_id uuid
                     DEFAULT nullif(current_setting(''jc.project'', true), '''')::uuid', t);
        EXECUTE format('CREATE INDEX IF NOT EXISTS %1$I ON %2$I (org_id, project_id)',
                       t || '_by_key', t);
        EXECUTE format('CREATE INDEX IF NOT EXISTS %1$I ON %2$I (project) WHERE project_id IS NULL',
                       t || '_unkeyed', t);
        EXECUTE format('DROP POLICY IF EXISTS tenant ON %1$I', t);
        EXECUTE format(
            'CREATE POLICY tenant ON %1$I
                 USING (org_id = nullif(current_setting(''jc.org'', true), '''')::uuid
                        AND project_id = nullif(current_setting(''jc.project'', true), '''')::uuid)
                 WITH CHECK (org_id = nullif(current_setting(''jc.org'', true), '''')::uuid
                        AND project_id = nullif(current_setting(''jc.project'', true), '''')::uuid)',
            t);
    END LOOP;
END
$$;
