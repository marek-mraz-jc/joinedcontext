-- A shard's login holds the switch into each App role of its shard and none of their privileges
-- (T-3362, AP-144): `INHERIT FALSE`, so nothing an App can do is the shard's own, and with
-- `set_config` executable only by the shard's logins (components/apps-db, postInitApplicationSQL)
-- an App role can never switch again once the host has set it.
CREATE OR REPLACE FUNCTION jc_apps.provision(app text, host_role text) RETURNS void
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
    EXECUTE format('GRANT %I TO %I WITH INHERIT TRUE, SET TRUE', owner, current_user);
    EXECUTE format('CREATE SCHEMA IF NOT EXISTS %I AUTHORIZATION %I', runtime, owner);
    EXECUTE format('REVOKE ALL ON SCHEMA %I FROM PUBLIC', runtime);
    EXECUTE format('GRANT USAGE ON SCHEMA %I TO %I', runtime, runtime);
    EXECUTE format('GRANT USAGE ON SCHEMA jc_apps TO %I', owner);
    EXECUTE format('GRANT EXECUTE ON FUNCTION jc_apps.owner_grants(), jc_apps.record_migration(text, text) TO %I', owner);
    EXECUTE format('GRANT %I TO %I WITH INHERIT FALSE, SET TRUE', runtime, host_role);
END $$;

-- The memberships provisioned before this migration, made the same.
DO $$
DECLARE
    held record;
BEGIN
    FOR held IN
        SELECT a.id, 'wasm_host_' || a.shard AS host
        FROM jc_apps.apps a
        WHERE EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'wasm_host_' || a.shard)
    LOOP
        EXECUTE format('GRANT %I TO %I WITH INHERIT FALSE, SET TRUE', 'app_' || held.id, held.host);
    END LOOP;
END $$;
