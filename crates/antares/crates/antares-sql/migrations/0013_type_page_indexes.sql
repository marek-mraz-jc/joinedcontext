-- The type page (T-4781). `types @> ARRAY[…]` never reaches an index for
-- the broker's role: `arraycontains` is not leakproof, so RLS evaluates it
-- only after the tenant policy, as a filter, and the planner walked
-- `entities_pkey` through the whole Tenant for one page (prod: 183,300
-- Entities read per page of 20, Postgres at its CPU limit at 18 req/s).
-- The store asks for one named type in two leakproof arms instead, and
-- each arm has an index that hands back its rows in id order:
--   types[1] = T                               -> i_entities_first_type_id
--   cardinality(types) > 1 AND types @> [T]    -> i_entities_multi_type_id
-- `types` and `expires_at` ride along so count=true is an index-only scan.
-- The first-type index replaces 0012's, a prefix of it, which the type
-- catalogue's liveness check keeps using through the new one.
-- An index changes no access: RLS and the query's own WHERE are untouched.
CREATE INDEX i_entities_first_type_id ON entities (tenant_id, (types[1]), id)
  INCLUDE (types, expires_at);
CREATE INDEX i_entities_multi_type_id ON entities (tenant_id, id)
  INCLUDE (types, expires_at) WHERE cardinality(types) > 1;
DROP INDEX i_entities_first_type;
