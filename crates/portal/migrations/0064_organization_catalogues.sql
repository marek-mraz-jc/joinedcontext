-- Each Organization's dataroom catalogue (T-7186): the catalogue id jc's dataroom serves the
-- Organization's open data under, beside its name, id and working host. jc.toml seeds it once
-- (`[[organizations]] catalogue_id`); a row written without one (an operator's Change) mints it
-- by the default. Rows from before stay without one until jc.toml names it. Like the id, a
-- catalogue id once set never changes: the catalogue's datasets live under it.
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS catalogue_id uuid UNIQUE;
ALTER TABLE organizations ALTER COLUMN catalogue_id SET DEFAULT gen_random_uuid();

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
        OR (OLD.organization_id IS NOT NULL AND NEW.organization_id IS DISTINCT FROM OLD.organization_id)
        OR (OLD.catalogue_id IS NOT NULL AND NEW.catalogue_id IS DISTINCT FROM OLD.catalogue_id) THEN
        RAISE EXCEPTION 'Organization % keeps its name, its id and its catalogue', OLD.name
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    RETURN NEW;
END
$$ LANGUAGE plpgsql;
