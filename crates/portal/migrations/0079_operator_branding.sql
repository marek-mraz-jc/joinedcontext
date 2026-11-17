-- The installation's own look as an operator Change (T-7188). `BrandingSet` joins the kinds; its
-- payload holds the look (names, colours, an SVG logo and favicon), and the last applied one is
-- what the Portal and the sign-in pages show over the branding file.
ALTER TABLE operator_changes DROP CONSTRAINT IF EXISTS operator_changes_kind_check;
ALTER TABLE operator_changes ADD CONSTRAINT operator_changes_kind_check
    CHECK (kind IN ('HostAdd', 'HostRemove', 'OperatorAdd', 'OperatorRemove', 'SmtpSet', 'BrandingSet'));
CREATE INDEX IF NOT EXISTS operator_changes_applied_kind
    ON operator_changes (kind, decided_at DESC) WHERE state = 'applied';
