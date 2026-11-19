-- The Tenant inventory is the maintenance role's (T-7310).
--
-- `tenants`, `tenant_rename` and `tenant_seal_alias` carried no row-level
-- security (0036, 0038): the role serving requests listed every
-- Organization's Tenants, their Organizations and the names renamed away.
-- Each now admits
--
--   * the row of the Tenant the transaction is scoped to
--     (`antares.tenant`, set by every tenant transaction): the first write's
--     claim, the purge and the counts of `GET /q/tenants/{tenant}` read and
--     write their own Tenant's row and nothing else; and
--   * a member of `antares_maintenance` (0042): the listings of
--     `/q/tenants`, the notification paths' walk of every Tenant and the
--     resealing of a renamed Tenant's credentials run on the maintenance
--     pool.
--
-- Row-level security is ENABLEd, not FORCEd: the owner, who migrates and
-- runs the operator's rename functions (0037), reads as before, and so does
-- a single-role install that serves as the owner (the ETSI stacks).
--
-- The request door asks whether a named Tenant exists and was renamed away
-- before any Tenant is set (EP-188): `antares_tenant_standing` answers that
-- for one name. It is SECURITY DEFINER, owned by the migrating role, with a
-- fixed search_path, and returns the standing of the name it is given and
-- nothing else.
CREATE FUNCTION antares_inventory_role() RETURNS boolean
  LANGUAGE sql STABLE PARALLEL SAFE AS $$
  SELECT coalesce(pg_catalog.pg_has_role(current_user,
           pg_catalog.to_regrole('antares_maintenance'), 'MEMBER'), false)
$$;

ALTER TABLE tenants ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_inventory ON tenants
  USING (tenant_id = current_setting('antares.tenant', true) OR (SELECT antares_inventory_role()))
  WITH CHECK (tenant_id = current_setting('antares.tenant', true) OR (SELECT antares_inventory_role()));

ALTER TABLE tenant_rename ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_inventory ON tenant_rename
  USING (old = current_setting('antares.tenant', true) OR (SELECT antares_inventory_role()))
  WITH CHECK ((SELECT antares_inventory_role()));

ALTER TABLE tenant_seal_alias ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_inventory ON tenant_seal_alias
  USING (tenant = current_setting('antares.tenant', true) OR (SELECT antares_inventory_role()))
  WITH CHECK ((SELECT antares_inventory_role()));

DO $do$
BEGIN
  EXECUTE format(
    $f$CREATE FUNCTION antares_tenant_standing(p_tenant text,
         OUT known boolean, OUT tombstoned boolean, OUT organization text)
         LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog, %I AS $b$
       SELECT EXISTS (SELECT 1 FROM tenants WHERE tenant_id = p_tenant),
              EXISTS (SELECT 1 FROM tenant_rename WHERE old = p_tenant),
              (SELECT organization_id FROM tenants WHERE tenant_id = p_tenant)
       $b$
    $f$,
    current_schema());
END $do$;
