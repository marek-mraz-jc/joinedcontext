-- Row-level security on every history partition (S5-30).
--
-- 0001, 0020 and 0023 put the policies on the partitioned parents
-- (attr_instances, attr_rows) and on attr_rows_long and attr_rows_short.
-- PostgreSQL applies a parent's policies only to a statement that names the
-- parent: one that names a partition itself (attr_instances_2026w01,
-- attr_rows_default, ...) read and wrote every Tenant's rows as the request
-- role. `antares_partition_rls` gives a partition its parent's policies as the
-- catalog holds them now (command, roles, USING, WITH CHECK), ENABLEd and
-- FORCEd as the parents are; this migration runs it on every partition there
-- is, and maintenance on each week it creates or adopts. A partition that
-- already has both flags is left alone, so a maintenance pass rewrites no
-- catalog row. Statements that name a parent see no change: a partition's own
-- policies are not consulted on that path. A later migration that changes the
-- parents' policies drops the partitions' flags and calls this function again.
--
-- A timescale hypertable keeps its chunks outside `public` and is left as it
-- is (ADR-0006).
CREATE OR REPLACE FUNCTION public.antares_partition_rls(part regclass) RETURNS void
  LANGUAGE plpgsql
  SET search_path = pg_catalog, public
  AS $$
DECLARE
  parent regclass;
  p record;
BEGIN
  SELECT inhparent INTO parent FROM pg_inherits WHERE inhrelid = part;
  IF parent IS NULL
     OR (SELECT relrowsecurity AND relforcerowsecurity FROM pg_class WHERE oid = part) THEN
    RETURN;
  END IF;
  FOR p IN
    SELECT pol.polname,
           CASE WHEN pol.polpermissive THEN 'PERMISSIVE' ELSE 'RESTRICTIVE' END AS kind,
           CASE pol.polcmd WHEN 'r' THEN 'SELECT' WHEN 'a' THEN 'INSERT'
                           WHEN 'w' THEN 'UPDATE' WHEN 'd' THEN 'DELETE' ELSE 'ALL' END AS cmd,
           (SELECT string_agg(CASE WHEN r = 0 THEN 'PUBLIC' ELSE quote_ident(pg_get_userbyid(r)) END, ', ')
              FROM unnest(pol.polroles) AS r) AS roles,
           pg_get_expr(pol.polqual, pol.polrelid) AS qual,
           pg_get_expr(pol.polwithcheck, pol.polrelid) AS checked
      FROM pg_policy pol WHERE pol.polrelid = parent
  LOOP
    EXECUTE format('DROP POLICY IF EXISTS %I ON %s', p.polname, part);
    EXECUTE format('CREATE POLICY %I ON %s AS %s FOR %s TO %s%s%s', p.polname, part, p.kind,
                   p.cmd, p.roles,
                   CASE WHEN p.qual IS NULL THEN '' ELSE ' USING (' || p.qual || ')' END,
                   CASE WHEN p.checked IS NULL THEN '' ELSE ' WITH CHECK (' || p.checked || ')' END);
  END LOOP;
  EXECUTE format('ALTER TABLE %s ENABLE ROW LEVEL SECURITY', part);
  EXECUTE format('ALTER TABLE %s FORCE ROW LEVEL SECURITY', part);
END
$$;

-- every partition below the two parents, the nested weeks of attr_rows_long too
SELECT public.antares_partition_rls(t.relid)
  FROM (SELECT relid FROM pg_partition_tree('public.attr_instances'::regclass)
         WHERE parentrelid IS NOT NULL
        UNION ALL
        SELECT relid FROM pg_partition_tree('public.attr_rows'::regclass)
         WHERE parentrelid IS NOT NULL
        ORDER BY 1) t
  JOIN pg_class c ON c.oid = t.relid
 WHERE c.relkind IN ('r', 'p');
