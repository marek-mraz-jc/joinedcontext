# Performance baseline

The `perf-baseline` workflow builds the broker twice in one job, a release
binary with overflow checks off and one with them on, and runs the same
shapes (`dev/perf/baseline.sh`: `shapes.sh` at 1 000 and 100 000 entities,
`fire.sh` for subscription fan-out) against each on the memory and the
PostgreSQL store. The two builds share the runner, the dataset and the
hour, so their ratio is the cost of the checks. The absolute numbers are a
shared GitHub `ubuntu-latest` runner's, not a capacity figure: the
[Performance](performance.md) chapter's dedicated-vCPU runs are those. The
job summary of each run names its commit and its CPU.

## 1. The run

| run | commit | runner | artifact |
|---|---|---|---|
| 38135751509 | `f38a43a` (branch, T-3735) | GitHub `ubuntu-latest` | `perf-baseline` |

The run and its artifact are gone: on 2026-10-15 the Actions API answers 404
for it and lists no `perf-baseline` run at all. The tables below are the
numbers its job summary printed. A new run of the workflow on `f38a43a`
measures them again.

Concurrency 50 for every shape; p99 in milliseconds. Each store is
seeded once before its rows, PostgreSQL is vacuumed, analysed and
checkpointed after the seed, and a row measures its shape alone
(`dev/perf/shapes.sh`, T-3735). The tables of run 38078053835 that stood
here counted every row's re-seed as the shape (memory query at 1 000
entities 548 req/s there, 6 122 here) and are withdrawn.

## 2. 1 000 entities, run 38135751509

| store | shape | checks off req/s | checks on req/s | change | off p99 | on p99 |
|---|---|---|---|---|---|---|
| memory | query | 6122 | 6176 | +0.9 % | 28.61 | 27.50 |
| memory | query-q | 3971 | 3893 | −2.0 % | 29.75 | 30.46 |
| memory | retrieve | 15933 | 15200 | −4.6 % | 10.67 | 10.74 |
| memory | upsert | 848 | 881 | +3.9 % | 67.96 | 66.69 |
| memory | subscribe | 7125 | 7388 | +3.7 % | 26.04 | 24.36 |
| postgres | query | 1547 | 1638 | +5.9 % | 46.26 | 41.19 |
| postgres | query-q | 916 | 536 | −41.5 % | 73.18 | 123.60 |
| postgres | retrieve | 1994 | 2079 | +4.3 % | 33.65 | 31.28 |
| postgres | upsert | 205 | 222 | +8.3 % | 333.54 | 294.92 |
| postgres | subscribe | 565 | 586 | +3.7 % | 112.11 | 99.70 |

## 3. 100 000 entities, run 38135751509

| store | shape | checks off req/s | checks on req/s | change | off p99 | on p99 |
|---|---|---|---|---|---|---|
| memory | query | 6160 | 6256 | +1.6 % | 27.19 | 27.53 |
| memory | query-q | 3967 | 3984 | +0.4 % | 28.56 | 28.07 |
| memory | retrieve | 15982 | 16358 | +2.4 % | 10.08 | 10.32 |
| postgres | query | 1580 | 1591 | +0.7 % | 43.98 | 48.94 |
| postgres | query-q | 762 | 777 | +2.0 % | 88.56 | 87.17 |
| postgres | retrieve | 2014 | 2078 | +3.2 % | 32.43 | 31.88 |

## 4. Subscription fan-out, run 38135751509

| store | subscriptions | delivered | checks off notifications/s | checks on notifications/s | off PATCH p99 | on PATCH p99 |
|---|---|---|---|---|---|---|
| memory | 1 | 10 of 10 | 1531 | 1683 | 0.8 ms | 0.6 ms |
| memory | 100 | 1000 of 1000 | 14072 | 14216 | 1.0 ms | 0.9 ms |
| memory | 10000 | 100000 of 100000 | 24429 | 24745 | 0.5 ms | 0.5 ms |
| postgres | 1 | 10 of 10 | 78 | 80 | 17.0 ms | 14.6 ms |
| postgres | 100 | 1000 of 1000 | 2566 | 2365 | 73.8 ms | 73.6 ms |
| postgres | 10000 | 100000 of 100000 | 10095 | 10574 | 16.0 ms | 15.7 ms |

## 5. Overflow checks in release

Release builds check integer overflow (`overflow-checks = true` in
`[profile.release]`, T-3552). The rule was: on, unless p99 or throughput
moves by more than 5 %. On the memory store every row of sections 2 and 3
stays within 5 % (−4.6 % to +3.9 %), and so does every PostgreSQL row at
100 000 entities (+0.7 % to +3.2 %). Three PostgreSQL rows at 1 000
entities move further: query +5.9 % and upsert +8.3 % in favour of the
checks, query-q −41.5 % against them (916 against 536 req/s). The same
query-q at 100 000 entities moves +2.0 %, so the −41.5 % is not the checks'
cost on a smaller dataset. The workflow keeps measuring the
cost: its "checks off" binary is built with
`CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=false`.

The −41.5 % is the order of the runs (T-4853). Both binaries used one
database, and the "checks on" rows at 1 000 entities ran after the "checks
off" rows at 100 000. The unseed deleted those 100 000 Vehicles, and the
plain `VACUUM` after the next seed marks their pages free without
shrinking the files: on a local repeat the `entities` table held 1 000 rows
in 75 MB and the `attr_instances` partition 262 MB with no live row.
`query` stops at its first 20 Vehicles; `query-q` (`speed>100;brand=="Tesla"`
matches about one Vehicle in 22) reads on through the empty pages to fill
its page of 20. The same binary, PostgreSQL store, 1 000 entities, before
and after a 100 000-entity seed and unseed on one database, read as query-q
over query so the busy host's own swing cancels (local runs on a shared
16-vCPU AMD EPYC Genoa sandbox, 2026-10-21, `dev/perf/shapes.sh`):

| run | query req/s | query-q req/s | query-q / query |
|---|---|---|---|
| plain `VACUUM`: fresh database | 4 213 | 2 857 | 0.68 |
| plain `VACUUM`: after the 100 000-entity seed and unseed | 1 926 | 725 | 0.38 |
| fixed rig (`VACUUM FULL` after the unseed): fresh database | 4 484 | 3 008 | 0.67 |
| fixed rig: after the 100 000-entity seed and unseed | 4 012 | 2 963 | 0.74 |
| run 38135751509, "checks off" (first) | 1 547 | 916 | 0.59 |
| run 38135751509, "checks on" (after "checks off" at 100 000) | 1 638 | 536 | 0.33 |

`shapes.sh` now runs `VACUUM (FULL, ANALYZE)` after each PostgreSQL
unseed, so a run starts from files the size of its own data.

## 6. The budget for a surface

Decided (worker-2, T-3552, owner may override): a surface mounted beside
the NGSI-LD API (`ANTARES_API_SURFACES`) keeps p99 within 10 % and
throughput at 90 % or more of the same request on the NGSI-LD API,
measured in one job as above; a translation layer adds at most 200 µs to
the median.

## Related

- [Performance](performance.md)
- [Conformance](conformance.md)
