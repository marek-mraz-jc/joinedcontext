-- S2: shako PostgreSQL schema, roles (`shako_migrate`, `shako_app`), forced RLS on every table,
-- and leakproof btree index on `(instance_id, owner, name)`.
-- No users, teams, collaborators, access tokens, sessions, or S16 Actions tables in this run.

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'shako_migrate') THEN
        CREATE ROLE shako_migrate NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'shako_app') THEN
        CREATE ROLE shako_app NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS;
    END IF;
END
$$;

CREATE SCHEMA IF NOT EXISTS shako;
GRANT USAGE ON SCHEMA shako TO shako_app;

CREATE TABLE IF NOT EXISTS shako.instance (
    instance_id UUID PRIMARY KEY,
    org_id UUID NOT NULL,
    host TEXT NOT NULL COLLATE "C",
    path_prefix TEXT NOT NULL DEFAULT '' COLLATE "C",
    max_repos BIGINT NOT NULL DEFAULT 200,
    max_storage_bytes BIGINT NOT NULL DEFAULT 5368709120,
    max_push_bytes BIGINT NOT NULL DEFAULT 536870912,
    secret_scan_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS shako.repo (
    id UUID PRIMARY KEY,
    instance_id UUID NOT NULL,
    org_id UUID NOT NULL,
    project_id TEXT NOT NULL DEFAULT '' COLLATE "C",
    owner TEXT NOT NULL COLLATE "C",
    name TEXT NOT NULL COLLATE "C",
    description TEXT NOT NULL DEFAULT '',
    default_branch TEXT NOT NULL DEFAULT 'main' COLLATE "C",
    private BOOLEAN NOT NULL DEFAULT TRUE,
    archived BOOLEAN NOT NULL DEFAULT FALSE,
    size_bytes BIGINT NOT NULL DEFAULT 0,
    next_pr_number BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- Leakproof btree index for `(instance_id, owner, name)` under forced RLS (S2)
CREATE UNIQUE INDEX IF NOT EXISTS idx_shako_repo_instance_owner_name
    ON shako.repo (instance_id, owner, name);
CREATE INDEX IF NOT EXISTS idx_shako_repo_org ON shako.repo (instance_id, org_id);

CREATE TABLE IF NOT EXISTS shako.protection_rule (
    id UUID PRIMARY KEY,
    instance_id UUID NOT NULL,
    org_id UUID NOT NULL,
    repo_id UUID NOT NULL,
    kind TEXT NOT NULL DEFAULT 'branch' COLLATE "C",
    rule_name TEXT NOT NULL COLLATE "C",
    doc JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (instance_id, repo_id, kind, rule_name)
);

CREATE TABLE IF NOT EXISTS shako.pull_request (
    id UUID PRIMARY KEY,
    instance_id UUID NOT NULL,
    org_id UUID NOT NULL,
    repo_id UUID NOT NULL,
    number BIGINT NOT NULL,
    state TEXT NOT NULL COLLATE "C",
    title TEXT NOT NULL,
    body TEXT NOT NULL DEFAULT '',
    head_ref TEXT NOT NULL COLLATE "C",
    head_sha TEXT NOT NULL COLLATE "C",
    base_ref TEXT NOT NULL COLLATE "C",
    base_sha TEXT NOT NULL COLLATE "C",
    merge_base TEXT NOT NULL DEFAULT '' COLLATE "C",
    mergeable BOOLEAN,
    merged BOOLEAN NOT NULL DEFAULT FALSE,
    merge_commit_sha TEXT NOT NULL DEFAULT '' COLLATE "C",
    author_login TEXT NOT NULL COLLATE "C",
    author_name TEXT NOT NULL DEFAULT '',
    author_email TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    closed_at TIMESTAMPTZ,
    UNIQUE (instance_id, repo_id, number)
);
CREATE INDEX IF NOT EXISTS idx_shako_pr_repo_state ON shako.pull_request (instance_id, org_id, repo_id, state);

CREATE TABLE IF NOT EXISTS shako.review (
    id UUID PRIMARY KEY,
    instance_id UUID NOT NULL,
    org_id UUID NOT NULL,
    repo_id UUID NOT NULL,
    pr_number BIGINT NOT NULL,
    verdict TEXT NOT NULL COLLATE "C",
    body TEXT NOT NULL DEFAULT '',
    author_login TEXT NOT NULL COLLATE "C",
    commit_id TEXT NOT NULL DEFAULT '' COLLATE "C",
    dismissed BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_shako_review_pr ON shako.review (instance_id, org_id, repo_id, pr_number);

CREATE TABLE IF NOT EXISTS shako.commit_status (
    id UUID PRIMARY KEY,
    instance_id UUID NOT NULL,
    org_id UUID NOT NULL,
    repo_id UUID NOT NULL,
    sha TEXT NOT NULL COLLATE "C",
    context TEXT NOT NULL COLLATE "C",
    state TEXT NOT NULL COLLATE "C",
    description TEXT NOT NULL DEFAULT '',
    target_url TEXT NOT NULL DEFAULT '',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (instance_id, repo_id, sha, context)
);

CREATE TABLE IF NOT EXISTS shako.webhook (
    id UUID PRIMARY KEY,
    instance_id UUID NOT NULL,
    org_id UUID NOT NULL,
    repo_id UUID NOT NULL,
    url TEXT NOT NULL,
    events JSONB NOT NULL DEFAULT '["push","pull_request"]'::jsonb,
    active BOOLEAN NOT NULL DEFAULT TRUE,
    secret_ref TEXT NOT NULL COLLATE "C",
    previous_secret_ref TEXT COLLATE "C",
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS shako.delivery (
    id UUID PRIMARY KEY,
    instance_id UUID NOT NULL,
    org_id UUID NOT NULL,
    webhook_id UUID NOT NULL,
    event TEXT NOT NULL COLLATE "C",
    status TEXT NOT NULL COLLATE "C",
    response_code INTEGER,
    delivered_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS shako.push_mirror (
    id UUID PRIMARY KEY,
    instance_id UUID NOT NULL,
    org_id UUID NOT NULL,
    repo_id UUID NOT NULL,
    remote_name TEXT NOT NULL COLLATE "C",
    remote_address TEXT NOT NULL,
    credential_secret_ref TEXT NOT NULL COLLATE "C",
    interval_str TEXT NOT NULL DEFAULT '8h0m0s',
    sync_on_commit BOOLEAN NOT NULL DEFAULT TRUE,
    last_sync TIMESTAMPTZ,
    last_error TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (instance_id, repo_id, remote_name)
);

CREATE TABLE IF NOT EXISTS shako.package (
    id UUID PRIMARY KEY,
    instance_id UUID NOT NULL,
    org_id UUID NOT NULL,
    owner TEXT NOT NULL COLLATE "C",
    package_type TEXT NOT NULL COLLATE "C",
    name TEXT NOT NULL COLLATE "C",
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (instance_id, owner, package_type, name)
);

CREATE TABLE IF NOT EXISTS shako.package_version (
    id UUID PRIMARY KEY,
    instance_id UUID NOT NULL,
    org_id UUID NOT NULL,
    package_id UUID NOT NULL,
    owner TEXT NOT NULL COLLATE "C",
    package_type TEXT NOT NULL COLLATE "C",
    package_name TEXT NOT NULL COLLATE "C",
    version TEXT NOT NULL COLLATE "C",
    digest TEXT NOT NULL COLLATE "C",
    media_type TEXT NOT NULL DEFAULT '',
    manifest_bytes BYTEA,
    subject_digest TEXT COLLATE "C",
    artifact_type TEXT COLLATE "C",
    size_bytes BIGINT NOT NULL DEFAULT 0,
    signed BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (instance_id, package_id, version)
);

CREATE TABLE IF NOT EXISTS shako.package_file (
    id UUID PRIMARY KEY,
    instance_id UUID NOT NULL,
    org_id UUID NOT NULL,
    version_id UUID NOT NULL,
    owner TEXT NOT NULL COLLATE "C",
    package_name TEXT NOT NULL COLLATE "C",
    version TEXT NOT NULL COLLATE "C",
    filename TEXT NOT NULL COLLATE "C",
    sha256 TEXT NOT NULL COLLATE "C",
    size_bytes BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (instance_id, version_id, filename)
);

CREATE TABLE IF NOT EXISTS shako.blob (
    id UUID PRIMARY KEY,
    instance_id UUID NOT NULL,
    org_id UUID NOT NULL,
    owner TEXT NOT NULL COLLATE "C",
    repo_name TEXT NOT NULL COLLATE "C",
    digest TEXT NOT NULL COLLATE "C",
    size_bytes BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (instance_id, owner, repo_name, digest)
);

CREATE TABLE IF NOT EXISTS shako.quota_usage (
    instance_id UUID NOT NULL,
    org_id UUID NOT NULL,
    repo_count BIGINT NOT NULL DEFAULT 0,
    git_bytes BIGINT NOT NULL DEFAULT 0,
    package_bytes BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (instance_id, org_id)
);

CREATE TABLE IF NOT EXISTS shako.outbox (
    id UUID PRIMARY KEY,
    instance_id UUID NOT NULL,
    org_id UUID NOT NULL,
    repo_id UUID,
    event_kind TEXT NOT NULL COLLATE "C",
    idempotency_key TEXT NOT NULL COLLATE "C",
    payload JSONB NOT NULL,
    dispatched BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    dispatched_at TIMESTAMPTZ,
    UNIQUE (instance_id, idempotency_key)
);

-- Enable and FORCE Row-Level Security on every table in `shako` using leakproof UUID equality
-- against `current_setting('shako.instance', true)` and `current_setting('shako.org', true)`.
DO $$
DECLARE
    tbl TEXT;
BEGIN
    FOREACH tbl IN ARRAY ARRAY[
        'instance',
        'repo',
        'protection_rule',
        'pull_request',
        'review',
        'commit_status',
        'webhook',
        'delivery',
        'push_mirror',
        'package',
        'package_version',
        'package_file',
        'blob',
        'quota_usage',
        'outbox'
    ] LOOP
        EXECUTE format('ALTER TABLE shako.%I ENABLE ROW LEVEL SECURITY', tbl);
        EXECUTE format('ALTER TABLE shako.%I FORCE ROW LEVEL SECURITY', tbl);
        EXECUTE format('DROP POLICY IF EXISTS tenant_isolation ON shako.%I', tbl);
        EXECUTE format(
            'CREATE POLICY tenant_isolation ON shako.%I
             USING (
                 instance_id = NULLIF(current_setting(''shako.instance'', true), '''')::uuid
                 AND org_id = NULLIF(current_setting(''shako.org'', true), '''')::uuid
             )
             WITH CHECK (
                 instance_id = NULLIF(current_setting(''shako.instance'', true), '''')::uuid
                 AND org_id = NULLIF(current_setting(''shako.org'', true), '''')::uuid
             )',
            tbl
        );
        EXECUTE format('GRANT SELECT, INSERT, UPDATE, DELETE ON shako.%I TO shako_app', tbl);
    END LOOP;
END
$$;
