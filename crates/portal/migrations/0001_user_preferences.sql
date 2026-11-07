-- One row per person (Keycloak `sub`, CC-40). The preferences are one JSON document: the UI owns
-- its shape, the Portal only checks the few fields it understands (API/01 section 8).
CREATE TABLE IF NOT EXISTS user_preferences (
    subject     text        PRIMARY KEY,
    preferences jsonb       NOT NULL,
    updated_at  timestamptz NOT NULL DEFAULT now()
);
