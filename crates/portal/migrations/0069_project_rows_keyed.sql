-- The tenancy key on the Portal's project tables, phase one (ARCH/STO-02, STO-03, TEN-04;
-- T-7196 PK-1d). A project's Organization lives only in Git, so SQL cannot key today's rows:
-- org_id and project_id stay nullable and default to the key `db::tenant_tx` sets, so every row
-- written inside one carries it, and `project_ids::backfill` keys the older rows once the mirror
-- holds a sync. The policy is in place and row-level security stays off: forcing it before the
-- backfill would hide today's rows. The forcing migration follows once no row is unkeyed.

DO $$
DECLARE
    t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['grafana_visualizations'] LOOP
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
