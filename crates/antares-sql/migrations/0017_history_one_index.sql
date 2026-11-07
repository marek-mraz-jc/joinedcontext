-- One btree for the history instead of two (T-5057). i_attr_instances_lookup
-- (tenant_id, entity_id, attr_id, observed_at DESC) served the reads and
-- u_attr_instances (tenant_id, entity_id, attr_id, instance_id, observed_at)
-- the upsert's conflict: on dev 9.0 GB + 7.6 GB of a 30 GB weekly partition,
-- ~720 B of a 1.25 KB row. u_attr_instances_time holds the same columns with
-- observed_at before instance_id: its (tenant, entity, attr) prefix with an
-- observed_at range is the lookup read backwards, and ON CONFLICT infers it
-- from the column set, whatever the order.
--
-- A database with history to rebuild is left as it is: building either index
-- here would hold writes for the length of a scan of the whole history. The
-- maintenance job (store/pg/maintenance.rs `history_one_index`) builds the
-- new one partition by partition with CREATE INDEX CONCURRENTLY and swaps
-- them in one short transaction. A timescale hypertable with history keeps
-- both indexes: timescale builds a unique index on all chunks at once only.
DO $$
BEGIN
  IF to_regclass('public.i_attr_instances_lookup') IS NULL THEN
    RETURN;
  END IF;
  -- Sizes from the catalog, never a SELECT: under forced RLS the migrating
  -- role sees no row of any tenant, and would rebuild 100 GB in place.
  -- Partitions and timescale chunks are both inheritors of attr_instances.
  IF (SELECT coalesce(sum(pg_relation_size(i.inhrelid)), 0) FROM pg_inherits i
      WHERE i.inhparent = 'public.attr_instances'::regclass) > 64 * 1024 * 1024 THEN
    RETURN;
  END IF;
  DROP INDEX i_attr_instances_lookup;
  DROP INDEX u_attr_instances;
  CREATE UNIQUE INDEX u_attr_instances_time
    ON attr_instances (tenant_id, entity_id, attr_id, observed_at, instance_id);
END $$;
