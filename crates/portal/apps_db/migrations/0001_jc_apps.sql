-- The Portal's own schema in apps-db, the database of the server WASM Apps (AP-149, ADR-N-044).
-- The Portal logs in as the database's owner, a CREATEROLE role and nothing more; every name it
-- builds is quoted by these functions (`%I`), so no App's id ever reaches a statement as text.

REVOKE ALL ON SCHEMA public FROM PUBLIC;
-- No temporary tables for anyone but the owner of the database (AP-144).
DO $$ BEGIN EXECUTE format('REVOKE TEMPORARY ON DATABASE %I FROM PUBLIC', current_database()); END $$;

CREATE SCHEMA jc_apps;
REVOKE ALL ON SCHEMA jc_apps FROM PUBLIC;

-- Every App the reconciler provisioned, and the shard its placement records (AP-143, AP-149).
CREATE TABLE jc_apps.apps (
    id         text PRIMARY KEY CHECK (id ~ '^[0-9a-f]{16}$'),
    project    text NOT NULL,
    name       text NOT NULL,
    shard      integer NOT NULL CHECK (shard >= 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (project, name)
);

-- The migrations each App's schema has had, written in the migration's own transaction.
CREATE TABLE jc_apps.migrations (
    app_id     text NOT NULL REFERENCES jc_apps.apps (id) ON DELETE CASCADE,
    file       text NOT NULL,
    sha256     text NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    applied_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (app_id, file)
);

CREATE FUNCTION jc_apps.check_id(app text) RETURNS void
LANGUAGE plpgsql IMMUTABLE AS $$
BEGIN
    IF app IS NULL OR app !~ '^[0-9a-f]{16}$' THEN
        RAISE EXCEPTION 'not an App id: %', left(coalesce(app, ''), 32);
    END IF;
END $$;

-- The App's owner role (owns its schema, logs in only to migrate), its run-time role (the one the
-- host sets per transaction) and its schema; idempotent. `host_role` is the login of the App's
-- shard, `wasm_host_<shard>`, which becomes a member of the run-time role and of nothing else.
CREATE FUNCTION jc_apps.provision(app text, host_role text) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    owner   text := 'app_' || app || '_owner';
    runtime text := 'app_' || app;
BEGIN
    PERFORM jc_apps.check_id(app);
    IF host_role !~ '^wasm_host_[0-9]{1,3}$' THEN
        RAISE EXCEPTION 'not a shard login role: %', left(host_role, 32);
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = owner) THEN
        EXECUTE format('CREATE ROLE %I NOLOGIN NOINHERIT NOCREATEDB NOCREATEROLE', owner);
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = runtime) THEN
        EXECUTE format('CREATE ROLE %I NOLOGIN NOINHERIT NOCREATEDB NOCREATEROLE', runtime);
    END IF;
    -- The Portal's own login holds the owner's privileges, to create its schema and to drop it at
    -- retire; a migration never runs as the Portal, so this reaches no App's code.
    EXECUTE format('GRANT %I TO %I WITH INHERIT TRUE, SET TRUE', owner, current_user);
    EXECUTE format('CREATE SCHEMA IF NOT EXISTS %I AUTHORIZATION %I', runtime, owner);
    EXECUTE format('REVOKE ALL ON SCHEMA %I FROM PUBLIC', runtime);
    EXECUTE format('GRANT USAGE ON SCHEMA %I TO %I', runtime, runtime);
    EXECUTE format('GRANT USAGE ON SCHEMA jc_apps TO %I', owner);
    EXECUTE format('GRANT EXECUTE ON FUNCTION jc_apps.owner_grants(), jc_apps.record_migration(text, text) TO %I', owner);
    EXECUTE format('GRANT %I TO %I', runtime, host_role);
END $$;

-- Run by the owner's login before and after its migrations: the run-time role reads and writes
-- every table and sequence of the schema, now and later, and never creates anything.
CREATE FUNCTION jc_apps.owner_grants() RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    owner   text := session_user;
    runtime text := regexp_replace(session_user, '_owner$', '');
BEGIN
    IF owner !~ '^app_[0-9a-f]{16}_owner$' THEN
        RAISE EXCEPTION 'only an App''s owner login grants its schema';
    END IF;
    EXECUTE format('ALTER DEFAULT PRIVILEGES IN SCHEMA %I GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO %I', runtime, runtime);
    EXECUTE format('ALTER DEFAULT PRIVILEGES IN SCHEMA %I GRANT USAGE, SELECT ON SEQUENCES TO %I', runtime, runtime);
    EXECUTE format('GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA %I TO %I', runtime, runtime);
    EXECUTE format('GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA %I TO %I', runtime, runtime);
END $$;
REVOKE ALL ON FUNCTION jc_apps.owner_grants() FROM PUBLIC;

-- A migration's record, written by the owner's login in the migration's own transaction: the App
-- is the login's, so one App can never record another's.
CREATE FUNCTION jc_apps.record_migration(file text, sha256 text) RETURNS void
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    app text := substring(session_user FROM '^app_([0-9a-f]{16})_owner$');
BEGIN
    IF app IS NULL THEN
        RAISE EXCEPTION 'only an App''s owner login records its migrations';
    END IF;
    INSERT INTO jc_apps.migrations (app_id, file, sha256) VALUES (app, file, sha256);
END $$;
REVOKE ALL ON FUNCTION jc_apps.record_migration(text, text) FROM PUBLIC;

-- The owner may log in with `verifier` (a SCRAM-SHA-256 verifier, never a password) until
-- `minutes` from now, for one migration run; `close_owner` ends it.
CREATE FUNCTION jc_apps.open_owner(app text, verifier text, minutes integer) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM jc_apps.check_id(app);
    IF verifier !~ '^SCRAM-SHA-256\$4096:' THEN
        RAISE EXCEPTION 'not a SCRAM-SHA-256 verifier';
    END IF;
    EXECUTE format('ALTER ROLE %I LOGIN PASSWORD %L VALID UNTIL %L',
        'app_' || app || '_owner', verifier, (now() + make_interval(mins => least(greatest(minutes, 1), 30)))::text);
END $$;

CREATE FUNCTION jc_apps.close_owner(app text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM jc_apps.check_id(app);
    EXECUTE format('ALTER ROLE %I NOLOGIN PASSWORD NULL', 'app_' || app || '_owner');
END $$;

-- Everything of the App, after its exports are written (AP-150): the shard's membership, the
-- schema with all it holds, both roles and the records.
CREATE FUNCTION jc_apps.retire(app text) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    owner   text := 'app_' || app || '_owner';
    runtime text := 'app_' || app;
BEGIN
    PERFORM jc_apps.check_id(app);
    EXECUTE format('DROP SCHEMA IF EXISTS %I CASCADE', runtime);
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = runtime) THEN
        EXECUTE format('DROP ROLE %I', runtime);
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = owner) THEN
        EXECUTE format('REVOKE ALL ON SCHEMA jc_apps FROM %I', owner);
        EXECUTE format('REVOKE ALL ON FUNCTION jc_apps.owner_grants(), jc_apps.record_migration(text, text) FROM %I', owner);
        EXECUTE format('DROP ROLE %I', owner);
    END IF;
    DELETE FROM jc_apps.apps WHERE id = app;
END $$;
