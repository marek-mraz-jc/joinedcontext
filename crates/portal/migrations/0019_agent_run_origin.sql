-- Who a run is for (AG-93, T-2816): `person`, or `journey` for a run the Portal's own live
-- journeys started. Lists leave journey runs out by default; nothing is deleted, so every run
-- recorded before this column is a person's.
ALTER TABLE agent_runs ADD COLUMN IF NOT EXISTS origin text NOT NULL DEFAULT 'person';
