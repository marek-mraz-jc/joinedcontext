-- An Organization's own backup target and the audit of every act on it (T-7154, T-7155). One
-- target per Organization and backup kind; the bucket's secret key is age-sealed with the
-- Portal's own key and never read back by any route. The log is who ran, deleted or restored
-- what and when, kept as long as the Portal's database.
CREATE TABLE IF NOT EXISTS organization_backup_targets (
    organization text NOT NULL,
    kind text NOT NULL CHECK (kind IN ('configuration', 'data')),
    endpoint text NOT NULL,
    region text NOT NULL,
    bucket text NOT NULL,
    prefix text NOT NULL DEFAULT '',
    access_key_id text NOT NULL,
    sealed_secret text NOT NULL,
    enabled boolean NOT NULL DEFAULT false,
    every_hours integer NOT NULL CHECK (every_hours BETWEEN 1 AND 672),
    keep_days integer NOT NULL CHECK (keep_days BETWEEN 1 AND 3650),
    updated_by text NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    last_run_at timestamptz,
    PRIMARY KEY (organization, kind)
);

CREATE TABLE IF NOT EXISTS organization_backup_log (
    id bigserial PRIMARY KEY,
    organization text NOT NULL,
    kind text NOT NULL,
    action text NOT NULL CHECK (action IN ('run', 'delete', 'restore-plan', 'restore')),
    actor text NOT NULL,
    at timestamptz NOT NULL DEFAULT now(),
    ok boolean NOT NULL,
    bytes bigint NOT NULL DEFAULT 0,
    object_key text NOT NULL DEFAULT '',
    version_id text NOT NULL DEFAULT '',
    message text NOT NULL DEFAULT ''
);

CREATE INDEX IF NOT EXISTS organization_backup_log_by_organization
    ON organization_backup_log (organization, kind, id DESC);
