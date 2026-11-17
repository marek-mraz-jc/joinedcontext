-- The project id registry (ARCH/TEN-01, TEN-04, TEN-12; T-7190): every project gets 128 random
-- bits the first time the Portal sees it, so the tenancy key of a carrier is (Organization,
-- project id) and never the project's name, a label a person may type again in a branch. A
-- live project is unique by name; a branch or copy is a row with a `parent` and may hold its
-- parent's name. An id is never renamed, released or reused: the trigger refuses every UPDATE
-- of a key column and every DELETE, so a deleted project's id stays reserved here.
CREATE TABLE IF NOT EXISTS project_ids (
    id uuid PRIMARY KEY CHECK (id <> '00000000-0000-0000-0000-000000000000'),
    name text NOT NULL CHECK (name <> ''),
    parent uuid REFERENCES project_ids (id),
    minted_at timestamptz NOT NULL DEFAULT now(),
    CHECK (parent IS DISTINCT FROM id)
);

CREATE UNIQUE INDEX IF NOT EXISTS project_ids_live_name ON project_ids (name) WHERE parent IS NULL;

CREATE OR REPLACE FUNCTION project_ids_keep() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'DELETE' OR NEW.id <> OLD.id OR NEW.name <> OLD.name
        OR NEW.parent IS DISTINCT FROM OLD.parent OR NEW.minted_at <> OLD.minted_at THEN
        RAISE EXCEPTION 'project id % is never renamed, released or reused', OLD.id
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    RETURN NEW;
END
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS project_ids_keep ON project_ids;
CREATE TRIGGER project_ids_keep BEFORE UPDATE OR DELETE ON project_ids
    FOR EACH ROW EXECUTE FUNCTION project_ids_keep();
