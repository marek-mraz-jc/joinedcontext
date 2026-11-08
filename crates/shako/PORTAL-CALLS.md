# Every forge call the Portal makes, and what shako answers (2026-11-08)

Inventory of `crates/portal/src` against shako (owner rule 2026-11-08: Gitea-compatible API and
OData, no GitHub API). Every call goes through one client, `GiteaClient` in
`crates/portal/src/git/gitea.rs`, except the SyncSource fetcher (`sync/remote.rs`) and the
webhook receivers (`api/webhook.rs`, `api/sync_sources.rs`). The client sends
`Authorization: token {JC_GITEA_TOKEN}` (or `JC_GITEA_APPS_TOKEN` for the applications'
organization `JC_GITEA_APPS_OWNER`) and `Accept: application/json`, and reads only the fields
named in the "Portal parses" column.

`crates/shako/server/tests/portal_contract.rs` drives each row with the Portal's own
`GiteaClient` (built by `GiteaClient::from_env` from `JC_GITEA_*`) over TCP against shako mounted
at `/git` beside `/v2`, as `jc` mounts it, with real bare repositories on disk. Webhooks go to the
Portal's real `/api/v1/webhooks/gitea` route (`server::app`), which checks the HMAC with the
Portal's own code. Test names below are functions of that file.

Status: **served** = the Portal's call works and its parse holds; **no-op** = answered in the
shape the Portal parses, with no effect (access follows RoleBindings); **gap** = not served.

## Repositories

| # | Portal method (`git/gitea.rs`) | Call | Portal parses | Main callers | Status | Test |
|---|---|---|---|---|---|---|
| 1 | `ensure_repository` :1039 | `GET /api/v1/repos/{o}/{r}`, then `POST /api/v1/orgs/{o}/repos` `{name, description, private, auto_init: true, default_branch: "main"}`; 409 = exists | status only | `api/projects.rs:670`, `api/app_from_template.rs:395`, `git/github_mirror.rs:157` | served | `repository_branches_contents_and_history` |
| 2 | `create_empty_repository` :1110 | same with `auto_init: false` | status only | `api/import_git.rs:453`, `api/app_transfer.rs:334` | served | `bundle_push_bundle_tags_and_migrate` |
| 3 | `migrate_repository` :1073 | `POST /api/v1/repos/migrate` `{clone_addr: "{base}/{o}/{r}.git", service: "gitea", auth_token, repo_owner, repo_name, private, mirror: false, ...}` | status; 409 = exists | `api/projects.rs:807` | served (intra-forge `clone_addr` only) | `bundle_push_bundle_tags_and_migrate` |
| 4 | `default_branch` :1252 | `GET /api/v1/repos/{o}/{r}` | `default_branch` | 48 callers, e.g. `space_move.rs:278`, `api/secrets.rs:86` | served | `repository_branches_contents_and_history` |
| 5 | `archive_repository` :1752 | `PATCH /api/v1/repos/{o}/{r}` `{archived: true}` | status | `reconciler/daemon.rs:1554` | served | `repository_branches_contents_and_history` |
| 6 | `delete_repository` :1597 | `DELETE /api/v1/repos/{o}/{r}`; 404 = done | status | `api/projects.rs:431,714,858`, `api/app_from_template.rs:401` | served | `repository_branches_contents_and_history` |
| 7 | `let_the_reader_in` :1763 | `PUT /api/v1/repos/{o}/{r}/collaborators/{JC_GITEA_READER}` `{permission: "read"}` | status | `api/projects.rs:710,830`, `api/import_git.rs:515` | no-op (records a transition warning) | `repository_branches_contents_and_history` |
| 8 | `protect_branch` :1779 | `POST /api/v1/repos/{o}/{r}/branch_protections` `{rule_name, enable_push: false}` | status | `api/projects.rs:709,829`, `api/app_transfer.rs:421`, `api/app_from_template.rs:260` | served; a contents write to the branch then answers 403 | `repository_branches_contents_and_history` |

## Branches, trees, contents, history (reconciler listing and staging, every proposal)

| # | Portal method | Call | Portal parses | Main callers | Status | Test |
|---|---|---|---|---|---|---|
| 9 | `branch_head` :1864 | `GET /api/v1/repos/{o}/{r}/branches/{branch}` (slashes in the name) | `commit.id` | `reconciler/daemon.rs:735,1084`, `ops/previews.rs:176` | served | `repository_branches_contents_and_history` |
| 10 | `list_tree_blobs` / `list_tree` :1810 / :1792 | `GET .../git/trees/{sha}?recursive=true&per_page=1000&page=N` (a branch with `/` resolved through row 9 first) | `sha`, `tree[].{path,type,sha}`, `truncated` | `reconciler/staging.rs:186`, `api/export.rs:212`, `api/delete.rs:37` | served | `repository_branches_contents_and_history` |
| 11 | `commit_state` :1877 | `GET .../commits/{sha}/status` | `state`, `total_count` | `api/changes.rs:1851` | served | `repository_branches_contents_and_history` |
| 12 | `create_branch` :1896 | `POST .../branches` `{new_branch_name, old_branch_name}` | status | `api/mutate.rs:314`, `ops/workspaces.rs:952` | served | both change tests |
| 13 | `delete_branch` :1908 | `DELETE .../branches/{branch}`; 404 = done | status | `api/mutate.rs:339`, `ops/workspaces.rs:738` | served | `repository_branches_contents_and_history` |
| 14 | `get_file` :1918 | `GET .../contents/{path}?ref=`; 404 = absent | `sha`, `content` (base64, whitespace allowed) | `reconciler/staging.rs:314`, `reconciler/daemon.rs:2743` (68 callers) | served | `repository_branches_contents_and_history` |
| 15 | `put_file` :1950 | `PUT .../contents/{path}` `{content, message, branch, sha?, author, committer}` | `commit.sha` or `sha` | `api/mutate.rs:1346`, `reconciler/daemon.rs:2750` | served; stale `sha` = 409 | `repository_branches_contents_and_history` |
| 16 | `change_files` / `update_files` / `write_files` :1980 / :2011 / :2033 | `POST .../contents` `{files: [{operation: upload\|create\|update\|delete, path, content?, sha?}], message, branch, author, committer}` | `commit.sha` | `api/import.rs:1559`, `api/restore.rs:187`, `reconciler/representations.rs:157`, `api/datamodels.rs:1471` | served; all or nothing, one stale `sha` = 409 and the branch unmoved | `repository_branches_contents_and_history` |
| 17 | `delete_file` :2080 | `DELETE .../contents/{path}` `{message, branch, sha, author, committer}` | `commit.sha` (optional) | `sync/proposal.rs:96` | served | `repository_branches_contents_and_history` |
| 18 | `list_commits` :2104 | `GET .../commits?sha=&path=&limit=&stat=false&verification=false&files=false` | `[].sha`, `commit.message`, `commit.author.{name,email,date}` | `reconciler/pipeline_authors.rs:248`, `api/export.rs:1137` | served | `repository_branches_contents_and_history` |
| 19 | `commit_message` :2313 | `GET .../git/commits/{sha}` | `commit.message` | `reconciler/pipeline_authors.rs:254`, `api/changes.rs:1476` | served | both |

## Merge requests (proposals, approvals, rejections)

| # | Portal method | Call | Portal parses | Main callers | Status | Test |
|---|---|---|---|---|---|---|
| 20 | `create_pull_request` :2202 | `POST .../pulls` `{head, base, title, body}` | pull JSON (below) | `api/blueprints.rs:291`, `api/secrets.rs:338` (19 callers) | served; a second open pull for one head and base = 409 (**fixed in this run**) | `merge_requests_reviews_merge_and_webhooks` |
| 21 | `list_pull_requests[_page]` :2174 / :2179 | `GET .../pulls?state=&sort=recentupdate&page=&limit=` | `[]` pull JSON | `references.rs:297`, `api/changes.rs:1304` | served | same |
| 22 | `pull_request` :2301 | `GET .../pulls/{n}` | pull JSON | `api/changes.rs:120`, `ops/plans.rs:1431` | served | same |
| 23 | `pull_request_files` :2228 | `GET .../pulls/{n}/files?page=&limit=50` | `[].filename`, `status` (`added`, `deleted`) | `api/restore.rs:113`, `ops/plans.rs:1541` | served | same |
| 24 | `review` :2345 | `POST .../pulls/{n}/reviews` `{event: COMMENT, body}` (the author's own pull) | status | `api/changes.rs:3130` | served | same |
| 25 | `review_bodies` :2328 | `GET .../pulls/{n}/reviews` | `[].body` | `api/changes.rs:1484` | served | same |
| 26 | `close_pull_request` :2360 | `PATCH .../pulls/{n}` `{state: "closed"}` | status | `api/changes.rs:3134`, `sync/proposal.rs:125` | served | same |
| 27 | `update_pull_request` :2377 | `POST .../pulls/{n}/update?style=merge` | status (Gitea's 500 when not behind is tolerated by the caller) | `api/changes.rs:2816` | served (200 when up to date) | same |
| 28 | `merge` :2391 | `POST .../pulls/{n}/merge` `{Do: merge\|squash, merge_message_field, head_commit_id?}` | status; moved head = 409 | `api/changes.rs:2775` (squash), `agents/repository.rs:198`, `api/app_dashboard.rs:1069` (merge) | served | same |

Pull JSON the Portal parses (`GiteaPullResponse`, `git/gitea.rs:608`): `number`, `html_url`,
`state`, `title`, `body`, `head.{ref,sha,repo_id}`, `base.{ref,sha,repo_id}`, `created_at`,
`user.{login,full_name,email}`, `mergeable`, `merged`, `merge_commit_sha`, `closed_at`,
`merge_base`. The test checks each, including `same_repository` (equal `repo_id`s).

## Copies, transfers, exports

| # | Portal method | Call | Portal parses | Main callers | Status | Test |
|---|---|---|---|---|---|---|
| 29 | `bundle` :2257 | `GET .../archive/{ref}.bundle` | bytes (v2 bundle) | `api/export_git.rs:97` | served | `bundle_push_bundle_tags_and_migrate` |
| 30 | `push_bundle` :1145 | `POST /{o}/{r}.git/git-receive-pack` with a hand-built `report-status` body | `unpack ok`, `ng` lines | `api/import_git.rs:468`, `api/app_transfer.rs:404` | served | same |
| 31 | `list_tags` :2268 | `GET .../tags?page=&limit=50` | `[].name`, `commit.sha` | `api/export_git.rs:113`, `api/projects.rs:1063` | served | same |

## Applications' organization: mirrors, packages, images

| # | Portal method | Call | Portal parses | Main callers | Status | Test |
|---|---|---|---|---|---|---|
| 32 | `push_mirrors` / `add_push_mirror` / `delete_push_mirror` / `sync_push_mirrors` :1200 / :1216 / :1236 / :1244 | `GET`/`POST .../push_mirrors`, `DELETE .../push_mirrors/{name}`, `POST .../push_mirrors-sync` | `[].{remote_name,remote_address,last_error}` | `git/github_mirror.rs:159-177` | served; the password never comes back | `push_mirrors_teams_and_permissions` |
| 33 | `put_generic_file` / `get_generic_file` :1462 / :1484 | `PUT`/`GET /api/packages/{o}/generic/{pkg}/{ver}/{file}` | status, bytes (capped) | `apps/built.rs:355,358`, `apps/fetch.rs:113` | served; overwrite = 409 | `generic_packages_and_container_images` |
| 34 | `push_image` (+ `registry_token` :1507) :1541 | `GET /v2/token?service=container_registry&scope=repository:{o}/{img}:push,pull` (Basic owner:token), `HEAD /v2/{o}/{img}/blobs/{d}`, `POST .../blobs/uploads/?digest=`, `PUT .../manifests/{tag}` | `token`; `Docker-Content-Digest` | `apps/built.rs:279` | served, under the forge mount (`{JC_GITEA_URL}/v2/...`) | same |

## Teams and permissions (layout 2)

| # | Portal method | Call | Portal parses | Main callers | Status | Test |
|---|---|---|---|---|---|---|
| 35 | `owns_organization` :1618 | `GET /api/v1/user`, `GET /api/v1/users/{login}/orgs/{org}/permissions` | `login`; `is_owner` | `reconciler/project_teams.rs:64` | served: always `is_owner: false`, so the reconciler leaves teams alone (`LeftToTheBootstrap`); access follows RoleBindings | `push_mirrors_teams_and_permissions` |
| 36 | `org_teams` / `create_team` / `delete_team` / `team_repositories` / `set_team_repository` :1648-1731 | `GET`/`POST /api/v1/orgs/{o}/teams`, `DELETE /api/v1/teams/{id}`, `GET /api/v1/teams/{id}/repos`, `PUT`/`DELETE /api/v1/teams/{id}/repos/{o}/{r}` | `[].{id,name,description}`; `[].name` | `reconciler/project_teams.rs:78-164` (not reached while row 35 is false) | no-op | same |

## Webhooks (forge to Portal)

| # | Portal side | Call | Portal checks | Status | Test |
|---|---|---|---|---|---|
| 37 | `api/webhook.rs:126` `gitea_webhook` | `POST {portal}/api/v1/webhooks/gitea` with `X-Gitea-Event`, `X-Gitea-Signature` (hex HMAC-SHA256 of the body with `JC_GITEA_WEBHOOK_SECRET`) | signature (constant time), then `action == "closed" && pull_request.merged` or a push to `repository.default_branch` | served: merges, contents commits and pushes deliver; the Portal answers 202 for merged pulls | `merge_requests_reviews_merge_and_webhooks`, `a_webhook_signed_with_another_secret_is_refused_by_the_portal` |
| 38 | `api/sync_sources.rs:327` `webhook` | `POST {portal}/api/v1/webhooks/sync/{project}/{name}`, same header, the source's own secret | signature | served by the same delivery code (any hook URL) | covered by row 37's delivery |
| 39 | hook registration (not the Portal: the deployment's bootstrap Job, now `jc-portal-integrate`'s first start) | `POST /api/v1/repos/{o}/{r}/hooks` `{type: "gitea", config: {url, content_type: "json", secret}, events, active}` | — | served in Gitea's shape (**fixed in this run**: the native `{url, secret_ref}` shape was the only one); a hook without a secret, a non-JSON or non-POST hook, another `type` and a `branch_filter` other than empty or `*` are 422 | `merge_requests_reviews_merge_and_webhooks`, `an_unsigned_hook_is_refused_and_a_lost_secret_delivers_nothing` |

Delivery fixes in this run: a hook whose stored secret is gone is not delivered (it was signed
with an empty key); a receiver's 4xx is not retried (a refused signature was sent three times).

## SyncSource origins (`sync/remote.rs`)

| # | Portal side | Call | Status | Test |
|---|---|---|---|---|
| 40 | `git_revision` `sync/remote.rs:101` | `GET {url}.git/info/refs?service=git-upload-pack`, anonymous | served to a signed-in caller only | `a_sync_source_origin_serves_the_advertisement_and_the_page_archive` |
| 41 | `checkout` + `archive_url` `sync/remote.rs:269` | `GET {repo}/archive/{rev}.zip` (the page path, not `/api/v1`), anonymous; one root directory stripped | page path served (**fixed in this run**, only `/api/v1/repos/.../archive/` existed), signed-in caller only | same |

**Gap 1 (needs a decision):** `HttpRemote` refuses an origin with a credential
(`credential_free`, `sync/remote.rs:195`) and fetches anonymously, while shako denies every
anonymous request, `private: false` repositories included (`LocalAuthorizer::check`,
`core/src/auth.rs:393`). A SyncSource whose origin is a shako repository therefore gets 401.
Opening anonymous reads of public repositories would open a closed route, so it is left for the
owner: either shako serves `private: false` repositories to anonymous readers (Gitea's default
without `REQUIRE_SIGNIN_VIEW`), or the Portal resolves a `secretRef` for git origins. Two calls
show it: `GET /git/{o}/{r}.git/info/refs?service=git-upload-pack` and
`GET /git/{o}/{r}/archive/{sha}.zip` without `Authorization` answer 401.

## Actions and App builds

| # | Portal method | Call | Main callers | Status | Test |
|---|---|---|---|---|---|
| 42 | `latest_runs` :1283 | `GET .../actions/runs?limit=` | `api/app_build.rs:174,304`, `api/pipelines.rs:834` | **gap**: 404 Gitea JSON | `actions_calls_answer_not_found_until_runners_land` |
| 43 | `dispatch_workflow` :1321 | `POST .../actions/workflows/{file}/dispatches` `{ref}` | `api/app_build.rs:251` | **gap**: 404 | same |
| 44 | `queued_jobs` :1337 | `GET /api/v1/orgs/{o}/actions/jobs?status=queued&limit=50` | `apps/build_pods.rs:363` | **gap**: 404 | same |
| 45 | `repository_registration_token` :1369 | `POST .../actions/runners/registration-token` | `apps/build_pods.rs:418` | **gap**: 404 | same |
| 46 | `artifacts_named` / `download_artifact` :1384 / :1412 | `GET .../actions/artifacts?name=`, `GET .../actions/artifacts/{id}/zip` (302 to a signed path) | `apps/built.rs:213,262,336` | **gap**: 404 | same |

**Gap 2 (App builds, S16/S17, contract D3):** the Portal reaches the Gitea runner protocol only
indirectly: `apps/build_pods.rs:230` starts an `act_runner` pod with `GITEA_INSTANCE` set to
`JC_GITEA_URL` and the token from row 45, and that runner speaks Gitea's connect-RPC runner
protocol (`/api/actions/runner.v1.RunnerService/*`). Neither the protocol nor rows 42-46 exist
in shako, so a generated App does not build. Every row answers 404 with Gitea's error JSON,
which the Portal reads as `GitError::NotFound` (no runs, no artifact) rather than a parse error.

## What `jc` has to wire (for `jc-portal-integrate`)

- `JC_GITEA_URL` = shako's mount (`https://{host}/git/` in slug mode); the registry calls of row
  34 go to `{JC_GITEA_URL}/v2/...`, which shako's main router serves.
- The tokens behind `JC_GITEA_TOKEN` and `JC_GITEA_APPS_TOKEN` need RoleBindings with `Admin`,
  `Write`, `Approve` and `Publish` in the owners' organizations, and each owner
  (`JC_GITEA_OWNER`, `JC_GITEA_APPS_OWNER`) registered with `ShakoState::register_owner_org`;
  `bins/jc/src/modules/shako.rs` binds one `admin` token in one organization today.
- A hook per configuration repository (row 39) pointing at the Portal's
  `/api/v1/webhooks/gitea` with `JC_GITEA_WEBHOOK_SECRET`. In one process the Portal is on the
  same host: shako's `OutboundGuard` refuses loopback and private addresses unless the Portal's
  host is in `OutboundPolicy::in_cluster_hosts` (`bins/jc` passes `OutboundGuard::default()`).
