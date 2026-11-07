-- Schema and tables for dataroom (C1, C2, C13, C14, C16, C17).
-- Every table is keyed by (org_id, catalogue_id, ...) and protected by forced RLS.

CREATE SCHEMA IF NOT EXISTS dataroom;

CREATE EXTENSION IF NOT EXISTS unaccent;

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_ts_config WHERE cfgname = 'dataroom_simple') THEN
        CREATE TEXT SEARCH CONFIGURATION dataroom.dataroom_simple (COPY = simple);
        ALTER TEXT SEARCH CONFIGURATION dataroom.dataroom_simple
            ALTER MAPPING FOR hword, hword_part, word WITH unaccent, simple;
    END IF;
END
$$;

CREATE OR REPLACE FUNCTION dataroom.immutable_unaccent(text)
RETURNS text
LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT AS $$
    SELECT public.unaccent('public.unaccent', $1);
$$;

CREATE TABLE IF NOT EXISTS dataroom.cat_catalogue (
    org_id uuid NOT NULL,
    id uuid NOT NULL,
    name text COLLATE "C" NOT NULL,
    host text COLLATE "C" NOT NULL UNIQUE,
    title text NOT NULL,
    description text,
    default_language text NOT NULL DEFAULT 'sk',
    main_profile text NOT NULL DEFAULT 'dcat-ap-3.0.1',
    modified timestamptz NOT NULL DEFAULT now(),
    doc jsonb NOT NULL,
    PRIMARY KEY (org_id, id),
    UNIQUE (org_id, name)
);

ALTER TABLE dataroom.cat_catalogue ENABLE ROW LEVEL SECURITY;
ALTER TABLE dataroom.cat_catalogue FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS cat_catalogue_isolation ON dataroom.cat_catalogue;
CREATE POLICY cat_catalogue_isolation ON dataroom.cat_catalogue
    USING (org_id = current_setting('jc.org', true)::uuid)
    WITH CHECK (org_id = current_setting('jc.org', true)::uuid);

CREATE TABLE IF NOT EXISTS dataroom.cat_publisher (
    org_id uuid NOT NULL,
    catalogue_id uuid NOT NULL,
    id text COLLATE "C" NOT NULL,
    name text COLLATE "C" NOT NULL,
    title text NOT NULL,
    uri text COLLATE "C" NOT NULL,
    state text COLLATE "C" NOT NULL DEFAULT 'active',
    merged_into text COLLATE "C",
    modified timestamptz NOT NULL DEFAULT now(),
    doc jsonb NOT NULL,
    PRIMARY KEY (org_id, catalogue_id, id),
    UNIQUE (org_id, catalogue_id, name)
);

ALTER TABLE dataroom.cat_publisher ENABLE ROW LEVEL SECURITY;
ALTER TABLE dataroom.cat_publisher FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS cat_publisher_isolation ON dataroom.cat_publisher;
CREATE POLICY cat_publisher_isolation ON dataroom.cat_publisher
    USING (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    )
    WITH CHECK (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    );

CREATE TABLE IF NOT EXISTS dataroom.cat_dataset (
    org_id uuid NOT NULL,
    catalogue_id uuid NOT NULL,
    id text COLLATE "C" NOT NULL,
    name text COLLATE "C" NOT NULL,
    title text NOT NULL,
    notes text,
    publisher_id text COLLATE "C" NOT NULL,
    visibility text COLLATE "C" NOT NULL DEFAULT 'public',
    state text COLLATE "C" NOT NULL DEFAULT 'active',
    licence_id text COLLATE "C",
    frequency text COLLATE "C",
    endpoint_url text COLLATE "C",
    issued timestamptz NOT NULL DEFAULT now(),
    modified timestamptz NOT NULL DEFAULT now(),
    search_sk tsvector NOT NULL DEFAULT ''::tsvector,
    search_cs tsvector NOT NULL DEFAULT ''::tsvector,
    search_en tsvector NOT NULL DEFAULT ''::tsvector,
    search_fi tsvector NOT NULL DEFAULT ''::tsvector,
    search_ca tsvector NOT NULL DEFAULT ''::tsvector,
    doc jsonb NOT NULL,
    PRIMARY KEY (org_id, catalogue_id, id),
    UNIQUE (org_id, catalogue_id, name)
);

CREATE INDEX IF NOT EXISTS idx_cat_dataset_publisher
    ON dataroom.cat_dataset (org_id, catalogue_id, publisher_id);
CREATE INDEX IF NOT EXISTS idx_cat_dataset_visibility
    ON dataroom.cat_dataset (org_id, catalogue_id, visibility, state);

ALTER TABLE dataroom.cat_dataset ENABLE ROW LEVEL SECURITY;
ALTER TABLE dataroom.cat_dataset FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS cat_dataset_isolation ON dataroom.cat_dataset;
CREATE POLICY cat_dataset_isolation ON dataroom.cat_dataset
    USING (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    )
    WITH CHECK (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    );

CREATE TABLE IF NOT EXISTS dataroom.cat_distribution (
    org_id uuid NOT NULL,
    catalogue_id uuid NOT NULL,
    dataset_id text COLLATE "C" NOT NULL,
    id text COLLATE "C" NOT NULL,
    name text NOT NULL,
    access_url text NOT NULL,
    download_url text,
    format text COLLATE "C",
    media_type text COLLATE "C",
    byte_size bigint,
    doc jsonb NOT NULL,
    PRIMARY KEY (org_id, catalogue_id, id)
);

CREATE INDEX IF NOT EXISTS idx_cat_distribution_dataset
    ON dataroom.cat_distribution (org_id, catalogue_id, dataset_id);

ALTER TABLE dataroom.cat_distribution ENABLE ROW LEVEL SECURITY;
ALTER TABLE dataroom.cat_distribution FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS cat_distribution_isolation ON dataroom.cat_distribution;
CREATE POLICY cat_distribution_isolation ON dataroom.cat_distribution
    USING (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    )
    WITH CHECK (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    );

CREATE TABLE IF NOT EXISTS dataroom.cat_view (
    org_id uuid NOT NULL,
    catalogue_id uuid NOT NULL,
    distribution_id text COLLATE "C" NOT NULL,
    id text COLLATE "C" NOT NULL,
    view_type text COLLATE "C" NOT NULL,
    title text NOT NULL,
    view_order integer NOT NULL DEFAULT 0,
    doc jsonb NOT NULL,
    PRIMARY KEY (org_id, catalogue_id, id)
);

ALTER TABLE dataroom.cat_view ENABLE ROW LEVEL SECURITY;
ALTER TABLE dataroom.cat_view FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS cat_view_isolation ON dataroom.cat_view;
CREATE POLICY cat_view_isolation ON dataroom.cat_view
    USING (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    )
    WITH CHECK (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    );

CREATE TABLE IF NOT EXISTS dataroom.cat_harvest_source (
    org_id uuid NOT NULL,
    catalogue_id uuid NOT NULL,
    id text COLLATE "C" NOT NULL,
    name text COLLATE "C" NOT NULL,
    kind text COLLATE "C" NOT NULL,
    url text NOT NULL,
    publisher_id text COLLATE "C" NOT NULL,
    enabled boolean NOT NULL DEFAULT true,
    modified timestamptz NOT NULL DEFAULT now(),
    doc jsonb NOT NULL,
    PRIMARY KEY (org_id, catalogue_id, id),
    UNIQUE (org_id, catalogue_id, name)
);

ALTER TABLE dataroom.cat_harvest_source ENABLE ROW LEVEL SECURITY;
ALTER TABLE dataroom.cat_harvest_source FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS cat_harvest_source_isolation ON dataroom.cat_harvest_source;
CREATE POLICY cat_harvest_source_isolation ON dataroom.cat_harvest_source
    USING (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    )
    WITH CHECK (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    );

CREATE TABLE IF NOT EXISTS dataroom.cat_harvest_run (
    org_id uuid NOT NULL,
    catalogue_id uuid NOT NULL,
    id text COLLATE "C" NOT NULL,
    source_id text COLLATE "C" NOT NULL,
    state text COLLATE "C" NOT NULL,
    started_at timestamptz NOT NULL,
    finished_at timestamptz,
    created_count bigint NOT NULL DEFAULT 0,
    updated_count bigint NOT NULL DEFAULT 0,
    unchanged_count bigint NOT NULL DEFAULT 0,
    withdrawn_count bigint NOT NULL DEFAULT 0,
    doc jsonb NOT NULL,
    PRIMARY KEY (org_id, catalogue_id, id)
);

ALTER TABLE dataroom.cat_harvest_run ENABLE ROW LEVEL SECURITY;
ALTER TABLE dataroom.cat_harvest_run FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS cat_harvest_run_isolation ON dataroom.cat_harvest_run;
CREATE POLICY cat_harvest_run_isolation ON dataroom.cat_harvest_run
    USING (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    )
    WITH CHECK (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    );

CREATE TABLE IF NOT EXISTS dataroom.cat_activity (
    org_id uuid NOT NULL,
    catalogue_id uuid NOT NULL,
    id text COLLATE "C" NOT NULL,
    at timestamptz NOT NULL DEFAULT now(),
    actor text COLLATE "C" NOT NULL,
    operation text COLLATE "C" NOT NULL,
    object_type text COLLATE "C" NOT NULL,
    object_id text COLLATE "C" NOT NULL,
    summary text NOT NULL,
    doc jsonb NOT NULL,
    PRIMARY KEY (org_id, catalogue_id, id)
);

ALTER TABLE dataroom.cat_activity ENABLE ROW LEVEL SECURITY;
ALTER TABLE dataroom.cat_activity FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS cat_activity_isolation ON dataroom.cat_activity;
CREATE POLICY cat_activity_isolation ON dataroom.cat_activity
    USING (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    )
    WITH CHECK (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    );

CREATE TABLE IF NOT EXISTS dataroom.cat_shacl_result (
    org_id uuid NOT NULL,
    catalogue_id uuid NOT NULL,
    id text COLLATE "C" NOT NULL,
    dataset_id text COLLATE "C" NOT NULL,
    profile_id text COLLATE "C" NOT NULL,
    profile_version text COLLATE "C" NOT NULL,
    conforms boolean NOT NULL,
    violation_count bigint NOT NULL DEFAULT 0,
    warning_count bigint NOT NULL DEFAULT 0,
    checked_at timestamptz NOT NULL DEFAULT now(),
    doc jsonb NOT NULL,
    PRIMARY KEY (org_id, catalogue_id, id)
);

ALTER TABLE dataroom.cat_shacl_result ENABLE ROW LEVEL SECURITY;
ALTER TABLE dataroom.cat_shacl_result FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS cat_shacl_result_isolation ON dataroom.cat_shacl_result;
CREATE POLICY cat_shacl_result_isolation ON dataroom.cat_shacl_result
    USING (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    )
    WITH CHECK (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    );

CREATE TABLE IF NOT EXISTS dataroom.cat_profile (
    org_id uuid NOT NULL,
    catalogue_id uuid NOT NULL,
    id text COLLATE "C" NOT NULL,
    version text COLLATE "C" NOT NULL,
    doc jsonb NOT NULL,
    PRIMARY KEY (org_id, catalogue_id, id, version)
);

ALTER TABLE dataroom.cat_profile ENABLE ROW LEVEL SECURITY;
ALTER TABLE dataroom.cat_profile FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS cat_profile_isolation ON dataroom.cat_profile;
CREATE POLICY cat_profile_isolation ON dataroom.cat_profile
    USING (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    )
    WITH CHECK (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    );

CREATE TABLE IF NOT EXISTS dataroom.cat_page (
    org_id uuid NOT NULL,
    catalogue_id uuid NOT NULL,
    slug text COLLATE "C" NOT NULL,
    lang text COLLATE "C" NOT NULL,
    modified timestamptz NOT NULL DEFAULT now(),
    doc jsonb NOT NULL,
    PRIMARY KEY (org_id, catalogue_id, slug, lang)
);

ALTER TABLE dataroom.cat_page ENABLE ROW LEVEL SECURITY;
ALTER TABLE dataroom.cat_page FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS cat_page_isolation ON dataroom.cat_page;
CREATE POLICY cat_page_isolation ON dataroom.cat_page
    USING (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    )
    WITH CHECK (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    );

CREATE TABLE IF NOT EXISTS dataroom.cat_plugin (
    org_id uuid NOT NULL,
    catalogue_id uuid NOT NULL,
    name text COLLATE "C" NOT NULL,
    modified timestamptz NOT NULL DEFAULT now(),
    doc jsonb NOT NULL,
    PRIMARY KEY (org_id, catalogue_id, name)
);

ALTER TABLE dataroom.cat_plugin ENABLE ROW LEVEL SECURITY;
ALTER TABLE dataroom.cat_plugin FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS cat_plugin_isolation ON dataroom.cat_plugin;
CREATE POLICY cat_plugin_isolation ON dataroom.cat_plugin
    USING (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    )
    WITH CHECK (
        org_id = current_setting('jc.org', true)::uuid
        AND catalogue_id = current_setting('jc.catalogue', true)::uuid
    );
