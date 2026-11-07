-- A leakproof key for `name=="…"` (T-5349). A q= term compiles to `@?`,
-- which no index serves under forced RLS (0019), so a name filter with no
-- type read the whole Tenant: STA `Things?$filter=name eq '…'` on dev
-- (186 000 Entities, no type) ran past the gateway's 10 s read limit.
--
-- `name_key` holds the `name` the core @context gives an Entity
-- (`@vocab` + name): the text of its one instance's `value` when that is a
-- string of at most 1 024 bytes (a btree entry has a size cap), '' when the
-- Entity has no `name`, NULL for every other shape (several instances, a
-- LanguageProperty, a typed or non-string value). The store ANDs
-- `(name_key = $n OR name_key IS NULL)` to a top-level `name=="…"`
-- (store/pg/entity.rs `named`): every match satisfies it, and the `@?`
-- stays and decides. `COLLATE "C"` makes `=` the byte equality the
-- evaluator and jsonpath compare with. Generated, so no write path can
-- forget it.
--
-- An index changes no access: RLS and every query's own WHERE are untouched.
ALTER TABLE entities ADD COLUMN name_key text COLLATE "C" GENERATED ALWAYS AS (
  CASE
    WHEN NOT entity ? 'https://uri.etsi.org/ngsi-ld/default-context/name' THEN ''
    WHEN jsonb_typeof(entity -> 'https://uri.etsi.org/ngsi-ld/default-context/name') <> 'array' THEN NULL
    WHEN jsonb_array_length(entity -> 'https://uri.etsi.org/ngsi-ld/default-context/name') <> 1 THEN NULL
    WHEN jsonb_typeof(entity -> 'https://uri.etsi.org/ngsi-ld/default-context/name' -> 0 -> 'value') <> 'string' THEN NULL
    WHEN octet_length(entity -> 'https://uri.etsi.org/ngsi-ld/default-context/name' -> 0 ->> 'value') > 1024 THEN NULL
    ELSE entity -> 'https://uri.etsi.org/ngsi-ld/default-context/name' -> 0 ->> 'value'
  END) STORED;
CREATE INDEX i_entities_name_key ON entities (tenant_id, name_key);
