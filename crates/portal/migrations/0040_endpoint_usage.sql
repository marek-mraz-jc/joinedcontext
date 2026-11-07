-- Usage per Endpoint for the publishers' Usage tab (T-5118): calls per project, Endpoint, surface
-- and UTC day, as the gateway replicas post their counts. Names and numbers only: no address,
-- token, person or query has a column here. Kept 400 days.
CREATE TABLE IF NOT EXISTS endpoint_usage_daily (
  project  text   NOT NULL,
  endpoint text   NOT NULL,
  surface  text   NOT NULL CHECK (surface IN ('ngsi-ld', 'ogc', 'odata', 'sta', 'ical', 'mcp', 'websocket', 'download')),
  day      date   NOT NULL,
  calls    bigint NOT NULL CHECK (calls >= 0),
  PRIMARY KEY (project, endpoint, day, surface)
);
CREATE INDEX IF NOT EXISTS endpoint_usage_daily_day ON endpoint_usage_daily (day);
