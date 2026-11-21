-- Upserts take the HOT path again (T-7487). 0010's partial index read the
-- `entity` document in its predicate, and PostgreSQL counts a column an index
-- predicate reads as indexed: every upsert changes `entity`, so none could
-- be a HOT update and each wrote a new entry into all of entities' indexes
-- (jc-prod: 0 HOT of 50.7M updates, 19 indexes). The predicate now reads a
-- flag column that a BEFORE trigger sets from the document: it changes only
-- when an Entity gains or loses its own `historyRetention`, so an upsert
-- changing attribute values touches no index.
--
-- Locks: ADD COLUMN with a constant default changes the catalogue only (no
-- rewrite); the trigger comes before the backfill so no Entity written
-- meanwhile slips between them (0012); the backfill updates only the Entities
-- 0010's index holds; CREATE INDEX scans entities once under SHARE, as 0010
-- and 0044 did (0044: 14.7 s on jc-dev's 2.46M rows, with the pod down).
-- RLS and FORCE on entities are untouched; the column sits under them.
ALTER TABLE entities ADD COLUMN history_retention boolean NOT NULL DEFAULT false;

CREATE FUNCTION note_entity_history_retention() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  NEW.history_retention := NEW.entity ? 'https://uri.etsi.org/ngsi-ld/default-context/historyRetention';
  RETURN NEW;
END $$;

CREATE TRIGGER entities_history_retention BEFORE INSERT OR UPDATE ON entities
  FOR EACH ROW EXECUTE FUNCTION note_entity_history_retention();

SELECT set_config('antares.service', 'on', true);
UPDATE entities SET history_retention = true
  WHERE entity ? 'https://uri.etsi.org/ngsi-ld/default-context/historyRetention';
SELECT set_config('antares.service', '', true);

CREATE INDEX i_entities_history_retention_flag ON entities (tenant_id)
  WHERE history_retention;
DROP INDEX i_entities_history_retention;
