// cg-audit 4 (T-4306): every store call runs under the tenant the request named; the broker's own
// tenants and the cross-tenant service escape stay off the request paths.
// Run from the repo root: cg build && cg q - < scripts/cg-audit/tenant-isolation.cypher
// Each statement must return only the rows listed under "expected"; any other row is a finding to
// read with SHOW before it counts.

// 1. The RLS service escape (antares.service = on) is for the outbox drain and temporal retention
// alone; no HTTP handler may reach it.
// expected: no row.
MATCH (h:Symbol)-[:PROVIDES]->(c:Channel {channel:'http'}) WHERE EXISTS { MATCH (h)-[:CALLS|DISPATCHES*1..10]->(s:Symbol {path:'antares_sql::store::pg::set_service'}) } RETURN DISTINCT c.key, h.path;

// 2. Who builds a TenantId that may be one of the broker's own (snap-*, antares-views, ...).
// expected (16 rows at 5d0c44e): negotiate::tenant_from (behind tenant_exists_layer, which refuses a
// reserved name the caller sent); notify::seed_mirror, matches_for, interval_tick (the store's own
// tenant enumeration); distsub::remote_notify_inner (tenant read back from the broker's inbound
// index, ADR-0019); distsub::ds_index_tenant, snapshots::snap_index_tenant / synth_tenant,
// state::view_tenant / policy_tenant / grant_tenant / AppState::with_drivers (the broker's own
// tenants by constant); broker::wiring::apply_delta, bus::nats::NatsBus::publish_registry;
// TenantId::new / try_from themselves.
MATCH (c:Symbol)-[:CALLS]->(f:Symbol) WHERE f.path ENDS WITH 'TenantId::new_internal' AND NOT c.is_test RETURN DISTINCT c.path, c.file ORDER BY c.path;

// 3. HTTP handlers that read the tenant through negotiate::tenant_from (which accepts a reserved
// name): each must be mounted under the API nest, behind tenant_exists_layer.
// expected: no row (every such handler is under /ngsi-ld/v1/).
MATCH (h:Symbol)-[:PROVIDES]->(c:Channel {channel:'http'}) WHERE NOT c.key CONTAINS ' /ngsi-ld/v1/' AND EXISTS { MATCH (h)-[:CALLS*1..4]->(t:Symbol {path:'antares_api::negotiate::tenant_from'}) } RETURN DISTINCT c.key, h.path ORDER BY c.key;
