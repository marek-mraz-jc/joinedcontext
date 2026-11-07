---
sidebar_position: 4
title: "`jcctl` CLI Reference"
---

# `jcctl` CLI Reference

`jcctl` is the platform's declarative reconciler, migration engine, and configuration CLI.

## 1. Command Syntax

```bash
jcctl [COMMAND] [OPTIONS]
```

### Commands

| Command | Arguments | Description |
|---|---|---|
| **`plan`** | `--repo-dir <path>`, `--gateway-url <url>` | Compares the seed entities a space declares with what it holds and prints the repository's flags; exit code 2 if a seed is pending. Configuration is read from the repository by every component, so it has no plan of its own (CC-72) |
| **`apply`** | `--repo-dir <path>`, `--gateway-url <url>` | Replays the seed entities a space declares into it; configuration needs no apply, every component reads it from the repository (CC-72) |
| **`drift`** | `--repo-dir <path>`, `--gateway-url <url>`, `[--json]`, `[--adopt-dir <path>]` | Compares the seed entities a space declares with what it holds; exit code 2 if drifted. Needs `--gateway-url`: configuration is read from the repository, so it cannot drift (CC-21, CC-38, CC-72) |
| **`import`** | `<source> --repo-dir <path>`, `[--namespace <slug>]`, `[--org-domain <from>=<to>]`, `[--conflict fail\|skip\|replace\|rename]` | Imports a manifest or an unpacked bundle, rewritten for this project (MF-20…MF-24, PF-22) |
| **`export`** | `--repo-dir <path>`, `--project <slug>`, `--out-dir <path>` | Writes `projects/{p}/` of the checkout as a bundle: manifests without `status` or secret values, native files verbatim, and a `kind: Bundle` index (MF-16, MF-17) |
| **`validate`**| `--repo-dir <path>` | Executes local JSON Schema and Conftest validation on manifests, and refuses a write no Endpoint can serve: a write Policy no Endpoint binds, a Pipeline target that does not bind its write (PL-111) |
| **`schema export`** | `--out <dir>` | Writes the JSON Schema draft-07 of every manifest kind to `<dir>/{Kind}.json` (default `schemas/kinds`, MF-09, CC-12) |
| **`model generate`** | `--repo-dir <path> [--org-dir <organization checkout>]` | Renders every DataModel's artifacts through Model Tools and writes the ones `spec.artifacts` declares (DM-02, DM-32); a model's `org.{name}.v{major}` imports are read from `--org-dir`, or from `--repo-dir` without it, and its `project.{name}.v{major}` imports from `--repo-dir`, at the version the checkout holds, which must be of the pinned major and not a draft (DM-76) |
| **`model diff`** | `--repo-dir <path> [--org-dir <organization checkout>]` | The same render, compared with what is committed; exit 2 and the stale paths on any difference (DM-02) |
| **`model validate`** | `--repo-dir <path>` | Compiles every DataModel and reports what does not compile; writes nothing (DM-17) |
| **`model import`** | `<dataModel.Subject/Model> --out <file>` | Imports one Smart Data Models model and writes the LinkML source it becomes (DM-07…DM-11) |
| **`policy export`** | `<policy.yaml>` | Prints the Policy as an ODRL 2.2 `Set` in the `ngsi-ld:` profile (§7, R52, ADR-N-057) |
| **`policy import`** | `<set.jsonld>`, `[--project <slug>]`, `[--space <name>]`, `[--name <name>]`, `[--out <file.yaml>]` | Writes the Policy manifest an ODRL `Set` in the profile says, or refuses it naming the member the profile does not map (§7, ADR-N-057 decision 4) |
| **`workspace render`** | `--repo-dir <path>`, `--prefix <ws-name->`, `[--out-dir <dir>]` | The workspace preview render of the checkout: `prefix` in front of every project namespace, space segment and id of this organization; one file per manifest under `<dir>`, else one YAML stream on stdout. Refused when a name would stay unprefixed (CC-78, PF-83) |
| **`workspace diff`** | `--base-dir <checkout of the base>`, `--repo-dir <checkout of the workspace>`, `[--json]` | What the workspace creates, changes (field by field) and removes against its base, as the Portal's compare lists it; exit 2 when it changes something (CC-79) |
| **`model infer`** | `--file <sample.csv\|xlsx\|json\|pdf>`, `[--url <url>]` | Reads one sample file and proposes the LinkML model it fits (DM-07) |
| **`roles render\|seed\|input`** | `--repo-dir <path>`, and for `input` `--base-dir`, `--changes`, `--author` | The role taxonomy: what the repository declares, seeding it, and the input a change's reviewers are computed from |
| **`pipeline test`** | `--pipeline <manifest.yaml>`, `--sample <file>`, `[--format csv\|json\|text]`, `[--capture <url>]` | Runs one pipeline over one sample without a runner, which is what the Portal's studio calls |
| **`artifacts rebuild`** | `--repo-dir <path>`, `--out-dir <dir>`, `[--space <name>]`, `[--revision <sha>]` | Re-renders a space's committed artifacts into a directory |
| **`sync`** | `--repo-dir <path>`, `--source <project>/<name>`, `--checkout <dir>`, `[--state <file>]`, `[--once]`, `[--json]` | Runs one `SyncSource` against a checkout, once or on its schedule |
| **`publish ckan`** | `--repo-dir <path>`, `--project <slug>`, `--host <gateway host>`, `[--organization-title <t>]`, `[--api-token-env <VAR>]`, `[--age-key-file <path>]`, `[--withdraw]` | Publishes the project's open datasets to a CKAN portal, or withdraws them |
| **`migrate`** | `--repo-dir <layout 1 clone>`, `--out-dir <empty dir>` | Splits each `projects/{slug}/` into a project repository of its own with its history, and writes the organization repository of layout 2 with the registry in their place (CC-85) |
| **`export --format git`** | `--repo-dir <project checkout>`, `--project <slug>`, `--out-dir <dir>`, `[--app-dir <name>=<checkout>]`, `[--org-dir <organization checkout>]` | One `git bundle` per repository of the project, its registry entry without values, a copy of every organization model the project imports (read from `--org-dir`, refused without it), and a `kind: Bundle` index of roles, SHA-256, head commits and carried models (MF-45, MF-49) |
| **`import --format git`** | `<dir> --out-dir <empty dir>` | Clones each bundle, refuses the import unless every file and head is the one the index lists, and migrates a layout 1 organization on the way in (MF-46, MF-47) |
| **`login`** | `--idm <issuer url>`, `[--token-file <path>]` | Signs a person in by the device flow (RFC 8628) with PKCE (RFC 7636, S256) on the Keycloak client `jcctl` and writes their Portal token to a file only they may read (PF-45, MF-14) |
| **`token`** | `--resource <endpoint url>`, `--idm <issuer url>`, `[--token-file <path>]` | Signs a person in the same way for one Endpoint and writes a token the gateway admits on that Endpoint alone, decided by the person's Policy: a script reads as the person who runs it (PF-45, PF-46, §2b) |
| **`get`** | `<plural> [<name>]`, `--project <slug>`, `[-o name\|yaml\|json]`, `[-l <labelSelector>]` | Reads live manifests from the Portal: a list follows `continue` to the end; `-o name` (the default) prints `{plural}/{name}` per line (MF-14) |
| **`describe`** | `<plural> <name>`, `--project <slug>` | One live manifest for a person to read: its identity and title, then `spec` and `status` as YAML (MF-14) |
| **`apply -f`** | `<file>`, `[--project <slug>]` | Proposes every manifest of the file as a `Change`: `POST` when the Portal holds no such resource, `PUT` when it holds a different one, nothing when it already matches; exit `1` when any was refused (MF-12, MF-14) |
| **`diff -f`** | `<file>`, `[--project <slug>]` | Compares every manifest of the file with the live one, member by member as `plan` does; exit `2` on any difference; writes nothing (MF-14, CC-17) |
| **`delete -f`** | `<file>`, `[--project <slug>]` | Proposes the deletion of every resource the file names as a `Change` on the deletion lane (MF-12, MF-14) |
| **`plan -f`** | `<file>`, `[--project <slug>]` | Sends every manifest of the file as its write with `dryRun=All`: the Portal validates and diffs it and writes nothing; prints the lane and each changed field, or every finding of a manifest it would refuse; exit `1` when any would be refused, `2` when any would change (MF-13, MF-14) |
| **`ops`** | `--project <slug>` | Lists the registry operations the caller may run in the project, one `name<TAB>description` per line: the same list MCP clients and the Portal assistant read ([01](01-portal-api.md)) |
| **`op`** | `<operation>`, `--project <slug>`, `[-f <input.yaml\|json>]` | Runs one registry operation with the object the file holds (`{}` without one) and prints its answer as JSON; a write answers the `Change` it proposed; exit `1` on a refusal, which prints the problem's `detail` and each of its `errors` |
| **`changelog`** | `--repo-dir <project checkout>`, `[--days <1..366>]`, `[--format md\|json]` | Lists the merged changes of every public Endpoint on the first-parent history of `main` in the window, per surface, from the manifests alone: no commit message or author ([API/10 §4](10-api-change-policy.md#4-the-public-changelog), EP-148, EP-149) |
| **`checkouts`** | `--org-dir <organization checkout>`, `--projects-dir <dir>`, `--forge <base>/<org>`, `[--token-file <path>]`, `[--secrets-dir <dir>]`, `[--once]`, `[--interval <seconds>]` | Keeps every registered project checked out at its `spec.ref` under `<dir>/{slug}`, a link swapped in one rename; a failed fetch keeps the last checkout, and `--once` fails only when the registry does not read (CC-86, CC-89) |

`jcctl --help` prints the same list; `crates/jcctl/src/main.rs` holds it as one `USAGE` string, so a
command that is not in that string does not exist. **There is no `jcctl serve`**: the one
`jcctl` that runs as a daemon is `checkouts`, the gateway's sidecar in its own image with git, and
there is no configuration MCP server behind it. The MCP servers the
platform has are the Portal's at `/api/v1/mcp` and an endpoint's or space's own; see
[00-intro](00-intro.md).

`plan` and `import` take `--json` as well, and `apply` takes `--prune` and `--confirm-deletions`;
`export` takes `--revision <sha>`.

## 2. Connecting to a platform

`plan`, `apply` and `drift` read and write live state, so each needs one address and one
identity. Both come from the command line or the environment and never from a manifest:

| Flag | Environment | Meaning |
|---|---|---|
| `--gateway-url <url>` | `JC_GATEWAY_URL` | Base URL of the Context Gateway, e.g. `http://context-gateway.prod.svc.cluster.local:9090` |
| `--token-file <path>` | `JC_TOKEN_FILE` | File holding the reconciler's access token; a projected ServiceAccount token in the cluster |

There is no flag for a broker address. Every write goes through the gateway under the reconciler's
own ServiceAccount, which holds write grants as ordinary `Policy` entities like any other client
(CC-04); a reconciler that could address a broker directly would be a policy bypass with a command
line flag in front of it. The gateway serves each space's CIM 009 tree at
`/cs/{space}/ngsi-ld/v1/`, byte for byte, so the reconciler is a stock NGSI-LD client and uses no
broker-specific call (CC-16, SP-03).

Replaying into a **second** broker, which is what a blue/green upgrade and a restore rehearsal
both do, therefore means putting a second gateway in front of it and pointing `--gateway-url`
there. The gateway is stateless and reads its tables from the configuration repository, so a green
gateway costs one more release of the same chart and keeps the policy layer in the path while the
blue stack still serves traffic (CC-50, CC-51).

Without `--gateway-url` the command works on the repository alone: `validate`, `export`, `schema export` and
the `model` verbs need no platform, and `plan` says so rather than comparing against an empty
world.

Neither `plan` nor `apply` compares or replays configuration manifests: the gateway, the Portal and
the reconciler read them from the repository (CC-72), so a manifest is live the moment its commit
is, and a comparison against a platform store that does not exist would report every manifest as
`CREATE` on every run. Both report the configuration half as read from the repository and decide
their outcome on the seed half alone, as `drift` does.

### 2a. Talking to the Portal (`get`, `describe`, `apply -f`, `diff -f`, `delete -f`)

The kubectl-shaped verbs are a client of the Portal resource API ([01 §4](01-portal-api.md#4-resource-api-mf-11mf-15)), not of a
repository checkout, and take their own address and identity:

| Flag | Environment | Meaning |
|---|---|---|
| `--server <url>` | `JC_SERVER` | Base URL of the Portal, e.g. `https://portal.dev.joinedcontext.com` |
| `--token-file <path>` | `JC_TOKEN_FILE` | File holding an OIDC access token for the Portal: a person's from `jcctl login` (PF-45), or a `ServiceAccount`'s from the client credentials grant (PF-34); without either, the file `jcctl login` wrote |
| `--idm <url>` (`login` and `token`) | `JC_IDM` | The realm's issuer, e.g. `https://idm.<domain>/realms/<realm>`; `https` except on the loopback interface |

`jcctl login` runs the device flow on the public Keycloak client `jcctl`, which offers that
grant and no other and whose tokens carry the `portal-api` audience the Portal accepts. The
client is public, so each sign-in carries a proof key (RFC 7636, `S256`): a fresh 32-byte
verifier, its hash on the device authorization and the verifier on every token request; the
realm refuses a device authorization without one. It prints the address and the code to open on any device, polls the token endpoint at the interval
the provider names (five seconds more on `slow_down`), and writes the access token to
`--token-file`, else `$XDG_CONFIG_HOME/jcctl/token` (`~/.config/jcctl/token`), with mode `600` in
a directory of mode `700`. The refresh token is not kept: when the token expires, the person
signs in again. A declined or expired sign-in writes nothing and exits `1`.

The token is read from a file so that it never stands in a process list or a shell history, and
it is never repeated in an error: an answer that echoes it has it replaced by `[redacted]`. A URL
carrying a user name or password is refused.

A manifest's project is `--project`, else its `metadata.namespace`; a file whose manifest names
another project than `--project` is refused before anything is sent. Every write is a proposal:
the Portal answers `202` with a `Change`, and the command prints one line per manifest,
`{plural}/{name}: {created|replaced|deleted}, {Change name} {phase} ({lane} lane)`. A bearer
caller never approves on propose (API/01 §5), so the `Change` waits for a person on the approval
page; `delete -f` proposes on the deletion lane and never removes anything by itself (MF-12). A
refusal prints the API's own `detail` and the command goes on with the next manifest, exiting `1`
at the end.

The routes these verbs call are one table in jcctl (`jcctl::portal::ROUTES`), and jcctl carries
the part of the Portal OpenAPI they depend on, extracted at a pinned Portal commit by
`scripts/vendor-portal-api.py` into `crates/jcctl/portal-api.json`. A test fails when a route,
method or query parameter of the table is not in that document, or when the Portal serves a kind
jcctl cannot name, so the CLI, REST and the MCP tools cannot drift apart unnoticed.

`get -o yaml` and `-o json` print the manifest as the API serves it. The Portal serves what the
repository holds, which carries a `secretRef` naming a secret and never its value (CC-06), so
no output of these verbs carries a resolved secret.

### 2b. A person's script on one Endpoint (`token`)

A script that reads a non-public Endpoint as the person who runs it takes a token from
`jcctl token`:

```sh
jcctl token --resource https://api.dev.joinedcontext.com/api/endpoint/<slug> \
  --idm https://idm.dev.joinedcontext.com/realms/dev
curl -H 'Accept: application/ld+json' \
  -H "Authorization: Bearer $(cat ~/.config/jcctl/endpoints/<slug>)" \
  'https://api.dev.joinedcontext.com/api/endpoint/<slug>/ngsi-ld/v1/entities?type=<Type>&limit=5'
```

`--resource` is the Endpoint's address as its page shows it; the slug is the path segment after
`/api/endpoint/`, and an address without one is refused before anything is sent. The command
runs the device flow of §2a for the scope `openid endpoint:{slug}` and writes the token to
`--token-file`, else `$XDG_CONFIG_HOME/jcctl/endpoints/{slug}`, with the same modes. The token
carries the audience `jcctl-endpoint` beside `portal-api`; the gateway admits a `jcctl-endpoint`
token from the client `jcctl` only on the Endpoint its `endpoint:{slug}` scope names, and the
Endpoint's Policy decides each call by the person (PF-46). Any other Endpoint answers `401`, an
Endpoint whose Policy does not grant the person `403`; `endpoint:*` reaches nothing there. A
token of `jcctl login` alone carries no Endpoint scope and reaches no Endpoint.

Decided (worker-t5614, owner may override): a second sign-in per Endpoint, not an RFC 8693
token exchange of the `jcctl login` token, because Keycloak's standard token exchange takes only
confidential clients and `jcctl` lives on a laptop without a secret; the browser session the
person already has turns the second sign-in into one confirmation. The `endpoint:{slug}` scopes
are the ones the Portal renders for the MCP hub (Endpoints serving MCP); the admin-permissions
Job links each to `jcctl` as an optional scope.

A program that runs without a person keeps using a `ServiceAccount` (PF-34).

## 3. JSON Output Contract (`--json`)

```json
{
  "summary": {
    "to_add": 1,
    "to_change": 0,
    "to_delete": 0
  },
  "changes": [
    {
      "kind": "Subscription",
      "id": "alert-airquality-pm25",
      "action": "CREATE",
      "diff": {
        "spec": {
          "q": "pm25 > 35"
        }
      }
    }
  ],
  "flags": [
    "ContextSourceRegistration/regional-hub: https://other.example/ngsi-ld/v1 is a broker, not a joinedcontext endpoint, so no schema surface is derived and no model is mirrored (DM-48)"
  ]
}
```

`summary` and `changes` describe configuration manifests and stay zero and empty while every
component reads them from the repository (section 2, CC-72); the seed half is printed as text.
`flags` carries what the plan noticed and will not act on: a peer with no schema surface, a
reference whose model cannot be mirrored, anything a reviewer should read before approving
rather than discover after applying (DM-48). It is advisory and never changes the exit code —
a run with flags and no changes still exits `0`. The array is always present and is empty when
there is nothing to say, so a caller reads `flags` without checking whether the key exists.

## 4. Exit Codes

- `0`: Success / No changes pending (for `plan`) / No drift detected (for `drift`).
- `1`: Operational error (network, authentication failure, invalid syntax).
- `2`: Succeeded with pending changes (for `plan`, `workspace diff` and `diff -f`) or drift detected (for `drift`). A stale committed artifact is a pending change, so `model diff` uses the same `2`.

## 5. `model` — Data Model Artifacts

`jcctl model` is the command line over Model Tools ([Architecture/11 §6.5](../Architecture/11-data-models.md)). CI, an agent and the Portal preview call one implementation, so a committed artifact and a preview cannot disagree about what a model compiles to (DM-32).

The service URL comes from `--url` or `JC_MODEL_TOOLS_URL`, never from a manifest, and the command holds no credentials. Before `generate` or `diff` writes or compares anything it reads `GET /healthz` and refuses a Model Tools whose `generatorVersion` differs from `modelTools.generatorVersion` in `platform-settings.yaml` (DM-19).

Every verb prints one JSON object on stdout, so a CI step reads the result instead of the log:

```json
{
  "generatorVersion": "linkml-1.11.1",
  "models": [
    {
      "id": "DataModel/mobility/parking-sensor",
      "linkml": "projects/mobility/spaces/parking/datamodels/parking-sensor.linkml.yaml",
      "written": ["json-schema/parking-sensor.v1.json"],
      "stale": ["context/parking-sensor.v1.jsonld"],
      "missing": [],
      "errors": []
    }
  ],
  "stale": 1,
  "errors": []
}
```

`written` is what `generate` wrote (empty for `diff` and `validate`), `stale` the committed artifacts that no longer match, `missing` the ones `spec.artifacts` declares and the answer does not carry, and `errors` what the model does not compile to. An artifact `spec.artifacts` does not declare is not rendered into the repository; a declared one the service does not render is `missing` and fails the run rather than being written empty.

## 6. `drift` and `import`

`drift` is `plan` read the other way round: run against a platform nobody was supposed to
change, a non-empty plan means somebody changed it (CC-21).

**What it compares is the seed entities, and only those.** Configuration is read from the
repository by every component that acts on it (CC-72), so the repository *is* what is running
and a manifest cannot drift away from itself; there is no second store to compare a `ContextSpace`
or an `Endpoint` against. `--gateway-url` is therefore what the command needs to do its work, and
without one it says so rather than reporting a clean run. The resource comparison below is kept
for the day a live configuration store exists — it is what `export` and adoption are built on —
and nothing calls it today.

Every drifted resource carries the
two resolutions UI-26 puts on the screen — **revert**, the manifest Git declares, and **adopt**,
the live state cleaned into a manifest — and where one of them would be a deletion it is absent
with its reason, because nothing removes a resource outside the explicit-deletion path (CC-19).
`--adopt-dir` writes the adoptable manifests at the paths their kinds prescribe, which is the
material a change proposal is made of (CC-38, CC-68). A credential the platform hands back is
dropped on the way, named on stderr, exactly as `export` drops it (MF-17).

Unmanaged sandbox spaces are never drift: they are created live through the green lane, carry a
TTL and are never backed by Git (CC-67). A space the repository *does* declare is managed
configuration whatever its `isSandbox` says.

```json
{
  "summary": { "checked": 12, "drifted": 1, "sandboxed": 2 },
  "drifted": [
    {
      "kind": "Endpoint", "id": "Endpoint/mobility/public-air", "drift": "MODIFIED",
      "diff": [{ "path": "spec.audience", "declared": "public", "live": "shared" }],
      "resolutions": ["revert", "adopt"], "unavailable": null, "redactions": []
    }
  ]
}
```

`drift` is `MODIFIED` when both sides have the resource and a member Git owns differs, `MISSING`
when Git declares it and the platform does not have it, and `UNEXPECTED` the other way round.
Which members Git owns is CC-69's question. A manifest answers it with its
`joinedcontext.com/managed-attributes` annotation; a seed entity, which is a plain NGSI-LD
entity and carries no annotations, answers it by what it declares: the attributes in the file are
the attributes Git owns, and live telemetry beside them is never drift.

`import` reads one manifest file or the directory an exported bundle unpacks to; a compressed
archive is refused with the instruction to extract it, because unpacking is what `tar` is for
and an importer that opened archives itself would be a second place for a path-traversal bug to
live. The rewrites are explicit arguments and never guessed: `--namespace` moves every
project-scoped resource and every typed reference that named the source project, and
`--org-domain <from>=<to>` rewrites the `{orgDomain}` segment of a `urn:ngsi-ld:` identifier
(or an anchored `idPattern`) only where that segment is `<from>`, and nothing else: a reference to
another organization's Entity or Endpoint keeps its domain. A bare domain without `<from>=` is
refused with the usage (MF-22, PF-22). Every imported manifest is tagged `joinedcontext.com/imported-from` with
the document it came from (MF-20).

MF-24 is a gate and not a warning: a literal credential, an `apiVersion` this platform does not
serve, a kind it does not know, or a reference that resolves neither inside the bundle nor in
the destination refuses the whole run, and a refused run writes nothing. Half an import is a
repository that no longer validates, and the half that landed is the half nobody reviewed.
`--conflict` says what happens when the destination already declares a resource: `fail` (the
default, because the other three each discard something), `skip`, `replace`, or `rename`, which
writes it beside the existing one under the next free name.

## 7. `policy export` and `policy import`

The two verbs translate between a Policy manifest and an ODRL 2.2 `Set` in the `ngsi-ld:` profile, the dialect the broker's access document speaks ([API/02 §7b](02-endpoint-representations.md), R52). The Portal's Policy form reads the same mapping; the YAML file stays the only source, so `import` writes a manifest for review and changes nothing live ([ADR-N-057](../Decisions/adr-n-057-policy-rules-policy-refs-authzen.md) decision 4).

`export` writes one ODRL rule per entity selector, as the broker does: the CIM 009 operations and the platform actions (`readVersions`, `personalData`) as `ngsi-ld:` actions, the assignee as a typed party (`{"@type": "ngsi-ld:Role", "uid": "public"}`; `ngsi-ld:User`, `Group`, `ServiceAccount`, `Did`), the Entity type as the target with `ngsi-ld:id`, `ngsi-ld:idPattern` and the attribute names as refinements (`ngsi-ld:attrs` when the property and relationship names are one list), and `q`, `scopeQ`, `geoQ`, `temporalQ` verbatim as constraints. The Policy's `validity` is `ngsi-ld:validity` (`gteq` from, `lt` to), because the profile's `dateTime` already stands for `temporalQ`. The `Set`'s `uid` is `urn:joinedcontext:policy:{project}:{space}:{name}`, and `conflict` is `prohibit`.

`import` reads that, and the broker's own documents: a DID assignee on the `Set`, `geoQ` as a geometry under `ngsi-ld:within` (or `contains`, `intersects`, `disjoint`, `equals`, `overlaps`), and `temporalQ` as `dateTime` bounds. Consecutive rules with the same assignee, actions and constraints become one rule, consecutive selectors with the same names (and `partOf`, when export named the `information` entry) one entry, so every seeded Policy exported and imported is the same manifest; one rule writes the single-rule shape, several write `rules[]`. A `Set` written elsewhere names no manifest: give `--project`, `--space` and `--name`, which also override the `uid`. `--out` never overwrites a file.

Nothing that does not map is dropped, because a dropped duty or constraint would grant more than the document says. The refusal names the member, for example `permission[0].duty`: a duty, obligation, remedy or consequence; a `conflict` other than `prohibit`; an `Offer` or `Agreement` (import the `Set` it grants); an `@context` other than ODRL's and `https://joinedcontext.com/odrl/ngsi-ld/v1/context.jsonld`; an action that is not a CIM 009 operation, group or platform action; an unknown `leftOperand` or an operator it does not take; a plain-string assignee other than a DID; a rule `assigner` other than the `Set`'s; rules with different validity; and any member the profile does not list. The result is then validated like any manifest. The profile's context and closed SHACL shapes are `schemas/odrl/ngsi-ld-v1/` in the platform repository; CI checks that the shapes accept every exported `Set` and refuse the same documents `import` refuses.

## Related

- [00-intro](00-intro.md) — all API surfaces.
- [04-context-spaces-and-endpoints](../Architecture/04-context-spaces-and-endpoints.md) — the model behind the endpoints.
