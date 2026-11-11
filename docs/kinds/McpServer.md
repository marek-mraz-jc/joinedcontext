# McpServer

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `McpServer`
- Lives in the Organization (`org`) or a project
- API: `/api/v1/projects/{project}/mcpservers`
- Repository path: `mcp/{name}.yaml`
- Repository path in a project: `projects/{project}/mcp/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (McpServerSpec)

One named MCP server (MF-53).

| Field | Type | Required | Description |
|---|---|---|---|
| `allowedProjects` | list of string | no | The projects a `project-list` server admits; the server's own project when empty. |
| `audience` | [Audience](#audience) | yes | Who may connect: `public`, `organization` or `project-list`. No wider than the narrowest member's own audience; `public` takes the red lane (EP-16). |
| `knowledgeSources` | list of string | no | The `KnowledgeSource`s its `search_documents` searches, up to twenty, each `{source}` of its own project or `{project}/{source}`; each a `visibility: public` source (MF-57). |
| `members` | list of [TypedRef](#typedref) | no | The Endpoints the server reads, up to ten, by typed reference (`kind: Endpoint`); a member of another project names its project in `namespace`. At least one without `select`. |
| `select` | [Select](#select) | no | The Endpoints it selects beside its members (MF-55). |

## Audience

Target audience access control setting for an endpoint (EP-14).

Type: one of `"project-list"`, `"organization"`, `"public"`

## TypedRef

Structured typed resource reference (MF-07).

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | yes | Target resource kind. |
| `name` | string | yes | Target resource name. |
| `namespace` | string | no | Target resource namespace (defaults to caller namespace if omitted). |

## Select

Which Endpoints a server selects beside its members (MF-55). An Endpoint is selected when it matches every field given, one value of a field being enough; `select: {}` selects every Endpoint the caller's token and the server's audience admit. The gateway resolves it per call from the caller's own reach, so it only narrows.

| Field | Type | Required | Description |
|---|---|---|---|
| `accessLevel` | list of [AccessLevel](#accesslevel) | no | How the caller reaches the Endpoint; no wider than the server's audience admits. |
| `domains` | list of string | no | Smart Data Models domains of the Endpoint's classification, e.g. `Environment` (T-4498). |
| `projects` | list of string | no | Project names. |
| `spaces` | list of string | no | Context Spaces as `{project}/{space}`. |
| `themes` | list of [DataTheme](#datatheme) | no | EU data themes of that classification, e.g. `ENVI`. |
| `types` | list of string | no | NGSI-LD entity types the Endpoint serves, e.g. `OffStreetParking` (MF-61). |

## AccessLevel

How a caller reaches an Endpoint, the levels of the per-caller catalogue (T-4503).

Type: one of `"public"`, `"organization"`, `"project"`, `"grant"`

## DataTheme

A theme of the EU data-theme table.

Type: one of `"AGRI"`, `"ECON"`, `"EDUC"`, `"ENER"`, `"ENVI"`, `"GOVE"`, `"HEAL"`, `"INTR"`, `"JUST"`, `"REGI"`, `"SOCI"`, `"TECH"`, `"TRAN"`
