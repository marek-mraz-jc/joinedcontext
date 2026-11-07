# External suites

Two conformance suites Antares did not write, run against the vanilla and the
all-features binary on every store (T-3549). The workflow is
`.github/workflows/external-suites.yml`; it runs on GitHub-hosted runners
(dispatch, weekly, and a smoke preset on a push that touches this folder),
never on a shared host. The triage of what fails is
`docs/conformance/final-validation.md`.

## Coraine (`coraine/`, Apache-2.0, see `coraine/NOTICE`)

`run.sh <coraine-dir> <corlibs-bin> <store> <out-dir>` runs
SEAMWARE/coraine `test/funcTests/cases` with corTest. corTest sources
`antares.sh` (as `COR_TEST_FUNCTIONS`), which keeps every Coraine helper and
replaces the broker lifecycle:

| Coraine | Antares |
|---|---|
| `coraineStart [-role R] [flags]` | `$ANTARES_BIN` on the role's port with `ANTARES_STORE`, a data directory per role (`ANTARES_TEST_DIR/R`), a database per role (`antares_r` under `ANTARES_TEST_PG`) |
| flags `-dist`, `--troe*`, `--noSplitEntities`, `-v`, sizing | ignored: Antares behaves so by default |
| any other flag (`--bridges`, `--apiPlugins`, `--dbDir`, ...) | the start fails with `CORAINE-ONLY <flag>`; the case is not applicable |
| `coraineStop [-role R] [-all]` | SIGTERM, then SIGKILL after 10 s |
| `corDbDrop [-role R] [-tenant T] [-db D]` | the role's store emptied (every tenant: Antares keeps them in one store); a running broker is restarted on the empty store |
| `corTroeInit`, `corTroeDrop` | nothing: history lives in the role's store |

`compare.py` gives each case one verdict: `pass` (corTest's byte diff is
empty), `pass-normalised` (equal once Date, Content-Length, the other
headers, ProblemDetails `detail`/`title` wording and array order are set
aside), `fail`, `error`, `not-applicable` (`not-applicable.txt` by tag or
name, per store where it says so, or a Coraine-only flag), `skipped`.

## ETSI (`etsi/`)

The job runs `dev/etsi-pipeline.sh` twice per cell: on the suite this
repository carries (a) and on forge's default branch of the day (b).
`forge-free.sh` rewrites every raw forge or GitHub URL of a suite file to a
local server of the suite root, so neither outage fails a run;
`https://uri.etsi.org` core contexts and the `easy-global-market` test
contexts the suite names stay remote, as in every other ETSI job here. In (b)
the pipeline still overlays `dev/MqttUtils.resource` (the mosquitto host of
the runner). `tp-diff.py` lists every TP whose status differs between (a) and
(b).

## Self-check

    python3 -m unittest discover -s tests/external/coraine
    python3 -m unittest discover -s tests/external/etsi

No suite runs in the self-check: a stub stands in for Coraine's helpers and a
script for the broker.
