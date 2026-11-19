-- Forces the key 0087 added to a person's own rows (T-7282): `connectors`, `connector_calls` and
-- `user_preferences` answer only inside `db::tenant_tx` of an Organization's reserved project.
--
-- A row still unkeyed here was written before 0087 on a Portal started without
-- JC_PORTAL_INSTALLATION_ORGANIZATION: no source names its Organization, and setting it aside
-- would revoke people's live connector grants, so the migration refuses and says the next step.

DO $$
DECLARE
    unkeyed bigint;
BEGIN
    SELECT (SELECT count(*) FROM connectors WHERE org_id IS NULL OR project_id IS NULL)
         + (SELECT count(*) FROM connector_calls WHERE org_id IS NULL OR project_id IS NULL)
         + (SELECT count(*) FROM user_preferences WHERE org_id IS NULL OR project_id IS NULL)
      INTO unkeyed;
    IF unkeyed > 0 THEN
        RAISE EXCEPTION '% rows of connectors, connector_calls and user_preferences have no Organization key: set JC_PORTAL_INSTALLATION_ORGANIZATION to the [[organizations]] entry the deployment host''s people use the Portal in, and start again; the Portal keys them under it before this migration', unkeyed;
    END IF;
END $$;

ALTER TABLE connectors
    ALTER COLUMN org_id SET NOT NULL,
    ALTER COLUMN project_id SET NOT NULL;
ALTER TABLE connector_calls
    ALTER COLUMN org_id SET NOT NULL,
    ALTER COLUMN project_id SET NOT NULL;
ALTER TABLE user_preferences
    ALTER COLUMN org_id SET NOT NULL,
    ALTER COLUMN project_id SET NOT NULL;

-- One person on two Organization hosts keeps a document and connector names in each: an upsert
-- or a name check never meets a row the key hides.
ALTER TABLE user_preferences DROP CONSTRAINT IF EXISTS user_preferences_pkey;
ALTER TABLE user_preferences ADD PRIMARY KEY (org_id, subject);
DROP INDEX IF EXISTS connectors_owner_name;
CREATE UNIQUE INDEX IF NOT EXISTS connectors_owner_name
    ON connectors (org_id, owner, name) WHERE revoked_at IS NULL;
CREATE INDEX IF NOT EXISTS connectors_by_key ON connectors (org_id, project_id);
CREATE INDEX IF NOT EXISTS connector_calls_by_key ON connector_calls (org_id, project_id);

DO $$
DECLARE
    t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['connectors', 'connector_calls', 'user_preferences'] LOOP
        EXECUTE format('DROP POLICY IF EXISTS tenant ON %I', t);
        EXECUTE format(
            'CREATE POLICY tenant ON %1$I
                 USING (org_id = nullif(current_setting(''jc.org'', true), '''')::uuid
                        AND project_id = nullif(current_setting(''jc.project'', true), '''')::uuid)
                 WITH CHECK (org_id = nullif(current_setting(''jc.org'', true), '''')::uuid
                        AND project_id = nullif(current_setting(''jc.project'', true), '''')::uuid)',
            t);
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
    END LOOP;
END $$;
