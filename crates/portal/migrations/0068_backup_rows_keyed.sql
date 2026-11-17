-- Forced row-level security on the Organization's backup rows (ARCH/STO-02, STO-03, TEN-11;
-- T-7191 PK-1c): the backup target with its sealed secret, the backup's audit and the restore
-- requests hold the key of their Organization's reserved project, and the request role reads
-- and writes them only inside `db::tenant_tx` with that key set. A statement without the key,
-- or with another project's, sees no row and writes none.
--
-- Today's rows get the key of the Organization they name. A row naming an Organization that is
-- not in the registry keeps no key: no request reads it (no Organization of the registry owns
-- it), and it stays in the table for the owner to read; the migration counts such rows in a
-- NOTICE and leaves the columns nullable until none is left.

DO $$
DECLARE
    t text;
    unkeyed bigint;
BEGIN
    FOREACH t IN ARRAY ARRAY['organization_backup_targets', 'organization_backup_log',
                             'organization_data_restores'] LOOP
        EXECUTE format(
            'ALTER TABLE %1$I
                 ADD COLUMN IF NOT EXISTS org_id uuid
                     DEFAULT nullif(current_setting(''jc.org'', true), '''')::uuid,
                 ADD COLUMN IF NOT EXISTS project_id uuid
                     DEFAULT nullif(current_setting(''jc.project'', true), '''')::uuid', t);
        EXECUTE format(
            'UPDATE %1$I r SET org_id = o.key, project_id = p.id
             FROM organizations o JOIN project_ids p ON p.organization = o.name AND p.reserved
             WHERE o.name = r.organization AND r.project_id IS NULL', t);
        EXECUTE format('SELECT count(*) FROM %1$I WHERE org_id IS NULL OR project_id IS NULL', t)
            INTO unkeyed;
        IF unkeyed = 0 THEN
            EXECUTE format('ALTER TABLE %1$I ALTER COLUMN org_id SET NOT NULL,
                                             ALTER COLUMN project_id SET NOT NULL', t);
        ELSE
            RAISE NOTICE '% rows of % name an Organization outside the registry and keep no key',
                unkeyed, t;
        END IF;
        EXECUTE format('CREATE INDEX IF NOT EXISTS %1$I ON %2$I (org_id, project_id)',
                       t || '_by_key', t);
        EXECUTE format('ALTER TABLE %1$I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %1$I FORCE ROW LEVEL SECURITY', t);
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
