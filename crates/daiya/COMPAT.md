# Grafana HTTP API Compatibility Adapters

The native API of `daiya` is the OData 4.01 service at `/odata/v4` (R10a). The following classic Grafana HTTP routes are served as thin adapters onto the same store, query, import, and authorization operations so `provision.sh` and `joinedcontext-portal` (`src/grafana.rs`, `src/api/grafana.rs`) work unchanged during migration:

| Grafana HTTP Route | Used By | Native OData / Internal Operation |
|---|---|---|
| `GET /api/health` | `provision.sh`, k8s probes | Health check |
| `POST /api/orgs` | `provision.sh` | Organization mapping registration |
| `GET /api/orgs/name/{name}` | `provision.sh`, `POR src/grafana.rs` | Organization lookup by project/org name |
| `GET /api/org` | Grafana clients | Current organization lookup |
| `GET /api/datasources` | `POR src/grafana.rs` | `GET /odata/v4/Datasources` |
| `POST /api/datasources` | `provision.sh`, `POR src/grafana.rs` | Datasource upsert in `Apply` |
| `GET /api/datasources/uid/{uid}` | Grafana clients | `GET /odata/v4/Datasources('{uid}')` |
| `PUT /api/datasources/uid/{uid}` | `provision.sh` | Datasource upsert in `Apply` |
| `DELETE /api/datasources/uid/{uid}` | Grafana clients | Datasource delete |
| `GET /api/datasources/uid/{uid}/health` | Query editor | `GET /odata/v4/Datasources('{uid}')/Health` |
| `GET /api/datasources/uid/{uid}/resources/{*path}` | Query editor | `GET /odata/v4/Datasources('{uid}')/Resource(path='...')` |
| `GET /api/folders` | Grafana clients | `GET /odata/v4/Folders` |
| `POST /api/folders` | `provision.sh` | Folder upsert in `Apply` |
| `GET /api/folders/{uid}` | Grafana clients | `GET /odata/v4/Folders('{uid}')` |
| `DELETE /api/folders/{uid}` | Grafana clients | Folder delete |
| `POST /api/dashboards/db` | `provision.sh`, `POR src/grafana.rs` | `POST /odata/v4/Import` |
| `GET /api/dashboards/uid/{uid}` | `POR src/grafana.rs` | `GET /odata/v4/Dashboards('{uid}')` + `Dashboard/Export` |
| `DELETE /api/dashboards/uid/{uid}` | `POR src/grafana.rs` | Dashboard delete |
| `GET /api/dashboards/uid/{uid}/versions` | Grafana clients | `GET /odata/v4/DashboardVersions?$filter=dashboardUid eq '{uid}'` |
| `GET /api/search` | Grafana clients | `GET /odata/v4/Dashboards` + `GET /odata/v4/Folders` |
| `POST /api/ds/query` | Grafana panels | `POST /odata/v4/Datasource.Query` |
| `GET /api/public/dashboards/{accessToken}` | Anonymous viewer | Public dashboard view (sanitized) |
| `POST /api/public/dashboards/{accessToken}/panels/{panelId}/query` | Anonymous viewer | `POST /odata/v4/PublicDashboard.Query` |

## Differences from Grafana and Behaviour Matched

These were verified by tests in `api/tests/compat_routes.rs`.

- `PUT /api/datasources/uid/{uid}` answers 404 for an unknown uid and 403 for a provisioned (read-only) datasource, as Grafana does. `provision.sh` only PUTs after a POST answered 409.
- `secureJsonData` in `POST`/`PUT /api/datasources` is refused with 400 naming `secretRefs` (§7.1). An empty `{}` is accepted.
- `readOnly` in a datasource body is ignored. Only provisioning sets it, as Grafana's `json:"-"` field does.
- A datasource or folder created without a uid gets a random 14-character short uid. A given uid over 40 characters or outside `a-zA-Z0-9-_` is refused with 400 (Grafana `IsValidShortUID`).
- `POST /api/folders` on an existing uid answers 409.
- `PATCH` of a public dashboard checks the `{uid}` in its path.
- Datasource resource paths accept Grafana's `api/v1/labels`, `api/v1/label/{name}/values` and `api/v1/series` forms as well as the short ones.
- Difference: the numeric `id` is a constant, or the row position in lists. Records have no numeric id.
