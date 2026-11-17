-- The Organizations jc runs (evidence/T-7129/design.md §5, T-7181): name, the minted id it is
-- proved by, and its working host (lowercase, a port only where jc.toml gives one), owned by jc
-- in the Portal's database beside organization_ids and the host registry; jc.toml only seeds it,
-- as it seeds `organization_hosts`. The row is the source of an Organization's name, id and
-- working host at run time; its look stays in the Organization's manifest (`spec.appearance`,
-- one writer). A `removed` row is a tombstone: no row is ever deleted, and the seed, which
-- inserts only names without a row, never brings a removed Organization back. The name never
-- changes, and the id, once set, neither.
CREATE TABLE IF NOT EXISTS organizations (
    -- The router's rule for an Organization name (jc-router hosts.rs valid_organization), and a
    -- host name with an optional port: nothing a list built from the rows could split on.
    name text PRIMARY KEY CHECK (name ~ '^[a-z][a-z0-9_-]{0,62}$'),
    organization_id text UNIQUE REFERENCES organization_ids (id),
    host text NOT NULL CHECK (host ~ '^[a-z0-9]([a-z0-9.-]*[a-z0-9])?(:[0-9]{1,5})?$'),
    state text NOT NULL CHECK (state IN ('active', 'removed')),
    created_by text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE OR REPLACE FUNCTION organizations_keep_removed() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'Organization % stays in the registry: mark it removed, never delete it', OLD.name
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    IF OLD.state = 'removed' THEN
        RAISE EXCEPTION 'Organization % is removed and stays removed', OLD.name
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    IF NEW.name <> OLD.name
        OR (OLD.organization_id IS NOT NULL AND NEW.organization_id IS DISTINCT FROM OLD.organization_id) THEN
        RAISE EXCEPTION 'Organization % keeps its name and its id', OLD.name
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    RETURN NEW;
END
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS organizations_keep_removed ON organizations;
CREATE TRIGGER organizations_keep_removed BEFORE UPDATE OR DELETE ON organizations
    FOR EACH ROW EXECUTE FUNCTION organizations_keep_removed();

CREATE OR REPLACE FUNCTION organizations_no_truncate() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'the Organization registry is never truncated: its removed rows keep removed Organizations removed'
        USING ERRCODE = 'integrity_constraint_violation';
END
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS organizations_no_truncate ON organizations;
CREATE TRIGGER organizations_no_truncate BEFORE TRUNCATE ON organizations
    FOR EACH STATEMENT EXECUTE FUNCTION organizations_no_truncate();

-- The Organizations the host registry already names (an operator's host Change before this
-- table existed) keep their hosts: each gets its row, at its working host where it has one.
INSERT INTO organizations (name, organization_id, host, state, created_by)
SELECT DISTINCT ON (organization) organization, organization_id, host, 'active', 'migration 0063'
FROM organization_hosts
WHERE state <> 'removed'
    AND organization ~ '^[a-z][a-z0-9_-]{0,62}$'
    AND host ~ '^[a-z0-9]([a-z0-9.-]*[a-z0-9])?$'
ORDER BY organization, (role = 'working') DESC, host
ON CONFLICT DO NOTHING;

-- One watcher follows both tables: a write here wakes jc as a host write does (migration 0061).
DROP TRIGGER IF EXISTS organizations_notify ON organizations;
CREATE TRIGGER organizations_notify AFTER INSERT OR UPDATE ON organizations
    FOR EACH STATEMENT EXECUTE FUNCTION organization_hosts_notify();
