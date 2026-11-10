-- W/history.tsv: per id under the prefix, its rows in temporal_entities, attr_instances,
-- attr_versions and attr_rows (attr_rows holds history only once history_cutover.at is set; its
-- entity_no is the iri_ids id of the entity's IRI in the same tenant).
BEGIN READ ONLY;
SELECT tenant_id, id, sum(te), sum(ai), sum(av), sum(ar) FROM (
  SELECT tenant_id, id, 1 te, 0 ai, 0 av, 0 ar FROM temporal_entities
   WHERE split_part(id, ':', 4) = 'portal.smartcity.joinedcontext.com'
  UNION ALL
  SELECT tenant_id, entity_id, 0, count(*), 0, 0 FROM attr_instances
   WHERE split_part(entity_id, ':', 4) = 'portal.smartcity.joinedcontext.com' GROUP BY 1, 2
  UNION ALL
  SELECT tenant_id, entity_id, 0, 0, count(*), 0 FROM attr_versions
   WHERE split_part(entity_id, ':', 4) = 'portal.smartcity.joinedcontext.com' GROUP BY 1, 2
  UNION ALL
  SELECT r.tenant_id, i.iri, 0, 0, 0, count(*) FROM attr_rows r
    JOIN iri_ids i ON i.tenant_id = r.tenant_id AND i.id = r.entity_no
   WHERE split_part(i.iri, ':', 4) = 'portal.smartcity.joinedcontext.com' GROUP BY 1, 2
) h GROUP BY 1, 2 ORDER BY 1, 2;
ROLLBACK;
