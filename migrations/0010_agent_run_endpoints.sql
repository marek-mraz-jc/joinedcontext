-- Every endpoint an application run reads, the primary first: [{name, slug, space}] (AP-44).
-- A run from before this column reads its one endpoint from endpoint_name and endpoint_slug.
ALTER TABLE agent_runs ADD COLUMN IF NOT EXISTS endpoints jsonb NOT NULL DEFAULT '[]'::jsonb;
