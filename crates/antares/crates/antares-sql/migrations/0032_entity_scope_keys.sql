-- Leakproof keys for scopeQ (T-6610). A scopeQ compiles to a regex per
-- scope (compile/scope.rs), and under forced RLS no index serves `~` or an
-- array operator (0019): every scopeQ page read every Entity of its types,
-- or of the Tenant (0013's 183,300 Entities a page).
--
-- entity_value_rows (0024) now also gives one row per distinct scope of the
-- Entity, attr `scope` (the document key it skipped as the Entity's own, so
-- no Attribute row carries it): txt the scope normalised the way the scope
-- regexes read it, one `/` before each segment and none after (`A//B/` is
-- `/A/B`), or f = 'NaN' past 1 024 bytes, 0024's unknown mark. The store
-- probes it like a q term (store/pg/entity.rs `scope_probe`): per OR group
-- the normalised scope equals the pattern's literal head or starts with it
-- and `/`, a text btree range on i_entity_values_txt; past 1 000 ids the
-- page reads as before, and the regex stays and decides.
--
-- The rows of every other attr are unchanged, so adding the scope rows is
-- the rebuild 0029 asks of a migration that changes this function. No
-- trigger, policy or grant changes: the same 0024 triggers keep the rows
-- and the same per-command RLS guards them.
CREATE OR REPLACE FUNCTION entity_value_rows(p_entity jsonb)
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
  UNION ALL
  SELECT DISTINCT 'scope',
         CASE WHEN octet_length(k.norm) > 1024 THEN 'NaN'::float8 END,
         CASE WHEN octet_length(k.norm) <= 1024 THEN k.norm END
    FROM (SELECT '/' || btrim(regexp_replace(e #>> '{}', '/+', '/', 'g'), '/') AS norm
            FROM jsonb_array_elements(
                   CASE jsonb_typeof(p_entity -> 'scope')
                     WHEN 'array' THEN p_entity -> 'scope'
                     WHEN 'string' THEN jsonb_build_array(p_entity -> 'scope')
                     ELSE '[]'::jsonb END) AS e
           WHERE jsonb_typeof(e) = 'string') k
$$;

-- The scope rows of the stored Entities. The service escape reads every
-- Tenant's Entities, but entity_values is FORCE RLS since 0024 and its
-- tenant_write INSERT policy takes no escape (0024 backfilled before RLS
-- existed): an owner that is no superuser, as on dev, is refused the rows
-- (T-6610, dev 2026-11-08 "new row violates row-level security policy").
-- So each Tenant's rows go in under that Tenant's own scope, transaction
-- local like set_tenant; the 0024 triggers keep them from here on.
SELECT set_config('antares.service', 'on', true);
DO $$
DECLARE t text;
BEGIN
  FOR t IN SELECT DISTINCT tenant_id FROM entities WHERE scopes IS NOT NULL LOOP
    PERFORM set_config('antares.tenant', t, true);
    INSERT INTO entity_values (tenant_id, id, attr, f, txt)
      SELECT e.tenant_id, e.id, r.attr, r.f, r.txt
        FROM entities e, entity_value_rows(e.entity) r
       WHERE e.tenant_id = t AND e.scopes IS NOT NULL AND r.attr = 'scope';
  END LOOP;
  PERFORM set_config('antares.tenant', '', true);
END $$;
SELECT set_config('antares.service', '', true);
ANALYZE entity_values;
