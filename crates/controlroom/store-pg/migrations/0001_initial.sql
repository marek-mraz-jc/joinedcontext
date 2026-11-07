-- controlroom PostgreSQL schema with mandatory Row-Level Security (R8, §5.3).
-- Roles: migrations run as `controlroom_migrate`; application queries run as `controlroom_app` (NOBYPASSRLS).

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'controlroom_migrate') THEN
        CREATE ROLE controlroom_migrate NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'controlroom_app') THEN
        CREATE ROLE controlroom_app NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS;
    END IF;
END
$$;

CREATE TABLE IF NOT EXISTS cr_quota (
    installation_id UUID NOT NULL,
    org_id UUID NOT NULL PRIMARY KEY,
    instance_id UUID NOT NULL,
    max_dashboards BIGINT NOT NULL DEFAULT 500,
    max_panels_per_dashboard BIGINT NOT NULL DEFAULT 250,
    max_datasources BIGINT NOT NULL DEFAULT 100,
    max_wasm_plugins BIGINT NOT NULL DEFAULT 32,
    max_queries_per_minute BIGINT NOT NULL DEFAULT 1200,
    max_concurrent_queries BIGINT NOT NULL DEFAULT 64,
    max_cache_bytes BIGINT NOT NULL DEFAULT 67108864,
    max_alert_rules BIGINT NOT NULL DEFAULT 200,
    max_evaluations_per_minute BIGINT NOT NULL DEFAULT 600,
    max_public_range_seconds BIGINT NOT NULL DEFAULT 2678400,
    min_public_interval_ms BIGINT NOT NULL DEFAULT 15000,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS cr_folder (
    installation_id UUID NOT NULL,
    org_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    project_id TEXT,
    uid TEXT NOT NULL,
    parent_uid TEXT,
    title TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    managed_by TEXT,
    created_by TEXT NOT NULL DEFAULT 'anonymous',
    updated_by TEXT NOT NULL DEFAULT 'anonymous',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (org_id, uid)
);
CREATE INDEX IF NOT EXISTS idx_cr_folder_org ON cr_folder (org_id);

CREATE TABLE IF NOT EXISTS cr_dashboard (
    installation_id UUID NOT NULL,
    org_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    project_id TEXT,
    uid TEXT NOT NULL,
    folder_uid TEXT,
    title TEXT NOT NULL,
    slug TEXT NOT NULL,
    tags JSONB NOT NULL DEFAULT '[]'::jsonb,
    panel_count INTEGER NOT NULL DEFAULT 0,
    datasource_uids JSONB NOT NULL DEFAULT '[]'::jsonb,
    version INTEGER NOT NULL DEFAULT 1,
    schema_version SMALLINT NOT NULL DEFAULT 42,
    is_provisioned BOOLEAN NOT NULL DEFAULT FALSE,
    provisioned_external_id TEXT,
    allow_ui_updates BOOLEAN NOT NULL DEFAULT TRUE,
    model JSONB NOT NULL,
    created_by TEXT NOT NULL DEFAULT 'anonymous',
    updated_by TEXT NOT NULL DEFAULT 'anonymous',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (org_id, uid)
);
CREATE INDEX IF NOT EXISTS idx_cr_dashboard_org ON cr_dashboard (org_id);
CREATE INDEX IF NOT EXISTS idx_cr_dashboard_folder ON cr_dashboard (org_id, folder_uid);

CREATE TABLE IF NOT EXISTS cr_dashboard_version (
    installation_id UUID NOT NULL,
    org_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    dashboard_uid TEXT NOT NULL,
    version INTEGER NOT NULL,
    parent_version INTEGER NOT NULL DEFAULT 0,
    restored_from INTEGER NOT NULL DEFAULT 0,
    message TEXT NOT NULL DEFAULT '',
    model JSONB NOT NULL,
    created_by TEXT NOT NULL DEFAULT 'anonymous',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (org_id, dashboard_uid, version)
);
CREATE INDEX IF NOT EXISTS idx_cr_dashboard_version_org ON cr_dashboard_version (org_id, dashboard_uid);

CREATE TABLE IF NOT EXISTS cr_datasource (
    installation_id UUID NOT NULL,
    org_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    project_id TEXT,
    uid TEXT NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    access TEXT NOT NULL DEFAULT 'proxy',
    url TEXT NOT NULL DEFAULT '',
    is_default BOOLEAN NOT NULL DEFAULT FALSE,
    read_only BOOLEAN NOT NULL DEFAULT FALSE,
    json_data JSONB NOT NULL DEFAULT '{}'::jsonb,
    secret_refs JSONB NOT NULL DEFAULT '{}'::jsonb,
    allow_hosts JSONB NOT NULL DEFAULT '[]'::jsonb,
    wasm_digest TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (org_id, uid),
    UNIQUE (org_id, name)
);
CREATE INDEX IF NOT EXISTS idx_cr_datasource_org ON cr_datasource (org_id);

CREATE TABLE IF NOT EXISTS cr_library_panel (
    installation_id UUID NOT NULL,
    org_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    folder_uid TEXT,
    uid TEXT NOT NULL,
    name TEXT NOT NULL,
    panel_type TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    model JSONB NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (org_id, uid)
);
CREATE INDEX IF NOT EXISTS idx_cr_library_panel_org ON cr_library_panel (org_id);

CREATE TABLE IF NOT EXISTS cr_public_dashboard (
    installation_id UUID NOT NULL,
    org_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    uid TEXT NOT NULL,
    dashboard_uid TEXT NOT NULL,
    access_token_hash TEXT NOT NULL UNIQUE,
    is_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    time_selection_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    annotations_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    show_query_text BOOLEAN NOT NULL DEFAULT FALSE,
    share TEXT NOT NULL DEFAULT 'public',
    created_by TEXT NOT NULL DEFAULT 'anonymous',
    updated_by TEXT NOT NULL DEFAULT 'anonymous',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (org_id, uid),
    UNIQUE (org_id, dashboard_uid)
);
CREATE INDEX IF NOT EXISTS idx_cr_public_dashboard_org ON cr_public_dashboard (org_id);

CREATE TABLE IF NOT EXISTS cr_alert_rule (
    installation_id UUID NOT NULL,
    org_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    uid TEXT NOT NULL,
    folder_uid TEXT,
    title TEXT NOT NULL,
    spec JSONB NOT NULL DEFAULT '{}'::jsonb,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (org_id, uid)
);
CREATE INDEX IF NOT EXISTS idx_cr_alert_rule_org ON cr_alert_rule (org_id);

CREATE TABLE IF NOT EXISTS cr_alert_state (
    installation_id UUID NOT NULL,
    org_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    rule_uid TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'Normal',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (org_id, rule_uid)
);
CREATE INDEX IF NOT EXISTS idx_cr_alert_state_org ON cr_alert_state (org_id);

CREATE TABLE IF NOT EXISTS cr_query_cache (
    installation_id UUID NOT NULL,
    org_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    id TEXT NOT NULL,
    datasource_uid TEXT NOT NULL,
    hits BIGINT NOT NULL DEFAULT 0,
    misses BIGINT NOT NULL DEFAULT 0,
    entries BIGINT NOT NULL DEFAULT 0,
    bytes_used BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (org_id, id)
);
CREATE INDEX IF NOT EXISTS idx_cr_query_cache_org ON cr_query_cache (org_id);

CREATE TABLE IF NOT EXISTS cr_import_report (
    installation_id UUID NOT NULL,
    org_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    id TEXT NOT NULL,
    dry_run BOOLEAN NOT NULL DEFAULT FALSE,
    status TEXT NOT NULL,
    dashboard_count INTEGER NOT NULL DEFAULT 0,
    refused_count INTEGER NOT NULL DEFAULT 0,
    mapped_count INTEGER NOT NULL DEFAULT 0,
    summary JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (org_id, id)
);
CREATE INDEX IF NOT EXISTS idx_cr_import_report_org ON cr_import_report (org_id);

-- Enforce Row-Level Security (FORCE ROW LEVEL SECURITY) on every table (§5.3, R8).
-- Predicate uses btree-indexed `org_id = current_setting('cr.org', true)::uuid` (leakproof equality).
DO $$
DECLARE
    tbl TEXT;
BEGIN
    FOREACH tbl IN ARRAY ARRAY[
        'cr_quota',
        'cr_folder',
        'cr_dashboard',
        'cr_dashboard_version',
        'cr_datasource',
        'cr_library_panel',
        'cr_public_dashboard',
        'cr_alert_rule',
        'cr_alert_state',
        'cr_query_cache',
        'cr_import_report'
    ] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', tbl);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', tbl);
        EXECUTE format('DROP POLICY IF EXISTS org_isolation ON %I', tbl);
        EXECUTE format(
            'CREATE POLICY org_isolation ON %I USING (org_id = NULLIF(current_setting(''cr.org'', true), '''')::uuid) WITH CHECK (org_id = NULLIF(current_setting(''cr.org'', true), '''')::uuid)',
            tbl
        );
        EXECUTE format('GRANT SELECT, INSERT, UPDATE, DELETE ON %I TO controlroom_app', tbl);
    END LOOP;
END
$$;
