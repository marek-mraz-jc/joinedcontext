-- The type catalogue (T-4683): every type a Tenant's Entities ever carried,
-- one row each, kept by the writes themselves. `/types` and MCP `list_types`
-- read the names here and ask the types index once per name whether a live
-- Entity still carries it, instead of a DISTINCT over every Entity's `types`
-- (66 ms per 100 000 Entities, 30 s and a 503 on a loaded dev).
--
-- Only an insert, or an update that changes `types`, writes; the row of a
-- type already listed is left alone (`ON CONFLICT DO NOTHING` takes no lock
-- on it), so writers of one type never queue on its row. A delete leaves the
-- name; the liveness check hides it, and a Tenant holds few distinct types.
-- Numbered 0012: 0011 is T-4665's, and sqlx applies an unapplied lower
-- version after a higher one.
CREATE TABLE entity_types (
  tenant_id text NOT NULL,
  type      text NOT NULL,
  PRIMARY KEY (tenant_id, type)
);

-- The catalogue's liveness check: a skip scan of the first-type btree finds
-- each type some Entity carries first in one probe; a type carried only
-- beside another sits on an Entity of two or more types, few of a Tenant's
-- Entities, which a partial GIN holds alone. The plain types GIN built a
-- bitmap of every Entity of each type (18 ms for five types of 20 000).
CREATE INDEX i_entities_first_type ON entities (tenant_id, (types[1]));
CREATE INDEX i_entities_types_multi ON entities USING gin (tenant_id, types)
  WHERE cardinality(types) > 1;

CREATE FUNCTION note_entity_types() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  INSERT INTO entity_types (tenant_id, type)
    SELECT NEW.tenant_id, t FROM unnest(NEW.types) AS t
    ON CONFLICT DO NOTHING;
  RETURN NULL;
END $$;

CREATE TRIGGER entity_types_on_insert AFTER INSERT ON entities
  FOR EACH ROW EXECUTE FUNCTION note_entity_types();
CREATE TRIGGER entity_types_on_retype AFTER UPDATE OF types ON entities
  FOR EACH ROW WHEN (OLD.types IS DISTINCT FROM NEW.types)
  EXECUTE FUNCTION note_entity_types();

-- The triggers come first: CREATE TRIGGER holds writes to `entities` until
-- this migration commits, so no Entity written meanwhile slips between the
-- backfill and the trigger. The backfill reads every Tenant's Entities under
-- the service escape, before the catalogue's own RLS exists.
SELECT set_config('antares.service', 'on', true);
INSERT INTO entity_types (tenant_id, type)
  SELECT DISTINCT tenant_id, t FROM entities, unnest(types) AS t
  ON CONFLICT DO NOTHING;
SELECT set_config('antares.service', '', true);

ALTER TABLE entity_types ENABLE ROW LEVEL SECURITY;
ALTER TABLE entity_types FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON entity_types
  USING (tenant_id = current_setting('antares.tenant', true))
  WITH CHECK (tenant_id = current_setting('antares.tenant', true));
