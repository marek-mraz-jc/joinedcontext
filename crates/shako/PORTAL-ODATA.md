# Every Portal forge call as a shako OData request (2026-11-08)

The owner's order of 2026-11-08 is "Gitea-compat + OData everywhere, NO GitHub API", then "can
it be rewritten to OData? and change names to shako". The Portal reaches shako through one client,
`ShakoClient` in `crates/portal/src/forge/shako.rs`, and that client speaks OData 4.01 to
`{JC_SHAKO_URL}/odata/` and nothing else. Git object transfer stays on git smart HTTP and container
images stay on the OCI distribution API: OData carries metadata and operations, never pack data.
shako keeps its Gitea-compatible REST (`COMPAT.md`) for outside clients only.

`PORTAL-CALLS.md` lists the same 46 calls as the old `GiteaClient` made them; the row numbers
below are its row numbers. `crates/shako/server/tests/portal_contract.rs` drives every row with
the Portal's `ShakoClient` over TCP against shako's router, with real git on disk, and delivers
webhooks into the Portal's real receiver.

## 1. Conventions

- **Service root**: `{JC_SHAKO_URL}/odata/` (`jc` mounts shako at `/git`, so
  `https://{host}/git/odata/`). Every request sends `Authorization: Bearer {JC_SHAKO_TOKEN}` (or
  `JC_SHAKO_APPS_TOKEN` for the applications' organization `JC_SHAKO_APPS_OWNER`) and
  `Accept: application/json`.
- **A repository** is addressed by its alternate key, owner and name, so the client never looks up
  a UUID first: `R` below stands for `Repositories(Owner='{owner}',Name='{repo}')`. The UUID key
  `Repositories('{id}')` names the same entity. A quote inside a key or parameter value is doubled
  (`''`), the OData rule.
- **Things inside a repository** are navigation under `R`: `R/Branches('{name}')`,
  `R/Commits('{sha}')`, `R/PullRequests({number})`, `R/PullRequests({number})/Files`. A branch name
  with `/` is one key value (`R/Branches('workspace/air-v2')`), percent-encoded by the client.
- **Function parameters** go as parameter aliases in the query string, so a file path or ref with
  `/` never splits a path segment: `R/Shako.File(Path=@p,Ref=@r)?@p='projects/a.yaml'&@r='main'`.
- **Actions** are `POST` with a JSON body whose members are the action's parameters
  (PascalCase, as `$metadata` declares them).
- **Answers**: an entity is a JSON object of its declared properties; a collection is
  `{"value": [...]}` with `@odata.count` when `$count=true` was asked and `@odata.nextLink` when a
  page was cut. An error is the OData error object `{"error": {"code", "message"}}` with the HTTP
  status the Portal maps: 404 absent or hidden (another organization's repository is 404, never
  403), 409 conflict (stale blob sha, moved head, existing repository, second open pull request for
  one head and base), 401 no or bad token, 403 a protected branch or a missing verb on a visible
  repository, 422 a refused body.

## 2. Repositories

| # | `ShakoClient` method | OData request | Answer the client reads |
|---|---|---|---|
| 1 | `ensure_repository` | `GET R`; when 404, `POST CreateRepository` `{Owner, Name, Description, Private: true, AutoInit: true, DefaultBranch: "main"}`; 409 = exists | status |
| 2 | `create_empty_repository` | same with `AutoInit: false` | status |
| 3 | `migrate_repository` | `GET R`; when 404, `POST ImportRepository` `{SourceOwner, SourceName, RepoOwner, RepoName, Private: true}`; 409 = exists | status |
| 4 | `default_branch` | `GET R` | `DefaultBranch` |
| 5 | `archive_repository` | `POST R/Shako.Archive` | status |
| 6 | `delete_repository` | `DELETE R`; 404 = done | status (204) |
| 7 | `let_the_reader_in` | removed: shako grants reads through RoleBindings, so the Context Gateway's reader needs a RoleBinding, not a collaborator row (shako answered the Gitea call as a no-op) | — |
| 8 | `protect_branch` | `POST R/Shako.Protect` `{Rule: {rule_name, enable_push: false}}` | status; a later contents write to the branch is 403 |

## 3. Branches, trees, contents, history

| # | `ShakoClient` method | OData request | Answer the client reads |
|---|---|---|---|
| 9 | `branch_head` | `GET R/Branches('{branch}')` | `HeadSha` |
| 10 | `list_tree_blobs` / `list_tree` | `GET R/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=1000)?@r='{ref}'&@s={n}` | `Sha`, `Tree[].{Path,Type,Sha}`, `Truncated` |
| 11 | `commit_state` | `GET R/Shako.CommitStatus(Sha=@s)?@s='{sha}'` | `State`, `TotalCount` |
| 12 | `create_branch` | `POST R/Shako.CreateBranch` `{NewBranchName, OldBranchName}` | status |
| 13 | `delete_branch` | `POST R/Shako.DeleteBranch` `{Branch}`; 404 = done | status |
| 14 | `get_file` | `GET R/Shako.File(Path=@p,Ref=@r)?@p='{path}'&@r='{ref}'`; 404 = absent | `Sha`, `Content` (base64) |
| 15 | `put_file` | `POST R/Shako.CommitFiles` `{Branch, Message, Author, Committer, Files: [{operation: create\|update, path, content, sha?}]}` | `Sha`; a stale `sha` is 409 |
| 16 | `change_files` / `update_files` / `write_files` | `POST R/Shako.CommitFiles` with `Files[]` of `upload`, `create`, `update`, `delete` | `Sha`; all or nothing, one stale `sha` is 409 and the branch does not move |
| 17 | `delete_file` | `POST R/Shako.CommitFiles` `{Files: [{operation: delete, path, sha}]}` | `Sha` |
| 18 | `list_commits` | `GET R/Shako.History(Ref=@r,Path=@p,Top={n})?@r='{ref}'&@p='{path}'` | `value[].{Sha, Message, Author, Email, Date}` |
| 19 | `commit_message` | `GET R/Commits('{sha}')` | `Message` (whole message) |

`CommitFiles` takes `Author` and `Committer` as `{Name, Email}`: commits are attributed to the
signed-in person while the token only authenticates the call (CC-44). A commit emits the `push`
event, so the Portal's webhook fires, and runs the repository's push mirrors.

## 4. Pull requests (proposals, approvals, rejections)

| # | `ShakoClient` method | OData request | Answer the client reads |
|---|---|---|---|
| 20 | `create_pull_request` | `POST R/Shako.OpenPullRequest` `{Head, Base, Title, Body}` | a `PullRequest` (below); a second open one for the same head and base is 409 |
| 21 | `list_pull_requests[_page]` | `GET R/PullRequests?$filter=State eq '{state}'&$orderby=UpdatedAt desc&$top={limit}&$skip={offset}` (`all` sends no `$filter`) | `value[]` of `PullRequest` |
| 22 | `pull_request` | `GET R/PullRequests({n})` (mergeability refreshed while open) | `PullRequest` |
| 23 | `pull_request_files` | `GET R/PullRequests({n})/Files` | `value[].{Filename, Status}` (`added`, `modified`, `deleted`, `renamed`) |
| 24 | `review` | `POST R/PullRequests({n})/Shako.Review` `{Event: COMMENT, Body}` | status |
| 25 | `review_bodies` | `GET R/PullRequests({n})/Reviews` | `value[].Body` |
| 26 | `close_pull_request` | `POST R/PullRequests({n})/Shako.Close` | status |
| 27 | `update_pull_request` | `POST R/PullRequests({n})/Shako.UpdateBranch` | status (200 when already up to date) |
| 28 | `merge` | `POST R/PullRequests({n})/Shako.Merge` `{Style: merge\|squash, Message, HeadCommitId?}` | status; a moved head is 409 |

`PullRequest` properties the client reads: `Number`, `HtmlUrl`, `State`, `Title`, `Body`, `Head`,
`HeadSha`, `Base`, `BaseSha`, `MergeBase`, `Mergeable`, `Merged`, `MergeCommitSha`, `Author`
(login), `AuthorName`, `AuthorEmail`, `CreatedAt`, `UpdatedAt`, `ClosedAt`. shako has no forks: a
pull request belongs to one repository, so head and base are always the same repository and the
client sets `same_repository` from that.

## 5. Copies, transfers, exports

| # | `ShakoClient` method | Request | Note |
|---|---|---|---|
| 29 | `bundle` | `GET R/Shako.BundleLink(Ref=@r)?@r='{ref}'` answers `{"value": "<url>"}`; then `GET <url>` for the bytes | the link is shako's archive download; bytes are not OData |
| 30 | `push_bundle` | `POST {JC_SHAKO_URL}/{owner}/{repo}.git/git-receive-pack` | git smart HTTP, unchanged |
| 31 | `list_tags` | `GET R/Tags?$top=50&$skip={offset}` | `value[].{Name, TargetSha}` |

## 6. Applications' organization: mirrors, packages, images

| # | `ShakoClient` method | Request | Answer the client reads |
|---|---|---|---|
| 32 | `push_mirrors` | `GET R/PushMirrors` | `value[].{Name, RemoteAddress, LastError}` |
| 32 | `add_push_mirror` | `POST R/Shako.AddPushMirror` `{RemoteAddress, RemoteUsername, RemotePassword, SyncOnCommit: true, Interval: "8h0m0s"}` | status; the password goes into shako's secret store and is never answered |
| 32 | `delete_push_mirror` | `POST R/Shako.RemovePushMirror` `{Name}` | status |
| 32 | `sync_push_mirrors` | `POST R/Shako.SyncPushMirrors` | status |
| 33 | `put_generic_file` | `POST PublishPackage` `{Owner, Package, Version, File, ContentBase64}`; overwrite is 409 | status |
| 33 | `get_generic_file` | `GET PackageFiles(Owner='{o}',Name='{p}',Version='{v}',File='{f}')/$value` | bytes (capped by the client) |
| 34 | `push_image` | `GET {JC_SHAKO_URL}/v2/token?...`, blob `HEAD`/`POST`, manifest `PUT` | the OCI distribution API, unchanged |

## 7. Teams and permissions

| # | Old call | Now |
|---|---|---|
| 35 | `owns_organization` | removed with the team wave: shako answered `is_owner: false` always, so the reconciler always took the `LeftToTheBootstrap` branch. Access to a shako repository follows RoleBindings; the read-only `Teams` entity set shows them. |
| 36 | `org_teams`, `create_team`, `delete_team`, `team_repositories`, `set_team_repository` | removed (never reached, see row 35) |

## 8. Webhooks (shako to Portal)

| # | Side | Request |
|---|---|---|
| 37 | Portal `api/webhook.rs` `shako_webhook` | `POST {portal}/api/v1/webhooks/shako` with `X-Shako-Event` (`push`, `pull_request`), `X-Shako-Delivery`, `X-Shako-Signature` (lowercase hex HMAC-SHA256 of the body with `JC_SHAKO_WEBHOOK_SECRET`), and `X-Shako-Signature-Previous` while a previous secret is set on the hook. The Portal checks the signature in constant time against `JC_SHAKO_WEBHOOK_SECRET` and `JC_SHAKO_WEBHOOK_SECRET_PREVIOUS` before it parses the body. |
| 38 | Portal `api/sync_sources.rs` | a SyncSource origin's own secret; the receiver takes `X-Shako-Signature` beside the outside forges' headers |
| 39 | registration (`jc` first start) | `POST R/Shako.AddWebhook` `{Url, Secret}` (or `{Url, SecretRef}` naming a secret already in shako's store; `PreviousSecretRef` during a rotation). A hook made this way delivers shako's native event shape below; a hook made through the Gitea route keeps Gitea's headers and body for outside clients. |

Native event body (`push`):

```json
{"event": "push", "ref": "refs/heads/main", "before": "", "after": "<sha>",
 "repository": {"owner": "joinedcontext", "name": "configuration", "defaultBranch": "main", "private": true},
 "sender": "jc-portal"}
```

Native event body (`pull_request`):

```json
{"event": "pull_request", "action": "closed",
 "pullRequest": {"number": 3, "state": "closed", "title": "...", "merged": true, "mergeCommitSha": "<sha>",
                 "head": {"ref": "portal/x", "sha": "<sha>"}, "base": {"ref": "main", "sha": "<sha>"}},
 "repository": {"owner": "joinedcontext", "name": "configuration", "defaultBranch": "main", "private": true},
 "sender": "jc-portal"}
```

The Portal synchronizes on a merged pull request and on a push to the repository's default branch.

## 9. SyncSource origins (`sync/remote.rs`)

| # | Request | Note |
|---|---|---|
| 40 | `GET {url}.git/info/refs?service=git-upload-pack` | git smart HTTP to any origin, not the forge client |
| 41 | `GET {repo}/archive/{rev}.zip` | the page archive of any origin; shako serves it too |

Gap 1 of `PORTAL-CALLS.md` (anonymous reads of a shako origin) is unchanged and waits for the
owner.

## 10. Actions and App builds

| # | `ShakoClient` method | OData request | Status |
|---|---|---|---|
| 42 | `latest_runs` | `GET R/WorkflowRuns?$orderby=Number desc&$top={limit}` | **gap**: 404 unknown entity set until S16; the client reads 404 as no runs |
| 43 | `dispatch_workflow` | `POST R/Shako.DispatchWorkflow` `{File, Ref}` | **gap**: 404 |
| 44 | `queued_jobs` | `GET Jobs?$filter=Owner eq '{owner}' and Status eq 'queued'&$top=50` | **gap**: 404 |
| 45 | `repository_registration_token` | `POST R/Shako.RunnerRegistrationToken` | **gap**: 404 |
| 46 | `artifacts_named` / `download_artifact` | `GET R/Artifacts?$filter=Name eq '{name}'`, `GET R/Artifacts({id})/$value` | **gap**: 404 |

These five keep `PORTAL-CALLS.md` gap 2 (S16/S17): the requests are the shape the Actions entity
sets will take, and shako answers each with 404 until they exist, which the Portal reads as
"nothing there" exactly as it read Gitea's 404.
