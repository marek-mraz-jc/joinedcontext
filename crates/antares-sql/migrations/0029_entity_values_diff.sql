-- entity_values (0024) rewrote every row of an Entity whose document moved
-- at all: a pipeline upsert that changed only an `observedAt` deleted and
-- reinserted each of its Attributes' rows and their three index entries.
-- On dev that was 131.9M inserts and 133.6M deletes in a week, the largest
-- writer of the WAL that filled the backup archive (T-5772).
--
-- The update trigger now writes the difference: per changed Entity it
-- deletes the old document's rows the new one lacks and inserts the new
-- rows the old one lacks, as sets (EXCEPT reads NULL and NaN as equal, as
-- the probes do). A row both documents hold stays where it is. The stored
-- rows are entity_value_rows() of the stored document, which every write
-- path has kept since 0024; a migration that changes entity_value_rows()
-- rebuilds the table. A duplicate row (two numbers clamped to one float8)
-- can outlive one of its sources: a probe reads ids, and a second copy
-- names the same one.
CREATE OR REPLACE FUNCTION entity_values_on_update() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  WITH c AS (
    SELECT n.tenant_id, n.id, o.entity AS old_doc, n.entity AS new_doc
      FROM new_rows n JOIN old_rows o USING (tenant_id, id)
     WHERE o.entity IS DISTINCT FROM n.entity
  ), was AS (
    SELECT c.tenant_id, c.id, r.attr, r.f, r.txt FROM c, entity_value_rows(c.old_doc) r
  ), now_ AS (
    SELECT c.tenant_id, c.id, r.attr, r.f, r.txt FROM c, entity_value_rows(c.new_doc) r
  ), gone AS (
    SELECT * FROM was EXCEPT SELECT * FROM now_
  ), dropped AS (
    DELETE FROM entity_values v USING gone g
     WHERE v.tenant_id = g.tenant_id AND v.id = g.id AND v.attr = g.attr
       AND v.f IS NOT DISTINCT FROM g.f AND v.txt IS NOT DISTINCT FROM g.txt
  )
  INSERT INTO entity_values (tenant_id, id, attr, f, txt)
    SELECT * FROM now_ EXCEPT SELECT * FROM was;
  RETURN NULL;
END $$;
