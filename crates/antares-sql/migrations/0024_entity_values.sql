-- Leakproof keys for q= and attrs= (T-5303). Under forced RLS only a
-- leakproof operator becomes an index condition (0019), and neither the
-- jsonb operators a q= term compiles to (`@?`, `?|`) nor numeric's `=` and
-- `<` are: `speed==42` read every Entity of its type (376 ms p50 at 1M
-- Entities per Tenant, docs/src/performance.md), and without a type the
-- whole Tenant. float8 and text (COLLATE "C") comparisons are leakproof.
--
-- `entity_values` holds, per Entity and Attribute, one row per distinct
-- scalar the q compiler's jsonpath can compare (antares-ql `sql::VALUE_KEYS`
-- members of every instance, the arrays and `@value` under them down to
-- four levels; a lone instance that is no array counts as one, as lax
-- jsonpath's `[*]` reads it): a number as float8 in f, a string up to
-- 1 024 bytes (a btree entry has a size cap) in txt. An Attribute with a
-- longer string or more than 32 scalars has one row f = 'NaN' instead, the
-- "unknown" mark: JSON has no NaN, PostgreSQL sorts NaN above every number
-- and NaN = NaN holds, so `f >= $x` keeps it and the store adds
-- `OR f = 'NaN'` to the other shapes. An Attribute with no scalar has one
-- presence row, f and txt NULL. Every Attribute thus has a row in
-- i_entity_values_num, which is what attrs= reads.
-- A number past float8's range is clamped to +-Infinity or 0, which keeps
-- the order (non-strict comparisons only, store/pg/entity.rs `valued`).
--
-- The store probes it for the ids a top-level q term's or attrs='s matches
-- are among, at most 1 000, and ANDs `id = ANY(...)` to the page
-- (store/pg/entity.rs `value_probe`); every match is among them and the
-- `@?` or `?|` stays and decides. Statement triggers with transition tables keep
-- the table from every write path (create, update, upsert, merge, delete,
-- the 4.22 reaps, COPY), set-based per statement. An index changes no
-- access: the table carries 0005's per-command RLS and every query's own
-- WHERE is untouched.
CREATE TABLE entity_values (
  tenant_id text NOT NULL,
  id        text NOT NULL,
  attr      text NOT NULL,
  f         float8,
  txt       text COLLATE "C"
);
CREATE INDEX i_entity_values_num ON entity_values (tenant_id, attr, f);
CREATE INDEX i_entity_values_txt ON entity_values (tenant_id, attr, txt) WHERE txt IS NOT NULL;
CREATE INDEX i_entity_values_id  ON entity_values (tenant_id, id);

-- The rows of one Entity document. IMMUTABLE: the answer depends on the
-- document alone. The skipped members are the Entity's own (doc keys that
-- `entities` columns already carry); the store never pushes attrs= naming one.
CREATE FUNCTION entity_value_rows(p_entity jsonb)
  RETURNS TABLE (attr text, f float8, txt text)
  LANGUAGE sql IMMUTABLE PARALLEL SAFE
  AS $$
  WITH a AS (
    SELECT key, value FROM jsonb_each(p_entity)
     WHERE key NOT IN ('id', 'type', 'scope', '@context', 'createdAt',
                       'modifiedAt', 'deletedAt', 'expiresAt')
  ), s AS (
    SELECT DISTINCT a.key AS attr, v
      FROM a
      CROSS JOIN LATERAL jsonb_array_elements(
        CASE jsonb_typeof(a.value) WHEN 'array' THEN a.value
                                   ELSE jsonb_build_array(a.value) END) AS i
      CROSS JOIN LATERAL jsonb_each(
        CASE jsonb_typeof(i) WHEN 'object' THEN i ELSE '{}' END) AS m
      CROSS JOIN LATERAL jsonb_path_query(m.value, 'strict $.**{0 to 4}') AS v
     WHERE m.key IN ('value', 'object', 'languageMap', 'vocab', 'json',
                     'valueList', 'objectList')
       AND jsonb_typeof(v) IN ('number', 'string')
  ), n AS (
    SELECT attr,
           count(*) > 32
             OR bool_or(jsonb_typeof(v) = 'string' AND octet_length(v #>> '{}') > 1024)
             AS unknown
      FROM s GROUP BY attr
  )
  SELECT key, NULL::float8, NULL::text FROM a
   WHERE NOT EXISTS (SELECT 1 FROM n WHERE n.attr = a.key)
  UNION ALL
  SELECT s.attr,
         CASE WHEN jsonb_typeof(v) = 'number' THEN
           CASE WHEN v::numeric > 1e308 THEN 'Infinity'::float8
                WHEN v::numeric < -1e308 THEN '-Infinity'::float8
                WHEN abs(v::numeric) < 1e-300 THEN 0::float8
                ELSE v::numeric::float8 END
         END,
         CASE WHEN jsonb_typeof(v) = 'string' THEN v #>> '{}' END
    FROM s JOIN n USING (attr) WHERE NOT n.unknown
  UNION ALL
  SELECT attr, 'NaN'::float8, NULL::text FROM n WHERE n.unknown
$$;

CREATE FUNCTION entity_values_on_insert() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  INSERT INTO entity_values (tenant_id, id, attr, f, txt)
    SELECT n.tenant_id, n.id, r.attr, r.f, r.txt
      FROM new_rows n, entity_value_rows(n.entity) r;
  RETURN NULL;
END $$;

-- Only an Entity whose document changed rewrites its rows: an UPDATE that
-- moves only `version` or the stamps leaves them alone.
CREATE FUNCTION entity_values_on_update() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  DELETE FROM entity_values v
   USING new_rows n JOIN old_rows o USING (tenant_id, id)
   WHERE v.tenant_id = n.tenant_id AND v.id = n.id
     AND o.entity IS DISTINCT FROM n.entity;
  INSERT INTO entity_values (tenant_id, id, attr, f, txt)
    SELECT n.tenant_id, n.id, r.attr, r.f, r.txt
      FROM new_rows n JOIN old_rows o USING (tenant_id, id),
           entity_value_rows(n.entity) r
     WHERE o.entity IS DISTINCT FROM n.entity;
  RETURN NULL;
END $$;

CREATE FUNCTION entity_values_on_delete() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  DELETE FROM entity_values v USING old_rows o
   WHERE v.tenant_id = o.tenant_id AND v.id = o.id;
  RETURN NULL;
END $$;

CREATE TRIGGER entity_values_insert AFTER INSERT ON entities
  REFERENCING NEW TABLE AS new_rows
  FOR EACH STATEMENT EXECUTE FUNCTION entity_values_on_insert();
CREATE TRIGGER entity_values_update AFTER UPDATE ON entities
  REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows
  FOR EACH STATEMENT EXECUTE FUNCTION entity_values_on_update();
CREATE TRIGGER entity_values_delete AFTER DELETE ON entities
  REFERENCING OLD TABLE AS old_rows
  FOR EACH STATEMENT EXECUTE FUNCTION entity_values_on_delete();

-- The triggers come first (0012): CREATE TRIGGER holds writes to `entities`
-- until this migration commits. The backfill reads every Tenant under the
-- service escape, before the table's own RLS exists.
SELECT set_config('antares.service', 'on', true);
INSERT INTO entity_values (tenant_id, id, attr, f, txt)
  SELECT e.tenant_id, e.id, r.attr, r.f, r.txt
    FROM entities e, entity_value_rows(e.entity) r;
SELECT set_config('antares.service', '', true);
ANALYZE entity_values;

-- 0005's per-command belt: the service escape reads and deletes (the 4.22
-- reaps delete Entities of every Tenant), never inserts or updates.
ALTER TABLE entity_values ENABLE ROW LEVEL SECURITY;
ALTER TABLE entity_values FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_read ON entity_values FOR SELECT
  USING (tenant_id = current_setting('antares.tenant', true)
         OR current_setting('antares.service', true) = 'on');
CREATE POLICY tenant_reap ON entity_values FOR DELETE
  USING (tenant_id = current_setting('antares.tenant', true)
         OR current_setting('antares.service', true) = 'on');
CREATE POLICY tenant_write ON entity_values FOR INSERT
  WITH CHECK (tenant_id = current_setting('antares.tenant', true));
CREATE POLICY tenant_update ON entity_values FOR UPDATE
  USING (tenant_id = current_setting('antares.tenant', true))
  WITH CHECK (tenant_id = current_setting('antares.tenant', true));
