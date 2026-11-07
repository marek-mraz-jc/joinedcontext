ALTER TABLE agent_runs ADD COLUMN IF NOT EXISTS kind text NOT NULL DEFAULT 'application';
ALTER TABLE agent_runs ADD COLUMN IF NOT EXISTS unattended boolean NOT NULL DEFAULT false;
ALTER TABLE agent_runs ADD COLUMN IF NOT EXISTS continues text;
