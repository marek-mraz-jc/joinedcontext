// cg-audit 1 (T-4303): every Portal HTTP handler reaches a permission check, or its rule in
// tests/authz_matrix.yaml says why not.
// Run from the repo root: cg build && cg q - < scripts/cg-audit/routes-authz.cypher
// expected: every row's route has a matrix row whose rule is not project-verb:* (public, signed-in,
// signature:*, app-edge, workload:*, the caller's own record); a row without one is a finding.
// At a1bb01b: 68 rows, all such (basemap tiles public, apps/{name}/me app-edge via apps::roles::verified,
// POST /activity workload:activity-ingest, /tools/* and GET /sync signed-in, the webhooks and
// /live-notify/{key} signature or key).
MATCH (h:Symbol)-[:PROVIDES]->(c:Channel {channel:'http'}) WHERE h.lang='rust' AND NOT EXISTS { MATCH (h)-[:CALLS|DISPATCHES*1..6]->(g:Symbol) WHERE g.path STARTS WITH 'joinedcontext_portal::permissions::' } RETURN DISTINCT c.key, h.path ORDER BY c.key;
