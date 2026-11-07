# `forge` (`jc-forge`) — Multi-Tenant Git Server, Code Review, Issues & Actions

## Scope
`forge` is a pure-Rust Git hosting and collaboration product backed by PostgreSQL ref compare-and-swap (`control`/`forge` schema) and encrypted bare Git packfiles in the object store (`org/{OrgId}/git/`) powered by `gitoxide`:
- Git Smart-HTTP v2 clone, fetch, and push with branch protection and signed-commit policies.
- Repositories, branches, tags, commits, diffs, and blame.
- Pull requests, inline code reviews, required approvals, merge queues (fast-forward, squash, rebase), and status checks.
- Issues, labels, milestones, and discussions.
- Actions-compatible workflow dispatch to `factory` microVM runners and outgoing/incoming signed webhooks.

## Public API
- Git Smart-HTTP: `/{org}/{repo}.git/info/refs`, `/{org}/{repo}.git/git-upload-pack`, `/{org}/{repo}.git/git-receive-pack`
- REST API (GitHub-compatible subset + native API): `/api/v1/repos/{org}/{repo}/{pulls,issues,commits,branches,releases,hooks,actions/runs}`
- gRPC / In-Process: `jc_forge_api::ForgeService`

## Standalone Binary
`jc-forge` (standalone Git + PR + Issue server needing only PostgreSQL and filesystem/S3 object storage).

## Configuration
- `DATABASE_URL` (role `jc_forge`, schema `forge`)
- `OBJECT_STORE_URL` (`s3://...` or `file://...` for `org/{OrgId}/git/`)
- `FORGE_LISTEN_ADDR` (default `127.0.0.1:8083`)

## Standards Implemented
- Git Smart-HTTP Transfer Protocol v2
- GitHub-compatible REST & Webhook payload signatures (`X-Hub-Signature-256`)

## Origin Today
Replaces **Gitea** (Go) and Gitea Actions coordination.
