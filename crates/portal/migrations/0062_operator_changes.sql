-- Operator Changes (evidence/T-7129/design.md §4, T-7139): what an installation operator asks
-- for (a host added or removed, an operator added or removed), kept until a second operator
-- approves or any operator rejects it. Each holds the exact rows it writes (`payload`). A decided
-- Change never changes again. `target` is the host or the person a Change is about: one open
-- Change per target, so two operators never race on one host.
CREATE TABLE IF NOT EXISTS operator_changes (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    kind text NOT NULL
        CHECK (kind IN ('HostAdd', 'HostRemove', 'OperatorAdd', 'OperatorRemove')),
    target text NOT NULL CHECK (target <> ''),
    payload jsonb NOT NULL,
    state text NOT NULL DEFAULT 'proposed' CHECK (state IN ('proposed', 'applied', 'rejected')),
    proposed_by text NOT NULL,
    proposed_at timestamptz NOT NULL DEFAULT now(),
    decided_by text,
    decided_at timestamptz,
    CHECK ((state = 'proposed') = (decided_by IS NULL))
);

CREATE UNIQUE INDEX IF NOT EXISTS operator_changes_one_open
    ON operator_changes (target) WHERE state = 'proposed';

CREATE OR REPLACE FUNCTION operator_changes_decided_once() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'operator Change % stays on record', OLD.id
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    IF OLD.state <> 'proposed' THEN
        RAISE EXCEPTION 'operator Change % is decided and stays as it was decided', OLD.id
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    IF NEW.kind <> OLD.kind OR NEW.target <> OLD.target OR NEW.payload <> OLD.payload
        OR NEW.proposed_by <> OLD.proposed_by THEN
        RAISE EXCEPTION 'operator Change % keeps what was proposed', OLD.id
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    RETURN NEW;
END
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS operator_changes_decided_once ON operator_changes;
CREATE TRIGGER operator_changes_decided_once BEFORE UPDATE OR DELETE ON operator_changes
    FOR EACH ROW EXECUTE FUNCTION operator_changes_decided_once();

-- The operators' audit: one row per proposal, approval, rejection and applied write. The triggers
-- below refuse UPDATE, DELETE and TRUNCATE; the Portal writes each row also as a log line.
-- Ceiling: the Portal migrates and serves with one role, which owns this table and so could drop
-- the triggers; an INSERT + SELECT serving role apart from the migrating one is T-7179.
CREATE TABLE IF NOT EXISTS operator_audit (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    at timestamptz NOT NULL DEFAULT now(),
    actor text NOT NULL,
    change_id uuid REFERENCES operator_changes (id),
    action text NOT NULL,
    before jsonb,
    after jsonb,
    source_ip text
);

CREATE INDEX IF NOT EXISTS operator_audit_change ON operator_audit (change_id);

CREATE OR REPLACE FUNCTION operator_audit_append_only() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'the operators'' audit is append-only'
        USING ERRCODE = 'integrity_constraint_violation';
END
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS operator_audit_append_only ON operator_audit;
CREATE TRIGGER operator_audit_append_only BEFORE UPDATE OR DELETE ON operator_audit
    FOR EACH ROW EXECUTE FUNCTION operator_audit_append_only();

DROP TRIGGER IF EXISTS operator_audit_no_truncate ON operator_audit;
CREATE TRIGGER operator_audit_no_truncate BEFORE TRUNCATE ON operator_audit
    FOR EACH STATEMENT EXECUTE FUNCTION operator_audit_append_only();
