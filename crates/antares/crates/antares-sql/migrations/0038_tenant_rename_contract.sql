-- The Tenant rename keeps every sealed credential readable and rewrites
-- only the members that name a Tenant (T-7274, review S5-01, S5-19; TEN-15).
--
-- tenant_seal_alias: per Tenant, the names its rows held before a rename.
-- A sealed receiverInfo value is bound to its row's Tenant name (store/pg
-- seal.rs `Row::aad`), and the rename runs here, where the key is not, so it
-- cannot reseal. The broker opens a value that fails under the current name
-- by these names, reseals such rows at boot (`PgDocStore::seal_plaintext`)
-- and then deletes the names. The rename and its rollback both write it,
-- in `tenant_rename_apply`, so a value sealed between a rename and its
-- rollback opens too. It holds Tenant names only and, like `tenant_rename`,
-- has no row-level security.
--
-- tenant_contract replaces tenant_token for the subscription, registration
-- and source-index rows of every Tenant: 0037 rewrote each JSON string equal
-- to the old name there, keys, attribute names and other Tenants' rows
-- included. Now only these members move, in any Tenant's rows (a
-- registration of one Tenant may point at another by `tenant`):
-- - a registration's `tenant`;
-- - an `NGSILD-Tenant` pair (key compared case-blind) of a subscription's
--   `notification.endpoint.receiverInfo`, a registration's
--   `contextSourceInfo` and the index's copy of it (`csource_index.headers`);
-- - `csource_index.tenant_at_peer` (unchanged from 0037).
-- A sealed `NGSILD-Tenant` value is renamed by the broker when it opens it
-- by a former name. The broker's own Tenants' documents keep 0037's rule.

CREATE TABLE tenant_seal_alias (
  tenant text NOT NULL,
  former text NOT NULL,
  PRIMARY KEY (tenant, former)
);

-- An array of {key, value} pairs with each `NGSILD-Tenant` value `p_from`
-- turned into `p_to`; anything else as it came.
CREATE FUNCTION tenant_pairs(p_pairs jsonb, p_from text, p_to text) RETURNS jsonb
  LANGUAGE sql IMMUTABLE PARALLEL SAFE
  AS $$
  SELECT CASE WHEN p_pairs IS NULL OR jsonb_typeof(p_pairs) <> 'array' THEN p_pairs ELSE
    (SELECT coalesce(jsonb_agg(
              CASE WHEN jsonb_typeof(p) = 'object' AND lower(p ->> 'key') = 'ngsild-tenant'
                        AND p -> 'value' = to_jsonb(p_from)
                   THEN jsonb_set(p, '{value}', to_jsonb(p_to)) ELSE p END ORDER BY o), '[]'::jsonb)
       FROM jsonb_array_elements(p_pairs) WITH ORDINALITY e(p, o)) END
$$;

-- `p_doc`, a document of table `p_tbl`, with the members that name Tenant
-- `p_from` by contract naming `p_to`.
CREATE FUNCTION tenant_contract(p_tbl text, p_doc jsonb, p_from text, p_to text) RETURNS jsonb
  LANGUAGE sql IMMUTABLE PARALLEL SAFE
  AS $$
  SELECT CASE
    WHEN p_doc IS NULL OR p_from = p_to OR jsonb_typeof(p_doc) NOT IN ('object', 'array') THEN p_doc
    WHEN p_tbl IN ('subscriptions', 'csource_subscriptions')
         AND jsonb_typeof(p_doc #> '{notification,endpoint,receiverInfo}') = 'array'
      THEN jsonb_set(p_doc, '{notification,endpoint,receiverInfo}',
                     tenant_pairs(p_doc #> '{notification,endpoint,receiverInfo}', p_from, p_to))
    WHEN p_tbl = 'csource_registrations' AND jsonb_typeof(p_doc) = 'object' THEN
      (SELECT CASE WHEN jsonb_typeof(d -> 'contextSourceInfo') = 'array'
                   THEN jsonb_set(d, '{contextSourceInfo}', tenant_pairs(d -> 'contextSourceInfo', p_from, p_to))
                   ELSE d END
         FROM (SELECT CASE WHEN p_doc -> 'tenant' = to_jsonb(p_from)
                           THEN jsonb_set(p_doc, '{tenant}', to_jsonb(p_to)) ELSE p_doc END AS d) s)
    WHEN p_tbl = 'csource_index' THEN tenant_pairs(p_doc, p_from, p_to)
    ELSE p_doc END
$$;

CREATE OR REPLACE FUNCTION tenant_rename_apply(p_old text, p_new text) RETURNS jsonb
  LANGUAGE plpgsql
  AS $$
DECLARE
  out jsonb := jsonb_build_object('old', p_old, 'new', p_new);
  t text;
  c text;
  n bigint;
  total bigint;
  tok text := '%"' || p_old || '"%';
  super boolean;
  deadline timestamptz;
  attempt timestamptz;
BEGIN
  PERFORM tenant_rename_guard(p_old, p_new);
  -- `entities`' statement triggers keep the catalogue and `entity_values` of
  -- a written document; the rename moves both itself, so the triggers would
  -- only copy every moved row twice into transition tables (7.5 GB of temp
  -- files for a 209 k entity Tenant of jc-dev). A superuser skips them for
  -- this transaction; a BYPASSRLS owner runs them, slower, with the same result.
  -- The foreign keys' own triggers go with them: csource_index follows its
  -- registrations by the UPDATE below instead of the cascade.
  super := (SELECT rolsuper FROM pg_roles WHERE rolname = current_user);
  IF super THEN
    PERFORM set_config('session_replication_role', 'replica', true);
  END IF;
  -- writers wait until the transaction ends; readers go on. One attempt
  -- takes every table within 300 ms or releases what it took (the
  -- subtransaction): a writer that waits on a table the attempt holds starts
  -- its deadlock check after deadlock_timeout (1 s by default), when the
  -- attempt has let go, so the rename never deadlocks a writer that locks
  -- entities, then the catalogue. A table-by-table wait did (40P01).
  deadline := clock_timestamp() + interval '60 seconds';
  LOOP
    BEGIN
      attempt := clock_timestamp() + interval '300 milliseconds';
      FOR t IN SELECT tbl FROM tenant_tables() UNION SELECT 'tenants' UNION SELECT 'jsonld_contexts' ORDER BY 1 LOOP
        PERFORM set_config('lock_timeout',
          greatest(1, ceil(extract(epoch FROM attempt - clock_timestamp()) * 1000))::int || 'ms', true);
        EXECUTE format('LOCK TABLE %I IN EXCLUSIVE MODE', t);
      END LOOP;
      EXIT;
    EXCEPTION WHEN lock_not_available THEN
      IF clock_timestamp() > deadline THEN
        RAISE EXCEPTION 'writers held the tables of Tenant % for 60 s', p_old
          USING ERRCODE = 'lock_not_available',
                HINT = 'run the rename again in a quieter window, or stop the brokers first';
      END IF;
      PERFORM pg_sleep(0.05 + random() * 0.2);
    END;
  END LOOP;
  PERFORM set_config('lock_timeout', '30s', true);

  FOR t IN SELECT tbl FROM tenant_tables() LOOP
    EXECUTE format('UPDATE %I SET tenant_id = $2 WHERE tenant_id = $1', t) USING p_old, p_new;
    GET DIAGNOSTICS n = ROW_COUNT;
    IF n > 0 THEN
      out := out || jsonb_build_object(t, n);
    END IF;
  END LOOP;
  -- `entities`' catalogue trigger added the new Tenant's rows the catalogue
  -- moved already: none of the old name are left to move or to delete
  UPDATE jsonld_contexts SET body = jsonb_set(body, '{owner}', to_jsonb(p_new))
   WHERE kind <> 'Cached' AND body ->> 'owner' = p_old;
  GET DIAGNOSTICS n = ROW_COUNT;
  out := out || jsonb_build_object('jsonld_contexts', n);
  -- the documents below are rewritten in place: their triggers run again
  IF super THEN
    PERFORM set_config('session_replication_role', 'origin', true);
  END IF;

  -- documents naming the Tenant: the broker's own Tenants, every column
  total := 0;
  FOR t, c IN
    SELECT a.attrelid::regclass::text, a.attname::text
      FROM pg_attribute a JOIN tenant_tables() tt ON a.attrelid = tt.tbl::regclass
     WHERE a.attnum > 0 AND NOT a.attisdropped AND a.atttypid = 'jsonb'::regtype AND a.attgenerated = ''
  LOOP
    CONTINUE WHEN t NOT IN (SELECT tbl FROM tenant_naming_tables()) AND NOT tenant_reserved_held(t);
    -- every Tenant's subscriptions and registrations: the members that name a
    -- Tenant by contract; the broker's own documents: every equal token
    IF t IN (SELECT tbl FROM tenant_naming_tables()) THEN
      EXECUTE format(
        'UPDATE %1$I SET %2$I = tenant_contract(%3$L, %2$I, $1, $2)
          WHERE %2$I::text LIKE $3 AND tenant_contract(%3$L, %2$I, $1, $2) IS DISTINCT FROM %2$I',
        t, c, t) USING p_old, p_new, tok;
    ELSE
      EXECUTE format(
        'UPDATE %I SET %I = tenant_token(%I, $1, $2) WHERE %s AND %I::text LIKE $3', t, c, c,
        reserved_rows(), c) USING p_old, p_new, tok;
    END IF;
    GET DIAGNOSTICS n = ROW_COUNT;
    total := total + n;
  END LOOP;
  -- a broker document keyed by its Tenant (antares-policy)
  FOR t IN
    SELECT tt.tbl FROM tenant_tables() tt
      JOIN pg_attribute a ON a.attrelid = tt.tbl::regclass AND a.attname = 'id' AND a.atttypid = 'text'::regtype
  LOOP
    CONTINUE WHEN NOT tenant_reserved_held(t);
    EXECUTE format('UPDATE %I SET id = $2 WHERE %s AND id = $1', t, reserved_rows())
      USING p_old, p_new;
    GET DIAGNOSTICS n = ROW_COUNT;
    total := total + n;
  END LOOP;
  UPDATE csource_index SET tenant_at_peer = p_new WHERE tenant_at_peer = p_old;
  GET DIAGNOSTICS n = ROW_COUNT;
  out := out || jsonb_build_object('documents', total + n);

  -- the names the moved rows' sealed credentials may be bound to: the old
  -- one and those it carried from earlier renames
  UPDATE tenant_seal_alias SET tenant = p_new WHERE tenant = p_old;
  INSERT INTO tenant_seal_alias (tenant, former) VALUES (p_new, p_old) ON CONFLICT DO NOTHING;
  DELETE FROM tenant_seal_alias WHERE tenant = former;

  UPDATE tenants SET tenant_id = p_new, organization_id = tenant_organization(p_new)
   WHERE tenant_id = p_old;
  RETURN out;
END $$;

CREATE OR REPLACE FUNCTION antares_tenant_digest(p_tenant text)
  RETURNS TABLE (tbl text, row_count bigint, digest numeric)
  LANGUAGE plpgsql
  AS $$
DECLARE
  t text;
  cols text;
  naming boolean;
  has_id boolean;
  rc bigint;
  dg numeric;
BEGIN
  IF NOT coalesce((SELECT rolsuper OR rolbypassrls FROM pg_roles WHERE rolname = current_user), false) THEN
    RAISE EXCEPTION 'the digest reads every row of a Tenant: run it as a role that bypasses row-level security, not as %', current_user;
  END IF;
  IF p_tenant !~ '^[A-Za-z0-9_-]{1,128}$' THEN
    RAISE EXCEPTION 'not a Tenant name: %', p_tenant;
  END IF;
  FOR t IN SELECT tt.tbl FROM tenant_tables() tt UNION ALL SELECT 'jsonld_contexts' LOOP
    naming := t IN (SELECT n.tbl FROM tenant_naming_tables() n);
    SELECT string_agg(
             CASE WHEN t = 'jsonld_contexts' AND a.attname = 'body'
                  THEN 'jsonb_set(body, ''{owner}'', ''"<tenant>"'')::text'
                  WHEN naming AND a.atttypid = 'jsonb'::regtype
                  THEN format('tenant_contract(%L, %I, $1, %L)::text', t, a.attname, '<tenant>')
                  WHEN t = 'csource_index' AND a.attname = 'tenant_at_peer'
                  THEN 'CASE WHEN tenant_at_peer = $1 THEN ''<tenant>'' ELSE tenant_at_peer END'
                  ELSE format('%I::text', a.attname) END, ', ' ORDER BY a.attnum)
      INTO cols
      FROM pg_attribute a
     WHERE a.attrelid = t::regclass AND a.attnum > 0 AND NOT a.attisdropped
       AND a.attname NOT IN ('tenant_id', 'digest');
    EXECUTE format(
      'SELECT count(*), coalesce(sum((''x'' || substr(md5(concat_ws(''|'', %s)), 1, 15))::bit(60)::bigint), 0)::numeric
         FROM %I WHERE tenant_id = $1', cols, t)
      INTO rc, dg USING p_tenant;
    IF rc > 0 THEN
      tbl := t; row_count := rc; digest := dg;
      RETURN NEXT;
    END IF;
  END LOOP;
  FOR t, has_id IN
    SELECT tt.tbl, EXISTS (SELECT 1 FROM pg_attribute a WHERE a.attrelid = tt.tbl::regclass
                                                       AND a.attname = 'id' AND a.atttypid = 'text'::regtype)
      FROM tenant_tables() tt
  LOOP
    CONTINUE WHEN NOT tenant_reserved_held(t);
    SELECT string_agg(
             CASE WHEN a.atttypid = 'jsonb'::regtype
                  THEN format('tenant_token(%I, $1, %L)::text', a.attname, '<tenant>')
                  WHEN a.attname = 'id' AND a.atttypid = 'text'::regtype
                  THEN 'CASE WHEN id = $1 THEN ''<tenant>'' ELSE id END'
                  ELSE format('%I::text', a.attname) END, ', ' ORDER BY a.attnum)
      INTO cols
      FROM pg_attribute a
     WHERE a.attrelid = t::regclass AND a.attnum > 0 AND NOT a.attisdropped
       AND a.attname NOT IN ('digest');
    EXECUTE format(
      'SELECT count(*), coalesce(sum((''x'' || substr(md5(concat_ws(''|'', %s)), 1, 15))::bit(60)::bigint), 0)::numeric
         FROM %I WHERE %s AND (%I::text LIKE $2 %s)', cols, t, reserved_rows(), t,
      CASE WHEN has_id THEN 'OR id = $1' ELSE '' END)
      INTO rc, dg USING p_tenant, '%"' || p_tenant || '"%';
    IF rc > 0 THEN
      tbl := 'reserved:' || t; row_count := rc; digest := dg;
      RETURN NEXT;
    END IF;
  END LOOP;
END $$;
