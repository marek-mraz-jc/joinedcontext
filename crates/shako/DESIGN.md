# shako Architecture & Design (`crates/shako`)

`shako` is the platform's in-process Rust Git forge replacing Gitea 1.27.3. It serves Git smart HTTP
(upload-pack v0 and v2, receive-pack with quarantine and thin-pack completion), the Gitea REST
compatibility subset (`COMPAT.md`), native REST (`/api/v1/projects/{project}/repositories/...`),
OData 4.01 (`/odata/`), the OCI distribution and generic package registries (`/v2/` and
`/api/packages/`), MCP tools (`jc_forge_*`), push mirrors, outbox-backed webhooks, and server-rendered
read-only web pages.

## 1. Crate Dependency Graph

```
shako-core
  ├── shako-git          (pkt-line, upload-pack v0/v2, receive-pack, quarantine, send-pack, bundle, merge, archive, compact, fsck)
  ├── shako-store-memory (in-memory Store + OData MemoryTable)
  ├── shako-store-pg     (PostgreSQL Store, forced RLS, boot check, LISTEN/NOTIFY, outbox dispatcher)
  ├── shako-registry     (OCI distribution /v2/*, /v2/token, generic packages, cosign simple-signing)
  └── shako-api          (native REST, Gitea compat 39 rows, OData 4.01 service, PRs/merge, contents, webhooks, mirrors)
        ├── shako-mcp    (jc_forge_* MCP tools from OperationRegistry)
        ├── shako-web    (read-only askama HTML views, pulldown-cmark with HTML escaping, Gitea redirects)
        └── shako-server (ShakoState, router(state, mount), registry_router(state, mount), CLI binary)
```

## 2. Core Types & Traits (`shako-core`)

- **Identifiers**: `InstanceId(Uuid)`, `OrgId(Uuid)`, `RepoId(Uuid)`, `WebhookId(Uuid)`, `PackageId(Uuid)`.
- **Errors (`ShakoError`)**: maps to RFC 9457 `ProblemDetails` with stable slugs (`shako.not_found`,
  `shako.unauthorized`, `shako.forbidden`, `shako.conflict`, `shako.unprocessable`,
  `shako.behind_base_branch`, `shako.quota_exceeded`, `shako.ssrf_blocked`, `shako.overloaded`, etc.),
  OData 4.01 JSON error envelopes, and Gitea `{"message": "...", "url": "..."}` error bodies.
- **Authorization (`Authorizer`)**:
  - `Action`: `Read | Propose | Write | Approve | Admin | Publish | Build`
  - `ObjectTarget`: `Instance | Organization | Repository | Package`
  - `Decision { allowed: bool, reason: String, visible: bool }`
  - Objects outside the caller's organization or unreadable by the caller return `visible: false`,
    which translates to `404 Not Found` on every surface (smart HTTP, REST, OData, MCP, registry, web).
- **Ref Policy (`RefPolicy`)**:
  - Evaluates `BranchProtectionRule` and `TagProtectionRule` on every ref-moving path:
    `receive-pack` command phase and post-quarantine tree check, `contents` single/multi-file
    commits, `branches` create/delete, PR `merge` and `update-branch`.
  - Refuses whitelisting any subject without `Action::Write` on the repository with `422` naming the
    subject (closing the Gitea silent-drop bug).
- **Memory Budget & Semaphore (`MemoryBudget`)**:
  - One memory-weighted semaphore of 192 MiB (`192 * 1024 * 1024` bytes) governing pack generation,
    pack indexing, archive streaming, and tree merges. Requests exceeding available permits after the
    acquire timeout receive `503 Service Unavailable` with `Retry-After: 5`.

## 3. Git Engine State Machines (`shako-git`)

- **Repository Storage**:
  - Bare repositories at `{repo_root}/{instance_id}/{repo_id}.git` (paths derived strictly from UUIDs).
  - Opened with `gix::open_opts(path, gix::open::Options::isolated())` so environment variables,
    system/global gitconfigs, hooks, and `fsmonitor` are never consulted.
- **Upload-Pack v0 & v2 (`upload_pack.rs`)**:
  - v0 advertisement (`GET .../info/refs?service=git-upload-pack` without `Git-Protocol: version=2`)
    starts with `# service=git-upload-pack\n0000`, then `HEAD`, `refs/heads/*`, `refs/tags/*` (with
    peeled `^{}` lines for annotated tags), hiding `refs/pull/*`.
  - v2 advertisement (`Git-Protocol: version=2`) sends `version 2\n` and capabilities (`ls-refs=unborn`,
    `fetch=shallow filter ref-in-want sideband-all wait-for-done`, `object-info`, `server-option`,
    `object-format=sha1`). `packfile-uris` is refused.
  - Pack generation calls `odb.prevent_pack_unload()` before `gix_pack` object counting/encoding and
    streams 64 KiB sideband frames over a bounded `mpsc` channel (`capacity = 4`).
- **Receive-Pack (`receive_pack.rs`)**:
  - Acquires per-repo write lock.
  - Validates commands (`<= 1,000` refs, valid refnames, `RefPolicy` precheck, quota precheck).
  - Streams incoming pack to `{repo}.git/objects/incoming-<uuid>/` with streaming byte cap (`512 MiB`
    or remaining quota), per-object decompressed cap (`100 MiB`), and delta chain depth cap (`50`).
  - Indexes and completes thin packs via `gix_pack::Bundle::write_to_directory`.
  - Verifies full object connectivity of every new tip, fast-forward rules, protected file globs, and
    pre-receive secret scanning before moving the pack into `objects/pack/` and committing the
    `gix-ref` transaction.

## 4. PostgreSQL Schema & Forced RLS (`shako-store-pg`)

All 15 tables reside in schema `shako` with `ENABLE ROW LEVEL SECURITY` and `FORCE ROW LEVEL SECURITY`
on `(instance_id, org_id)` via `current_setting('shako.instance', true)::uuid` and
`current_setting('shako.org', true)::uuid`:
`instance`, `repo`, `protection_rule`, `pull_request`, `review`, `commit_status`, `webhook`,
`delivery`, `push_mirror`, `package`, `package_version`, `package_file`, `blob`, `quota_usage`,
`outbox`.

## 5. Memory Budget Plan (S22)

- Idle anonymous RSS target: ~30 MiB (ceiling 64 MiB).
- Bounded JWKS cache: max 64 keys, refresh cooldown 10 s.
- Per-operation pack/archive/merge semaphore: 192 MiB total across concurrent blocking tasks.
- Streaming channel buffer: `4 * 64 KiB = 256 KiB` per active pack or archive stream.
