-- Re-mint Entity ids to the owner prefix (T-6770, ADR-N-068). Every URN
-- names the Organization owning the data, `urn:ngsi-ld:{Type}:{orgId}:...`
-- with orgId the verified domain reversed (`sk_bbsk`); ids written before
-- carry the dotted domain (`urn:ngsi-ld:KPI:bbsk.sk:...`).
--
-- antares_remint(tenant, from, to) renames, in the caller's transaction, every
-- id of one Tenant whose fourth segment is `from`: the Entity, its temporal
-- Entity, its history in both parents (attr_rows through iri_ids), its
-- superseded versions, its entity_values keys, and every string anywhere in a
-- stored document that is such an id (Relationship objects, objectList,
-- Subscription and registration selectors, entity maps). It writes the map
-- old -> new into entity_id_remap, which the broker reads to answer a GET of
-- an old id with the Entity under its new id for twelve months.
-- antares_remint_rollback reverses it from the map; antares_remint_digest
-- gives per table a row count and an order-free hash of the rows as the
-- remint leaves them, so a rehearsal compares before and after.
--
-- Nothing calls these functions: the deployer runs them by hand, after a
-- pg_dump, as a role that bypasses row-level security (runbook:
-- docs/src/operations.md "Re-minting Entity ids"). They refuse any other
-- role, so the broker's own role can never rename ids. A regular expression (idPattern, a Policy) is no id
-- and stays as written; the result counts the documents still naming `from`.

CREATE TABLE entity_id_remap (
  tenant_id   text NOT NULL,
  old_id      text NOT NULL,
  new_id      text NOT NULL,
  remapped_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, old_id)
);
-- 0005's per-command policies. No UPDATE policy: only the remint, outside
-- row-level security, moves a row.
ALTER TABLE entity_id_remap ENABLE ROW LEVEL SECURITY;
ALTER TABLE entity_id_remap FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_read ON entity_id_remap FOR SELECT
  USING (tenant_id = current_setting('antares.tenant', true));
CREATE POLICY tenant_reap ON entity_id_remap FOR DELETE
  USING (tenant_id = current_setting('antares.tenant', true));
CREATE POLICY tenant_write ON entity_id_remap FOR INSERT
  WITH CHECK (tenant_id = current_setting('antares.tenant', true));

-- Every string of a document that is an NGSI-LD URN with `from` as its
-- fourth segment gets `to` there. A string starts at a `"` no backslash
-- escapes; the segment ends at `:` or at the string's end. Callers check
-- that both segments match [A-Za-z0-9._-]+, so neither needs quoting
-- beyond the regex dot and dash.
CREATE FUNCTION remint_doc(p_doc jsonb, p_from text, p_to text) RETURNS jsonb
  LANGUAGE sql IMMUTABLE PARALLEL SAFE
  AS $$
  SELECT CASE WHEN p_doc IS NULL OR p_from = p_to THEN p_doc ELSE
    regexp_replace(p_doc::text,
      '(^|[^\\])"urn:ngsi-ld:([^":\\]+):'
        || regexp_replace(p_from, '([.\-])', '\\\1', 'g') || '([:"])',
      '\1"urn:ngsi-ld:\2:' || p_to || '\3', 'g')::jsonb END
$$;

CREATE FUNCTION remint_id(p_id text, p_from text, p_to text) RETURNS text
  LANGUAGE sql IMMUTABLE PARALLEL SAFE
  AS $$ SELECT remint_doc(to_jsonb(p_id), p_from, p_to) #>> '{}' $$;

CREATE FUNCTION remint_guard(p_tenant text, p_from text, p_to text) RETURNS void
  LANGUAGE plpgsql
  AS $$
BEGIN
  IF NOT coalesce((SELECT rolsuper OR rolbypassrls FROM pg_roles WHERE rolname = current_user), false) THEN
    RAISE EXCEPTION 'the remint reads and writes every row of a Tenant: run it as a role that bypasses row-level security (the database owner), not as %', current_user;
  END IF;
  IF p_from !~ '^[A-Za-z0-9][A-Za-z0-9._-]*$' OR p_to !~ '^[A-Za-z0-9][A-Za-z0-9._-]*$' THEN
    RAISE EXCEPTION 'owner segments are letters, digits, dot, dash and underscore: got % and %', p_from, p_to;
  END IF;
  IF NOT EXISTS (SELECT 1 FROM tenants WHERE tenant_id = p_tenant) THEN
    RAISE EXCEPTION 'no Tenant %', p_tenant;
  END IF;
END $$;

CREATE FUNCTION remint_apply(p_tenant text, p_from text, p_to text, p_record boolean)
  RETURNS jsonb LANGUAGE plpgsql
  AS $$
DECLARE
  pat text := '%' || p_from || '%';
  out jsonb := jsonb_build_object('tenant', p_tenant, 'from', p_from, 'to', p_to);
  n bigint;
  clash text;
BEGIN
  IF p_from = p_to THEN
    RAISE EXCEPTION 'from and to are both %', p_from;
  END IF;
  PERFORM set_config('lock_timeout', '30s', true);
  -- writers wait until the transaction ends; readers go on
  LOCK TABLE entities, temporal_entities, attr_instances, attr_rows, attr_versions,
             iri_ids, entity_values, subscriptions, csource_subscriptions,
             csource_registrations, csource_index, views, snapshots,
             entity_map_docs, dist_subs, entity_id_remap IN EXCLUSIVE MODE;

  DROP TABLE IF EXISTS pg_temp.remint_map;
  CREATE TEMP TABLE remint_map (old_id text PRIMARY KEY, new_id text NOT NULL UNIQUE)
    ON COMMIT DROP;
  INSERT INTO remint_map
    SELECT id, remint_id(id, p_from, p_to) FROM (
      SELECT id FROM entities WHERE tenant_id = p_tenant
      UNION SELECT id FROM temporal_entities WHERE tenant_id = p_tenant
      UNION SELECT entity_id FROM attr_instances WHERE tenant_id = p_tenant
      UNION SELECT entity_id FROM attr_versions WHERE tenant_id = p_tenant
      UNION SELECT iri FROM iri_ids WHERE tenant_id = p_tenant AND iri LIKE 'urn:ngsi-ld:%'
    ) s(id)
    WHERE id LIKE 'urn:ngsi-ld:%' AND remint_id(id, p_from, p_to) <> id;
  GET DIAGNOSTICS n = ROW_COUNT;
  out := out || jsonb_build_object('ids', n);

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

  UPDATE entities SET id = remint_id(id, p_from, p_to), entity = remint_doc(entity, p_from, p_to)
   WHERE tenant_id = p_tenant
     AND (id IN (SELECT old_id FROM remint_map) OR entity::text LIKE pat);
  GET DIAGNOSTICS n = ROW_COUNT;
  out := out || jsonb_build_object('entities', n);
  -- the update trigger pairs old and new rows by id, so a renamed Entity's
  -- keys are rebuilt here; a document that only names one is the trigger's
  DELETE FROM entity_values v USING remint_map m
   WHERE v.tenant_id = p_tenant AND v.id = m.old_id;
  INSERT INTO entity_values (tenant_id, id, attr, f, txt)
    SELECT e.tenant_id, e.id, r.attr, r.f, r.txt
      FROM entities e JOIN remint_map m ON e.id = m.new_id,
           entity_value_rows(e.entity) r
     WHERE e.tenant_id = p_tenant;

  UPDATE temporal_entities SET id = remint_id(id, p_from, p_to), meta = remint_doc(meta, p_from, p_to)
   WHERE tenant_id = p_tenant
     AND (id IN (SELECT old_id FROM remint_map) OR meta::text LIKE pat);
  GET DIAGNOSTICS n = ROW_COUNT;
  out := out || jsonb_build_object('temporal_entities', n);

  UPDATE attr_instances
     SET entity_id = remint_id(entity_id, p_from, p_to), data = remint_doc(data, p_from, p_to)
   WHERE tenant_id = p_tenant
     AND (entity_id IN (SELECT old_id FROM remint_map) OR data::text LIKE pat);
  GET DIAGNOSTICS n = ROW_COUNT;
  out := out || jsonb_build_object('attr_instances', n);

  UPDATE attr_versions
     SET entity_id = remint_id(entity_id, p_from, p_to), data = remint_doc(data, p_from, p_to)
   WHERE tenant_id = p_tenant
     AND (entity_id IN (SELECT old_id FROM remint_map) OR data::text LIKE pat);
  GET DIAGNOSTICS n = ROW_COUNT;
  out := out || jsonb_build_object('attr_versions', n);

  -- attr_rows keys its Entity by iri_ids number: renaming the IRI renames
  -- every row; only the values naming an id are rewritten
  UPDATE iri_ids i SET iri = m.new_id FROM remint_map m
   WHERE i.tenant_id = p_tenant AND i.iri = m.old_id;
  GET DIAGNOSTICS n = ROW_COUNT;
  out := out || jsonb_build_object('iri_ids', n);
  UPDATE attr_rows SET data = remint_doc(data, p_from, p_to)
   WHERE tenant_id = p_tenant AND data::text LIKE pat;
  GET DIAGNOSTICS n = ROW_COUNT;
  out := out || jsonb_build_object('attr_rows', n);

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
  IF p_record THEN
    INSERT INTO entity_id_remap (tenant_id, old_id, new_id)
      SELECT p_tenant, old_id, new_id FROM remint_map
    ON CONFLICT (tenant_id, old_id) DO UPDATE
      SET new_id = EXCLUDED.new_id, remapped_at = now();
  END IF;

  -- what still names `from` is no id: a pattern or free text to read
  SELECT (SELECT count(*) FROM subscriptions WHERE tenant_id = p_tenant AND subscription::text LIKE pat)
       + (SELECT count(*) FROM csource_subscriptions WHERE tenant_id = p_tenant AND subscription::text LIKE pat)
       + (SELECT count(*) FROM csource_registrations WHERE tenant_id = p_tenant AND registration::text LIKE pat)
       + (SELECT count(*) FROM csource_index WHERE tenant_id = p_tenant AND id_pattern LIKE pat)
       + (SELECT count(*) FROM views WHERE tenant_id = p_tenant AND doc::text LIKE pat)
    INTO n;
  RETURN out || jsonb_build_object('selectors_still_naming_from', n);
END $$;

CREATE FUNCTION antares_remint(p_tenant text, p_from text, p_to text) RETURNS jsonb
  LANGUAGE plpgsql
  AS $$
BEGIN
  PERFORM remint_guard(p_tenant, p_from, p_to);
  RETURN remint_apply(p_tenant, p_from, p_to, true);
END $$;

-- The reverse from the map: refused when an id carrying `to` is not one the
-- remint wrote (written since, or renamed by another run), because renaming
-- it back would invent an id nobody minted; restore the pg_dump instead.
CREATE FUNCTION antares_remint_rollback(p_tenant text, p_from text, p_to text) RETURNS jsonb
  LANGUAGE plpgsql
  AS $$
DECLARE
  stray text;
  out jsonb;
BEGIN
  PERFORM remint_guard(p_tenant, p_from, p_to);
  SELECT id INTO stray FROM (
      SELECT id FROM entities WHERE tenant_id = p_tenant
      UNION SELECT id FROM temporal_entities WHERE tenant_id = p_tenant
      UNION SELECT iri FROM iri_ids WHERE tenant_id = p_tenant AND iri LIKE 'urn:ngsi-ld:%'
    ) s(id)
   WHERE remint_id(id, p_to, p_from) <> id
     AND NOT EXISTS (SELECT 1 FROM entity_id_remap r
                      WHERE r.tenant_id = p_tenant AND r.new_id = s.id
                        AND remint_id(r.old_id, p_from, p_to) = s.id)
   LIMIT 1;
  IF stray IS NOT NULL THEN
    RAISE EXCEPTION 'Tenant % holds %, which the remint did not write: restore the pg_dump instead', p_tenant, stray;
  END IF;
  out := remint_apply(p_tenant, p_to, p_from, false);
  DELETE FROM entity_id_remap WHERE tenant_id = p_tenant AND old_id = new_id;
  RETURN out;
END $$;

-- Per table: rows and an order-free sum of row hashes, every id and
-- document as remint_doc(from -> to) leaves it. Before a remint run it with
-- (from, to), after it with (to, to): equal numbers mean the remint changed
-- nothing but the ids.
CREATE FUNCTION antares_remint_digest(p_tenant text, p_from text, p_to text)
  RETURNS TABLE (tbl text, row_count bigint, digest numeric)
  LANGUAGE plpgsql
  AS $$
BEGIN
  PERFORM remint_guard(p_tenant, p_from, p_to);
  RETURN QUERY
  WITH h(tbl, r) AS (
    SELECT 'entities', concat_ws('|', remint_id(id, p_from, p_to), remint_doc(entity, p_from, p_to),
                                version, types, scopes, created_at, modified_at, expires_at)
      FROM entities WHERE tenant_id = p_tenant
    UNION ALL
    SELECT 'temporal_entities', concat_ws('|', remint_id(id, p_from, p_to), remint_doc(meta, p_from, p_to),
                                          types, scopes, created_at, modified_at, deleted_at)
      FROM temporal_entities WHERE tenant_id = p_tenant
    UNION ALL
    SELECT 'attr_instances', concat_ws('|', remint_id(entity_id, p_from, p_to), attr_id, instance_id,
                                       dataset_id, observed_at, version, remint_doc(data, p_from, p_to))
      FROM attr_instances WHERE tenant_id = p_tenant
    UNION ALL
    SELECT 'attr_versions', concat_ws('|', remint_id(entity_id, p_from, p_to), attr_id, instance_id,
                                      observed_at, version, remint_doc(data, p_from, p_to))
      FROM attr_versions WHERE tenant_id = p_tenant
    UNION ALL
    SELECT 'attr_rows', concat_ws('|', remint_id(i.iri, p_from, p_to), a.attr_no, a.instance_no,
                                  a.observed_at, a.version, a.short, remint_doc(a.data, p_from, p_to))
      FROM attr_rows a JOIN iri_ids i ON i.tenant_id = a.tenant_id AND i.id = a.entity_no
     WHERE a.tenant_id = p_tenant
    UNION ALL
    SELECT 'entity_values', concat_ws('|', remint_id(id, p_from, p_to), attr, f,
                                      remint_id(txt, p_from, p_to))
      FROM entity_values WHERE tenant_id = p_tenant
    UNION ALL
    SELECT 'subscriptions', concat_ws('|', id, remint_doc(subscription, p_from, p_to))
      FROM subscriptions WHERE tenant_id = p_tenant
    UNION ALL
    SELECT 'csource_registrations', concat_ws('|', id, remint_doc(registration, p_from, p_to))
      FROM csource_registrations WHERE tenant_id = p_tenant
  )
  SELECT h.tbl, count(*), coalesce(sum(('x' || substr(md5(h.r), 1, 15))::bit(60)::bigint), 0)::numeric
    FROM h GROUP BY h.tbl ORDER BY h.tbl;
END $$;

REVOKE EXECUTE ON FUNCTION remint_guard(text, text, text),
                           remint_apply(text, text, text, boolean),
                           antares_remint(text, text, text),
                           antares_remint_rollback(text, text, text),
                           antares_remint_digest(text, text, text) FROM PUBLIC;
