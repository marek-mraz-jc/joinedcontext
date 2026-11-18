-- The tenancy key on `agent_run_events` (ARCH/STO-02, STO-03, ISO-09; review S1-27, T-7278). The
-- events were read on the bare pool by run id alone, so any caller holding a run id read another
-- Organization's run stream. Each event now carries its run's key, copied from `agent_runs`
-- (keyed and forced since 0080), and the table answers only inside `db::tenant_tx` of that key.
-- Every event has its run (the foreign key cascades), so every row is keyed and none is set aside.

ALTER TABLE agent_run_events
    ADD COLUMN IF NOT EXISTS org_id uuid
        DEFAULT nullif(current_setting('jc.org', true), '')::uuid,
    ADD COLUMN IF NOT EXISTS project_id uuid
        DEFAULT nullif(current_setting('jc.project', true), '')::uuid;

-- agent_runs is forced since 0080, which holds for its owner too: the copy reads it with forcing
-- off for this transaction alone, and the forcing is back before the transaction commits.
ALTER TABLE agent_runs NO FORCE ROW LEVEL SECURITY;
UPDATE agent_run_events e
   SET org_id = r.org_id, project_id = r.project_id
  FROM agent_runs r
 WHERE r.id = e.run_id AND (e.org_id IS NULL OR e.project_id IS NULL);
ALTER TABLE agent_runs FORCE ROW LEVEL SECURITY;

ALTER TABLE agent_run_events
    ALTER COLUMN org_id SET NOT NULL,
    ALTER COLUMN project_id SET NOT NULL;
CREATE INDEX IF NOT EXISTS agent_run_events_by_key ON agent_run_events (org_id, project_id);

DROP POLICY IF EXISTS tenant ON agent_run_events;
CREATE POLICY tenant ON agent_run_events
    USING (org_id = nullif(current_setting('jc.org', true), '')::uuid
           AND project_id = nullif(current_setting('jc.project', true), '')::uuid)
    WITH CHECK (org_id = nullif(current_setting('jc.org', true), '')::uuid
           AND project_id = nullif(current_setting('jc.project', true), '')::uuid);
ALTER TABLE agent_run_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent_run_events FORCE ROW LEVEL SECURITY;
