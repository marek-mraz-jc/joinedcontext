-- Rename a Tenant (T-7039, PF-130, ARCH/TEN-10, TEN-15): every row of one
-- client Tenant moves to a new name, `{OrgId}__{ProjectId}__{space}` from the
-- old space segment, with its history, subscriptions, registrations and
-- hosted @contexts, in one transaction per Tenant.
--
-- antares_tenant_rename(old, new) moves:
-- - `tenant_id` of every table that has one (partitioned parents included, so
--   every attr_rows and attr_instances partition, attr_rows_short too), the
--   `tenants` row and its `organization_id` (0036) last;
-- - `jsonld_contexts`, whose `tenant_id` is GENERATED from `body.owner`, by
--   its owner;
-- - each JSON string equal to `old` (an exact token, never a substring) in
--   the documents that name a Tenant: every document of the broker's own
--   Tenants (`antares-views`, `antares-policy`, `antares-grants`,
--   `antares-channels`, `distsub-index`, `snap-*`: view, retention, erasure
--   and quality members, policy keys, grant source and target), and in every
--   Tenant the subscription, registration and source-index documents (the
--   `tenant` member of a registration, an `NGSILD-Tenant` header of a
--   receiverInfo), plus `csource_index.tenant_at_peer`;
-- - the `id` of a broker document named after the Tenant (an
--   `antares-policy` document is keyed by its Tenant).
-- Entities and their history are never rewritten: only their Tenant moves.
-- It writes `tenant_rename` (old, new), so the old name answers as an unknown
-- Tenant and is never created again (design 10.1), and refuses a `new` that
-- holds any row, is tombstoned, or is reserved.
--
-- antares_tenant_rename_rollback(new) moves a renamed Tenant back to the name
-- `tenant_rename` records and deletes that row. antares_tenant_digest(tenant)
-- gives per table a row count and an order-free hash of the Tenant's rows
-- with the Tenant name taken out, so a rehearsal compares the old name before
-- with the new name after: equal numbers mean nothing but the name moved.
--
-- Nothing calls these functions: the deployer runs them by hand, after a
-- pg_dump, as a role that bypasses row-level security (runbook:
-- docs/src/operations.md "Renaming Tenants"). They refuse any other role.

-- A registration's index rows follow it when its Tenant moves.
ALTER TABLE csource_index
  DROP CONSTRAINT csource_index_tenant_id_registration_id_fkey,
  ADD CONSTRAINT csource_index_tenant_id_registration_id_fkey
    FOREIGN KEY (tenant_id, registration_id) REFERENCES csource_registrations
    ON DELETE CASCADE ON UPDATE CASCADE;

CREATE FUNCTION tenant_reserved(p_tenant text) RETURNS boolean
  LANGUAGE sql IMMUTABLE PARALLEL SAFE
  AS $$ SELECT p_tenant LIKE 'snap-%' OR p_tenant IN
    ('distsub-index', 'antares-views', 'antares-policy', 'antares-grants', 'antares-channels') $$;

-- The broker's own Tenants as a condition on `tenant_id` an index can serve
-- (every table's key leads with it): `snap-*` as a range.
CREATE FUNCTION reserved_rows() RETURNS text
  LANGUAGE sql IMMUTABLE
  AS $$ SELECT '(tenant_id IN (''distsub-index'', ''antares-views'', ''antares-policy'', '
             || '''antares-grants'', ''antares-channels'') OR (tenant_id >= ''snap-'' AND tenant_id < ''snap.''))' $$;

-- Whether table `t` holds any reserved-Tenant row: an index probe that lets
-- the document passes skip a table such as `entity_values`, where the planner
-- would otherwise scan every row for the `::text LIKE` filter. One probe per
-- arm of `reserved_rows()`: under an OR the index serves no condition, and
-- statistics a rename left stale make a full scan look cheaper, so it is
-- ruled out.
CREATE FUNCTION tenant_reserved_held(t text) RETURNS boolean
  LANGUAGE plpgsql STABLE
  SET enable_seqscan = off
  AS $$
DECLARE
  held boolean;
BEGIN
  EXECUTE format(
    'SELECT EXISTS (SELECT 1 FROM %1$I WHERE tenant_id IN (''distsub-index'', ''antares-views'',
                    ''antares-policy'', ''antares-grants'', ''antares-channels''))
         OR EXISTS (SELECT 1 FROM %1$I WHERE tenant_id >= ''snap-'' AND tenant_id < ''snap.'')', t)
    INTO held;
  RETURN held;
END $$;

-- Every JSON string of `p_doc` equal to `p_from` becomes `p_to`; a string
-- starts at a `"` no backslash escapes. Callers check both names against
-- [A-Za-z0-9_-]+, where only the dash is special in a regex, and only inside
-- brackets.
CREATE FUNCTION tenant_token(p_doc jsonb, p_from text, p_to text) RETURNS jsonb
  LANGUAGE sql IMMUTABLE PARALLEL SAFE
  AS $$
  SELECT CASE WHEN p_doc IS NULL OR p_from = p_to THEN p_doc ELSE
    regexp_replace(p_doc::text, '(^|[^\\])"' || p_from || '"', '\1"' || p_to || '"', 'g')::jsonb END
$$;

-- The tables holding a `tenant_id` the rename sets, written in this order:
-- the catalogue tables before `entities`, whose statement trigger adds the
-- new Tenant's catalogue rows (ON CONFLICT DO NOTHING), and registrations
-- before their index, which follows by the foreign key.
CREATE FUNCTION tenant_tables() RETURNS TABLE (tbl text)
  LANGUAGE sql STABLE
  AS $$
  SELECT c.relname::text FROM pg_class c
    JOIN pg_namespace n ON n.oid = c.relnamespace AND n.nspname = current_schema()
    JOIN pg_attribute a ON a.attrelid = c.oid AND a.attname = 'tenant_id' AND a.attgenerated = ''
   WHERE c.relkind IN ('r', 'p') AND NOT c.relispartition
     AND c.relname NOT IN ('tenants', 'tenant_rename')
   ORDER BY CASE c.relname WHEN 'entity_attrs' THEN 0 WHEN 'entity_types' THEN 0
                           WHEN 'csource_registrations' THEN 1 ELSE 2 END, c.relname
$$;

-- The tables whose documents name a Tenant in every Tenant's rows.
CREATE FUNCTION tenant_naming_tables() RETURNS TABLE (tbl text)
  LANGUAGE sql IMMUTABLE
  AS $$ VALUES ('subscriptions'), ('csource_subscriptions'), ('csource_registrations'),
               ('csource_index') $$;

CREATE FUNCTION tenant_rename_guard(p_old text, p_new text) RETURNS void
  LANGUAGE plpgsql
  AS $$
DECLARE
  t text;
  held boolean;
BEGIN
  IF NOT coalesce((SELECT rolsuper OR rolbypassrls FROM pg_roles WHERE rolname = current_user), false) THEN
    RAISE EXCEPTION 'the rename reads and writes every row of a Tenant: run it as a role that bypasses row-level security (the database owner), not as %', current_user;
  END IF;
  IF p_old !~ '^[A-Za-z0-9_-]{1,128}$' OR p_new !~ '^[A-Za-z0-9_-]{1,128}$' THEN
    RAISE EXCEPTION 'Tenant names are 1 to 128 letters, digits, dash and underscore: got % and %', p_old, p_new;
  END IF;
  IF p_old = p_new THEN
    RAISE EXCEPTION 'old and new are both %', p_old;
  END IF;
  IF p_old = 'default' OR tenant_reserved(p_old) OR tenant_reserved(p_new) THEN
    RAISE EXCEPTION 'the default and the broker''s own Tenants keep their names: % -> %', p_old, p_new;
  END IF;
  IF NOT EXISTS (SELECT 1 FROM tenants WHERE tenant_id = p_old) THEN
    RAISE EXCEPTION 'no Tenant %', p_old;
  END IF;
  IF EXISTS (SELECT 1 FROM tenant_rename WHERE old = p_new) THEN
    RAISE EXCEPTION 'Tenant % was renamed away and is never used again', p_new;
  END IF;
  IF EXISTS (SELECT 1 FROM tenants WHERE tenant_id = p_new)
     OR EXISTS (SELECT 1 FROM jsonld_contexts WHERE tenant_id = p_new) THEN
    RAISE EXCEPTION 'Tenant % holds rows already: nothing renamed', p_new;
  END IF;
  FOR t IN SELECT tbl FROM tenant_tables() LOOP
    EXECUTE format('SELECT EXISTS (SELECT 1 FROM %I WHERE tenant_id = $1)', t) INTO held USING p_new;
    IF held THEN
      RAISE EXCEPTION 'Tenant % holds rows in % already: nothing renamed', p_new, t;
    END IF;
  END LOOP;
END $$;

-- The Organization half the broker stores for a Tenant (0036), by the rule of
-- `TenantId::organization_of`: the `OrgId` of the project-key form, else the
-- Organization id of `{organization id}__{segment}`, else none.
CREATE FUNCTION tenant_organization(p_tenant text) RETURNS text
  LANGUAGE sql IMMUTABLE PARALLEL SAFE
  AS $$
  SELECT CASE
    WHEN p_tenant ~ '^[a-h][a-z2-7]{25}__[a-h][a-z2-7]{25}__[^_]+$' THEN left(p_tenant, 26)
    WHEN p_tenant ~ '^[^_]*_[^_]*(_[^_]+)*__[^_]+$' AND position('__' IN p_tenant) > 1
      AND strpos(left(p_tenant, position('__' IN p_tenant) - 1), '_') > 0
      THEN left(p_tenant, position('__' IN p_tenant) - 1)
  END
$$;

CREATE FUNCTION tenant_rename_apply(p_old text, p_new text) RETURNS jsonb
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
BEGIN
  PERFORM tenant_rename_guard(p_old, p_new);
  PERFORM set_config('lock_timeout', '30s', true);
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
  -- writers wait until the transaction ends; readers go on
  FOR t IN SELECT tbl FROM tenant_tables() UNION SELECT 'tenants' UNION SELECT 'jsonld_contexts' LOOP
    EXECUTE format('LOCK TABLE %I IN EXCLUSIVE MODE', t);
  END LOOP;

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
    EXECUTE format(
      'UPDATE %I SET %I = tenant_token(%I, $1, $2) WHERE %s AND %I::text LIKE $3', t, c, c,
      CASE WHEN t IN (SELECT tbl FROM tenant_naming_tables()) THEN 'true' ELSE reserved_rows() END,
      c) USING p_old, p_new, tok;
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

  UPDATE tenants SET tenant_id = p_new, organization_id = tenant_organization(p_new)
   WHERE tenant_id = p_old;
  RETURN out;
END $$;

-- What still names `old` in a document after the rename, as a word inside a
-- longer string (a URL path, a sentence; never a longer name like
-- `old-x`): free text to read, never rewritten.
CREATE FUNCTION tenant_rename_leftovers(p_old text) RETURNS bigint
  LANGUAGE plpgsql
  AS $$
DECLARE
  t text;
  c text;
  n bigint;
  total bigint := 0;
BEGIN
  FOR t, c IN
    SELECT a.attrelid::regclass::text, a.attname::text
      FROM pg_attribute a JOIN tenant_tables() tt ON a.attrelid = tt.tbl::regclass
     WHERE a.attnum > 0 AND NOT a.attisdropped AND a.atttypid = 'jsonb'::regtype
  LOOP
    CONTINUE WHEN t NOT IN (SELECT tbl FROM tenant_naming_tables()) AND NOT tenant_reserved_held(t);
    -- the predicate is chosen here: an `OR ... IN (SELECT ...)` in the
    -- statement turns the index scan of `entities` into a full scan
    EXECUTE format(
      'SELECT count(*) FROM %I WHERE %s AND %I::text ~ $1', t,
      CASE WHEN t IN (SELECT tbl FROM tenant_naming_tables()) THEN 'true' ELSE reserved_rows() END,
      c) INTO n
      USING '(^|[^A-Za-z0-9_-])' || p_old || '($|[^A-Za-z0-9_-])';
    total := total + n;
  END LOOP;
  RETURN total;
END $$;

CREATE FUNCTION antares_tenant_rename(p_old text, p_new text) RETURNS jsonb
  LANGUAGE plpgsql
  AS $$
DECLARE
  out jsonb;
BEGIN
  PERFORM tenant_rename_guard(p_old, p_new);
  out := tenant_rename_apply(p_old, p_new);
  INSERT INTO tenant_rename (old, new) VALUES (p_old, p_new);
  RETURN out || jsonb_build_object('text_still_naming_old', tenant_rename_leftovers(p_old));
END $$;

-- The reverse of a recorded rename. It refuses a name the rename did not
-- write; the old name holds nothing meanwhile, since the broker never
-- creates a tombstoned Tenant.
CREATE FUNCTION antares_tenant_rename_rollback(p_new text) RETURNS jsonb
  LANGUAGE plpgsql
  AS $$
DECLARE
  v_old text;
BEGIN
  IF NOT coalesce((SELECT rolsuper OR rolbypassrls FROM pg_roles WHERE rolname = current_user), false) THEN
    RAISE EXCEPTION 'the rename reads and writes every row of a Tenant: run it as a role that bypasses row-level security (the database owner), not as %', current_user;
  END IF;
  SELECT old INTO v_old FROM tenant_rename WHERE new = p_new;
  IF v_old IS NULL THEN
    RAISE EXCEPTION 'no rename to % is recorded: restore the pg_dump instead', p_new;
  END IF;
  -- the guard reads the tombstone, so it goes first, inside this transaction
  DELETE FROM tenant_rename WHERE old = v_old;
  RETURN tenant_rename_apply(p_new, v_old);
END $$;

-- Per table: the Tenant's rows and an order-free sum of row hashes, the
-- Tenant column left out and the Tenant's name in each rewritten document
-- replaced by one placeholder. Run it with the old name before a rename and
-- with the new name after it: equal numbers mean only the name moved.
-- `reserved:<table>` covers the broker's own documents that name the Tenant.
CREATE FUNCTION antares_tenant_digest(p_tenant text)
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
                  THEN format('tenant_token(%I, $1, %L)::text', a.attname, '<tenant>')
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

REVOKE EXECUTE ON FUNCTION tenant_rename_guard(text, text),
                           tenant_rename_apply(text, text),
                           tenant_rename_leftovers(text),
                           antares_tenant_rename(text, text),
                           antares_tenant_rename_rollback(text),
                           antares_tenant_digest(text),
                           tenant_reserved_held(text) FROM PUBLIC;
