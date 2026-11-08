# Rehearsal export bundle

**Stopped (owner, 2026-11-08): no data is copied from dev or prod.** No bundle is requested from the
deployers and none exists. The rehearsal runs on config and files from the deployment repo plus
synthetic test data (`rehearsal.py synth`), which uses this layout. The rest of this page describes
the format only.

The deployer (dev) and the prod-deployer (prod) write one bundle per environment, read only, to
`/workspace/evidence/jc-rehearsal/export/{dev,prod}/`. `rehearsal.py import` reads it into a local
`jc`; nothing in the bundle is written back anywhere.

Rules for every file:

- No people: no user accounts, passwords, OTP seeds, passkeys, sessions, tokens or e-mail lists.
  Service-account users (`serviceAccountClientId` set) may stay, because they carry a client's role
  bindings; give them no `credentials`.
- Nothing older than 7 days: no git history (current files only), no temporal instance observed before
  `exportedAt - 7 days`, no activity rows, run logs or job logs. The importer refuses an older instance
  again on its side and writes the cut-off into its report.
- No secret values: client secrets stay `$(env:NAME)` placeholders, as keycloak-config-cli holds them.
  The rehearsal makes a new value for each.

## Layout

```text
manifest.json                       {"env": "dev", "exportedAt": "2026-11-08T12:00:00Z", "source": "..."}
keycloak/realm-<name>.json          the realm representation without people (master is skipped)
gitea/repos.json                    [{"owner", "name", "defaultBranch", "private", "description"}]
gitea/files/<owner>/<repo>/<path>   the files of the default branch head, byte for byte
antares/entities.jsonl              {"tenant": "<project>-<space>", "entity": {<normalized NGSI-LD entity>}}
antares/temporal.jsonl              {"tenant", "id", "type", "attr", "instance": {"type", "value", "observedAt", ...}}
grafana/dashboards/<file>.json      a dashboard model, or <project>.folder.json, <project>.datasource.<name>.json,
                                    <project>.dashboard.json (an /api/dashboards/db body) as the deployment provisions them
```

## Making it

- Realm: `kc.sh export --realm <name> --users skip` (or the admin API's partial export without users),
  then drop `users` except service accounts. Never `--users realm_file`.
- Gitea: per repository, `GET /api/v1/repos/{owner}/{repo}/archive/{default branch}.tar.gz`, unpacked
  into `gitea/files/<owner>/<repo>/`. No clone, no `.git`.
- Antares: per Tenant, `GET /ngsi-ld/v1/entities?type=...&limit=1000` (paged) for the current state;
  `GET /ngsi-ld/v1/temporal/entities?type=...&timerel=after&timeAt=<exportedAt - 7 days>` for history,
  one line per instance.
- Grafana: `GET /api/search?type=dash-db`, then `GET /api/dashboards/uid/{uid}` and keep `.dashboard`.

A synthetic bundle of the same shape comes from the deployment repo's config:
`rehearsal.py render` then `rehearsal.py synth` (REHEARSAL.md).
