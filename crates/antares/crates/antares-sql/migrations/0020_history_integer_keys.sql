-- Integer history keys, slice 1 (T-5200; design in docs/src/storage.md
-- "Integer history keys", T-5193 shape B). A second history parent keyed by
-- per-Tenant dictionary ids and the instance uuid instead of three texts.
-- Nothing writes it yet: the one-row history_cutover holds NULL, and the
-- store writes every row to attr_instances until a later migration sets the
-- cut-over Monday. Every history read already reads both parents, so the
-- release that sets it finds its reads in place.

-- Full expanded IRIs (Entity ids and Attribute names) of one Tenant.
-- An id names an IRI only together with its tenant_id: every lookup carries
-- the Tenant, and RLS refuses another Tenant's rows on top.
CREATE TABLE IF NOT EXISTS iri_ids (
  tenant_id text NOT NULL,
  id        bigint GENERATED ALWAYS AS IDENTITY,
  iri       text NOT NULL,
  PRIMARY KEY (tenant_id, id),
  UNIQUE (tenant_id, iri)
);
-- 0005's per-command policies. No UPDATE policy: an id never changes its IRI.
ALTER TABLE iri_ids ENABLE ROW LEVEL SECURITY;
ALTER TABLE iri_ids FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_read ON iri_ids FOR SELECT
  USING (tenant_id = current_setting('antares.tenant', true)
         OR current_setting('antares.service', true) = 'on');
CREATE POLICY tenant_reap ON iri_ids FOR DELETE
  USING (tenant_id = current_setting('antares.tenant', true)
         OR current_setting('antares.service', true) = 'on');
CREATE POLICY tenant_write ON iri_ids FOR INSERT
  WITH CHECK (tenant_id = current_setting('antares.tenant', true));

-- The cut-over: rows observed from it on go to attr_rows. NULL: inactive.
CREATE TABLE IF NOT EXISTS history_cutover (
  one boolean PRIMARY KEY DEFAULT true CHECK (one),
  at  timestamptz
);
INSERT INTO history_cutover (one, at) VALUES (true, NULL) ON CONFLICT DO NOTHING;

-- attr_instances' columns with entity_no, attr_no and instance_no in place
-- of entity_id, attr_id and instance_id, built the way 0001 built it.
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
  provenance  jsonb
';
BEGIN
  IF to_regclass('public.attr_rows') IS NOT NULL THEN
    RETURN;
  END IF;
  IF EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'timescaledb') THEN
    EXECUTE 'CREATE TABLE attr_rows (' || cols || ')';
    PERFORM public.create_hypertable('attr_rows', 'observed_at',
                                     chunk_time_interval => INTERVAL '7 days');
  ELSE
    EXECUTE 'CREATE TABLE attr_rows (' || cols || ') PARTITION BY RANGE (observed_at)';
    EXECUTE 'CREATE TABLE attr_rows_default PARTITION OF attr_rows DEFAULT';
  END IF;
  EXECUTE 'CREATE UNIQUE INDEX u_attr_rows_time
             ON attr_rows (tenant_id, entity_no, attr_no, observed_at, instance_no)';
  IF EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'timescaledb') THEN
    EXECUTE 'ALTER TABLE attr_rows SET (timescaledb.compress,
               timescaledb.compress_segmentby = ''tenant_id, attr_no'',
               timescaledb.compress_orderby   = ''entity_no, observed_at DESC'')';
    PERFORM public.add_compression_policy('attr_rows',
                                          compress_after => INTERVAL '7 days');
  ELSE
    EXECUTE 'ALTER TABLE attr_rows ENABLE ROW LEVEL SECURITY';
    EXECUTE 'ALTER TABLE attr_rows FORCE ROW LEVEL SECURITY';
    EXECUTE 'CREATE POLICY tenant_read ON attr_rows FOR SELECT
               USING (tenant_id = current_setting(''antares.tenant'', true)
                      OR current_setting(''antares.service'', true) = ''on'')';
    EXECUTE 'CREATE POLICY tenant_reap ON attr_rows FOR DELETE
               USING (tenant_id = current_setting(''antares.tenant'', true)
                      OR current_setting(''antares.service'', true) = ''on'')';
    EXECUTE 'CREATE POLICY tenant_write ON attr_rows FOR INSERT
               WITH CHECK (tenant_id = current_setting(''antares.tenant'', true))';
    EXECUTE 'CREATE POLICY tenant_update ON attr_rows FOR UPDATE
               USING (tenant_id = current_setting(''antares.tenant'', true))
               WITH CHECK (tenant_id = current_setting(''antares.tenant'', true))';
  END IF;
END $$;

-- The id of one IRI of one Tenant, NULL when it was never written. STABLE
-- and invoker's rights, so RLS on iri_ids applies and an index condition
-- `entity_no = iri_no($1, $2)` is evaluated once per scan: the btree then
-- feeds an ordered read directly (a join with iri_ids would keep only the
-- outer side's order and sort every row of the Attribute).
CREATE OR REPLACE FUNCTION public.iri_no(p_tenant text, p_iri text) RETURNS bigint
  LANGUAGE sql STABLE PARALLEL SAFE
  AS $$ SELECT id FROM public.iri_ids WHERE tenant_id = p_tenant AND iri = p_iri $$;

-- attr_rows of one Entity (and one Attribute) in the row type of
-- attr_instances, so a read appends them as a second UNION ALL arm. Inlined
-- by the planner (one SELECT, LANGUAGE sql, STABLE, not STRICT): the caller's
-- predicates reach the btree. The broker's own instance ids are
-- urn:ngsi-ld:Instance:<uuid>; any other id stays in data as instanceId.
-- Every name is schema-qualified: a SET search_path would stop the inlining,
-- and a same-named table earlier on the caller's path must not stand in.
CREATE OR REPLACE FUNCTION attr_rows_of(p_tenant text, p_entity text)
  RETURNS SETOF public.attr_instances LANGUAGE sql STABLE PARALLEL SAFE
  AS $$
  SELECT r.tenant_id, p_entity, a.iri,
         COALESCE(r.data->>'instanceId', 'urn:ngsi-ld:Instance:' || r.instance_no::text),
         r.dataset_id, r.observed_at, r.created_at, r.modified_at, r.deleted_at,
         r.data, r.geo_value, r.version, r.provenance
    FROM public.attr_rows r
    JOIN public.iri_ids a ON a.tenant_id = r.tenant_id AND a.id = r.attr_no
   WHERE r.tenant_id = p_tenant AND r.entity_no = public.iri_no(p_tenant, p_entity)
  $$;

CREATE OR REPLACE FUNCTION attr_rows_at(p_tenant text, p_entity text, p_attr text)
  RETURNS SETOF public.attr_instances LANGUAGE sql STABLE PARALLEL SAFE
  AS $$
  SELECT r.tenant_id, p_entity, p_attr,
         COALESCE(r.data->>'instanceId', 'urn:ngsi-ld:Instance:' || r.instance_no::text),
         r.dataset_id, r.observed_at, r.created_at, r.modified_at, r.deleted_at,
         r.data, r.geo_value, r.version, r.provenance
    FROM public.attr_rows r
   WHERE r.tenant_id = p_tenant AND r.entity_no = public.iri_no(p_tenant, p_entity)
     AND r.attr_no = public.iri_no(p_tenant, p_attr)
  $$;
