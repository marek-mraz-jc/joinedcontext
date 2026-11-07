-- The Entities that carry their own history retention (DM-97, ADR-N-046
-- 2.3): the retention sweep reads them across every Tenant once a minute
-- (store/pg/maintenance.rs `history_overrides`), and the key is a literal, so
-- a partial index holds exactly that set and costs every other write nothing.
--
-- On entities, not attr_instances (see 0009's note, T-4310): building it is
-- one scan of current state (626 MB on dev, a few seconds of writes waiting at
-- upgrade), never of the history.
CREATE INDEX IF NOT EXISTS i_entities_history_retention ON entities (tenant_id)
  WHERE entity ? 'https://uri.etsi.org/ngsi-ld/default-context/historyRetention';
