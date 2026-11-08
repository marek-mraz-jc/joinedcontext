# shako Gitea 1.27.3 REST & Smart HTTP Compatibility Table (Contract S7, R§1.1)

Every route called by `joinedcontext-portal` (`src/git/gitea.rs`, `src/git/github_mirror.rs`,
`src/sync/remote.rs`, `src/reconciler/project_teams.rs`), `agent-proxy` (`src/routes/forge.rs`),
`jcctl checkouts` / `jc-manifest` (`src/assemble.rs`), `git-sync`, and node registry pulls, with
its caller `file:line`, status in this run, and test coverage in `crates/shako/server/tests/e2e.rs`.

| # | Method & Path | Portal / Platform Caller `file:line` | Status | Test in `crates/shako/server/tests/e2e.rs` |
|---|---|---|---|---|
| 1 | `GET /api/v1/repos/{owner}/{repo}` | `portal:src/git/gitea.rs:1039,1073,1110,1252` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 2 | `POST /api/v1/orgs/{owner}/repos` (`409` when exists) | `portal:src/git/gitea.rs:1046,1117` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 3 | `POST /api/v1/repos/migrate` (intra-forge `clone_addr` only) | `portal:src/git/gitea.rs:1081` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 4 | `POST /{owner}/{repo}[.git]/git-receive-pack` (`push_bundle`) | `portal:src/git/gitea.rs:1145` | served | `s3_s4_git_client_matrix_v0_v2_and_portal_push_bundle` |
| 5 | `GET`/`POST` `push_mirrors`, `DELETE` `push_mirrors/{name}`, `POST` `push_mirrors-sync` | `portal:src/git/gitea.rs:1200,1216,1236,1244`, `github_mirror.rs:159-177` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 6 | `GET /api/v1/repos/{owner}/{repo}/actions/runs` | `portal:src/git/gitea.rs:1283` | later (S16; answers 404 Gitea JSON) | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 7 | `POST /api/v1/repos/{owner}/{repo}/actions/workflows/{file}/dispatches` | `portal:src/git/gitea.rs:1321` | later (S16; answers 404 Gitea JSON) | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 8 | `GET /api/v1/orgs/{owner}/actions/jobs` | `portal:src/git/gitea.rs:1337` | later (S16; answers 404 Gitea JSON) | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 9 | `POST /api/v1/repos/{owner}/{repo}/actions/runners/registration-token` | `portal:src/git/gitea.rs:1369` | later (S16; answers 404 Gitea JSON) | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 10 | `GET /api/v1/repos/{owner}/{repo}/actions/artifacts` | `portal:src/git/gitea.rs:1384` | later (S16; answers 404 Gitea JSON) | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 11 | `GET /api/v1/repos/{owner}/{repo}/actions/artifacts/{id}/zip` | `portal:src/git/gitea.rs:1412` | later (S16; answers 404 Gitea JSON) | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 12 | `PUT`/`GET /api/packages/{owner}/generic/{pkg}/{ver}/{file}` (`409` on overwrite) | `portal:src/git/gitea.rs:1462,1484` | served | `s8_s9_s13_s18_s19_s21_odata_registry_mcp_web_and_security_threats` |
| 13 | `GET`/`POST /v2/token` | `portal:src/git/gitea.rs:1507`, `dep:apisix-routes.yaml:45` | served | `s8_s9_s13_s18_s19_s21_odata_registry_mcp_web_and_security_threats` |
| 14 | `HEAD`/`GET` `/v2/{owner}/{image}/blobs/{d}`, `POST` `blobs/uploads/`, `PUT`/`GET`/`HEAD` `manifests/{tag}` | `portal:src/git/gitea.rs:1541` | served | `s8_s9_s13_s18_s19_s21_odata_registry_mcp_web_and_security_threats` |
| 15 | `DELETE /api/v1/repos/{owner}/{repo}` | `portal:src/git/gitea.rs:1597` | served | `s8_s9_s13_s18_s19_s21_odata_registry_mcp_web_and_security_threats` |
| 16 | `GET /api/v1/user`, `GET /api/v1/users/{login}/orgs/{org}/permissions` | `portal:src/git/gitea.rs:1618`, `project_teams.rs:64` | served (transition no-op) | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 17 | `GET`/`POST /api/v1/orgs/{org}/teams`, `DELETE /api/v1/teams/{id}`, `GET`/`PUT`/`DELETE /api/v1/teams/{id}/repos[/{org}/{repo}]` | `portal:src/git/gitea.rs:1648-1748` | served (transition no-op) | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 18 | `PATCH /api/v1/repos/{owner}/{repo}` (`{archived: true}`) | `portal:src/git/gitea.rs:1752` | served | `s8_s9_s13_s18_s19_s21_odata_registry_mcp_web_and_security_threats` |
| 19 | `PUT /api/v1/repos/{owner}/{repo}/collaborators/{reader}` | `portal:src/git/gitea.rs:1763` | served (transition no-op) | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 20 | `GET`/`POST`/`PATCH`/`DELETE /api/v1/repos/{owner}/{repo}/branch_protections[/{name}]` | `portal:src/git/gitea.rs:1779`, `dep:job.yaml:344-356` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 21 | `GET /api/v1/repos/{owner}/{repo}/git/trees/{ref}?recursive=true&per_page=1000&page=N` | `portal:src/git/gitea.rs:1792,1810` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 22 | `GET /api/v1/repos/{owner}/{repo}/branches/{*branch}` | `portal:src/git/gitea.rs:1864` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 23 | `GET /api/v1/repos/{owner}/{repo}/commits/{sha}/status` | `portal:src/git/gitea.rs:1877` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 24 | `POST /api/v1/repos/{owner}/{repo}/branches`, `DELETE /branches/{*branch}` | `portal:src/git/gitea.rs:1896,1908`, `plat:forge.rs:199` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 25 | `GET /api/v1/repos/{owner}/{repo}/contents/{*path}?ref=` | `portal:src/git/gitea.rs:1918`, `plat:forge.rs:164` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 26 | `PUT`/`DELETE /api/v1/repos/{owner}/{repo}/contents/{*path}` | `portal:src/git/gitea.rs:1950,2080`, `plat:forge.rs:164` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 27 | `POST /api/v1/repos/{owner}/{repo}/contents` (multi-file commit, stale sha -> `409`) | `portal:src/git/gitea.rs:1980,2011,2033,2052` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 28 | `GET /api/v1/repos/{owner}/{repo}/commits?sha=&path=&limit=` | `portal:src/git/gitea.rs:2104`, `plat:forge.rs:223` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 29 | `GET /api/v1/repos/{owner}/{repo}/pulls?state=&sort=recentupdate&page=&limit=` | `portal:src/git/gitea.rs:2174,2179`, `plat:forge.rs:223` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 30 | `POST /api/v1/repos/{owner}/{repo}/pulls` | `portal:src/git/gitea.rs:2202`, `plat:forge.rs:213` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 31 | `GET /api/v1/repos/{owner}/{repo}/pulls/{n}/files?page=&limit=50` | `portal:src/git/gitea.rs:2228` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 32 | `GET /api/v1/repos/{owner}/{repo}/archive/{ref}.bundle|.zip|.tar.gz` | `portal:src/git/gitea.rs:2257`, `remote.rs:269` | served | `s3_s4_git_client_matrix_v0_v2_and_portal_push_bundle` |
| 33 | `GET /api/v1/repos/{owner}/{repo}/tags?page=&limit=50` | `portal:src/git/gitea.rs:2268` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 34 | `GET /api/v1/repos/{owner}/{repo}/pulls/{n}` | `portal:src/git/gitea.rs:2301` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 35 | `GET /api/v1/repos/{owner}/{repo}/git/commits/{sha}` | `portal:src/git/gitea.rs:2313` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 36 | `GET`/`POST /api/v1/repos/{owner}/{repo}/pulls/{n}/reviews` | `portal:src/git/gitea.rs:2328,2345` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 37 | `PATCH /api/v1/repos/{owner}/{repo}/pulls/{n}` (`{state:"closed"}`) | `portal:src/git/gitea.rs:2360` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 38 | `POST /api/v1/repos/{owner}/{repo}/pulls/{n}/update?style=merge` (`200` when already up to date; `409` on conflict) | `portal:src/git/gitea.rs:2377` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
| 39 | `POST /api/v1/repos/{owner}/{repo}/pulls/{n}/merge` (`405 behind the base branch`, `409` moved head/conflict) | `portal:src/git/gitea.rs:2391` | served | `s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup` |
