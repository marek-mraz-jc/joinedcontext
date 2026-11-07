-- The capture of `capture` MqttBrokers (ADR-N-048 §2.10, T-4880): the newest messages a broker
-- received, for a steward to design a mapping before a model exists. Each row lives until its
-- window passes (at most 24 hours); the Portal trims a broker to its messagesPerTopic, 1000 rows
-- and 8 MiB on every write, and deletes a broker's rows when it leaves capture or is deleted.
-- The payload is a device's message and may hold personal data: it is read by stewards only and
-- never copied into a log, an activity entry or a metric.
CREATE TABLE IF NOT EXISTS mqtt_capture (
  id          bigserial   PRIMARY KEY,
  project     text        NOT NULL,
  broker      text        NOT NULL,
  topic       text        NOT NULL,
  client_id   text,
  received_at timestamptz NOT NULL,
  expires_at  timestamptz NOT NULL,
  bytes       integer     NOT NULL CHECK (bytes >= 0),
  payload     bytea       NOT NULL
);
CREATE INDEX IF NOT EXISTS mqtt_capture_broker ON mqtt_capture (project, broker, id DESC);
CREATE INDEX IF NOT EXISTS mqtt_capture_topic ON mqtt_capture (project, broker, topic, id DESC);
CREATE INDEX IF NOT EXISTS mqtt_capture_expiry ON mqtt_capture (expires_at);
