-- Single-person approvals (T-7388): an Organization whose only person holds every role may let
-- that person approve the access Changes PF-58 sends to a second person, after a fresh second
-- factor. Off by default; only an installation operator turns it on, through the operator door
-- (`SinglePersonSet`), so the row lives here and never in the Organization's own repository,
-- which its administrator edits. The Portal turns the mode off, with the reason, the moment a
-- second person holds a role in the Organization. Installation-level, keyed by the
-- Organization's name like the host registry, so no tenant key and no row-level security.
ALTER TABLE operator_changes DROP CONSTRAINT IF EXISTS operator_changes_kind_check;
ALTER TABLE operator_changes ADD CONSTRAINT operator_changes_kind_check
    CHECK (kind IN ('HostAdd', 'HostRemove', 'OperatorAdd', 'OperatorRemove', 'SmtpSet',
                    'BrandingSet', 'ProjectMove', 'SinglePersonSet'));

CREATE TABLE IF NOT EXISTS organization_approval_modes (
    organization text PRIMARY KEY CHECK (organization <> ''),
    single_person boolean NOT NULL DEFAULT false,
    changed_by text NOT NULL,
    changed_at timestamptz NOT NULL DEFAULT now(),
    -- Why the Portal turned the mode off by itself, e.g. a second person holds a role; NULL when
    -- an operator set the row.
    off_reason text
);
