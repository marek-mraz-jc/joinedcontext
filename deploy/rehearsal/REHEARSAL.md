# Rehearsal environment

A rehearsal is the new version (`jc --role all` plus PostgreSQL in docker) brought up on one
environment's config and files from the deployment repo (dev, or prod = smartcity) with synthetic
test data. It writes nowhere outside itself and copies no data from dev or prod.

## 1. Inputs

- No data from dev or prod (owner, 2026-11-08: the data migration is stopped). No Antares entities,
  no temporal instances and no export bundle from either environment.
- The only input: a synthetic bundle of the same shape made from the deployment repo's
  config as code (realm, seeds, CkanInstances, Grafana provisioning):

```sh
python3 -I rehearsal.py render --deployment <joinedcontext-deployment clone> --env dev --out <render>/dev
python3 -I rehearsal.py synth  --render <render>/dev --env dev --out $REHEARSAL_HOME/dev/export
```

`--env smartcity` gives prod's config.

## 2. Rebuild

```sh
export REHEARSAL_HOME=~/rehearsal JC_BIN=<target>/debug/jc
./up.sh dev 42101 55441
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

## 3. The 7-day rule

- Import: a temporal instance observed before `now - 7 days` is refused; the report holds the
  cut-off (`cutoff`) and every refused instance.
- Rolling: Antares deletes history older than 7 days at start and every `REHEARSAL_SWEEP_SECS`
  (default 3600) through its own retention (`ANTARES_TEMPORAL_RETENTION_DAYS=7`); current entity
  state is never deleted. `rehearsal.py retain` deletes the Portal's dated rows (activity, run logs,
  pipeline log, health history, notifications and the rest of `PORTAL_DATED`) older than 7 days at
  start and hourly (`retain.log`).
- ekiben run state lives in memory and ends with the process.

## 4. Tests

```sh
python3 -m pytest deploy/rehearsal/tests                    # unit
REHEARSAL_LIVE=1 python3 -m pytest deploy/rehearsal/tests   # + a live env `rt` on 42119 / 55449
```

The live tests insert 8-day and 6-day rows and instances and check that only the 8-day ones go and
that current state stays.
