-- Short history unlogged (T-5249; docs/src/storage.md "Short history").
-- The rows of a type whose retention keeps PT1H of history or less go to an
-- UNLOGGED table: they write no WAL, a crash empties the table and a replica
-- holds none of it, which loses at most the trail the sweep drops within the
-- hour anyway. Current state and every longer history stay logged.
--
-- attr_rows becomes a parent partitioned by the new column `short`: the
-- range-partitioned parent 0020 built is renamed attr_rows_long and attached
-- as `short = false`, and the unlogged attr_rows_short holds `short = true`.
-- Every statement keeps reading and deleting attr_rows, and the planner merges
-- both children in index order. Under timescale a hypertable can be neither
-- unlogged nor a partition: attr_rows only gains the column and keeps every
-- row logged.
--
-- The unique key takes `short` (a partition key column must be in it), so a
-- release before this one finds no index its ON CONFLICT names: breaking.
DO $$
DECLARE cols text := '
  tenant_id   text NOT NULL,
  entity_no   bigint NOT NULL,
  attr_no     integer NOT NULL,
  instance_no uuid NOT NULL,
  dataset_id  text,
  observed_at timestamptz NOT NULL,
  created_at  timestamptz NOT NULL,
  modified_at timestamptz NOT NULL,
  deleted_at  timestamptz,
  data        jsonb NOT NULL,
  geo_value   geometry(Geometry, 4326),
  version     integer NOT NULL DEFAULT 1,
  provenance  jsonb,
  short       boolean NOT NULL DEFAULT false
';
  tbl text;
BEGIN
  IF EXISTS (SELECT 1 FROM information_schema.columns
              WHERE table_schema = 'public' AND table_name = 'attr_rows'
                AND column_name = 'short') THEN
    RETURN;
  END IF;
  IF EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'timescaledb') THEN
    EXECUTE 'ALTER TABLE attr_rows ADD COLUMN short boolean NOT NULL DEFAULT false';
    EXECUTE 'DROP INDEX u_attr_rows_time';
    EXECUTE 'CREATE UNIQUE INDEX u_attr_rows_time
               ON attr_rows (tenant_id, entity_no, attr_no, observed_at, instance_no, short)';
    RETURN;
  END IF;
  EXECUTE 'ALTER TABLE attr_rows RENAME TO attr_rows_long';
  EXECUTE 'DROP INDEX u_attr_rows_time';
  EXECUTE 'ALTER TABLE attr_rows_long ADD COLUMN short boolean NOT NULL DEFAULT false';
  EXECUTE 'CREATE TABLE attr_rows (' || cols || ') PARTITION BY LIST (short)';
  EXECUTE 'ALTER TABLE attr_rows ATTACH PARTITION attr_rows_long FOR VALUES IN (false)';
  EXECUTE 'CREATE UNLOGGED TABLE attr_rows_short PARTITION OF attr_rows FOR VALUES IN (true)';
  EXECUTE 'CREATE UNIQUE INDEX u_attr_rows_time
             ON attr_rows (tenant_id, entity_no, attr_no, observed_at, instance_no, short)';
  -- 0005's policies on the new parent, which every statement reaches, and on
  -- the unlogged child; attr_rows_long keeps the ones 0020 gave it.
  EXECUTE 'ALTER TABLE attr_rows ENABLE ROW LEVEL SECURITY';
  EXECUTE 'ALTER TABLE attr_rows FORCE ROW LEVEL SECURITY';
  EXECUTE 'ALTER TABLE attr_rows_short ENABLE ROW LEVEL SECURITY';
  EXECUTE 'ALTER TABLE attr_rows_short FORCE ROW LEVEL SECURITY';
  FOREACH tbl IN ARRAY ARRAY['attr_rows', 'attr_rows_short'] LOOP
    EXECUTE format('CREATE POLICY tenant_read ON %I FOR SELECT
               USING (tenant_id = current_setting(''antares.tenant'', true)
                      OR current_setting(''antares.service'', true) = ''on'')', tbl);
    EXECUTE format('CREATE POLICY tenant_reap ON %I FOR DELETE
               USING (tenant_id = current_setting(''antares.tenant'', true)
                      OR current_setting(''antares.service'', true) = ''on'')', tbl);
    EXECUTE format('CREATE POLICY tenant_write ON %I FOR INSERT
               WITH CHECK (tenant_id = current_setting(''antares.tenant'', true))', tbl);
    EXECUTE format('CREATE POLICY tenant_update ON %I FOR UPDATE
               USING (tenant_id = current_setting(''antares.tenant'', true))
               WITH CHECK (tenant_id = current_setting(''antares.tenant'', true))', tbl);
  END LOOP;
END $$;
