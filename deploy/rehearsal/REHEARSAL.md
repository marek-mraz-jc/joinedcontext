# Rehearsal environment

A rehearsal is the new version (`jc --role all` plus PostgreSQL in docker) brought up on one
environment's config and files (dev, or prod = smartcity) and the last day of its Antares history.
It writes nowhere outside itself; the export from dev and prod is read only.

## 1. Inputs

- The deployers' export bundle, read only, at `/workspace/evidence/jc-rehearsal/export/{dev,prod}/`
  (format: EXPORT.md): realm config without people, repository files, entities' current state,
  temporal instances of the last 24 hours, dashboards.
- Until it lands: a synthetic bundle of the same shape made from the deployment repo's config as
  code (realm, seeds, CkanInstances, Grafana provisioning):

```sh
python3 -I rehearsal.py render --deployment <joinedcontext-deployment clone> --env dev --out <render>/dev
python3 -I rehearsal.py synth  --render <render>/dev --env dev --out $REHEARSAL_HOME/dev/export
```

`--env smartcity` gives prod's config.

## 2. Rebuild

```sh
export REHEARSAL_HOME=~/rehearsal JC_BIN=<target>/debug/jc
./import.sh dev 42101 55441 /workspace/evidence/jc-rehearsal/export/dev
python3 -I rehearsal.py smoke  --base http://localhost:42101 --state $REHEARSAL_HOME/dev/state \
  --export <export> --report $REHEARSAL_HOME/dev/smoke.json --realms $REHEARSAL_HOME/dev/state/realms \
  --pg-port 55441 --antares-password-file $REHEARSAL_HOME/dev/secrets/antares-pw
python3 shots.py --base http://localhost:42101 --state $REHEARSAL_HOME/dev/state --out <shots>/dev
```

Dashboards: dev's and prod's Grafana load them from the deployment's files, not from a database,
so a bundle holds none. Render the deployment at the environment's ref (main for dev, release for
prod; `just _dev-assemble` first) and import its Grafana files: each project's folder, datasources
and dashboard into its own daiya organization (as `provision.sh` does), every datasource on the
rehearsal's own gateway for the same Endpoint slug, the platform models in the main organization.

```sh
python3 -I rehearsal.py dashboards --render <render>/dev --out <dashboards>/dev
python3 -I rehearsal.py import --export <export> --base http://localhost:42101 --state $REHEARSAL_HOME/dev/state \
  --report $REHEARSAL_HOME/dev/dashboards-report.json --only daiya --dashboards <dashboards>/dev
python3 -I rehearsal.py portal-dashboards --base http://localhost:42101 --state $REHEARSAL_HOME/dev/state \
  --dashboards <dashboards>/dev --report $REHEARSAL_HOME/dev/portal-dashboards.json
```

smartcity (prod) deploys no `grafana` component: its rehearsal takes the 7 platform files of
`components/grafana/charts/dashboards/files/` on release, for comparison only.

prod: the same with `prod 42102 55442`. A clean rebuild removes `$REHEARSAL_HOME/<env>` first
(the `rh-pg-<env>` container too: `docker rm -f rh-pg-<env>`).

- `import.sh` is the import, scripted. jc's broker runs the `joinedcontext` policy, which answers
  401 to the import's writes (they carry no subject). So `import.sh` starts jc with
  `ANTARES_POLICY=allow-all` on a loopback bind only (up.sh refuses allow-all on any other bind),
  imports, restarts jc on its default policy and runs `rehearsal.py policy-check`: an anonymous
  write must answer 401 and `/q/health` must name the `joinedcontext` engine, or jc is stopped
  and the script fails. A plain `up.sh` always starts on the default policy.
- `up.sh` starts PostgreSQL (PostGIS image by digest), makes one role per store with a random
  password file under `secrets/` (0600, never printed or passed in argv), runs the kaisatsu
  migrations, writes the export's realm without people into the kaisatsu import dir, and starts
  `jc`. Each `$(env:NAME)` secret of the realm gets a fresh random value in
  `secrets/realm/NAME` (0600), and the realm file names it as `$(file:...)`: kaisatsu inside jc
  reads no environment, only `--kaisatsu-import-secrets-dir`. It stops a running `jc` of the same
  environment first (`stop-jc.sh`: SIGTERM, then SIGKILL after 10 s).
- kaisatsu, the Portal, Antares, shako, daiya, dataroom and ekiben keep their state in PostgreSQL
  (one database each, request roles without BYPASSRLS). Each module's `[<m>.database]` in
  `jc.toml` names role, host and database inline; jc refuses a password in a URL, and every
  password stays in its 0600 file under `secrets/`. The Portal still reads its URL with the
  password from `JC_PORTAL_DATABASE_URL` (no file form in jc yet). The import is idempotent
  (a rerun reports `unchanged`).
- `import` writes only to a loopback base and only from a settled bundle (no data file newer than
  `manifest.json`). It turns each CkanInstance `spec.url` to `https://catalogue.rehearsal.invalid`
  (report: `fenced`), which up.sh routes to the rehearsal's own dataroom, so the Portal never
  publishes into the source environment's catalogue.
- `realms` leaves out only what a live Keycloak export carries that kaisatsu refuses and nothing
  uses (unknown built-in mappers, unbound built-in flows, client-flow alternatives no client uses),
  each named in `state/realm-<name>-left-out.json`; anything else kaisatsu refuses stops the start.
- People (dev only, supervisor G14): the deployer's read-only `users-<realm>.jsonl` goes into
  `$REHEARSAL_HOME/people/dev/` (0700, never evidence/ or a note); `realms` refuses a directory
  others can read. Each person keeps id, attributes, groups and realm roles, gets no credential
  and must set a password (and OTP where they had one). Dev signs people in by e-mail, so only the
  personas of `demo-users.yaml` (`$REHEARSAL_HOME/demo-users.yaml`) keep e-mail and names; anyone
  else becomes `person-<id>`. The realm file is 0600 in `state/realms`; the counts are in
  `state/realm-<name>-people.json`. Prod gets no people.
- Realm: jc runs on the export's realm (`[identity] realm` in `jc.toml`: `dev` for dev, `smartcity`
  for prod; `realms` writes it to `state/realm-name.txt`). That realm keeps its own clients, so each
  start copies the realm's `portal-api`, `portal-reconciler` and `context-gateway` secrets into jc's
  state files `portal-client-secret`, `portal-admin-client-secret` and `gateway-client-secret`, and
  each `<project>-pipelines` client's secret into `secrets/realm/<project>-pipelines`, which jc's
  `--ekiben-client-secrets-dir` reads for the runner's `JC_CLIENT_SECRET_<PROJECT>`.
- Test persons: `smoke` and `shots.py` create their own (`rehearsal-*`, random passwords kept in
  memory) in that realm, in group `platform-admins` (the Portal's default bootstrap admins; added
  to an imported realm without it, as `rehearsal-tester` is).

## 3. The 24-hour rule (Antares history only)

- Import: a temporal instance observed before the later of `exportedAt - 24 h` and `now - 24 h` is refused; the report holds the
  cut-off (`cutoff`) and every refused instance. Config, files, current entity state and dashboards
  have no age cut.
- Rolling: `rehearsal.py retain` (started by up.sh, at start and every `REHEARSAL_RETAIN_SECS`,
  default 3600) deletes temporal instances older than 24 h from the rehearsal's Antares database,
  audit rows kept (`retain.log`). Antares' own retention cannot do it: it works in whole days and
  drops a weekly partition only once all of it is older. Current entity state is never deleted.

## 4. Tests

```sh
python3 -m pytest deploy/rehearsal/tests                    # unit
REHEARSAL_LIVE=1 python3 -m pytest deploy/rehearsal/tests   # + a live env `rt` on 42119 / 55449
```

The live test writes instances 25 h and 23 h old and a current state observed 25 h ago, and checks
that only the 25 h instance goes.
