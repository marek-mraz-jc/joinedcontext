-- A project restored from one folder of a data backup into a new project (T-7208; TEN-12,
-- TEN-16). The copy's id is a project_ids row whose `parent` is the source project's id; this
-- row is its lineage: the backup version it came from, when that backup was taken, who asked
-- and when. A copy is never renamed or released, so no row here is updated or deleted.
CREATE TABLE IF NOT EXISTS project_copies (
    id uuid PRIMARY KEY REFERENCES project_ids (id),
    source uuid NOT NULL REFERENCES project_ids (id),
    organization text NOT NULL REFERENCES organizations (name),
    version_id text NOT NULL,
    taken_at text NOT NULL,
    copied_by text NOT NULL,
    copied_at timestamptz NOT NULL DEFAULT now(),
    rows bigint NOT NULL CHECK (rows >= 0),
    CHECK (source <> id)
);

CREATE INDEX IF NOT EXISTS project_copies_organization ON project_copies (organization, copied_at DESC);

CREATE OR REPLACE FUNCTION project_copies_keep() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'project copy % keeps its lineage', OLD.id
        USING ERRCODE = 'integrity_constraint_violation';
END
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS project_copies_keep ON project_copies;
CREATE TRIGGER project_copies_keep BEFORE UPDATE OR DELETE ON project_copies
    FOR EACH ROW EXECUTE FUNCTION project_copies_keep();

-- The backup's audit names the two steps of a copy: its check and the copy itself.
ALTER TABLE organization_backup_log DROP CONSTRAINT IF EXISTS organization_backup_log_action_check;
ALTER TABLE organization_backup_log ADD CONSTRAINT organization_backup_log_action_check
    CHECK (action IN ('run', 'delete', 'restore-plan', 'restore-request', 'restore-decline', 'restore',
                      'copy-plan', 'copy'));
