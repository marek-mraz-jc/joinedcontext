-- One audit of MCP calls (MCP-16, AG-147, T-7542): a call of the Portal MCP is a row without a
-- connector, under the same forced key. A call is two rows sharing `call_id`, `started` before it
-- runs and its outcome after, so nothing updates a row; a row kept before this has no `call_id`.
ALTER TABLE connector_calls
    ALTER COLUMN connector DROP NOT NULL,
    ADD COLUMN IF NOT EXISTS call_id text;
CREATE INDEX IF NOT EXISTS connector_calls_call ON connector_calls (call_id, verdict)
    WHERE call_id IS NOT NULL;
-- What the retention job deletes by (AUD-04).
CREATE INDEX IF NOT EXISTS connector_calls_kept ON connector_calls (org_id, project_id, at);
