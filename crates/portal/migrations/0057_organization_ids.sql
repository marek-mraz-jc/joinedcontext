-- The Organization ids an operator minted from a verified domain (EP-186, API/15 §2, T-7012): the
-- domain's labels reversed and joined by `_` (`bbsk.sk` -> `sk_bbsk`). A row starts with the
-- challenge the operator publishes at `_joinedcontext.{domain}`; the mint stamps minted_at once
-- the TXT record carries it. A minted id is never renamed, released or reused (PF-78): the
-- trigger refuses every UPDATE and DELETE of a minted row, so deleting the Organization later
-- keeps its id reserved here.
CREATE TABLE IF NOT EXISTS organization_ids (
    id text PRIMARY KEY,
    domain text NOT NULL UNIQUE,
    challenge text NOT NULL,
    requested_by text NOT NULL,
    requested_at timestamptz NOT NULL DEFAULT now(),
    minted_by text,
    minted_at timestamptz,
    CHECK ((minted_by IS NULL) = (minted_at IS NULL))
);

CREATE OR REPLACE FUNCTION organization_ids_keep_minted() RETURNS trigger AS $$
BEGIN
    IF OLD.minted_at IS NOT NULL THEN
        RAISE EXCEPTION 'organization id % is minted and is never renamed, released or reused', OLD.id
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS organization_ids_keep_minted ON organization_ids;
CREATE TRIGGER organization_ids_keep_minted BEFORE UPDATE OR DELETE ON organization_ids
    FOR EACH ROW EXECUTE FUNCTION organization_ids_keep_minted();
