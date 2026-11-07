-- The attribute catalogue (T-4934): every (type, member name) pair a
-- Tenant's Entities ever carried, one row each, kept by the writes. `/types`
-- with details and `/attributes` read the pairs here and probe a few
-- Entities per type through the caller's view, instead of folding every
-- Entity of the Tenant (13-19 s for `/attributes` on dev).
--
-- One statement trigger per INSERT and per UPDATE reads the statement's
-- rows from its transition table and lists their distinct pairs at once, so
-- a batch of 100 Entities of one shape probes entity_attrs' key a dozen times, not
-- 1 200. Its cost is pulling the member names out of each written document:
-- 8-15 us a 12-member Entity under EXPLAIN ANALYZE, where a row trigger
-- comparing OLD and NEW took 26-64 us (T-4934 task, 20 000 Entities).
-- `ON CONFLICT DO NOTHING` takes no lock on a pair already listed; two
-- transactions inserting the same new pair wait on each other's unique index
-- entry until the first commits, once per pair, and ORDER BY makes them take
-- new pairs in one order, so they never deadlock. A removed member or a deleted
-- Entity leaves its pairs; the read checks each against a live Entity.
-- The member names are the document's keys, the Entity frame (id, type,
-- createdAt...) included: the reader drops those, as the fold does.
-- Numbered 0025: 0021-0024 landed first.
CREATE TABLE entity_attrs (
  tenant_id text NOT NULL,
  type      text NOT NULL,
  attr      text NOT NULL,
  PRIMARY KEY (tenant_id, type, attr)
);

CREATE FUNCTION note_entity_attrs() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  INSERT INTO entity_attrs (tenant_id, type, attr)
    SELECT DISTINCT n.tenant_id, t, k
      FROM written AS n, unnest(n.types) AS t, jsonb_object_keys(n.entity) AS k
     ORDER BY 1, 2, 3
    ON CONFLICT DO NOTHING;
  RETURN NULL;
END $$;

-- A transition table cannot sit on a column-list trigger (UPDATE OF), so the
-- update trigger sees every UPDATE of `entities`; a same-shape one inserts
-- nothing.
CREATE TRIGGER entity_attrs_on_insert AFTER INSERT ON entities
  REFERENCING NEW TABLE AS written FOR EACH STATEMENT EXECUTE FUNCTION note_entity_attrs();
CREATE TRIGGER entity_attrs_on_update AFTER UPDATE ON entities
  REFERENCING NEW TABLE AS written FOR EACH STATEMENT EXECUTE FUNCTION note_entity_attrs();

-- Triggers first, as in 0012: no Entity written meanwhile slips between the
-- backfill and the trigger.
SELECT set_config('antares.service', 'on', true);
INSERT INTO entity_attrs (tenant_id, type, attr)
  SELECT DISTINCT tenant_id, t, k FROM entities, unnest(types) AS t, jsonb_object_keys(entity) AS k
  ON CONFLICT DO NOTHING;
SELECT set_config('antares.service', '', true);

ALTER TABLE entity_attrs ENABLE ROW LEVEL SECURITY;
ALTER TABLE entity_attrs FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON entity_attrs
  USING (tenant_id = current_setting('antares.tenant', true))
  WITH CHECK (tenant_id = current_setting('antares.tenant', true));
