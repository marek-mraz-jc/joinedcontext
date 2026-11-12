-- The per-range statements of 0034 bounded by the range (T-6830 reopened).
-- jc-dev copy, praha-mesto, 1.19 M ids: a range took 6.7 s and grew with
-- the Tenant (helsinki 2-3 s), because a statement joined the range's
-- remint_map to every row of the Tenant:
-- - entity_values: a Merge Join whose outer side was the index scan of
--   (tenant_id, id) from the Tenant's first id, 9.66 M rows read to rename
--   16.9 k (40 s of a cold 67 s range). Every map id lies in [p_lo, p_hi),
--   so the bound on v.id changes no row and makes the scan a range scan.
-- - iri_ids: the same Merge Join shape, bounded the same way.
-- - attr_rows: `(both ends NULL) OR entity_no IN (...)`. The planner pulls
--   an IN up into a semi-join before it folds constants, so the OR left a
--   hashed SubPlan and a sequential scan of attr_rows_short (177 k rows of
--   every Tenant, 0.3 s a range); split into two statements.
-- The batched procedure reads its range ends once: a FOR over a query in a
-- procedure that commits holds the whole result from the first COMMIT.
-- Only remint_apply changes; 0034 stays as jc-dev applied it.

CREATE OR REPLACE FUNCTION remint_apply(p_tenant text, p_from text, p_to text, p_record boolean,
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
  -- two statements: an OR around the IN keeps it a hashed SubPlan over
  -- every row of the partition, where alone it is a semi-join on the index
  IF p_lo IS NULL AND p_hi IS NULL THEN
    UPDATE attr_rows SET data = remint_doc(data, p_from, p_to)
     WHERE tenant_id = p_tenant AND data::text LIKE pat;
  ELSE
    UPDATE attr_rows SET data = remint_doc(data, p_from, p_to)
     WHERE tenant_id = p_tenant AND data::text LIKE pat
       AND entity_no IN (SELECT id FROM iri_ids WHERE tenant_id = p_tenant
                            AND (p_lo IS NULL OR iri >= p_lo) AND (p_hi IS NULL OR iri < p_hi));
  END IF;
  GET DIAGNOSTICS n = ROW_COUNT;
  out := out || jsonb_build_object('attr_rows', n);
  UPDATE iri_ids i SET iri = m.new_id FROM remint_map m
   WHERE i.tenant_id = p_tenant AND i.iri = m.old_id
     AND (p_lo IS NULL OR i.iri >= p_lo) AND (p_hi IS NULL OR i.iri < p_hi);
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
   WHERE v.tenant_id = p_tenant AND v.id = m.old_id
     AND (p_lo IS NULL OR v.id >= p_lo) AND (p_hi IS NULL OR v.id < p_hi);

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

REVOKE EXECUTE ON FUNCTION remint_apply(text, text, text, boolean, text, text) FROM PUBLIC;
