-- The fingerprint of the stream a run ran (T-5825): a Portal that restarts compares each replace
-- pipeline's stream with the one its newest logged run ran, and runs it at once when they differ.
-- Runs logged before this migration keep it NULL: unknown, so they keep their clock.
ALTER TABLE pipeline_runs ADD COLUMN IF NOT EXISTS stream_hash text;
