-- Agent alerts as notices (T-5301, AG-136): a person subscribes to an agent profile (scope
-- `agent`) and is told of its refusal spikes, runs near the token cap and writes outside office
-- hours. The incident's `pipeline` column holds the profile's name for these events.
ALTER TABLE alert_subscriptions DROP CONSTRAINT IF EXISTS alert_subscriptions_scope_check;
ALTER TABLE alert_subscriptions ADD CONSTRAINT alert_subscriptions_scope_check
  CHECK (scope IN ('pipeline', 'space', 'type', 'agent'));

ALTER TABLE alert_incidents DROP CONSTRAINT IF EXISTS alert_incidents_event_check;
ALTER TABLE alert_incidents ADD CONSTRAINT alert_incidents_event_check
  CHECK (event IN ('failure', 'stale', 'zero', 'invalid', 'refusalSpike', 'spendShare', 'offHoursWrite'));
