-- The installation's e-mail server as an operator Change (T-7152). `SmtpSet` joins the kinds; a
-- Change that brings a new SMTP password holds it sealed for the Portal's own age recipient in
-- `sealed_secret`, never in `payload`, until a second operator decides it: the decision clears it,
-- so a decided Change holds no secret.
ALTER TABLE operator_changes DROP CONSTRAINT IF EXISTS operator_changes_kind_check;
ALTER TABLE operator_changes ADD CONSTRAINT operator_changes_kind_check
    CHECK (kind IN ('HostAdd', 'HostRemove', 'OperatorAdd', 'OperatorRemove', 'SmtpSet'));
ALTER TABLE operator_changes ADD COLUMN IF NOT EXISTS sealed_secret text;
ALTER TABLE operator_changes DROP CONSTRAINT IF EXISTS operator_changes_secret_only_while_open;
ALTER TABLE operator_changes ADD CONSTRAINT operator_changes_secret_only_while_open
    CHECK (state = 'proposed' OR sealed_secret IS NULL);
