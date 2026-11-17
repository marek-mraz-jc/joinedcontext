-- The tenancy key on the erasure log and the access requests, phase one (ARCH/STO-02, STO-03,
-- TEN-04; T-7202 PK-1i), as 0071: nullable org_id and project_id defaulting to the key
-- `db::tenant_tx` sets, the `tenant` policy in place, row-level security off until
-- `project_ids::backfill` has keyed today's rows. An erasure is keyed by its project; an access
-- request by the project whose Endpoint it asks for (`endpoint_project`), whose owners decide it.

DO $$
DECLARE
    t text;
    by_name text;
BEGIN
    FOREACH t IN ARRAY ARRAY['erasures', 'access_requests'] LOOP
        by_name := CASE t WHEN 'access_requests' THEN 'endpoint_project' ELSE 'project' END;
        EXECUTE format(
            'ALTER TABLE %1$I
                 ADD COLUMN IF NOT EXISTS org_id uuid
                     DEFAULT nullif(current_setting(''jc.org'', true), '''')::uuid,
                 ADD COLUMN IF NOT EXISTS project_id uuid
                     DEFAULT nullif(current_setting(''jc.project'', true), '''')::uuid', t);
        EXECUTE format('CREATE INDEX IF NOT EXISTS %1$I ON %2$I (org_id, project_id)',
                       t || '_by_key', t);
        EXECUTE format('CREATE INDEX IF NOT EXISTS %1$I ON %2$I (%3$I) WHERE project_id IS NULL',
                       t || '_unkeyed', t, by_name);
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
