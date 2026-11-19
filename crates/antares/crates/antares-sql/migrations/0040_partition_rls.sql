-- Row-level security on every history partition (S5-30).
--
-- 0001, 0020 and 0023 put the policies on the partitioned parents
-- (attr_instances, attr_rows) and on attr_rows_long and attr_rows_short.
-- PostgreSQL applies a parent's policies only to a statement that names the
-- parent: one that names a partition itself (attr_instances_2026w01,
-- attr_rows_default, ...) read and wrote every Tenant's rows as the request
-- role. Each partition now carries 0005's four policies of its own, ENABLEd
-- and FORCEd as the parents are; maintenance gives a week it creates the same
-- (`partition_rls_sql`). Statements that name a parent see no change: the
-- partitions' own policies are not consulted on that path.
--
-- A timescale hypertable keeps its chunks outside `public` and is left as it
-- is (ADR-0006).
DO $$
DECLARE part text;
BEGIN
  FOR part IN
    WITH RECURSIVE tree(oid) AS (
      SELECT i.inhrelid FROM pg_inherits i
       WHERE i.inhparent IN (SELECT c.oid FROM pg_class c
                              WHERE c.relnamespace = 'public'::regnamespace
                                AND c.relname IN ('attr_instances', 'attr_rows')
                                AND c.relkind = 'p')
      UNION
      SELECT i.inhrelid FROM pg_inherits i JOIN tree t ON i.inhparent = t.oid
    )
    SELECT c.relname FROM tree t JOIN pg_class c ON c.oid = t.oid
     WHERE c.relnamespace = 'public'::regnamespace AND c.relkind IN ('r', 'p')
  LOOP
    EXECUTE format('ALTER TABLE public.%I ENABLE ROW LEVEL SECURITY', part);
    EXECUTE format('ALTER TABLE public.%I FORCE ROW LEVEL SECURITY', part);
    EXECUTE format('DROP POLICY IF EXISTS tenant_read ON public.%I', part);
    EXECUTE format('CREATE POLICY tenant_read ON public.%I FOR SELECT
               USING (tenant_id = current_setting(''antares.tenant'', true)
                      OR current_setting(''antares.service'', true) = ''on'')', part);
    EXECUTE format('DROP POLICY IF EXISTS tenant_reap ON public.%I', part);
    EXECUTE format('CREATE POLICY tenant_reap ON public.%I FOR DELETE
               USING (tenant_id = current_setting(''antares.tenant'', true)
                      OR current_setting(''antares.service'', true) = ''on'')', part);
    EXECUTE format('DROP POLICY IF EXISTS tenant_write ON public.%I', part);
    EXECUTE format('CREATE POLICY tenant_write ON public.%I FOR INSERT
               WITH CHECK (tenant_id = current_setting(''antares.tenant'', true))', part);
    EXECUTE format('DROP POLICY IF EXISTS tenant_update ON public.%I', part);
    EXECUTE format('CREATE POLICY tenant_update ON public.%I FOR UPDATE
               USING (tenant_id = current_setting(''antares.tenant'', true))
               WITH CHECK (tenant_id = current_setting(''antares.tenant'', true))', part);
  END LOOP;
END $$;
