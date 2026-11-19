-- The cross-Tenant escape is keyed on a role as well as a setting (T-7307).
--
-- 0001 and 0005 opened the reads and deletes the outbox drain and the reaps
-- need across Tenants on `current_setting('antares.service', true) = 'on'`.
-- That is a plain custom setting: any role can arm it, the role serving
-- requests included, so a SQL-injection bug on the request path read every
-- Tenant's entities, outbox and history. The escape now also needs the role
-- to be
--
--   * a member of the role `antares_maintenance`, which the deployment grants
--     to the broker's maintenance pool and never to the request role; or
--   * the owner of the schema's tables, which can switch Row-Level Security
--     off anyway, so this path opens nothing an owner lacks. A deployment
--     that serves as the owner (the ETSI stacks, a single-role install)
--     works as before, and still needs the setting armed, so a request
--     statement that forgets its Tenant predicate stays inside the Tenant.
--
-- The role need not exist: until the deployment creates it, only the owner
-- path holds.
--
-- Every name is schema-qualified and the owner lookup is baked in at
-- migration time, so no `search_path` a caller sets changes what the
-- function reads. It stays a plain SQL function (no SET clause, no
-- SECURITY DEFINER) so the planner inlines it; policies call it as
-- `(SELECT antares_service())`, an InitPlan evaluated once per statement.
DO $do$
BEGIN
  EXECUTE format(
    $f$CREATE FUNCTION antares_service() RETURNS boolean
         LANGUAGE sql STABLE PARALLEL SAFE AS $b$
       SELECT coalesce(pg_catalog.current_setting('antares.service', true), '') = 'on'
          AND (coalesce(pg_catalog.pg_has_role(current_user,
                          pg_catalog.to_regrole('antares_maintenance'), 'MEMBER'), false)
               OR pg_catalog.pg_has_role(current_user,
                    (SELECT c.relowner FROM pg_catalog.pg_class c
                      WHERE c.oid = %L::pg_catalog.regclass), 'MEMBER'))
       $b$
    $f$,
    format('%I.entities', current_schema()));
END $do$;

-- Rewrite every policy that names the setting. Only USING clauses do (0005
-- split the escape off every WITH CHECK); a WITH CHECK naming it, or a USING
-- naming it in a form other than the one 0001-0024 wrote, stops the
-- migration instead of leaving an escape behind.
DO $$
DECLARE
  p record;
  arm constant text := '(current_setting(''antares.service''::text, true) = ''on''::text)';
  rewritten text;
BEGIN
  FOR p IN
    SELECT schemaname, tablename, policyname, qual, with_check
      FROM pg_catalog.pg_policies
     WHERE schemaname = current_schema()
       AND (qual LIKE '%antares.service%' OR with_check LIKE '%antares.service%')
  LOOP
    IF p.with_check LIKE '%antares.service%' THEN
      RAISE EXCEPTION 'policy % on % names antares.service in WITH CHECK',
        p.policyname, p.tablename;
    END IF;
    rewritten := replace(p.qual, arm, '(SELECT antares_service())');
    IF rewritten LIKE '%antares.service%' THEN
      RAISE EXCEPTION 'policy % on % names antares.service as %',
        p.policyname, p.tablename, p.qual;
    END IF;
    EXECUTE format('ALTER POLICY %I ON %I.%I USING (%s)',
                   p.policyname, p.schemaname, p.tablename, rewritten);
  END LOOP;
END $$;

-- jsonld_contexts (0006): a Cached copy (tenant_id NULL) was UPDATE- and
-- DELETE-able by every Tenant, so one Tenant could rewrite or evict the
-- @context every other Tenant resolves. Any Tenant still inserts a Cached
-- copy (the write-through of a first use), and reads it; replacing,
-- refetching and evicting one is maintenance work, under the same escape.
-- A Tenant's own rows keep their per-Tenant belt.
DROP POLICY tenant_update ON jsonld_contexts;
DROP POLICY tenant_reap ON jsonld_contexts;
CREATE POLICY tenant_update ON jsonld_contexts FOR UPDATE
  USING (tenant_id = current_setting('antares.tenant', true)
         OR (tenant_id IS NULL AND (SELECT antares_service())))
  WITH CHECK (tenant_id = current_setting('antares.tenant', true)
              OR (tenant_id IS NULL AND (SELECT antares_service())));
CREATE POLICY tenant_reap ON jsonld_contexts FOR DELETE
  USING (tenant_id = current_setting('antares.tenant', true)
         OR (tenant_id IS NULL AND (SELECT antares_service())));
