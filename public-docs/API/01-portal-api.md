---
sidebar_position: 2
title: "Portal REST API"
---

# Portal REST API

The Portal REST API provides administrative and platform management capabilities. OpenAPI 3.1 definitions are available at `https://portal.<domain>/api/v1/openapi.json`.

## 1. Standard Headers

- `Authorization: Bearer <jwt>` (Required for all non-public endpoints)
- `Accept-Language: sk, en;q=0.8` (Requests translated titles and descriptions)
- `Content-Type: application/json`

## 2. Error Response Format (RFC 7807)

An error answers `application/problem+json` (`joinedcontext-portal/src/error.rs`):

```json
{
  "type": "https://joinedcontext.com/errors/resource-not-found",
  "title": "Resource Not Found",
  "status": 404,
  "detail": "ContextSpace 'mobility-traffic' does not exist in project 'city-center'"
}
```

`type`, `title` and `status` are always there; `detail` is the one sentence a page shows as it is.
No route sets `instance`: the member stays absent rather than repeating the URL the caller just
called.

One optional member is added to RFC 7807's own: `errors`, a list of strings, present when a single
request violated more than one rule and the caller can act on all of them at once. A form marks
every bad field from it in one pass instead of sending the user round the loop once per mistake
(CC-24). `detail` stays the one-sentence summary, so a client that ignores `errors` still has
something to show.

```json
{
  "type": "https://joinedcontext.com/errors/invalid-request",
  "title": "Invalid Request",
  "status": 400,
  "detail": "the parameters do not match blueprint 'threshold-alert': \"title\" is a required property; \"webhookUrl\" is a required property",
  "errors": [
    "\"title\" is a required property",
    "\"webhookUrl\" is a required property"
  ]
}
```

`type` is `https://joinedcontext.com/errors/{slug}`, and the slug is what a client branches on:

| Slug | Status | What it means |
|---|---|---|
| `invalid-request` | 400 | the body or a parameter is wrong; `errors` carries every violation at once |
| `unauthorized` | 401 | no live session and no accepted bearer token |
| `forbidden` | 403 | the verb the caller's bindings lack, named in `detail` (PF-50) |
| `self-approval` | 403 | the author of a proposal approving their own (CC-41) |
| `resource-not-found` | 404 | not there, or not readable by this caller: one answer for both (R20) |
| `conflict` | 409 | the state moved under the request, or a name is taken |
| `unsupported-media-type` | 415 | a `PATCH` whose content type is neither patch type of section 4 |
| `not-implemented` | 501 | the route exists and this form of it does not, such as an import from a URL |
| `service-unavailable` | 503 | a tier the route needs did not answer: the forge, the database, Model Tools, a runner |
| `internal-error` | 500 | anything else; `detail` says nothing about the cause |
| `rate-limit-exceeded` | 429 | the basemap proxy's own bucket, with the limit in `detail` |

Three answers carry a JSON document of their own instead of a problem, all `application/json`:

- the verdict gate's `409` (sections 4 and 21): `error`, `check`, `reason`, `detail`;
- the routes the operations registry answers directly, `POST …/ops/{name}` and the draft routes
  of section 4: `403 {"error": "forbidden", "role": …}`,
  `422 {"error": "invalid_input", "path": …, "message": …}` and
  `409 {"error": "draft_conflict", "current": …}` (`joinedcontext-portal/src/ops/mod.rs`);
- `POST /api/v1/mcp`, which answers JSON-RPC errors because its clients read that shape
  (section 21).

A client that reads the status code and then `type` or `error` handles all of them.

## 3. Session and Identity (CC-40, CC-42)

Humans reach the portal through the browser, never with a bearer token of their own. The
authorization code flow with PKCE runs server-side and the result is an encrypted session cookie
(`__Host-jc_session`, `Secure`, `HttpOnly`, `SameSite=Lax`); a second, deliberately readable
cookie (`__Host-jc_csrf`) carries the double-submit token the SPA echoes in `x-csrf-token` on
every mutation. Every Portal cookie (`__Host-jc_session`, `__Host-jc_refresh`,
`__Host-jc_oidc_flow`, `__Host-jc_csrf`) and every edge session cookie carries the `__Host-`
prefix, `Secure` and `Path=/` and no `Domain`, removals included: an App on
`{name}.apps.{domain}` may set a cookie for `{domain}`, and the browser keeps none of those under
a `__Host-` name, so a planted cookie never rides beside the real one under its name (AP-29,
T-4395).

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/api/v1/auth/login?redirect_to=<in-app path>&lang=<language>` | starts the code flow; `redirect_to` is honoured only when it is a same-origin path; `lang` (a 2–3 letter language tag, the Portal's current language) goes to the realm as `ui_locales`, so the sign-in page speaks it (T-4199) |
| `GET` | `/api/v1/auth/callback` | consumes the code, verifies the ID token against the stored nonce, issues the session |
| `GET` | `/api/v1/auth/me` | the signed-in identity, or `401` `application/problem+json` when there is no live session |
| `GET` | `/api/v1/auth/sso-check` | a redirect into the realm with `prompt=none` for the public `portal-ui` client, answered to `/api/v1/auth/sso-check/done` in the fragment: `#error=login_required` when the person must sign in, `#code=…` when the realm session lives; the Open page reads it from a hidden frame of its own origin to decide whether a silent App means "sign in again" (AP-122); the code is never redeemed |
| `GET` | `/api/v1/auth/sso-check/done` | an empty page the check lands on; the answer is in its fragment, which never reaches the server |
| `POST` | `/api/v1/auth/logout` | clears the session and answers `{ "endSessionUrl": "…" }`; the SPA navigates there itself, because a cross-origin 302 to Keycloak is unreadable to `fetch` |
| `POST` | `/api/v1/auth/backchannel-logout` | Keycloak-initiated logout; revokes every session issued at or before the token's mark |

`GET /api/v1/auth/me` answers with the identity:

```json
{
  "subject": "b7c1e0f4-2c17-4b7c-9a2f-8f0b0f6b3d21",
  "username": "jana.kovacova",
  "email": "aino.virtanen@example.org",
  "name": "Aino Virtanen",
  "roles": ["portal-viewer", "space-editor"]
}
```

- **CC-42** [portal, identity] — `roles` carries the realm roles Keycloak asserts in the ID token
  (`realm_access.roles`), in the order the token lists them, and is `[]` when the token asserts
  none. The UI MUST use it to decide what it renders and enables; it is defence in depth only, and
  the forge and the resource API enforce the same boundaries independently, so a bypassed UI can
  never exceed a user's Git rights.
- **CC-40** [portal, identity] — `subject` is the Keycloak `sub` and is the only durable user key;
  `username`, `email` and `name` are display data and MUST NOT be used for authorization.
- No token is ever returned to the browser and none is written to a log line.

## 4. Resource API (MF-11…MF-15)

Every configuration kind is also served as a resource collection under the same `/api/v1` prefix as the rest of the Portal API (no Kubernetes-style group path in URLs; `apiVersion`/`kind` live only inside the manifest body):

```text
GET    /api/v1/projects/{project}/{plural}            list (labelSelector, fieldSelector, limit, continue)
GET    /api/v1/projects/{project}/{plural}/{name}     one manifest incl. status
POST   /api/v1/projects/{project}/{plural}            create  → 202 + Change (merge request)
PUT    /api/v1/projects/{project}/{plural}/{name}     replace → 202 + Change
PATCH  /api/v1/projects/{project}/{plural}/{name}     application/merge-patch+json or
                                                      application/apply-patch+yaml → 202 + Change
DELETE /api/v1/projects/{project}/{plural}/{name}     → 202 + Change (explicit deletion lane)
POST   /api/v1/projects/{project}/{plural}?dryRun=All validate + plan, no change created
DELETE /api/v1/projects/{project}/{plural}/{name}?dryRun=All
                                                      the removal's plan and `goesWith`, no change created
GET    /api/v1/projects                                 the projects this caller may read
GET    /api/v1/blueprints                               the Blueprint catalogue of the organization
GET    /api/v1/endpoints                                every Endpoint of every project, each with its project: administrators of the organization only (PF-61)
GET    /api/v1/organization/datamodels?search=          every DataModel the caller may read, and matching Smart Data Models entries: what the model and type pickers list (DM-63)
POST   /api/v1/projects                                 open a project → 202 + Change: project.yaml and the creator's steward binding in one merge request (PF-65, PF-66)
GET    /api/v1/projects/{project}                       the project and `status.usage`: what it holds of each quota (PF-73, PF-75)
DELETE /api/v1/projects/{project}                       delete a project → 202 + red-lane Change over everything it holds (PF-77, PF-78)
POST   /api/v1/projects/{project}/duplicate             duplicate a project into a new slug → 202 + Change: its registry entry and the caller's steward binding (PF-89)
GET    /api/v1/projects/{project}/registry              the project's registry entry: repository, pinned ref, parameter values, the declarations at that ref and the repository's tags (PF-86, CC-88)
PUT    /api/v1/projects/{project}/registry              pin another ref or set other parameter values → 202 + red-lane Change on the organization repository (PF-86, CC-88)
GET    /api/v1/projects/{project}/permissions/me        the caller's effective rules here (PF-51, PF-61)
GET    /api/v1/projects/{project}/apps/{name}/me        the caller's roles in one published App, for a fullstack backend (AP-109)
GET    /api/v1/projects/{project}/apps/{name}/audience  how many people each visibility of the App reaches, for the Share dialog: `read` on `App` (AP-173)
GET    /api/v1/shared-apps                              the published Apps of other projects shared with the caller's organization that the caller may open (AP-174)
```

`DELETE /api/v1/projects/{project}` proposes one red-lane `Change` whose merge request removes
`projects/{project}/` entirely and, with it, every `RoleBinding` and project `Role` under
`users/` whose scope is the project or one of its spaces: no grant outlives the project it was
written for (PF-77). The `Change` body lists every file, so the approver reads the cascade before
approving it, and the approval is held to `approve` and `delete` on every kind in that list. A
`SharedSpaceReference` in another project pointing at one of this project's Endpoints refuses the
deletion with `409` and names the referencing manifests; removing the reference is that project's
own change. On merge the reconciler drops each space's broker tenant last, after the export the
`Change` body offers (CC-07), retires the endpoint slugs and removes the app builds.

`DELETE /api/v1/projects/org/groups/{name}` works the same way for a `Group` (PF-95): the one
red-lane `Change` removes the group's manifest and, in the same commit, takes the group out of
every `RoleBinding` subject list and every `App.spec.access` entry naming it. A binding left with
no subject is removed, and so is an access entry left with none; a removal that would leave the
organization without an administrator is `409` (PF-03). A binding or an App in a project with a
repository of its own (layout 2) cannot ride in that `Change`, so it refuses the deletion with
`409` naming it; removing the group there is that project's own change.

The name is then reserved for `Organization.spec.projects.nameCooldownDays` (30 days by default,
`0` for none) counted from the commit that removed it, and `POST /api/v1/projects` answers `409`
with the date it becomes free (PF-78).

`POST /api/v1/projects/{project}/duplicate` takes `{name, displayName?, parameters?}` and answers
what `POST /api/v1/projects` answers for `name`: the same checks on the new slug (a DNS-1123 label,
not taken, not proposed, past its cooldown) and the same `202` with the organization's `Change`,
which carries the registry entry `projects/{name}.yaml` (`spec.repository.name` the new slug, `ref`
`main`, the `parameters` given) and the caller's steward binding for the new project. It is held to
`read` on the origin and to what opening a project needs (`Organization.spec.projects.creation`).
Before the `Change`, the Portal copies the origin's repository with its whole history into a new
private repository named after the slug, with `main` protected; that copy is PF-89's fork, made
through the forge's migration from its own address because the forge refuses a fork into the
owner that already holds the origin. A repository of that name already there is `409`, never
adopted, and a failed `Change` removes the copy again, as opening does (CC-85). The origin is only
read: the copy's teams and bindings are its own and grant nothing in the origin (PF-83), and its
ids render from the new slug (PF-79). Layout 1 answers `409`, because a project there has no
repository to copy; its duplicate is an import of its export under the new name (MF-45).

`GET /api/v1/projects/{project}/registry` answers what the registry entry `projects/{project}.yaml`
of the organization repository says this deployment runs: `{repository, ref, parameters,
declarations, tags}`. `declarations` are the `spec.parameters` of the project's own `project.yaml`
at the pinned ref, so the parameter form shows the knobs of the release that runs; `tags` are the
project repository's tags with their commits, as the forge lists them: the releases a pin may
name (CC-88). An external repository (CC-89) answers no tags and no declarations. It is held to `read` on the project; a caller without it gets `404`, and layout 1, where a
project has no registry entry, answers `409`.

`PUT /api/v1/projects/{project}/registry` takes `{ref?, parameters?}`, at least one of them, and
proposes the edited registry entry as one red-lane `Change` on the organization repository: the
only door that repoints what a project runs (PF-86). `ref` is a tag, a branch or a commit of the
project repository, and the Portal reads `project.yaml` there before proposing, so a ref the
repository does not hold, or one without a `project.yaml`, is `400` naming the ref.
`parameters`, when given, is the whole set of this deployment's values and replaces the entry's
(an empty object returns every knob to its default); each value is checked against the
declarations of `project.yaml` at the ref the entry will pin, and an undeclared name, a value that
does not fit or a declared parameter left with neither value nor default is `400` naming it. A
parameter of type `secret` takes a secret's name, never its value (CC-88). The `Change` carries
only `projects/{project}.yaml`, on the branch `portal/update-project-{project}-registry`, and the
organization's change list shows it as the `Project` it changes; a second repoint while
one is open is `409` naming it, and a body that changes nothing is `409`. It is held to `propose`
on `Project` at the organization, the rule that edits the project's title there, because the entry
is the organization's manifest of the project: a steward of the project alone gets `403`. It
applies only through the organization's red-lane approval
(PF-58); once merged, the checkouts fetch the new ref and the next render runs it (CC-86, CC-90).
For an external repository the ref is not read before proposing; a ref the checkouts cannot fetch
renders at the last fetched ref and says so on the entry's status (CC-86).

`GET /api/v1/endpoints` is the organization-level Endpoints page (PF-61), an administration view:
an administrator of the organization (`approve` and `delete` on `RoleBinding` at organization
scope, PF-03, as `org-admin` holds them) reads every Endpoint of every project, each with its
project, and every other signed-in caller gets `404`, the answer for a page that is not theirs
(R20). The MCP operation `jc_endpoint_list_all` is the same read with the same refusal. A
project's own Endpoints stay at `GET /api/v1/projects/{project}/endpoints`, under that project's
`read`.

`GET /api/v1/organization/datamodels` is the one list behind the data model and type pickers
(DM-63, ADR-N-033). `items` holds every `DataModel` of every project whose manifest the caller may
`read`, project by project and space by space (PF-60), and every organization model when the caller
holds any binding in the organization (DM-75), a retired version excepted (DM-26), each as
`{name, level, project, space?, version, lifecycle, classes, origin?}`: `level` is `organization`
or `project` (DM-79), `project` is `org` for an organization model, `space` is absent on a model
no space owns, and `origin` (`{project, space?, name, version}`) names the project model an
organization model was shared from (DM-77). Sorted organization models first, then by project,
space and name.
`smartDataModels` holds up to 50 entries of the Smart Data Models catalogue index (DM-12) as
`{id, name, subject, description}`, and only when `search` has two characters or more: the index
holds about a thousand models, and a picker lists the organization's own first. `search` matches
case-insensitively on a model's name, project, space and classes, and on a catalogue entry's name,
id and description; longer than 100 characters is `400`. A catalogue that Model Tools cannot
answer leaves `smartDataModels` empty and sets `catalogueUnavailable` to the reason a person can
act on, and the organization's own models are still listed. A caller no binding names gets empty
`items`, never a `403` (R20).

Every list and get answers under `read` of a binding whose scope covers the project (PF-59, PF-60):
a project the caller may not read is `404` on every route of this section, on `export`,
`/revisions`, `permissions/me` and the MCP resources alike, the one answer for "missing" and
"not yours". A write answers `403` with the missing verb (PF-50).

A list takes `labelSelector`, `fieldSelector`, `limit` and `continue`. A list and a get both take
`revision={commit}` (MF-11, MF-16): the project's subtree of the repository at that commit id, read
from the forge the way `GET …/export?revision={commit}` of section 10 reads it, with no `status`,
because status is what the Portal computes now. Only a commit id (7 to 40 lowercase hex digits) is
a revision; a branch name is `400`, because it would read a workspace's unmerged edits past the
workspace's own door (CC-76), and so is `revision` beside `workspace`. A commit the repository does
not know is the caller's own `404`, the same as a resource the commit did not hold (R20). The read
grants are the caller's current ones.

`GET /api/v1/projects` answers a `kind: List` of the projects the caller may read (PF-59). Each
item carries `name` (the slug, the `{project}` of every other path), `sample` (whether its
`Project` manifest is labelled `joinedcontext.com/sample`, PF-109) and `title`, the Project
manifest's `metadata.title`: one string, or the legacy map per locale, absent when the manifest
names none (T-4243). The project switcher and the breadcrumb show the title in the person's
language and the slug beside it, without one request per project. A project no binding covers is
not in the list, which is the same answer as `404` on its routes and discloses no department's
project list to a session that may not see it.

```json
{
  "apiVersion": "joinedcontext.com/v1alpha1",
  "kind": "List",
  "items": [
    { "name": "banskabystrica", "sample": false, "title": { "sk": "Mesto Banská Bystrica", "en": "City of Banská Bystrica" } },
    { "name": "helsinki-mobility", "sample": true }
  ]
}
```

`{plural}` is the kind's plural from the catalogue in
[Development/04-manifest-kinds.md §2](../Development/04-manifest-kinds.md#2-standard-kind-catalog),
so a new kind is served the moment it is registered and needs no route of its own. `datasources`
is the newest of them: the connection of one external feed, described in
[Architecture/08-pipelines.md §6](../Architecture/08-pipelines.md#6-external-feeds-the-datasource-kind-mf-35-pl-39).
Every kind has a plural of its own, so a route never has to guess the kind: a `ScopeDefinition`
answers under `/api/v1/projects/{project}/scopedefinitions` although its file is
`projects/{p}/policies/{name}.yaml`, and `policies` is `Policy` alone (R19).
A write carrying a credential as a literal string rather than a `secretRef` is refused with `400`
before any merge request exists (MF-24, CC-06), whatever the kind.

Portability operations:

```text
GET  /api/v1/projects/{project}/export?format=yaml|json|zip|git&revision={commit}&kinds=…&names=…   section 10
GET  /api/v1/projects/{project}/revisions?limit=20                                            section 10
POST /api/v1/projects/{project}/import        multipart (file) or JSON manifests; fields: targetNamespace, conflictPolicy, dryRun; {"url": …} is 501
GET  /api/v1/projects/{project}/syncsources/{name}/status   what the loop reports (section 10)
POST /api/v1/projects/{project}/syncsources/{name}/sync     trigger a run now
POST /api/v1/projects/{project}/syncsources/{name}/pause    switch the loop off or back on
POST /api/v1/projects/{project}/syncsources/{name}/detach   stop syncing: a merge request removing the source
```

Export and import are per project. There is no organization-wide pair: moving a whole
organization is every project exported in turn, and a route that took one archive for all of
them would also take one conflict policy for all of them, which is a decision per project.

`GET /api/v1/projects/{project}` carries the count and the limit of every countable quota
dimension, so a person sees the limit before the verdict does (PF-75). The limit is the one in
force: the project's own `spec.quotas` when it has one, else `Organization.spec.projects.quota`
(PF-73). A dimension no quota limits carries its count and no `limit`.

The answer is the `Project` manifest as the resource API serves it, with one `status` the stored
manifest does not carry:

```json
{
  "status": {
    "usage": {
      "apps": { "used": 0 },
      "contextSpaces": { "used": 2, "limit": 3 },
      "publicEndpoints": { "used": 1, "limit": 4 },
      "residentPipelines": { "used": 3, "limit": 3 }
    }
  }
}
```

A write that would put the project over one of them is refused before a `Change` exists, on every
door, naming the count and the limit — `quota: residentPipelines 4 of 3 in project ovzdusie` —
and the dry run of the same manifest answers the same refusal (PF-74).

`apps/{name}/me` answers `{id, name, email, roles}` for the caller, the same object the static
host writes into `#jc-config` (AP-95), with `roles` computed from the published App's
`spec.access` as AP-92 computes them. A `fullstack` backend calls it with the edge's
`X-Access-Token` as `Authorization: Bearer` (Architecture/16 §13). It is the one
route of this section answered without `read` on the project: an `organization` App admits
people who hold no rule in its project, the answer is only about the caller, and it names no
other resource. An App that
is not published, or does not exist, is `404`; no valid token is `401`.

`permissions/me` answers the rules in force for the caller in one project, each grant naming the
scope it was inherited from, and what the organization's own settings let them do that no rule
expresses (PF-61, PF-65, UI-44):

```json
{
  "project": "helsinki",
  "bootstrap": false,
  "grants": [
    {
      "role": "steward",
      "binding": "hel-stewards",
      "scope": "organization",
      "rule": { "kinds": ["Endpoint", "Pipeline"], "verbs": ["read", "propose"] }
    }
  ],
  "projects": { "creation": { "allowed": false, "reason": "opening a project here needs propose on Project, which org-admin holds (PF-65)" } }
}
```

`scope` is `organization`, `project:{name}` or `contextSpace:{name}`, so a page can say "steward,
inherited from the organization" rather than making an inherited grant look local. `projects.creation`
is the answer `POST /api/v1/projects` would give this caller, with the refusal in the API's own
words: the "New project" control is rendered disabled with that reason, never hidden (UI-44).

### Files beside the manifest (DM-39)

Some kinds are incomplete as one file. A `Mapping` names golden tests it cannot be accepted
without (DM-39), and the examples that satisfy them are two more files in the repository. A
`POST`, `PUT` or `PATCH` body may therefore carry a `files` member beside the manifest, taken
out of the body the way `draft` is, and the Change commits them to the same branch and the same
merge request as the manifest (a request body, so the manifest is shown in excerpt):

```json excerpt
{ "apiVersion": "joinedcontext.com/v1alpha1", "kind": "Mapping", "metadata": { "name": "sdm-airquality-to-bb" },
  "spec": { "tests": [{ "input": "./tests/sdm-airquality-to-bb.input.json",
                        "expect": "./tests/sdm-airquality-to-bb.expect.json" }], "…": "…" },
  "files": { "./tests/sdm-airquality-to-bb.input.json": "{ \"id\": \"urn:ngsi-ld:AirQualityObserved:…\" }",
             "./tests/sdm-airquality-to-bb.expect.json": "{ \"id\": \"urn:ngsi-ld:BBAirQuality:…\" }" } }
```

Every path is relative to the manifest's own folder and stays under it: an absolute path, a
`..` segment, a path outside the kind's folder, an empty path and a duplicate are `400`, naming
the path. A body may carry at most 16 files and 256 KiB of them together; more is an import
(`POST …/import`), which is the route for a repository's worth of files. The files are written
before the manifest, so a reviewer opening the merge request never reads a manifest that points
at a file the change does not carry, and one refusal leaves nothing behind. A file the manifest
does not name is written as sent and reviewed like any other: the plan lists it.

Example dry-run result (`?dryRun=All`; `probe` only for an `http` DataSource, MF-39). The
`verdict` is recorded under the manifest's own kind and name, and it is what the proposal of the
same manifest needs under `strict` (PF-57):

```json
{
  "valid": true,
  "lane": "yellow",
  "plan": { "summary": "create hsl-citybikes-free", "fields": [ { "path": "spec.http.url", "to": "https://…/free_bike_status.json" } ] },
  "probe": { "records": 1, "bytes": 512034, "sample": { "last_updated": 1789314850, "data": { "bikes": [] } } },
  "verdict": { "ok": true, "findings": [], "checkedAt": "2026-09-18T12:00:00Z", "inputDigest": "sha256:…" }
}
```

The dry run of a `DELETE` adds `goesWith`, what leaves with the resource once its removal is
approved, so the person sees it before typing the name (T-3247). Each entry is `what` and `count`,
and `names` (`Kind/name`) when they are resources:

| `what` | Kind | Comes back with a restore |
|---|---|---|
| `entities` | ContextSpace | yes: the broker keeps them, and they are served again once the space is restored |
| `rejectedRecords` | Pipeline | no: the reconciler drops a removed pipeline's refused records (PL-61) |
| `runs` | Pipeline | no: and its run history (PL-62) |
| `grants` | App | yes: the endpoint and the policies the App was given leave in the same change |
| `bindings` | Group | yes: the role bindings that name it, edited or removed in the same change (PF-95) |

A count the store cannot give is left out rather than guessed; `goesWith` is absent when nothing
leaves with the resource.

Under `strict`, a `POST`, `PUT` or `PATCH` that carries no `draft` and whose manifest has no
fresh green verdict is refused after the manifest's own checks and before anything reaches the
forge, with the gate's own document (`reason` is `verdict_absent`, `verdict_failed` or `stale`;
`check` names the check of the kind):

```json
409 { "error": "verdict_required", "check": "jc_manifest_dry_run", "reason": "verdict_absent",
      "detail": "The manifest has not been checked; check it, then propose it." }
```

The check is the same request with `?dryRun=All`, so a client sends the manifest twice: first to
check it, then, unchanged, to propose it. The `409` is a problem document of type
`verdict-required` (`application/problem+json`) that keeps `error`, `check` and `reason` beside
`type`, `title` and `status`. `?validate_only=true` (AIP-163) is the same check under
the name other API styles use: every write the resource API takes (create, replace, patch, delete,
a draft, a workspace write, a model's source, an import) answers it as `?dryRun=All`, saves nothing
and returns the plan; a caller who may not write the resource gets the answer `?dryRun=All` gives
them (T-5949). A proposal that became a Change forgets the draft its
check created; a person's draft of the same resource with other content stays.

A DataSource with an `authorization` answers `"probe": { "skipped": "the source declares a credential; a dry run resolves none (MF-38)" }`.

Example write result:

```json
{
  "apiVersion": "joinedcontext.com/v1alpha1",
  "kind": "Change",
  "metadata": { "name": "chg-7f3a", "namespace": "helsinki" },
  "status": {
    "lane": "yellow",
    "mergeRequest": "https://git.example.fi/hel/org/pulls/412",
    "plan": { "create": 1, "update": 0, "delete": 0 },
    "phase": "PendingApproval"
  }
}
```

A Change proposed inside an MCP `tools/call` also carries `status.proposedVia`, for example
`{ "channel": "mcp", "client": "mcp-mobile" }` (AG-137, ADR-N-060 §2.2): `client` is the token's
`azp`, at most 64 characters of `[A-Za-z0-9._:-]`, otherwise `unknown`. Every commit of that call
ends in the trailer `Proposed-Via: mcp; client=mcp-mobile`, and its merge request body opens with
"Proposed through an AI client over MCP (client `mcp-mobile`)." and ends in the same trailer. The
Portal reads `proposedVia` back from that last line, so the approval page shows it as a badge.
A Change proposed in the Portal or with the person's own token has no `proposedVia`.

A change a connector opens (`/c/{id}`, AG-144) also names the connector and, for
`jc_change_apply`, the plan it took (T-5973): `proposedVia` gains `connector` (the id),
`connectorName` and `plan`, the trailer reads
`Proposed-Via: mcp; client=claude-code; connector={id}; connectorName=doprava-in-claude; plan={planId}`,
and the merge request's first line adds "through connector `doprava-in-claude`". Each value comes
from the verified token and the stored connector, never from a tool argument; a trailer with a
key outside these or a value outside `[A-Za-z0-9._:-]` shows no `proposedVia`. The `mcp.tool`
activity rows carry `details.connector` and `details.connectorName`, and
`GET /api/v1/projects/{project}/activity?connector={id}` (and its stream) keeps only that
connector's rows; an id that is not 26 characters of `a-z2-7` is 400 naming `connector`.

### Drafts of a manifest (AG-61, UI-47, UI-48)

A form saves what a person has typed before anything is proposed. A draft is shared, not
per-browser: the window, an assistant run and an MCP client see the same one, so a person who asks
the assistant to fill a field watches it appear in the open form
(`joinedcontext-portal/src/api/drafts.rs`).

```text
GET    /api/v1/projects/{project}/drafts                       one line per draft
GET    /api/v1/projects/{project}/drafts/{kind}/{name}         one draft with its manifest and verdict
PUT    /api/v1/projects/{project}/drafts/{kind}/{name}         save it; body { "manifest": …, "expectedVersion": n }
DELETE /api/v1/projects/{project}/drafts/{kind}/{name}         discard it → { "dropped": true }
GET    /api/v1/projects/{project}/drafts/events                the same changes as Server-Sent Events
```

```json
{
  "project": "helsinki",
  "kind": "DataSource",
  "name": "hsl-citybikes-free",
  "manifest": { "…": "the manifest as the form holds it, whole" },
  "verdict": { "ok": true, "findings": [], "checkedAt": "2026-09-18T12:00:00Z", "inputDigest": "sha256:…" },
  "touchedBy": "jana.kovacova",
  "touchedKind": "person",
  "version": 7,
  "updatedAt": "2026-09-18T12:00:04Z"
}
```

- Writing or discarding a draft needs `propose` on its kind, the verb the proposal itself needs:
  a draft is the proposal before it is sent, and a caller who may not propose an `Endpoint` may not
  park one either. Reading one asks what reading the manifest asks, so a draft of a space the
  caller may not read is `404` like the manifest would be (PF-59, R20).
- `version` counts up on every save. Send the version you read as `expectedVersion` and a save
  against a newer draft is refused with `409 {"error": "draft_conflict", "current": 9}` instead of
  overwriting what the other window wrote; omit it and the last save wins.
- `touchedBy` is the caller's username and `touchedKind` is how they reached it: `person`, `mcp`
  or `run`, so a form can say the assistant wrote this, not you.
- A literal secret under one of the secret keys of section 5 is `400` here as it is on a write
  (MF-24): a draft is a manifest on its way to Git.
- `?workspace={name}` addresses the drafts of one workspace (section 22). A draft of a workspace is
  not a draft of the project, and the query is absent for the project's own.
- `/drafts/events` streams `put`, `verdict` and `drop` events, each carrying `kind`, `name`,
  `version`, `touchedBy`, `touchedKind` and `updatedAt`, with the SSE id set to the version. Every
  event is checked against the reader's bindings as it passes, so a revoked binding stops the flow
  without a reconnect. A keep-alive comment goes every fifteen seconds and the answer carries
  `Cache-Control: no-cache` and `X-Accel-Buffering: no`, so no proxy buffers the stream.
- A draft never becomes a resource by itself. A proposal names it
  (`{"draft": {"kind": …, "name": …}}`, section 21) or carries the manifest; the Change that
  results forgets the draft its check created.

## 5. Change Proposals and Approvals (CC-34, CC-41, CC-63, UI-23…UI-25)

A write answers with a `Change` and leaves a merge request open. These endpoints are the Portal's
view of those merge requests, so that an approver never has to open the forge to do their job:

```text
GET  /api/v1/projects/{project}/changes                  open proposals, newest first
GET  /api/v1/projects/{project}/changes/history          merged and rejected ones, newest first, paged
GET  /api/v1/projects/{project}/changes/history/export   the same, as CSV (T-5114)
POST /api/v1/projects/{project}/changes/{id}/restore     propose again what a merged removal took
POST /api/v1/projects/{project}/changes/{id}/undo        propose again what a merged update altered
GET  /api/v1/projects/{project}/changes/{id}/manifests   the manifests a merged change wrote
GET  /api/v1/projects/{project}/changes/{id}             one proposal with its plan diff
POST /api/v1/projects/{project}/changes/{id}/approve     review + merge
POST /api/v1/projects/{project}/changes/{id}/reject      review "request changes" + close
```

A write is judged against the Portal's copy of the repository and cut from the default branch.
When the default branch already holds the resource otherwise than that copy (another `spec`,
labels, annotations, title or description at the path the write goes to), the write is `409
conflict`, "changed on main after the Portal last read it", and nothing is committed or published:
the caller reads the resource again and sends the change once more, so no Change ever sets back
what main moved to (T-2674).

`{id}` is the `metadata.name` a write returned: `chg-` plus the merge request number in eight
lowercase hex digits. A Change targets one repository (CC-87), which `status.repository` names. In
layout 2 ([ADR-N-029](../Decisions/adr-n-029-one-repository-per-project.md)) a project's own
kinds land in the project repository and the organization kinds in the organization repository, so
under `/projects/{project}/changes` `chg-{hex}` is a merge request of the project repository and
`chg-org-{hex}` one of the organization repository; the two number their merge requests apart. A
layout 1 organization has one repository and only the first form.

A listed proposal carries what a reviewer decides on, not what the forge stores:

```json
{
  "apiVersion": "joinedcontext.com/v1alpha1",
  "kind": "ChangeList",
  "items": [
    {
      "apiVersion": "joinedcontext.com/v1alpha1",
      "kind": "Change",
      "metadata": { "name": "chg-0000019c", "namespace": "helsinki" },
      "status": {
        "lane": "red",
        "repository": "helsinki",
        "mergeRequest": "https://git.example.fi/hel/helsinki/pulls/412",
        "plan": { "create": 0, "update": 1, "delete": 0 },
        "phase": "PendingApproval"
      },
      "summary": { "key": "change.summary.update", "params": { "kind": "Endpoint", "name": "public-air", "fields": 1 } },
      "author": { "name": "Aino Virtanen", "email": "aino.virtanen@example.org" },
      "createdAt": "2026-09-06T09:14:22Z"
    }
  ]
}
```

`summary` is a message key and its parameters, derived from the plan and never from the raw diff
(CC-34). The sentence is composed in the UI, which already carries the sk/en/de/cs catalogues; the
API stays language-free so a second set of translations never has to be kept in step with the
first. `author` is read from the commit, so a proposal always names the human who made it and never
a shared bot identity (CC-44). `GET …/changes/{id}` adds `plan.fields`, the field-level
diff of `PlanDiff`, with any value under a `password`, `token`, `secret`, `clientSecret` or
`apiKey` key replaced by `"[REDACTED]"` (CC-06).
It also lists `files`, every file of the merge request (`path`, `kind`, `operation`, `lane`), and
each manifest among them carries `fields`: its own field-level diff, base against head, redacted the
same way, so an approver reads what each file of a bundle changes and not only the headline
manifest's. A native file (a LinkML source, a `bento.yaml`) carries no `fields`.

`GET …/changes/history` lists the closed changes, most recently closed first, with the read rule of
the open list: a project no binding covers is `404`, and a change to a kind the caller does not read
is not listed (PF-59, R20). Each item is a `Change` whose `status.phase` is `Merged` or `Rejected`
and whose `summary` names the resource; `plan` counts and the diff are not carried, since the
default branch already holds what a merged change made. `decision` says who decided it, as the
Portal wrote it on the forge: the approver of the merge commit, or the rejecter and the reason of
the rejection. A change merged or closed in the forge itself carries no `decision`.

| Parameter | Meaning |
|---|---|
| `page` | the page, from 1; `next` of the previous answer, present while the forge holds older ones |
| `kind` | one kind only, any case, as `Endpoint` |
| `name` | resource names containing this, any case |
| `author` | changes whose author's name or e-mail contains this, any case (T-3274) |
| `since`, `until` | changes decided on or after, on or before this day, `YYYY-MM-DD`; another form is `400` (T-3274) |

An unknown parameter or `page=0` is `400`. A page reads 20 closed merge requests of the forge, so it
can hold fewer items than that, none at all included, and still carry `next`.

`GET …/changes/history/export` takes the same parameters and read rule and answers `text/csv` as an
attachment `{project}-changes.csv`, one row per closed change: `closedAt`, `change`, `outcome`,
`kind`, `name`, `operation`, `proposedBy`, `decidedBy`, `reason`, `mergeRequest` (T-5114). It reads
up to ten pages from `page`; while the forge holds older ones, the `JC-Next-Page` header names the
page that continues the file. The proposer is a name, never an e-mail, and a cell starting with
`=`, `+`, `-` or `@` gets a leading `'` so a spreadsheet does not run it.

```json
{
  "apiVersion": "joinedcontext.com/v1alpha1",
  "kind": "ChangeList",
  "items": [
    {
      "apiVersion": "joinedcontext.com/v1alpha1",
      "kind": "Change",
      "metadata": { "name": "chg-0000019b", "namespace": "helsinki" },
      "status": { "lane": "yellow", "repository": "helsinki", "phase": "Rejected",
                  "mergeRequest": "https://git.example.fi/hel/helsinki/pulls/411",
                  "plan": { "create": 0, "update": 0, "delete": 0 } },
      "summary": { "key": "change.summary.update", "params": { "kind": "Endpoint", "name": "public-air", "fields": 0 } },
      "author": { "name": "Aino Virtanen", "email": "aino.virtanen@example.org" },
      "createdAt": "2026-09-05T08:02:10Z",
      "fileCount": 1,
      "decision": { "by": "mikko.approver@example.org", "at": "2026-09-05T11:40:00Z", "reason": "the URL is the old one" }
    }
  ],
  "next": 2
}
```

`POST …/changes/{id}/restore` brings back what a merged change removed (T-3247). It reads every
file the change deleted under `projects/{project}/` at the commit its branch was cut from, and
proposes them again as one new Change (`202`, `PendingApproval`). An approver decides that Change
like any other. It needs `propose` on every kind it brings back (`403` names the kind). A change
that is not merged, that removed nothing, or whose resource is in the project again answers `409`
in words. What the reconciler dropped with the resource, a pipeline's refused records and runs,
does not come back.

`POST …/changes/{id}/undo` puts back what a merged change modified (T-3274): every file it changed
under `projects/{project}/`, read at the commit its branch was cut from, proposed as one new Change
(`202`). It answers `409` while any of those files holds something other than what the change wrote,
so an undo never drops a later change; a change that is not merged or that modified nothing is `409`
too (a removal is restored, a creation removed). It needs `propose` on every kind it puts back.

`GET …/changes/{id}/manifests` answers the manifests a merged change wrote, each read at the
change's own last commit, as a `List` of resources (T-3975). It is how a resource's editor offers
"Restore this version": the person loads one into the form, checks it and proposes it as a new
Change, so nothing is written by this read and the restore goes through Check and approval like any
edit. It carries what Git holds, so a `secretRef` and never a value. A manifest of a kind the caller
does not read is left out, as is a native file (a LinkML source, a `bento.yaml`) and a file the
change deleted. A change the caller may not read is `404`; one that is not merged is `409`, since
what it proposed never was a version of the resource.

```json
{
  "apiVersion": "joinedcontext.com/v1alpha1",
  "kind": "List",
  "metadata": {},
  "items": [
    {
      "apiVersion": "joinedcontext.com/v1alpha1",
      "kind": "Pipeline",
      "metadata": { "name": "aq-mqtt-ingest", "namespace": "banskabystrica" },
      "spec": { "class": "resident", "secretRefs": [{ "name": "mqtt-credentials", "key": "password", "envVar": "MQTT_PASSWORD" }] }
    }
  ]
}
```

The page of one change reads its "before" at the commit its branch was cut from once the change is
closed (`merge_base`): after a merge the default branch already holds what it made, so a merged
change's `planFields` say what it changed and not nothing.

Approving a change records a `change.merged` activity event naming the change, its resource and the
approver ([§14](#14-activity-ui-31-ops-48-ops-49)), so the project's activity says what went live and
links to it, beside the reconciler's `config.applied` for the commit. Rejecting one records a
`change.rejected` event naming the change, its resource, the rejecter and the reason (at most 500
characters), so a decision not to ship is on the feed too and not only on the forge.

Approval rules, enforced by the API and not only by the UI:

- the caller needs the `portal-approver` realm role; anyone else gets `403` (CC-41);
- the author of a proposal may not approve it — `403` with `type: ".../self-approval"`;
- a proposal a person sends with a Portal session (the cookie or the edge's), when a binding
  covering the project grants them both `approve` and `delete` on every kind of every file it
  touches (an administrator, PF-58), is approved as it is proposed: the propose route answers
  `202` with the `Change` already `Deploying`, and the merge commit says `Approved in the Portal
  by {email}, its author, as an administrator of {kind}`. The same holds for `jc_resource_propose`
  and the other propose operations when the form calls them with that session. A red-lane
  proposal is approved so only with `?confirm=<resource name>` on the propose or `DELETE` route
  (the `confirm` of `jc_resource_delete`); without it the `Change` stays `PendingApproval` and
  its author approves it on the approval page with the name typed. Anyone else's proposal
  answers `202` `PendingApproval`. A bearer caller, MCP and an agent run never approve on
  propose, whoever runs them (AG-11, AG-82), and the bootstrap administrators' group does not
  count;
- a proposal in the `red` lane needs the `portal-approver` role **and** an explicit
  `{"confirm": "<resource name>"}` body, so a destructive merge is never one click (CC-19, CC-39);
- approving answers `202` with the `Change`, its `phase` moved to `Deploying`; the reconciler
  moves it to `Live` when the merge lands on the default branch and the mirror sync observes it.

## 6. Core Endpoint Examples

### List Context Spaces

```http
GET /api/v1/projects/{project}/spaces?limit=20 HTTP/1.1
Host: portal.example.joinedcontext.com
Authorization: Bearer eyJhbGciOiJSUzI1Ni...
```

A collection answers with a `List` envelope whose `items` are whole manifests; paging is a
`continue` token, not a page number, so a page stays stable while the collection changes.

```json
{
  "apiVersion": "joinedcontext.com/v1alpha1",
  "kind": "List",
  "metadata": {
    "continue": "eyJhZnRlciI6Im1vYmlsaXR5LWxpdmUifQ",
    "remainingItemCount": 3
  },
  "items": [
    {
      "apiVersion": "joinedcontext.com/v1alpha1",
      "kind": "ContextSpace",
      "metadata": {
        "name": "mobility-live",
        "namespace": "helsinki",
        "title": { "sk": "Živá mobilita", "en": "Live Mobility Feed" }
      },
      "spec": { "orgDomain": "hel.fi" },
      "status": {
        "phase": "Live",
        "observedRevision": "9c1f0ab",
        "sourceUrl": "https://git.example.fi/hel/org/src/branch/main/projects/helsinki/spaces/mobility-live/space.yaml"
      }
    }
  ]
}
```

`metadata.continue` and `metadata.remainingItemCount` appear only while more items follow.

`status.sourceUrl` is the forge page of the very file the manifest was read from, computed by the
Portal during the mirror sync and never read from Git (MF-04). It lets every view link "Source"
without teaching the browser where a kind lives in the repository; it is absent when the manifest
has not been mirrored yet or no forge is configured.

## 7. Pipeline Runtime Metrics (PL-24, CC-35)

A pipeline manifest says what should run; the numbers come from the Bento runner, which serves
Prometheus text on port 4195 (PL-24). The Portal reads that port on the browser's behalf, so the
UI never talks to a workload and the runner never needs an ingress:

```text
GET /api/v1/projects/{project}/pipelines/{name}/metrics    counters of one running stream
```

```json
{
  "pipeline": "aq-mqtt-ingest",
  "scrapedAt": "2026-09-06T16:20:11Z",
  "received": 128401,
  "sent": 128390,
  "errors": 3,
  "bufferDepth": 11,
  "latencyP99Ms": 42.5
}
```

- Counters are cumulative since the runner started, exactly as the runner reports them. The view
  samples them to show a rate; the Portal keeps no history, that is Prometheus' job (OPS-16).
- A field is absent when the runner does not export it. Absent is not zero.
- Only series carrying `stream="{project}.{name}"` are read, so one pipeline's view never
  discloses the traffic of its neighbours in the same runner, another project's pipeline of the
  same name included (T-3002).
- `503` with `problem+json` when the project has no runner configured or its metrics port does not
  answer within the scrape timeout. The view then shows the pipeline without numbers, not as
  failed: a runner that is down is an operational fact, not a manifest error.
- The body carries counters only. Runner logs, stream configuration and secret values are never
  part of it (PL-17).
- `nodes` carries the same counters per component of the stream, by the Bento `label` the
  reconciler writes (PL-66): `input` for the sources, `step_{j}` for the manifest's `j`-th
  processor step, `compute` for its compute step, `output` for the writes, and
  `processor_{at}` for what the platform adds around them. A label the author wrote stays the
  author's. The Studio paints each node of the flow with its own entry, so the step a message
  stops at is the one shown failing, live and not only in a test:

  ```json
  "nodes": {
    "input":   { "received": 1200, "sent": 1200 },
    "step_0":  { "received": 1200, "sent": 1180, "errors": 20 },
    "compute": { "received": 1180, "sent": 1180 },
    "output":  { "received": 1180, "sent": 1180 }
  }
  ```

Rates per node come from a second route, which the Studio polls every five seconds while its tab
is visible (PL-68):

```text
GET /api/v1/projects/{project}/pipelines/{name}/stats      counts per node over 1, 5 and 15 minutes
```

```json
{
  "pipeline": "hfp-vehicles",
  "scrapedAt": "2026-10-12T08:15:05Z",
  "coveredSeconds": 900,
  "nodes": {
    "input":  { "1m": { "received": 1200, "sent": 1200, "errors": 0 }, "5m": { "received": 6010, "sent": 6010, "errors": 0 }, "15m": { "received": 18050, "sent": 18050, "errors": 0 } },
    "step_0": { "1m": { "received": 1200, "sent": 1180, "errors": 20 }, "5m": { "received": 6010, "sent": 5990, "errors": 20 }, "15m": { "received": 18050, "sent": 18030, "errors": 20 } }
  }
}
```

- Each request scrapes the runner as `…/metrics` does and keeps the reading in the Portal's
  memory for 15 minutes, per pipeline; a reading younger than two seconds is reused, so many open
  tabs cost the runner one scrape. A window's count is what the counters grew by across the
  readings inside it; a counter that went down (the runner restarted) counts from zero. Errors
  and other counters the runner does not export stay absent, not zero.
- `coveredSeconds` is the age of the oldest sample kept. A window longer than it holds what was
  seen so far, so the first answer after a Portal restart has every window at zero and
  `coveredSeconds: 0`. The Portal keeps no history beyond that; Prometheus does (OPS-16).
- Same rights, same `404` and same `503` as `…/metrics`: a runner that is down is `503`, never a
  body of zeros, and the Studio shows "stats unavailable".
- The body carries counts only. The Studio shows received and sent of the last minute on each
  node and paints a node, and the wire into it, red while it has errors in the last five
  minutes. A click on a failing node shows its last error from `…/rejected`, the newest record
  of that step: the message and the time, never the record, whose values the rejected list
  shows masked to the same readers (PL-52, PL-61).

### 7b. The last messages through each step (T-3974, PL-66)

```text
GET /api/v1/projects/{project}/pipelines/{name}/samples    what last crossed each node of a running stream
```

The runner taps the stream after its source (`input`), after each step (`step_{j}`, `compute`)
and before the write (`output`), the labels of `nodes` above. A tap is a branch whose answer is
thrown away and whose failure is caught, so the Portal being away costs the stream nothing. It
sends the first 20 messages after the runner starts and then one in a hundred, to the internal
`POST /internal/pipelines/{project}/{name}/samples` the runner authenticates to like its
rejected sink; the Portal keeps the newest 20 per node in memory, each at most 8 KiB of JSON
(a larger one is kept as a note that it was too large), for 24 hours.

```json
{
  "pipeline": "aq-mqtt-ingest",
  "withheld": false,
  "nodes": {
    "input":  [{ "at": "2026-10-12T09:00:01Z", "record": { "id": "SK0263A", "pm10": 21.4 } }],
    "step_0": [{ "at": "2026-10-12T09:00:01Z", "record": { "id": "urn:ngsi-ld:AirQualityObserved:SK0263A", "pm10": 21.4 } }]
  }
}
```

- Reading needs `read` on `Pipeline` in the project; another project's pipeline, or one the
  caller may not read, is `404`.
- A sample shows no more than the pipeline's target Endpoint serves: the attributes its
  `projection.hiddenAttributes` hides are taken out, and every key of the secret list (MF-24)
  is masked, wherever it sits in the record.
- Before the mapping a record carries the source's own field names, so a hidden attribute is
  not recognisable there (Linked Events' organizer contact becomes `contactPoint` only in the
  mapping). When any target Endpoint hides an attribute, or a target cannot be read, `nodes`
  holds `output` alone and `withheld` is `true`; the Studio says so on the earlier steps.
- An empty `nodes` is a stream that has not run since the runner started, or a Portal that
  restarted; it is not an error.

### 7c. Runs, and one run now (PL-62, PL-78, T-4349)

```text
GET  /api/v1/projects/{project}/pipelines/{name}/runs    the latest runs with their counts (read on Pipeline)
POST /api/v1/projects/{project}/pipelines/{name}/runs    queue one run now (propose and approve on Pipeline)
```

A run is named by its due time, `2026-10-17T05:00:00Z`, and its log files every record under that name. The `POST` takes no body. It queues one run of a pipeline that runs from the queue (Architecture/08 §1, PL-78), outside its schedule, and answers `202` with the run it queued:

```json
{ "run": "2026-10-16T14:02:11Z" }
```

The run waits its turn in its lane, so runs started for 97 pipelines go one after another. A second `POST` while the run waits answers the same run. A pipeline whose run is in the runner answers `409` and names that run. A pipeline that waits for pushes, and so always runs, answers `409` too, as does one the reconciler has not deployed. A caller without propose and approve on Pipeline gets `403`, and one who may not read the pipeline gets `404`. Each accepted `POST` writes one `pipeline.run` activity event with the actor, the run and `object: pipelines/{name}`.

### 7c-2. Latest runs (PL-62, T-5818)

```text
GET  /api/v1/projects/{project}/pipeline-runs    the newest run of each pipeline the caller reads (read on Pipeline)
```

The pipelines list reads every row's last run with this one request, whatever the row count. One item per pipeline that ran, by name; a pipeline that never ran is absent. `run` has the fields of `GET .../pipelines/{name}/runs`:

```json
{ "items": [
  { "pipeline": "podujatia-detva",
    "run": { "run": "2026-10-26T06:00:00Z", "firstAt": "2026-10-26T06:00:00Z", "lastAt": "2026-10-26T06:00:04Z", "sent": 64, "rejected": 0, "failed": 0 } }
] }
```

Each pipeline is filtered by read on Pipeline in the project or in the space its target Endpoint writes. A caller who cannot read the project gets `404`; one who reads no pipeline of it gets `403`; a run store that does not answer gives `503`.

### 7d. Source health (PL-62, T-5048)

```text
GET  /api/v1/projects/{project}/pipeline-health    each pipeline's health from its kept runs (approve on Pipeline)
```

One item per pipeline the caller approves, by name:

```json
{ "items": [
  { "pipeline": "podujatia-zvolen", "state": "failing", "lastSuccessAt": "2026-10-18T06:00:00Z",
    "lastRunAt": "2026-10-21T06:00:00Z", "lastSent": 0, "medianSent": 10, "changePercent": -100,
    "failedRuns": 3, "lastError": "GET https://www.zvolen.sk/rss.html answered 404",
    "lastErrorAt": "2026-10-21T06:00:01Z", "alert": true,
    "links": { "checkedAt": "2026-10-19T02:00:00Z", "checked": 20, "answered": 14, "skipped": 1, "deadCount": 6,
      "dead": [ { "url": "https://www.zvolen.sk/podujatia/koncert-v-parku", "status": 404 } ] } }
] }
```

- A run fails when it sent nothing and its log holds failed or rejected lines. `state` is `failing` when the newest run failed or the error ratio is above 5 %, `drift` when it sent 20 % or less of the median of the runs in the 7 days before it, `empty` when it sent nothing and reported nothing, `never` with no run kept, and `ok` otherwise.
- `lastError` is the newest failed or rejected line of the log, masked as the log is (§7c) and with the query and fragment cut from every URL in it.
- `errors` (T-5291) counts the runs that ended within 1 h of the newest one and sent at least one record: `errors` failed and rejected records, `records` every record they took (sent, failed and rejected), `percent` the share with one decimal, e.g. `{ "errors": 13, "records": 120, "percent": 10.8 }`. It is absent when those runs took no record. A run that sent nothing counts toward the failed-run streak, not here.
- `alert` is `true` after 3 failed runs in a row, on a drop, or when `errors` is above 5 % of `records`. The Portal's alert check reads the same health once an hour: 3 failed runs or an error ratio above 5 % open a `failure` incident and a drop a `zero` incident (§37), so a person who follows the pipeline, its space or its type is told once. The same read sets the Portal metric `jc_pipeline_error_ratio{project,pipeline}` (0 to 1), which the monitoring alert `PipelineErrorRatio` reads.
- `links` is the newest weekly link check (T-5133), absent before the first: `checked` links asked, `answered` with 2xx or 3xx, `skipped` not asked, `deadCount` links that failed 2 checks in a row and `dead` the first 5 of them (`url` with `status`, or `error` when the host did not answer), and `checkedAt`. The share `answered / checked` is the T-5081 measure (95 % or more).
- The link check runs on the leader once every 7 days. It reads the `url` and `organizer.url` of each pipeline's entities through a public Endpoint of the space it writes, anonymously, so it holds no right a visitor lacks; a space without a public Endpoint is not checked. An entity belongs to the pipeline whose DataSource host is the host of its `source` (else of its `url`). A link is asked only on an inventoried host, the host of a DataSource of the project; an IP address or a cluster-internal name is never asked. Per host: robots.txt first (RFC 9309; a 5xx or no answer skips the site for that run), then 1 request a second, HEAD first and a GET of the first byte when HEAD answers 405 or 501, 10 s per request, up to 5 redirects on an inventoried host; a redirect elsewhere counts as an answer and is not followed. A 4xx, a 5xx or no answer counts as one failure.
- A caller who approves no pipeline of the project gets `403`; one who may not read the project gets `404`. The Portal's pipelines page shows the answer on its Source health tab, to stewards only, with the dead links and the fix hint "The source moved these pages: update the address in its data source, or ask its publisher."

### 7a. Testing a candidate pipeline (PL-43, MF-38)

```text
POST /api/v1/projects/{project}/pipelines/test
```

```json
{
  "pipeline": { "apiVersion": "joinedcontext.com/v1alpha1", "kind": "Pipeline", "metadata": { "name": "shmu-air-quality" }, "spec": { "class": "resident", "source": { "dataSourceRef": { "kind": "DataSource", "name": "shmu-csv" } }, "compute": { "kind": "bloblang", "bloblang": "root.id = `urn:ngsi-ld:AirQualityObserved:hel.fi:air-quality:%v`.format(this.station_id)" }, "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all" } },
  "sample": { "text": "station_id,pm10,timestamp\n01,18.2,2026-09-13T07:00:00Z\n", "format": "csv" }
}
```

- `sample.text` is at most 5 MiB and never leaves memory; `sample.url` instead makes the runner fetch it under its own egress policy. `format` is `csv`, `json` or `text` (the default, one message as it is).
- The answer is the trace of Architecture/08 §7: `input`, `mapping`, `validation`, `errors`. A manifest the kind refuses is `400` (MF-37); no runner, or a runner that does not answer within three seconds, is `503`; a second test while one runs in the project is `409`.
- A mapping that yields an array is one entity per element in `mapping` and `validation`, at most 20 (PL-48).
- An error of stage `mapping` carries `step`, the index into `spec.steps` of the step it failed at (PL-52); `lint` and `runner` errors have no step and leave it absent.
- `stages` lists every step of `spec.steps` as the test saw it (PL-67): `step` (its index), `reached` (captured messages that reached it whole) and `sample`, what that step made of the first of them, cut at 4 KiB, as data or as text; a step at or after a failure has `reached: 0` and no sample. The Studio shows each node's own sample from it.
- A derived pipeline (`spec.source.endpointRef`, the studio's `kpi` preset, PL-45) is tested on a page of its source endpoint: `sample.url` is that endpoint's `…/ngsi-ld/v1/entities?type=…&attrs=…` URL (the runner fetches it with the pipeline's read grant) or `sample.text` is such a page, `format: json`; the page reaches the mapping as one message, so a fold yields one entity in `mapping` and `validation` checks it as an indicator (PF-43): `calculationFormula`, `derivedFrom` and `computedBy` missing are `problems`, not a `200` that admission then refuses.
- Nothing is written, no `secretRef` is resolved, the stream is deleted whatever happened.

## 8. User Preferences (UI-09, UI-10)

Preferences are the one thing the Portal stores for itself: how a person likes the UI, never how the
platform is configured. They live in the Portal's PostgreSQL preferences tier, keyed by the Keycloak
`sub` of the session (CC-40), and a change takes effect on the answer, with no commit, no merge
request and no approval (UI-10):

```text
GET /api/v1/preferences      the caller's preferences; an empty object before the first save
PUT /api/v1/preferences      replaces them whole; answers 200 with what is now stored
```

```json
{
  "theme": "system",
  "locale": "sk",
  "defaultProject": "air-quality",
  "dashboardLayouts": { "air-quality-overview": { "collapsedLegend": true } },
  "advancedMode": false,
  "firstRunDismissed": false,
  "recent": [{ "path": "/projects/air-quality/pipelines/shmu/edit", "title": "shmu · air-quality" }],
  "favourites": [{ "path": "/projects/air-quality/explore?type=AirQualityObserved", "title": "Explore data · air-quality" }],
  "connectors": [{ "name": "my-air", "select": { "themes": ["ENVI"], "endpoints": ["zt4qm7ge2xdv6ksb3ncf5arw2y"] } }]
}
```

- `theme` is `light`, `dark` or `system`; `locale` an ISO 639-1 code; `defaultProject` a DNS-1123
  name. `dashboardLayouts` is free JSON per dashboard name, capped at 64 KiB in total: it is the
  browser's saved state, the Portal does not interpret it.
- Every field is optional; a `PUT` with a field absent clears it. Unknown fields are a `400`.
- A caller only ever reads or writes their own row: there is no path parameter and no admin view
  over other people's preferences.
- `advancedMode` shows the form fields a `UiSchema` marks `advanced` (CC-29): the commit message,
  the branch, the target revision. Absent means off. It changes what a form shows, never what a
  write may do.
- `firstRunDismissed` hides the five-step first-run checklist on the project home page (create a
  space, connect a data source, run a pipeline, see the data, share it); the help menu shows it
  again by clearing it. Absent means shown. Each step ticks itself from the project's own state,
  never from this record (T-3233).
- Personal MCP connectors are no preference since T-5970: they are connector records (API/14,
  ADR-N-064 §9); a body naming `connectors` is a `400`. A list an older Portal stored moves into
  the connector store once, on that person's first signed-in `GET` or `PUT` of this resource
  (T-6362): the start-up and the gateway know no person's groups, so neither moves it, and a person
  who only calls `/api/mcp/me/{name}` gets an empty list there until they sign in to the Portal
  once. A client token moves nothing (AG-151). Each moved connector is `data-read`, every item
  `read`, expiring after the `maxExpiry` of the person's own DelegationPolicy ceiling, and holds
  only the Endpoints, spaces and projects that person may read today and the ceiling admits,
  checked item by item as `POST /api/v1/connectors` checks them. An old connector that does not
  move (its selector takes no Endpoint the person may read, the ceiling would queue or refuse it,
  its name is live already, the person holds the most live connectors) stays in the stored row
  under `connectorsNotMoved` as `{name, select, reason}`, and a list the Portal cannot read stays
  under `connectors`; no read answers either key and a `PUT` carries both over, so the move loses
  nothing a person made. The gateway reads a person's live `data-read` connectors for
  `/api/mcp/me/{name}` by the caller's
  `sub` alone (EP-123): `GET /internal/connectors/{sub}` on the Portal's internal listener answers
  `{ "connectors": [ { "id": "…", "name": "my-air", "scope": [ … ], "expiresAt": "…",
  "allowedClients": [ … ] } ] }`, an empty list for a `sub` with none, with the same Keycloak
  ServiceAccount check as `/internal/previews` (the gateway's own token, audience
  `portal-internal`; any other token or none is a `401`, and the NetworkPolicy opens the listener
  to the gateway alone). The gateway keeps an answer at most 60 seconds per `sub` and answers
  `503` while the Portal cannot be read, never an older list. `me` is therefore no project a named
  server can be declared in.
- `recent` holds the last ten item pages the person opened, newest first, and `favourites` up to
  fifty pages they starred (UI-90). Each place is a `path` inside the Portal, with its query and
  without `lang`, and the `title` the page had: a path that is not one of this Portal's (`//host`,
  a scheme, a backslash, a control character) or longer than 512 characters, an empty title or one
  over 200 characters, an eleventh recent page or a fifty-first star is a `400`. The UI filters
  both to the projects the caller may read before showing them.
- `POST /api/v1/preferences/recent` with `{ "places": [Place, …] }` (1 to 10, newest first) puts
  those pages at the top of the caller's `recent`, each path once, the newest ten kept, and leaves
  every other field as stored; it answers `200` with what is now saved. The browser sends it when
  its tab is hidden, so opening a page is no write of its own (UI-90).
- `503` with `problem+json` when the Portal runs without a preferences database. The UI then works
  from browser defaults; nothing else depends on this tier (UI-09).

### 8a. Form arrangements (UI-02, CC-29)

```text
GET /api/v1/forms            every UiSchema manifest of portal/forms/, as a List
```

- One unpaged `List` of the organization's `UiSchema` manifests, read from the same mirror as every
  other resource; a form dialog indexes it by `spec.for`. An instance with no manifests answers an
  empty `List`, never `404`.
- The session is the only gate, as for every resource read; there is no path parameter.
- A dialog for a kind with a manifest renders the manifest's order, groups, widgets and help; a
  kind without one renders its built-in form. What a manifest asks for and the form cannot do (a
  field the schema lacks, a widget the Portal does not register, two manifests for one kind) is
  listed in the dialog itself, and the dialog's footer carries the `advancedMode` switch.

## 9. ServiceAccounts (PF-34…PF-40)

A `ServiceAccount` manifest declares that a credential exists; the secret itself never enters Git
(PF-36). The manifest is read and written through the resource API like every other kind
(`/api/v1/projects/{project}/serviceaccounts`). Its credential is an `oauth-client`: the reconciler
provisions the Keycloak client, and a program signs in with `client_credentials` or, inside the
cluster, with its workload identity (PF-47). The Portal mints, stores and accepts no API key
(PF-37; the key routes and the MCP key claim were removed with the keys, owner decision 2026-10-18,
T-4362).

## 10. Export, import, revisions and sync (MF-16…MF-32, CC-49)

The repository at any revision is the complete configuration export (CC-49). These routes hand it
out without Git knowledge, always read from the forge and never from the live mirror, so what is
downloaded is byte-for-byte what a `git archive` of that path would hold:

```text
GET /api/v1/projects/{project}/export?format=yaml|json|zip|git&revision={commit}&kinds={plurals}&names={names}
GET /api/v1/projects/{project}/revisions?limit=20
GET /api/v1/projects/{project}/apps/{name}/export
```

- `format=yaml` (default) is one multi-document YAML stream, one manifest per document; with
  `kinds=endpoints&names=public-air` it is a single manifest file. `format=json` is a `kind: List`
  with the same items. `format=zip` is the archive of `projects/{project}/` at the revision: the
  manifests, the native files next to them (`bento.yaml`, LinkML, committed schema artifacts) and a
  `bundle.yaml` index at the archive root. The index is the platform's own `kind: Bundle`
  (namespace `org`, name the project), so `jcctl validate` accepts what the Portal wrote: `spec`
  carries `exportedAt`, `exportedBy`, `sourceRevision` (the commit the export was taken at, never
  a branch name), `items` (`kind`, `namespace`, `name`, `path` per manifest), `nativeFiles`,
  `files` (`path` and `sha256` of every file the bundle holds, over the bytes as exported, so a
  transfer can be verified before the source is deleted, MF-42) and `omitted`. An export whose filters select no manifest carries no index, because a Bundle lists
  at least one resource. Media types: `application/yaml`, `application/json`, `application/zip`,
  with a `Content-Disposition: attachment` filename that names the project and the short revision.
- A whole-project export (no `names` filter, and every `format=git` export) is for an organization
  administrator (`approve` and `delete` on `RoleBinding` at organization scope, PF-03, UI-87): anybody else who may read the project gets
  `403` naming that, and an export that names its manifests stays with the project's readers.
- A whole-project export is complete and self-describing (MF-41). The archive
  adds, at its root, `README.md` (what each kind it holds is, how many resources, where the schema
  is, the revision and the exporter) and `schemas/`: `schemas/kinds/{Kind}.schema.json`, the JSON
  Schema (draft-07) of every kind in the archive with its field descriptions, and
  `schemas/models/{name}/{name}.linkml.yaml` and `{name}.schema.json` for every `DataModel`, read
  from the repository or generated by Model Tools when the repository has no JSON Schema; a model
  whose files cannot be had is listed under "Missing" in the README. `format=yaml` ends the stream
  with the same `kind: Bundle` index as its last document, whose `spec` carries `readme` (the README
  text) and `schemas` (`kinds`: kind → schema, `models`: name → `{linkml, jsonSchema}`) beside the
  members above; `format=json` carries `readme` and `schemas` beside `items`. A `kinds` filter keeps
  the schemas of the kinds it selects. With `names`, the answer is the manifests alone. Import skips
  `README.md`, `schemas/` and the index, so none of them is written into a project.
- `format=git` exports a project of layout 2, which lives in a repository of its own (MF-45).
  The archive (`application/zip`, `{project}-git-{short}.zip`) holds `{project}.bundle`, the
  forge's `git bundle` of the project repository's default branch with that branch's whole
  history; `{app}.bundle` for every App of the project whose `source.git` names a repository of
  the forge's organization, under the App's name; `{name}.tags` beside a bundle whose repository
  has tags, one `{commit} refs/tags/{tag}` line per tag, because the forge bundles one ref and
  the import sets the tags again; `projects/{project}.yaml`, the registry entry with no
  parameter values, so the target sets its own (CC-88); and the `kind: Bundle` index
  `bundle.yaml`, whose `spec.repositories` lists each bundle with its `role` and the `head`
  commit it ends at and whose `spec.files` carries every file's SHA-256 (MF-42). `project.yaml`
  at the archive root is the project's own file at that head, the one an import reads the
  parameter declarations from without unpacking a bundle. Every organization model a DataModel of
  the project imports (`org.{name}.v{major}`, and what those import in turn) travels as
  `models/{name}.v{major}.yaml`, its manifest, and `models/{name}.v{major}.linkml.yaml`, its
  source, at the version the organization holds; `spec.models` of the index lists each with its
  `name`, `version`, `manifest`, `file`, the `sha256` of the source and its `origin`
  (`{organization, name}`), and nothing else about it travels: no data, no credential (MF-49). An
  import the organization no longer holds at that major is `409`, named. Other branches
  and the annotation of an annotated tag do not travel; `git bundle create --all` of a mirror
  clone (Deployment/11 §7) carries them. A bundle that ends elsewhere than the head read in the
  same export is `409` (export again), and so is an App whose repository is outside the forge's
  organization, named. A git export is the whole repository, so it answers `403` to a caller
  who may not read every manifest of the project (MF-18), where `format=zip` leaves those out;
  it takes no `revision`, `kinds` or `names` (`400`), and a project of layout 1 answers `409`.
- `revision` is a commit sha or branch name; absent means the default branch head. A revision the
  forge does not know is `404`.
- `kinds` and `names` are comma-separated filters on the manifests; the archive format keeps native
  files regardless. Filters that match nothing answer an empty stream, list or archive, not `404`.
- `status` is stripped from every manifest and string values under the secret keys of section 5
  (`password`, `token`, `secret`, `clientSecret`, `apiKey`) are removed, so a bundle is re-importable
  and carries no credential (MF-17). Native files are copied as they are; MF-24 keeps literal
  secrets out of them at write time.
- Grants apply as on the resource API: the bundle holds the kinds the caller may `read` in the
  project (PF-59), a project they may not read is `404`, and `omitted` in the index counts what
  was withheld without naming it (MF-18). Live entities and histories are
  never part of a bundle (MF-19); they are the endpoint's `file.csv` and its APIs.
- `/revisions` is the history of `projects/{project}/` on the default branch, newest first, for the
  revision picker (Architecture/06 section 6): `sha`, `message`, `author`, `date`. `limit` is 1…100,
  default 20.

```json
{
  "items": [
    { "sha": "8c56954a1f0e2b3c4d5e6f708192a3b4c5d6e7f8", "message": "Endpoint public-air: add csv", "author": "Aino Virtanen", "date": "2026-09-06T16:30:00Z" }
  ]
}
```

- `503` with `problem+json` when no forge is configured (CC-03): a Portal that cannot read the
  repository has nothing to export.
- `GET …/apps/{name}/export` exports one application, for an organization administrator only
  (UI-87): anybody else who may read the project gets `403`, and a project or an App the caller
  may not read is `404`. The App must build from a repository of the forge's applications
  organization (`source.git`, AP-72, AP-75); one that builds from anywhere else is `409` naming
  it, because an export without its source would import an App that cannot build. The archive
  (`application/zip`, `{name}-app-{short}.zip`) holds `{name}.bundle`, the forge's `git bundle`
  of the App repository's default branch with its whole history; `{name}.tags` as in
  `format=git`; `app.yaml`, the App manifest as the project's repository holds it at its head,
  stripped as every export strips it (`status`, the built digest, secret values; MF-17, AP-13a);
  and the `kind: Bundle` index `bundle.yaml` (namespace `org`, name the App), whose one item is
  the App with its project as namespace, whose `repositories` list the one `application` bundle
  with the head it ends at, and whose `files` carry every file's SHA-256 (MF-42). A bundle that ends elsewhere than the
  head read in the same export is `409` (export again).

Import is the same bundle read back, into a project that is not the one it left (MF-20…MF-26):

```text
POST /api/v1/projects/{project}/import?dryRun=All
```

- The body is either `multipart/form-data` with the archive or manifest as the `file` part and the
  options beside it as fields, or `application/json` with the options and `manifests` holding one
  manifest, a `kind: List`, or an array of them. Options are the same in both: `targetNamespace`,
  `orgDomain`, `conflictPolicy`, `dryRun`. `?dryRun=All` is the query form of the last one.
  `orgDomain` is the organisation every imported id is rewritten to; absent, it is the domain of
  the instance's own `Organization`. The `{orgDomain}` segment of an id, and of an anchored
  `idPattern`, is rewritten for the spaces the bundle carries, so a moved project stops claiming
  the ids of the city it came from and a federated registration that named another city keeps
  naming it (MF-22, PF-43). The `{space}` segment moves only when the conflict policy renames that
  ContextSpace. An organization-scoped kind is written in namespace `org` whatever project imported
  it, and a `Project` manifest inside the bundle is dropped: the destination project is the one in
  the path.
- An upload is at most the organization's `spec.limits.data.uploadMegabytes` (16 MiB when it sets
  none, never past 64 MiB, the edge's largest body; ADR-N-035), judged on the body as it arrives,
  and at most 2 000 archive entries. A larger one is refused with `400` naming its size, the limit
  and the setting. An entry whose path leaves the archive root is refused before it is read.
- `bundle.yaml` describes the bundle and is never imported as a resource. Its `project` and
  `revision` become `joinedcontext.com/imported-from` on every manifest that lands, so an imported
  object says where it came from (MF-20); an upload with no index is annotated `upload`.
- Every manifest is remapped into the project: the namespace and the typed and string references
  between the manifests (MF-22). Entity URNs in the spec are kept as written: identity is the
  space and the URN (ADR-N-041). `targetNamespace` may only name the project itself, because the repository
  path of every kind starts `projects/{project}/`.
- A Policy's `spec.assigner` lands as `did:web:{orgDomain}`, the placeholder the loader renders
  for the organisation that owns the file (CC-82): a Policy grants over a space of this project,
  so the organisation that may give that data away is this one, whatever DID the bundle carried.
  Every assigner the import rewrote is listed in the report's `reassigned` (`Policy/{name}`, the
  DID the bundle carried, the placeholder it became), so the person who approves the Change sees
  that the grant is now signed by this organisation (R6).
- `conflictPolicy` is `fail` (the default: `409` naming the first collision, and nothing is
  written), `skip`, `replace` or `rename`. A rename is `{name}-{origin}`, the origin being the
  project the manifest came from, then `-2`, `-3` while the name is taken; every reference to the
  renamed resource inside the bundle follows it, so a duplicated project does not import and then
  dangle (MF-26).
- The gates run over the whole bundle before anything is written, and any one of them refuses all
  of it with `400` (MF-24): a kind this platform does not serve, an unsupported `apiVersion`, a
  `status` block (MF-04), a literal secret under one of the secret keys of section 5, or a
  reference that neither the bundle nor the project resolves.
- The answer is `202` and a `Change`: one branch, one merge request for the whole bundle, and the
  lane is the riskiest of the resources in it (CC-63). Native files travel with their manifests and
  keep their path under the new project.
- `dryRun` answers `200` with the report instead: `created`, `replaced`, `skipped`, `renamed`
  (old name → new), `reassigned` (Policy → the assigner it carried, when one was rewritten),
  `nativeFiles`, `lane` and `source`. The `Change` body lists the same reassignments. Nothing is written and no branch is made.
- The dry run is the bundle's check (PF-57, every door): it records a verdict over the bundle as
  sent (its SHA-256) and the options that decide what it writes (`conflictPolicy`, `orgDomain`,
  `targetNamespace`), for the caller and the project. Under `strict` an import without `dryRun`
  needs that verdict, green and fresh. Nothing reaches the forge otherwise, and the answer is the
  gate's own `409` with `"check": "jc_project_import"` (run it with `dryRun`) and `reason`
  `verdict_absent`, or `stale` when the bundle or an option differs from the checked one. An
  import that became a `Change` forgets its verdict; `lax` lets an unchecked bundle through.
- A bundle carrying `spec.files` is verified as it is written (MF-42): the report gains
  `verified` (`path`, `equal` per file), the SHA-256 of what the import wrote compared with the
  index after the namespace mapping is undone, and the `Change` body says "n of m files equal".
  A bundle from an older exporter carries no `spec.files`, and the report then carries no
  `verified`: an unverifiable transfer says so rather than claiming every file is equal.
- Both answers, the dry run's report and the `Change`, carry `needs`: what the bundle cannot carry
  and must be provided where it lands (CC-84). Each item is `{kind, where, why, link}` with `kind`
  one of `secret` (a `secretRef` whose value is set in the target), `person` (a user named in a
  RoleBinding or a Policy), `host` (an Environment host or certificate) and `credential` (a feed's
  authorization); `where` is the manifest path, and `link` the Portal page that sets it. An empty
  list means nothing is to be provided. No item ever holds a secret value, and a caller who may
  not read the target project gets no list. Nothing in it blocks the import; each item keeps the
  resource it belongs to from going Live with a plain reason. Save as across projects and a
  workspace's answers carry the same list. Values the loader renders stay in their placeholder or
  local form on both sides (`{orgDomain}`, `endpointRef`, no `{space}` literal, MF-43).
- `?format=git` imports the archive of a `format=git` export as a new project, the `{project}`
  of the path, in an organization of layout 2 (MF-45, MF-46). The body is `multipart/form-data`
  with the archive as `file`, `parameters` (a JSON object of values for what `project.yaml`
  declares, CC-88; a `secret` parameter takes a `secretRef` name, never a value) and an optional
  `displayName`. An organization administrator imports one (UI-87), within the rules of who may open a project (PF-65), and the name passes the checks of
  `POST /api/v1/projects`. Before anything is created the archive is checked: the index is a
  valid `kind: Bundle` with one `project` repository and no `organization` one, every file's
  SHA-256 equals the index (MF-42), `project.yaml` loads at this release's `apiVersion` (an older
  one is migrated with `jcctl migrate` first and a newer one is refused, MF-47), and the
  parameters resolve against its declarations; each failure is `400` naming it. Every repository
  it would create (`{project}`, and `{project}_{app}` per application, AP-75) must not exist yet
  (`409`). Then each repository is created empty and private, its bundle is pushed as its default
  branch `main` with the tags of `{name}.tags` (the Portal speaks git's receive-pack; it holds no
  git), and its head is read back: a head that is not the index's removes every repository the
  import created and answers `409`. When the slug differs from the one the bundle left, one
  commit on `main` of the project repository remounts it (every manifest that names the old slug
  names the new one; an Endpoint slug another project of this organization serves is drawn
  anew; `CODEOWNERS` names the new writers; an App's `source.git.url` names its new
  repository), so the head after the import is that commit and the answer names both. `main` is
  then protected, and the answer is `202` and the organization's `Change`: the registry entry
  with the given values and the caller's steward binding, as opening a project proposes it. A
  `Change` that cannot be opened removes the repositories again (CC-85). Each model of
  `spec.models` (MF-50) is **mapped** when this organization holds a published model of that name
  and major whose source is byte-identical, and the project's imports stay as they are; otherwise
  it **lands** as a project model, `projects/{project}/datamodels/{name}/{name}.yaml` in the
  project's namespace with the carried `spec.origin` kept, its source beside it and the four
  artifacts Model Tools compiles from that source (DM-02), and every DataModel source of the
  project that imports `org.{name}.v{major}` imports `project.{name}.v{major}` instead, in one
  commit on `main` before it is protected. A landing model is compiled before anything is
  created, so a source Model Tools refuses is `409` from the dry run on. A model that can do
  neither, because the project holds a model of that name already, is `409` naming it, and the
  repositories the import created are removed again (CC-85). The organization repository is never
  written. The report in the
  `Change` body carries `verified`, one `{path, equal}` per bundle with its head. `dryRun` answers
  `200` with the checks alone, the parameters `project.yaml` declares, which the Portal's
  import form is drawn from, and `models`, one `{name, version, action}` per carried model with
  `action` `map` or `land`, which the form shows before the person imports; nothing is created. As for every import, that dry run is the check
  PF-57 holds the import to under `strict`, over the archive's SHA-256, the slug and the
  parameters.
- `?format=app` imports the archive of `GET …/apps/{name}/export` as a new App of `{project}`, a
  project of layout 2, for an organization administrator only (UI-87, `403` otherwise, before
  the body is read). The body is `multipart/form-data` with the archive as `file` and an optional
  `name`, the App's name here (the archive's by default, a DNS-1123 label). Before anything is
  created the archive is checked: the index is a valid `kind: Bundle` whose `repositories` are
  one `application` bundle and nothing else, every file's SHA-256 equals the index (MF-42), the
  bundle ends at the head the index lists (MF-46), and `app.yaml` loads as an `App` at this
  release's `apiVersion` (MF-47); each failure is `400` naming it. An App of that name in the
  project, or a repository `{project}_{name}` in the forge (AP-75), is `409`. Then the
  repository is created empty and private in the applications organization, the bundle is
  pushed as `main` with its tags, its head is read back against the index, and `main` is
  protected; the answer is `202` and the project's `Change`, red lane, that adds the App under
  the new name with `source.git.url` naming the new repository and every other field as
  exported. A failure after the repository exists removes it again (CC-85). `dryRun` answers
  `200` with the repository it would create and the head, and is the check PF-57 holds the
  import to, over the archive's SHA-256, the project and the name.
- The `{"url": …}` source of MF-20 answers `501`. Fetching a host the caller names is an egress
  decision the Portal has no policy behind, and the upload form carries the same bundle; see
  `OPEN-QUESTIONS.md`.

### Sync sources (MF-27…MF-32)

A `SyncSource` is a manifest like any other, written and read through the resource API of section
4. What these four routes address is the loop around it: the run, the switch, and the way out.

```text
GET  /api/v1/projects/{project}/syncsources/{name}/status
POST /api/v1/projects/{project}/syncsources/{name}/sync
POST /api/v1/projects/{project}/syncsources/{name}/pause     {"paused": true|false}
POST /api/v1/projects/{project}/syncsources/{name}/detach
POST /api/v1/webhooks/sync/{project}/{name}                  signed, no session
```

- `/status` is what the project page shows (MF-30): `phase`
  (`Synced | OutOfSync | PendingApproval | Error | Paused`), `observedRevision` — the source
  revision this repository carries — `lastRunAt`, `mergeRequest` for the proposal a run opened and
  nobody has answered, `lastError` when the last run did not finish, `paused`, and `durable`, which
  is `false` on a Portal with no database, where a restart forgets where each source stood.
- `/sync` runs the loop once whatever the schedule says and answers `200` with `proposed` (the
  `kind: Change` envelope of each merge request the run opened, carrying its URL), `unchanged`,
  `flags` — what the run could not do, in sentences — and the `status` that follows it. A paused
  source answers with its status and runs nothing: the switch is not overridden by the button.
- `/pause` records an operator's decision about a running loop. It writes nothing to the repository
  and opens no merge request, because nothing about the configuration changed.
- `/detach` stops syncing for good, and that *is* a change to the repository: it answers `202` with
  the merge request that removes the `SyncSource` manifest. Syncing pauses at once, so a source
  cannot keep proposing while its own removal waits for review, and stays paused if the merge
  request is closed rather than merged. The resources the source brought into the project stay
  where they are — detaching a source is not deleting what it published.
- The webhook route is what a `schedule: { webhook: true }` source runs on. It carries no session
  and is authenticated by an HMAC-SHA256 of the request body in `x-gitea-signature`, against **that
  source's own** secret — `spec.webhook.secretRef`, resolved like every other credential, and
  `spec.webhook.previousSecretRef` while one is being rotated (MF-44). The signature covers the
  body and not the path, so a shared secret would make every path reachable by whoever holds it:
  the origin of one project's source could force a run of another project's. Each source therefore
  carries its own, and the secret a department is given opens their door and nothing else.
- One `401`, with one body, for every refusal at that door: a signature that does not verify, a
  source that is not there, a source with no `spec.webhook`, and a source whose reference this
  instance cannot resolve. The route answers the same to all four on purpose — it is unauthenticated
  until the signature verifies, so a `404` would make it an existence oracle over the sources of
  every project (PF-59, R20), and a `503` would say which instances have no secret backend. The
  source's own `/status` is where an operator reads that its webhook secret does not resolve; the
  value itself never appears in an answer, a log line or the status.
- Every run reaches its origin over `https` and nothing else, follows a redirect only to another
  `https` address, and refuses an origin whose `source.*.secretRef` this instance cannot resolve
  rather than fetching it anonymously (MF-31). A `platformApi` origin is read through the partner's
  own `/revisions` and `/export` of this section and through nothing else (MF-32).

### The mirror itself (MF-04, CC-08, OPS-51)

Three routes are about the Portal's own copy of the repository rather than about any project
(`joinedcontext-portal/src/api/webhook.rs`, `joinedcontext-portal/src/api/health.rs`); a replica
is ready only once its first sync has landed (T-4453 removed `GET /api/v1/sync`, which nothing read):

```text
POST /api/v1/webhooks/gitea    the forge's own hook: signed, no session
GET  /api/v1/health            liveness; public
GET  /api/v1/ready             readiness; public
GET  /api/v1/version           the commits of the binary and of the UI bundle; public
```

- `/webhooks/gitea` is what makes a merged change visible without waiting for the next tick. The
  body is authenticated by an HMAC-SHA256 in `x-gitea-signature` against the configured webhook
  secret, and the previous one while a rotation runs, so a rotation never closes the door
  (T-0982). A push to the default branch or a merged pull request answers `202` and starts a sync;
  any other event answers `204`; a body that is not JSON is `400`, a missing or wrong signature
  `401`, and an instance with no webhook secret configured `503`, because a hook nobody can
  authenticate is refused rather than trusted.
- `/version` answers `{"portal": {"commit", "openapi"}, "ui": {"commit", "openapi"} | null, "contract"}` (T-2485): `openapi` is the sha256 of the OpenAPI document, the binary's own and the `ui/openapi.json` the bundle was built against, and `contract` is `match` when they agree, `mismatch` when they differ and `unknown` when the bundle is unreachable or names no hash. `dev-smoke` warns on anything but `match`.
- `/health` answers `{"name": …, "version": …, "status": "ok"}` whenever the process serves, and
  it is the liveness probe. `/ready` is the readiness probe (OPS-51): `503 {"status": "loading"}`
  until this replica's mirror holds the repository, `200 {"status": "ready"}` afterwards. A Portal
  configured without a forge has no repository to wait for and is ready at once. Both are public,
  because a probe has no session.

## 11. Model Tools preview (DM-10, DM-17, DM-18, DM-19)

The LinkML editor needs a compiled preview on every keystroke, and the compilation runs in Model
Tools: a stateless, versioned image holding `linkml`, `schema-automator`, `pysmartdatamodels` and
the `ngsi_ld_kind` post-processor (DM-18). The Portal calls it for the browser, so the editor never
reaches a third-party host itself and Model Tools needs no ingress:

```text
GET  /api/v1/tools/sdm-catalog   the Smart Data Models catalogue index: subjects, their models
                                 and each model's attribute names (?refresh=true refills the cache,
                                 ?subject=dataModel.X fills that subject's attribute names first)
POST /api/v1/tools/generate      LinkML source  → JSON Schema, @context, SHACL, OWL, example
POST /api/v1/tools/import-sdm    a Smart Data Models model id → the same artifacts as LinkML
POST /api/v1/tools/infer-schema  a sample file (multipart, ≤ 10 MiB: CSV, XLSX, JSON, PDF)
                                 → a draft LinkML model and the editor operations that build it
```

The same draft from an address instead of a file is the operation `jc_model_infer` with `url`
(`POST /api/v1/projects/{project}/ops/jc_model_infer`, `{"url": "https://…", "name": "…"}`): the
address is fetched once on the project's runner, as a DataSource Check fetches it (MF-39), with
`propose` on `DataSource`; a feed that fails answers `422` at `/url` with the runner's reason, a
missing runner `503` (DM-54, T-3250).

The source of a model is a file of the repository, read and saved through one route scoped to it
(DM-56, Architecture/11 §6.8): `GET` answers the text at the manifest's `spec.linkml`, `PUT` with
the new text answers `202` and a `Change` whose commit carries the source, the manifest and the
generated artifacts, in the lane DM-24 assigns. An organization model's source,
`GET /api/v1/projects/org/datamodels/{name}/source`, is every member's to read, like the list
(DM-75).

A name the project does not hold yet is created by the same `PUT` when it names the space the
model belongs to (DM-57): the Change then carries the new manifest as well, and without `space`
the answer is `400`.

```text
GET /api/v1/projects/{project}/datamodels/{name}/source          → 200 text/yaml, the LinkML document
PUT /api/v1/projects/{project}/datamodels/{name}/source           text/yaml body → 202 Change (MF-12)
PUT /api/v1/projects/{project}/datamodels/{name}/source?space=s   text/yaml body → 202 Change, the model created (DM-57)
```

A text whose relationships break a rule of DM-68 creates no Change, whatever the editor said
before sending it: the answer is `400` with one `errors` entry per broken rule, each
`{path}: {rule}: {message}` with the rule identifier of
[Architecture/11 §1.2](../Architecture/11-data-models.md#12-relationships-between-classes-dm-64dm-73) (CC-24).

```json
{
  "type": "https://joinedcontext.com/errors/invalid-request",
  "title": "Invalid Request",
  "status": 400,
  "detail": "the model breaks 2 relationship rules; nothing was saved (DM-68)",
  "errors": [
    "slots.users: inverse-missing: users (School → User) names no inverse",
    "slots.school: required-on-computed-end: school is computed on read and cannot be required"
  ]
}
```

A project shares a published model with the organization through one more route of the model
(DM-77, ADR-N-039 §3.2). The
caller needs `propose` on the model; the answer is a red-lane `Change` of the organization
repository (`chg-org-…`, CC-87) that copies the source byte for byte to `datamodels/{name}/`, with
the manifest in namespace `org`, no `contextSpaceRef`, `spec.origin` naming the project, the space,
the model, its version and the project repository's commit, and the artifacts compiled from that
source. Only a person whose organization-scope binding grants `approve` on `DataModel` approves it,
with the model's name typed (CC-19); a project-scope approver is refused `403` there, and so is a
bearer caller, an agent run or MCP (PF-58). An organization administrator who shares their own
model sends `confirm` with the name and the Change merges at once (PF-58).

```text
POST /api/v1/projects/{project}/datamodels/{name}/share   {"confirm"?: name} → 202 Change
```

| Answer | When |
|---|---|
| `202` | the Change, waiting for an organization approver, or merged when `confirm` came from one |
| `400` | the source imports a model of the project (`project.{name}.v{major}`): share that one first, an organization model imports organization models only |
| `404` | the project holds no model of that name, or the caller may not read it (R20) |
| `409` | the model is not `published`; the organization holds a model of that name from another origin (the answer names it); or the organization's copy from this origin already has this source |

A second share from the same origin proposes the organization copy's next version, by the
severity of its difference (DM-22): a breaking difference is a new major, which projects importing
the old major keep until they move their `import`.

After the share merges, the model's page offers **Use the organization's copy** (DM-78): the same
`PUT …/source` with a text that keeps the model's `id`, `name` and prefixes, imports
`org.{name}.v{major}` and drops every class, slot, enum and type the organization copy defines, so
class names and IRIs stay as they were and no entity id changes.

```json
{
  "refreshedAt": "2026-09-06T04:00:00Z",
  "stale": false,
  "subjects": [
    {
      "name": "dataModel.Environment",
      "title": "Environment",
      "models": [
        {
          "id": "dataModel.Environment/AirQualityObserved",
          "name": "AirQualityObserved",
          "description": "An observation of air quality conditions.",
          "attributes": ["pm10", "pm25", "dateObserved"]
        }
      ]
    }
  ]
}
```

```json
{ "source": "id: https://example.org/aq\nname: aq\nclasses:\n  AirQualityObserved:\n    slots: [temperature]\n" }
```

```json
{
  "linkml": "id: https://github.com/smart-data-models/dataModel.Environment/AirQualityObserved\nname: AirQualityObserved\n",
  "jsonSchema": { "$schema": "http://json-schema.org/draft-07/schema#", "title": "AirQualityObserved" },
  "context": { "@context": { "temperature": "https://example.org/aq/temperature" } },
  "docs": "# AirQualityObserved\n\n| Attribute | NGSI-LD kind | Range |\n",
  "shacl": "@prefix sh: <http://www.w3.org/ns/shacl#> .",
  "owl": "@prefix owl: <http://www.w3.org/2002/07/owl#> .",
  "brokerSchema": { "id": "https://example.org/aq", "name": "aq", "classes": { "AirQualityObserved": { "name": "AirQualityObserved", "slots": ["temperature"] } }, "slots": { "temperature": { "name": "temperature", "range": "string" } } },
  "example": { "id": "urn:ngsi-ld:AirQualityObserved:hel.fi:air-quality:sensor-01", "type": "AirQualityObserved" },
  "generatorVersion": "linkml-1.8.0",
  "errors": []
}
```

The inference answer (DM-54): the model as source and as the operations the visual editor applies, the slots it typed and the ones it could not.

```json
{
  "linkml": "id: https://hel.fi/models/air-quality\nname: air_quality\nclasses:\n  AirQuality:\n    slots: [station_id, pm10, timestamp]\n",
  "operations": [
    { "op": "addClass", "name": "AirQuality" },
    { "op": "addSlot", "name": "station_id", "class": "AirQuality", "range": "string" },
    { "op": "addSlot", "name": "pm10", "class": "AirQuality", "range": "float", "slot_uri": "https://smartdatamodels.org/dataModel.Environment/pm10" },
    { "op": "setSlot", "name": "pm10", "field": "unit", "value": "GQ" },
    { "op": "setSlot", "name": "pm10", "field": "minimum_value", "value": 0 },
    { "op": "addSlot", "name": "timestamp", "class": "AirQuality", "range": "datetime" }
  ],
  "detectedTypes": { "station_id": "string", "pm10": "float", "timestamp": "datetime" },
  "matches": { "pm10": "dataModel.Environment/AirQualityObserved" },
  "untyped": [],
  "rows": 240
}
```

The editor's operations (DM-13) are `addClass`, `removeClass`, `renameClass`, `setClass`, `addSlot`, `removeSlot`, `renameSlot`, `attachSlot`, `detachSlot`, `setSlot`, `setTitle`, `addEnum` and `addEnumValue`, applied in order and all or nothing. A rename carries `to` and follows the element through the model: a renamed class is renamed in every `is_a` and every slot `range` that names it, a renamed slot in every class's slot list. An operation naming an element the model does not have, or a new name it already has, is refused with its index and the reason.

- `sdm-catalog` and `import-sdm` answer from the Smart Data Models mirror when the image holds one (`JC_PORTAL_SDM_MIRROR_DIR`): the catalogue names each model's upstream `commit` (DM-08) and lists in `failed` the models that did not convert, each with its `finding`. A failed model is never offered: `import-sdm` refuses it with that finding. A model the mirror lacks, or a stored file that no longer matches the catalogue's pin, goes to Model Tools as before.
- `sdm-catalog` is the index the import wizard browses (DM-07): the subjects and their models, with
  the attribute names and descriptions the wizard searches. Model Tools caches it and refreshes it
  daily; `?refresh=true` asks for a refresh now, and an index Model Tools could not refresh is
  answered from the cache with `stale: true` rather than withheld (DM-12). The index carries no
  URLs: a model is named by its catalogue identifier, and fetching it is `import-sdm`.
- `refresh` and `subject` are the two parameters the route takes, and the only things a caller may
  steer: which catalogue is fetched is Model Tools' own allowlist (DM-10), never the request
  (`joinedcontext-portal/src/tools/model_tools.rs`). `subject` (comma-separated, at most 8) names
  subjects of the index whose model attribute names Model Tools fills before it answers, once per
  replica; a name the index does not list fetches nothing. The CSV upload (T-3840) fills the city
  subjects it ranks a file against this way. The index is passed through as Model Tools wrote it,
  so `attributes` is there for every model whose schema that replica read and `[]` for the rest;
  the wizard narrows by subject and searches by attribute in the browser, over the one index it
  already holds.
- `linkml` is the LinkML source itself. `import-sdm` answers with it, because an import produces
  the document the editor then edits, and its `annotations` carry `spec.source.repository`,
  `spec.source.path` and `spec.source.commit` so the import is reproducible and its provenance
  reaches the manifest (DM-08). `generate` is given a source and does not echo one back.
- A source that imports platform models (`org.{name}.v{major}`, `project.{name}.v{major}`, DM-76)
  is compiled with them: the Portal reads each one it may resolve, an organization model or a model
  of the route's own project at the pinned major, and hands Model Tools its source. A caller never
  sends another model's source, and an import the Portal cannot resolve is an `errors` entry naming
  it.
- `import-sdm` takes a model **identifier** (`dataModel.Environment/AirQualityObserved`), never a
  URL. The allowlist that limits fetching to the Smart Data Models organisation lives in Model
  Tools (DM-10); the Portal refuses `400` for anything that is not an identifier so no caller can
  steer the fetch at all.
- Every artifact field is optional and absent when this Model Tools version does not render it. A
  compilation that fails is `200` with the messages in `errors[]`: a half-written model is the
  normal state of an editor, not a server error.
- `jsonSchema`, `context`, `docs` and `example` are the four artifacts DM-02 commits beside the
  source, and `jcctl model generate` writes exactly these (see [API/03 §4](03-jcctl.md)); `shacl`
  and `owl` are rendered in the same run for the artifact store (DM-44), and so is `qb`, the RDF
  Data Cube in Turtle, present only when the model declares a Data Structure Definition (DM-60).
  `brokerSchema` is the model with its imports merged, in the LinkML subset the broker reads at
  `PUT /q/schemas/{tenant}` (Antares `linkml-model`): every class lists all its slots, every slot
  is top-level, a class's narrowing of a slot is its `slot_usage`, a range is a class, an enum or a
  built-in type. The Portal's reconciler writes it to the tenant of every Context Space whose
  `dataModelRef` names the model ([Architecture 11 §6.5](../Architecture/11-data-models.md)). `docs` is one Markdown
  page for the whole model, not a directory, and `example` is one entity in the key-value form the
  JSON Schema describes, validated against that schema and against the `@context` before it is
  answered (DM-21). An import answers with the catalogue's own example instead of a generated one:
  real data beats a value derived from a range.
- `generatorVersion` is the version that produced the artifacts. CI invokes the same image version
  as the preview (DM-19), so a preview and a committed artifact set can be compared.
- A source over 512 KiB is `413`, and a sample over 10 MiB the same. The routes are session routes
  and carry the CSRF token like every other mutating Portal call.
- `503` with `problem+json` when no Model Tools URL is configured or the container does not answer
  within the compile timeout. The editor then shows the source without a preview, not an error.

## 12. Static apps host (AP-12, AP-14, AP-17)

A `static` app is served by the Portal on its own host, `https://{name}.apps.{domain}/`
(AP-14, AP-133). The edge rewrites a request on that host to these paths of the static host,
which nothing else routes to:

```text
GET  /apps/{name}/                      the app's index.html
GET  /apps/{name}/{path}                any asset of the built bundle
POST /apps/{name}/api/functions/{fn}    one function of the served build, run in jc-functions (AP-84)
```

The app's data calls go to `/api/endpoint/{slug}/…` on its own host. The Portal does not serve
them: the edge route `app-{name}-endpoint` sets the App's session as the bearer and proxies them
to the Context Gateway, for the App's own endpoints only (Deployment/10, AP-133).

- Only an app whose manifest is `lifecycle: published` is reachable. A draft, a preview or a
  retired app is `404` — the same answer as a name that does not exist, so the host never
  discloses which apps are being worked on.
- Every response carries the app's own Content Security Policy, built from `spec.csp`:
  `default-src 'self'; connect-src 'self'; frame-src 'self'; frame-ancestors {portal origin}`
  by default, with `connect-src` extended by `spec.csp.connectSrc`, `frame-src` by the https
  origins of `spec.csp.frameSrc` (an App's own map or video frame, T-2871), `connect-src` and `img-src` by the project's
  basemap route prefix when the platform configures a basemap (its style URL is `basemap` in
  `#jc-config`, AP-67), and the origins of `spec.csp.frameAncestors`
  added to `frame-ancestors` only when `spec.embeddable: true` (AP-12). The Portal's own origin
  is always there, since "Open app" frames the App under the Portal's header (AP-122), unless
  Apps have no origin of their own (`JC_PORTAL_APPS_URL` unset): an App on the Portal's origin
  would reach into the Portal's page, so it keeps `frame-ancestors 'none'` (`'self'` when
  embeddable). It
  replaces the Portal's own CSP for these paths, and no `X-Frame-Options` is sent: its
  `SAMEORIGIN` would refuse the Portal, whose host is not the apps origin.
- The bundle is served from the app artifact root, one directory per app, and each directory
  carries an `integrity.json` written by the build lane: a map of bundle-relative path to the
  `sha384-…` Subresource Integrity digest of that file. The host verifies the digest of every
  file it serves and refuses `502` on a mismatch, so a modified artifact is never served under a
  published app's name. An app directory without `integrity.json` serves nothing (`404`): the
  integrity manifest is what makes the bundle publishable, not an optional extra.
- Paths are resolved inside the app's own directory. A traversal attempt (`..`, an absolute path,
  a symlink leaving the root) is `404`, never a read outside the root.
- Unknown paths inside a published app answer `404` rather than the app's `index.html`: a static
  app that wants client-side routing declares it in its build, and the host does not invent a
  fallback that would mask a missing asset.
- `POST /apps/{name}/api/functions/{fn}` runs the function `fn` of the served build's
  `functions.js`, integrity-checked like every other file, in `jc-functions`, with the caller's
  `X-Access-Token` as the one credential its data calls carry; an anonymous caller of a `public`
  app sends none (AP-84, SDK-23). The answer is the function's own status and JSON body, `500`
  with `{error: {message, file, line}}` when it throws. A name outside `[a-z][a-z0-9-]{0,39}`, an
  app that is not published or not the caller's to read, and a build with no such function are
  `404`; a body that is not JSON is `400`, one over 256 KiB `413`, a full runtime `429` with
  `Retry-After`. A call that carries the edge's token also carries `X-CSRF-Token` matching the
  `__Host-jc_csrf` cookie, or it is `403`: the edge sets the token from a cookie a cross-site form
  would send too. The host sets that cookie on the apps origin with a signed-in person's first
  index, and a copy of the same token as `jc_csrf` for an App built on an SDK before T-4395; only
  `__Host-jc_csrf` is compared.

### 12a. The build of an application (AP-100, AP-103, ADR-N-028)

A `static` App whose source is its own repository on the forge (`spec.source.git`, AP-75) is
built there, by the repository's `.gitea/workflows/build.yml`. The App page reads where that
build is and asks for another one through two routes:

```text
GET  /api/v1/projects/{project}/apps/{name}/build      the repository, the latest run, the package
POST /api/v1/projects/{project}/apps/{name}/rebuild    dispatches build.yml on the default branch
GET  /api/v1/projects/{project}/apps/{name}/builds     the successful builds, newest first
POST /api/v1/projects/{project}/apps/{name}/restore    opens a merge request back to one build
```

`GET …/build` answers `200` for a person who may read the App:

```json
{
  "repositoryUrl": "https://forge.example/user/oauth2/keycloak?redirect_to=%2Fjoinedcontext-apps%2Fhelsinki_city-bikes",
  "configurationUrl": "https://forge.example/user/oauth2/keycloak?redirect_to=%2Fjoinedcontext%2Fhelsinki",
  "run": { "status": "completed", "conclusion": "success", "commit": "3f1c…", "url": "https://forge.example/user/oauth2/keycloak?redirect_to=…",
           "number": 8, "startedAt": "2026-10-07T10:00:00Z", "completedAt": "2026-10-07T10:03:05Z" },
  "typicalSeconds": 185,
  "packageUrl": "https://forge.example/user/oauth2/keycloak?redirect_to=%2Fjoinedcontext%2F-%2Fpackages%2Fgeneric%2Fapp-city-bikes%2F3f1c…",
  "rebuild": { "allowed": false, "reason": "Rebuild needs propose on App in project helsinki" }
}
```

- Every link carries the forge's sign-in, so a person without a forge session is offered the
  Keycloak button and lands on the page (PF-79, PF-81).
- The repository is `{project}_{app}` of the applications' organization (`{org}-apps`, PF-106;
  the configuration's organization where the installation keeps none), derived from the names
  in the path and never read from the manifest's `url` (AP-75).
- `configurationUrl` is the project's configuration repository, the one its Changes merge into:
  its own repository in layout 2, the organization repository otherwise (CC-87). It is `null`
  for a caller the forge does not let read that repository: in layout 2 a person PF-87 does not
  place in the project's readers, in layout 1 a person without a binding at the organization;
  an organization administrator always has it (AP-103, T-3039).
- `repositoryUrl` is there for every caller who may read the App: the applications'
  organization reads every App's repository to every signed-in person (PF-79, T-3030).
- `run` is the newest run of the repository's workflows, or `null` before the first one;
  `packageUrl` names the package of `status.build.commit`, or is `null` while the App has no build.
- `run.number` is the run's number in its repository, and `startedAt`/`completedAt` are present
  once a runner took it and once it completed. A client that pressed Rebuild tells its run from
  the one before by the number (T-3245).
- `typicalSeconds` is how long the newest finished successful run of the last five took: the
  estimate a running build is shown against. It is absent before the first success, so there
  is no estimate rather than a guess (T-3245).
- An App without `spec.source.git` has no build here: `repositoryUrl`, `run` and `packageUrl`
  are `null` (`configurationUrl` still names the configuration repository), and `rebuild.reason` says the App is not built on the forge.
- `rebuild.allowed` is `true` for a person holding `propose` on `App` in the project; otherwise
  `reason` says what is missing (PF-50, UI-44).
- `404` for an App the caller may not read, the same answer as a name that does not exist
  (PF-59); `503` when no forge is configured.

`POST …/rebuild` takes no body and answers `202` once the forge accepted the dispatch of
`build.yml` on the repository's default branch; the run then appears in `GET …/build`. It is
refused `403` without `propose` on `App`, `404` as above, `409` for an App that is not built on
the forge, and `503` with the forge's reason when there is no forge or it refuses the dispatch.

`GET …/builds` answers `200` for a person who may read the App with the successful runs of the
repository's workflows, one per commit, newest first, and whether Restore is offered (AP-171):

```json
{
  "builds": [
    { "commit": "3f1c…", "number": 8, "completedAt": "2026-10-07T10:03:05Z", "current": true },
    { "commit": "9a2b…", "number": 6, "completedAt": "2026-10-05T08:12:40Z", "current": false }
  ],
  "restore": { "allowed": true },
  "dataNote": "Restore brings back the App's source only; its schema, files and data stay as they are."
}
```

`current` marks the build `status.build.commit` names. An App that is not built on the forge
answers an empty `builds` and `restore.reason` saying so; `404` and `503` as for `…/build`.

`POST …/restore` takes `{ "commit": "9a2b…" }` and answers `201` with
`{ "branch": "restore/9a2b3c4-1759831960", "pullRequestUrl": "…" }` once the forge opened the
merge request into the default branch; the build lane builds the restored source after it is
merged, and only the lane writes `status.build` (AP-73). It is refused `403` without `propose` on
`App`, `404` as above, `409` for an App not built on the forge, for a commit that is not one of
the listed builds, for the current build, for a build whose files equal the default branch, and
for a build holding a file that is not UTF-8 text (the forge's change API takes text), naming
the file; `503` when the forge refuses.

### 12b. Renaming the shapes of a project's Apps (AP-124)

For one release `static` and `fullstack` are read as `ui` and `ui-rust`. The Apps page of a
project that still holds one shows a notice with **Propose the rename**; the button calls

```text
POST /api/v1/projects/{project}/apps/rename-shapes
```

with no body. The Portal collects every App of the project whose `spec.kind` is an old name,
writes each with the new one and nothing else changed, and proposes them as one Change in the
caller's name, through the import door with `conflictPolicy: replace`: the same check, the same
refusals per manifest (propose on `App`, AP-14a, the kind's invariants) and the lane of the
riskiest file. The merge request is titled `rename the App shapes of {project} (AP-124)`.

- `202` with the Change, as an import answers.
- `403` when the caller may not propose an App in the project, before anything is read.
- `400` naming the App when a rewritten App does not pass the App door.
- `409` when no App of the project carries an old name: there is nothing to propose.
- An App already named in an open Change is proposed again in this one; the reviewer sees both.

### 12a. The App templates (AP-141)

```text
GET /api/v1/app-templates      every template, by name, for any signed-in person
```

```json
{ "templates": [{
  "name": "kpi-dashboard",
  "title": "KPI dashboard",
  "purpose": "Show managers how the organization's indicators stand…",
  "audience": "managers",
  "access": "Read-only; the indicators may be public.",
  "dataNeeds": [{ "types": ["KeyPerformanceIndicator"], "attrs": ["kpiValue", "target"], "operations": ["queryEntity"] }]
}],
  "modelAvailable": true }
```

`modelAvailable` is `false` when the Portal has no agent runner or the agent proxy last reported
the model key refused or out of credit (AG-96); the gallery then says why "Create from this" is
unavailable and offers "Use as is" below.

```text
GET /api/v1/app-templates/{name}/screenshot/{width}   a PNG of the template at 1440 or 375 px; 404 otherwise
GET /templates/{name}/                                its live demo on its fixtures, outside /api/v1
```

The demo is one bundle of every sample (`sdk/vite.demos.config.ts`), served with its own
Content Security Policy: `connect-src 'none'`, `form-action 'none'`, `frame-ancestors 'self'`.
The templates are the samples of SDK-24, compiled into the Portal. A run started with a prompt
that contains `(template: {name})` adapts that template, whatever else the prompt says; a name no
template has is ignored and the prompt's words choose, as before.

```text
POST /api/v1/projects/{project}/app-templates/{name}/apps   use a template as it ships (T-5617)
```

```json
{ "name": "bike-finder", "endpointName": "bikes" }
```

No model is asked. The Portal creates the repository `{project}_{name}` of the App (AP-75), commits
the SDK template with the sample's `src/` over it, the build workflow and a README to `main` in
the caller's name, protects `main`, and proposes the App building from that commit: class `ui`,
visibility `project`, the template's `dataNeeds` in the Endpoint's context space. The proposal
runs as a dry run before the forge is touched, and a failure after the repository exists deletes
it again (CC-85).

- `202` with the Change.
- `403` when the caller may not propose an App in the project, before the body is read.
- `404` for a template the gallery does not hold.
- `422` with `errors` entries prefixed by the field: `name:` for a name that is not a DNS-1123
  label, too long for its repository or already an App; `endpointName:` for an Endpoint that is
  not there or that the caller may not read (one answer for both), or one that hides an attribute
  the template reads.
- `409` when a repository of that name is already in the forge; it is never adopted.

## 13. Flows: running a blueprint (CC-24, CC-30, CC-31, CC-32, CC-59)

A flow is one instantiation of a Blueprint. The gallery lists the blueprints an organisation
publishes and the wizard turns the chosen one's parameter schema into a form; saving the form is
what produces the repository change, so the user never sees YAML or a merge request (CC-29).

```text
GET  /api/v1/blueprints                          the gallery (organization-level kind, section 4)
POST /api/v1/projects/{project}/flows            parameters in → 202 + Change
```

The gallery list is the ordinary organization-level collection, so it carries no new response
shape. What it does carry is a filter: the Portal returns only the blueprints whose
`spec.allowedRoles` intersect the caller's roles (CC-59). A blueprint a role may not run is
absent, not greyed out, and the same check runs again on `POST /flows`, because a list that
merely hides a card is not an authorisation.

```json
{
  "blueprint": "threshold-alert",
  "version": "1.2.0",
  "parameters": { "entityType": "AirQualityObserved", "thresholdValue": 50 }
}
```

- `blueprint` names an organization-level Blueprint; `version` is the version the form was
  generated from. A mismatch with the blueprint's current `spec.version` is `409`: the parameters
  were filled against a schema that has since changed, and silently expanding them against the new
  one is how a form produces a manifest nobody reviewed (CC-26).
- `parameters` is validated against `spec.parameterSchema` before anything is rendered. A
  violation is `400` with every failure listed at once in `problem.errors[]`, each naming its
  instance path, so the form can mark all its fields in one pass rather than one per round trip
  (CC-24). The wizard validates the same schema in the browser; the server check is the one that
  counts.
- On success the response is the usual `202` with a `Change` (section 4). All the manifests of one
  flow go into a single merge request, so they are reviewed and merged together or not at all
  (CC-32).
- The lane is the stricter of what the blueprint's `spec.riskClass` declares and what the rendered
  kinds are on their own terms (CC-59, CC-63). A Green blueprint whose template renders a `Policy`
  therefore lands in the Red lane; declaring a lane can narrow nothing that section 4 already
  calls Red.
- A flow that is Green after that rule is merged in the same call, for the person who started it
  (AG-14, CC-63, CC-65): the Portal merges it pinned to the commit it wrote, with the merge
  message naming the blueprint, its version and that person, and answers the `Change` as
  `Deploying`. It merges only while the merge request holds exactly the files the flow wrote; a
  merge request with any other file, one whose files cannot be read, or one the forge will not
  merge answers `PendingApproval` and waits for a person. A Yellow or Red flow always waits. Over
  MCP, `jc_flow_start` runs in the flow's own lane, so a Green flow is not asked about (AG-63)
  and a Red one is asked about as Red.
- Every rendered manifest passes the same checks a hand-written one does. A template that renders
  a namespace other than the project the flow runs in is `400`, and so is a literal secret
  (MF-04, MF-24): a blueprint is authored once and run in many projects, so a namespace in a
  template is a way out of the project the caller chose.
- Expansion is `jcctl`'s, the same code the reconciler runs, never a second engine in the Portal
  (CC-25). Every rendered manifest carries `joinedcontext.com/blueprint`,
  `joinedcontext.com/blueprint-version` and `joinedcontext.com/blueprint-parameters`, which is
  what lets the Portal re-open a flow as the form that created it (CC-27, CC-32).
- `503` with `problem+json` when the Git forge is not configured. Everything the Portal can check
  on its own has passed by then, so a deployment without a forge fails loudly rather than
  reporting a change it never opened.

## 14. Activity (UI-31, OPS-48, OPS-49)

What is happening in a project, as one paged list and one live tail of the same query. The shape
of an event and where it comes from are
[Architecture/09 §6](../Architecture/09-portal.md#6-activity-what-is-happening).

```text
GET /api/v1/projects/{project}/activity            the list, newest first, paged
GET /api/v1/projects/{project}/activity/stream     the same filter, as Server-Sent Events
```

Both take the same query parameters, so a view switches between them without rewriting anything.
Both answer readers of the project (`404` otherwise). An event of a Context Space reaches only a
reader whose bindings reach that space (a project or organization binding, or one scoped to that
space); the stream checks each event against the bindings of that moment (T-4876).

| Parameter | Meaning |
|---|---|
| `space` | one Context Space |
| `kind` | one or more event kinds, repeated or comma-separated |
| `source` | `reconciler`, `pipeline`, `gateway`, `broker`, `ckan`, `portal` |
| `severity` | `info`, `warning`, `error`; a value includes everything above it |
| `since` | RFC 3339 instant, the oldest event to return |
| `object` | one object the events belong to, as `{plural}/{name}`, for an object page |
| `limit`, `cursor` | page size and the opaque cursor of the previous answer |

```json
{
  "apiVersion": "joinedcontext.com/v1alpha1",
  "kind": "List",
  "items": [
    {
      "time": "2026-09-06T16:21:03Z",
      "project": "helsinki",
      "space": "air-quality",
      "kind": "access.denied",
      "source": "gateway",
      "summary": "An anonymous caller was refused a write to AirQualityObserved.",
      "severity": "warning",
      "correlationId": "4bf92f3577b34da6a3ce929d0e0e4736",
      "details": { "endpoint": "public-air", "operation": "updateAttrs", "rule": "public-read" }
    }
  ],
  "next": "b3RoZXItcGFnZQ"
}
```

The stream sends the same objects as `data:` lines of named events, one per activity kind, so a
browser subscribes to the kinds it draws:

```text
event: access.denied
data: {"time":"2026-09-06T16:21:03Z","project":"helsinki", … }

: keep-alive
```

- A viewer of the project reads it and nobody else; there is no cross-project activity route.
- `summary` is localised to the request's `Accept-Language`, the rest of the record is not: an
  event kind and an endpoint slug are identifiers, not prose.
- `details` never carries a request body, a header, a token or an entity attribute value
  (OPS-48). A field that would have carried one is absent, not empty.
- Events live seven days and their per-minute counters ninety (OPS-49), so `since` older than the
  retention window returns what is left rather than an error.
- The stream sends a comment line as a keep-alive every twenty seconds and closes on the
  Portal's shutdown; a client reconnects with `since` set to the last event it saw, which is why
  `time` is part of every record.
- `404` with `problem+json` when the project does not exist or the caller is not a member of it,
  never `403`: an activity route that distinguishes the two says which projects exist (R20).

### Ingest, for the collector only

```text
POST /api/v1/activity        OTLP log records, application/json
```

The OpenTelemetry Collector ([Deployment/05 §5](../Deployment/05-monitoring-logging.md#5-the-activity-pipeline))
posts here, one batch a request, as an OTLP `ExportLogsServiceRequest` whose every log record
carries the event's fields as its attributes. The route sits outside the project API on purpose:
a record names its own project in its attributes, and the collector is a member of no project and
must not be given one.

- The caller is a Keycloak ServiceAccount client whose token is bound to this audience; a human
  token is refused. A caller that is neither gets `401`, a caller with the wrong audience `403`.
- No APISIX route publishes it, so it is reachable only from inside the cluster (OPS-48).
- A record whose `project` is unknown, whose `kind` is outside the vocabulary of
  [Architecture/09 §6](../Architecture/09-portal.md#6-activity-what-is-happening), or whose
  attributes fail validation is rejected on its own. The batch still succeeds: one malformed
  record must not cost the other four hundred.
- The answer is the OTLP `ExportLogsServiceResponse`, so the collector reads rejections with the
  code it already has rather than a shape invented here:

  ```json
  { "partialSuccess": { "rejectedLogRecords": 2, "errorMessage": "kind not in the vocabulary" } }
  ```

- The Portal validates what it is handed. The collector strips attributes, which is a redaction
  step and not a trust boundary; the boundary is this route.

## 15. Branding (UI-30, OPS-46)

One image serves every installation. The deployment renders `global.branding`
([Deployment/12](../Deployment/12-branding-and-naming.md)) into a ConfigMap, mounts it and names
it in `JC_BRANDING_FILE`; the Portal reads that file and answers:

```text
GET /api/v1/branding             the block, public and cacheable
GET /api/v1/branding/logo        the logo file, from the same mount
GET /api/v1/branding/favicon     the favicon file, from the same mount
```

```json
{
  "instanceName": "Banská Bystrica Context",
  "shortName": "BB Context",
  "organisation": "Mesto Banská Bystrica",
  "contactEmail": "opendata@example.org",
  "logo": "logo.svg",
  "colours": { "primary": "#0000bf", "secondary": "#0072c6", "accent": "#ffe977", "background": "#ffffff", "text": "#1a1a1a" },
  "fonts": { "heading": "HelsinkiGrotesk, system-ui, sans-serif", "body": "system-ui, sans-serif" },
  "languages": { "default": "sk", "offered": ["sk", "en"] },
  "primaryForeground": "#ffffff",
  "primaryDark": "#3666d7",
  "primaryForegroundDark": "#ffffff"
}
```

- All three routes are unauthenticated on purpose: the login page needs the name and the logo
  before anyone has signed in, and the block holds no secret. They answer `Cache-Control: public,
  max-age=300`, the only API answers a browser may keep.
- Every colour is validated as a hex triplet or sextet; anything else is replaced by the neutral
  default before it is served, because the UI writes these values into CSS custom properties
  (OPS-46). `primaryForeground` is computed from the primary colour rather than authored, so text
  on a light brand colour stays readable.
- `primaryDark` and `primaryForegroundDark` are the same pair for the dark theme, and are computed
  too. A dark page cannot paint a button in the brand colour as it was configured — a navy button
  on a navy page is not a button — so the theme lightens it, and the readable text is then the
  readable text on *that* colour, not on the configured one. Both are computed in one place, here,
  because the choice needs the contrast ratio of the lightened colour: the UI wrote a near-black
  label on it by rule instead, which left an installation branded `#111827` at 2.31:1 and one
  branded `#0000bf` at 3.43:1 (T-2324, UI-30). A value for either in the file is overwritten.
- `hiddenSections` lists the project sections this installation hides, always present and empty
  when none is: `["dashboards"]` unless `JC_PORTAL_DASHBOARDS` is `true` (T-2874). The Portal
  leaves a hidden section out of the project menu, sends its addresses to the project's spaces and
  does not offer its assistant path, which `POST …/assistant/conversations` refuses with `400`;
  the kind, its manifests and its API stay. A value in the file is overwritten.
- The logo and the favicon are file names, never URLs. A value carrying a scheme, a host or `..`
  is dropped, and the file is read from the branding file's own directory: the two assets the
  ConfigMap carries are the only files those routes can reach.
- A branding file that is missing, unreadable or not valid YAML is not an error: the Portal logs
  the reason and answers the neutral defaults, so an installation whose ConfigMap has not been
  rendered looks plain rather than failing to load. The two asset routes have nothing to fall back
  to and answer `404` with `problem+json` when the block names no logo or favicon, or the file
  behind the name is not readable (`joinedcontext-portal/src/api/branding.rs`); a page renders its
  own mark then, and never a broken image.

## 16. Open-data catalogue status (EP-62…EP-67)

A `CkanInstance` is a manifest, so a catalogue is configured through the resource API like
everything else (`POST /api/v1/projects/{project}/ckaninstances`), which makes it a change
proposal a steward approves rather than a setting somebody flips. What needs a route of its own is
the picture across both kinds:

```text
GET /api/v1/projects/{project}/ckan/status    catalogues, and what each endpoint publishes
```

```json
{
  "instances": [
    {
      "name": "open-data",
      "url": "https://data.banskabystrica.sk",
      "organizationDefault": "mesto-banska-bystrica",
      "apiTokenRef": "ckan-open-data"
    }
  ],
  "publications": [
    {
      "endpoint": "ovzdusie-public",
      "phase": "Live",
      "instance": "open-data",
      "instanceMissing": false,
      "organization": "mesto-banska-bystrica",
      "dataset": "kvalita-ovzdusia",
      "datasetUrl": "https://data.banskabystrica.sk/dataset/kvalita-ovzdusia",
      "resources": [
        { "name": "NGSI-LD API", "url": "https://api.{domain}/api/endpoint/{endpointSlug}/ngsi-ld/v1/", "format": "NGSI-LD" },
        { "name": "CSV", "url": "https://api.{domain}/api/endpoint/{endpointSlug}/file.csv", "format": "CSV" }
      ],
      "datastore": { "representation": "csv", "refresh": "onChange" }
    }
  ]
}
```

- The dataset shown is the dataset the reconciler would write: both are rendered by the same code,
  so the monitor cannot drift from the publisher (EP-63, EP-64).
- `apiTokenRef` is the name of the secret, never its value. No CKAN call is made to build this
  answer and no credential is read (EP-67).
- An endpoint whose `instanceRef` names a catalogue this project does not have is reported with
  `instanceMissing: true` and no dataset URL, because a dangling reference is the misconfiguration
  a steward has to see.
- The route needs a session, like every other project route.

## 16a. The catalogue (EP-81, EP-82)

The platform-wide catalogue page of [Architecture/21 §6](../Architecture/21-open-data-catalogue.md#6-the-portals-catalogue-page)
reads these routes. They need no session and answer `Cache-Control: public, max-age=60`: what
they carry is what the public catalogue already shows.

```text
GET /api/v1/catalogue                          search, facets and one page of datasets
GET /api/v1/catalogue/datasets/{name}          one dataset
GET /api/v1/catalogue/datasets/{name}/sample   up to ten rows, read through its Endpoint
GET /api/v1/catalogue/datasets/{name}/map      its located entities, for the dataset page's map
GET /api/v1/catalogue/endpoints/{slug}/standards  a public Endpoint's standards, fields and examples
```

`GET /api/v1/catalogue` takes `q` (full text, passed to CKAN), the facet filters `publisher`,
`theme`, `format`, `licence`, `spatial` and `year` (each repeatable; values of one filter are
alternatives, filters combine with AND) and `page` (from 1, twenty datasets a page):

```json
{
  "total": 1,
  "page": 1,
  "pageSize": 20,
  "datasets": [
    {
      "name": "bbsk-kpi",
      "title": "Ukazovatele kraja",
      "notes": "Ukazovatele Banskobystrického samosprávneho kraja…",
      "publisher": { "name": "bbsk", "title": "Banskobystrický samosprávny kraj" },
      "licence": { "id": "cc-by", "title": "Creative Commons Attribution" },
      "formats": ["CSV", "JSON", "NGSI-LD"],
      "themes": ["ECON"],
      "modified": "2026-09-21T05:52:34Z"
    }
  ],
  "facets": {
    "publisher": [{ "value": "bbsk", "label": "Banskobystrický samosprávny kraj", "count": 1 }],
    "theme": [{ "value": "ECON", "label": "Economy and finance", "count": 1 }],
    "format": [{ "value": "CSV", "label": "CSV", "count": 1 }],
    "licence": [{ "value": "cc-by", "label": "Creative Commons Attribution", "count": 1 }],
    "spatial": [],
    "year": []
  },
  "unavailable": []
}
```

- The Portal calls `package_search` on the distinct `spec.url` of every `CkanInstance` of the
  installation, with no API token, and drops any dataset marked `private`: an anonymous CKAN
  caller sees public datasets only, so the answer cannot list a restricted one (EP-67, EP-69).
- Facet counts are over the datasets that match `q` and every other facet's filter.
- A catalogue that does not answer is named in `unavailable` and the others still answer; when
  none answers, the route answers `503` with `problem+json`, as every Portal route whose upstream is down does. No `CkanInstance` at all is an empty
  catalogue, `200` with `total: 0`.

`GET /api/v1/catalogue/datasets/{name}` answers the dataset page:

```json
{
  "name": "bbsk-kpi",
  "title": "Ukazovatele kraja",
  "notes": "…",
  "keywords": ["ukazovatele", "kraj"],
  "publisher": { "name": "bbsk", "title": "Banskobystrický samosprávny kraj" },
  "licence": { "id": "cc-by", "title": "Creative Commons Attribution", "url": "https://creativecommons.org/licenses/by/4.0/" },
  "frequency": "http://publications.europa.eu/resource/authority/frequency/MONTHLY",
  "themes": [{ "code": "ECON", "label": "Economy and finance" }],
  "spatial": ["SK032"],
  "temporal": { "start": "2020-01-01", "end": null },
  "contact": { "name": "Open data desk", "email": "opendata@example.org" },
  "modified": "2026-09-21T05:52:34Z",
  "catalogueUrl": "https://data.{host}/dataset/bbsk-kpi",
  "resources": [
    { "name": "CSV", "format": "CSV", "url": "https://api.{domain}/api/endpoint/{endpointSlug}/file.csv", "description": "…", "previewUrl": "https://data.{host}/dataset/bbsk-kpi/resource/{id}" }
  ],
  "endpoint": { "url": "https://api.{domain}/api/endpoint/{endpointSlug}/", "representations": ["ngsi-ld", "csv", "mcp"] },
  "model": {
    "name": "key-performance-indicator",
    "classes": [{ "name": "KeyPerformanceIndicator", "description": "…" }],
    "docsUrl": "https://api.{domain}/api/endpoint/{endpointSlug}/schema/1/key-performance-indicator.v1.md"
  }
}
```

- `previewUrl` is set on a resource with an active DataStore sheet (§3 of Architecture/21).
- `endpoint` and `model` are set only when the dataset's `endpoint` extra names an Endpoint of this
  installation whose audience is `public`: the Portal parses the slug out of the extra, finds the
  Endpoint in its mirror and builds the URL on its own host. `docsUrl` is the dataset's Markdown
  schema resource. A dataset whose extra names another host, or no Endpoint, has neither.
- `representations` are the Endpoint's `enabledRepresentations`; the "Use this data" snippets
  offer only what it serves.
- An unknown or private dataset answers `404`: a caller cannot tell the two apart. When no
  catalogue answers at all, `503`.

`GET /api/v1/catalogue/datasets/{name}/sample` answers up to ten entities of the Endpoint's first
model class, read anonymously through its NGSI-LD representation with `options=keyValues`, as
`{ "type": "KeyPerformanceIndicator", "columns": ["id", "name", "value"], "rows": [["urn:…", "…", "12"]] }`.
A dataset with no Endpoint of this installation, or one that does not serve `ngsi-ld`, answers
`404`; an Endpoint that does not answer, `503`. Nested values are written as compact JSON.

`GET /api/v1/catalogue/datasets/{name}/map` answers the located entities of the same class, read
the same way with `limit=1000&count=true`, for the map on the dataset page (EP-82, T-5628).
`lang` (two or three lowercase letters, default `en`; anything else answers `400`) picks the
labels:

```json
{
  "type": "AirQualityObserved",
  "total": 14,
  "fields": [{ "slot": "name", "label": "Názov" }, { "slot": "pm10", "label": "PM10" }],
  "features": [
    {
      "id": "urn:ngsi-ld:AirQualityObserved:a",
      "geometry": { "type": "Point", "coordinates": [14.42, 50.08] },
      "properties": { "name": "Legerova", "pm10": "21.5" }
    }
  ],
  "basemap": "praha"
}
```

- `fields` are the key attributes a popup shows, chosen by the Endpoint's `display` override, then
  the model's `summary` annotations, then jc-core's field order (DM-98, DM-99, EP-135); a
  structured value or a geometry is never one of them.
- `features` holds every entity with a GeoJSON geometry (`location` first, else the first
  attribute holding one); an entity without one is left out, so a dataset without locations
  answers no features and the page shows no map.
- `total` counts the entities of the class, located or not, when the Endpoint counted them; past
  1000 the page says how many it shows.
- `basemap` names the project whose basemap draws under the features, when the platform has one.
- The same `404` and `503` as the sample.

`GET /api/v1/catalogue/endpoints/{slug}/standards` answers the "Standards" part of a public
Endpoint's page (`/d/{slug}` and the Endpoint page in the Portal, TS-19, T-4484):

```json
{
  "slug": "{endpointSlug}",
  "types": ["AirQualityObserved"],
  "entityType": "AirQualityObserved",
  "standards": [
    {
      "representation": "ngsi-ld",
      "fields": ["name", "no2", "pm10"],
      "examples": [
        { "kind": "equality", "field": "name", "url": "https://api.{domain}/api/endpoint/{endpointSlug}/ngsi-ld/v1/entities?type=AirQualityObserved&q=name%3D%3D%22Zvolen%22", "answered": 1 },
        { "kind": "range", "field": "no2", "url": "https://api.{domain}/api/endpoint/{endpointSlug}/ngsi-ld/v1/entities?type=AirQualityObserved&q=no2%3E%3D21.5", "answered": 4 }
      ]
    },
    { "representation": "ogc-features", "fields": ["name", "no2", "pm10"], "examples": [] }
  ]
}
```

- Every read is anonymous, through the Endpoint on the gateway, so the answer holds what a
  visitor without a token may read and nothing a view hides. A non-public or unknown slug answers
  `404`.
- `standards` lists the query standards the Endpoint serves, in this order: `ngsi-ld`,
  `ogc-features`, `odata`, `sta`. `fields` are the names that standard filters on: the
  attributes of one entity for `ngsi-ld` and `sta`, the collection's queryables for
  `ogc-features`, and the properties of the entity type in `$metadata` for `odata`.
- The examples are built from one real entity, the first of the first type in `types` that has
  one (`entityType`): an `equality` on a text field with the entity's own value (no spaces or
  quotes) and a `range` (`>=`) on a numeric field with its value, written in the standard's own
  syntax (`q`, OGC `?{field}=` and CQL2 `filter`, OData and SensorThings `$filter`). Each one is
  run once and kept only when it answers `200` with at least one entity; `answered` is the
  number on its first page. No example is written by hand.
- A standard whose surface does not answer, or an Endpoint with no entity, lists no examples.
- The answer is kept an hour per Endpoint (`Cache-Control: public, max-age=3600`).

## 16b. Publish a dataset in one step (EP-83)

```text
POST /api/v1/projects/{project}/catalogue/drafts    {"endpoint": "bbsk-kpi"}
```

Drafts what the publish flow of [Architecture/21 §7](../Architecture/21-open-data-catalogue.md#7-publish-a-dataset-in-one-step)
proposes; it writes nothing. The caller needs `propose` on `Endpoint` in the project.

```json
{
  "endpoint": "bbsk-kpi",
  "makesPublic": false,
  "catalog": {
    "license": "CC_BY_4_0",
    "themes": ["ECON"],
    "keywords": { "sk": ["ukazovateľ", "hodnota"] },
    "temporal": { "start": "2020-01-01" },
    "publisher": { "name": { "sk": "Banskobystrický samosprávny kraj" } },
    "contactPoint": { "name": "Open data desk", "email": "opendata@example.org" }
  },
  "publish": { "ckan": { "instanceRef": { "kind": "CkanInstance", "name": "bbsk" },
                         "datastore": { "representation": "csv" } } },
  "missing": ["spatial", "frequency"],
  "attributes": [
    { "type": "KeyPerformanceIndicator", "name": "kpiValue" },
    { "type": "KeyPerformanceIndicator", "name": "reporterEmail", "withheld": "personal" }
  ],
  "datastoreAvailable": true
}
```

- `catalog` is the drafted `spec.catalog` (EP-78) and `publish` the drafted `spec.publish`
  (EP-62); a block the Endpoint already declares is returned as it is, so re-running the flow
  never overwrites what a steward wrote. `missing` names the catalogue fields nothing could fill;
  `spatial`, `temporal` and `frequency` are never drafted.
- `makesPublic` is `true` when the Endpoint's audience is not `public`: the UI then says so before
  the proposal, and the Change the UI proposes with `spec.audience: public` takes the red lane
  with a publisher's approval (EP-76, PF-72).
- The UI proposes the Change through the resource API (`PUT …/endpoints/{name}`, §4) with the two
  blocks merged into the Endpoint's manifest.
- `attributes` lists each slot of the space model's classes (`id` and `type` left out) with
  `withheld` when a public dataset cannot carry it: `personal` for a slot the model marks
  `personalData` (DM-122), `hidden` for one the Endpoint's `projection.hiddenAttributes` names
  already. The UI never offers a withheld slot, adds every personal one and each slot the person
  leaves out to `projection.hiddenAttributes`, and asks for the Endpoint's name typed back before
  it proposes a public publication (EP-142).
- `datastoreAvailable` is `true` when the Endpoint serves `csv`, the one representation a DataStore
  mirror reads (EP-65); a drafted `publish.ckan` then carries `datastore`.
- `404` for an unknown Endpoint; `409` naming `CkanInstance` when the project has none to publish to.

## 17. Federation registrations and the graph (MF-36, UI-27, UI-28, EP-71, PF-48)

A `ContextSourceRegistration` is a manifest, so it is listed, created and removed through the
resource routes every kind shares. Its plural is `csrs`:

```text
GET    /api/v1/projects/{project}/csrs            every registration in the project
GET    /api/v1/projects/{project}/csrs/{name}     one registration
POST   /api/v1/projects/{project}/csrs            propose a new one
PUT    /api/v1/projects/{project}/csrs/{name}     propose a change to one
DELETE /api/v1/projects/{project}/csrs/{name}     propose its removal
```

A write answers `202` with a `Change` like any other manifest, and a federation edge always
takes the Red lane: it makes one tenant's data answerable in another, which is a decision a
steward signs (CC-63). The manifest lands at `projects/{project}/spaces/{space}/registrations/{name}.yaml`.

What needs an operation of its own is the picture across five kinds at once, the registry's
`jc_federation_graph` (§21; T-4453 removed the `GET …/federation-graph` route beside it):

```text
POST /api/v1/projects/{project}/ops/jc_federation_graph   {}   who reads whose data, as nodes and edges
```

```json
{
  "nodes": [
    { "id": "ContextSpace/hub", "kind": "ContextSpace", "name": "hub", "health": "ok", "phase": "Live" },
    {
      "id": "ContextSourceRegistration/zvolen-ovzdusie",
      "kind": "ContextSourceRegistration",
      "name": "zvolen-ovzdusie",
      "health": "degraded",
      "phase": "Error",
      "registration": {
        "mode": "inclusive",
        "identity": "caller",
        "types": ["AirQualityObserved", "WeatherObserved"],
        "external": true
      }
    },
    {
      "id": "ExternalSource/zvolen-ovzdusie",
      "kind": "ExternalSource",
      "name": "zvolen-ovzdusie",
      "health": "unknown"
    }
  ],
  "edges": [
    {
      "from": "ContextSourceRegistration/zvolen-ovzdusie",
      "to": "ContextSpace/hub",
      "kind": "registers",
      "manifest": "ContextSourceRegistration/zvolen-ovzdusie"
    }
  ]
}
```

- A node id is `kind/name`, so a selection survives a refetch and two manifests naming the same
  object produce one node.
- `kind` is a manifest kind, or `ExternalSource` for a source outside this platform. A `CkanInstance`
  is a node when an Endpoint of the project publishes to it (Architecture/04 §7); its card carries no
  URL and no token. An external
  node is named after the registration that reaches it and never after its address, which is the
  rule provenance already follows (EP-71).
- `health` is `ok`, `degraded` or `unknown`, read off the phase the object last reported. Anything
  still converging is `unknown` rather than `ok` (UI-27).
- `phase` is the lifecycle phase the object last reported (`Draft`, `Pending`, `Deploying`, `Live`,
  `Error`), what the card's status chip shows (UI-25). It is absent for an external source and for
  an object that has reported nothing.
- `kind` on an edge is `registers`, `serves`, `feeds`, `consumes` or `publishes`, and `manifest`
  names the file the edge was read from so a reader can open it. `publishes` leaves an Endpoint for
  the `CkanInstance` its `spec.publish.ckan.instanceRef` names.
- A registration whose target has left the repository still draws its edge, with no node at the
  other end: a dangling reference is what this view exists to make visible.
- Nothing secret is in the answer by construction rather than by filtering. A card says that a
  registration authenticates and how, never with what: no token, no resolved `secretRef` and no
  `serviceAccountRef` name (PF-48).
- The graph is a projection of the manifests plus reported health. It holds no state and is not a
  second opinion about who talks to whom.
- Every route of this section needs a session, like every other project route.

## 18. Assistant catalog search (AG-58, UI-46)

The question a person asks the assistant, answered from the mirror (Architecture/09 §9) by the
registry's `jc_catalog_search` (§21), which is also the assistant's `search_catalog` tool, so a
run and a person get the same answer (T-4453 removed the `GET …/assistant/catalog` route beside it):

```text
POST /api/v1/projects/{project}/ops/jc_catalog_search   { "q": "bike availability", "scope": "Endpoint" }
```

Without `scope`, or with `"scope": "Dataset"`, the answer adds `datasets`: at most 10 public
datasets of the installation's open-data catalogues (CKAN `package_search`, read anonymously as
the catalogue page reads them), each `{ title, url, publisher, formats }` with `url` the dataset's
page; a catalogue that did not answer is named in `unavailable` (T-6027).

```json
{
  "q": "bike availability",
  "items": [
    {
      "kind": "Endpoint",
      "name": "helsinki-bikes",
      "space": "helsinki",
      "owner": "helsinki",
      "title": "Helsinki city bikes",
      "endpointSlug": "si6epqkx364lprho5uaigutk274r5grb",
      "matchReason": ["title", "description"],
      "access": { "verdict": "allowed", "reason": "audience public" },
      "freshness": { "pipeline": "hsl-bikes", "scrapedAt": "2026-09-13T07:41:02Z", "received": 1200, "errors": 0 }
    },
    {
      "kind": "ContextSpace",
      "name": "helsinki",
      "space": "helsinki",
      "owner": "helsinki",
      "matchReason": ["endpoint helsinki-bikes"],
      "access": { "verdict": "allowed", "reason": "endpoint helsinki-bikes admits you" },
      "freshness": null
    }
  ]
}
```

- `q` is required and is split into words of two characters or more, less the function words of a
  request (`the`, `a`, `to`, `and`, `new`, …); an empty `q` is `400`. A word matches a field when one
  of the field's own words begins with it: `bike` matches "city bikes", `the` does not match
  "weather". `scope` narrows to one kind. At most twenty items are returned, the best matches first.
- `matchReason` names the fields that matched (`name`, `title`, `description`, `labels`, `slug`,
  `classes`) or the endpoint through which a space or a model matched.
- `access.verdict` is `allowed` or `restricted` with the reason beside it; a restricted item carries
  its `kind`, `name`, `space` and `owner` only. The verdict comes from the endpoint's audience; the
  data's own grants are `GET …/access` on the endpoint (EP-55).
- `freshness` is the runner's counters for the pipeline that feeds the endpoint (§7), read when the
  search runs, or `null` when nothing feeds it or the runner does not answer within the scrape
  timeout. It is never a guess.
- A run publishes the same body as the `output` of a `tool` event named `search_catalog`
  (04-agent-runs §4), which the conversation renders as cards (UI-46).

## 19. Assistant endpoint proposal (EP-72)

The share request, rendered but not written: the same call the assistant's `propose_endpoint`
tool makes (Architecture/04 §5). The caller must be allowed to propose an Endpoint in the project
(PF-50), or the answer is `403`.

```text
POST /api/v1/projects/{project}/assistant/propose-endpoint
```

```json
{
  "contextSpace": "helsinki",
  "name": "bikes-regional-transport",
  "audience": "project-list",
  "allowedProjects": ["regional-transport"],
  "representations": ["ngsi-ld", "csv"],
  "hiddenAttributes": ["maintenanceNote"],
  "entityTypes": ["BikeHireDockingStation"]
}
```

```json
{
  "lane": "yellow",
  "slug": "k7m2p9q4r6s8t3v5w7x2y4z6a8b3",
  "endpoint": { "apiVersion": "joinedcontext.com/v1alpha1", "kind": "Endpoint", "metadata": { "name": "bikes-regional-transport", "namespace": "helsinki" }, "spec": { "contextSpaceRef": "helsinki", "slug": "k7m2p9q4r6s8t3v5w7x2y4z6a8b3", "audience": "project-list", "allowedProjects": ["regional-transport"], "enabledRepresentations": ["ngsi-ld", "csv"], "projection": { "hiddenAttributes": ["maintenanceNote"] } } },
  "policies": [ { "apiVersion": "joinedcontext.com/v1alpha1", "kind": "Policy", "metadata": { "name": "bikes-regional-transport-regional-transport", "namespace": "helsinki" }, "spec": { "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" }, "assigner": "did:web:hel.fi", "assignee": { "kind": "group", "id": "regional-transport" }, "operations": ["retrieveOps"], "information": [ { "entities": [ { "type": "BikeHireDockingStation" } ] } ] } } ],
  "prefill": { "name": "bikes-regional-transport", "contextSpaceRef": "helsinki", "slug": "k7m2p9q4r6s8t3v5w7x2y4z6a8b3", "audience": "project-list", "allowedProjects": ["regional-transport"], "enabledRepresentations": ["ngsi-ld", "csv"], "hiddenAttributes": ["maintenanceNote"] }
}
```

- `audience` defaults to `project-list`; `project-list` needs at least one project in
  `allowedProjects`; `representations` defaults to `ngsi-ld` and `csv`; every name is a
  DNS-1123 label and every attribute an identifier, or the answer is `400`.
- `access` names what the new endpoint's Policy grants: absent, `retrieveOps` (a share); `read`,
  `update` or `full`, the operations of AP-132 for an endpoint an app proposes; `create`,
  `createEntity` and nothing else, for a public form (§33, T-3103), which needs exactly one type in
  `entityTypes`, names in `writeAttributes` the properties and in `writeRelationships` the
  relationships the form asks for (the Policy's `propertyNames` and `relationshipNames`, at least
  one name, so any other attribute is refused, T-3172) and may set
  `createsPerDay` (default 200, at most 10,000), which the Endpoint carries as `spec.creates`
  `{mintIds: true, perDay}` (EP-97), and `embedOrigins`, the sites that may frame the form, which
  it carries as `spec.creates.embedOrigins` (EP-101).
- The slug is minted here and is read-only in the form; a slug in the request is ignored.
- `lane` is what the Endpoint's Change would be classified as (§5): `red` for `public`.
- Nothing is written. A run publishes this body as the `output` of a `tool` event named
  `propose_endpoint` and follows it with a `navigate` to `/projects/{project}/endpoints`
  carrying `prefill` (04-agent-runs §4); the person's submission is the Change.

## 20. Drift: what the platform holds and Git does not declare (CC-21, CC-38, UI-25, UI-26)

Configuration cannot drift: every component reads it from the repository, so the repository is
what it is running (CC-72). What can drift is the one live state Git describes and nothing
reloads by itself — a space's seed entities. The Portal compares them on its own tick, the same
comparison `jcctl drift` makes, and serves the result per project.

```text
GET  /api/v1/projects/{project}/drift                         what drifted, with the two resolutions
POST /api/v1/projects/{project}/drift/{space}/{id}/revert     write Git's entity back into the broker
POST /api/v1/projects/{project}/drift/{space}/{id}/adopt      propose the live entity as the declared one
```

`{id}` is the entity's URN, percent-encoded. The list answers what the last tick found, with the
instant it ran at, so a page renders without waiting on the broker:

```json
{
  "apiVersion": "joinedcontext.com/v1alpha1",
  "kind": "List",
  "metadata": { "observedAt": "2026-09-17T09:12:03Z" },
  "items": [
    {
      "space": "ovzdusie",
      "id": "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:stanica-1",
      "drift": "MODIFIED",
      "diff": [{ "path": "airQualityIndex.value", "declared": 42, "live": 7 }],
      "resolutions": ["revert", "adopt"],
      "source": "projects/mesto/spaces/ovzdusie/entities/seed/stanice.json"
    }
  ]
}
```

`drift` is `MODIFIED` when the broker holds the entity and an attribute the file declares
differs, and `MISSING` when the broker does not hold it at all. There is no `UNEXPECTED`: a space
holds what its pipelines and its devices write, and an entity nobody seeded is data, not drift
(CC-07, CC-69). Which attributes Git owns is the same answer: the ones the seed file declares,
and no others — live telemetry beside a seeded entity never drifts.

**Revert** upserts the declared entity through the space surface under the Portal's own
ServiceAccount and answers `204`; it writes only the attributes the file declares and never
deletes an entity or an attribute (CC-19).

**Revert** and **adopt** are the two resolutions UI-26 puts on the screen, and **adopt** is the
only way an out-of-band value becomes durable: it answers `202` and an ordinary `Change`
proposing the live entity as the seed file's content, reviewed in the lane its content earns like
any other change (CC-38). An entity the broker no longer holds cannot be adopted, and the answer
says so rather than proposing an empty file.

Both refuse with `403` unless the caller may `propose` `Entity` in that project; revert is a
write to the live space and adopt is a write to the repository, so neither is a read.

## 21. Operations and the Portal MCP (AG-59, AG-60, AG-61, AG-62, AG-77)

One registry behind every door (ADR-N-021). An operation lets through whom the REST route of the same action lets through (PF-50, AG-77): `jc_resource_list` and `jc_resource_get` any signed-in person, `jc_resource_propose` and `jc_resource_delete` a binding with `propose` or `delete` on the kind, `jc_change_approve` and `jc_change_reject` a binding with `approve`, on the change's kind once the change is read; any other operation its verb on its kind, or a grant in the project when it names no verb. A refusal is `403` naming the verb and the kind, the same words as the route's, except that a removal of a resource the caller may not read answers the `404` of a name that does not exist, so a removal never tells whether something is there (R20, PF-59); input is validated against the operation's schema with unknown fields refused (`422`, the schema path). `GET …/ops` lists exactly the operations the caller would be let through. A `verdict_required` refusal names the check to run and why in `reason` (`verdict_absent`, `verdict_failed` or `stale`) and says it in `detail`, one sentence a page shows as it is; a form writes the draft it shows before it proposes that draft, so a proposal never takes an older manifest than the one on screen.

```text
GET  /api/v1/projects/{project}/ops
  → 200 [{ "name": "jc_datasource_check", "title": "…", "description": "…",
           "inputSchema": {…}, "outputSchema": {…},
           "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true },
           "lane": "green" }, …]          # only what this caller may run

POST /api/v1/projects/{project}/ops/{name}
  body: the operation's input; a draft may be named instead of a manifest:
        { "draft": { "kind": "DataSource", "name": "hsl-citybikes-free" } }
  → 200 the operation's output (a check answers a Verdict)
  → 202 a proposal: { "changeId": "…", "lane": "yellow", "url": "…" }
  → 409 { "error": "verdict_required", "check": "jc_datasource_check", "reason": "stale",
          "detail": "The draft changed since its check; check it again, then propose it." }
  → 403 { "error": "forbidden", "role": "steward" }
  → 422 { "error": "invalid_input", "path": "/spec/http/url", "message": "…" }

Verdict:
  { "ok": false, "findings": [{ "level": "error", "path": "/spec/http/url", "message": "…" }],
    "trace": {…}, "checkedAt": "2026-09-13T10:00:00Z", "inputDigest": "sha256:…" }

Drafts (also operations: jc_draft_put, jc_draft_get, jc_draft_drop):
  { "project": "helsinki", "kind": "DataSource", "name": "hsl-citybikes-free",
    "manifest": {…}, "verdict": {…} | null, "version": 7,
    "touchedBy": "jana.kovacova", "touchedKind": "person" | "mcp" | "run",
    "updatedAt": "…" }                                 # the REST routes are §4
  Draft changes are `draft` events on the activity stream (§7 of Architecture/09).
  jc_draft_list (GET /projects/{project}/drafts) answers one line per draft and never the
  manifest or the verdict's trace, the way jc_resource_list answers a line per resource:
  { "items": [{ "kind": "DataSource", "name": "hsl-citybikes-free", "version": 7,
                "touchedBy": "jana.kovacova", "touchedKind": "person", "updatedAt": "…",
                "verdict": { "ok": true, "findings": 0, "checkedAt": "…" } | null }] }
  A caller that needs a draft reads it with jc_draft_get. A project holding 44 drafts answered
  2.3 MB of manifests and verdict traces, which is more than one model call can carry, so the
  assistant could answer nothing at all while those drafts existed (T-2248).

Resources (AG-77): one operation per action for every kind the resource API serves, the
function behind the REST route of that action. A proposal, a deletion and a rejection answer
the Change the route answers, wrapped with its id, lane and merge request; a refusal is the
route's problem document.
  jc_resource_list     { "kind": "Pipeline", "space": "helsinki" }            # space optional
    → 200 { "items": [{ "kind": "Pipeline", "name": "bikes-hsl", "space": "helsinki",
                        "title": "…", "phase": "Ready", "sourceUrl": "…" }] }
    → 422 { "error": "invalid_input", "path": "/kind", "message": "'Bikes' is not a kind …; the kinds are …" }
    An Endpoint's line adds `types` (its spec's `entityTypes`, else the classes of its space's
    model) and `url`, its address on the data host (T-6027).
  jc_resource_get      { "kind": "Endpoint", "name": "helsinki-bikes" }
    → 200 the manifest with its status; secret values never, `secretRef` names only
    An Endpoint's answer adds `access`: `url`, `types`, `surfaces` (one `{ name, url }` per address
    of each representation the Endpoint serves: `ngsi-ld/v1/`, `odata/v4/`, `ogc/features/` and a
    `geojson` items address per type, `file.csv`, `sta/v1.1`, `ws`, `mcp`, each feed and calendar
    output; a representation it does not serve is not offered) and, for a public Endpoint, `embed`,
    the viz host's frame (`https://viz.{domain}/embed.html#{slug}&type={T}`, as its Embed button
    writes it). `"writers": true` adds `writers`: each person, group, role or ServiceAccount the
    gateway grants a write on one of its types, with the types and the Policies (`via`), from the
    access matrix (§57), so only an organization administrator or a reader of the space's Policies
    is answered (`403` otherwise) and the read is recorded; on another kind `writers` is `422`.
    → 404 problem: "Endpoint 'helsinki-bike' not found in project 'helsinki'; it has helsinki-bikes, …"
  jc_resource_propose  { "manifest": { "apiVersion": "…", "kind": "Pipeline",
                                       "metadata": { "name": "bikes-hsl" }, "spec": { … } } }
                                                             # or { "draft": { "kind", "name" } }
                       { "manifest": {…}, "files": { "./tests/x.input.json": "…" } }   # §4 rules
    → 202 { "changeId": "chg-00000081", "lane": "yellow", "url": "…/pulls/81",
            "change": { "kind": "Change", "status": { "plan": { "create": 0, "update": 1, "delete": 0 }, … } } }
    → 409 { "error": "verdict_required", "check": "jc_manifest_dry_run", "reason": "stale",
            "detail": "The draft changed since its check; check it again, then propose it." }   # a named draft, strict
    → 409 { "error": "verdict_required", "check": "jc_manifest_dry_run", "reason": "verdict_absent",
            "detail": "The manifest has not been checked; check it, then propose it." }   # a bare manifest never checked, strict
    → 400 problem: "literal secret in field 'token' is forbidden; use secretRef instead (MF-24)"
    → 403 problem naming the kind and the verb the caller lacks (PF-50)
  jc_resource_delete   { "kind": "Endpoint", "name": "helsinki-bikes-ops", "confirm": "helsinki-bikes-ops" }
    → 202 { "changeId": "chg-00000082", "lane": "red", "url": "…", "change": { … "plan": { "delete": 1 } } }
    → 409 { "error": "referenced", "detail": "2 dependent resources block deletion: Pipeline bikes-hsl and 1 in other projects",
            "by": [{ "kind": "Pipeline", "name": "bikes-hsl" }], "elsewhere": 1 }
    → 400 problem: "confirm must be the resource's name, 'helsinki-bikes-ops' (CC-39)"
  jc_change_reject     { "id": "chg-00000081", "reason": "The schedule stays hourly." }
    → 202 { "changeId": "chg-00000081", "lane": "yellow", "url": "…", "change": { … "phase": "Rejected" } }
  An assistant or application run calling jc_change_approve or jc_change_reject is refused
  (AG-11): 403 problem "an agent never approves or rejects a change; a person does (AG-11)".
  Four more operations are the person's in the same way, refused for a run whatever its profile
  names and so absent from its `GET …/ops` and its `tools/list`: jc_run_create, jc_run_cancel and
  jc_run_publish (a run does not drive another run) and jc_run_answer (a run does not answer the
  question a run asked the person it acts for, AG-45). Each refusal names its own act. Proposing a
  deletion is not among them: jc_resource_delete and jc_project_delete stay open to a run and to
  an MCP client, because a deletion is a Red change a person still approves (AG-77, CC-39).
  The rejection's reason is written on the merge request beside who rejected it; the REST route
  takes it as `{ "reason": "…" }` too.
  A run reaches this registry through the proxy's `/v1/mcp`, which the Portal serves on its
  internal listener as `POST /internal/agent-runs/{run}/mcp`: the call runs as the person who
  started the run and is narrowed again by the run's `AgentProfile` (AG-70), so `tools/list`
  answers only what both halves allow and a `tools/call` outside the profile is refused naming
  it. A profile never widens: an operation the person may not run stays refused whether the
  profile names it or not.

Example, an MCP client removing an endpoint:
  POST /api/v1/mcp
  { "jsonrpc": "2.0", "id": 7, "method": "tools/call",
    "params": { "name": "jc_resource_delete",
                "arguments": { "project": "helsinki", "kind": "Endpoint",
                               "name": "helsinki-bikes-ops", "confirm": "helsinki-bikes-ops" } } }
  → { "jsonrpc": "2.0", "id": 7, "result": { "isError": false, "structuredContent": {
      "changeId": "chg-00000082", "lane": "red", "url": "…", "change": { … } } } }

MCP (Streamable HTTP, protocol 2026-07-28):
  POST /api/v1/mcp                         # server/discover, tools/*, resources/*, prompts/*, tasks/*
  GET  /.well-known/oauth-protected-resource/api/v1/mcp   # RFC 9728
  Bearer only, aud = the Portal; 401 + WWW-Authenticate: Bearer resource_metadata="…" otherwise.
  The revisions this server speaks are `2026-07-28`, `2025-11-25`, `2025-06-18` and `2025-03-26`.
  `initialize` and `server/discover` answer the client's own revision when it names one of them
  and `2026-07-28` otherwise — never the client's string for a revision the server does not
  implement, which would move the disagreement from the handshake to the first unknown method.
  tools/list = the registry as GET …/ops shows it to this caller.
  resources/list and resources/read serve, for the caller's own bindings (AG-60):
    jc://schemas/{Kind}                       the kind's JSON Schema
    jc://{project}/{plural}/{name}            one manifest
    jc://{project}/drafts/{Kind}/{name}       one draft
    jc://{project}/changes/{changeId}         one change with its plan, redacted as the route
                                              redacts it (CC-06)
    jc://{project}/datamodels/{name}/linkml   the model's LinkML source. The JSON Schema and the
                                              `@context` are the endpoint façade's renderings of
                                              it and are read there (EP-47).
  A URI the caller may not read answers `-32002 resource not found`, the same as one that names
  nothing.
  Grounding (AG-12, CC-47): every `tools/call` result — an answer, a conflict, a refusal, an
  elicitation and a task's `tasks/result` — carries in `_meta` the state of the configuration it
  was evaluated against, so an agent can tell a fresh answer from a stale one without asking again:
    "_meta": { "joinedcontext.com/grounding": {
      "revision": "<the commit the Portal's mirror holds, null before the first sync>",
      "syncedAt": "<UTC RFC 3339 of that sync, null before the first>",
      "evaluatedAt": "<UTC RFC 3339 of this answer>",
      "drift": "none" | "sync-failed" | "unsynced" } }
  `drift` is the mirror against the repository: `unsynced` before the first sync, `sync-failed`
  when the last sync did not complete (the answer is from `revision`, which may be behind the
  repository), `none` otherwise. It never carries the repository's URL, a branch or a token.
  Elicitation (AG-63): a call whose operation takes the Yellow or Red lane, or carries
  `destructiveHint`, never runs on the agent's word. The first `tools/call` runs nothing and
  answers
    { "isError": false, "status": "input_required",
      "content": [{ "type": "text", "text": "<the sentence>\n\nThe person decides here: <url>\n…" }],
      "_meta": { "joinedcontext.com/elicitation": { "elicitation": {
        "elicitationId": "eli-…", "mode": "url", "url": "https://portal…/projects/{project}/…",
        "message": "<what this call would do, in one sentence>",
        "expiresIn": 600 } } } }
  The text names the URL and the `elicitationId`, because a client hands its model the text
  alone (T-6361). The answer carries no `structuredContent`: every tool declares an
  `outputSchema` and a question is not the tool's output, so `_meta` holds the same fields for a
  client that reads them by name. A proposal whose check has not run carries the route's own
  refusal beside the question (PF-57, ADR-N-021): the `_meta` object gains `"error":
  "verdict_required"`, `"check": "<the operation to run>"`, `"reason": "verdict_absent" |
  "verdict_failed" | "stale"` and the same one-sentence `detail` at its top level, with
  `elicitation` beside them, and the text names the check. A client that reads
  `verdict_required` on the route reads it here too and knows which check to run, and the person
  still has the URL to run it at. The door refuses either way: nothing is proposed without a
  fresh Verdict.
  The host shows it to the person, who opens the URL and decides; the client then repeats the
  same `tools/call` with `params.elicitation = { "elicitationId": "eli-…" }`, or with
  `elicitation` among its `arguments` for a client that can only fill arguments, and the
  operation runs once the person accepted on the page. `"action": "decline"` (or `"cancel"`) answers a refusal and nothing
  runs. An id is one-shot, belongs to the token's subject, expires in ten minutes and is bound to
  the operation and the arguments it was asked for: different arguments need a new answer. Every
  `tools/call` naming a known operation in a project the caller reads is one `mcp.tool` row from
  `portal` on that project's activity (T-5609). Its `details` hold `operation`, `decision`
  (`ran`, `started` for a task, `asked`, `declined` or `refused`), `answer` (`accepted` or
  `declined`) when the call carried one, `subject`, `username`, `channel` `mcp`, `client` (the
  token's `azp`) and `change` with the Change's name, also its `correlationId`, when the call
  opened one. Names only: no argument value or manifest content reaches the row.
  Tasks (AG-60): `jc_pipeline_test`, `jc_model_infer`, `jc_space_complete` and
  `jc_datasource_check` wait on the runner, on Model Tools or on somebody's feed, so `tools/call`
  answers `{ "task": { "taskId", "status": "working", "pollInterval", "ttl" } }` at once and the
  client follows it with `tasks/get`, `tasks/result` (the tool's own result), `tasks/list` and
  `tasks/cancel`; `tools/list` marks them `execution.taskSupport: "required"` and every other tool
  `"forbidden"`. A task belongs to the token's subject: another caller's `tasks/get` answers
  `unknown task`, the same as one that never existed. A finished task is kept ten minutes.
  Limits (AG-60), counted by the token's subject in the Portal itself, so an in-cluster caller
  is bounded too: a ninth call running at once, counted with the person's connectors → 429 with
  `Retry-After`; a request body over 1 MiB and an answer over 4 MiB →
  413 naming what to narrow. Calls a minute have no limit in the Portal (T-6484). The edge's own
  bucket stays on top of them.
```

## 22. Workspaces (CC-76…CC-81, PF-82, PF-83, AG-82, UI-61…UI-63)

A workspace is a branch of the Organization repository with a record beside the drafts ([ADR-N-024](../Decisions/adr-n-024-workspaces-branch-and-preview.md), Architecture/06 §7). The same actions are operations of the registry (§21), so the Portal, REST, MCP and the assistant reach one implementation: `jc_workspace_open`, `jc_workspace_list`, `jc_workspace_get`, `jc_workspace_compare`, `jc_workspace_update_from_main`, `jc_workspace_propose`, `jc_workspace_discard`, and in the second phase `jc_workspace_preview_start`, `jc_workspace_preview_get` and `jc_workspace_preview_stop`. Opening, writing into and proposing a workspace need `propose` in the project; reading one needs `read`, and only its owner writes into it or brings it back. An agent may open, write, compare and update, and is refused `jc_workspace_propose` and every approval (AG-82, AG-11); an MCP client is not offered it. A comparison lists a file both sides changed under `conflicts` even when its fields do not collide (`fields` empty): bringing back waits until the workspace is updated from main.

```text
POST   /api/v1/projects/{project}/workspaces
  body: { "name": "bikes-cleanup", "title": "Bike stations cleanup",
          "scope": { "kind": "project" },              # or { "kind": "space", "name": "helsinki" }
                                                       # or { "kind": "resources", "items": [{ "kind": "Pipeline", "name": "bikes-ingest" }] }
          "ttlDays": 7 }
  → 201 the workspace
  → 409 a workspace of that name exists in the project

GET    /api/v1/projects/{project}/workspaces            → 200 { "items": [ workspace, … ] }
GET    /api/v1/projects/{project}/workspaces/{name}     → 200 the workspace, or 404

POST|PUT|PATCH|DELETE /api/v1/projects/{project}/{plural}[/{name}]?workspace={name}
  the ordinary resource routes (§4), committed to the workspace's branch instead of a Change
  of their own: the same checks (PF-82), and a 200 with the workspace in place of a 202 Change

GET    /api/v1/projects/{project}/workspaces/{name}/compare
  → 200 { "files": [ ChangeFile, … ],                  # §5, each with its fields, lane and operation
          "conflicts": [ { "path": "…", "fields": [ { "path": "spec.period", "ours": "30s",
                                                      "theirs": "60s", "base": "10s" } ] } ] }

POST   /api/v1/projects/{project}/workspaces/{name}/update
  body: { "resolutions": [ { "path": "…", "field": "spec.period", "keep": "ours" } ] }   # "theirs"; field "" is the whole file
  → 200 { "taken": [ "…" ], "merged": [ "…" ], "baseRevision": "…", "comparison": { … } }
       main merged into the branch: a file only main changed is taken, a file both changed is
       merged field by field and checked again before it is written; the base moves to main (CC-80)
  → 409 a conflicting field without a resolution; nothing is written

POST   /api/v1/projects/{project}/workspaces/{name}/propose
  → 202 the Change: the pull request of the branch, lane the riskiest file's (CC-79)
  → 409 a conflict not resolved, or a touched file without a fresh green Verdict (CC-80, PF-57)

DELETE /api/v1/projects/{project}/workspaces/{name}     → 204; the branch and any preview go (CC-81)

POST   /api/v1/projects/{project}/workspaces/{name}/preview   → 202 the preview (CC-78)
  → 200 { "state": "running", "prefix": "ws-bikes-cleanup-",
          "endpoints": [ { "name": "bikes-ops", "slug": "q3v7…",         # minted, Architecture/06 §7.2
                           "url": "https://…/api/endpoint/q3v7…",
                           "originSlug": "zt4q…" } ],                  # main's, absent for a new Endpoint
          "pausedPipelines": [ "bikes-ingest" ] }                  # the same body as the 202
  → 409 a preview of this workspace runs already, or two run on the node (ADR-N-024 §10),
        or the loader refuses the render: the preview is then `error` with that reason (CC-78)
GET    /api/v1/projects/{project}/workspaces/{name}/preview   → 200 the preview, as above
DELETE /api/v1/projects/{project}/workspaces/{name}/preview   → 204; stopping a stopped one is a no-op
```

The gateway reads the running previews from the Portal's internal listener, which the edge does not route and a NetworkPolicy opens to the gateway alone: the path `/internal/previews` answers `{ "items": [ { "prefix": "ws-bikes-cleanup-", "files": { "<path>": "<manifest>" }, "readers": ["<Keycloak user id>"], "salt": "<22+ base64url characters>" } ] }`: `readers` are the Keycloak user ids of whoever started the preview and of everyone a binding in force lets approve in its project, and `salt` is 128 bits of the OS random source minted at each start, so a restarted preview answers on new slugs (CC-91, CC-92). The answer leaves out the repository's encrypted secrets and every other project's files. The gateway presents its own Keycloak ServiceAccount token on that call — `client_credentials`, audience `portal-internal`, `azp` its own client — and the route refuses a call without one: the NetworkPolicy is the second control and never the only one (PF-46, [Architecture 13 §6](../Architecture/13-security.md)). A page of manifests is what the route answers, so a pod that reached the port through a policy mistake would otherwise read every project's configuration. The answer carries an `ETag`, a SHA-256 over the body: the gateway sends it back as `If-None-Match` on its next fetch, every ten seconds, and a list that did not change answers `304` with no body, so an idle instance moves no manifests (CC-78). Authentication comes first, so a call without the token learns nothing from a `304` either.

The gateway reads the domain verification of every Organization from the same listener, with the same token, the same `ETag` and the same ten-second fetch: `GET /internal/domain-verifications` answers `{ "items": [ { "organization": "hel", "domain": "hel.fi", "state": "verified" } ] }`, one item per Organization manifest of the repository, `state` one of `pending`, `verified`, `failed` and `pending` for an Organization the Portal has not checked yet. The challenge, the record and the reason stay off this route: the gateway decides on the state alone ([Architecture/03 §3](../Architecture/03-domain-model.md#3-identity-and-urn-specification)). Under `JC_GATEWAY_DOMAIN_VERIFICATION=enforce` a write to a space of an Organization that is not `verified`, or whose state the gateway does not know, answers `403` with the problem type `https://joinedcontext.com/errors/domain-not-verified` and a detail naming the Organization and its domain.

The gateway reads the revoked subjects from the same listener, with the same token, the same `ETag` and the same ten-second fetch: `GET /internal/revocations` answers `{ "items": [ { "subject": "<Keycloak user id>", "revokedAt": 1798761600 } ] }`, one item per subject the Portal disabled, deleted or signed out, or whose ServiceAccount client it removed, or whose session Keycloak ended through the back-channel logout, for as long as a token issued before the mark could still be valid. The gateway refuses a token of a listed subject issued at or before `revokedAt` on every door that verifies one (REST, the hub and Endpoint MCP, a WebSocket upgrade, an MQTT CONNECT) with `401`; a token issued later, after the person was enabled again and signed in anew, is admitted. A failed fetch keeps the last list (T-5568, TR-03187 AR-6).

Only the owner starts or stops a preview; an agent may, as it may open and write the workspace. Real data reaches a preview only when the person copies it from the Try it panel (PF-83): the browser reads at most 1 000 entities per type through the origin's Endpoint and writes them through the preview's, both on the person's own session at the edge, with every id moved to the preview's space segment. The Portal holds no token of the person's for the gateway, so no operation of the registry copies data, and an agent never does. The operations are `jc_workspace_preview_start`, `jc_workspace_preview_get` and `jc_workspace_preview_stop`.

A workspace:

```json
{
  "name": "bikes-cleanup",
  "title": "Bike stations cleanup",
  "project": "helsinki",
  "owner": "jana@example.org",
  "branch": "workspace/bikes-cleanup",
  "baseRevision": "9c1f0ab",
  "scope": { "kind": "project" },
  "createdAt": "2026-09-18T09:00:00Z",
  "expiresAt": "2026-09-25T09:00:00Z",
  "changes": 3,
  "preview": { "state": "stopped" }
}
```

A write the owner's rights do not cover is refused as it is outside a workspace:

```json
{
  "type": "https://joinedcontext.com/errors/forbidden",
  "title": "Forbidden",
  "status": 403,
  "detail": "Proposing a Pipeline needs a role with propose on Pipeline in helsinki."
}
```

## 23. Routes outside the resource API

The OpenAPI document describes `/api/v1` and nothing else, so these routes are in it only where
noted. An integrator meets the first three before any resource: the login, the MCP discovery and
the MCP endpoint itself.

| Method | Path | Who calls it | What it answers | Authentication |
|---|---|---|---|---|
| `GET` | `/api/v1/auth/login` | a browser | a redirect into the realm's code flow (§3) | none: it is how a session starts |
| `GET` | `/api/v1/auth/callback` | the realm, through the browser | the session cookie and a redirect to the page asked for (§3) | the code and the stored nonce |
| `GET` | `/api/v1/auth/sso-check`, `/api/v1/auth/sso-check/done` | the Open page's hidden frame | a silent (`prompt=none`) check of the realm session and the empty page it lands on (§3, AP-122) | the Portal session; the realm's own cookie decides the answer |
| `POST` | `/api/v1/auth/backchannel-logout` | Keycloak | revokes the sessions the logout token names (§3) | the logout token, verified against the realm's keys |
| `GET` | `/.well-known/oauth-protected-resource` | an MCP client | the RFC 9728 metadata naming the Portal's MCP resource and its realm | none: RFC 9728 makes it public, and it names no secret |
| `GET` | `/.well-known/oauth-protected-resource/api/v1/mcp` | an MCP client | the same document, at the path RFC 9728 derives from the resource | none, as above |
| `POST` | `/api/v1/mcp` | an MCP client, the assistant | JSON-RPC over Streamable HTTP (§21); in the OpenAPI document | a Bearer token whose audience is the Portal; `401` with `WWW-Authenticate` otherwise |
| `GET` | `/api/v1/mcp` | an MCP client probing for a stream | `405`: the server opens no server-initiated stream | none needed to learn that |
| `GET` | `/api/v1/openapi.json` | a client generator, the docs lane | this API's OpenAPI 3.1 document | none: it describes the API, not a project |
| `GET` | `/apps/{name}/`, `/apps/{name}/{path}` | a browser | a published static app's `index.html` and assets (§12) | a `public` app is served to anyone; any other visibility needs a session, and what the app then reads is its endpoints' authorization |
| `POST` | `/apps/{name}/api/functions/{fn}` | a published app's SDK | the function's own answer from `jc-functions` (§12) | as the app's pages; with the edge's token, also the CSRF double-submit |
| `GET` | `/metrics` | the cluster's Prometheus | the Portal's counters in the Prometheus text format | none, and the edge refuses the path, so only a caller inside the cluster reaches it (OPS-16) |

A route here that needs no authentication says so because of what it carries, never for
convenience: none of them answers anything of a project.

## 24. People (PF-90…PF-94)

A person is a user of the organization's Keycloak realm and never a manifest
([ADR-N-031](../Decisions/adr-n-031-people-groups-and-app-groups.md)). The Portal manages people
through the realm's admin API with its admin client, which holds `manage-users` and
`query-groups` of `realm-management` and nothing else (PF-63). Membership stays as code: a person
joins a group through the `Group` manifest, and deleting a person is a Change.

```text
GET    /api/v1/organization/people                          search and page → 200
POST   /api/v1/organization/people                          create → 201
GET    /api/v1/organization/people/{id}                     one person, their groups and roles → 200
PATCH  /api/v1/organization/people/{id}                     edit the name, the e-mail, the language → 200
POST   /api/v1/organization/people/{id}/disable             disable and end every session → 200
POST   /api/v1/organization/people/{id}/enable              enable → 200
POST   /api/v1/organization/people/{id}/reset-password      send a password reset → 202, or 200 with a temporary password
POST   /api/v1/organization/people/{id}/resend-invitation   send a pending invitation again → 202, or 200 with a temporary password
POST   /api/v1/organization/people/{id}/remove-second-factor  remove every OTP and WebAuthn credential → 204
POST   /api/v1/organization/people/{id}/sign-out            end every session → 204
DELETE /api/v1/organization/people/{id}                     remove → 202 with the Change, or 204
POST   /api/v1/organization/people/remove                   remove up to 200 at once → 202 with the Change, or 204
```

`{id}` is the Keycloak user id. `GET` takes `search` (a substring of the name or e-mail), `first`
(default 0) and `max` (default 50, at most 100), and answers the page with `next`, the `first` of
the following page, when there is one:

```json
{
  "items": [
    {
      "id": "7d1f0c9e-4b8a-4f63-9a51-2c0d8e3b6f14",
      "email": "jana.kovacova@example.org",
      "firstName": "Jana",
      "lastName": "Kováčová",
      "locale": "sk",
      "enabled": true,
      "emailVerified": true,
      "requiredActions": [],
      "createdAt": "2026-09-24T09:12:40Z",
      "lastSeen": "2026-09-25T07:02:11Z",
      "pendingDeletion": null,
      "invitationExpires": null
    }
  ],
  "next": 50
}
```

- `lastSeen` is the last access of the person's newest open session, `null` when none is open. The
  realm keeps no login history the Portal's client may read, so there is no "last login".
- `pendingDeletion` names the Change a deletion waits for, `null` otherwise.
- `invitationExpires` is when the link of the last invitation the Portal sent stops working, while
  the person still has `requiredActions` left; `null` once they are done, or when the Portal has
  no database to remember the sending in (PF-108).

`GET {id}` answers the person with where they are granted something, each item naming the
manifest that grants it (PF-94):

```json
{
  "person": { "id": "7d1f0c9e-…", "email": "jana.kovacova@example.org", "…": "as above" },
  "groups": [{ "name": "helsinki-stewards" }],
  "platformRoles": [
    { "role": "steward", "binding": "stewards", "scope": { "project": "helsinki" }, "via": { "group": "helsinki-stewards" } }
  ],
  "appRoles": [
    { "project": "helsinki", "app": "helsinki-alerts", "role": "steward", "via": { "user": "jana.kovacova@example.org" } }
  ]
}
```

Creating takes the e-mail, the name, the language and the project the invitation leads into, and
nothing else (`deny_unknown_fields`):

```json
{ "email": "jana.kovacova@example.org", "firstName": "Jana", "lastName": "Kováčová", "locale": "sk", "project": "helsinki" }
```

`project` is optional and names a project of the organization (`404` otherwise): the e-mail's link
ends, once the person has set their password, on `/projects/{project}/home?welcome=1`, where the
project's home page greets them with the first step of the role they hold there (PF-108). It
grants nothing: the Portal proposes the person's role as a `RoleBinding` through the one propose
function, and the role holds once that Change is approved, like any other grant (PF-95).

The e-mail is also the username, the name members and subjects name the person by today (PF-04); [ADR-N-061](../Decisions/adr-n-061-group-members-without-personal-data.md) moves a `Group` member, a binding subject and a `Policy` assignee to `person.id`, the Keycloak user id, once PF-126 is built. The
realm sends its execute-actions e-mail, `VERIFY_EMAIL` and `UPDATE_PASSWORD` (PF-92), and the
answer is `201` with `{ "person": {…}, "emailSent": true }`. When the realm cannot send mail, the
Portal sets a temporary password with `UPDATE_PASSWORD` required instead and answers it once, as
`{ "person": {…}, "emailSent": false, "temporaryPassword": "…" }`. A password reset works the same
way: `202` when the e-mail went, `200` with `temporaryPassword` when it could not. The Portal never
stores, logs or returns that password again, and nothing else ever carries it.

`resend-invitation` sends the execute-actions e-mail again for the steps the person has not taken,
with a fresh link of the organization's `invitationHours`, and answers like a password reset. A
person with no step left is `409`: there is nothing to accept. It needs `create` on Person and
every right the person holds. Revoking an invitation is `DELETE` on a person who has not accepted
it: a person no manifest names yet is deleted at once, and the link stops working with them.

`PATCH` takes any of `firstName`, `lastName`, `email` and `locale`. A changed e-mail is set
unverified and `VERIFY_EMAIL` is required again.

`DELETE` proposes one Change, in the organization repository, that takes the person's e-mail out
of every `Group` member list and every `RoleBinding` subject list naming it, and removes a binding
left with no subject. The person is disabled at once and marked with the Change; the reconciler
deletes the Keycloak user once that Change is on `main`, and clears the mark, leaving the person
disabled, when the Change is rejected. A person no manifest names is deleted at once: `204`.
An App's `spec.access` entry naming the e-mail then matches nobody, and the App page lists it.

`POST /api/v1/organization/people/remove` takes `{"ids": ["7d1f0c9e-…", …]}`, 1 to 200 Keycloak
user ids (a department leaving), and does what `DELETE` does for all of them at once: ONE Change
takes every one of their e-mails out of every `Group` and `RoleBinding`, and each person is
disabled, signed out and marked with that Change. When no manifest names any of them they are
deleted at once: `204`. Each person is held to the `DELETE` rules before anything is written: a
person holding a right the caller lacks is `403` naming them, the caller, the last Organization
Administrator, or every Organization Administrator together in the list is `409`, an unknown id is `404` naming it, a person whose removal is
already open is `409` naming it, and no id or more than 200 is `422`. The Change's title and
description name the ids, never the e-mails. Each person gets their own `person.changed` event
(T-5606).

- Every route needs a binding at organization scope whose role grants the verb on `Person`
  (PF-91): `read` for the two `GET`s, `create` for `POST`, `update` for `PATCH` and
  `reset-password`, `disable` for `disable`, `enable`, `remove-second-factor` and `sign-out`, and
  `delete` for `DELETE` and `remove`. Anyone else gets `403` naming the verb.
- A person who holds a right the caller does not hold (an `org-admin`, for a caller who is only
  `people-admin`) is refused every write above with `403` naming that right, so a reset's temporary
  password never opens a stronger account; disabling or deleting yourself, or the last Organization
  Administrator, is `409` (PF-93, PF-03).
- An e-mail another person already has is `409` naming it. A malformed e-mail, a name longer than
  255 characters, a locale outside the Portal's languages or an unknown field is `400`.
- An `{id}` the realm does not know is `404`.
- Without the admin client every route answers `503`.
- Every action writes one `person.changed` event of the project `org` to the activity feed (§14):
  who, what, and the person's id, never an e-mail body, a password or a token (PF-90).
- The registry offers the same actions as operations (§21, AG-77, T-2732): `jc_person_list`,
  `jc_person_get`, `jc_person_create`, `jc_person_edit`, `jc_person_disable`, `jc_person_enable`
  and `jc_person_sign_out`. Each calls its route's function, so the check and the answers are the
  route's. `jc_person_create` never answers a temporary password: when the realm sends no e-mail
  its answer carries `handOver`, which says that a person gives one with `reset-password` on the
  person's page. Resetting a password, removing a second factor and deleting a person have no
  operation: the first two hand over or take away a way in, and a deletion is a Change a person
  proposes on the page (PF-92, PF-94).

## 25. Organization setup (PF-90, UI-82)

What a new organization still lacks, in one answer, for the page that walks an Organization
Administrator through it (`/organization/setup`, T-2748). The Portal reads it from what it already
holds and writes nothing: every step links to the page that proposes that change the normal way.

```text
GET    /api/v1/organization/setup                           the steps and the operator's part → 200
```

```json
{
  "complete": false,
  "steps": [
    { "id": "organization", "done": true },
    { "id": "domain", "done": false },
    { "id": "people", "done": false },
    { "id": "project", "done": true },
    { "id": "publishers", "done": false },
    { "id": "policies", "done": false }
  ],
  "operator": [
    { "id": "branding", "done": true },
    { "id": "loginTheme", "done": true },
    { "id": "smtp", "done": false },
    { "id": "backups", "done": false }
  ]
}
```

- `steps`, in the order the page shows them, each `done` when:
  - `organization`: the `Organization` manifest names a domain and at least one locale;
  - `domain`: that domain is verified (PF-41);
  - `people`: the realm holds a second person besides the one reading, so the organization does
    not hang on one account; `done` is `false`, never an error, when no admin client is configured;
  - `project`: a project holds a `ContextSpace` that names a data model;
  - `publishers`: a `CkanInstance` is declared, the catalogue the projects publish to (EP-62);
  - `policies`: the `Organization` manifest sets `spec.projects` (ADR-N-035).
- `operator` is what the installation provides and the Portal cannot change: `branding` is `true`
  when `global.branding` names the installation (an `instanceName` other than `joinedcontext`, or a
  logo); `loginTheme`, `smtp` and `backups` are the deployment's own statements,
  `JC_SETUP_LOGIN_THEME`, `JC_SETUP_SMTP` and `JC_SETUP_BACKUPS`, rendered from the values that
  switch those on ([Deployment/13](../Deployment/13-configuration-reference.md)). An unset
  statement is `false`: the page never claims what nobody said.
- `complete` is `true` when every step and every operator item is done.
- The route needs an administrator of the organization, `approve` and `delete` on `RoleBinding` at organization scope, which `org-admin` holds (PF-03, PF-56);
  anyone else gets `403`.

## 26. Validation health (OPS-53)

The validation checks run outside the Portal, on their own schedules: deployment drift and the
supply chain, the conformance suites, the authorization matrix, the performance budgets, the
restore drill, the live sweep. Each writes a summary for `tasks/file-failures`, and
`joinedcontext-deployment/scripts/publish-health.py` turns that summary into one digest, the key
`{check}.json` of the ConfigMap `jc-validation-results`. The Portal mounts that ConfigMap
(optional, so an installation that publishes nothing still starts), names the directory in
`JC_HEALTH_DIR`, and reads it on every request:

```text
GET /api/v1/organization/health     every published check with its state → 200
```

```json
{
  "checks": [
    {
      "check": "deployment",
      "state": "red",
      "result": {
        "check": "deployment",
        "at": "2026-09-25T08:00:00Z",
        "everyHours": 1,
        "run": "dev-validate 2026-09-25T08:00:00Z",
        "counts": { "pass": 41, "fail": 1, "error": 0, "skip": 3 },
        "failures": [
          { "key": "images/portal", "verdict": "fail", "title": "portal runs an unsigned image", "task": "T-2901" }
        ],
        "history": [
          { "at": "2026-09-25T07:00:00Z", "pass": 42, "fail": 0, "error": 0, "skip": 3 }
        ]
      }
    }
  ]
}
```

- A digest is what the publisher writes and nothing more: the check's name, when it ran, how
  often it runs, the run's label, the verdict counts, at most 50 failing or erroring results
  (`key`, `verdict`, `title`, and the open task that `check: {check}/{key}` names, when there is
  one) and at most 200 history points of the last seven days. It never carries a result's
  `detail` or `evidence`: those stay in the summary, beside the task. A check whose passing keys
  a page shows, as `apps` does, also lists them in `passed` (at most 500, each at most 300
  characters); every other check leaves it empty.
- `result` is the digest the publisher wrote. `state` is the Portal's: `stale` when the last run
  is older than twice `everyHours` (1 to 744, a month), whatever it found; otherwise `red` when
  it has a `fail` or an `error` and `green` when not. A file that is not such a digest (larger
  than 256 KiB, an unknown field, more than 50 failures or 200 points, a text over 300
  characters, a task that is not a task id, a `check` that differs from its file name) is
  `unreadable` and has no `result`. Rows come in the order of their names.
- Only an administrator of the organization reads it: a caller whose bindings at organization
  scope grant `approve` and `delete` on `RoleBinding` (PF-03). Anyone else signed in gets `403`,
  nobody signed in `401`.
- Without `JC_HEALTH_DIR`, or before anything is published, the answer is `{"checks": []}`.

### The chip of an App (AP-136)

The probe publishes the check `apps`, one key `{project}/{name}` per published App. The Apps
list and the App's page read one project's share of it:

```text
GET /api/v1/projects/{project}/app-checks     the last check of each App of the project → 200
```

```json
{
  "checks": [
    { "name": "air-quality", "state": "green", "at": "2026-09-25T09:00:00Z" },
    { "name": "helsinki-alerts", "state": "red", "at": "2026-09-25T09:00:00Z", "reason": "no row read in 60 s" }
  ]
}
```

- `state` is `green` for a key in `passed`, `red` for a key in `failures` (`reason` is its title),
  and `amber` for either when the digest is stale; an App the last run did not check has no
  row. An unreadable or absent digest answers `{"checks": []}`.
- Whoever reads `App` in the project gets the rows; a project the caller may not read is `404`,
  a caller without `read` on `App` gets `403`, nobody signed in `401`.

## 27. Data quality of a space (DM-74)

Once a day the leading Portal replica reads every entity of every space that names a model, as
its own client through the space surface, and holds it to the model the way a pipeline's
validation stage does (PL-59). The last result stays in memory until the next run replaces it;
a run that fails keeps the one before.

```text
GET /api/v1/projects/{project}/spaces/{space}/quality     the last run's report → 200
```

```json
{
  "observedAt": "2026-09-25T02:00:00Z",
  "checked": 1200,
  "invalid": 16,
  "truncated": false,
  "rules": [
    { "rule": "sh:minCount", "path": "name", "count": 12, "examples": ["urn:ngsi-ld:BikeStation:hel.fi:bikes:001"] }
  ],
  "freshness": [
    { "pipeline": "bikes-feed", "type": "BikeHireDockingStation", "newest": "2026-09-25T01:58:00Z", "targetSeconds": 600, "state": "fresh", "paused": false }
  ],
  "types": [
    {
      "type": "BikeHireDockingStation", "count": 1200, "newest": "2026-09-25T01:58:00Z",
      "attributes": [
        { "name": "totalSlotNumber", "present": 1200, "min": 2, "max": 400, "outliers": 1, "outlierExamples": ["urn:ngsi-ld:BikeHireDockingStation:hel.fi:bikes:031"] },
        { "name": "name", "present": 1188, "outliers": 0, "outlierExamples": [] }
      ]
    }
  ]
}
```

- Before the first run, and for a space that names no model, the answer is `{}`: no
  `observedAt`, which the page reads as "not checked yet", never as "all valid".
- `invalid` counts entities with at least one problem; `rules` counts problems by the SHACL
  component and the path (`type` for a class the model does not declare, `id` for PF-42), most
  frequent first. `truncated` is `true` when the space held more than the 20,000 entities a run
  reads.
- `freshness` has one row per pipeline whose output Endpoint writes into the space. `type` is the
  pipeline's output type, empty when it names none (then the whole space counts). `targetSeconds`
  is the pipeline's interval (its `period`, or what its cron `schedule` implies) plus the larger
  of a twelfth of it and 600: 615 for a `15s` feed, 93600 (26 hours) for a daily run. It is
  `null` for a pipeline its source drives or whose schedule this reading does not understand,
  and that row is `untargeted`. `state` is `empty`
  when the space holds no entity of the type, `stale` when the newest is older than the target,
  `fresh` otherwise.
- `types` has one row per entity type the run read (T-3252): `count` entities, the `newest`
  `modifiedAt`, and every attribute an entity of the type carries with `present`, the entities
  that carry it (completeness is `present / count`), most complete first. A numeric attribute also
  has `min` and `max` and `outliers`, the values outside the Tukey fences (1.5 interquartile
  ranges below the first or above the third quartile), counted only from eight numbers up, with at
  most five `outlierExamples` ids.
- A project the caller may not read, or a space they may not read, is `404`. `examples`,
  `outlierExamples`, `min` and `max` are absent or empty for a caller without `read` on `Entity`
  in the space; the counts are the same for everyone who reads the space.

### 27.1 Checks on write (DM-94)

Every write through an Endpoint is held to the space's `spec.quality` rules, and the gateway
counts each check in `jc_quality_checks_total{space,type,rule,outcome}` (ADR-N-046 2.7). Every
Portal replica reads the gateway's counters once an hour and on a request at most once a minute,
and keeps the readings for eight days; a window's count is what the counters grew by inside it.

```text
GET /api/v1/projects/{project}/spaces/{space}/quality/checks     the space's checks → 200
```

```json
{
  "at": "2026-10-12T08:00:00Z",
  "windows": { "24h": "2026-10-11T08:00:00Z", "7d": "2026-10-10T09:00:00Z" },
  "rules": [
    {
      "type": "AirQualityObserved", "rule": "pm10-plausible", "attribute": "pm10",
      "24h": { "pass": 1180, "reject": 0, "flag": 20, "quarantine": 0, "passRate": 0.9833 },
      "7d": { "pass": 8200, "reject": 0, "flag": 140, "quarantine": 0, "passRate": 0.9832 },
      "lastRejected": null
    }
  ]
}
```

- `at` is the newest reading, `null` while there is none (no gateway address, or it never
  answered); `rules` is then empty and the page says the checks are unknown, never "all pass".
- `windows` names where each window starts: the oldest reading inside it, later than 24 hours or
  7 days ago while the replica has not held its readings that long. A counter that went down
  between two readings (the gateway restarted) counts from zero.
- `rules` has one row per rule of the space's `spec.quality` and per type and rule the counters
  name, ordered by type and rule. `attribute` comes from the manifest, empty for a rule no longer
  declared. `passRate` is `pass` over all four, `null` when nothing was checked in the window.
- `lastRejected` is the reading at which the rule's `reject` counter last grew, `null` when it
  did not in seven days: the latest rejected write by rule and attribute, never its value.
- The retention deletions sit beside the checks (DM-85, DM-96): `GET /api/v1/projects/{project}/spaces/{space}/retention/deleted`
  answers what the broker's sweeps deleted of the space since the broker started, per class and
  kind (`entity`, `history`), and `POST …/retention/preview` with a draft `retention` answers
  what a shorter one would delete now; `503` when the broker does not answer.
- Whoever reads the space reads its checks; a project or space the caller may not read is `404`.

### 27.2 A dataset's quality score (T-5112)

The score of the dataset an Endpoint serves is a number from 0 to 100, the mean of four parts.
A part nothing measures yet has no `score` and stays out of the mean.

- `freshness`: each type a pipeline promises a schedule for, as the daily run (§27) found it,
  and each writing pipeline as the health log judged it at its last beat: a pipeline whose `runs`
  check is not ok makes its types late within one beat, not at the next daily run.
- `completeness`: the mean share of entities carrying each required field of the model; a field
  the Endpoint's `spec.projection.hiddenAttributes` hides is not counted.
- `validity`: the entities that follow the model over those the daily run checked.
- `links`: the Endpoint's surfaces as the health probes answered; a slow one counts half.

```text
GET /api/v1/projects/{project}/endpoints/{name}/quality     the score → 200, 404
```

```json
{
  "score": 72,
  "measuredAt": "2026-10-22T08:04:00Z",
  "parts": [
    {
      "part": "freshness", "score": 0, "measuredAt": "2026-10-22T08:04:00Z",
      "reason": { "code": "late", "args": { "late": "1", "total": "1", "newest": "2026-10-21T02:50:00Z" }, "text": "1 of 1 feeds are late; the newest observation is from 2026-10-21T02:50:00Z" },
      "fix": { "code": "fixFailing", "args": { "pipeline": "weather", "since": "2026-10-22T08:00:00Z" }, "text": "Pipeline weather fails since 2026-10-22T08:00:00Z: open it and fix the step its newest run names" }
    }
  ]
}
```

- The route answers whoever reads the project and this Endpoint in its Context Space (a grant
  scoped to another space gets 404), 404 otherwise. `fix` comes only
  to a caller who reads the Endpoint's Context Space; it names the pipeline, field, rule or surface
  to change.
- `GET /api/v1/catalogue/datasets/{name}` carries the same score as `quality` in its public form:
  counts and dates, no `fix`, no field, type, rule or pipeline name. A score never holds a value.

## 28. Organization limits (PF-96…PF-102, ADR-N-035)

What Organization settings shows for every policy and limit (PF-102): the catalog of ADR-N-035
with the bound the operator's file sets on each entry (PF-97), the value in force and where it
comes from (PF-99, PF-101), and the quota of each project with what it uses (PF-73, PF-75).

```text
GET /api/v1/organization/limits     the catalog, its bounds and the values in force → 200
```

```json
{
  "entries": [
    {
      "path": "spec.projects.quota.contextSpaces",
      "section": "projects",
      "security": false,
      "default": null,
      "min": 0,
      "max": 20,
      "value": 10,
      "origin": "organization"
    },
    {
      "path": "spec.limits.signIn.sessionIdleMinutes",
      "section": "signIn",
      "security": true,
      "default": 60,
      "min": 5,
      "max": 120,
      "value": null,
      "origin": "default"
    }
  ],
  "projects": [
    {
      "project": "helsinki",
      "origin": "project",
      "quota": {
        "contextSpaces": { "limit": 12, "used": 3 },
        "agentRunsPerDay": { "limit": null, "used": null }
      }
    }
  ]
}
```

- `entries` lists every numeric entry of the catalog in its order. `section` is one of
  `projects`, `applications`, `edge`, `signIn`, `people`, `agents`, `pipelinesAndData`, `mqtt`.
  `security` is `true` for an entry whose bound the operator may only tighten.
- `min` and `max` are the range an organization may set: the operator's bound where
  `portal.organizationBounds` sets one, else the catalog's own. `max` is `null` where neither
  sets a ceiling.
- `value` is what the Organization manifest sets, `null` when it sets nothing. `origin` is then
  `organization` or `default`, and `default` is the catalog's default, `null` for no limit.
- `projects` has one row per project the caller may read. `origin` says whose quota is in force:
  `project` (the Project's own `spec.quotas`), `organization` (`spec.projects.quota`) or
  `default` (neither, so no limit). `quota` holds each dimension: `limit` is `null` for no
  limit, and `used` is the manifest count for the four countable dimensions (PF-75), `null` for
  a limit enforced at run time.
- The MQTT allowance (PF-117, ADR-N-048 §2.9) is in `entries` under section `mqtt`, one entry
  per number (`spec.limits.mqtt.brokers`, `connections`, `messagesPerSecond`,
  `maxMessageBytes`); `enabled` and the lists are read from §47 per project. A project row's
  `quota` carries `mqttBrokers`, `mqttConnections` and `mqttMessagesPerSecond` with `limit` and
  `used`, and `origin` `project` when its `spec.quotas.mqtt` is set.
- Any signed-in person reads the route: the catalog and the bounds are the installation's, not
  a project's. A project the caller may not read is left out of `projects`, so nothing of its
  size is said (R20).

## 29. Size of a space (T-2889)

How much a space holds, read from the broker that holds it: the entity count of the space's
tenant, asked with the conformant NGSI-LD count query every broker serves (CIM 009 6.3.13,
T-2996): `GET /ngsi-ld/v1/entities?local=true&count=true&limit=0` with `NGSILD-Tenant: {space}`,
read from `NGSILD-Results-Count`. `local=true` counts every type of the tenant's own entities and
none of a registered source's. A `404` whose problem type is `NonexistentTenant` is a space never
written to (`0`); any other `404`, an answer without a readable count, or a broker that does not
answer is `503` with the reason, never a zero. The Portal keeps each answer: these are dashboard
numbers, not per-request work. A space under 10 000 rows (entities and history together) is
measured again on a read after five minutes; a larger one is kept for an hour, and the Portal's
health sweep measures every space within that hour (T-6208).

```text
GET  /api/v1/projects/{project}/spaces/{space}/usage               the kept count and size → 200
POST /api/v1/projects/{project}/spaces/{space}/usage/recalculate   measure the space again → 200
```

```json
{ "entities": 14232, "attrInstances": 90311, "estimated": false,
  "broker": "main", "observedAt": "2026-09-25T11:40:00Z", "measuredAt": "2026-09-25T11:38:12Z",
  "bytes": { "entities": 48211968, "attrInstances": 1203456,
             "byType": { "https://smartdatamodels.org/dataModel.Environment/AirQualityObserved": 48211968 } } }
```

- A space the broker has no tenant for yet holds nothing: `entities` is `0`.
- `broker` names the `ContextBroker` the space lives on (PF-111, §43); the count is asked of that
  broker. A space whose `brokerRef` names no broker, or whose broker has no address, is `503`
  naming the broker or the `brokerRef`, never counted on `main`.
- Without a broker address (`JC_PORTAL_BROKER_URL`), or while the broker does not answer, the
  answer is `503` with the reason. The page shows the reason, never a zero.
- A project or a space the caller may not read is `404`, as in §27. The count is the space's,
  the same for everyone who reads the space; which entities they are is the entity read's.
- The per-type breakdown on the space's page is the gateway's own `count=true` per type, read
  with the person's token, so it narrows as the person's grants do.
- `bytes` is the storage the space takes, measured by the broker in its store (`GET
  /q/tenants/{tenant}` `bytes`, T-2890): `entities` the current state,
  `attrInstances` the history, `byType` the current state by each Entity's first type as the
  broker stores it (an expanded IRI), summing to `entities`. Like the count it is the space's,
  the same for everyone who reads the space; the type names are the space's DataModels, which the
  reader already sees. A broker that cannot measure leaves `bytes` out, and so does a space it has
  no tenant for yet; the page then shows no size, never a zero.
- `attrInstances` is the space's history, its temporal attribute instances, as the broker counts
  them (`counts.attrInstances`, T-6085); left out when the broker neither counted nor estimated
  them, never a zero. The broker counts and sizes the history exactly within one second; a longer
  history it estimates from its database's statistics, and `estimated` is `true`: then
  `attrInstances` and `bytes.attrInstances` are estimates, and the page says so. The space's page
  shows the entity count, the history count, the current-state and history sizes and their total.
- `measuredAt` is when the broker measured the size: the broker keeps a measured size for five
  minutes (Antares `TENANT_BYTES_TTL`), so it can be older than `observedAt`, when the broker
  answered. A broker that sends no `measuredAt` gives `observedAt`.
- `recalculate` measures the space again for anyone who may read it (`404` otherwise, as the
  read). Concurrent asks of one space share one broker read, and an answer under 30 seconds old
  stands, so repeated clicks cost the broker nothing. It answers the same body as the read.
- The Context Spaces list shows the count and size with an (i) beside them: its panel says when
  the size was measured and holds the Recalculate button.
- The Context Spaces list and the space's page show the size in binary units (KiB, MiB, GiB) with
  the locale's separators and the exact bytes in the tooltip; the space's per-type table adds the
  size of each type beside its count. The list still sorts by the entity count.

## 30. Data views of a space (ADR-N-042, T-3104, T-3099)

A saved way of looking at one entity type of a space: which attributes show, the filter, the
order, the grouping and the colours, and the view kind (grid, gallery, kanban, calendar, timeline,
map, form). The view is the Portal's own record in its database (`data_views`); the rows it shows are
the space's entities, read with the person's own session, so a view narrows what the person may
read and never widens it (ADR-N-042 §3.2).

```text
GET    /api/v1/projects/{project}/spaces/{space}/views          the views this caller sees → 200
POST   /api/v1/projects/{project}/spaces/{space}/views          save a new one → 201
PUT    /api/v1/projects/{project}/spaces/{space}/views/{id}     rename it, change its mode or settings → 200
DELETE /api/v1/projects/{project}/spaces/{space}/views/{id}     delete it → 204
```

```json
{
  "id": "7f1c2a9e-4b1d-4a57-9a0e-2f6d1c3b8e01",
  "type": "BikeHireDockingStation",
  "kind": "grid",
  "mode": "collaborative",
  "title": "Stations short of bikes",
  "owner": "jana.kovacova",
  "config": {
    "q": "availableBikeNumber<3",
    "sort": [{ "attr": "availableBikeNumber", "desc": false }],
    "group": "status",
    "hidden": ["dateLastReported"],
    "width": { "name": 240 },
    "order": ["name", "availableBikeNumber"],
    "labels": { "availableBikeNumber": "Bikes" },
    "colour": [{ "when": "availableBikeNumber==0", "colour": "danger" }],
    "settings": {}
  },
  "version": 3,
  "createdAt": "2026-10-06T19:00:00Z",
  "updatedAt": "2026-10-06T19:05:00Z"
}
```

- The body of `POST` is `type`, `kind`, `mode`, `title` and `config`; of `PUT` the same without
  `type`, plus `expectedVersion`. Unknown keys are `400`. `type` is an NGSI-LD type name
  (`^[A-Za-z][A-Za-z0-9_-]*$`), `kind` one of `grid`, `gallery`, `kanban`, `calendar`, `timeline`,
  `map` (the rows placed by a GeoProperty, T-4884), `form`, `mode` one of `personal`, `collaborative`, `locked`, `title` 1 to 120 characters.
- `config`: `q` is an NGSI-LD query of at most 4,096 characters, sent as the person's own `q`
  when the view opens; `sort` names at most 3 attributes; `hidden` and `width` name attributes,
  a width between 40 and 2,000 pixels; `order` lists at most 256 attributes in the order the grid
  shows them, the rest after them in the display order (DM-98); `labels` gives at most 256
  attributes a label of the view's own, 1 to 80 characters without control characters, shown as
  text and never written to the model (T-4827);
  `colour` holds at most 20 rules, each a `q` and one of the colour tokens `neutral`, `info`,
  `success`, `warning`, `danger`, evaluated in the page on the rows it shows (terms joined by `;`
  and `|`, without parentheses; a rule the page cannot evaluate marks nothing and the editor says
  so), the first matching rule marking the row with a swatch that names it; `settings` is the
  kind's own (the card fields of a gallery, the attribute a kanban groups by, the date attributes
  of a calendar or a timeline, the fields of a form). The whole `config` is at most 64 KiB.
- A form's `settings` (T-3103): `fields`, the attributes it asks for in their order, each
  `{attr, label?, help?, required?}`, where a missing `label` or `help` is the model slot's own and
  `required` can only add to what the model requires; `conditions`, each `{attr, when: {attr,
  equals}}`, which shows `attr` only while the answer to the other field equals the value (a hidden
  field is not sent); `prefill: true` takes `?{attr}=value` from the page's URL for the fields it
  names, never for an attribute the form does not ask for. A submission creates one entity through
  the space surface with the person's session, under `urn:ngsi-ld:{type}:{uuid}` (ADR-N-041), so the
  Policy decides it like any write.
- Who sees and changes a view, by its `mode`: `personal` its owner alone, and to anyone else it
  does not exist (`404`); `collaborative` everyone who may read the space sees and changes it;
  `locked` everyone who may read the space sees it, and only its owner or a steward of the space
  (a caller with `update` on the `ContextSpace`) changes or deletes it (`403` for others).
  Deleting a `collaborative` view, or changing any view's `mode`, is its owner's or a steward's:
  a collaborator who made a shared view personal would take it from everyone else.
- A project or a space the caller may not read is `404`, as in §27. Saving needs no more than
  reading: a view changes nothing in the space.
- `version` counts up on every save. Send the version you read as `expectedVersion`: a save
  against a newer view is `409` with the current version, never an overwrite of another window's
  change. Duplicating a view is a `POST` of its settings under a new title; renaming is a `PUT`.
- A person keeps at most 200 views per space; the 201st is `409` with the limit in the problem.
- A grid's `settings` (T-4883, T-5188): `filters`, the filter panel's condition tree; `rowSize`;
  `aggregations`, the footer's choice per attribute; `groupBy`, at most 3 different attribute
  names, outermost first (anything else is `422` naming `config.settings.groupBy`).
- `sort` holds up to 3 keys, sent to the space surface as one `orderBy` (`a;desc,b`, CIM 009 4.23)
  after the `groupBy` attributes, so the broker orders the whole filtered set and a hidden
  attribute orders as an absent one. Each group of `groupBy` gets a header in the grid with its
  count; `group`, kept as the first `groupBy` attribute where that is an enum, adds a row of
  value buttons above the grid. Every count, a group header's included, is the gateway's own
  `count=true` answer for `q` plus the group's values, read with the person's session, so the
  counts narrow as their grants do and are never the page's tally.
- The footer's minimum, maximum and median of a number and earliest and latest of a date are the
  rows the space surface returns first for `orderBy=attr` (or `attr;desc`) under `q` plus `attr`,
  the median at the middle offset (the mean of the two middle values for an even count). A sum,
  an average and a count of distinct values need an aggregate of current values the broker does
  not have, and the footer does not offer them.

## 31. Trash of a space's entities (ADR-N-042 §3.3, T-3107)

What a person deleted from a data view, kept for them for 30 days so they can put it back. The
entity itself is deleted from the space through the gateway with the person's session, as any
delete, and the Endpoint's Policy decides it; the Portal keeps only the person's own copy of what
the entity was, in its database (`entity_trash`), and restoring is the person's own create through
the gateway, refused like any write their grants no longer allow.

```text
GET    /api/v1/projects/{project}/spaces/{space}/trash               this caller's deleted entities, newest first → 200
POST   /api/v1/projects/{project}/spaces/{space}/trash               keep the copy of one entity about to be deleted → 201
DELETE /api/v1/projects/{project}/spaces/{space}/trash/{id}          forget a copy: restored, or the delete was refused → 204
```

```json
{
  "id": 42,
  "urn": "urn:ngsi-ld:BikeHireDockingStation:hel.fi:bikes:7",
  "type": "BikeHireDockingStation",
  "entity": { "id": "urn:ngsi-ld:BikeHireDockingStation:hel.fi:bikes:7", "type": "BikeHireDockingStation", "name": { "type": "Property", "value": "Station 7" } },
  "deletedAt": "2026-10-06T19:20:00Z",
  "expiresAt": "2026-11-05T19:20:00Z"
}
```

- The body of `POST` is `entity`: the NGSI-LD entity as the person read it, normalized, with `id`
  an NGSI-LD URN (PF-43) and `type` an NGSI-LD type name; at most 256 KiB. Unknown keys are `400`.
- The order is the client's: read the entity, `POST` its copy, delete it through the gateway; a
  delete the gateway refuses is followed by a `DELETE` of the copy. A restore is a create of
  `entity` through the gateway, then a `DELETE` of the copy.
- Each copy is its keeper's alone: another caller's `id` is `404`, and the list shows only the
  caller's own. A copy grants nothing: restoring it is the keeper's own write, so a copy of an
  entity the keeper never could write is refused at the gateway like any other create.
- A project or a space the caller may not read is `404`, as in §27. A person keeps at most 1,000
  copies per space; the oldest goes first. A copy older than 30 days is gone.

## 32. Live updates of a space's entities (ADR-N-042 §3.4, T-3105, T-4876)

An open data view hears when an entity of its type changes, from anyone and through any door,
without polling. The Portal holds one NGSI-LD subscription per space and type that a view has
open, created through the space surface with the Portal's own ServiceAccount, and tells the views
that show that project, space and type that it changed.

```text
GET  /api/v1/projects/{project}/spaces/{space}/live?type={type}    the changes of one type, as server-sent events → 200
POST /live-notify/{liveKey}                                        where the gateway delivers the subscription's notifications → 204
```

- `GET …/live` follows the space's read rule (`404` for a space the caller may not read) and needs
  `type`, an NGSI-LD type name (`400` otherwise). Each event is `changed` with `{"type": "…"}` and
  nothing else: no entity id, no attribute name, no value. The notification was read under the
  Portal's ServiceAccount, whose grants are not the reader's, so the Portal cannot tell which rows and
  attributes the reader's row and attribute grants cover. The view reads the rows again with the
  person's own session, which the gateway narrows to those grants; a keep-alive comment comes every
  20 seconds.
- Channels are keyed by project, space and type: a space of the same name in another project
  never reaches the view.
- The subscription watches `entities: [{type}]`, notifies with `attributes` only by name
  (`notification.format: keyValues` of `id`, `type` and the changed attributes is what the broker
  sends; the Portal reads none of it) and expires 24 hours after it was written; a view opened
  after half of that writes it again. Nothing else removes it.
- `POST /live-notify/{liveKey}` is outside `/api/v1`: the broker that calls it through the gateway holds
  no token. `liveKey` is an HMAC of the project, space and type under a secret only this Portal process holds
  (drawn from the CSPRNG at start, never stored or logged), so a key names one subscription and
  cannot be made up; it is compared with each watched pair's HMAC in constant time, and an unknown
  key is `404`. A body larger than 1 MiB
  is `413`. A forged notification can only make a view read again.
- The process that receives a notification passes it on to its own views. With several replicas
  the edge hands each notification to one of them, so a view on another replica hears it on that
  type's next change; the Portal runs one replica until the replicas share these events the way
  they share draft events.

## 33. Data views published as a public link (ADR-N-042 §3.5, T-3108)

Publishing a view of a space proposes a public Endpoint narrowed to the view: one entity type,
the attributes the view shows (the rest are the Endpoint's `hiddenAttributes`), read operations
only. The manifests are what `POST …/assistant/propose-endpoint` renders (§19) with
`audience: public`, `entityTypes: [type]` and `hiddenAttributes`, proposed as one Change with the
Policy it needs, reviewed like every Endpoint. Once merged and applied, the view is a page:

The page `/v/{slug}` (`slug` is the Endpoint's) is the published view: read-only, no sign-in,
served by the Portal's interface like its other pages, not an API route.

- The page holds no data: every read it makes is an anonymous read of the Endpoint's NGSI-LD
  surface (`/api/endpoint/{slug}/ngsi-ld/v1/…`), so the gateway enforces what the link shows and
  rate-limits it as it does every anonymous read. The page shows the Endpoint's types, a type's
  first 100 entities in key-value form, and the attributes they carry.
- Revoking the link is deleting the Endpoint, a Change like its creation; the page then says the
  view is not published.
- A form view publishes the same way with `access: "create"` (§19): the Endpoint's Policy grants the
  `public` role `createEntity` on the one type, and the link is the page `/f/{slug}`. The page asks
  for the type's attributes the Endpoint's published schema lists, required where the schema
  requires them, labelled and helped from the schema, prefilled from `?{attr}=value`, and its submit
  is one anonymous `POST /api/endpoint/{slug}/ngsi-ld/v1/entities`: the gateway decides it and
  rate-limits it as every anonymous call. The Policy grants only the form's fields, the gateway
  mints the entity's id and stops at the form's daily count (EP-97); the page carries a field a
  person never sees and a script fills, sent as an attribute the Policy does not grant, so such a
  submission is refused; and the page says that what is sent is public data of the space. The form view's own labels and conditions stay with the
  signed-in form for now; the public page reads nothing from the Portal's database.
- The public form page sends no credentials on any request, its reads included: a signed-in
  person, a visitor and a site that frames it all get the anonymous page (EP-102).
- Embedding a form (EP-101): the sites named in the Endpoint's `spec.creates.embedOrigins` may
  frame `/f/{slug}`, which the Portal answers with `Content-Security-Policy: frame-ancestors 'self'
  <origins>` and, while the list holds one, no `X-Frame-Options`; with no list only the Portal
  frames it. The share panel gives two snippets: an `<iframe>` of the page, and
  `<script src="https://{portal}/f/embed.js" data-jc-form="{slug}" async>`, which inserts the same
  frame and sizes it to the form from the page's `{"type": "jc-form-height", "height": n}`
  message (the only message the page sends, to its parent).
- Testing a form (EP-102): `/f/{slug}?test=1` is the same page whose submit writes nothing. It
  checks the entry against the published schema, then asks
  `POST /api/endpoint/{slug}/access/check` `{"action": {"name": "createEntity"}, "resource": {"type":
  …}}` anonymously, and says whether the gateway would take it. The share panel previews the page
  this way in a frame of its own (`sandbox="allow-scripts allow-forms allow-same-origin"`), so a
  steward sees what a visitor sees and can try a submission without filling the space.
- Not yet: a password on a link needs the `data_views` record of §30 to hold its hash. Until then
  a published view is public to everyone; a view other than a form is not framed by other sites.

## 34. Knowledge assistant administration (T-3057, MF-51, MF-52)

A project's `KnowledgeSource` and `AssistantDeployment` manifests are resources like every other
(`/api/v1/projects/{project}/knowledgesources`, `…/assistantdeployments`), proposed as Changes; a
public, `ckan` or `iframe` deployment is a Red change (Architecture/06 §4). What a source holds
once crawled lives in `jc-assistant`'s database, and these routes read and steer it. The Portal
checks the caller's permission on the project and asks `jc-assistant` with its own service
token ([API/05 §3](05-knowledge-assistant.md#3-administration-for-the-portal)).

```text
GET  /api/v1/projects/{project}/knowledge/sources                          every source with what it holds
GET  /api/v1/projects/{project}/knowledge/sources/{source}/pages?parent=   one level of the page tree; no parent: the roots
GET  /api/v1/projects/{project}/knowledge/sources/{source}/pages?q=        pages whose title or address holds q, at most 100
GET  /api/v1/projects/{project}/knowledge/sources/{source}/search?q=&limit= the hits the MCP's search_documents would answer
GET  /api/v1/projects/{project}/knowledge/sources/{source}/documents       the documents (PDFs) the pages link
GET  /api/v1/projects/{project}/knowledge/sources/{source}/pages/{page}/links
GET  /api/v1/projects/{project}/knowledge/sources/{source}/passages?page=|document=
POST /api/v1/projects/{project}/knowledge/sources/{source}/inclusion       include or exclude pages, subtrees, documents
POST /api/v1/projects/{project}/knowledge/sources/{source}/recrawl         queue a crawl now
GET  /api/v1/projects/{project}/knowledge/sources/{source}/errors          the pages and documents the last crawls could not read
GET  /api/v1/projects/{project}/knowledge/deployments/{deployment}/usage   requests and tokens per day, 30 days
POST /api/v1/projects/{project}/knowledge/deployments/{deployment}/chat    ask the assistant as the signed-in person
DELETE /api/v1/projects/{project}/knowledge/deployments/{deployment}/answers  forget the answers the deployment keeps (T-4731)
GET  /api/v1/projects/{project}/knowledge/conversations                    stored conversations, filtered (API/05 §3.1)
GET  /api/v1/projects/{project}/knowledge/conversations/usage               the usage summary of the filtered turns (API/05 §3.1)
GET  /api/v1/projects/{project}/knowledge/conversations/export              the filtered turns as CSV (API/05 §3.1)
GET  /api/v1/projects/{project}/knowledge/conversations/{conversation}     one conversation, every turn with its steps
GET  /api/v1/projects/{project}/knowledge/conversations/{conversation}/turns/{turn}/same  stored turns of the same question
DELETE /api/v1/projects/{project}/knowledge/conversations/{conversation}   erase one conversation
```

- The read routes answer only a caller who may read `KnowledgeSource` in the project (the usage
  route `AssistantDeployment`), anyone else `404` like every project route; `inclusion` and
  `recrawl` need `propose` on `KnowledgeSource` and answer `403` naming it. Nothing reaches
  `jc-assistant` for a refused caller (AG-113).
- `inclusion` takes `{"pages": [id…], "documents": [id…], "subtree": bool, "included": bool}`, at
  most 500 ids. Excluding removes the passages of what it names at once, so no answer cites it
  again; the choice holds over every later crawl, whatever the source's include and exclude
  patterns say. Including again indexes it at the next crawl. The answer counts what changed.
- `chat` answers a caller who may read `AssistantDeployment` in the project, anyone else `404`,
  with the Server-Sent Events of API/05 §1.3; the body is API/05 §1.1. The Portal passes the
  person's own access token on for an `internal` deployment's connectors and keeps no copy
  ([API/05 §1.7](05-knowledge-assistant.md#17-in-the-portal), AG-115).
- `recrawl` answers `202` with the queued job, or `409` when one is already queued or running.
- `DELETE …/answers` needs `approve` on `AssistantDeployment` (the assistant's administrator),
  anyone else `403` naming it, and answers `200` `{"forgotten": n}`: the deployment's kept answers
  are gone, so its next question of each is answered anew (API/05 §3, T-4731). Every call writes
  an audit record naming the caller, the project and the deployment. The Portal calls it itself,
  with no caller, when it applies a Change to a `Policy` or an `Endpoint` that a connector of the
  deployment reads, so no kept answer outlives the rule it was read under.
- The `conversations` routes answer only a caller with `approve` on `AssistantDeployment` in the
  project (the assistant's administrator; `org-admin` holds it), anyone else `403` naming it, and
  `DELETE` needs `delete` on `AssistantDeployment`. Every call writes an audit record naming the
  caller, the project and the conversation (or the filter of a list). The Portal page Asistent →
  Konverzácie is shown to those callers only, and "Kopírovať ako test" copies a turn as the JSON
  fixture of the assistant eval sets (T-4656, T-4670). A conversation of another project, or one
  that does not exist, answers `404`. A caller gets at most 60 calls a minute on these routes,
  then `429`; more than 20 `404`s of one caller in 5 minutes writes an `assistant.transcript`
  audit record with the action `probe` (T-4681).
- "Porovnať" opens two turns side by side, picked from "Rovnaká otázka" (the turns `…/same`
  answers); `?open={conversation}&compare={conversation}` (UUIDs only, never a token) opens one
  conversation and compares its last turn with the other's, the link the opendata eval writes per
  question, this run against the previous. The view shows a header line with each turn's steps,
  tool and model calls, time, price and cached share; the steps aligned by kind, each with its
  arguments, rows, time, tokens and price and the model's reasoning text; the changes between the
  tables, charts and maps each turn showed, named from their stored specs (kind, chart type,
  units, row count; the stored spec holds no data, so nothing is redrawn); and the two answers as
  a word diff. Every stored text is shown as React text, never markup: no stored HTML, markdown
  or link runs, bidi and zero-width characters show as visible markers, and a step shows its
  first 64 kB until "Zobraziť viac" (T-4681).
- The index status of a source is in its row of `sources`: `passages`, `embedded` and `pending`
  (passages not embedded yet), `failed` (pages and documents that could not be read), and the last
  job with its `startedAt` and `finishedAt`. `errors` lists at most 50 of the failed pages and
  documents, newest first, each `{"url", "kind": "page"|"document", "reason"}`: the URL and the
  HTTP status or network reason, never what a page holds. It needs `propose` on `KnowledgeSource`
  and answers `403` naming it, so a viewer sees the counts and not the list (T-4197).
- A source the manifests declare and `jc-assistant` has not crawled yet is listed with
  `state: "not-crawled"`; a page, a document or a source of another project is `404`.

## 35. Comments on a space's entities, mentions and notifications (ADR-N-042 §3.1, T-3106)

A person who may read a space comments on one of its entities and names colleagues with
`@identifier`. Comments and notifications are the Portal's records (`entity_comments`,
`notifications`), referring to the entity by `(project, space, urn)` (ADR-N-041); commenting
changes no data and needs no write grant, only a read of the space (§27).

```text
GET    /api/v1/projects/{project}/spaces/{space}/comments?urn={urn}   the comments on one entity, oldest first → 200
POST   /api/v1/projects/{project}/spaces/{space}/comments             comment on one entity → 201
DELETE /api/v1/projects/{project}/spaces/{space}/comments/{id}        remove one's own comment → 204
GET    /api/v1/notifications                                          the caller's notifications, newest first → 200
POST   /api/v1/notifications/{id}/read                                mark one of the caller's notifications read → 204
```

```json
{
  "id": 7,
  "urn": "urn:ngsi-ld:BikeHireDockingStation:hel.fi:bikes:7",
  "author": "demo.steward@hel.fi",
  "authorName": "Demo Steward",
  "text": "@demo.editor@example.org the count looks stale since Monday",
  "mentions": ["demo.editor@example.org"],
  "createdAt": "2026-10-06T19:20:00Z",
  "mine": true
}
```

- `POST` takes `urn` (an NGSI-LD URN, PF-43) and `text` (1 to 4,000 characters); a URN or text
  outside that is `400`, an unknown key `422`. A mention is `@` after the start or a character that
  is not part of a word, followed by a person's identifier as the RoleBindings and Groups name
  them (`demo.editor@example.org`). A mention counts only for a person a binding in force lets read the
  space, by name or through a `Group`'s members, exactly as §27 decides it for a caller; each such
  person gets one notification, the author none. Mentions of anyone else are kept in the text and
  listed in the answer's `unknownMentions`, notified to nobody.
- A comment is its author's: only they `DELETE` it (another caller's `id` is `404`); nobody edits
  one. Removing a comment removes its notifications.
- A notification names the space, the URN, the comment, its author and the first 200 characters;
  `GET /notifications` answers the caller's own (matched by their username and their e-mail, lower
  case), at most 100, newest first, with `unread` counted. The Portal sends no e-mail of its own.
- Each notification has a `kind`: `mention` (above), or one of three connector events (AG-150,
  T-5979), which carry `link`, a Portal path to act on it, and no `commentId`:
  - `change.opened`: a change a connector opened with `jc_change_apply` waits for approval. Every
    person who holds `approve` in the project (PF-50) learns of it, except the person who opened
    it; `urn` is the change id, `link` its approval page `/projects/{project}/approvals/{id}`.
  - `connector.pending`: a connector request above the ceiling waits in the queue. Every person who
    decides such requests (an organization administrator, or `approve` on DelegationPolicy) learns
    of it, except the owner; `link` is `/ai-connectors`.
  - `connector.expiring`: a live connector of the caller's ends within 7 days; a new `expiresAt`
    is a new notification. `link` is `/ai-connectors`.

  The text a connector event carries is the person's name, the connector's name, the project and
  the change or connector id: never a manifest, a scope or a secret. One event is one notification
  per recipient. The approvers and deciders are judged when they read `GET /notifications`, by
  their rights at that moment, so a person who lost the right gets no new notification of it.
- A project or a space the caller may not read is `404`, as in §27. Listing or adding the
  comments of an entity first reads that entity on the space surface
  (`/cs/{space}/ngsi-ld/v1/entities/{urn}`) with the caller's own token: an entity their Policy
  hides, or one that is not there, is the same `404`, so a comment shows nobody what their
  Policy hides and still says nothing about whether an entity exists. A request that carries no
  token of the caller's (a Portal cookie session) is `404` with that reason (T-3284). A mention
  is the author's own disclosure to a reader of the space, as an e-mail would be. At most 1,000
  comments per entity.

## 36. A type's attributes in a space's data model (T-3223, DM-61, PL-59)

What a pipeline editor's output node maps a record's fields onto: the attributes an entity type
has in the data model the space pins (`spec.dataModelRef`), read from the same compiled schema the
runner's validation stage checks every record against. Reading the space is all it needs; a person
who may not read it gets the space's 404 and learns nothing of its model.

```text
GET /api/v1/projects/{project}/spaces/{space}/types/{type}/attributes   the type and its attributes → 200
```

```json
{
  "model": "bb-air-quality",
  "version": "1.0.0",
  "type": "AirQualityObserved",
  "description": "One air-quality station in the city and the latest hourly means it reported.",
  "attributes": [
    { "name": "dateObserved", "kind": "Property", "valueType": "string", "format": "date-time", "required": true,
      "description": "The end of the latest hour a reading of the entity covers." },
    { "name": "pm10", "kind": "Property", "valueType": "number", "required": false, "minimum": 0,
      "description": "Particulate matter up to 10 µm.", "unit": { "code": "GQ", "ucum": "ug/m3" } },
    { "name": "refDevice", "kind": "Relationship", "valueType": "string", "required": false,
      "relationship": { "target": "Device", "many": false } }
  ]
}
```

- Required attributes come first, then by name; `id` and `type` are the entity's own and are not
  listed. `kind` is the NGSI-LD attribute kind the slot declares; `unit.code` is the UN/CEFACT code a
  quantity's `unitCode` carries; `values` lists a coded slot's permitted values; `relationship`
  names the type a Relationship points at.
- `404` with the reason for a type the model does not declare ("… is not a class of the space's data
  model …") and for a space that names no model this Portal compiled.
- The pipeline test (`POST /api/v1/projects/{project}/pipelines/test`, §7a) holds a pipeline's output
  to the same model: before the sample is read, an output whose type is not a class of the model its
  Endpoint's space pins is an error finding at `spec.output.type` (or `spec.outputs[i].type`); on the
  sample, every output record is checked per attribute (missing required `sh:minCount`, wrong type
  `sh:datatype`, an attribute the class does not declare `sh:closed`, a quantity in another unit
  `ngsi-ld:unitCode`). A red verdict holds the draft back from proposal in strict mode (PF-57); the
  draft itself is saved with the report.

## 37. Alerts a person chooses (T-3261, PL-71)

A person who may read a pipeline, or a space, chooses to be told when something goes wrong with
it. The Portal keeps the choice, opens one incident when a signal starts and closes it when the
signal clears, and leaves one notice per subscriber for each: no storm of repeats while it lasts.

```text
GET    /api/v1/alerts                       the caller's alert subscriptions → 200
PUT    /api/v1/alerts                       subscribe, or change the subscription of the same target → 200
DELETE /api/v1/alerts/{id}                  stop one of the caller's subscriptions → 204
POST   /api/v1/alerts/{id}/mute             mute one for a while, or unmute it → 200
GET    /api/v1/alerts/notices               the caller's notices, newest first → 200
POST   /api/v1/alerts/notices/{id}/read     mark one read → 204
```

```json
{
  "id": 3,
  "project": "helsinki",
  "scope": "space",
  "target": "bikes",
  "events": ["failure", "stale", "zero"],
  "delivery": "portal",
  "mutedUntil": null
}
```

- `scope` is `pipeline` (`target` the pipeline's name), `space` (the space's name: every pipeline
  whose output Endpoint writes into it) or `type` (`{space}/{Type}`: the pipelines writing that type
  there). `events` is one or more of `failure` (the pipeline's phase is `Error`), `zero` (its
  `StreamWriting` condition is `False`: it wrote nothing while erring, or stalled) and `stale` (the
  daily quality run found its newest entity older than the freshness target, DM-74). One
  subscription per caller and target: a second `PUT` changes it. An unknown key is `422`, an
  unknown scope or event `400`.
- `scope` `agent` (T-5301) names an agent profile (`target` its name) and is told of its runs in
  `project`, with the signals of §51 as `events`: `refusalSpike`, `spendShare` and
  `offHoursWrite`; an agent event under another scope, or a pipeline event under `agent`, is `400`
  naming `events`. An incident holds while its signal held within the last hour (a run counted
  while it is live or finished within the hour) and recovers after an hour without it; the notice's
  `pipeline` member carries the profile's name and its `detail` the count and the run's title.
  Subscribing needs approve in the project, since the signals count every person's runs (AG-93):
  `403` naming `project` otherwise, `404` naming `target` for a profile that does not exist. The
  Activity tab offers the projects from §51 `choices.alertProjects`; the notices list drops an
  agent notice once the caller no longer approves there.
- `delivery` is `portal` (a notice when the incident opens and one when it closes) or `digest` (one
  notice a day, after 07:00 UTC, naming the incidents opened in the last day). `email` is refused
  with `400` and its reason while the Portal has no mail relay (T-3261).
- Subscribing needs `read` on the pipeline (`pipeline`) or on the space (`space`, `type`); a target
  the caller may not read is `404`, as in §27. The notices list is filtered with the caller's grants
  when it is read, so a notice of a target they lost access to is not shown.
- `POST …/mute` takes `{ "for": "1h" | "1d" | "7d" | "forever" }`, or `{ "for": null }` to unmute; a
  muted subscription gets no notice until then. Every notice carries the `subscription` it came from,
  so the Portal offers "Mute" beside each one.
- A notice names the project, the pipeline, the event, whether it `opened` or `recovered`, and when;
  at most 200 per caller are kept, the oldest dropped first; `unread` is counted.

## 38. Feedback from any page (T-3272)

A person tells the platform's owners what got in their way, from the page where it happened. A
feedback is untrusted text: it reaches the work board as a proposed task, blocked until a person
has read it, and never as work an agent takes by itself.

```text
POST /api/v1/feedback                                  a signed-in person's feedback → 202 {"id": 7}
GET  /api/v1/organization/feedback?after=0             administrators: the feedback after an id, oldest first, at most 100
GET  /api/v1/organization/feedback/{id}/screenshot     administrators: its screenshot, image/png; 404 without one
```

```json
{ "text": "The Approve button stays grey after I type the name", "page": "/projects/helsinki/approvals/chg-0000001c",
  "screenshot": "data:image/png;base64,iVBORw0KGgo…" }
```

- `text` is 1 to 2 000 characters once trimmed. Before it is kept, e-mail addresses, phone
  numbers and words shaped like a credential are replaced by `[removed]`, so the task the board
  receives carries no personal data and no secret the person pasted by mistake.
- `page` is the Portal address the person was on. Its query and fragment are dropped, since
  they can carry a name or a filter value. It starts with `/` and is at most 500 characters.
  A part of the path holding anything but `A-Z a-z 0-9 _ . ~ % -`, or an e-mail address
  (`@`, `%40`), is kept as `_`: the page is one line wherever it is written, the board's task
  file included, and carries no address.
- `screenshot` is present only when the person ticked it: a PNG `data:` URL of at most 2 MB.
  The Portal paints over every input field of the page in the image before it is sent (a
  password, a token, a person's name typed into a form). The person also chooses what the
  browser shares.
- Nobody is named: the feedback keeps no author, no e-mail and no IP address. The Portal's
  version (`APP_VERSION`) is attached by the server. A person sends at most 10 an hour, and the
  11th is `429` with `Retry-After`.
- An unknown member is `400`; a page that does not start with `/`, an empty or oversized text,
  or a screenshot that is not a PNG is `400` naming the field.
- The list is for administrators of the organization (PF-03), `403` for anyone else, and each
  item is `{id, createdAt, page, version, text, screenshot}`, `screenshot` a boolean.
  `scripts/feedback-to-tasks.mjs` in the Portal repository turns one page of it into task files,
  `status: blocked` and `group: feedback`, each saying the text is untrusted input to read first.

## 39. Trying a Policy on a person (T-3311, EP-103)

An organization administrator picks a person, a group, a role, a service account or the public,
one Endpoint, an action and optionally a type, and reads what the gateway would decide and which
Policy decided it. The Portal does not evaluate anything itself: it resolves the chosen subject and
asks the gateway's `access/simulate` ([API/02 §7b](02-endpoint-representations.md)), which runs the
same admission and evaluator as every real call.

```text
POST /api/v1/projects/{project}/endpoints/{name}/access/simulate   → 200
```

```json
{"subject": {"kind": "person", "id": "8c0e…"},
 "action": "retrieveEntity", "type": "AirQualityObserved"}
```

- `subject.kind` is `person` (`id`: the Keycloak user id), `group` or `role` (`name`: a member of
  it with no person of their own), `serviceAccount` (`name`: a ServiceAccount of the project) or
  `public`. For a person the Portal reads their groups and realm roles through the Keycloak admin
  API it uses for People (PF-108), the same names their token carries.
- `action` is a CIM 009 operation name; `type` is optional. An unknown member, an unknown kind, or
  an action that is not an operation is `400` naming it; an unknown Endpoint or person is `404`.
- The answer is the gateway's, with the subject as resolved:
  `{"decision", "reason", "policy"?, "assigner"?, "subject": {"user"?, "groups", "roles",
  "serviceAccount"?}}`, `reason` one of `policy_grant_matched`, `prohibited`, `no_grant`,
  `not_admitted`.
- For organization administrators only (`403` for anyone else): it reveals what another person may
  do. The Portal calls the gateway with its own service-account token (audience
  `context-gateway-simulate`), never with the administrator's, and records an `access.simulated`
  activity event naming the administrator and the simulated subject, the Endpoint, the action, the
  type and the decision. `503` when no gateway or no Keycloak admin client is configured.
- The Policies page has a "Try a person" panel that asks this route and shows the decision with
  the Policy that decided it, linked to that Policy.

## 40. Previewing an Endpoint's filter (T-3802, EP-85)

The filter editor's left side ([EP-86](../Requirements/endpoints.md)): one page of what the
Endpoint would serve if its filter were the draft in the body. The Portal compiles the draft into
the Endpoint's candidate broker view exactly as it compiles the saved filter (the view the
reconciler writes, T-3640), with the draft in place of the saved `ModelProjection` and
`hiddenAttributes`, and asks the broker's view preview (`POST /q/views/{id}/preview`, Antares
`docs/src/admin-api.md`) to run the page through it. Nothing is stored and the broker's views are
not reloaded.

```text
POST /api/v1/projects/{project}/endpoints/{name}/preview   → 200
```

```json
{"projection": {"classes": [{"name": "Event", "slots": ["name", "startDate", "location"]}],
                "filter": {"q": "endDate>=\"2026-09-25T00:00:00Z\""}},
 "hiddenAttributes": ["source"],
 "type": "Event", "id": ["urn:ngsi-ld:Event:agent-123"], "limit": 50, "offset": 0}
```

- The body has the members of [API/02 §7c](02-endpoint-representations.md): `projection`
  (`classes` and an optional `filter`, validated like a `ModelProjection`, MP-01; `null` or absent
  names none, and the Endpoint then serves the classes its Policies grant), `hiddenAttributes`,
  `type` (required), `id` (at most 100), `limit` (1 to 100, default 50) and `offset` (default 0).
  An unknown member is `400`; a refused draft is `400` naming the member at fault. A `temporalQ`
  is `400`: a projection's window narrows the permits in the space's rule document (G6, T-3812),
  which the preview does not replace, so a draft's window would be left out.
- The answer is `{"count": 42, "entities": [ … ]}`: the total under the draft and the page, the
  entities normalized as `application/json` returns them.
- The draft replaces the saved projection and hidden attributes only; the Endpoint's Policies,
  audience and view mapping stay as saved, so the preview never serves what the Policies do not
  grant, and the same draft saved answers the same entities through an ordinary read.
- Who may ask: a person who may read the Endpoint's Context Space (SP-06, SP-11). Anybody else
  gets the `404` an unknown Endpoint gets, because a draft can name a class or an attribute the
  saved projection does not publish yet. A workload token is `403`: the preview is a person's
  editor.
- Whose reading it is: on a `public` Endpoint the anonymous caller's (`x-jc-roles: public`), on
  the others the person's own (`x-jc-subject`, `x-jc-groups`, `x-jc-roles`), the subject headers
  the gateway sends on every hop (EP-14, EP-16). The broker's policy engine decides the page as it
  decides that person's read.
- `503` when the Portal has no broker address or admin token, or has not compiled the Endpoint's
  view yet; a broker that refuses the page answers through the route with its status.

## 41. What a principal reaches (T-3792, EP-103, EP-59)

The Access explorer (Access → Explorer): everything one person, group member, realm-role holder or
ServiceAccount reaches in a project, one row per Endpoint. The Portal decides nothing itself: for
each of the project's Endpoints it asks the gateway's `access/simulate` with no action
([API/02 §7b](02-endpoint-representations.md)) for the resolved subject, exactly as §39 asks one
decision, and keeps the Endpoints whose answer is admitted and grants or prohibits something.

```text
POST /api/v1/projects/{project}/access/explore                     → 200
DELETE /api/v1/organization/people/{id}/consents/{client}          → 204
```

```json
{"subject": {"kind": "serviceAccount", "name": "etl"}}
```

```json
{"subject": {"serviceAccount": "ovzdusie-etl", "groups": [], "roles": []},
 "endpoints": [{"name": "air", "slug": "zt4qm7ge2xdv6ksb3ncf5arw2y", "title": "Air quality",
                "space": "ovzdusie", "representations": ["ngsi-ld", "csv", "mcp"],
                "policies": ["public-air"],
                "permissions": [{"resource": {"type": "AirQualityObserved"},
                                 "actions": ["queryEntity", "retrieveEntity"],
                                 "attributes": ["pm10", "pm25"],
                                 "constraints": {"geoQ": {"geometry": "Polygon", "coordinates": [[[17.0, 48.1], [17.2, 48.1], [17.2, 48.2], [17.0, 48.1]]], "georel": "within"}}}],
                "prohibitions": []}],
 "mcpServers": ["city-air"],
 "consents": []}
```

- The body's `subject` is §39's: `person` with `id`, `group` or `role` with `name`,
  `serviceAccount` with the project's ServiceAccount `name`, or `public`. An unknown member or
  kind is `400`; a person the realm does not hold, or a ServiceAccount of another project, is
  `404`.
- `endpoints` lists the Endpoints the subject reaches, in name order, and only those: an Endpoint
  whose audience refuses the subject, or that grants them nothing, is absent, with its name and
  existence (EP-59, SP-20). Each row carries the gateway's AuthZEN `permissions` and
  `prohibitions` as the gateway answered them (types, actions, attributes, and the view
  restriction as `constraints` `q`, `scopeQ`, `geoQ`, `temporalQ`), `policies` naming the Policies
  in force for the subject, and `representations`, the surfaces the Endpoint serves (the list of
  its manifest, then `ngsi-ld` and, unless `spec.mcp` is `false`, `mcp`). The page
  builds each surface's URL under `/api/endpoint/{slug}` and the hub's at `/api/mcp`.
- `mcpServers` names the project's McpServers (ADR-N-043) with a member among the rows.
- The page is Project settings → Access explorer, `/projects/{project}/settings/explorer`, with
  the principal in the query (`?kind=serviceAccount&name=etl`, a person by `name=` their id).
- `consents`, for a person only, are the person's Keycloak consents (`clientId`,
  `grantedClientScopes`, `createdDate`, `lastUpdatedDate`): the hub, Endpoint MCP clients and Apps
  they connected. `DELETE …/consents/{client}` revokes one, which in Keycloak also ends that
  client's offline sessions of the person; it changes no grant and no Change is opened, because
  the grant stays where a Policy, group or role binding put it.
- Who may ask: organization administrators only; anybody else is `403`. Who may revoke: whoever
  may sign the person out (`disable` on Person and every right the person holds, §24). Each explore
  is recorded as an `access.explored` activity event naming the administrator and the subject,
  each revoke as a `person.changed` event naming the client. No token or secret is ever part of an
  answer.
- `503` when the Portal has no gateway address, no realm admin client or no Keycloak client to
  ask the gateway with, or one of them did not answer.

## 42. Pipeline templates of a project (T-3976, PL-52)

A pipeline a person saved to start the next one from, offered in the pipeline gallery beside the
fixed recipes. A template is the Portal's own record in its database (`pipeline_templates`), not
a kind in Git: it runs nothing and configures nothing, so saving one is no Change and needs no
approval. Using one fills the pipeline form, and that pipeline is checked and proposed like any
other.

```text
GET    /api/v1/projects/{project}/pipelinetemplates        the project's templates, newest first → 200
POST   /api/v1/projects/{project}/pipelinetemplates        save one from a pipeline manifest → 201
DELETE /api/v1/projects/{project}/pipelinetemplates/{id}   delete one → 204
```

The `POST` body is `{ "title": "…", "manifest": { …a Pipeline… } }`. The Portal keeps the
manifest's `spec` alone and takes out of it what belongs to the project it came from: the
`secretRefs`, each source's `dataSourceRef` and `endpointRef`, each output's `targetEndpoint`, and
the `contextSpaceRef`. What stays is the shape of the pipeline: its class, schedule, steps and
mapping, each source's query and trigger, each output's type and mode, its quotas. A manifest
holding a credential written out (a `password`, `token` or another key of the secret list, as a
write refuses it, MF-24) is `400` and nothing is saved. The title is 1 to 120 characters; a
project holds at most 50 templates (`409` past that), each at most 64 KiB of JSON (`400` past that).

Templates are per project: one project's never lists in another. Reading them needs `read` on
`Pipeline` in the project, saving and deleting `propose` on it (`403`); a project the caller may
not read is `404`, as is a template id the project does not hold.

```json
{
  "items": [
    {
      "id": "3c0f6f0e-8a1b-4c2d-9e4f-5a6b7c8d9e0f",
      "title": "MQTT sensor feed",
      "spec": {
        "class": "resident",
        "sources": [{}],
        "steps": [{ "kind": "bloblang", "bloblang": "root.type = \"AirQualityObserved\"" }],
        "outputs": [{ "type": "AirQualityObserved", "mode": "upsert" }]
      },
      "createdBy": "Jana Kováčová",
      "createdAt": "2026-10-12T09:00:00Z"
    }
  ]
}
```

## 43. Brokers (T-3990, PF-116, ADR-N-047)

Administration → Brokers: every context broker of the organization, the spaces on it, its version,
health and load. The Portal asks each broker itself; the browser never reaches a broker.

```text
GET /api/v1/organization/brokers                                   → 200
```

```json
{"items": [
  {"name": "main", "declared": true, "default": true, "size": "small",
   "features": ["temporal", "ogc", "odata", "mcp", "csr"],
   "health": "up", "version": "0.14.2", "reason": null,
   "spaces": ["ovzdusie/air", "doprava/stops"], "entities": 14232,
   "observedAt": "2026-10-12T08:00:00Z"},
  {"name": "canary", "declared": true, "default": false, "size": "small", "features": [],
   "health": "down", "version": null,
   "reason": "the broker did not answer; try again in a minute",
   "spaces": [], "entities": 0, "observedAt": "2026-10-12T08:00:00Z"}
]}
```

- `items` lists the `ContextBroker` manifests in name order; an organization that declares none
  has one row, the implicit `main`, with `declared: false` and no `size` or `features`.
- `health` is `up` when the broker's `GET /q/health` answers `200`, `down` when it answers
  anything else or nothing within five seconds, and `unknown` when the Portal has no address for
  it (`JC_PORTAL_BROKER_URL` for `main`, `JC_PORTAL_BROKER_URL_TEMPLATE` for the others);
  `reason` says why in a sentence for `down` and `unknown`. `version` is that answer's `version`.
- `spaces` names the spaces living on the broker as `project/space` (PF-111), and `entities`,
  the load, sums their entity counts as §29 reads them; a space whose count is unavailable adds
  nothing and the row keeps the reason.
- A space's page names its broker from §29's `broker`.
- Who may read: organization administrators only; anybody else is `403`. No address or token is
  part of the answer.

## 44. What an MQTT broker measures (T-4043, MF-54, ADR-N-048 §2.8)

The MQTT brokers page shows, per broker, the devices connected now, the messages per second and
when each account connected. The Portal reads the broker's own HTTP API inside the cluster; the
browser never reaches a broker, and nothing here is typed by anyone.

```text
GET /api/v1/projects/{project}/mqttbrokers/{name}/measured          → 200
```

```json
{"measured": true, "observedAt": "2026-10-12T08:47:05Z", "messagesPerSecond": 2.0,
 "publishers": 1, "reason": null,
 "accounts": [{"serviceAccount": "cam-west", "connected": 1, "since": "2026-10-12T08:46:51Z"},
              {"serviceAccount": "cam-east", "connected": 0, "since": null}]}
```

```json
{"measured": false, "observedAt": "2026-10-12T08:47:05Z", "messagesPerSecond": null,
 "publishers": null, "reason": "the broker did not answer; try again in a minute",
 "accounts": []}
```

- `messagesPerSecond` is RMQTT's `messages.publish` counter of the two newest samples of
  `GET /api/v1/metrics/history/sum` (one every five seconds), their difference over the time
  between them; `null` while the broker holds fewer than two samples.
- `accounts` lists every ServiceAccount the broker's roles name, in name order. `connected` counts
  the broker's connected clients (`GET /api/v1/clients`) whose `username` is the account's
  Keycloak client `{project}-{serviceAccount}`; `since` is the newest `connected_at` among them,
  and `null` when none is connected: sessions are clean (§2.2), so the broker keeps nothing of a
  client that left. `publishers` sums `connected` over the accounts of roles with `publish`
  filters. The pipeline runner and any other client is neither counted nor named.
- `measured: false` with a `reason` when the broker did not answer within five seconds, answered
  something else than its HTTP API, or the instance runs without the service mesh (the broker then
  serves no HTTP API at all, ADR-N-048 §2.8). A number is never shown in that case, and never
  zero for "unknown".
- Who may read: whoever may read the project's MqttBroker manifests. Anybody else gets `404`, the
  same answer as a broker the project does not declare (PF-59). No token, topic or client address
  is part of the answer.

### 44a. Testing a device message and watching for it (T-5534, PL-75, ADR-N-048 §2.3)

The broker card's **Test a message** dialog answers, before a device sends anything, what would
happen to one message of one ServiceAccount on one topic, and then watches for the real one.

```text
POST /api/v1/projects/{project}/mqttbrokers/{name}/test-publish    → 200 | 404 | 422
GET  /api/v1/projects/{project}/mqttbrokers/{name}/watch?account=&since=   → 200 | 404 | 422
```

```json
{"account": "cam-west", "topic": "cameras/helsinki-cam-west/frame", "payload": "{\"temperature\": 21.5}"}
```

```json
{"clientId": "helsinki-cam-west", "published": false,
 "steps": [
  {"step": "acl", "outcome": "ok", "reason": "allowed by cameras/helsinki-cam-west/#"},
  {"step": "size", "outcome": "ok", "reason": "22 of 65536 bytes"},
  {"step": "rate", "outcome": "ok", "reason": "one message fits; a client may publish 10 per second ..."},
  {"step": "binding", "outcome": "ok", "reason": "DataSource 'cameras' reads it, Pipeline 'frames' takes it"},
  {"step": "mapping", "outcome": "ok", "reason": "1 record(s)"},
  {"step": "endpoint", "outcome": "ok", "reason": "every attribute is written through helsinki-cameras"}]}
```

- The steps, in order: `acl` (a role of `account` publishes on a filter matching `topic`, its
  `${clientid}` bound to the account's Keycloak client `{project}-{account}`), `size` (the
  payload's bytes against `spec.limits.maxMessageBytes`), `rate` (`spec.limits.messagesPerSecond`
  per client), `binding` (the project's DataSources whose `mqtt.brokerRef` names the broker and
  whose `mqtt.topics` match, and the Pipelines that take each as their source; the first one
  the caller may read is tested; a DataSource or Pipeline the caller may not read is named only
  as "a DataSource you may not read" or "a Pipeline you may not read", as §47 hides a hop, and
  its mapping is not run), `mapping` (that Pipeline's steps over the payload on the project's
  runner, as §7a runs them) and `endpoint` (the record check of §7a for each mapped record). Each answers `ok`,
  `refused` or `unchecked` with a sentence; `unchecked` says why the step could not run (no
  Pipeline the caller may read, no runner, or a caller who may not propose Pipelines).
- Nothing is published: the Portal opens no MQTT connection and writes nothing. The broker, not
  the gateway, applies the ACL and the size limit to each PUBLISH, so a refused `acl` or `size`
  step is the only place a person sees those two before a device sends.
- `422` names the member at fault: an `account` no role of the broker names, a `topic` that is
  empty, longer than 1024 bytes, holds `+`, `#` or NUL or starts with `$`, a `payload` above
  1 MiB (the ceiling of every broker, MF-54). A payload above the broker's own limit is the
  `size` step's refusal, not a 422.
- `watch` answers `{observedAt, refusals, refusalsRead, connected, messagesPerSecond, reason}`:
  the CONNECTs the gateway refused for this broker after `since` (RFC 3339), of `account` when
  given, newest first, each `{at, reason, account}`; the account's connected clients and the
  broker's messages per second as §44 measures them. `reason` is one of `no_token`, `expired`,
  `wrong_audience`, `invalid_token`, `wrong_client`, `unknown_account`, `other_project`,
  `no_role`, `no_subscription`, `source_address` (the address is outside `allowedSources`, PF-129), `no_realm`. `refusalsRead: false` with `reason` when the gateway
  did not answer; a refusal is never invented and an empty list then means "unknown".
- The gateway counts each refusal of a broker the repository declares as
  `jc_mqtt_auth_refused_total{broker,reason}` and keeps the newest 50 per broker in memory: the
  time, the reason and the broker's own ServiceAccount the username named, never the token, the
  client id or the address. It serves them at `GET /internal/mqtt/refusals/{release}` on its
  internal listener (port 8082) to the mesh identity `JC_GATEWAY_MQTT_PORTAL_IDENTITY` names and
  answers `403` to anyone else; the Portal reads that listener on the host of
  `JC_PORTAL_GATEWAY_URL`.
- Who may call: whoever may read the project's MqttBrokers; anybody else gets `404`, as for a
  broker the project does not declare (PF-59). The `mapping` and `endpoint` steps run only for a
  caller who may propose a Pipeline in the project.

## 45. Moving a space to another broker (T-4000, PF-115, ADR-N-047 §2.6)

One job moves one space, with its history, from the broker it lives on to another `ContextBroker`
of the organization. Editing `brokerRef` by hand is refused for a space that holds entities
(§43's admission); this is the way.

```text
POST /api/v1/projects/{project}/spaces/{space}/move                → 202
GET  /api/v1/projects/{project}/spaces/{space}/move                → 200
```

```json
{"to": "canary"}
```

```json
{"space": "ovzdusie/air", "from": "main", "to": "canary", "state": "done", "reason": null,
 "source": {"entities": 14232, "attrInstances": 90311, "sha256": "4f0c…"},
 "target": {"entities": 14232, "attrInstances": 90311, "sha256": "4f0c…"},
 "startedAt": "2026-10-12T08:00:00Z", "endedAt": "2026-10-12T08:03:12Z"}
```

- The job, in order: commits the annotation `joinedcontext.com/moving-to: {to}` on the space's
  `ContextSpace` as the Portal (state `holding`); waits until the gateway's scrape reports
  `jc_gateway_space_held{space="{tenant}"} 1`, at most ten minutes; streams the source broker's
  `GET /q/tenants/{tenant}/export` into the target's `POST /q/tenants/{tenant}/import`
  (`copying`); reads `GET /q/tenants/{tenant}/digest` on both (`comparing`); then commits
  `spec.brokerRef: {to}` and removes the annotation in one commit (`switching`), and is `done`.
- `failed` with `reason` when any step does not finish: the annotation is removed so the space
  is read-write again on its source broker, and a target that took part of the copy is purged
  (it held nothing before: the import refuses a tenant that does). A digest that differs fails
  with both digests in the answer.
- The source keeps its copy of the tenant after a move. Nothing routes to it; an operator purges
  it with the broker's `DELETE /q/tenants/{tenant}` once the move is checked.
- A broker refuses the export of a tenant holding Subscriptions, registrations or snapshots
  (`409`, Antares ADR-0030 §5): the job fails with the broker's counts, and the space stays.
- `400` for a `to` that names no `ContextBroker`, or the broker the space already lives on.
  `409` while another move of the same space runs. `503` when the Portal has no address for
  either broker, no broker admin token, no gateway address or no forge.
- `GET …/move` answers the last move of the space since the Portal started, `404` when there was
  none. A Portal that restarts mid-move forgets the job; the annotation stays, the space stays
  read-only, and starting the move again to the same broker resumes from the hold. A target left
  holding part of a copy refuses the import, and the job fails saying so.
- Who may move: organization administrators only; anybody else is `403`. The job's commits are
  the Portal's and name the administrator who started it. No address or token is part of an
  answer.

## 46. Personal data of a space (DM-124…DM-127, DM-142)

For a space whose model names its data subject ([Architecture/11 §9](../Architecture/11-data-models.md#9-personal-data-in-a-model): a class with the annotation `dataSubjectRef`, the slot that holds the person), an owner of the space (`approve` on its ContextSpace) runs the data subject's rights. The subject's value, the one the class's `dataSubjectRef` slot holds, travels in the request body and never in a URL an access log keeps; the Portal keeps only its keyed hash, an HMAC-SHA256 per project and space. A subject slot ingested as `keyedHash` (DM-121) holds the value's HMAC under the space's `spec.personalData.keyRef`: the Portal searches such a slot for the subject's HMAC-SHA-256 under the same key, the subject's UTF-8 bytes in lowercase hex, which is what the gateway stored. While the Portal holds no key for the space (no `keyRef`, or one that did not resolve to at least 32 bytes at the last sync), an export or an erasure on such a space answers `409` and names the slot, never an erasure `done` with no Entities.

```text
POST   /api/v1/projects/{project}/spaces/{space}/subjects/export           { "subject", "reason" } → 200, Cache-Control: no-store
GET    /api/v1/projects/{project}/spaces/{space}/erasures                  the space's newest 50 erasures, proposals included → 200 { "items" }
POST   /api/v1/projects/{project}/spaces/{space}/erasures                  { "subject", "reason" } → 202 `proposed`, Location of the erasure; nothing erased
POST   /api/v1/projects/{project}/spaces/{space}/erasures?dryRun=All       the same body → 200 { "dryRun": true, "entities", "groups": [{ "class", "entities", "attrs" }] }, nothing erased or logged
GET    /api/v1/projects/{project}/spaces/{space}/erasures/{id}             its state and counts per store → 200
POST   /api/v1/projects/{project}/spaces/{space}/erasures/{id}/approve     { "subject" } → 200 `open`, the erasure runs (DM-142)
POST   /api/v1/projects/{project}/spaces/{space}/erasures/{id}/decline     { "reason" } → 200 `declined`
```

An erasure takes four eyes (DM-142). The first owner proposes it with the subject and the reason, and nothing is erased yet: the erasure is `proposed`, with `proposedBy` and the Entities the search found then. Another person with `approve` on the ContextSpace approves it: an owner of the space, or an `org-admin`, so a space with a single owner is never stuck. The proposer approving answers `403 self-approval`, and a bearer token approving or declining answers `403` (AG-133). The approver types the subject again; the Portal compares its keyed hash with the proposal's and answers `422` naming `subject` when they differ, so the log never holds the value and the second person checks the request instead of confirming a click. At approval the Portal searches the space again and erases what it holds then; the erasure becomes `open` with `approvedBy` and `decidedAt`. Either owner, the proposer included, declines a proposal with a reason (`declined`, `400 reason-required` without one). A proposal nobody decided within 7 days reads `expired` and is approved by nobody; the owner proposes it again. Approving or declining an erasure that is no longer `proposed` answers `409` with its state.

Rectification (Art. 16) has no route of its own yet: the corrected value is written through the space's Endpoint like any other write, and its superseded version keeps the old value until a rectification route redacts it.

The Portal finds those Entities on the space's own surface, `/cs/{space}`, as its `portal-reconciler` account. The gateway grants that account read access (`retrieveOps`, `queryTemporal`, `retrieveTemporal`) on the classes whose attributes the model marks `personalData`, through the Policy `portal-personal-data` it derives for each such space; no space owner writes it, and it grants no write. A surface that refuses the Portal (`404`) or fails is a `503`, never an answer of no Entities.

The export answers every Entity of the space naming the subject, each with its class, its current state and its stored history, to the caller's session only. The read log of those Entities joins it once the gateway keeps one (DM-128, T-4666).

An erasure finds the Entities naming the subject in each such class and erases, from every store that holds a copy, the class's Attributes marked `personalData` and the subject slot itself, or the whole Entity of a class marked `personalData` as a whole:

| Store | What goes |
| --- | --- |
| `broker` | the Attributes from the current state, every version and the dead letters; the broker keeps a content-free record and applies it again when it starts (Antares `POST /q/erasures/{tenant}`) |
| `ckanDatastore` | the Entities' rows of the DataStore tables the space's published Endpoints mirror, by `entity_id`; the next mirror run writes back what an Entity still holds |
| `portal` | the Portal's trash and comments of the Entities |
| `conversations` | every stored assistant conversation that mentions an Entity's URN or, on the first attempt, the subject's own words |
| `exports`, `caches` | nothing: the platform keeps no export file (`file.csv` is streamed) and no entity cache (every answer is `private`), so both are `done` at once |

```json
{
  "id": "er-20261019-0a1b2c3d4e5f",
  "state": "open",
  "requestedAt": "2026-10-19T10:00:00Z",
  "proposedBy": "user:steward",
  "approvedBy": "user:owner2",
  "decidedAt": "2026-10-19T11:20:00Z",
  "completedAt": null,
  "entities": 14,
  "counts": { "broker": 14, "versions": 52, "ckanDatastore": 14, "portal": 2, "conversations": 1, "exports": 0, "caches": 0 },
  "stores": { "broker": { "state": "done", "count": 14, "versions": 52, "attempts": 1 }, "ckanDatastore": { "state": "pending", "count": 0, "attempts": 1, "error": "CKAN unavailable: …" } }
}
```

An approved erasure answers at once and stays `open` until every store reports `done`: a store that does not answer is asked again each minute, the CKAN DataStore at each reconciler run, and after an hour of refusals it is `failed`, which leaves the erasure for a person to read why. The erasure log keeps the subject's keyed hash, the Entities and the counts, never a value, outside the broker's database; when the Portal starts and every hour after, it sends again every erasure whose record a broker no longer holds, after that broker's database was restored from an older backup ([Deployment/07 §5](../Deployment/07-backup-restore.md#5-backups-and-erasure)). Every call is audited with its `reason`; a call without one answers `400` naming `reason-required`.

## 47. MQTT allowance and capture (PF-117…PF-120, MF-62, PL-87, ADR-N-048 §2.9–2.10)

What a project may run as MQTT edge brokers, what its brokers use of it, and, in `capture` mode,
the newest messages a broker received. The allowance is written only as a field of a manifest:
`Organization.spec.limits.mqtt` or `Project.spec.quotas.mqtt`, through a red-lane Change an
`org-admin` approves (PF-98, PF-117). There is no write route for it here.

```text
GET    /api/v1/projects/{project}/mqtt/allowance                       → 200
GET    /api/v1/projects/{project}/mqttbrokers/{name}/captured?topic=&limit= → 200
GET    /api/v1/projects/{project}/mqttbrokers/{name}/bindings          → 200
GET    /api/v1/projects/{project}/mqttbrokers/{name}/access?account=   → 200
DELETE /api/v1/projects/{project}/mqttbrokers/{name}/captured          → 204
```

```json
{"enabled": true, "origin": "project",
 "raise": "an org admin can raise this in Organization settings > Projects > helsinki",
 "modes": ["ingest", "capture"], "addresses": ["platform", "dedicatedPort"],
 "ports": [18831], "customHostnames": [], "deviceAuth": ["token"],
 "brokers": {"limit": 2, "used": 1, "left": 1},
 "connections": {"limit": 2000, "used": 1000, "left": 1000},
 "messagesPerSecond": {"limit": 20000, "used": 10000, "left": 10000},
 "maxMessageBytes": {"limit": 65536},
 "portsInUse": [{"port": 18831, "broker": "cameras"}]}
```

```json
{"broker": "cameras", "mode": "capture", "window": "PT1H", "messagesPerTopic": 20,
 "messages": [{"topic": "cams/cam-west/frame-meta", "receivedAt": "2026-10-20T19:41:02Z",
               "clientId": "helsinki-cam-west", "bytes": 212,
               "payload": "{\"seq\": 4411, \"objects\": 3}", "encoding": "utf-8"}],
 "truncated": false}
```

- `origin` is `project` (the Project's `spec.quotas.mqtt`), `organization`
  (`spec.limits.mqtt`) or `default` (neither: `enabled: false` and every number `0`). `raise`
  is the sentence the verdict uses when it refuses a broker (PF-101).
- `used` sums the project's committed `MqttBroker` manifests, not the connected devices:
  `connections` sums `spec.limits.maxConnections`, `messagesPerSecond` sums
  `maxConnections` × `messagesPerSecond` of each broker (ADR-N-048 §2.9). `left` is never below
  `0`; a project over a lowered allowance shows `used` above `limit` (PF-100).
- Who may read the allowance: whoever may read the project. `clientCaRef` and the hostnames'
  `certificateRef` are not part of the answer; a hostname with a stored certificate carries its
  `notAfter` instead.
- `captured` answers the broker's kept messages, newest first, at most `limit` (1…1000, default
  100), only of `topic` when given (an exact topic, no wildcard). `payload` is the message as
  UTF-8 text when it decodes, else base64 with `encoding: "base64"`. A broker not in `capture`
  mode answers an empty `messages` and its `mode`, without `window` and `messagesPerTopic`.
  `clientId` is the device a topic names where a publish filter of the broker holds
  `${clientid}` as a whole level; MQTT 3.1.1 carries no publisher, so it is absent otherwise. A
  `topic` with `+` or `#`, or a `limit` outside 1…1000, answers `400` naming it; a capture store
  that does not answer, `503`.
- The capture subscriber runs on the reconciler's leader: one MQTT connection per `capture`
  broker to its internal listener, username `jc-portal-capture` and no password, from a Portal
  pod (the gateway resolves `JC_GATEWAY_MQTT_PORTAL_HOST`), subscribed to the publish filters of
  the broker's roles with each `${clientid}` level widened to `+`. It writes what arrived once a
  second and trims the broker to its caps in the same transaction.
- Who may read or delete the capture: a person whose binding holds `approve` on `MqttBroker`
  in the project (PL-87). Anybody else, a reader of the manifest included, gets `404`, the
  same answer as a broker the project does not declare (PF-59). `DELETE` removes every kept
  message of the broker at once and records who did it in the activity log, without the
  payloads.

```json
{"broker": "sensors",
 "bindings": [{"topicFilter": "devices/+/data",
               "dataSource": {"kind": "DataSource", "name": "sensors-in"},
               "pipeline": {"kind": "Pipeline", "name": "sensors"},
               "endpoint": {"kind": "Endpoint", "name": "weather-write", "space": "weather"},
               "type": "WeatherObserved",
               "attributes": [{"from": "temperature", "to": "temperature"},
                              {"from": "lon, lat", "to": "location"},
                              {"to": "dateObserved"}]},
              {"dataSource": {"kind": "DataSource", "hidden": true}}]}
```

- `bindings` answers how a topic reaches an attribute, read from the manifests the Portal
  mirrors: each DataSource of the project whose `mqtt.brokerRef` names the broker, each of its
  `mqtt.topics`, each pipeline of the same project that reads the DataSource by
  `dataSourceRef`, and each Endpoint the pipeline writes through. One row per topic filter,
  pipeline and output; a DataSource no pipeline reads is a row without `pipeline`, a broker no
  DataSource names answers an empty `bindings`.
- `type` is the output's `entityType`, else the type the output mapper's step names.
  `attributes` lists the mapper's map (`from` the record field, two fields for a point, absent
  for an expression or a fixed value); a step written by hand has no `attributes`, and the
  pipeline editor shows its code.
- Who may read: whoever may read `MqttBroker` in the project; anybody else gets `404` (PF-59).
  Each hop is checked on its own: a DataSource the caller may not read is
  `{"kind": "DataSource", "hidden": true}` without its name and topics, a pipeline as for the
  flow map (§50), an Endpoint by its space. A hidden hop ends the row there: nothing it leads to
  is shown. A pipeline of another project is never joined. Nothing here holds a secret, a
  password reference or a payload.
- `access?account=<serviceAccount>` explains which roles of the broker the account holds now:
  the answer of jc-core `mqtt::roles_held` at the time of the request, the decision the gateway
  makes at CONNECT and publish (ADR-N-048 §2.13), read over the two manifests and decided nowhere
  else. Each held role carries its `publish` and `subscribe` filters, `bound` (a RoleBinding
  `mqtt/<broker>/<role>` or `mqtt/*/<role>` in force), `legacy` (the deprecated
  `serviceAccounts` list) and `until`, when its last grant ends. `outOfWindow` lists the
  account's grants of this broker outside their validity window. A caller who may not read the
  project gets `404` (PF-59), a member who may not read `MqttBroker` `403`, an unknown broker or
  an account the caller may not read `404`, a missing or malformed `account` `400`.

```json
{"broker": "air", "account": "air-fleet", "at": "2026-10-29T12:00:00Z",
 "roles": [{"role": "sensor", "publish": ["air/${clientid}/pm10"], "subscribe": [],
            "legacy": false, "bound": true, "until": "2026-12-01T00:00:00Z"}],
 "outOfWindow": []}
```

## 48. Pipeline access requests (PL-90…PL-93)

A pipeline that reads or writes a Context Space it has no grant on asks the space's owner for one. The request lives in the owning project and grants nothing until an owner there approves it; approval proposes the grant as a Change of the owning project, through its lane.

```text
POST   /api/v1/projects/{project}/pipelines/{name}/access-requests   { "endpoint": { "project", "name" }, "operations": "read"|"write"|"history", "types": [{ "type", "attrs": [...] }], "until"?, "reason" } → 201
GET    /api/v1/projects/{project}/pipelines/{name}/access-requests   the pipeline's requests, outgoing → 200
GET    /api/v1/projects/{project}/pipelines/{name}/access-requests/endpoints   the Endpoints the request may name → 200
GET    /api/v1/projects/{project}/access-requests                        the requests into this project's spaces, incoming → 200
POST   /api/v1/projects/{project}/access-requests/{id}/approve           {} → 202, the Change
POST   /api/v1/projects/{project}/access-requests/{id}/decline           { "reason" } → 200
POST   /api/v1/projects/{project}/access-requests/{id}/revoke            { "reason" } → 202, the Change
```

Filing needs `propose` on the Pipeline in `{project}`, and so does the `endpoints` list the request form picks from: each Endpoint of another Context Space (in this project or another) that the caller may read in its project, or whose `audience` is `organization` or `public`, with the classes of its space's compiled model and their attribute names, as `{ "items": [{ "project", "name", "space", "types": [{ "type", "attrs": [...] }] }] }`, names only and never data; a private Endpoint of a project the caller cannot read is never listed. Filing checks a listed Endpoint against its model: a type or attribute the model does not hold answers `400` with one `errors` line per value naming `types[i].type` or `types[i].attrs`; a space with no compiled model lists no type, and its owner judges the names. Any other Endpoint is asked for by its typed `project/endpoint` name: one the caller may not read and one that does not exist get the same answer, `201` with the request pending and no model check, so filing never tells which Endpoints, types or attributes exist; the owners of the Endpoint's space judge the types and attributes when they decide, and a request for a name that exists nowhere is shown to no owner. A name that is no DNS-1123 label, or an Endpoint of the pipeline's own space, answers `400` naming `endpoint.name`. The pipeline's side of a request (the filing answer and the outgoing list) carries no `space`. The incoming list, approve, decline and revoke need `approve` on the Endpoint's ContextSpace in the owning project, and a caller without it reads `404` for a request of a space they cannot read. An outgoing request shows the owning project's name, the Endpoint's name, the types and attributes asked for and the state (`pending`, `approved`, `declined`, `revoked`, `ended`), never the space's data. Approving or revoking answers the Change (MF-12); its lane is the owning project's, and its approval follows the four-eyes rule there. The requester approving their own request answers `403 self-approval`.

```json
{
  "id": "ar-0a1b2c3d4e5f",
  "pipeline": { "project": "city-kpis", "name": "air-index" },
  "account": "city-kpis/pl-air-index",
  "endpoint": { "project": "helsinki", "name": "air-quality" },
  "operations": "read",
  "types": [{ "type": "AirQualityObserved", "attrs": ["pm10", "pm25", "dateObserved"] }],
  "until": "2027-01-31T23:59:59Z",
  "state": "pending",
  "requestedBy": "jana",
  "requestedAt": "2026-10-20T19:40:00Z",
  "decidedBy": null,
  "change": null
}
```

### 48.1 A person's request on an Endpoint (PL-115)

A person who reads an Endpoint but gets nothing through it asks the owners of its space on the Endpoint's page. The request is the same kind of row, in the same incoming list, decided by the same doors.

```text
GET    /api/v1/projects/{project}/endpoints/{name}/access-requests   { "types": [...], "refusal": string|null, "items": [...] } → 200
POST   /api/v1/projects/{project}/endpoints/{name}/access-requests   { "types": ["AirQualityObserved"], "until"?, "reason" } → 201
```

Both need `read` on the Project and on the Endpoint in its space; anybody else reads `404`. `GET` answers the types the caller may ask for (the classes of the space's compiled model, narrowed to the classes of the ModelProjection the Endpoint names), `refusal` when no new grant could reach the Endpoint (it names `policyRef` or `policyRefs`, or it is an App's own Endpoint, AP-139), and the caller's own requests on it, never another person's. `POST` comes from a person's session; a service account's or an agent's token answers `403`. The body names no grantee: an unknown member answers `400`, and the grant is always for the caller. A type the Endpoint does not serve, an empty or doubled type, an end that passed and an empty reason each answer `422` with one `errors` line naming its field (`types[i]`, `until`, `reason`); an Endpoint with a `refusal`, or a second open or granted request of the same person on it, answers `409`. A person's request reads `"pipeline": null`, `"person": "{username}"` and `"account": "{username}"`; its types carry `"attrs": []` (every attribute of the type). Approving proposes one Policy into the Endpoint's project: `{endpoint}-{id}` (or `access-{id}` when that is over 63 characters), assignee `{ "kind": "user", "id": "{username}" }`, `retrieveOps`, `information` naming the asked-for types, `validity.to` the end; the Endpoint itself is unchanged. Revoking ends that Policy now and leaves the Endpoint unchanged.

## 49. The agent registry (T-4849, AG-126…AG-129)

Administration → Agents: every agent identity of the organization and its projects in one
answer, so an administrator sees which agents exist, whom each acts as, what it reaches and
which ones run now.

```text
GET /api/v1/organization/agents                                    → 200
```

```json
{"items": [
  {"kind": "AssistantDeployment", "name": "city-chat", "project": "helsinki",
   "title": "City chat", "owner": {"type": "project", "name": "helsinki"},
   "actsAs": "self", "spaces": ["air"], "endpoints": ["air-quality"],
   "lastActivity": null, "spend": {"used": null, "cap": 200000, "per": "day"},
   "state": "active", "runId": null},
  {"kind": "AgentRun", "name": "Air quality dashboard", "project": "helsinki",
   "title": null, "owner": {"type": "person", "name": "jana@example.org"},
   "actsAs": "caller", "spaces": ["air"], "endpoints": ["air-quality"],
   "lastActivity": "2026-10-21T02:10:00Z", "spend": {"used": 41000, "cap": 400000, "per": "run"},
   "state": "running", "runId": "run-7f3a", "profile": "builder"}
]}
```

- `kind` is `AssistantDeployment`, `AgentProfile`, `McpServer`, `ServiceAccount` or `AgentRun`;
  `project` is `org` for the organization's own manifests (agent profiles, organization service
  accounts and MCP servers). Items come namespace by namespace, the organization first, then
  kind by kind in the order above and by name, a project's runs after its manifests, newest
  first.
- `owner` is the service account's `spec.owner.user` or the run's starter (`type: person`), else
  the project that holds the manifest (`type: project`).
- `actsAs` is `self` for a service account and for a deployment on a channel other than
  `internal` (it calls with the `jc-assistant` service account), and `caller` for an `internal`
  deployment (the person's own token, AG-115), an agent profile and its runs (AG-94) and an MCP
  server (each client's own token).
- `endpoints` names the Endpoints an item reaches as far as its manifest says: a deployment's
  `connectors` and `mcpServer`, an MCP server's `Endpoint` members, a run's endpoints; `spaces`
  adds their `contextSpaceRef` and a service account's role scopes.
- `spend` is in tokens: a deployment's `budget.tokensPerDay` as its `cap` per `day` with `used`
  `null`, since the day's count lives in `jc-agent-proxy` (AG-110); an agent profile's `used` sums
  today's (UTC) runs of it that the caller sees, with no `cap`; a run's `used` is its tokens so far
  and its `cap` its profile's `model.maxTokensPerRun`. Service accounts and MCP servers carry
  `null` for all three.
- `state` is `active`, `paused` (a deployment with `enabled: false`), `running` or `waiting` (a
  run interviewing its person or awaiting approval). `lastActivity` is a run's latest timestamp,
  and for a deployment or a profile the newest of its runs the caller sees, else `null`.
- Who may read: any signed-in person, who gets per namespace only the kinds their bindings read
  there (`read` on the kind; a binding to one space reads only the manifests of that space), and
  a run only when they started it or approve in its project; nothing they may not read is
  counted. A caller who reads no agent kind anywhere gets `403` with the sentence that names who
  can grant it. No token, credential, key, `secretRef`, system prompt or
  prompt is part of the answer (AG-127).
- Pause and resume use no route of their own: the page proposes the deployment's manifest with
  `spec.enabled` flipped by `PUT /api/v1/projects/{project}/assistantdeployments/{name}`, after
  its dry run, like any edit (AG-129); the answer is the Change, which the project's review
  merges.

## 50. The data flow map (T-4862, PL-31…PL-33, PL-45, PL-52, PL-76)

The Pipelines page draws every path data takes through a project: MQTT broker or DataSource,
pipeline, Endpoint, Context Space, and on through derived and KPI pipelines into the next space.
The Portal builds it from the manifests it mirrors and the runs it logged; it asks neither the
runner nor the broker.

```text
GET /api/v1/projects/{project}/flow-map[?space={space}]          → 200
```

```json
{"project": "helsinki",
 "nodes": [{"id": "helsinki/DataSource/hsl-hfp", "kind": "DataSource", "name": "hsl-hfp",
            "project": "helsinki", "opaque": false, "sourceType": "mqtt",
            "hosts": ["mqtt.hsl.fi:8883"], "topics": ["/hfp/v2/journey/ongoing/vp/#"]},
           {"id": "helsinki/Pipeline/stop-delays", "kind": "Pipeline", "name": "stop-delays",
            "project": "helsinki", "opaque": false, "role": "derived", "phase": "Live",
            "enabled": true, "lastRun": {"at": "2026-10-21T04:00:00Z", "sent": 310,
            "rejected": 0, "failed": 0, "perMinute": 5.2}}],
 "edges": [{"from": "helsinki/Pipeline/stop-delays", "to": "helsinki/Endpoint/stops-write",
            "relation": "writes",
            "grant": {"state": "missing", "endpoint": "stops-write", "operation": "upsertBatch",
                      "fix": "add a Policy on space stops that grants the ServiceAccount pipelines upsertBatch"}}]}
```

- `kind` is one of `MqttBroker`, `DataSource`, `Pipeline`, `Endpoint`, `ContextSpace`; `id` is
  `{project}/{kind}/{name}`, which the edges name. `relation` is `connects` (a broker a
  DataSource reads by `mqtt.brokerRef`), `feeds` (a DataSource a pipeline reads), `exposes` (a
  space an Endpoint serves), `reads` (an Endpoint a pipeline reads), `writes` (an Endpoint a
  pipeline writes) or `stores` (the space an Endpoint writes into).
- Every `reads` and `writes` edge carries its `grant`: `granted` names the Policy that gives the
  `pipelines` ServiceAccount `upsertBatch` (a write) or `queryEntity`/`queryBatch` (a read) on the
  Endpoint's space (the name only to a caller who reads Policy in that space; others see
  `granted` without it); `missing` says which Policy to add, and the runner is refused there;
  `external` marks an Endpoint of another project, whose owner's grant decides (PL-32).
- A pipeline's `role` is `kpi` when it writes a space ending in `-kpi` or the type
  `KeyPerformanceIndicator` (PL-45, PF-54), `derived` when it reads an Endpoint, else `ingest`.
  `phase` is the reconciler's, `enabled: false` a paused pipeline, `lastRun` the newest run
  (PL-62) with `perMinute` over runs of a minute or more, read only for the pipelines on the
  map; a run log that cannot be read leaves that pipeline without `lastRun`.
- `space` keeps only the pipelines that read or write that space, with their sources, Endpoints
  and spaces; without it the map also lists the brokers, DataSources and spaces no pipeline uses
  yet. A value that is not a space name is `400`.
- Who may read: whoever may read the project; each node only when the caller may read its kind
  there (a pipeline, Endpoint or space in the space a binding names). Anybody else gets `404`
  (PF-59). An Endpoint of another project a pipeline reads is a node with its name alone
  (`opaque: true`) unless the caller reads that project. A DataSource shows its hosts without
  user, path or query, never a secret or a token.

## 51. Agent activity (T-4851, AG-134…AG-136)

An agent's Activity tab: what its runs did, step by step, and the alerts an administrator acts
on.

```text
GET /api/v1/organization/agents/activity?profile=builder&verdict=refused&limit=20  → 200
```

```json
{"runs": [
  {"id": "run-7f3a", "project": "helsinki", "profile": "builder",
   "title": "Air quality dashboard", "startedBy": "jana@example.org", "status": "building",
   "createdAt": "2026-10-21T02:10:00Z", "spaces": ["air"], "tokens": 41000,
   "costUsd": 0.0125, "refusals": 1, "writes": 0, "mayStop": true,
   "steps": [
     {"at": "2026-10-21T02:10:04Z", "tool": "jc_resource_list", "target": "Endpoint",
      "verdict": "ok", "write": false, "durationMs": 9},
     {"at": "2026-10-21T02:10:09Z", "tool": "jc_endpoint_propose", "target": "air",
      "verdict": "refused", "write": true, "reason": "forbidden: missing role steward"}
   ]}
 ],
 "nextCursor": "2026-10-21T02:10:00Z|run-7f3a",
 "alerts": [
   {"kind": "spendShare", "project": "helsinki", "profile": "builder", "runId": "run-7f3a",
    "runTitle": "Air quality dashboard", "count": 85, "at": "2026-10-21T02:10:00Z"}
 ],
 "choices": {"people": ["jana@example.org"], "spaces": ["air"]}}
```

- Filters, each optional: `project`, `profile` (an agent profile's name), `run` (a run id),
  `person` (the starter), `space` (a Context Space one of its Endpoints is in), `verdict` (`ok`:
  no refused step, `refused`: at least one), `since` and `until` (RFC 3339, on the start),
  `limit` (1–50, default 20) and `cursor` (`nextCursor` of the previous page). Any other
  parameter, or a malformed value, is `400`.
- `choices` lists the starters and the Context Spaces of the runs the caller sees for this
  agent or run, whatever the other filters; the Activity tab offers `person` and `space` only
  from it. A `person` or `space` outside it is `403` naming the field. `alertProjects` lists the
  projects the caller approves in, where they may subscribe to the agent's alerts (§37).
- Who sees what: the runs of every project the caller reads, their own, or every person's where
  they approve (AG-93); a caller who reads no project gets `403`. The answer reads the 500 newest
  of those runs.
- A step is one tool call: `target` is the Endpoint, resource, page or space it named, never its
  arguments; `write` is true for an operation that is not read-only; `reason` is a refusal's
  problem text or the reason a write gave, masked and cut at 400 characters. Its input and output
  are never part of the answer (MF-52 `transcriptRedaction`). `costUsd` sums the costs the
  provider reported and is absent when none did.
- `mayStop` is true for a run in flight that the caller may cancel with
  `POST /api/v1/projects/{project}/agent-runs/{id}/cancel`; that run's event stream then says
  who stopped it (AG-135).
- `alerts`, newest first, over the runs the filters select before paging: `refusalSpike` per
  agent profile with 5 or more refused steps in the last hour (`count` the refusals),
  `spendShare` per run at 80 % or more of its profile's `model.maxTokensPerRun` (`count` the
  percent), `offHoursWrite` per run that wrote outside Monday to Friday 06:00–18:00 UTC
  (`count` the writes); a run alert carries `runId` and `runTitle`, the title the run's card shows.
- Every read is logged with the caller and the filters.

## 52. What an agent account may do (T-4850, AG-130…AG-133)

The Access view of one `ServiceAccount`, the identity an agent acts as when it acts as itself:
each grant, where it holds, until when, and what it gives in the Portal.

```text
GET  /api/v1/projects/{project}/serviceaccounts/{name}/access     → 200
POST /api/v1/projects/{project}/serviceaccounts/{name}/revoke     → 202 Change
POST /api/v1/projects/{project}/serviceaccounts/{name}/secret     → 200, shown once
```

```json
{"account": "vendor-ci", "project": "helsinki", "delegates": false,
 "mayPropose": true, "mayRevoke": true, "noSecret": null, "mayIssueSecret": true,
 "grants": [
   {"index": 0, "role": "pipeline-admin", "scope": "project:helsinki", "state": "active",
    "notAfter": "2026-11-01T00:00:00Z",
    "rules": [{"kinds": ["Pipeline"], "verbs": ["propose", "delete"]}]},
   {"index": 1, "role": "space-reader", "scope": "contextSpace:air", "state": "expired",
    "notAfter": "2026-10-20T00:00:00Z", "rules": []}
 ]}
```

- `state` is `active` (no window, or now inside it), `pending` (`notBefore` ahead) or `expired`
  (`notAfter` passed); only an `active` grant gives anything, here and at the gateway (AG-131).
- `rules` are the rules of the `Role` the grant names, without `approve`, which no account holds
  (PF-58); a role that names no `Role` (a gateway template, CC-60) has none and acts on data
  alone, through the Policies of the Endpoints the account reaches (§41 shows those).
- `delegates` is `true` for an account with `spec.delegation: token-exchange`: a person's token
  its client exchanged is decided as that person, not with these grants (AG-95).
- Who may read: `read` on `ServiceAccount` in the project; otherwise `403`, and `404` for no such
  account. `mayRevoke` is `true` for a Portal session with `propose` and `approve` on it.
- Revoke takes `{"index": 1, "role": "space-reader"}`, the grant's position and its role, so a
  list that moved since the page read it removes nothing: a mismatch is `409`, and so is the
  account's last grant (delete the account to take all its access away). The Portal writes the
  Change and merges it at once when the manifest on the main branch differs from it by removed
  grants alone; otherwise the Change waits for a second person like every access change. A
  bearer token is `403`. Each revoke is an `access.revoked` activity event (AG-132).
- Widening (a new grant, a wider scope, a later or removed `notAfter`) uses no route of its own:
  the page proposes the account's manifest like any edit, a red-lane Change that someone other
  than its proposer approves (PF-58, T-4833).
- `POST /api/v1/projects/{project}/changes/{id}/approve` and `/reject` refuse every bearer token
  with `403` (AG-133).
- `noSecret` says why the account has no client secret the Portal can issue: `workload` (its pod
  token opens its client), `key-pair` (a credential with `jwks`) or `no-credential`; `null` when it
  signs in with a secret. `mayIssueSecret` is `true` for a Portal session with `approve` on it.
- `POST .../secret` has Keycloak generate a new secret for the account's client `{project}-{name}`
  and answers it once, with `Cache-Control: no-store`:
  `{"clientId": "helsinki-vendor", "clientSecret": "…", "tokenUrl": "https://idm…/protocol/openid-connect/token"}`.
  The Portal keeps no copy and no route reads it again. The previous secret is refused at once,
  the account's earlier tokens are marked revoked (the gateway refuses them at its next poll), and
  an `access.revoked` activity event names who did it (PF-37, T-5626). Only a person's session
  with `approve` on `ServiceAccount`: a bearer token is `403`, and so is a caller without
  `approve`. An account with `noSecret`, or whose client does not exist yet or is the platform's
  own (`keycloak-clients.yaml`, rotated by runbook 4), is `409` with the reason; Keycloak
  unreachable is `503`.

## 53. Costs per project (T-5126)

One calendar month of the installation split per project, for Administration → Costs, and the
same month as CSV. Organization administrators only (PF-03); anyone else gets `403`.

```text
GET /api/v1/organization/costs?month=2026-09          → 200
GET /api/v1/organization/costs/export?month=2026-09   → 200 text/csv
```

```json
{"month": "2026-09", "currency": "EUR", "measuredAt": "2026-10-22T01:00:00Z",
 "prices": {"currency": "EUR", "perUsd": 0.86, "nodes": [{"name": "cpx42 nbg1", "monthly": 30.0}],
            "volumes": [{"name": "postgres", "gigabytes": 300, "monthlyPerGigabyte": 0.044}],
            "fixed": [{"name": "primary IPv4", "monthly": 0.5}]},
 "projects": [
   {"project": "doprava", "node": 30.5, "storage": 0.53, "ai": 3.61, "total": 34.64, "share": 1.0,
    "spaces": [{"space": "cars", "bytes": 12884901888, "storage": 0.53}],
    "assistants": [{"assistant": "help", "questions": 120, "uncostedQuestions": 2,
                    "costUsd": 4.2, "cost": 3.61}]}
 ],
 "platform": {"node": 0.0, "storage": 12.67, "total": 12.67},
 "infrastructure": 43.7, "ai": 3.61, "total": 47.31}
```

- `month` is `YYYY-MM`, the last whole month when absent; a month that has not started, or any
  other parameter, is `400`. Without a price table (`JC_PORTAL_COST_PRICES_FILE`, the
  deployment's `portal.costPrices`) the answer is `503` naming the value to set.
- The rule: the nodes and fixed items are split by each project's share of the bytes the
  brokers measure for its spaces (entities and history, §29); a project pays its own bytes at the
  blended volume price; AI is each `AssistantDeployment`'s priced turns of the month (API/05
  §3.1) at `perUsd`. What no project carries is `platform`, so the projects and the platform add
  up to `infrastructure`, the price table, which the cloud invoice bills.
- Storage is measured when the page is read, not averaged over the month. A space or an
  assistant that did not answer counts `0` and carries its `reason`; an unpriced turn is counted
  in `uncostedQuestions`, never as `0` cost.
- The CSV has the columns `month,project,part,item,bytes,cost,currency`, one line per node
  share (`node`), space (`storage`) and assistant (`ai`), then `platform-node` and
  `platform-storage`. A cell that starts with `=`, `+`, `-` or `@` is prefixed with `'`.
- The Portal's `scripts/cost-reconcile.mjs <csv> <invoice net total>` sums every line but `ai`
  and exits `1` when the two are more than 10 % apart.

## 54. Usage per Endpoint (T-5118)

What each Endpoint answered, per UTC day and surface, for its publisher. Every gateway replica
counts the calls it answered with a status below 400, by the Endpoint slug of the path and the
surface of the route (`ngsi-ld`, `ogc`, `odata`, `sta`, `ical`, `feed`, `mcp`, `websocket`,
`download` for `file.csv`) and the kind of caller (T-6186), and posts what it counted since its
last post every five minutes. The Portal adds the counts into one row per project, Endpoint,
surface, kind of caller and day and keeps rows 400 days.

```text
GET  /api/v1/projects/{project}/endpoints/{name}/usage?days=30    → 200 EndpointUsage
GET  /api/v1/projects/{project}/usage/endpoints?days=30           → 200 ProjectUsage
GET  /api/v1/projects/{project}/usage/endpoints/export?month=2026-10 → 200 text/csv
```

```json
{"name": "air", "from": "2026-09-23", "to": "2026-10-22",
 "days": [{"day": "2026-10-22", "calls": {"ngsi-ld": 120, "download": 3}, "total": 123}],
 "totals": {"ngsi-ld": 120, "download": 3},
 "callers": {"person": 20, "agent": 100, "anonymous": 3}, "total": 123}
```

- The kind of caller comes from the bearer token the realm signed (signature, issuer, lifetime
  checked), never from one it did not: `agent` for an MCP connector's client (`mcp-*`, a client
  metadata document URL) or the Portal's assistant (`jc-assistant`), `workload` for a Keycloak
  service account or a token without a username, `person` for any other username, and
  `anonymous` for no token or one the realm did not sign. Rows counted before the split, or posted
  by a gateway that sends no kind, are `unknown`. `callers` holds the window's calls per kind in
  `EndpointUsage` and in each line of `ProjectUsage`.

- The gateway posts to the internal listener's `/internal/usage` (`204`); the body is `{"counts": [{"endpoint": "<slug>", "surface": "ogc", "caller": "person", "count": 7}]}`, at most
  5,000 lines and 10,000,000 per line; any other member, an unknown surface or kind of caller, or a slug that is no
  DNS-1123 name refuses the whole post with `400`. A slug no Endpoint has is dropped. The route
  is on the internal listener and answers the gateway's own workload token alone (PF-46); any
  other caller gets `401` before the body is read.
- `days` is 1 to 90, 30 when absent; every day of the window is in `days`, an empty one with
  `total: 0`. `ProjectUsage` lists every Endpoint the caller may read, with `daily` holding one
  total per day, oldest first.
- The export is `day,endpoint,surface,calls`, one line per day, Endpoint and surface with calls,
  every kind of caller summed, for the Endpoints the caller may read; `month` other than `YYYY-MM` is `400`.
- Who may read: whoever may read the Endpoint (`read` on `Endpoint` in its Context Space); a
  project the caller cannot read, or an Endpoint they cannot read, is `404`.
- Nothing personal is kept: the gateway sends no address, token, subject, client id, query or
  entity id, only a kind of caller, and `endpoint_usage_daily` has no column for one.

## 55. Checking a KPI over several sources (PL-102…PL-105, T-4935)

The KPI editor sends its `joinedcontext.com/v1alpha2` Pipeline draft each time typing rests. The
answer names every field jc-core refuses, with the units the sources' space models give, and
says of each source whether the pipeline's account may read it. `preview: true` also computes
the indicator on each source Endpoint's current page. Nothing is stored or written.

```text
POST /api/v1/projects/{project}/kpis/check   {pipeline, preview?}   → 200 KpiChecked
```

```json
{"problems": [],
 "sources": [{"name": "parking", "endpoint": "helsinki-parking", "readable": false},
             {"name": "stops", "endpoint": "helsinki-transit", "readable": true}],
 "preview": {"values": [{"key": "kallio", "name": "parking-per-stop-kallio", "value": 7.5}],
             "rows": {"parking": 100, "stops": 40}}}
```

- `problems` holds `{field, reason}` per refusal: the manifest rules of every Pipeline, then the
  `kpi` step's (PL-102, PL-103) with each attribute's `unitCode` from the model of the source
  Endpoint's space. A draft with problems is still `200`; the editor shows each reason beside
  its field. `preview` is computed only for a draft without problems.
- A source must name an Endpoint of the same project in a Context Space the caller may read;
  any other is refused as `no Endpoint '<name>' in project '<project>' that you may read`, the
  same words as one that does not exist, and is never read.
- `readable` is true when a Policy grants the account `{project}/pl-{name}` `retrieveOps` on the
  source's type, or the Endpoint is public. A source it cannot read is asked for with the
  pipeline access request (§48, PL-90) once the pipeline exists; the run would fail with the
  source's `403` until then (PL-104).
- The preview reads at most 100 rows per source through the Endpoint's compiled view, as the
  person (the anonymous caller on a public Endpoint), the filter preview's way (§40, EP-85):
  the query is the source's type, `q`, its `attrs` and the join key. `rows` counts what each
  page held; `problem` says in words why nothing was computed (a division by zero, an average
  over nothing, a source that did not answer, PL-104).
- A `prev` term (PL-117) adds one read per source and period: `rows` holds it under
  `parking@P7D`, at most 100 rows. The broker's view preview serves current pages only, so this
  read goes through the gateway to the Endpoint's Temporal API, with the person's token
  exchanged for the Endpoint (anonymous on a public one). A session without the person's token,
  and a read with no rows, give `problem` in words and no value.
- `readable` for a source with a `prev` term also needs `queryTemporal`; the pipeline access
  request (§48) asks for it with `operations: history`, which grants `retrieveOps` and
  `queryTemporal`.
- Who may call: a person with `propose` on `Pipeline` in the project; anyone else gets `403`,
  and so does a workload token, whose reading is its ServiceAccount's grants.

## 56. Adding a device to an MQTT broker (T-5532, PF-47, ADR-N-048 §5)

The MQTT brokers page adds a device that signs in by its own key pair. The browser makes the key
pair (WebCrypto ECDSA P-256) or takes the public half pasted as a JWKS, one JWK or a PEM public
key. The private key stays in the browser and the person's download; no request carries it.

```text
GET  /api/v1/projects/{project}/mqttbrokers/{name}/devices          → 200
POST /api/v1/projects/{project}/mqttbrokers/{name}/devices          → 202
```

```json
{"mayAdd": true,
 "roles": [{"name": "camera", "publish": ["cameras/${clientid}/#"]}],
 "tokenUrl": "https://id.example.org/realms/jc/protocol/openid-connect/token",
 "issuer": "https://id.example.org/realms/jc"}
```

```json
{"name": "sensor-001", "role": "camera", "topic": "cameras/${clientid}/#",
 "jwks": {"keys": [{"kty": "EC", "crv": "P-256", "kid": "k1", "x": "…", "y": "…"}]}}
```

- `GET` answers whoever may read the project's MqttBrokers; `roles` is empty and `mayAdd` false
  unless the caller is a person who may propose a ServiceAccount and this MqttBroker. Anybody else
  gets `404`, as for a broker the project does not declare (PF-59).
- `POST` proposes one Change in the caller's name: a ServiceAccount `{name}` whose only credential
  is `{kind: oauth-client, name: device-key, jwks}` and whose one grant is `mqtt-device` on the
  project, which names no Role and scopes no space, so it gives no Portal right and no Endpoint
  audience; and the broker with `{name}` added to `role`'s `serviceAccounts`. The Change waits for
  a second person; the Portal never approves it on proposal.
- Refusals, before anything reaches the forge: a JWK carrying a private or symmetric member (`d`,
  `p`, `q`, `dp`, `dq`, `qi`, `oth`, `k`) is `422` naming `jwks.keys[i].{member}`, and the value is
  never echoed; any other PF-47 rule (one to five keys, P-256 or RSA 2048+, unique `kid`) is `422`
  naming `jwks`; a `role` the broker lacks or a `topic` outside the role's `publish` filters is
  `422` naming the field (a role that publishes needs a `topic`); a name that is not a DNS label is
  `422` naming `name`; an existing ServiceAccount of that name is `409`; a caller who may not
  propose both kinds is `403`, and so is any bearer token of a workload or agent, which proposes
  the two manifests through `jc_resource_propose` instead.
- After approval the device signs a client assertion (RFC 7523, `iss` and `sub` its client
  `{project}-{name}`, `aud` the `issuer`) with its private key, sends it to `tokenUrl` with
  `grant_type=client_credentials`, and connects with the access token as the MQTT password.
  Rotating the key is a proposal of a new `jwks` on the account; revoking is removing the account
  from the role.

## 57. Who may do what on one Endpoint (T-5531, EP-103)

The Endpoint page's Access tab shows every principal against each type and four actions: read,
write, delete and history. The Portal decides nothing. It lists the principals the Endpoint's
space names: the `assignee` of each Policy of that space and of each of its `rules` (a `user` as
`person`, `role: public` as the public), each ServiceAccount of the project the caller may
read (a kind they cannot read stays invisible, PF-59) and the public. It
then asks the gateway's `access/simulate` with no action for each principal (§39), at most 16 at
once, with its own service-account token, never the caller's.

```text
GET /api/v1/projects/{project}/endpoints/{name}/access/matrix?kind=group → 200 Matrix
```

```json
{"endpoint": "bikes-ops", "space": "bikes",
 "principals": [{"kind": "group", "name": "bike-stewards",
                 "via": [{"kind": "policy", "name": "stewards-write"}]}],
 "types": ["BikeHireDockingStation"],
 "cells": [{"principal": "group:bike-stewards", "type": "BikeHireDockingStation",
            "action": "write", "effect": "permit", "attributes": ["status"],
            "constraint": "q: operator==\"HSL\"", "decidedBy": "stewards-write"}],
 "edges": [{"from": "principal:group:bike-stewards", "to": "policy:stewards-write"},
           {"from": "policy:stewards-write", "to": "endpoint:bikes-ops"},
           {"from": "endpoint:bikes-ops", "to": "type:BikeHireDockingStation"}]}
```

- A cell is one type and one action a permission (`effect: permit`) or a prohibition
  (`effect: prohibit`) of the principal's grant document names. An operation falls into a
  column as the UI's operation areas read it: `retrieve*` and `query*` are read, a `*Temporal*`
  operation is history, create, update, replace, append, merge, upsert and batch are write,
  delete and purge are delete; subscriptions, registrations and entity maps have no column. A
  combination without a cell is refused.
- `attributes` is `"*"` or the attribute names; `constraint` joins the grant's `q`, `scopeQ`,
  `geoQ` and `temporalQ` into one line. `decidedBy` is the first Policy in force for the
  principal, of the cell's effect, whose `information` names the type or no type, else `null`.
- A principal the gateway admits to nothing is left out. A `did` assignee is no token the
  gateway can be asked about and is left out too.
- `kind` is `person`, `group`, `role`, `serviceAccount` or `public` (`400` otherwise). More than
  50 principals is `422`, naming `kind` as the member to narrow by; the gateway is not asked.
- Who may read: an organization administrator, or whoever reads Policies in the Endpoint's
  Context Space; anybody else is `403` before the gateway is asked. An unknown Endpoint is `404`;
  no gateway, or a gateway that does not answer, is `503`.
- Each read is one `access.simulated` activity event naming the reader, the Endpoint and the
  number of principals. The answer carries rules only, never a data value.

## 58. Why a cell is not editable (T-5528, GW6, EP-55)

The entity grid's cell inspector shows, per attribute, what the person may do with it and why.
The table itself is the gateway's: the browser reads `GET /api/endpoint/{slug}/access/attributes?type=T`
([API/02 §7b](02-endpoint-representations.md)) with the person's own session through the edge,
so every chip is the decision their real request gets. The Portal holds no access token of the
person and computes no grant in the browser. This route adds one thing the gateway never says
to a caller: which Policy refused an attribute, for a caller who may read that Policy.

```text
GET /api/v1/projects/{project}/endpoints/{name}/access/attributes?type=T&attribute=A&action=write → 200 AttributeExplained
```

```json
{"type": "AirQualityObserved", "attribute": "no2", "action": "write",
 "decision": false, "reason": "prohibited",
 "policy": "no2-frozen", "policyTitle": "NO2 is corrected by the lab only", "policyHidden": false}
```

- `type` and `attribute` are required, 1 to 255 printable characters; `action` is `read`,
  `write` or `delete` (`queryEntity`, `updateAttrs`, `deleteAttrs`), `write` when absent. Any
  other value is `400` naming the parameter.
- The question goes to the gateway's `access/simulate` with the Portal's own service-account
  token (the only caller the gateway answers there) and with the caller themself as subject:
  their username, groups and realm roles from their verified session, never from the request.
  `resource.properties.attribute` carries the attribute.
- `policy` and `policyTitle` name the deciding Policy only when the caller may read that Policy
  (`read` on `Policy` in its Context Space). For anyone else both are `null` and `policyHidden`
  is `true` when a Policy decided; the gateway is then not asked at all when the caller may read
  no Policy in the project. The panel says "hidden by a policy you cannot read".
- Who may call: whoever may read the Endpoint (`read` on `Endpoint` in its Context Space). Not
  signed in is `401`; an Endpoint the caller may not read is `403`; an unknown Endpoint and a
  type a space model with classes does not list are both `404` with the same body.
- No attribute value is read: the route asks a decision, never an entity.
- `decision` and `reason` are the gateway's answer, `null` when it was not asked.
- The gateway unreachable is `503`; the panel keeps the gateway's chips and says the Policy
  could not be named.

## 59. The secret form (T-5971, MF-24, CC-06)

A credential a source or a plan needs is typed into the Portal's secret form, never into a chat,
a tool argument or a manifest. The form writes it encrypted into the project's repository.

```text
GET  /api/v1/projects/{project}/secrets   → 200 {"items": [{"name", "keys", "path"}]}
POST /api/v1/projects/{project}/secrets   → 202 Change
```

```json
{"name": "counters-api", "values": {"apiKey": "…"}}
```

- `GET` lists the names and keys every `*.enc.yaml` of the repository declares, never a value.
- `POST` encrypts the values with SOPS + age for the Portal's own recipient, and for every
  recipient the replaced file had, into `projects/{project}/secrets/{name}.enc.yaml`, and opens a
  change request a person approves. A secret that exists is replaced whole: send every key.
  `name` and each key are letters, digits, `-` and `_` (1 to 63); a value is 1 to 16 KiB; at most
  10 keys. A bad field is `422` naming it; a name another file declares is `409`.
- Both doors take a person's Portal session with `propose` on `DataSource`; a token of a
  connector, an agent or a service account is `403`. A Portal without an age identity answers
  `503`.
- The page `/projects/{project}/secrets/new?name=…&key=…` is the form; `jc_change_plan` answers
  its link in `needsSecret` for each `secretRef` the repository does not hold (API/14 §4.3).

## 60. A person's Grafana visualizations (T-6509)

```text
GET    /api/v1/projects/{project}/grafana                the saved visualizations, and whether Grafana is ready → 200
GET    /api/v1/projects/{project}/grafana/options        what the form may offer the caller → 200
POST   /api/v1/projects/{project}/grafana                save one → 201
DELETE /api/v1/projects/{project}/grafana/{id}           remove one → 204
```

- **API-601** [mvp] — The Portal writes each visualization into the project's Grafana organization with the local administrator. That credential comes from a Secret reference and never reaches a browser.
- **API-602** [mvp] — The options list only Endpoints the caller may read. `POST` checks the Endpoint, type, attribute, kind and title again and answers `403` or `422` naming the field.
- **API-603** [mvp] — A panel reads as whoever opens it. A private Endpoint reads through a datasource that forwards that person's token and that the deployment provisions; a public Endpoint reads anonymously through a datasource the Portal adds.
- **API-604** [mvp] — Saving and removing need `propose` on `Dashboard` in the project. Reading needs a role in the project, and a row shows only when the caller may read its Endpoint.
- **API-605** [mvp] — The Portal's table is the record: Grafana's database lives in its pod, and the Portal puts back a lost dashboard within five minutes.

## Related

- [00-intro](00-intro.md) — all API surfaces.
- [04-context-spaces-and-endpoints](../Architecture/04-context-spaces-and-endpoints.md) — the model behind the endpoints.
