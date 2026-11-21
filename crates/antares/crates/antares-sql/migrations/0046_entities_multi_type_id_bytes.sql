-- The multi-type arm of a deep page's skip in byte order (T-7484). The skip
-- reads each arm of a type in `id COLLATE "C"` order, keyset chunk by chunk
-- (store::pg::entity::skip_matches). The first-type arm walks 0044's index;
-- the multi-type arm had only 0013's partial index, ordered by the default
-- collation, so under the skip's index-only settings it walked the whole
-- type's id range in 0044's index for its few rows, once per chunk (358k
-- Trees, none multi-type: 358k index entries filtered per chunk, 180 ms
-- warm). This partial index holds only the Entities of two or more types,
-- in byte order, so the arm reads those alone.
-- An index changes no access: RLS and the query's own WHERE are untouched.
CREATE INDEX i_entities_multi_type_id_bytes ON entities (tenant_id, (id COLLATE "C"))
  INCLUDE (types, expires_at) WHERE cardinality(types) > 1;
