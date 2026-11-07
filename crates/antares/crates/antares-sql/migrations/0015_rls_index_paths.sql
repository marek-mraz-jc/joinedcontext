-- Index paths the broker's role can use under RLS (T-4852). With the
-- tables' policies in force, Postgres takes an operator into an index
-- condition only when it is leakproof, and no PostGIS operator, no jsonb
-- operator and no array operator is: the GIST on `location` and the GIN on
-- `temporal_entities.types` never served a request, and every geoquery and
-- every temporal `type=` read its whole Tenant (geo within at 100 000
-- Entities: 776 ms, 100 000 rows read). Btree text comparisons are
-- leakproof, so both get a text key a btree serves:
--
-- `location_cell`: the geohash cell that holds the whole bounding box of
-- `location` (up to 12 characters; '' when no single cell does or PostGIS
-- refuses the geometry, which every cover then keeps). A geoquery ANDs a
-- cover of its region over it (compile/geo.rs); `COLLATE "C"` makes the
-- prefix ranges of that cover byte order. Generated, so no write path can
-- forget it.
--
-- `temporal_entities`: the first-type and multi-type indexes `entities`
-- got in 0013, for the `types[1] = ANY(..)` arms of the temporal query.
--
-- `csource_index` (tenant_id, registration_id): the key of its foreign key
-- to `csource_registrations` and of every registration update's
-- `DELETE FROM csource_index` (store/pg/doc.rs). Without it each cascaded
-- or updated registration read the whole table: deleting 22 000
-- registrations ran past the 30 s statement_timeout.
--
-- An index changes no access: RLS and every query's own WHERE are untouched.
CREATE OR REPLACE FUNCTION geo_cell(g geometry) RETURNS text
LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE AS $$
BEGIN
  IF g IS NULL THEN
    RETURN NULL;
  END IF;
  RETURN COALESCE(left(ST_GeoHash(g), 12), '');
EXCEPTION WHEN OTHERS THEN
  RETURN '';
END $$;

ALTER TABLE entities ADD COLUMN location_cell text COLLATE "C"
  GENERATED ALWAYS AS (geo_cell(location)) STORED;
CREATE INDEX i_entities_location_cell ON entities (tenant_id, location_cell)
  WHERE location_cell IS NOT NULL;

CREATE INDEX i_temporal_entities_first_type ON temporal_entities (tenant_id, (types[1]));
CREATE INDEX i_temporal_entities_multi_type ON temporal_entities (tenant_id)
  WHERE cardinality(types) > 1;

CREATE INDEX i_csource_index_registration ON csource_index (tenant_id, registration_id);
