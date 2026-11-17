-- The tenancy key's Organization half and the reserved project (ARCH/TEN-01, TEN-04, TEN-11;
-- T-7191 PK-1a). Every Organization of the registry gets `key`, 128 random bits minted once and
-- never changed: the `OrgId` of the pair (OrgId, ProjectId) every carrier holds. Its text id
-- (`sk_bbsk`, minted from a domain) stays the address form. Each Organization also gets one
-- reserved project in the project registry, created with the Organization, for the rows that
-- belong to the Organization and to no project of it (members, groups, hosts, sign-in, backups),
-- so the key is never empty. A reserved project has no live name of its own: an ordinary project
-- spelled like its Organization gets an id of its own, never the reserved one.
--
-- A project row records the Organization it belongs to once the Portal has seen its
-- `spec.organizationRef` (or the installation's, ADR-N-058 section 2.3); the record is set once
-- and never moved, because moving a project between Organizations is an export and an import.

ALTER TABLE organizations
    ADD COLUMN IF NOT EXISTS key uuid NOT NULL DEFAULT gen_random_uuid()
        CHECK (key <> '00000000-0000-0000-0000-000000000000');
CREATE UNIQUE INDEX IF NOT EXISTS organizations_key ON organizations (key);

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
    IF NEW.name <> OLD.name OR NEW.key <> OLD.key
        OR (OLD.organization_id IS NOT NULL AND NEW.organization_id IS DISTINCT FROM OLD.organization_id)
        OR (OLD.catalogue_id IS NOT NULL AND NEW.catalogue_id IS DISTINCT FROM OLD.catalogue_id) THEN
        RAISE EXCEPTION 'Organization % keeps its name, its key, its id and its catalogue', OLD.name
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    RETURN NEW;
END
$$ LANGUAGE plpgsql;

ALTER TABLE project_ids
    ADD COLUMN IF NOT EXISTS organization text REFERENCES organizations (name),
    ADD COLUMN IF NOT EXISTS reserved boolean NOT NULL DEFAULT false;
ALTER TABLE project_ids DROP CONSTRAINT IF EXISTS project_ids_reserved_shape;
ALTER TABLE project_ids ADD CONSTRAINT project_ids_reserved_shape
    CHECK (NOT reserved OR (parent IS NULL AND organization IS NOT NULL));
CREATE UNIQUE INDEX IF NOT EXISTS project_ids_reserved ON project_ids (organization) WHERE reserved;
DROP INDEX IF EXISTS project_ids_live_name;
CREATE UNIQUE INDEX IF NOT EXISTS project_ids_live_project ON project_ids (name)
    WHERE parent IS NULL AND NOT reserved;

-- An id is never renamed, released or reused (0066); the Organization a project belongs to is
-- recorded once, from none, and never changed after.
CREATE OR REPLACE FUNCTION project_ids_keep() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'DELETE' OR NEW.id <> OLD.id OR NEW.name <> OLD.name
        OR NEW.parent IS DISTINCT FROM OLD.parent OR NEW.minted_at <> OLD.minted_at
        OR NEW.reserved <> OLD.reserved THEN
        RAISE EXCEPTION 'project id % is never renamed, released or reused', OLD.id
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    IF OLD.organization IS NOT NULL AND NEW.organization IS DISTINCT FROM OLD.organization THEN
        RAISE EXCEPTION 'project id % belongs to Organization % and stays there', OLD.id, OLD.organization
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    RETURN NEW;
END
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION organizations_reserve_project() RETURNS trigger AS $$
BEGIN
    INSERT INTO project_ids (id, name, organization, reserved)
    VALUES (gen_random_uuid(), NEW.name, NEW.name, true)
    ON CONFLICT (organization) WHERE reserved DO NOTHING;
    RETURN NEW;
END
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS organizations_reserve_project ON organizations;
CREATE TRIGGER organizations_reserve_project AFTER INSERT ON organizations
    FOR EACH ROW EXECUTE FUNCTION organizations_reserve_project();

INSERT INTO project_ids (id, name, organization, reserved)
SELECT gen_random_uuid(), name, name, true FROM organizations
ON CONFLICT (organization) WHERE reserved DO NOTHING;
