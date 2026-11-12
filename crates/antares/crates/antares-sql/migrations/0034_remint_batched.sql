-- The remint in bounded transactions (T-6830). 0033's remint_apply picked
-- the rows to rename with `id IN (SELECT old_id FROM remint_map) OR
-- doc::text LIKE pat`: an OR keeps the planner from a semi-join, and a map
-- larger than work_mem cannot be a hashed SubPlan, so every row of the
-- Tenant rescanned the materialised map from a temp file (jc-dev copy:
-- bbsk-otvorene-data, 215 k ids, over 2 h in BufFileRead). Every id in the
-- map holds `from`, so `id LIKE pat` selects a superset of the map and the
-- scan stays linear; remint_id leaves any other id as it is.
--
-- remint_apply now takes an id range [p_lo, p_hi) (NULL: open) and renames
-- only the rows keyed by an id in it: entities and temporal_entities by id,
-- attr_instances and attr_versions by entity_id, iri_ids by iri and
-- attr_rows through its iri. The tables keyed by no Entity id
-- (Subscriptions, registrations, views, snapshots, entity maps, the remap
-- chain) go with the range open above. With both ends NULL it is 0033's
-- single transaction, which antares_remint and the rollback still call.
--
-- antares_remint_batched(tenant, from, to, batch) is the live form: it
-- checks every new id for a clash first, renaming nothing on one, then
-- commits one range of `batch` Entity ids (default 1000) at a time, so a
-- writer waits on the locks of one range (seconds), not of the whole
-- Tenant. Between commits a Relationship can name a new id whose range is
-- still to come: the run ends with every id and document renamed, as one
-- transaction would. CALL it outside a transaction block; a range that
-- deadlocks with a writer is tried again, and a run that stopped (a clash
-- written meanwhile) is resumed by calling it again.

CREATE FUNCTION remint_map_build(p_tenant text, p_from text, p_to text, p_lo text, p_hi text)
  RETURNS bigint LANGUAGE plpgsql
  SET plan_cache_mode = force_custom_plan
  AS $$
DECLARE
  n bigint;
  clash text;
BEGIN
  IF to_regclass('pg_temp.remint_map') IS NOT NULL THEN
    DROP TABLE pg_temp.remint_map;
  END IF;
  CREATE TEMP TABLE remint_map (old_id text PRIMARY KEY, new_id text NOT NULL UNIQUE)
    ON COMMIT DROP;
  INSERT INTO remint_map
    SELECT id, remint_id(id, p_from, p_to) FROM (
      SELECT id FROM entities WHERE tenant_id = p_tenant
         AND (p_lo IS NULL OR id >= p_lo) AND (p_hi IS NULL OR id < p_hi)
      UNION SELECT id FROM temporal_entities WHERE tenant_id = p_tenant
         AND (p_lo IS NULL OR id >= p_lo) AND (p_hi IS NULL OR id < p_hi)
      UNION SELECT entity_id FROM attr_instances WHERE tenant_id = p_tenant
         AND (p_lo IS NULL OR entity_id >= p_lo) AND (p_hi IS NULL OR entity_id < p_hi)
      UNION SELECT entity_id FROM attr_versions WHERE tenant_id = p_tenant
         AND (p_lo IS NULL OR entity_id >= p_lo) AND (p_hi IS NULL OR entity_id < p_hi)
      UNION SELECT iri FROM iri_ids WHERE tenant_id = p_tenant AND iri LIKE 'urn:ngsi-ld:%'
         AND (p_lo IS NULL OR iri >= p_lo) AND (p_hi IS NULL OR iri < p_hi)
    ) s(id)
    WHERE id LIKE 'urn:ngsi-ld:%' AND remint_id(id, p_from, p_to) <> id;
  GET DIAGNOSTICS n = ROW_COUNT;
  ANALYZE pg_temp.remint_map;

  -- a new id some row already holds would merge two Entities: refuse
  SELECT m.new_id INTO clash FROM remint_map m
   WHERE EXISTS (SELECT 1 FROM entities WHERE tenant_id = p_tenant AND id = m.new_id)
      OR EXISTS (SELECT 1 FROM temporal_entities WHERE tenant_id = p_tenant AND id = m.new_id)
      OR EXISTS (SELECT 1 FROM attr_instances WHERE tenant_id = p_tenant AND entity_id = m.new_id)
      OR EXISTS (SELECT 1 FROM attr_versions WHERE tenant_id = p_tenant AND entity_id = m.new_id)
      OR EXISTS (SELECT 1 FROM iri_ids WHERE tenant_id = p_tenant AND iri = m.new_id)
   LIMIT 1;
  IF clash IS NOT NULL THEN
    RAISE EXCEPTION 'Tenant % already holds %: nothing renamed', p_tenant, clash;
  END IF;
  RETURN n;
END $$;

DROP FUNCTION remint_apply(text, text, text, boolean);
CREATE FUNCTION remint_apply(p_tenant text, p_from text, p_to text, p_record boolean,
                             p_lo text DEFAULT NULL, p_hi text DEFAULT NULL)
  RETURNS jsonb LANGUAGE plpgsql
  -- the range ends are NULL or not per call: plan each with them folded in
  SET plan_cache_mode = force_custom_plan
  AS $$
DECLARE
  pat text := '%' || p_from || '%';
  out jsonb := jsonb_build_object('tenant', p_tenant, 'from', p_from, 'to', p_to);
  n bigint;
BEGIN
  -- guarded here too: REVOKE does not bind the function's owner, so an owner
  -- under row-level security calling remint_apply directly is refused as well
  PERFORM remint_guard(p_tenant, p_from, p_to);
  IF p_from = p_to THEN
    RAISE EXCEPTION 'from and to are both %', p_from;
  END IF;
  PERFORM set_config('lock_timeout', '30s', true);
  -- writers wait until the transaction ends; readers go on
  LOCK TABLE entities, temporal_entities, attr_instances, attr_rows, attr_versions,
             iri_ids, entity_values, subscriptions, csource_subscriptions,
             csource_registrations, csource_index, views, snapshots,
             entity_map_docs, dist_subs, entity_id_remap IN EXCLUSIVE MODE;

  out := out || jsonb_build_object('ids', remint_map_build(p_tenant, p_from, p_to, p_lo, p_hi));

  -- attr_rows keys its Entity by iri_ids number: renaming the IRI renames
  -- every row; only the values naming an id are rewritten, picked by the
  -- IRI before it moves out of the range
  UPDATE attr_rows SET data = remint_doc(data, p_from, p_to)
   WHERE tenant_id = p_tenant AND data::text LIKE pat
     AND ((p_lo IS NULL AND p_hi IS NULL)
          OR entity_no IN (SELECT id FROM iri_ids WHERE tenant_id = p_tenant
                              AND (p_lo IS NULL OR iri >= p_lo) AND (p_hi IS NULL OR iri < p_hi)));
  GET DIAGNOSTICS n = ROW_COUNT;
  out := out || jsonb_build_object('attr_rows', n);
  UPDATE iri_ids i SET iri = m.new_id FROM remint_map m
   WHERE i.tenant_id = p_tenant AND i.iri = m.old_id;
  GET DIAGNOSTICS n = ROW_COUNT;
  out := out || jsonb_build_object('iri_ids', n);

  UPDATE entities SET id = remint_id(id, p_from, p_to), entity = remint_doc(entity, p_from, p_to)
   WHERE tenant_id = p_tenant AND (id LIKE pat OR entity::text LIKE pat)
     AND (p_lo IS NULL OR id >= p_lo) AND (p_hi IS NULL OR id < p_hi);
  GET DIAGNOSTICS n = ROW_COUNT;
  out := out || jsonb_build_object('entities', n);
  -- the update trigger pairs old and new rows by id, so a renamed Entity's
  -- keys move here, each string renamed as remint_doc renamed it in the
  -- document (0033 recomputed them from the document: 3 of every 4 ms an
  -- Entity cost); a document that only names one is the trigger's
  UPDATE entity_values v SET id = m.new_id, txt = remint_id(v.txt, p_from, p_to)
    FROM remint_map m
   WHERE v.tenant_id = p_tenant AND v.id = m.old_id;

  UPDATE temporal_entities SET id = remint_id(id, p_from, p_to), meta = remint_doc(meta, p_from, p_to)
   WHERE tenant_id = p_tenant AND (id LIKE pat OR meta::text LIKE pat)
     AND (p_lo IS NULL OR id >= p_lo) AND (p_hi IS NULL OR id < p_hi);
  GET DIAGNOSTICS n = ROW_COUNT;
  out := out || jsonb_build_object('temporal_entities', n);

  UPDATE attr_instances
     SET entity_id = remint_id(entity_id, p_from, p_to), data = remint_doc(data, p_from, p_to)
   WHERE tenant_id = p_tenant AND (entity_id LIKE pat OR data::text LIKE pat)
     AND (p_lo IS NULL OR entity_id >= p_lo) AND (p_hi IS NULL OR entity_id < p_hi);
  GET DIAGNOSTICS n = ROW_COUNT;
  out := out || jsonb_build_object('attr_instances', n);

  UPDATE attr_versions
     SET entity_id = remint_id(entity_id, p_from, p_to), data = remint_doc(data, p_from, p_to)
   WHERE tenant_id = p_tenant AND (entity_id LIKE pat OR data::text LIKE pat)
     AND (p_lo IS NULL OR entity_id >= p_lo) AND (p_hi IS NULL OR entity_id < p_hi);
  GET DIAGNOSTICS n = ROW_COUNT;
  out := out || jsonb_build_object('attr_versions', n);

  IF p_record THEN
    INSERT INTO entity_id_remap (tenant_id, old_id, new_id)
      SELECT p_tenant, old_id, new_id FROM remint_map
    ON CONFLICT (tenant_id, old_id) DO UPDATE
      SET new_id = EXCLUDED.new_id, remapped_at = now();
  END IF;

  IF p_hi IS NOT NULL THEN
    RETURN out;
  END IF;

  -- the documents keyed by no Entity id: once, with the last range
  UPDATE subscriptions SET subscription = remint_doc(subscription, p_from, p_to)
   WHERE tenant_id = p_tenant AND subscription::text LIKE pat;
  GET DIAGNOSTICS n = ROW_COUNT;
  out := out || jsonb_build_object('subscriptions', n);
  UPDATE csource_subscriptions SET subscription = remint_doc(subscription, p_from, p_to)
   WHERE tenant_id = p_tenant AND subscription::text LIKE pat;
  GET DIAGNOSTICS n = ROW_COUNT;
  out := out || jsonb_build_object('csource_subscriptions', n);
  UPDATE csource_registrations SET registration = remint_doc(registration, p_from, p_to)
   WHERE tenant_id = p_tenant AND registration::text LIKE pat;
  GET DIAGNOSTICS n = ROW_COUNT;
  out := out || jsonb_build_object('csource_registrations', n);
  UPDATE csource_index SET entity_id = remint_id(entity_id, p_from, p_to)
   WHERE tenant_id = p_tenant AND entity_id LIKE pat;
  UPDATE views SET doc = remint_doc(doc, p_from, p_to)
   WHERE tenant_id = p_tenant AND doc::text LIKE pat;
  UPDATE snapshots SET doc = remint_doc(doc, p_from, p_to)
   WHERE tenant_id = p_tenant AND doc::text LIKE pat;
  UPDATE entity_map_docs SET doc = remint_doc(doc, p_from, p_to)
   WHERE tenant_id = p_tenant AND doc::text LIKE pat;
  UPDATE dist_subs SET doc = remint_doc(doc, p_from, p_to)
   WHERE tenant_id = p_tenant AND doc::text LIKE pat;

  -- an id renamed before keeps resolving to the newest one
  UPDATE entity_id_remap SET new_id = remint_id(new_id, p_from, p_to)
   WHERE tenant_id = p_tenant AND new_id LIKE pat;

  -- what still names `from` is no id: a pattern or free text to read
  SELECT (SELECT count(*) FROM subscriptions WHERE tenant_id = p_tenant AND subscription::text LIKE pat)
       + (SELECT count(*) FROM csource_subscriptions WHERE tenant_id = p_tenant AND subscription::text LIKE pat)
       + (SELECT count(*) FROM csource_registrations WHERE tenant_id = p_tenant AND registration::text LIKE pat)
       + (SELECT count(*) FROM csource_index WHERE tenant_id = p_tenant AND id_pattern LIKE pat)
       + (SELECT count(*) FROM views WHERE tenant_id = p_tenant AND doc::text LIKE pat)
    INTO n;
  RETURN out || jsonb_build_object('selectors_still_naming_from', n);
END $$;

-- The range ends are every p_batch-th Entity id of the Tenant, read once:
-- a range holds about p_batch Entities with their history and the other
-- rows keyed by an id in it. ponytail: a range is sized by Entities, so
-- one Entity with millions of history rows makes one long range; split by
-- history rows if a Tenant ever has one.
CREATE PROCEDURE antares_remint_batched(p_tenant text, p_from text, p_to text,
                                        p_batch integer DEFAULT 1000,
                                        INOUT result jsonb DEFAULT NULL)
  LANGUAGE plpgsql
  AS $$
DECLARE
  lo text;
  hi text;
  r jsonb;
  batches integer := 0;
  tries integer;
BEGIN
  PERFORM remint_guard(p_tenant, p_from, p_to);
  IF p_from = p_to THEN
    RAISE EXCEPTION 'from and to are both %', p_from;
  END IF;
  IF p_batch IS NULL OR p_batch < 1 THEN
    RAISE EXCEPTION 'batch is the number of Entity ids per transaction, at least 1: got %', p_batch;
  END IF;
  -- every clash before the first rename, as one transaction would
  PERFORM remint_map_build(p_tenant, p_from, p_to, NULL, NULL);
  result := jsonb_build_object('tenant', p_tenant, 'from', p_from, 'to', p_to);
  COMMIT;
  -- the last range is open above (hi NULL) and takes the unkeyed tables
  FOR hi IN
    SELECT s.id FROM (SELECT id, row_number() OVER (ORDER BY id) AS k
                        FROM entities WHERE tenant_id = p_tenant) s
     WHERE s.k % p_batch = 0
    UNION ALL SELECT NULL
    ORDER BY 1 NULLS LAST
  LOOP
    -- a writer taking its table locks in another order can deadlock with
    -- the range's LOCK TABLE, and a long one runs out lock_timeout: the
    -- range rolls back alone and is tried again
    tries := 0;
    LOOP
      BEGIN
        r := remint_apply(p_tenant, p_from, p_to, true, lo, hi);
        EXIT;
      EXCEPTION WHEN deadlock_detected OR lock_not_available THEN
        tries := tries + 1;
        IF tries >= 5 THEN
          RAISE;
        END IF;
        RAISE NOTICE 'remint %: range % waited on a writer (%), try % of 5',
          p_tenant, batches + 1, SQLERRM, tries + 1;
        PERFORM pg_sleep(tries);
      END;
    END LOOP;
    result := result || (SELECT jsonb_object_agg(key, coalesce((result ->> key)::bigint, 0) + value::bigint)
                           FROM jsonb_each_text(r) WHERE value ~ '^[0-9]+$');
    batches := batches + 1;
    COMMIT;
    RAISE NOTICE 'remint %: range % committed, % ids so far', p_tenant, batches, result ->> 'ids';
    lo := hi;
  END LOOP;
  result := result || jsonb_build_object('batches', batches);
END $$;

REVOKE EXECUTE ON FUNCTION remint_map_build(text, text, text, text, text),
                           remint_apply(text, text, text, boolean, text, text) FROM PUBLIC;
REVOKE EXECUTE ON PROCEDURE antares_remint_batched(text, text, text, integer, jsonb) FROM PUBLIC;
