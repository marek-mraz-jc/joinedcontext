-- Notifications beyond mentions (T-5979): a change opened through a connector, a connector request
-- waiting above the ceiling, a connector about to expire. Such a row names people and objects and
-- carries a Portal link; it holds no manifest content. `event_key` makes one event one row per
-- recipient. A row addressed to an audience (`@approve:{project}`, `@connector-admin`) is copied to
-- each person when they read their notifications and still hold the right it needs.
ALTER TABLE notifications ALTER COLUMN comment_id DROP NOT NULL;
ALTER TABLE notifications ADD COLUMN IF NOT EXISTS kind text NOT NULL DEFAULT 'mention';
ALTER TABLE notifications ADD COLUMN IF NOT EXISTS link text;
ALTER TABLE notifications ADD COLUMN IF NOT EXISTS event_key text;
CREATE UNIQUE INDEX IF NOT EXISTS notifications_event ON notifications (recipient, event_key);
