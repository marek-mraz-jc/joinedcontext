-- Row estimates for a Tenant's type page (T-4852). The planner multiplies
-- the selectivity of `tenant_id = X` by that of `types[1] = T` as if the
-- two were independent, but a Tenant holds its own types: on a shared
-- table it guessed 18-204 Entities of a type a Tenant holds 2000 of, and
-- then sorted a bitmap of every one for a page of 20 instead of reading
-- the type index in id order (CI runs 39569761909, 39590613787). A
-- multivariate MCV list over the Tenant, the first type and the type count
-- gives the planner each common combination's real frequency.
-- Statistics change no access and no result; ANALYZE fills them.
CREATE STATISTICS entities_tenant_types (mcv)
  ON tenant_id, (types[1]), (cardinality(types)) FROM entities;
