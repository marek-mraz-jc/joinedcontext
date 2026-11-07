# joinedcontext

The joinedcontext platform in Rust, one repository: one binary (`bins/jc`) whose modules can also run alone.

| Module | Replaces | Folder |
|---|---|---|
| Antares | context broker | crates/antares |
| ekiben | Bento | crates/ekiben |
| wardkeep | Keycloak, FIWARE VCVerifier | crates/wardkeep |
| shako | Gitea | crates/shako |
| dataroom | CKAN | crates/dataroom |
| controlroom | Grafana | crates/controlroom |
| factory | code factory and deploy | crates/factory |
| portal | management UI and API | crates/portal |

Shared: jc-core (kinds), jc-auth (one identity and permission decision), jc-store (one Postgres, RLS per organization), jc-web (server-rendered
HTML), jc-mcp (every operation as an MCP tool, scoped per organization), jc-wasm (WebAssembly host, millisecond start).

Rules: multi-domain, multi-organization, several instances of a module per organization; one permission model; whole platform under 2 GB
without the AI embedder at 100 req/s. Status: folders only; research tasks T-6636..T-6641 and module tasks T-6633..T-6635 are held for the owner.
