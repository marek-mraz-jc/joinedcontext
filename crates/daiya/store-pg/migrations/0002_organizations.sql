-- T-7509: the installation's organizations, so every replica and restart gives one Grafana
-- number and one id to one name. The table is installation-level, so it is no cr_ table (those
-- hold one organization's rows under `cr.org`): row security is on, the application role has no
-- grant on it, and it calls the three functions below, as it does for daiya_public_token.
-- A retired row stays: its number and id are never given again, its name is free.
CREATE TABLE IF NOT EXISTS daiya_org (
    installation_id UUID NOT NULL,
    number BIGINT NOT NULL CHECK (number >= 2),
    name TEXT NOT NULL COLLATE "C",
    org_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    retired_at TIMESTAMPTZ,
    PRIMARY KEY (installation_id, number),
    UNIQUE (installation_id, org_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_daiya_org_live_name
    ON daiya_org (installation_id, name) WHERE retired_at IS NULL;
ALTER TABLE daiya_org ENABLE ROW LEVEL SECURITY;
REVOKE ALL ON daiya_org FROM PUBLIC, daiya_app;

-- One organization by number, id or live name; with none of the three, the live ones.
CREATE OR REPLACE FUNCTION daiya_org_find(p_installation UUID, p_number BIGINT, p_id UUID, p_name TEXT)
    RETURNS SETOF daiya_org
    LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path = pg_catalog, public AS $$
BEGIN
    IF p_number IS NOT NULL THEN
        RETURN QUERY SELECT o.* FROM daiya_org o
            WHERE o.installation_id = p_installation AND o.number = p_number;
    ELSIF p_id IS NOT NULL THEN
        RETURN QUERY SELECT o.* FROM daiya_org o
            WHERE o.installation_id = p_installation AND o.org_id = p_id;
    ELSIF p_name IS NOT NULL THEN
        RETURN QUERY SELECT o.* FROM daiya_org o
            WHERE o.installation_id = p_installation AND o.name = p_name AND o.retired_at IS NULL;
    ELSE
        RETURN QUERY SELECT o.* FROM daiya_org o
            WHERE o.installation_id = p_installation AND o.retired_at IS NULL ORDER BY o.number;
    END IF;
END
$$;
REVOKE ALL ON FUNCTION daiya_org_find(UUID, BIGINT, UUID, TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION daiya_org_find(UUID, BIGINT, UUID, TEXT) TO daiya_app;

-- A name's organization: p_first_id the first time the installation holds the name, p_next_id
-- once one of that name was retired. A declared number (p_number) answers the row it already
-- is; a name or number another organization holds answers no row.
CREATE OR REPLACE FUNCTION daiya_org_create(p_installation UUID, p_name TEXT, p_number BIGINT,
                                            p_first_id UUID, p_next_id UUID)
    RETURNS SETOF daiya_org
    LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, public AS $$
DECLARE
    held daiya_org%ROWTYPE;
    chosen BIGINT := p_number;
BEGIN
    PERFORM pg_advisory_xact_lock(hashtextextended('daiya.org:' || p_installation::text, 0));
    IF p_number IS NOT NULL THEN
        SELECT o.* INTO held FROM daiya_org o
            WHERE o.installation_id = p_installation AND o.number = p_number;
        IF FOUND THEN
            IF held.name = p_name AND held.retired_at IS NULL THEN
                RETURN NEXT held;
            END IF;
            RETURN;
        END IF;
    END IF;
    PERFORM 1 FROM daiya_org o
        WHERE o.installation_id = p_installation AND o.name = p_name AND o.retired_at IS NULL;
    IF FOUND THEN
        RETURN;
    END IF;
    IF chosen IS NULL THEN
        SELECT GREATEST(COALESCE(MAX(o.number), 1), 1) + 1 INTO chosen FROM daiya_org o
            WHERE o.installation_id = p_installation;
    END IF;
    RETURN QUERY INSERT INTO daiya_org (installation_id, number, name, org_id)
        VALUES (p_installation, chosen, p_name, CASE WHEN EXISTS (
            SELECT 1 FROM daiya_org o WHERE o.installation_id = p_installation AND o.name = p_name
        ) THEN p_next_id ELSE p_first_id END)
        RETURNING *;
END
$$;
REVOKE ALL ON FUNCTION daiya_org_create(UUID, TEXT, BIGINT, UUID, UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION daiya_org_create(UUID, TEXT, BIGINT, UUID, UUID) TO daiya_app;

CREATE OR REPLACE FUNCTION daiya_org_retire(p_installation UUID, p_number BIGINT) RETURNS BOOLEAN
    LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, public AS $$
BEGIN
    UPDATE daiya_org SET retired_at = now()
        WHERE installation_id = p_installation AND number = p_number AND retired_at IS NULL;
    RETURN FOUND;
END
$$;
REVOKE ALL ON FUNCTION daiya_org_retire(UUID, BIGINT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION daiya_org_retire(UUID, BIGINT) TO daiya_app;
