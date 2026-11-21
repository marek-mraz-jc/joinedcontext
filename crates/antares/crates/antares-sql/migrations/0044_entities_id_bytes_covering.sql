-- A deep page's skip reads ids in byte order from the index alone (T-7484).
-- Under a rule's idPatterns a page past offset 0 streams the arms' ids in
-- `id COLLATE "C"` order and skips its offset with the broker's regex
-- (store::pg::entity). 0027's index gives that order, but each id it hands
-- back costs a heap visit for `types` and `expires_at`, and over a type
-- whose rows lie in the heap in no id order the planner prices that walk
-- above a Seq Scan of the table and a sort of the whole type (CI, 164k
-- Trees: 29 MB on disk; prod: a scan of every Tenant's rows). With the two
-- columns in the index the walk is an Index Only Scan in order and the arms
-- merge without a sort. Built on 164k rows in 0.3 s.
-- An index changes no access: RLS and the query's own WHERE are untouched.
DROP INDEX i_entities_id_bytes;
CREATE INDEX i_entities_id_bytes ON entities (tenant_id, (id COLLATE "C"))
  INCLUDE (types, expires_at);
