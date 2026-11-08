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
REHEARSAL_EXPORT=/workspace/evidence/jc-rehearsal/export/dev ./up.sh dev 42101 55441
python3 -I rehearsal.py import --export <export> --base http://localhost:42101 \
  --state $REHEARSAL_HOME/dev/state --report $REHEARSAL_HOME/dev/import-report.json --daiya-bin <target>/debug/daiya
python3 -I rehearsal.py smoke  --base http://localhost:42101 --state $REHEARSAL_HOME/dev/state \
  --export <export> --report $REHEARSAL_HOME/dev/smoke.json --realms $REHEARSAL_HOME/dev/import/realms
python3 shots.py --base http://localhost:42101 --state $REHEARSAL_HOME/dev/state --out <shots>/dev
```

prod: the same with `prod 42102 55442`. A clean rebuild removes `$REHEARSAL_HOME/<env>` first
(the `rh-pg-<env>` container too: `docker rm -f rh-pg-<env>`).

- `up.sh` starts PostgreSQL (PostGIS image by digest), makes one role per store with a random
  password file under `secrets/` (0600, never printed or passed in argv), runs the kaisatsu
  migrations, writes the export's realm without people into the kaisatsu import dir, and starts
  `jc`. Each `$(env:NAME)` secret of the realm gets a fresh random value. It restarts a running
  `jc` of the same environment.
- shako, daiya, dataroom and ekiben keep memory stores in `jc`: run `import` after every `up.sh`.
  The import is idempotent (a rerun reports `unchanged`).
- `import` writes only to a loopback base. It turns each CkanInstance `spec.url` to the
  rehearsal's own `/opendata` (report: `fenced`), so the Portal never publishes into the source
  environment's catalogue.
- Test persons: `smoke` and `shots.py` create their own (`rehearsal-*`, random passwords kept in
  memory) in realm `jc`; the imported realm gets `rehearsal-tester`.

## 3. The 24-hour rule (Antares history only)

- Import: a temporal instance observed before `exportedAt - 24 h` is refused; the report holds the
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
