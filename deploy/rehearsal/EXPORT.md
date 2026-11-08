# Rehearsal export bundle

The copy is read only on dev and prod (owner, amendment 5, 2026-11-08): nothing is written or
deleted there. Only the last day of Antares history is copied; everything else is copied whole.

Rules for every file:

- No people: no user accounts, passwords, OTP seeds, passkeys, sessions, tokens or e-mail lists.
  Service-account users (`serviceAccountClientId` set) may stay, because they carry a client's role
  bindings; give them no `credentials`.
- Antares history: only temporal instances observed in the last 24 hours (`observedAt >= exportedAt -
  24 h`). The importer refuses an older instance again on its side and writes the cut-off into its
  report. Entities: their current state, whole.
- No git history: the current files of each repository, whole. No activity rows, run logs or job logs.
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

## Next time: the broker's admin move

Antares has an admin pair made for this (ADR-0030): `GET /q/tenants/{tenant}/export` streams a
Tenant's Entities and history as stored, system attributes kept, and `POST
/q/tenants/{tenant}/import` loads that stream into an empty Tenant, all or nothing; `GET
/q/tenants/{tenant}/digest` on both sides proves the copy. An export made that way, its
`temporal` lines cut to the last 24 hours, imports through the admin token with no policy
bypass. The bundles of 2026-11-08 hold API-normalized entities, which that import does not take,
so they go through `import.sh` (REHEARSAL.md).

## Making it

- Realm: `kc.sh export --realm <name> --users skip` (or the admin API's partial export without users),
  then drop `users` except service accounts. Never `--users realm_file`.
- Gitea: per repository, `GET /api/v1/repos/{owner}/{repo}/archive/{default branch}.tar.gz`, unpacked
  into `gitea/files/<owner>/<repo>/`. No clone, no `.git`.
- Antares: per Tenant, `GET /ngsi-ld/v1/entities?type=...&limit=1000` (paged) for the current state;
  `GET /ngsi-ld/v1/temporal/entities?type=...&timerel=after&timeAt=<exportedAt - 24 h>` for history,
  one line per instance.
- Grafana: `GET /api/search?type=dash-db`, then `GET /api/dashboards/uid/{uid}` and keep `.dashboard`.

Until the bundle lands, a synthetic bundle of the same shape comes from the deployment repo's
config: `rehearsal.py render` then `rehearsal.py synth` (REHEARSAL.md).
