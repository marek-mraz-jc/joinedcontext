-- The files a kit run wrote, `spec.json` above all, and the specification the preview serves
-- (Architecture/19 §1.2, AP-56, AP-60). A path → content map; empty until the first pass.
ALTER TABLE agent_runs ADD COLUMN IF NOT EXISTS files jsonb NOT NULL DEFAULT '{}'::jsonb;
