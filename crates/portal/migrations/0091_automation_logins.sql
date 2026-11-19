-- Automation logins (T-7432): a sign-in an agent or a script holds (the `<org>-mcp-admin` behind
-- an Organization's management connector) is a person to the realm, but never a second person to
-- PF-58. A row here marks one: it never approves a Change, an access request or an erasure, and
-- the single-person rule (T-7388) does not count it. Only an installation operator writes a row,
-- through the operator door (`AutomationLoginSet`), so neither the Organization's administrator,
-- a mapper nor the login itself sets or clears the mark (IDN-13). Installation-level, keyed by the
-- Organization's name like `organization_approval_modes`, so no tenant key and no row-level
-- security.
ALTER TABLE operator_changes DROP CONSTRAINT IF EXISTS operator_changes_kind_check;
ALTER TABLE operator_changes ADD CONSTRAINT operator_changes_kind_check
    CHECK (kind IN ('HostAdd', 'HostRemove', 'OperatorAdd', 'OperatorRemove', 'SmtpSet',
                    'BrandingSet', 'ProjectMove', 'SinglePersonSet', 'AutomationLoginSet'));

CREATE TABLE IF NOT EXISTS automation_logins (
    organization text NOT NULL CHECK (organization <> ''),
    -- The login as the Organization's RoleBinding names it: a user id, username or e-mail,
    -- lower case.
    login text NOT NULL CHECK (login <> '' AND login = lower(login)),
    -- The connector the login stands behind, when the operator named one.
    connector text,
    marked_by text NOT NULL,
    marked_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (organization, login)
);
