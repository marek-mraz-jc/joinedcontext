-- Who started the run, so a call made on the run's behalf is judged as that person and never as
-- more (AG-70, AG-64): the identity the Portal would have used had the person made the call
-- themselves. Kept out of every API answer; a run from before this column carries `null` and
-- reaches the registry as nobody.
ALTER TABLE agent_runs ADD COLUMN IF NOT EXISTS starter jsonb NOT NULL DEFAULT 'null'::jsonb;
