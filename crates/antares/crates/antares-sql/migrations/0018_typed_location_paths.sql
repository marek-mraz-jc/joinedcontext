-- Two index paths of a typed read the broker's role can use under RLS
-- (T-5136, measured on the distinct-1000 rig, 581 000 Entities, ten types):
--
-- `i_entities_type_location_cell`: a geoquery of one type. Its cover over
-- `i_entities_location_cell` (0015) holds every type in the cells, and the
-- type was a filter after it: a near/within page of 20 read 17 000-47 000
-- rows of other types, 0.1-0.9 s each. With the type in the key the cover
-- reads only the asked type's rows.
--
-- `i_entities_type_unlocated`: the Entities of a type without `location`.
-- `!location` (an OGC `bbox` page's second part, C12, the Features without
-- a geometry) is a negated `@?`, never an index condition, so it read the
-- whole type for no row: 64 000 rows, 68 ms, on every bbox and CSV page.
-- The store ANDs `location IS NULL` to it, true of every Entity `!location`
-- matches (store/pg/entity.rs `unlocated`); that conjunct is leakproof and
-- this partial index holds exactly its rows.
--
-- An index changes no access: RLS and every query's own WHERE are untouched.
CREATE INDEX i_entities_type_location_cell ON entities (tenant_id, (types[1]), location_cell)
  WHERE location_cell IS NOT NULL;
CREATE INDEX i_entities_type_unlocated ON entities (tenant_id, (types[1]), id)
  WHERE location IS NULL;
