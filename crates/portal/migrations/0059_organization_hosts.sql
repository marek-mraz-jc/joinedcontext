-- The host registry (evidence/T-7129/design.md §1, T-7135): every host an Organization answers
-- on, owned by jc in the Portal's database; jc.toml only seeds it. A host belongs to one
-- Organization (the primary key) and links to the Organization id minted from its verified
-- domain where there is one. Only `served` rows answer (the router and the edge read those).
-- A `removed` row is a tombstone: it never changes again and no row is ever deleted, so the
-- seed, which inserts only hosts without a row, never brings a removed host back.
CREATE TABLE IF NOT EXISTS organization_hosts (
    host text PRIMARY KEY CHECK (host <> '' AND host = lower(host)),
    organization text NOT NULL,
    organization_id text REFERENCES organization_ids (id),
    role text NOT NULL,
    state text NOT NULL
        CHECK (state IN ('proposed', 'approved', 'dns-ok', 'cert-issued', 'served', 'removed')),
    created_by text NOT NULL,
    approved_by text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS organization_hosts_organization ON organization_hosts (organization);

CREATE OR REPLACE FUNCTION organization_hosts_keep_removed() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'host % stays in the registry: mark it removed, never delete it', OLD.host
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    IF OLD.state = 'removed' THEN
        RAISE EXCEPTION 'host % is removed and stays removed', OLD.host
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    IF NEW.host <> OLD.host OR NEW.organization <> OLD.organization THEN
        RAISE EXCEPTION 'host % keeps its name and its Organization', OLD.host
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    RETURN NEW;
END
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS organization_hosts_keep_removed ON organization_hosts;
CREATE TRIGGER organization_hosts_keep_removed BEFORE UPDATE OR DELETE ON organization_hosts
    FOR EACH ROW EXECUTE FUNCTION organization_hosts_keep_removed();

CREATE OR REPLACE FUNCTION organization_hosts_no_truncate() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'the host registry is never truncated: its removed rows keep removed hosts removed'
        USING ERRCODE = 'integrity_constraint_violation';
END
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS organization_hosts_no_truncate ON organization_hosts;
CREATE TRIGGER organization_hosts_no_truncate BEFORE TRUNCATE ON organization_hosts
    FOR EACH STATEMENT EXECUTE FUNCTION organization_hosts_no_truncate();
