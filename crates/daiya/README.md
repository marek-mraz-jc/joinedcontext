# daiya

Dashboards, metrics, logs and alerts per organization (replaces Grafana). Name proposed by the supervisor 2026-11-07, owner to confirm.

## Formats (supervisor proposal 2026-11-07, owner may override)

| Layer | Format | Holds |
|---|---|---|
| Dashboard | own small versioned JSON schema (deny_unknown_fields), a manifest kind in Git | layout grid, variables (organization, project, time range), panels, alert rules, permissions |
| Query | one field per panel: PromQL (metrics), the scoped log query (jc-mcp), NGSI-LD or OData (context data) | always narrowed to the caller's organization by jc-auth; no data-source credential in a dashboard |
| Chart | Vega-Lite spec per panel, plus built-in HTML panels: stat, table, logs | rendering; SVG on the server for alert images |

Grafana JSON is an importer, not the format: our 10 deployment dashboards hold 49 panels (timeseries 42, table 4, bargauge 2, stat 1),
all convertible; other panel types import as "not supported" with their name.
