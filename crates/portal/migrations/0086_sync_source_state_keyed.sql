-- The tenancy key on `sync_source_state` (ARCH/STO-02, STO-03; review S1-27, T-7281). The table
-- was read on the bare pool by project name and source name, so any statement could read or
-- overwrite another Organization's sync memory. Each row now carries its project's key, copied
-- from the project registry by `namespace` (the project name), and the table answers only inside
-- `db::tenant_tx` of that key.
--
-- A row whose namespace the registry holds no live project of (a removed project, `org`, a name
-- never minted) can never be keyed: it moves whole to `project_rows_set_aside`, which the boot
-- counts in its log. The row is re-derivable: the next run of the source reads it again and
-- re-proposes at worst (org_backup restore leaves this table out for the same reason).

ALTER TABLE sync_source_state
    ADD COLUMN IF NOT EXISTS org_id uuid
        DEFAULT nullif(current_setting('jc.org', true), '')::uuid,
    ADD COLUMN IF NOT EXISTS project_id uuid
        DEFAULT nullif(current_setting('jc.project', true), '')::uuid;

UPDATE sync_source_state s
   SET org_id = o.key, project_id = p.id
  FROM project_ids p
  JOIN organizations o ON o.name = p.organization
 WHERE p.name = s.namespace AND p.parent IS NULL AND NOT p.reserved
   AND (s.org_id IS NULL OR s.project_id IS NULL);

WITH gone AS (
    DELETE FROM sync_source_state s
     WHERE s.org_id IS NULL OR s.project_id IS NULL
    RETURNING to_jsonb(s) AS content, s.namespace AS project
)
INSERT INTO project_rows_set_aside (source_table, project, content)
SELECT 'sync_source_state', project, content FROM gone;

ALTER TABLE sync_source_state
    ALTER COLUMN org_id SET NOT NULL,
    ALTER COLUMN project_id SET NOT NULL;
CREATE INDEX IF NOT EXISTS sync_source_state_by_key ON sync_source_state (org_id, project_id);

DROP POLICY IF EXISTS tenant ON sync_source_state;
CREATE POLICY tenant ON sync_source_state
    USING (org_id = nullif(current_setting('jc.org', true), '')::uuid
           AND project_id = nullif(current_setting('jc.project', true), '')::uuid)
    WITH CHECK (org_id = nullif(current_setting('jc.org', true), '')::uuid
           AND project_id = nullif(current_setting('jc.project', true), '')::uuid);
ALTER TABLE sync_source_state ENABLE ROW LEVEL SECURITY;
ALTER TABLE sync_source_state FORCE ROW LEVEL SECURITY;
