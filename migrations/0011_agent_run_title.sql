-- The name a person reads for an application run, chosen by the model from the request and the
-- data ("Helsinki Traffic Alerts Map"); app_name stays the id. NULL until the first version.
ALTER TABLE agent_runs ADD COLUMN IF NOT EXISTS title text;
